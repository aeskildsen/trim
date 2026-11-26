//! Find out if a branch is already in a base branch.
//!
//! The check has three levels, from the strongest proof to the weakest:
//!
//! 1. `merged`: the branch tip is an ancestor of the base.
//! 2. `rebased`: each commit on the branch has a commit with the same patch-id
//!    on the base. Rebase merges and cherry-picks give this result.
//! 3. `squashed`: the full diff of the branch, from the merge base to the tip,
//!    has the same patch-id as one commit on the base. Squash merges give this
//!    result.
//!
//! A patch-id is a hash of a diff that ignores line numbers and whitespace, so
//! the same change gives the same id after it moves to a different parent.
//! The check can miss a branch (for example, when a merge needed a conflict
//! fix), but it does not report a branch whose change is not on the base.
//!
//! This module uses gitoxide. gitoxide has no `git patch-id`, so the module
//! calculates its own id (see [`Detector::diff_patch_id`]). The ids do not
//! agree with git, but they agree with each other, and trim only compares
//! them with each other.

use anyhow::Result;
use gix::ObjectId;
use gix::diff::blob::{Algorithm, Diff, InternedInput, sources::byte_lines};
use gix::diff::tree::{Recorder, State, recorder::Change};
use gix::objs::TreeRefIter;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    Merged,
    Rebased,
    Squashed,
}

impl How {
    pub fn as_str(self) -> &'static str {
        match self {
            How::Merged => "merged",
            How::Rebased => "rebased",
            How::Squashed => "squashed",
        }
    }
}

/// A patch-id: a 128-bit hash of a diff.
type PatchId = u128;

/// Changed file names of one commit, compared with its parent.
type Paths = Rc<BTreeSet<Vec<u8>>>;

/// Lines of context before and after each hunk that go into a patch-id, as
/// in a unified diff. Without context, two small changes in different places
/// of a file could get the same id.
const CONTEXT: u32 = 3;

/// git treats a file as binary if its first 8000 bytes contain a NUL byte.
const BINARY_PROBE: usize = 8000;

/// One changed file: its path and the blob on each side. `None` is a side
/// where the file does not exist (or is a directory).
struct FileChange {
    path: Vec<u8>,
    old: Option<(ObjectId, bool)>,
    new: Option<(ObjectId, bool)>,
}

/// Classifies branches of one repository. It keeps the changed files and the
/// patch-id of each commit, because many branches share the same range of
/// base commits.
pub struct Detector {
    repo: gix::Repository,
    state: State,
    patch_ids: HashMap<ObjectId, Option<PatchId>>,
    paths: HashMap<ObjectId, Option<Paths>>,
}

impl Detector {
    pub fn new(mut repo: gix::Repository) -> Self {
        // A cache for commits and trees helps the merge-base search and the
        // many tree diffs. The gitoxide documentation recommends one for
        // repeated merge-base calls.
        repo.object_cache_size_if_unset(64 * 1024 * 1024);
        Self {
            repo,
            state: State::default(),
            patch_ids: HashMap::new(),
            paths: HashMap::new(),
        }
    }

    /// Returns how `tip` got into `base`, or `None` if its change is not there.
    pub fn classify(&mut self, tip: ObjectId, base: ObjectId) -> Result<Option<How>> {
        let Some(merge_base) = self.repo.merge_base(tip, base)? else {
            // The histories are not related.
            return Ok(None);
        };
        let merge_base = merge_base.detach();
        if merge_base == tip {
            return Ok(Some(How::Merged));
        }

        let branch_commits = self.range(merge_base, tip)?;
        let net = self.tree_diff(Some(self.tree_of(merge_base)?), self.tree_of(tip)?)?;
        let net_paths: BTreeSet<Vec<u8>> = net.iter().map(|c| c.path.clone()).collect();
        let net_id = self.diff_patch_id(&net)?;

        // Two diffs with the same patch-id change the same files. So compute
        // the patch-id (which compares file contents) only for base commits
        // whose file names can match. This is much faster on a long history.
        // A rebased commit changes a subset of the files that the branch
        // commits change; a squash commit changes the same files as the net
        // diff of the branch.
        let mut branch_paths = BTreeSet::new();
        let mut can_be_rebased = true;
        for oid in &branch_commits {
            match self.commit_paths(*oid)? {
                Some(p) => branch_paths.extend(p.iter().cloned()),
                None => can_be_rebased = false,
            }
        }

        let mut on_base = HashSet::new();
        for oid in self.range(merge_base, base)? {
            let Some(paths) = self.commit_paths(oid)? else {
                continue;
            };
            let rebase_match = can_be_rebased && paths.is_subset(&branch_paths);
            let squash_match = *paths == net_paths;
            if (rebase_match || squash_match)
                && let Some(id) = self.commit_patch_id(oid)?
            {
                on_base.insert(id);
            }
        }
        if on_base.is_empty() {
            return Ok(None);
        }

        if can_be_rebased {
            let mut all_found = true;
            for oid in &branch_commits {
                match self.commit_patch_id(*oid)? {
                    Some(id) if on_base.contains(&id) => {}
                    _ => {
                        all_found = false;
                        break;
                    }
                }
            }
            if all_found {
                return Ok(Some(How::Rebased));
            }
        }

        if let Some(id) = net_id
            && on_base.contains(&id)
        {
            return Ok(Some(How::Squashed));
        }
        Ok(None)
    }

    /// Commits that `to` can reach and `from` cannot.
    fn range(&self, from: ObjectId, to: ObjectId) -> Result<Vec<ObjectId>> {
        let walk = self.repo.rev_walk([to]).with_hidden([from]).all()?;
        let mut out = Vec::new();
        for info in walk {
            out.push(info?.id);
        }
        Ok(out)
    }

    fn tree_of(&self, commit: ObjectId) -> Result<ObjectId> {
        Ok(self.repo.find_commit(commit)?.tree_id()?.detach())
    }

    /// The single parent of a commit: `Some(None)` for a root commit, `None`
    /// for a merge commit.
    fn parent_tree(&self, commit: ObjectId) -> Result<Option<Option<ObjectId>>> {
        let c = self.repo.find_commit(commit)?;
        let parents: Vec<ObjectId> = c.parent_ids().map(|id| id.detach()).collect();
        match parents[..] {
            [] => Ok(Some(None)),
            [p] => Ok(Some(Some(self.tree_of(p)?))),
            _ => Ok(None),
        }
    }

    /// The changed files between two trees. `None` is the empty tree. It does
    /// not detect renames, so a rename is a deletion and an addition.
    fn tree_diff(&mut self, old: Option<ObjectId>, new: ObjectId) -> Result<Vec<FileChange>> {
        let kind = self.repo.object_hash();
        let old_data = match old {
            Some(id) => self.repo.find_tree(id)?.detach().data,
            None => Vec::new(),
        };
        let new_data = self.repo.find_tree(new)?.detach().data;
        let mut recorder = Recorder::default();
        gix::diff::tree(
            TreeRefIter::from_bytes(&old_data, kind),
            TreeRefIter::from_bytes(&new_data, kind),
            &mut self.state,
            &self.repo.objects,
            &mut recorder,
        )?;

        // A directory entry is not a file. Its files come as separate changes.
        let side = |mode: gix::objs::tree::EntryMode, oid: ObjectId| {
            (!mode.is_tree()).then_some((oid, mode.is_commit()))
        };
        let mut out = Vec::new();
        for change in recorder.records {
            let (path, old, new) = match change {
                Change::Addition {
                    entry_mode,
                    oid,
                    path,
                    ..
                } => (path, None, side(entry_mode, oid)),
                Change::Deletion {
                    entry_mode,
                    oid,
                    path,
                    ..
                } => (path, side(entry_mode, oid), None),
                Change::Modification {
                    previous_entry_mode,
                    previous_oid,
                    entry_mode,
                    oid,
                    path,
                } => (
                    path,
                    side(previous_entry_mode, previous_oid),
                    side(entry_mode, oid),
                ),
            };
            if old.is_some() || new.is_some() {
                out.push(FileChange {
                    path: path.into(),
                    old,
                    new,
                });
            }
        }
        // The diff visits the trees breadth first; sort so that the same
        // changes always give the same patch-id.
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    /// The diff of a commit against its parent, or `None` for a merge commit.
    fn commit_diff(&mut self, oid: ObjectId) -> Result<Option<Vec<FileChange>>> {
        let Some(parent) = self.parent_tree(oid)? else {
            return Ok(None);
        };
        let tree = self.tree_of(oid)?;
        Ok(Some(self.tree_diff(parent, tree)?))
    }

    /// The files that a commit changes, compared with its parent. A merge
    /// commit gives `None`.
    fn commit_paths(&mut self, oid: ObjectId) -> Result<Option<Paths>> {
        if let Some(p) = self.paths.get(&oid) {
            return Ok(p.clone());
        }
        let p = self
            .commit_diff(oid)?
            .map(|diff| Rc::new(diff.into_iter().map(|c| c.path).collect()));
        self.paths.insert(oid, p.clone());
        Ok(p)
    }

    /// The patch-id of one commit against its parent. A merge commit or an
    /// empty commit has no patch-id.
    fn commit_patch_id(&mut self, oid: ObjectId) -> Result<Option<PatchId>> {
        if let Some(id) = self.patch_ids.get(&oid) {
            return Ok(*id);
        }
        let id = match self.commit_diff(oid)? {
            Some(diff) => self.diff_patch_id(&diff)?,
            None => None,
        };
        self.patch_ids.insert(oid, id);
        Ok(id)
    }

    /// A hash of the changes in `diff`. For each file, it hashes the path,
    /// then for each hunk of a line diff the removed lines, the added lines
    /// and up to three lines of context on each side. It ignores line numbers
    /// and all whitespace. For a binary file or a submodule, it hashes the
    /// two object ids.
    ///
    /// An empty diff matches every other empty diff, so it proves nothing and
    /// gives `None`.
    fn diff_patch_id(&self, diff: &[FileChange]) -> Result<Option<PatchId>> {
        if diff.is_empty() {
            return Ok(None);
        }
        let mut h = PatchHasher::new();
        for change in diff {
            h.bytes(b"file", &change.path);
            let old = self.blob(change.old)?;
            let new = self.blob(change.new)?;
            match (old, new) {
                (Blob::Text(old), Blob::Text(new)) => h.text_diff(&old, &new),
                (old, new) => {
                    h.bytes(b"old", old.opaque_id());
                    h.bytes(b"new", new.opaque_id());
                }
            }
        }
        Ok(Some(h.finish()))
    }

    fn blob(&self, side: Option<(ObjectId, bool)>) -> Result<Blob> {
        match side {
            None => Ok(Blob::Text(Vec::new())),
            Some((oid, true)) => Ok(Blob::Opaque(oid)),
            Some((oid, false)) => {
                let data = self.repo.find_blob(oid)?.detach().data;
                let probe = &data[..data.len().min(BINARY_PROBE)];
                if probe.contains(&0) {
                    Ok(Blob::Opaque(oid))
                } else {
                    Ok(Blob::Text(data))
                }
            }
        }
    }
}

/// One side of a file change, ready to hash.
enum Blob {
    Text(Vec<u8>),
    /// A binary file or a submodule commit: only its id is hashed.
    Opaque(ObjectId),
}

impl Blob {
    fn opaque_id(&self) -> &[u8] {
        match self {
            Blob::Opaque(oid) => oid.as_bytes(),
            Blob::Text(data) if data.is_empty() => b"",
            // A text side next to a binary side: the content identifies it.
            Blob::Text(data) => data,
        }
    }
}

/// Two 64-bit hashers with different start states, together a 128-bit hash.
/// The standard hasher is not stable between Rust versions, but trim only
/// compares ids from the same run.
struct PatchHasher(DefaultHasher, DefaultHasher);

impl PatchHasher {
    fn new() -> Self {
        let a = DefaultHasher::new();
        let mut b = DefaultHasher::new();
        b.write_u8(0xa5);
        Self(a, b)
    }

    /// Hashes a tag and a byte string. The length prefix keeps the parts
    /// separate, so "ab"+"c" and "a"+"bc" give different hashes.
    fn bytes(&mut self, tag: &[u8], data: &[u8]) {
        for h in [&mut self.0, &mut self.1] {
            tag.hash(h);
            data.hash(h);
        }
    }

    /// Hashes a line without its whitespace.
    fn line(&mut self, tag: &[u8], line: &[u8]) {
        let stripped: Vec<u8> = line
            .iter()
            .copied()
            .filter(|b| !b.is_ascii_whitespace())
            .collect();
        self.bytes(tag, &stripped);
    }

    fn text_diff(&mut self, old: &[u8], new: &[u8]) {
        let input = InternedInput::new(byte_lines(old), byte_lines(new));
        let diff = Diff::compute(Algorithm::Histogram, &input);
        let old_lines = &input.before;
        let new_lines = &input.after;
        for hunk in diff.hunks() {
            let before = hunk.before.start.saturating_sub(CONTEXT)..hunk.before.start;
            let after = hunk.before.end..(hunk.before.end + CONTEXT).min(old_lines.len() as u32);
            self.bytes(b"hunk", b"");
            for i in before {
                self.line(b" ", input.interner[old_lines[i as usize]]);
            }
            for i in hunk.before.clone() {
                self.line(b"-", input.interner[old_lines[i as usize]]);
            }
            for i in hunk.after.clone() {
                self.line(b"+", input.interner[new_lines[i as usize]]);
            }
            for i in after {
                self.line(b" ", input.interner[old_lines[i as usize]]);
            }
        }
    }

    fn finish(&self) -> PatchId {
        (u128::from(self.0.finish()) << 64) | u128::from(self.1.finish())
    }
}

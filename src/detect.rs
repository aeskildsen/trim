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

use anyhow::Result;
use git2::{Diff, DiffOptions, Oid, Repository};
use std::collections::{BTreeSet, HashMap, HashSet};
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

/// Classifies branches of one repository. It keeps the changed files and the
/// patch-id of each commit, because many branches share the same range of
/// base commits.
pub struct Detector {
    repo: Repository,
    patch_ids: HashMap<Oid, Option<Oid>>,
    paths: HashMap<Oid, Option<Rc<BTreeSet<Vec<u8>>>>>,
}

impl Detector {
    pub fn new(repo: Repository) -> Self {
        Self {
            repo,
            patch_ids: HashMap::new(),
            paths: HashMap::new(),
        }
    }

    /// Returns how `tip` got into `base`, or `None` if its change is not there.
    pub fn classify(&mut self, tip: Oid, base: Oid) -> Result<Option<How>> {
        // On a long history the merge base is slow to find, so find it only
        // once. If the tip is the merge base, the base contains the tip.
        let merge_base = match self.repo.merge_base(tip, base) {
            Ok(oid) => oid,
            // The histories are not related.
            Err(e) if e.code() == git2::ErrorCode::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        if merge_base == tip {
            return Ok(Some(How::Merged));
        }

        let branch_commits = self.range(merge_base, tip)?;
        // The net diff of the branch, from the merge base to the tip.
        let (net_paths, net_id) = {
            let from = self.repo.find_commit(merge_base)?.tree()?;
            let to = self.repo.find_commit(tip)?.tree()?;
            let net = self.repo.diff_tree_to_tree(Some(&from), Some(&to), None)?;
            (paths_of(&net), patch_id(&net)?)
        };

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

    /// The files that a commit changes, compared with its parent. A merge
    /// commit gives `None`.
    fn commit_paths(&mut self, oid: Oid) -> Result<Option<Rc<BTreeSet<Vec<u8>>>>> {
        if let Some(p) = self.paths.get(&oid) {
            return Ok(p.clone());
        }
        // Only the file names are necessary, so do not read file contents
        // to find binary files.
        let p = self
            .commit_diff(oid, true)?
            .map(|diff| Rc::new(paths_of(&diff)));
        self.paths.insert(oid, p.clone());
        Ok(p)
    }

    /// The diff of a commit against its parent, or `None` for a merge commit.
    fn commit_diff(&self, oid: Oid, names_only: bool) -> Result<Option<Diff<'_>>> {
        let commit = self.repo.find_commit(oid)?;
        let parent = match commit.parent_count() {
            0 => None,
            1 => Some(commit.parent(0)?.tree()?),
            _ => return Ok(None),
        };
        let mut opts = DiffOptions::new();
        opts.skip_binary_check(names_only);
        let diff =
            self.repo
                .diff_tree_to_tree(parent.as_ref(), Some(&commit.tree()?), Some(&mut opts))?;
        Ok(Some(diff))
    }

    /// Commits that `to` can reach and `from` cannot.
    fn range(&self, from: Oid, to: Oid) -> Result<Vec<Oid>> {
        let mut walk = self.repo.revwalk()?;
        walk.push(to)?;
        walk.hide(from)?;
        Ok(walk.collect::<Result<_, _>>()?)
    }

    /// The patch-id of one commit against its parent. A merge commit or an
    /// empty commit has no patch-id.
    fn commit_patch_id(&mut self, oid: Oid) -> Result<Option<Oid>> {
        if let Some(id) = self.patch_ids.get(&oid) {
            return Ok(*id);
        }
        let id = match self.commit_diff(oid, false)? {
            Some(diff) => patch_id(&diff)?,
            None => None,
        };
        self.patch_ids.insert(oid, id);
        Ok(id)
    }
}

fn patch_id(diff: &Diff) -> Result<Option<Oid>> {
    // An empty diff matches every other empty diff, so it proves nothing.
    if diff.deltas().len() == 0 {
        return Ok(None);
    }
    Ok(Some(diff.patchid(None)?))
}

fn paths_of(diff: &Diff) -> BTreeSet<Vec<u8>> {
    diff.deltas()
        .flat_map(|d| [d.old_file().path_bytes(), d.new_file().path_bytes()])
        .flatten()
        .map(<[u8]>::to_vec)
        .collect()
}

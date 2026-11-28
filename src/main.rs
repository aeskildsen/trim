//! trim: list local git branches whose change is already on the default
//! branch, also after a rebase or squash merge. With `--delete`, remove them.

mod detect;
mod output;

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use detect::{Detector, How};
use gix::ObjectId;
use gix::bstr::ByteSlice;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{SystemTime, UNIX_EPOCH};

/// The only remote that trim reads and changes.
const REMOTE: &str = "origin";

#[derive(Clone, Copy, PartialEq, ValueEnum)]
enum ColorWhen {
    /// Colour on a terminal, if the NO_COLOR variable is not set.
    Auto,
    /// Always colour, and always use the layout for a terminal.
    Always,
    /// Never colour.
    Never,
}

/// List local git branches that are already merged into the default branch,
/// also by rebase or squash merge.
///
/// Each PATH is a repository, or a directory to search for repositories.
/// trim reads only local refs; run `git fetch` first to compare with the
/// newest remote state.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Repositories, or directories that contain repositories.
    #[arg(default_value = ".")]
    paths: Vec<PathBuf>,

    /// Delete the branches that trim finds. A branch that is checked out
    /// stays.
    #[arg(short, long)]
    delete: bool,

    /// With --delete: if the branch of the worktree at PATH is stale, switch
    /// it to the base branch first, then delete the stale branch. trim does
    /// not switch a worktree with changes that are not committed.
    #[arg(short, long, requires = "delete")]
    switch: bool,

    /// Also check the branches of the remote origin, from the last fetch.
    /// With --delete, delete them on the remote. A remote branch that changed
    /// after the last fetch stays.
    #[arg(short, long)]
    remotes: bool,

    /// Compare with this branch, not with the default branch.
    #[arg(short, long, value_name = "BRANCH")]
    base: Option<String>,

    /// How many directory levels below each PATH to search for repositories.
    #[arg(long, default_value_t = 2)]
    depth: usize,

    /// When to colour the output.
    #[arg(long, alias = "colour", value_name = "WHEN", default_value = "auto")]
    color: ColorWhen,
}

struct Found {
    branch: String,
    how: How,
    time: i64,
}

struct Deleted {
    found: Found,
    /// The abbreviated last commit, to restore the branch.
    tip: String,
}

struct Kept {
    found: Found,
    /// Why `--switch` did not switch away from the branch.
    why: Option<String>,
    /// True if the branch is checked out in the worktree at the path, so
    /// that `--switch` can switch away from it.
    here: bool,
}

#[derive(Default)]
struct RepoResult {
    found: Vec<Found>,
    deleted: Vec<Deleted>,
    /// Stale branches that stay because they are checked out.
    kept: Vec<Kept>,
    /// The branch that `--switch` changed the worktree to.
    switched: Option<String>,
    /// Lines for stderr.
    errors: Vec<String>,
    failed: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let mut repos = Vec::new();
    for path in &cli.paths {
        if !path.is_dir() {
            eprintln!("trim: {}: not a directory", path.display());
            return ExitCode::from(2);
        }
        find_repos(path, cli.depth, &mut repos);
    }

    let mut results: Vec<(PathBuf, RepoResult)> = repos
        .par_iter()
        .map(|path| {
            let name = path.display().to_string();
            let result = scan(path, &name, &cli).unwrap_or_else(|e| RepoResult {
                errors: vec![format!("trim: {name}: {e:#}")],
                failed: true,
                ..Default::default()
            });
            (path.clone(), result)
        })
        .collect();
    results.sort_by(|a, b| a.0.cmp(&b.0));

    let stdout = std::io::stdout();
    let terminal = stdout.is_terminal();
    let for_person = terminal || cli.color == ColorWhen::Always;
    let painter = output::Painter {
        colour: match cli.color {
            ColorWhen::Always => true,
            ColorWhen::Never => false,
            ColorWhen::Auto => {
                terminal && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
            }
        },
    };

    let mut failed = false;
    for (path, r) in &results {
        for error in &r.errors {
            eprintln!("{error}");
        }
        // The output for a person shows these in the group of the repository.
        if !for_person {
            for k in &r.kept {
                let why = k
                    .why
                    .as_ref()
                    .map_or_else(String::new, |w| format!("; not switched: {w}"));
                eprintln!(
                    "trim: {}: {} is {}, but it is checked out; not deleted{why}",
                    path.display(),
                    k.found.branch,
                    k.found.how.as_str()
                );
            }
        }
        failed |= r.failed;
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let mut out = stdout.lock();
    let written = match (for_person, cli.delete) {
        (true, false) => output::list(&mut out, &painter, &results, now),
        (true, true) => output::deleted(&mut out, &painter, &results),
        (false, false) => print_tsv(&mut out, &results, now),
        (false, true) => results.iter().try_for_each(|(path, r)| {
            if let Some(to) = &r.switched {
                writeln!(out, "Switched to {to} in {}", path.display())?;
            }
            r.deleted.iter().try_for_each(|d| {
                writeln!(
                    out,
                    "Deleted {} in {} ({}, was {})",
                    d.found.branch,
                    path.display(),
                    d.found.how.as_str(),
                    d.tip
                )
            })
        }),
    };
    // A closed pipe (for example `trim | head`) is not an error.
    if let Err(e) = written
        && e.kind() != std::io::ErrorKind::BrokenPipe
    {
        eprintln!("trim: {e}");
        failed = true;
    }

    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Adds `path` if it is a repository. If not, searches its subdirectories,
/// down to `depth` levels. The search does not go into a repository, a
/// hidden directory or a symbolic link.
fn find_repos(path: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if path.join(".git").exists() {
        out.push(path.to_path_buf());
        return;
    }
    if depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| e.path())
        .collect();
    dirs.sort();
    for dir in dirs {
        find_repos(&dir, depth - 1, out);
    }
}

/// A branch to classify. For a remote branch, `name` has the remote prefix
/// (`origin/feat`).
struct Candidate {
    name: String,
    tip: ObjectId,
    remote: bool,
}

impl Candidate {
    /// The branch name on the remote, without the remote prefix.
    fn remote_name(&self) -> &str {
        &self.name[REMOTE.len() + 1..]
    }
}

fn scan(path: &Path, name: &str, cli: &Cli) -> Result<RepoResult> {
    let repo = gix::open(path).context("cannot open repository")?;
    let mut result = RepoResult::default();

    let mut candidates = Vec::new();
    let remote_prefix = format!("{REMOTE}/");
    let refs = repo.references()?;
    let local = refs.local_branches()?.map(|r| (r, false));
    let remote = refs.remote_branches()?.map(|r| (r, true));
    for (reference, remote) in local.chain(remote) {
        if remote && !cli.remotes {
            continue;
        }
        let reference = reference?;
        // A symbolic ref such as origin/HEAD has no object id. A name that is
        // not UTF-8 cannot go into the output, so trim does not check it.
        let Some(tip) = reference.try_id() else {
            continue;
        };
        let Ok(name) = reference.name().shorten().to_str() else {
            continue;
        };
        if !remote || name.starts_with(&remote_prefix) {
            candidates.push(Candidate {
                name: name.to_owned(),
                tip: tip.detach(),
                remote,
            });
        }
    }
    // A new repository with no commits has no branches and no default branch.
    if candidates.is_empty() {
        return Ok(result);
    }
    candidates.sort_by(|a, b| a.name.cmp(&b.name));

    let bases = bases(&repo, cli.base.as_deref())?;
    // `--switch` changes to a local base only. It does not make a branch.
    let switch_to = bases.iter().find(|b| b.local).map(|b| b.name.clone());
    // Do not report a base, or the remote copy of a local base.
    let skip: HashSet<String> = bases
        .iter()
        .flat_map(|b| {
            let remote_copy = b.local.then(|| format!("{REMOTE}/{}", b.name));
            [Some(b.name.clone()), remote_copy]
        })
        .flatten()
        .collect();
    let bases = drop_redundant_bases(&repo, bases)?;
    let mut checked_out = checked_out_branches(&repo)?;
    let head = head_branch(&repo)?;
    candidates.retain(|c| !skip.contains(&c.name));

    let mut detector = Detector::new(repo.clone());
    let mut classified = Vec::with_capacity(candidates.len());
    for c in &candidates {
        let mut how = None;
        for base in &bases {
            how = detector.classify(c.tip, base.oid)?;
            if how.is_some() {
                break;
            }
        }
        classified.push(how);
    }

    // If the branch of this worktree is stale, switch away from it before
    // the deletes, so that it can go with the other stale branches.
    let mut switch_refused = None;
    if cli.switch
        && let Some(head) = &head
        && candidates
            .iter()
            .zip(&classified)
            .any(|(c, how)| !c.remote && &c.name == head && how.is_some())
    {
        match switch_away(&repo, path, switch_to.as_deref()) {
            Ok(to) => {
                checked_out.remove(head);
                result.switched = Some(to);
            }
            Err(why) => switch_refused = Some(why),
        }
    }

    let mut to_delete_on_remote = Vec::new();
    for (c, how) in candidates.into_iter().zip(classified) {
        let Some(how) = how else { continue };
        let found = Found {
            time: repo.find_commit(c.tip)?.time()?.seconds,
            branch: c.name.clone(),
            how,
        };
        if !cli.delete {
            result.found.push(found);
        } else if c.remote {
            to_delete_on_remote.push((c.remote_name().to_owned(), c.tip, found));
        } else if checked_out.contains(&c.name) {
            let here = head.as_ref() == Some(&c.name);
            let why = if here { switch_refused.clone() } else { None };
            result.kept.push(Kept { found, why, here });
        } else {
            // The delete fails if the branch moved after trim read it.
            repo.find_reference(format!("refs/heads/{}", c.name).as_str())?
                .delete()
                .with_context(|| format!("cannot delete {}", c.name))?;
            // The tip lets the user restore the branch: git branch <name> <tip>
            result.deleted.push(Deleted {
                found,
                tip: short_id(c.tip),
            });
        }
    }

    if !to_delete_on_remote.is_empty() {
        let refs: Vec<(String, ObjectId)> = to_delete_on_remote
            .iter()
            .map(|(b, tip, _)| (b.clone(), *tip))
            .collect();
        let outcome = delete_on_remote(path, &refs)?;
        for (branch, tip, found) in to_delete_on_remote {
            match outcome.get(&branch) {
                Some(Ok(())) => result.deleted.push(Deleted {
                    found,
                    tip: short_id(tip),
                }),
                Some(Err(why)) => {
                    result.errors.push(format!(
                        "trim: {name}: {REMOTE}/{branch} not deleted: {why}"
                    ));
                    result.failed = true;
                }
                None => {
                    result.errors.push(format!(
                        "trim: {name}: {REMOTE}/{branch}: no result from git push"
                    ));
                    result.failed = true;
                }
            }
        }
    }
    Ok(result)
}

fn short_id(id: ObjectId) -> String {
    id.to_string()[..10].to_owned()
}

/// Deletes branches on the remote with one `git push`. The git CLI does the
/// push, so the user's SSH and credential configuration apply.
///
/// Each deletion has a lease on the tip from the last fetch. If the branch
/// on the remote has a different tip, the remote keeps it, because it can
/// hold commits that trim did not check.
///
/// Returns the result for each branch name.
fn delete_on_remote(
    path: &Path,
    branches: &[(String, ObjectId)],
) -> Result<HashMap<String, Result<(), String>>> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(path)
        .args(["push", "--porcelain", REMOTE]);
    for (branch, tip) in branches {
        cmd.arg(format!("--force-with-lease=refs/heads/{branch}:{tip}"));
    }
    for (branch, _) in branches {
        cmd.arg(format!(":refs/heads/{branch}"));
    }
    let out = cmd.output().context("cannot run git push")?;
    let stdout = String::from_utf8_lossy(&out.stdout);

    // Porcelain lines: <flag> TAB :<ref> TAB <summary>. The flag "-" is a
    // deleted ref; "!" is a rejected ref.
    let mut outcome = HashMap::new();
    for line in stdout.lines() {
        let mut fields = line.split('\t');
        let (Some(flag), Some(refspec), summary) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let Some(branch) = refspec.strip_prefix(":refs/heads/") else {
            continue;
        };
        let result = if flag == "-" {
            Ok(())
        } else {
            Err(summary.unwrap_or("rejected").to_owned())
        };
        outcome.insert(branch.to_owned(), result);
    }

    // If the push failed before it got to the refs (for example, no network),
    // give the git error for each branch.
    if !out.status.success() {
        let error = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        for (branch, _) in branches {
            outcome
                .entry(branch.clone())
                .or_insert_with(|| Err(error.clone()));
        }
    }
    Ok(outcome)
}

struct Base {
    name: String,
    oid: ObjectId,
    /// True for a local branch, false for a remote-tracking ref.
    local: bool,
}

/// The branches to compare with. Without `--base`, these are the local
/// default branch and `origin/<default>`. A branch counts as merged if one of
/// them contains it, because after a merge on the forge the local default
/// branch is often behind the remote.
fn bases(repo: &gix::Repository, explicit: Option<&str>) -> Result<Vec<Base>> {
    let find = |name: &str, local: bool| -> Result<Option<Base>> {
        let full = if local {
            format!("refs/heads/{name}")
        } else {
            format!("refs/remotes/{name}")
        };
        match repo.try_find_reference(full.as_str())? {
            Some(mut r) => Ok(Some(Base {
                name: name.to_owned(),
                oid: r.peel_to_id()?.detach(),
                local,
            })),
            None => Ok(None),
        }
    };

    if let Some(name) = explicit {
        return match find(name, true)? {
            Some(b) => Ok(vec![b]),
            None => match find(name, false)? {
                Some(b) => Ok(vec![b]),
                None => bail!("no branch {name}"),
            },
        };
    }

    let default = default_branch(repo)?.context("cannot find the default branch; use --base")?;
    let bases: Vec<Base> = [
        find(&default, true)?,
        find(&format!("{REMOTE}/{default}"), false)?,
    ]
    .into_iter()
    .flatten()
    .collect();
    if bases.is_empty() {
        bail!("no branch {default}");
    }
    Ok(bases)
}

/// If one base contains the other, keeps only the one that contains the
/// other, because each check against a base is slow on a long history. This
/// is the usual case: the local default branch is behind `origin/<default>`.
fn drop_redundant_bases(repo: &gix::Repository, mut bases: Vec<Base>) -> Result<Vec<Base>> {
    if let [a, b] = &bases[..] {
        // x contains y if y is the merge base of the two.
        let merge_base = repo.merge_base(a.oid, b.oid)?.map(|id| id.detach());
        if merge_base == Some(b.oid) {
            bases.truncate(1);
        } else if merge_base == Some(a.oid) {
            bases.remove(0);
        }
    }
    Ok(bases)
}

/// The branch that `origin/HEAD` points to, or else the first local branch
/// of `main`, `master` and `trunk` that exists.
fn default_branch(repo: &gix::Repository) -> Result<Option<String>> {
    let prefix = format!("refs/remotes/{REMOTE}/");
    if let Some(head) = repo.try_find_reference(format!("{prefix}HEAD").as_str())?
        && let gix::refs::TargetRef::Symbolic(target) = head.target()
        && let Some(name) = target
            .as_bstr()
            .to_str()
            .ok()
            .and_then(|t| t.strip_prefix(&prefix))
    {
        return Ok(Some(name.to_owned()));
    }
    for name in ["main", "master", "trunk"] {
        if repo
            .try_find_reference(format!("refs/heads/{name}").as_str())?
            .is_some()
        {
            return Ok(Some(name.to_owned()));
        }
    }
    Ok(None)
}

/// The branch that HEAD of the worktree at the path points to, or nothing
/// for a detached HEAD.
fn head_branch(repo: &gix::Repository) -> Result<Option<String>> {
    Ok(repo.head_name()?.and_then(|n| {
        n.as_bstr()
            .to_str()
            .ok()
            .and_then(|n| n.strip_prefix("refs/heads/"))
            .map(str::to_owned)
    }))
}

/// Switches the worktree at `path` to the branch `to`. Returns the branch,
/// or the reason why trim did not switch.
///
/// trim does not switch if a change is not committed, because `git switch`
/// moves such a change to the new branch without a message. Untracked files
/// do not stop the switch: `git switch` stops by itself if the switch would
/// overwrite one. The git CLI does the switch, so the checks and the hooks
/// of git apply.
fn switch_away(
    repo: &gix::Repository,
    path: &Path,
    to: Option<&str>,
) -> std::result::Result<String, String> {
    let to = to.ok_or("the base branch is not a local branch")?;
    if let Some(state) = repo.state() {
        use gix::state::InProgress::*;
        let what = match state {
            ApplyMailbox | ApplyMailboxRebase => "git am",
            Bisect => "a bisect",
            CherryPick | CherryPickSequence => "a cherry-pick",
            Merge => "a merge",
            Rebase | RebaseInteractive => "a rebase",
            Revert | RevertSequence => "a revert",
        };
        return Err(format!("{what} is in progress"));
    }
    let git = |args: &[&str]| -> std::result::Result<String, String> {
        let out = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .output()
            .map_err(|e| format!("cannot run git: {e}"))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
        }
    };
    if !git(&["status", "--porcelain", "--untracked-files=no"])?.is_empty() {
        return Err("changes are not committed".to_owned());
    }
    git(&["switch", "--quiet", to])?;
    Ok(to.to_owned())
}

/// Branches that HEAD of the main worktree or of a linked worktree points to.
fn checked_out_branches(repo: &gix::Repository) -> Result<HashSet<String>> {
    let mut names = HashSet::new();
    for wt in repo.worktrees_including_main()? {
        if let Some(head) = wt?.head_name()?
            && let Some(name) = head
                .as_bstr()
                .to_str()
                .ok()
                .and_then(|n| n.strip_prefix("refs/heads/"))
        {
            names.insert(name.to_owned());
        }
    }
    Ok(names)
}

/// One branch on each line, with tabs between the fields: repository,
/// branch, result and age.
fn print_tsv(
    out: &mut impl Write,
    results: &[(PathBuf, RepoResult)],
    now: i64,
) -> std::io::Result<()> {
    for (path, r) in results {
        for f in &r.found {
            writeln!(
                out,
                "{}\t{}\t{}\t{}",
                path.display(),
                f.branch,
                f.how.as_str(),
                age(now - f.time)
            )?;
        }
    }
    Ok(())
}

fn age(secs: i64) -> String {
    let (n, unit) = match secs.max(0) {
        s if s < 3600 => (s / 60, "minute"),
        s if s < 86_400 => (s / 3600, "hour"),
        s if s < 14 * 86_400 => (s / 86_400, "day"),
        s if s < 60 * 86_400 => (s / (7 * 86_400), "week"),
        s if s < 365 * 86_400 => (s / (30 * 86_400), "month"),
        s => (s / (365 * 86_400), "year"),
    };
    let plural = if n == 1 { "" } else { "s" };
    format!("{n} {unit}{plural} ago")
}

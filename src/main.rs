//! trim: list local git branches whose change is already on the default
//! branch, also after a rebase or squash merge. With `--delete`, remove them.

mod detect;

use anyhow::{Context, Result, bail};
use clap::Parser;
use detect::{Detector, How};
use git2::{BranchType, Oid, Repository};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{SystemTime, UNIX_EPOCH};

/// The only remote that trim reads and changes.
const REMOTE: &str = "origin";

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
}

struct Found {
    repo: String,
    branch: String,
    how: How,
    time: i64,
}

#[derive(Default)]
struct RepoResult {
    found: Vec<Found>,
    /// Lines for stdout about deleted branches.
    deleted: Vec<String>,
    /// Lines for stderr: errors and branches that were not deleted.
    notes: Vec<String>,
    failed: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    tune_libgit2();

    let mut repos = Vec::new();
    for path in &cli.paths {
        if !path.is_dir() {
            eprintln!("trim: {}: not a directory", path.display());
            return ExitCode::from(2);
        }
        find_repos(path, cli.depth, &mut repos);
    }

    let mut results: Vec<(String, RepoResult)> = repos
        .par_iter()
        .map(|path| {
            let name = path.display().to_string();
            let result = scan(path, &name, &cli).unwrap_or_else(|e| RepoResult {
                notes: vec![format!("trim: {name}: {e:#}")],
                failed: true,
                ..Default::default()
            });
            (name, result)
        })
        .collect();
    results.sort_by(|a, b| a.0.cmp(&b.0));

    let mut failed = false;
    for (_, r) in &results {
        for note in &r.notes {
            eprintln!("{note}");
        }
        failed |= r.failed;
    }

    let stdout = std::io::stdout();
    let aligned = stdout.is_terminal();
    let mut out = stdout.lock();
    let written = if cli.delete {
        results
            .iter()
            .flat_map(|(_, r)| &r.deleted)
            .try_for_each(|line| writeln!(out, "{line}"))
    } else {
        let found: Vec<&Found> = results.iter().flat_map(|(_, r)| &r.found).collect();
        print_table(&mut out, &found, aligned)
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

/// By default libgit2 does not cache a tree larger than 4 KiB. A large
/// directory then is read from the pack again for each commit, which makes
/// the tree diffs on a long history about 1.5 times slower.
fn tune_libgit2() {
    // SAFETY: these calls change global libgit2 settings. They occur before
    // any other libgit2 call and before trim starts its threads.
    unsafe {
        let _ = git2::opts::set_cache_object_limit(git2::ObjectType::Tree, 1 << 20);
        let _ = git2::opts::set_cache_max_size(1 << 30);
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
    tip: Oid,
    remote: bool,
}

impl Candidate {
    /// The branch name on the remote, without the remote prefix.
    fn remote_name(&self) -> &str {
        &self.name[REMOTE.len() + 1..]
    }
}

fn scan(path: &Path, name: &str, cli: &Cli) -> Result<RepoResult> {
    let repo = Repository::open(path).context("cannot open repository")?;
    let mut result = RepoResult::default();

    let mut candidates = Vec::new();
    let remote_prefix = format!("{REMOTE}/");
    for branch in repo.branches(None)? {
        let (branch, kind) = branch?;
        let remote = kind == BranchType::Remote;
        if remote && !cli.remotes {
            continue;
        }
        // A symbolic ref such as origin/HEAD has no target.
        if let (Some(name), Some(tip)) = (branch.name()?, branch.get().target())
            && (!remote || name.starts_with(&remote_prefix))
        {
            candidates.push(Candidate {
                name: name.to_owned(),
                tip,
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
    let checked_out = checked_out_branches(&repo)?;
    candidates.retain(|c| !skip.contains(&c.name));

    let mut detector = Detector::new(Repository::open(path)?);
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

    let mut to_delete_on_remote = Vec::new();
    for (c, how) in candidates.into_iter().zip(classified) {
        let Some(how) = how else { continue };
        let short = c.tip.to_string()[..10].to_owned();

        if cli.delete {
            if c.remote {
                to_delete_on_remote.push((c.remote_name().to_owned(), c.tip, how));
            } else if checked_out.contains(&c.name) {
                result.notes.push(format!(
                    "trim: {name}: {} is {}, but it is checked out; not deleted",
                    c.name,
                    how.as_str()
                ));
            } else {
                repo.find_branch(&c.name, BranchType::Local)?
                    .delete()
                    .with_context(|| format!("cannot delete {}", c.name))?;
                // The tip lets the user restore the branch: git branch <name> <tip>
                result.deleted.push(format!(
                    "Deleted {} in {name} ({}, was {short})",
                    c.name,
                    how.as_str()
                ));
            }
        }
        result.found.push(Found {
            repo: name.to_owned(),
            time: repo.find_commit(c.tip)?.time().seconds(),
            branch: c.name,
            how,
        });
    }

    if !to_delete_on_remote.is_empty() {
        let refs: Vec<(String, Oid)> = to_delete_on_remote
            .iter()
            .map(|(b, tip, _)| (b.clone(), *tip))
            .collect();
        let outcome = delete_on_remote(path, &refs)?;
        for (branch, tip, how) in to_delete_on_remote {
            match outcome.get(&branch) {
                Some(Ok(())) => result.deleted.push(format!(
                    "Deleted {REMOTE}/{branch} in {name} ({}, was {})",
                    how.as_str(),
                    &tip.to_string()[..10]
                )),
                Some(Err(why)) => {
                    result.notes.push(format!(
                        "trim: {name}: {REMOTE}/{branch} not deleted: {why}"
                    ));
                    result.failed = true;
                }
                None => {
                    result.notes.push(format!(
                        "trim: {name}: {REMOTE}/{branch}: no result from git push"
                    ));
                    result.failed = true;
                }
            }
        }
    }
    Ok(result)
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
    branches: &[(String, Oid)],
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
    oid: Oid,
    /// True for a local branch, false for a remote-tracking ref.
    local: bool,
}

/// The branches to compare with. Without `--base`, these are the local
/// default branch and `origin/<default>`. A branch counts as merged if one of
/// them contains it, because after a merge on the forge the local default
/// branch is often behind the remote.
fn bases(repo: &Repository, explicit: Option<&str>) -> Result<Vec<Base>> {
    let find = |name: &str, kind: BranchType| -> Result<Option<Base>> {
        match repo.find_branch(name, kind) {
            Ok(b) => Ok(Some(Base {
                name: name.to_owned(),
                oid: b.get().peel_to_commit()?.id(),
                local: kind == BranchType::Local,
            })),
            Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    };

    if let Some(name) = explicit {
        return match find(name, BranchType::Local)? {
            Some(b) => Ok(vec![b]),
            None => match find(name, BranchType::Remote)? {
                Some(b) => Ok(vec![b]),
                None => bail!("no branch {name}"),
            },
        };
    }

    let default = default_branch(repo).context("cannot find the default branch; use --base")?;
    let bases: Vec<Base> = [
        find(&default, BranchType::Local)?,
        find(&format!("{REMOTE}/{default}"), BranchType::Remote)?,
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
fn drop_redundant_bases(repo: &Repository, mut bases: Vec<Base>) -> Result<Vec<Base>> {
    if let [a, b] = &bases[..] {
        if a.oid == b.oid || repo.graph_descendant_of(a.oid, b.oid)? {
            bases.truncate(1);
        } else if repo.graph_descendant_of(b.oid, a.oid)? {
            bases.remove(0);
        }
    }
    Ok(bases)
}

/// The branch that `origin/HEAD` points to, or else the first local branch
/// of `main`, `master` and `trunk` that exists.
fn default_branch(repo: &Repository) -> Option<String> {
    let prefix = format!("refs/remotes/{REMOTE}/");
    if let Ok(head) = repo.find_reference(&format!("{prefix}HEAD"))
        && let Ok(Some(target)) = head.symbolic_target()
        && let Some(name) = target.strip_prefix(&prefix)
    {
        return Some(name.to_owned());
    }
    ["main", "master", "trunk"]
        .into_iter()
        .find(|n| repo.find_branch(n, BranchType::Local).is_ok())
        .map(str::to_owned)
}

/// Branches that HEAD of the main worktree or of a linked worktree points to.
fn checked_out_branches(repo: &Repository) -> Result<HashSet<String>> {
    let mut names = HashSet::new();
    let mut add = |repo: &Repository| {
        if let Ok(head) = repo.find_reference("HEAD")
            && let Ok(Some(target)) = head.symbolic_target()
            && let Some(name) = target.strip_prefix("refs/heads/")
        {
            names.insert(name.to_owned());
        }
    };
    add(repo);
    for wt in repo.worktrees()?.iter().flatten().flatten() {
        if let Ok(wt) = repo.find_worktree(wt)
            && let Ok(wt_repo) = Repository::open_from_worktree(&wt)
        {
            add(&wt_repo);
        }
    }
    Ok(names)
}

/// Aligned columns on a terminal; tab-separated fields in a pipe.
fn print_table(out: &mut impl Write, rows: &[&Found], aligned: bool) -> std::io::Result<()> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let cells: Vec<[String; 4]> = rows
        .iter()
        .map(|f| {
            [
                f.repo.clone(),
                f.branch.clone(),
                f.how.as_str().to_owned(),
                age(now - f.time),
            ]
        })
        .collect();
    if !aligned {
        for c in &cells {
            writeln!(out, "{}", c.join("\t"))?;
        }
        return Ok(());
    }
    let mut width = [0; 3];
    for c in &cells {
        for (w, cell) in width.iter_mut().zip(c) {
            *w = (*w).max(cell.chars().count());
        }
    }
    for c in &cells {
        writeln!(
            out,
            "{:w0$}  {:w1$}  {:w2$}  {}",
            c[0],
            c[1],
            c[2],
            c[3],
            w0 = width[0],
            w1 = width[1],
            w2 = width[2]
        )?;
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

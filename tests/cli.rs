//! End-to-end tests: make scratch repositories with the git CLI, run the
//! trim binary on them and read its tab-separated output.

use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        // Isolate the tests from the user's git configuration.
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn init(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    commit(dir, "base.txt", "base\n", "base");
}

fn commit(dir: &Path, file: &str, content: &str, msg: &str) {
    std::fs::write(dir.join(file), content).unwrap();
    git(dir, &["add", file]);
    git(dir, &["commit", "-q", "-m", msg]);
}

fn new_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    init(dir.path());
    dir
}

fn trim(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_trim"))
        .args(args)
        .arg(dir)
        .output()
        .expect("trim runs")
}

/// (branch, how) for each line of trim's output.
fn found(dir: &Path) -> Vec<(String, String)> {
    let out = trim(dir, &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            assert_eq!(f.len(), 4, "line: {l}");
            (f[1].to_owned(), f[2].to_owned())
        })
        .collect()
}

fn pair(branch: &str, how: &str) -> (String, String) {
    (branch.to_owned(), how.to_owned())
}

/// A branch with two commits, and one unrelated commit on main after it.
fn diverged_feature(dir: &Path) {
    git(dir, &["checkout", "-q", "-b", "feat"]);
    commit(dir, "a.txt", "one\n", "a1");
    commit(dir, "a.txt", "one\ntwo\n", "a2");
    git(dir, &["checkout", "-q", "main"]);
    commit(dir, "other.txt", "other\n", "other");
}

#[test]
fn finds_merge_commit() {
    let repo = new_repo();
    let d = repo.path();
    git(d, &["checkout", "-q", "-b", "feat"]);
    commit(d, "a.txt", "a\n", "a");
    git(d, &["checkout", "-q", "main"]);
    git(d, &["merge", "-q", "--no-ff", "-m", "merge", "feat"]);
    assert_eq!(found(d), vec![pair("feat", "merged")]);
}

#[test]
fn finds_cherry_picked_commits() {
    let repo = new_repo();
    let d = repo.path();
    diverged_feature(d);
    git(d, &["cherry-pick", "main..feat"]);
    assert_eq!(found(d), vec![pair("feat", "rebased")]);
}

#[test]
fn finds_squash_merge() {
    let repo = new_repo();
    let d = repo.path();
    diverged_feature(d);
    git(d, &["merge", "-q", "--squash", "feat"]);
    git(d, &["commit", "-q", "-m", "squashed feat"]);
    assert_eq!(found(d), vec![pair("feat", "squashed")]);
}

#[test]
fn ignores_unmerged_branch() {
    let repo = new_repo();
    let d = repo.path();
    diverged_feature(d);
    assert_eq!(found(d), vec![]);
}

#[test]
fn ignores_partly_picked_branch() {
    let repo = new_repo();
    let d = repo.path();
    diverged_feature(d);
    git(d, &["cherry-pick", "feat~1"]);
    assert_eq!(found(d), vec![]);
}

#[test]
fn ignores_squash_with_later_work_on_branch() {
    let repo = new_repo();
    let d = repo.path();
    diverged_feature(d);
    git(d, &["merge", "-q", "--squash", "feat"]);
    git(d, &["commit", "-q", "-m", "squashed feat"]);
    git(d, &["checkout", "-q", "feat"]);
    commit(d, "a.txt", "one\ntwo\nthree\n", "a3");
    git(d, &["checkout", "-q", "main"]);
    assert_eq!(found(d), vec![]);
}

#[test]
fn finds_squash_on_remote_when_local_main_is_behind() {
    let upstream = new_repo();
    let up = upstream.path();
    let clone = TempDir::new().unwrap();
    let cl = clone.path();
    git(cl, &["clone", "-q", up.to_str().unwrap(), "."]);

    git(cl, &["checkout", "-q", "-b", "feat"]);
    commit(cl, "a.txt", "one\n", "a1");
    commit(cl, "a.txt", "one\ntwo\n", "a2");
    git(cl, &["checkout", "-q", "main"]);

    // The forge squash-merges the branch; the clone only fetches.
    git(up, &["fetch", "-q", cl.to_str().unwrap(), "feat:feat"]);
    git(up, &["merge", "-q", "--squash", "feat"]);
    git(up, &["commit", "-q", "-m", "squashed feat"]);
    git(up, &["branch", "-q", "-D", "feat"]);
    git(cl, &["fetch", "-q"]);

    assert_eq!(found(cl), vec![pair("feat", "squashed")]);
}

#[test]
fn delete_removes_stale_branches_only() {
    let repo = new_repo();
    let d = repo.path();
    diverged_feature(d);
    git(d, &["merge", "-q", "--squash", "feat"]);
    git(d, &["commit", "-q", "-m", "squashed feat"]);
    git(d, &["branch", "wip", "feat"]);
    git(d, &["checkout", "-q", "wip"]);
    commit(d, "b.txt", "b\n", "b");
    git(d, &["checkout", "-q", "main"]);

    let out = trim(d, &["--delete"]);
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.starts_with("Deleted feat in "), "{stdout}");
    let branches = git(d, &["branch", "--format=%(refname:short)"]);
    assert_eq!(branches.lines().collect::<Vec<_>>(), vec!["main", "wip"]);
}

#[test]
fn delete_keeps_checked_out_branch() {
    let repo = new_repo();
    let d = repo.path();
    diverged_feature(d);
    git(d, &["merge", "-q", "--squash", "feat"]);
    git(d, &["commit", "-q", "-m", "squashed feat"]);
    git(d, &["checkout", "-q", "feat"]);

    let out = trim(d, &["--delete"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("checked out; not deleted"));
    assert!(git(d, &["branch", "--list", "feat"]).contains("feat"));
}

/// An upstream repository and a clone of it. The clone pushed `feat`, the
/// upstream squash-merged it but kept the branch, and the clone fetched.
fn squashed_on_remote() -> (TempDir, TempDir) {
    let upstream = new_repo();
    let up = upstream.path();
    let clone = TempDir::new().unwrap();
    let cl = clone.path();
    git(cl, &["clone", "-q", up.to_str().unwrap(), "."]);

    git(cl, &["checkout", "-q", "-b", "feat"]);
    commit(cl, "a.txt", "one\n", "a1");
    commit(cl, "a.txt", "one\ntwo\n", "a2");
    git(cl, &["push", "-q", "origin", "feat"]);
    git(cl, &["checkout", "-q", "main"]);

    git(up, &["merge", "-q", "--squash", "feat"]);
    git(up, &["commit", "-q", "-m", "squashed feat"]);
    git(cl, &["fetch", "-q"]);
    (upstream, clone)
}

fn lines(out: &Output) -> Vec<String> {
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.split('\t').skip(1).take(2).collect::<Vec<_>>().join(" "))
        .collect()
}

#[test]
fn remotes_lists_remote_branches() {
    let (_up, clone) = squashed_on_remote();
    let cl = clone.path();
    assert_eq!(lines(&trim(cl, &[])), vec!["feat squashed"]);
    // origin/main and origin/HEAD are not in the list.
    assert_eq!(
        lines(&trim(cl, &["--remotes"])),
        vec!["feat squashed", "origin/feat squashed"]
    );
}

#[test]
fn remotes_delete_removes_branch_on_remote() {
    let (upstream, clone) = squashed_on_remote();
    let (up, cl) = (upstream.path(), clone.path());

    let out = trim(cl, &["--remotes", "--delete"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("Deleted origin/feat in "), "{stdout}");
    assert_eq!(git(up, &["branch", "--format=%(refname:short)"]), "main\n");
    // git push also removes the remote-tracking ref in the clone.
    assert_eq!(
        git(cl, &["branch", "-a", "--format=%(refname)"]),
        "refs/heads/main\nrefs/remotes/origin/HEAD\nrefs/remotes/origin/main\n"
    );
}

#[test]
fn remotes_delete_keeps_branch_that_changed_after_fetch() {
    let (upstream, clone) = squashed_on_remote();
    let (up, cl) = (upstream.path(), clone.path());

    // Someone else adds a commit to feat on the remote.
    let other = TempDir::new().unwrap();
    let ot = other.path();
    git(ot, &["clone", "-q", up.to_str().unwrap(), "."]);
    git(ot, &["checkout", "-q", "feat"]);
    commit(ot, "a.txt", "one\ntwo\nthree\n", "a3");
    git(ot, &["push", "-q", "origin", "feat"]);

    let out = trim(cl, &["--remotes", "--delete"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("origin/feat not deleted"), "{stderr}");
    assert!(git(up, &["branch", "--list", "feat"]).contains("feat"));
}

#[test]
fn skips_repository_without_commits() {
    let dir = TempDir::new().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    let out = trim(dir.path(), &[]);
    assert!(out.status.success());
    assert!(out.stdout.is_empty() && out.stderr.is_empty());
}

#[test]
fn searches_directories_for_repositories() {
    let root = TempDir::new().unwrap();
    for name in ["one", "group/two"] {
        let d = root.path().join(name);
        std::fs::create_dir_all(&d).unwrap();
        init(&d);
        git(&d, &["branch", "done"]);
        commit(&d, "more.txt", "more\n", "more");
    }

    let out = trim(root.path(), &[]);
    let stdout = String::from_utf8(out.stdout).unwrap();
    let repos: Vec<&str> = stdout
        .lines()
        .map(|l| l.split('\t').next().unwrap())
        .collect();
    assert_eq!(repos.len(), 2, "{stdout}");
    assert!(repos[0].ends_with("group/two") && repos[1].ends_with("one"));

    let out = trim(root.path(), &["--depth", "1"]);
    assert_eq!(String::from_utf8(out.stdout).unwrap().lines().count(), 1);
}

/// stdout of trim with the layout for a person, without colour codes.
fn for_person(dir: &Path, args: &[&str]) -> (String, String) {
    let mut args = args.to_vec();
    args.extend(["--color", "always"]);
    let out = trim(dir, &args);
    let strip = |b: &[u8]| {
        let s = String::from_utf8_lossy(b).into_owned();
        let mut plain = String::new();
        let mut in_code = false;
        for c in s.chars() {
            match (in_code, c) {
                (false, '\x1b') => in_code = true,
                (true, 'm') => in_code = false,
                (false, c) => plain.push(c),
                _ => {}
            }
        }
        plain
    };
    (strip(&out.stdout), strip(&out.stderr))
}

#[test]
fn groups_branches_by_repository() {
    let repo = new_repo();
    let d = repo.path();
    git(d, &["branch", "done"]);
    commit(d, "more.txt", "more\n", "more");

    let out = trim(d, &["--color", "always"]);
    let stdout = String::from_utf8(out.stdout).unwrap();
    let name = d.file_name().unwrap().to_str().unwrap();
    assert!(
        stdout.starts_with(&format!("\x1b[1m{name}\x1b[0m  ")),
        "{stdout}"
    );

    let (stdout, _) = for_person(d, &[]);
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(lines[0].starts_with(&format!("{name}  ")), "{stdout}");
    assert!(lines[1].starts_with("  done  merged  "), "{stdout}");
    assert_eq!(lines[2], "");
    assert_eq!(lines[3], "1 stale branch. Run with --delete to remove it.");
}

#[test]
fn says_when_nothing_is_stale() {
    let repo = new_repo();
    let (stdout, _) = for_person(repo.path(), &[]);
    assert_eq!(stdout, "No stale branches in 1 repository.\n");
}

#[test]
fn delete_shows_kept_branch_in_group() {
    let repo = new_repo();
    let d = repo.path();
    git(d, &["branch", "done"]);
    git(d, &["checkout", "-q", "-b", "current"]);

    let (stdout, stderr) = for_person(d, &["--delete"]);
    assert!(stderr.is_empty(), "{stderr}");
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(
        lines[1].starts_with("  deleted  done     merged  was "),
        "{stdout}"
    );
    assert!(
        lines[2].starts_with("  kept     current  merged  checked out"),
        "{stdout}"
    );
    assert_eq!(lines[4], "Deleted 1 branch. Kept 1 checked-out branch.");
}

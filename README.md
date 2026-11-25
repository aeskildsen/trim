# trim

List local git branches that are already on the default branch, also after a rebase or a squash merge. `git branch --merged` finds only true merges, so after a forge squash-merges a pull request the branch stays in the list.

```console
$ trim ~/src
~/src/site     redesign       squashed  6 weeks ago
~/src/engine   fix-release   rebased   9 days ago
~/src/notes       add-index     merged    2 months ago

$ trim --delete ~/src/site
Deleted redesign in ~/src/site (squashed, was 3f9c2a1b7e)
```

Each path is a repository, or a directory that trim searches for repositories (`--depth`, default 2). trim reads only local refs, so run `git fetch` first. The default branch is the target of `origin/HEAD`, else `main`, `master` or `trunk`; a branch counts if the local default branch or `origin/<default>` contains it. Use `--base` to compare with a different branch.

With `--remotes` (`-r`), trim also checks the branches of the remote `origin`, as the last fetch saw them, and lists them as `origin/<branch>`. With `--remotes --delete`, it deletes them on the remote through `git push`, so your SSH and credential setup apply. Each deletion carries a lease on the tip that trim checked: if someone pushed to the branch after your last fetch, the remote refuses the deletion and trim reports it. trim needs the `git` command only for this.

On a terminal the columns are aligned; in a pipe they are tab-separated, one branch per line.

## How a branch counts as stale

1. **merged** — the tip is an ancestor of the base. A new branch with no commits of its own also has this result.
2. **rebased** — each commit on the branch has a commit with the same [patch-id](https://git-scm.com/docs/git-patch-id) on the base (rebase merge, cherry-pick).
3. **squashed** — the full diff from the merge base to the tip has the same patch-id as one commit on the base.

A branch can be missed (for example, when a conflict fix changed the squash commit), but trim does not report a branch whose change is not on the base. `--delete` skips branches that are checked out in a worktree, and prints each deleted tip, so `git branch <name> <tip>` restores it.

## Speed

trim uses libgit2. libgit2 1.9 cannot read a split commit-graph (`objects/info/commit-graphs/`), and its tree diffs are slower than git's. On a long history with old branches, such as a clone of a large upstream project, one run can take some seconds. A repository with recent branches takes milliseconds.

## Build

```sh
nix develop -c cargo test
nix build        # result/bin/trim
```

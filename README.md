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

trim uses gitoxide (`gix`), in pure Rust. It checks repositories in parallel. The time for one repository grows with the number of commits on the default branch since the oldest branch split off. With a commit-graph (git writes one during maintenance), a test branch 10,000 commits deep in Home Assistant core (126,000 commits) takes about 3.5 s; without one, about 21 s. A repository with recent branches takes milliseconds.

## Benchmark

`bench/run.sh` checks correctness and speed on pinned public repositories (`bench/repos.tsv`: ripgrep, the Home Assistant frontend and core, SuperCollider). In a clone of each, it makes test branches from real commits at depths 10, 100, 1000 and 10000 below the tip: the commit itself (`merged`), its change as a new commit (`rebased`), its change in two commits (`squashed`), and its change plus one line (must not be found). It measures each repository with a split commit-graph, a single-file commit-graph and none.

```sh
nix develop -c bench/run.sh                  # all repositories
nix develop -c bench/run.sh ripgrep          # one repository
TRIM=path/to/other/trim BENCH_LABEL=other nix develop -c bench/run.sh
```

The first run clones about 1.5 GB into `$XDG_CACHE_HOME/trim-bench`. Results go to `results/` in that folder, one TSV file per run. The script header lists all settings.

## Install

```sh
cargo install --git https://github.com/aeskildsen/trim
```

trim is pure Rust, so this needs only a Rust toolchain. With Nix: `nix profile install github:aeskildsen/trim`. `trim --remotes --delete` also needs the `git` command.

## Build

```sh
nix develop -c cargo test
nix build        # result/bin/trim
```

## License

MIT

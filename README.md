# trim

trim finds local git branches whose changes are already on the default branch. It finds a branch after a merge, a rebase or a squash merge. With `--delete`, it deletes these branches.

`git branch --merged` finds only true merges. When a forge squash-merges a pull request, the branch stays in its list. trim finds that branch too.

```console
$ trim ~/src
engine  ~/src/engine
  fix-release  rebased   9 days ago

notes  ~/src/notes
  add-index  merged  2 months ago

site  ~/src/site
  redesign  squashed  6 weeks ago

3 stale branches in 3 of 8 repositories. Run with --delete to remove them.

$ trim --delete ~/src/site
site  ~/src/site
  deleted  redesign  squashed  was 3f9c2a1b7e

Deleted 1 branch.
```

## Install

```sh
cargo install --locked --git https://github.com/aeskildsen/trim
```

trim is pure Rust. You need only a Rust toolchain, version 1.95 or later.
The `--locked` flag makes Cargo use the dependency versions in `Cargo.lock`,
which are the versions that the tests use.

With Nix, use this command:

```sh
nix profile install github:aeskildsen/trim
```

These commands install the latest version on `main`. To install a release, add its tag:

```sh
cargo install --locked --git https://github.com/aeskildsen/trim --tag v0.1.0
nix profile install github:aeskildsen/trim/v0.1.0
```

`trim --version` shows the installed version. [CHANGELOG.md](CHANGELOG.md) lists the changes in each release.

## Use

```
trim [OPTIONS] [PATHS]...
```

Each path is a repository or a directory. trim searches a directory for repositories, to a depth of 2 levels. The default path is the current directory.

trim reads only the local refs. Run `git fetch` before trim to get the latest state of the remote.

| Option | Function |
|---|---|
| `-d`, `--delete` | Delete the branches that trim finds. trim does not delete a branch that is checked out in a worktree. |
| `-r`, `--remotes` | Also check the branches of the remote `origin`. With `--delete`, delete them on the remote. |
| `-b`, `--base <BRANCH>` | Compare with this branch, not with the default branch. |
| `--depth <N>` | Search for repositories to this number of directory levels. The default is 2. |
| `--color <WHEN>` | `auto`, `always` or `never`. The default is `auto`. |

### The default branch

trim uses the branch that `origin/HEAD` points to. If `origin/HEAD` does not exist, trim uses the first of `main`, `master` and `trunk` that exists. If none of them exists, use `--base`.

A branch is stale if the local default branch contains its changes. It is also stale if `origin/<default>` contains them. After a merge on the forge, the local default branch is frequently behind the remote.

### Output

On a terminal, trim shows the branches in groups by repository. Each group has a heading with the name and the path of the repository. A line at the end gives the totals. With `--delete`, a group also shows each checked-out branch that trim did not delete.

In a pipe, trim writes one branch on each line, with a tab between the fields. The fields are the repository, the branch, the result and the age of the last commit. With `--delete`, trim writes one line for each deleted branch, and a message on stderr for each checked-out branch that it did not delete. Use the pipe format in scripts.

On a terminal, trim uses colour. `--color never` or the `NO_COLOR` environment variable turns the colour off. `--color always` uses colour and the terminal layout also in a pipe, for example `trim --color always | less -R`.

Errors go to stderr. The exit status is 0 if all checks and deletions are successful. It is 1 if an error occurs, and 2 if a path is not a directory.

### Delete on the remote

`--remotes --delete` deletes the branches on the remote with `git push`. Your SSH and credential configuration apply. For this function only, trim needs the `git` command.

Each deletion has a lease on the commit that trim checked. If a person pushed to the branch after your last fetch, the remote does not delete the branch. trim then shows an error.

### Restore a branch

For each deleted branch, trim shows the last commit. To restore the branch, use this command:

```sh
git branch <name> <commit>
```

## How trim finds a stale branch

trim does three checks, in this sequence:

1. **merged**: The default branch contains the last commit of the branch. A new branch without commits also gets this result.
2. **rebased**: Each commit on the branch has a commit with the same patch-id on the default branch. A rebase merge or a cherry-pick gives this result.
3. **squashed**: The full change of the branch has the same patch-id as one commit on the default branch. A squash merge gives this result.

A patch-id is a hash of a change. It does not include line numbers or whitespace. Thus, a change keeps its patch-id when it moves to a different parent commit. trim calculates its own patch-ids. They are not the same as the ids of `git patch-id`.

trim can miss a stale branch. For example, if a person fixed a conflict in the squash commit, the patch-ids are different. trim does not show a branch that has a change that is not on the default branch.

## Speed

trim uses gitoxide (`gix`). It checks the repositories in parallel.

The time for a repository increases with the number of commits on the default branch since the oldest branch started. A repository with new branches takes some milliseconds.

A commit-graph makes trim faster. git writes a commit-graph during maintenance. In Home Assistant core (126,000 commits), a test branch 10,000 commits old takes approximately 3.5 s with a commit-graph. Without a commit-graph, it takes approximately 21 s.

## Development

The Nix dev shell supplies all tools. Run each command in it:

```sh
nix develop -c just check     # the tests, fast
nix develop -c just test      # the tests
nix develop -c just lint      # rustfmt and clippy
nix develop -c just build     # release build into target/release/
nix build                     # Nix package into result/bin/trim
```

To let rustfmt correct the formatting, use `just lint correct`.

### Release

trim uses [Semantic Versioning](https://semver.org/). `Cargo.toml` holds the version, and each release has a tag `v<version>`. There are no releases on crates.io and no prebuilt binaries.

1. In a pull request, set the new version in `Cargo.toml`, run `cargo check` to update `Cargo.lock`, and move the `Unreleased` items in `CHANGELOG.md` to a section for the new version.
2. Merge the pull request, then update the local `main`.
3. Run `nix develop -c just release <version>`. The recipe makes the tag only if `Cargo.toml`, `Cargo.lock` and `CHANGELOG.md` agree with the version.
4. Push the tag with `git push origin v<version>`.

### Benchmark

`bench/run.sh` measures the speed and the correctness of trim. It uses public repositories at fixed commits. `bench/repos.tsv` lists them: ripgrep, the Home Assistant frontend and core, and SuperCollider.

In each repository, the script makes test branches from real commits. The commits are 10, 100, 1,000 and 10,000 commits below the tip. For each commit, the script makes four branches:

| Branch | Content | Correct result |
|---|---|---|
| `bench/dN-merged` | The commit | `merged` |
| `bench/dN-rebased` | The change of the commit, as a new commit | `rebased` |
| `bench/dN-squashed` | The change of the commit, in two commits | `squashed` |
| `bench/dN-open` | The change of the commit, with one more line | Not found |

The script measures each repository three times: with a split commit-graph, with a single-file commit-graph and without a commit-graph.

```sh
nix develop -c bench/run.sh                  # all repositories
nix develop -c bench/run.sh ripgrep          # one repository
TRIM=path/to/other/trim BENCH_LABEL=other nix develop -c bench/run.sh
```

The first run downloads approximately 1.5 GB into `$XDG_CACHE_HOME/trim-bench`. Each run writes a TSV file into `results/` in that directory. The header of the script lists all settings.

## License

MIT

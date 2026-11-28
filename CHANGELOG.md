# Changelog

All important changes to trim are in this file. The format is from [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). trim uses [Semantic Versioning](https://semver.org/). Before version 1.0, a change to the minor version (0.1 to 0.2) can change the options or the results. A change to the patch version (0.1.0 to 0.1.1) only fixes problems.

## [Unreleased]

### Added

- `--switch` with `--delete`: if the branch of the worktree is stale, trim switches to the base branch and then deletes the stale branch. trim does not switch a worktree with changes that are not committed, or with a merge, rebase or other operation in progress.

### Removed

- The `just check` recipe. The tests are fast, so just use `just test`.

## [0.1.0] - 2026-11-28

The first release.

### Added

- Find local branches whose changes are already on the default branch: after a merge, a rebase or a squash merge.
- `--delete` deletes these branches. trim does not delete a branch that is checked out in a worktree.
- `--remotes` also checks the branches of `origin`. With `--delete`, trim deletes them on the remote with a lease.
- `--base` sets the branch to compare with. `--depth` sets how deep trim searches a directory for repositories.
- Output in groups by repository on a terminal, and one branch on each line in a pipe. `--color` and `NO_COLOR` control the colour.

[Unreleased]: https://github.com/aeskildsen/trim/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/aeskildsen/trim/releases/tag/v0.1.0

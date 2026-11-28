# Requires just 1.23.0 or later.

# The recipe `release` below replaces the recipe from just/tailor.just.
set allow-duplicate-recipes

import 'just/loader.just'

# List available recipes
default:
    @just --list

# Fast check: the tests
check:
    cargo test --quiet

# Create a local release tag with just release x.y.z
release $VERSION:
    #!/usr/bin/env bash
    set -euo pipefail

    fail() { echo "Error: $*" >&2; exit 1; }

    [[ "${VERSION}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
        || fail "VERSION must be in format x.y.z (e.g., 0.1.0)"

    # The tag must match the source files, so the checkout must be clean.
    [[ -z "$(git status --porcelain)" ]] || fail "Working directory is not clean"

    branch="$(git branch --show-current)"
    [[ "${branch}" == main ]] || fail "Release from main, not from ${branch}"

    # The version in Cargo.toml is the version that `trim --version` shows.
    # The first line that starts with `version =` is in the [package] table.
    cargo_version="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)"
    [[ "${cargo_version}" == "${VERSION}" ]] \
        || fail "Cargo.toml has version ${cargo_version}, not ${VERSION}"

    # `cargo install --locked` stops if Cargo.lock does not agree with Cargo.toml.
    cargo metadata --locked --format-version 1 > /dev/null \
        || fail "Cargo.lock does not agree with Cargo.toml"

    grep -q "^## \[${VERSION}\]" CHANGELOG.md \
        || fail "CHANGELOG.md has no section for ${VERSION}"

    ! git show-ref --tags --verify --quiet "refs/tags/v${VERSION}" \
        || fail "Tag v${VERSION} already exists"

    git tag -a "v${VERSION}" -m "v${VERSION}"
    echo "Tag v${VERSION} created on $(git rev-parse --short HEAD)"
    echo ""
    echo "To publish the release:"
    echo "  git push origin v${VERSION}"

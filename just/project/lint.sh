#!/usr/bin/env bash
# Static checks: rustfmt and clippy. With "correct", rustfmt first writes its
# changes.
set -euo pipefail
cd "$(dirname "$0")/../.."
if [ "${1:-check}" = correct ]; then
	cargo fmt
fi
cargo fmt --check
cargo clippy --all-targets -- -D warnings

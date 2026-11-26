#!/usr/bin/env bash
# Release build of the trim binary into target/release/.
set -euo pipefail
cd "$(dirname "$0")/../.."
exec cargo build --release

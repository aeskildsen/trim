#!/usr/bin/env bash
# The unit and integration tests. A pass here is what counts as green.
# The benchmark suite (bench/run.sh) is separate: it needs the network and
# takes minutes.
set -euo pipefail
cd "$(dirname "$0")/../.."
exec cargo test

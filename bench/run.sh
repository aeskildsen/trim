#!/usr/bin/env bash
# Correctness and speed check of trim on public repositories with known
# answers. Run it in the dev shell: nix develop -c bench/run.sh [name...]
#
# For each repository in bench/repos.tsv, the script makes a clone without a
# work tree and sets the default branch to the pinned commit. Then, for each
# depth N, it selects a real commit X about N commits below the tip and makes
# four test branches from it:
#
#   bench/dN-merged    the commit X itself            expected: merged
#   bench/dN-rebased   a new commit with the change   expected: rebased
#                      of X, on the parent of X
#   bench/dN-squashed  the change of X in two commits expected: squashed
#   bench/dN-open      the change of X plus one line  expected: not found
#
# The history above X is real, so later commits can change the same lines
# again. Each repository is measured with a split commit-graph, a single-file
# commit-graph and no commit-graph, because git libraries differ in which
# formats they read, and the graph can change the time a lot.
#
# Environment:
#   TRIM          the trim binary to measure (default: a release build of
#                 this checkout)
#   BENCH_LABEL   name of the measured build in the results (default: the
#                 short commit of this checkout)
#   BENCH_CACHE   folder for the clones and results
#                 (default: $XDG_CACHE_HOME/trim-bench)
#   BENCH_DEPTHS  depths for the test branches (default: 10 100 1000 10000)
#   BENCH_GRAPHS  commit-graph variants (default: split single none)
#   BENCH_RUNS    timed runs per case, after one warm-up run (default: 3)

set -euo pipefail
# printf reads numbers in the locale format; some locales use a decimal comma.
export LC_ALL=C

here=$(cd "$(dirname "$0")" && pwd)
root=$(dirname "$here")
cache=${BENCH_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/trim-bench}
read -r -a depths <<<"${BENCH_DEPTHS:-10 100 1000 10000}"
read -r -a graphs <<<"${BENCH_GRAPHS:-split single none}"
runs=${BENCH_RUNS:-3}

if [[ -z ${TRIM:-} ]]; then
  cargo build --release --quiet --manifest-path "$root/Cargo.toml"
  TRIM=$root/target/release/trim
  dirty=$(git -C "$root" diff --quiet HEAD -- src Cargo.toml Cargo.lock || echo "-dirty")
  label=${BENCH_LABEL:-$(git -C "$root" rev-parse --short HEAD)$dirty}
else
  label=${BENCH_LABEL:-$(basename "$TRIM")}
fi

# Fixed identities and dates give the same commit ids on each run.
export GIT_AUTHOR_NAME="trim bench" GIT_AUTHOR_EMAIL="bench@example.invalid"
export GIT_COMMITTER_NAME="trim bench" GIT_COMMITTER_EMAIL="bench@example.invalid"
export GIT_AUTHOR_DATE="2000-01-01T00:00:00Z" GIT_COMMITTER_DATE="2000-01-01T00:00:00Z"

repo=""
idx=""
# Automatic gc or maintenance would change the commit-graph during a run.
g() { git -C "$repo" -c gc.auto=0 -c maintenance.auto=false -c core.quotePath=false "$@"; }
gi() { GIT_INDEX_FILE=$idx git -C "$repo" "$@"; }

mkdir -p "$cache/results"
stamp=$(date +%Y-%m-%dT%H%M)
results=$cache/results/$stamp-$label.tsv
printf 'label\trepo\tcommits\tgraph\tmean_s\tstddev_s\tpeak_mib\tcorrect\tcases\n' >"$results"
summary=()

prepare_clone() {
  local name=$1 url=$2 branch=$3 pin=$4
  repo=$cache/$name
  if [[ ! -d $repo/.git ]]; then
    echo "== $name: cloning $url"
    git clone --quiet --no-checkout --single-branch --branch "$branch" "$url" "$repo"
  fi
  if ! g cat-file -e "$pin^{commit}" 2>/dev/null; then
    g fetch --quiet origin "$branch"
  fi
  g update-ref "refs/heads/$branch" "$pin"
  g update-ref "refs/remotes/origin/$branch" "$pin"
  g symbolic-ref HEAD "refs/heads/$branch"
  # A --single-branch clone has no origin/HEAD; a normal clone has one, and
  # trim uses it to find the default branch.
  g symbolic-ref "refs/remotes/origin/HEAD" "refs/remotes/origin/$branch"
  g for-each-ref --format='delete %(refname)' refs/heads/bench/ | g update-ref --stdin
}

# A commit about $1 commits below the tip, on the first-parent line. It must
# add or change at least two files and no others, and its first file must be
# text, so that the squashed and open branches can be made from it.
pick_commit() {
  local depth=$1 pin=$2 x
  while read -r x; do
    local status
    status=$(g diff-tree --no-commit-id -r --name-status "$x")
    (($(wc -l <<<"$status") >= 2)) || continue
    grep -qv $'^[AM]\t' <<<"$status" && continue
    local first
    first=$(head -n1 <<<"$status" | cut -f2-)
    [[ $(g diff-tree --no-commit-id -r --numstat "$x" -- "$first") == -* ]] && continue
    echo "$x"
    return
  done < <(g rev-list --first-parent --no-merges --skip="$depth" --max-count=500 "$pin")
}

# Prints "branch<TAB>expected" for each test branch it makes.
make_branches() {
  local depth=$1 x=$2 parent first mode blob t c1 c2
  parent=$(g rev-parse "$x^")
  first=$(g diff-tree --no-commit-id -r --name-only "$x" | head -n1)
  read -r mode _ blob _ < <(g ls-tree "$x" -- "$first")
  idx=$(mktemp)

  g update-ref "refs/heads/bench/d$depth-merged" "$x"
  echo -e "bench/d$depth-merged\tmerged"

  c1=$(g commit-tree "$x^{tree}" -p "$parent" -m "bench: change of $x")
  g update-ref "refs/heads/bench/d$depth-rebased" "$c1"
  echo -e "bench/d$depth-rebased\trebased"

  gi read-tree "$parent"
  gi update-index --add --cacheinfo "$mode,$blob,$first"
  t=$(gi write-tree)
  c1=$(g commit-tree "$t" -p "$parent" -m "bench: part 1 of $x")
  c2=$(g commit-tree "$x^{tree}" -p "$c1" -m "bench: part 2 of $x")
  g update-ref "refs/heads/bench/d$depth-squashed" "$c2"
  echo -e "bench/d$depth-squashed\tsquashed"

  gi read-tree "$x"
  blob=$({ g cat-file blob "$x:$first"; printf '\ntrim bench change\n'; } | g hash-object -w --stdin)
  gi update-index --cacheinfo "$mode,$blob,$first"
  t=$(gi write-tree)
  c1=$(g commit-tree "$t" -p "$parent" -m "bench: $x with one more line")
  g update-ref "refs/heads/bench/d$depth-open" "$c1"
  echo -e "bench/d$depth-open\tnone"

  rm -f "$idx"
}

set_graph() {
  rm -f "$repo/.git/objects/info/commit-graph"
  rm -rf "$repo/.git/objects/info/commit-graphs"
  case $1 in
    single) g commit-graph write --reachable --no-progress ;;
    split) g commit-graph write --reachable --split --no-progress ;;
    none) ;;
    *) echo "unknown commit-graph variant: $1" >&2; exit 2 ;;
  esac
}

# Compares the output of trim with the expected results. Prints the number
# of correct cases and a line for each wrong one.
check() {
  local expected=$1 out correct=0 total=0 branch want got
  if ! out=$("$TRIM" "$repo"); then
    echo "trim failed on $repo" >&2
    return 1
  fi
  while IFS=$'\t' read -r branch want; do
    got=$(awk -F'\t' -v b="$branch" '$2 == b { print $3 }' <<<"$out")
    got=${got:-none}
    total=$((total + 1))
    if [[ $got == "$want" ]]; then
      correct=$((correct + 1))
    else
      echo "   WRONG $branch: expected $want, got $got" >&2
    fi
  done <<<"$expected"
  # No other branch exists, so any other line is a false result.
  while IFS=$'\t' read -r _ branch got _; do
    [[ -z $branch || $branch == bench/* ]] && continue
    total=$((total + 1))
    echo "   WRONG $branch: expected nothing, got $got" >&2
  done <<<"$out"
  echo "$correct $total"
}

selected=("$@")
while IFS=$'\t' read -r name url branch pin; do
  [[ -z $name || $name == \#* ]] && continue
  if ((${#selected[@]})) && [[ ! " ${selected[*]} " == *" $name "* ]]; then
    continue
  fi
  prepare_clone "$name" "$url" "$branch" "$pin"
  commits=$(g rev-list --count "$pin")
  echo "== $name: $commits commits at ${pin:0:10}"

  expected=""
  for depth in "${depths[@]}"; do
    if ((depth >= commits)); then
      continue
    fi
    x=$(pick_commit "$depth" "$pin")
    if [[ -z $x ]]; then
      echo "   no usable commit near depth $depth" >&2
      continue
    fi
    expected+=$(make_branches "$depth" "$x")$'\n'
  done
  expected=${expected%$'\n'}

  for graph in "${graphs[@]}"; do
    set_graph "$graph"
    # A command substitution (not a process substitution) lets set -e stop
    # the script when trim fails.
    result=$(check "$expected")
    read -r correct total <<<"$result"
    csv=$(mktemp)
    hyperfine --shell=none --style=none --warmup 1 --runs "$runs" \
      --export-csv "$csv" "$TRIM $repo" >/dev/null
    # Find the columns by name: the CSV layout changed in hyperfine 2.
    read -r mean stddev mem < <(awk -F, '
      NR == 1 { for (i = 1; i <= NF; i++) col[$i] = i; next }
      {
        scale = ($col["time_wall_clock_unit"] == "ms") ? 0.001 : 1
        printf "%.3f %.3f %.0f\n", $col["time_wall_clock_mean"] * scale,
          $col["time_wall_clock_stddev"] * scale,
          $col["memory_peak_resident_mean"] / 1048576
      }' "$csv")
    rm -f "$csv"
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$label" "$name" "$commits" "$graph" \
      "$mean" "$stddev" "$mem" "$correct" "$total" >>"$results"
    line=$(printf '%-14s %7d commits  %-7s %8.3f s ± %6.3f  %5d MiB  %2d/%-2d correct' \
      "$name" "$commits" "$graph" "$mean" "$stddev" "$mem" "$correct" "$total")
    echo "   $line"
    summary+=("$line")
  done
done <"$here/repos.tsv"

echo
echo "trim $label"
printf '%s\n' "${summary[@]}"
echo "Results: $results"

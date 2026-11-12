#!/usr/bin/env bash
# One SHARD of the test suite on a CI runner (v1.18.0).
#
#   ci_test_shard.sh <index> <count> <all|lib> [cargo test arguments, e.g. --release]
#
# The library's tests are split <count> ways: the test program's own list
# (`-- --list`), sorted, and every test whose position is <index> modulo
# <count> — so each test is in exactly one shard, and neighbours in a module
# (the reverse fit's long toy fits sit together in the alphabet) land in
# different shards. With `all`, shard 0 also runs everything that is not the
# library: the CLI's own tests, the contract tests under tests/, the doc
# tests. `lib` is the library alone (the debug-assertions trip).
#
# Why: the v1.18.0 solver made the reverse fit's toy fits an order of
# magnitude longer; one runner took over three hours on the library alone
# (run 43119864352, cancelled at 3 h 23 min with the suite unfinished) where
# v1.17.1 took 31 minutes, and a job is cut off at six hours. Nothing is
# skipped and nothing is shortened: the same tests, on more runners.
#
# A shard that did not run exactly the tests it selected is an error (a name
# the test program no longer matches would otherwise run nothing, in green).
# Plain bash 3.2 (the macOS runner's): no mapfile.
set -euo pipefail

index=$1
count=$2
mode=$3
shift 3
case "$mode" in all | lib) ;; *) echo "::error::mode must be all or lib, not $mode"; exit 2 ;; esac
if [ "$index" -lt 0 ] || [ "$index" -ge "$count" ]; then
  echo "::error::shard $index is outside 0..$count"
  exit 2
fi

work=$(mktemp -d)
cargo test --locked "$@" --lib -- --list --format terse | sed -n 's/: test$//p' | LC_ALL=C sort > "$work/all"
total=$(wc -l < "$work/all" | tr -d ' ')
if [ "$total" -eq 0 ]; then
  echo "::error::the library's test list is empty"
  exit 1
fi
awk -v n="$count" -v k="$index" '(NR - 1) % n == k' "$work/all" > "$work/mine"
names=()
while IFS= read -r line; do names+=("$line"); done < "$work/mine"
echo "shard $index of $count: ${#names[@]} of $total library tests"

cargo test --locked "$@" --lib -- --exact "${names[@]}" 2>&1 | tee "$work/run"
ran=$(sed -n 's/^running \([0-9][0-9]*\) tests\{0,1\}$/\1/p' "$work/run" | head -n 1)
if [ "${ran:-0}" -ne "${#names[@]}" ]; then
  echo "::error::shard $index selected ${#names[@]} library tests and the test program ran ${ran:-0}"
  exit 1
fi

if [ "$mode" = all ] && [ "$index" -eq 0 ]; then
  # `--bins --test '*'`, not `--bins --tests`. `--tests` is every target with
  # `test = true`, the library among them: it ran the whole library suite a
  # second time, unsharded, inside this one job (run 43247169220: 273 minutes
  # on Linux after the shard's own 30, and macOS cut off at six hours). The
  # glob names the integration tests under tests/ and nothing else. Guarded,
  # as the shard's own count is: the library's test program starting in this
  # step is an error.
  cargo test --locked "$@" --bins --test '*' 2>&1 | tee "$work/rest"
  if grep -q 'unittests src/lib\.rs' "$work/rest"; then
    echo "::error::shard 0's non-library step ran the library's tests again"
    exit 1
  fi
  cargo test --locked "$@" --doc
fi

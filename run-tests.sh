#!/usr/bin/env bash
#
# Every test suite, one command, one PASS/FAIL line per suite.
#
#   ./run-tests.sh             build, then run everything
#   ./run-tests.sh --no-build  reuse the existing dist/index.html
#
# Suites, cheapest first:
#   build    ./build.sh (clippy, WASM, check-dist gate); browser suites need it fresh
#   gate     scripts/test-check-dist.sh: the gate refuses a faulty copy per rule
#   engine   cargo test
#   clippy   the no-panic lints (for a --no-build run)
#   bison    tests/bison/compare.py: counts against GNU Bison
#   browser  tests/browser/run.py, in Chromium and Firefox
#
# A failing suite does not stop later ones. Output goes to test-logs/<suite>.log.

set -uo pipefail

cd "$(dirname "$0")"
ROOT="$(pwd)"
LOGS="$ROOT/test-logs"

BUILD=1
for arg in "$@"; do
  case "$arg" in
    --no-build) BUILD=0 ;;
    -h|--help) sed -n '2,26p' "$0"; exit 0 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

for tool in cargo bison python3; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "$tool is not on PATH; see README.md, \"Running from source\"." >&2
    exit 2
  fi
done

rm -rf "$LOGS"
mkdir -p "$LOGS"

NAMES=()
RESULTS=()
TIMES=()

# run <name> <command...>: runs the command, logs it, records PASS/FAIL.
run() {
  local name="$1"; shift
  local log="$LOGS/$name.log"
  local start=$SECONDS
  printf '%-8s ... ' "$name"
  if "$@" >"$log" 2>&1; then
    RESULTS+=("PASS")
  else
    RESULTS+=("FAIL")
  fi
  local took=$((SECONDS - start))
  NAMES+=("$name")
  TIMES+=("$took")
  printf '%s  (%ss)\n' "${RESULTS[-1]}" "$took"
  if [ "${RESULTS[-1]}" = FAIL ]; then
    echo "         last lines of test-logs/$name.log:"
    tail -n 15 "$log" | sed 's/^/           /'
  fi
}

if [ "$BUILD" = 1 ]; then
  run build ./build.sh
elif [ ! -f dist/index.html ]; then
  echo "--no-build given but dist/index.html does not exist." >&2
  exit 2
fi

run gate   scripts/test-check-dist.sh dist/index.html
run engine bash -c 'cd engine && cargo test --locked --no-fail-fast'
run clippy bash -c 'cd engine && cargo clippy --locked --all-targets -- -D warnings'
run bison  python3 tests/bison/compare.py
run browser python3 tests/browser/run.py

echo
echo "suite     result  time"
FAILED=0
for i in "${!NAMES[@]}"; do
  printf '%-8s  %-6s  %ss\n' "${NAMES[$i]}" "${RESULTS[$i]}" "${TIMES[$i]}"
  [ "${RESULTS[$i]}" = FAIL ] && FAILED=$((FAILED + 1))
done
echo
if [ "$FAILED" = 0 ]; then
  echo "ALL SUITES PASS"
else
  echo "$FAILED SUITE(S) FAILED - see test-logs/"
  exit 1
fi

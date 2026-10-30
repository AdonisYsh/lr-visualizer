#!/usr/bin/env bash
#
# Usage: scripts/engine-identical.sh <old-tree> [<new-tree>]
# Compares the full JSON analyze() returns for two engine trees, case by case:
#   presets, perf grammars, the probe, the layered family and refusals.
# Prints ENGINE IDENTICAL when every case matches; exit 0 identical, 1 differs, 2 cannot run.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VARIANTS=(lr0 slr1 lalr1 clr1)

if [ $# -lt 1 ] || [ $# -gt 2 ]; then
  sed -n '3,6p' "$0" >&2
  exit 2
fi
for tree in "$1" "${2:-$ROOT}"; do
  if [ ! -f "$tree/engine/Cargo.toml" ] || [ ! -d "$tree/engine/src" ]; then
    echo "$tree: no engine/Cargo.toml and engine/src here" >&2
    exit 2
  fi
done
# Each engine's Cargo files and src/ are copied to scratch, the driver added, and each built in its own target dir.
OLD="$(cd "$1" && pwd)"
NEW="$(cd "${2:-$ROOT}" && pwd)"

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo is not on PATH; see README.md, \"Running from source\"." >&2
  exit 2
fi

# --- scratch directory ------------------------------------------------------
# ENGINE_IDENTICAL_DIR keeps the scratch directory and its build cache; otherwise it is removed on exit.
if [ -n "${ENGINE_IDENTICAL_DIR:-}" ]; then
  mkdir -p "$ENGINE_IDENTICAL_DIR"
  WORK="$(cd "$ENGINE_IDENTICAL_DIR" && pwd)"
else
  WORK="$(mktemp -d)"
  # Only ever a directory mktemp just made for this run.
  trap '[ -n "$WORK" ] && [ -d "$WORK" ] && rm -rf -- "$WORK"' EXIT
fi
RUN="$(mktemp -d "$WORK/run.XXXXXX")"
CASES="$RUN/cases"
mkdir -p "$CASES" "$RUN/old" "$RUN/new"

# --- the driver -------------------------------------------------------------
# Same source for both builds: writes each analyze() result, untouched, to <out>/<id>.json.
# index.tsv (timing, description) is for reading and is not compared.
DRIVER="$RUN/engine_identical_driver.rs"
cat > "$DRIVER" <<'RUST'
//! Injected by scripts/engine-identical.sh into a copy of each engine.
//! Not part of the engine; see that script.

use std::fmt::Write as _;
use std::process::exit;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(manifest), Some(out)) = (args.first(), args.get(1)) else {
        eprintln!("usage: engine_identical_driver <manifest.tsv> <out-dir>");
        exit(2);
    };
    let cases = std::fs::read_to_string(manifest).unwrap_or_else(|e| {
        eprintln!("{manifest}: {e}");
        exit(2);
    });

    let mut index = String::new();
    for line in cases.lines().filter(|l| !l.is_empty()) {
        // id <TAB> variant <TAB> grammar file <TAB> test string (may be empty)
        let mut fields = line.splitn(4, '\t');
        let id = fields.next().unwrap_or("");
        let variant = fields.next().unwrap_or("");
        let path = fields.next().unwrap_or("");
        let test = fields.next().unwrap_or("");
        let grammar = std::fs::read_to_string(path).unwrap_or_else(|e| {
            eprintln!("{path}: {e}");
            exit(2);
        });

        let started = Instant::now();
        let json = lr_engine::analyze(&grammar, variant, test);
        let ms = started.elapsed().as_secs_f64() * 1000.0;

        if let Err(e) = std::fs::write(format!("{out}/{id}.json"), &json) {
            eprintln!("{out}/{id}.json: {e}");
            exit(2);
        }
        let _ = writeln!(index, "{id}\t{ms:.1}\t{}", describe(&json));
    }
    if let Err(e) = std::fs::write(format!("{out}/index.tsv"), index) {
        eprintln!("{out}/index.tsv: {e}");
        exit(2);
    }
}

/// `ok<TAB>312 steps, 12 states, 4 rows, accepted` or `error<TAB>the message`.
fn describe(json: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return "invalid\tnot JSON".to_string();
    };
    if v["ok"] != serde_json::Value::Bool(true) {
        let message: String =
            v["error"]["message"].as_str().unwrap_or("").chars().take(72).collect();
        return format!("error\t{message}");
    }
    let empty = Vec::new();
    let steps = v["steps"].as_array().unwrap_or(&empty);
    let parse = match steps.last().and_then(|s| s["kind"].as_str()) {
        Some("parseAccept") => ", accepted",
        Some("parseReject") => ", rejected",
        _ => "",
    };
    format!(
        "ok\t{} steps, {} states, {} rows, {} warnings{parse}",
        steps.len(),
        v["table"]["states"],
        v["verdict"]["comparison"].as_array().map_or(0, Vec::len),
        v["warnings"].as_array().map_or(0, Vec::len),
    )
}
RUST

# build <side> <tree>: copy that tree's engine, add the driver, build it.
build() {
  local side="$1" tree="$2" copy
  copy="$(mktemp -d "$WORK/$side-engine.XXXXXX")"
  cp "$tree/engine/Cargo.toml" "$tree/engine/Cargo.lock" "$copy/"
  cp -R "$tree/engine/src" "$copy/src"
  mkdir -p "$copy/src/bin"
  cp "$DRIVER" "$copy/src/bin/engine_identical_driver.rs"
  echo "building the $side engine  ($tree/engine)" >&2
  if ! (cd "$copy" && CARGO_TARGET_DIR="$WORK/$side-target" \
          cargo build --release --locked --quiet --bin engine_identical_driver) >&2; then
    echo "the $side engine did not build" >&2
    exit 2
  fi
}
build old "$OLD"
build new "$NEW"

# --- the cases --------------------------------------------------------------
MANIFEST="$RUN/manifest.tsv"
: > "$MANIFEST"
IDS=()
declare -A EXPECT

# add <id> <ok|error> <variant> <grammar file> [<test string>]
add() {
  IDS+=("$1")
  EXPECT["$1"]="$2"
  printf '%s\t%s\t%s\t%s\n' "$1" "$3" "$4" "${5:-}" >> "$MANIFEST"
}

# inline <name> <text>: a grammar that is not a file of the repository.
inline() {
  printf '%s' "$2" > "$CASES/$1.txt"
  echo "$CASES/$1.txt"
}

# Layered-precedence family at any size: (n - 2) / 2 left-recursive levels over an atom.
layered() {
  local productions="$1" levels i tighter
  levels=$(( (productions - 2) / 2 ))
  for (( i = 1; i <= levels; i++ )); do
    if [ "$i" -lt "$levels" ]; then
      tighter="$(printf 'E%02d' $(( i + 1 )))"
    else
      tighter=atom
    fi
    printf 'E%02d -> E%02d op%02d %s | %s\n' "$i" "$i" "$i" "$tighter" "$tighter"
  done
  printf 'atom -> ( E01 ) | id\n'
}

# Accepted and rejected test strings per preset; the accepted ones are those web/grammars.js ships.
# A handout missing here is an error, so a new preset cannot go untested.
declare -A ACCEPT REJECT
ACCEPT[01-expression-grammar]='id + id * id';        REJECT[01-expression-grammar]='id + * id'
ACCEPT[02-balanced-pairs]='a a c b b';               REJECT[02-balanced-pairs]='a c b b'
ACCEPT[03-two-halves]='c d d';                       REJECT[03-two-halves]='c d'
ACCEPT[04-epsilon]='b';                              REJECT[04-epsilon]='a a b'
ACCEPT[05-dangling-else]='i i a e a';                REJECT[05-dangling-else]='i e a'
ACCEPT[06-ambiguous-expression]='id + id * id';      REJECT[06-ambiguous-expression]='id + + id'
ACCEPT[07-reduce-reduce]='a';                        REJECT[07-reduce-reduce]='a a'
ACCEPT[08-pointers-and-assignment]='* id = * * id';  REJECT[08-pointers-and-assignment]='id = = id'
ACCEPT[09-merge-casualty]='a c d';                   REJECT[09-merge-casualty]='a c c'
ACCEPT[10-named-tokens]='if id then id = num';       REJECT[10-named-tokens]='if id then'
ACCEPT[11-nested-parentheses]='( ( a ) )';           REJECT[11-nested-parentheses]='( a ) )'
ACCEPT[12-optional-pairs]='a a b b';                 REJECT[12-optional-pairs]='a b b'
ACCEPT[13-empty-nonterminals]='a b';                 REJECT[13-empty-nonterminals]='a a'
ACCEPT[pascal-subset]='PROGRAM ID ; VAR ID , ID : INTEGER ; BEGIN ID ASSIGN ID + NUM * ( ID - NUM ) ; IF ID < NUM THEN ID ASSIGN NUM END .'
REJECT[pascal-subset]='PROGRAM ID ; BEGIN ID ASSIGN END .'

for file in "$ROOT"/examples/*.txt; do
  name="$(basename "$file" .txt)"
  if [ -z "${ACCEPT[$name]:-}" ] || [ -z "${REJECT[$name]:-}" ]; then
    echo "examples/$name.txt has no test strings in $0 - add them to ACCEPT and REJECT" >&2
    exit 2
  fi
  for v in "${VARIANTS[@]}"; do
    add "preset.$name.$v.none"   ok    "$v" "$file"
    add "preset.$name.$v.accept" ok    "$v" "$file" "${ACCEPT[$name]}"
    add "preset.$name.$v.reject" ok    "$v" "$file" "${REJECT[$name]}"
    # A symbol the grammar does not have: refused, and refused the same way.
    add "preset.$name.$v.typo"   error "$v" "$file" "${ACCEPT[$name]} ?"
  done
done

for file in "$ROOT"/tests/perf/grammars/*.txt; do
  name="$(basename "$file" .txt)"
  case "$name" in
    gen-*) yes='id op01 ( id op02 id ) op01 id'; no='id op01 op02 id' ;;
    *)     yes="${ACCEPT[$name]:-}"; no="${REJECT[$name]:-}" ;;
  esac
  for v in "${VARIANTS[@]}"; do
    add "perf.$name.$v.none" ok "$v" "$file"
    if [ -n "$yes" ]; then
      add "perf.$name.$v.accept" ok "$v" "$file" "$yes"
      add "perf.$name.$v.reject" ok "$v" "$file" "$no"
    fi
  done
done

# tests/bison/compare.py's probe: shift, reduce and reduce in a single cell.
probe="$(inline probe-three-actions $'S -> A a | B a | a a\nA -> a\nB -> a\n')"
for v in "${VARIANTS[@]}"; do
  add "probe.three-actions.$v.none"   ok "$v" "$probe"
  add "probe.three-actions.$v.accept" ok "$v" "$probe" 'a a'
  add "probe.three-actions.$v.reject" ok "$v" "$probe" 'a'
done

# Past the corpus, where the layered family runs out of steps or rows; each size is compared as the old engine answers it.
for n in 100 120 140 172; do
  file="$(inline "gen-$n" "$(layered "$n")"$'\n')"
  for v in "${VARIANTS[@]}"; do
    add "more.gen-$n.$v.none" any "$v" "$file"
  done
done

# CLR(1) over the state cap while LR(0) is not: the weaker methods answer, the stronger are refused.
alts=""
for (( i = 0; i < 80; i++ )); do alts+="${alts:+ | }x$i A y$i"; done
over="$(inline clr1-over-the-state-cap "S -> $alts"$'\nA -> B | c\nB -> d A | e\n')"
for v in "${VARIANTS[@]}"; do
  add "more.clr1-over-the-state-cap.$v.none" any "$v" "$over"
done

# Refusals: they must read the same too.
refuse() {  # refuse <name> <text> [<variant>]
  local file
  file="$(inline "$1" "$2")"
  add "refuse.$1.${3:-lr0}" error "${3:-lr0}" "$file"
  [ -n "${3:-}" ] || add "refuse.$1.clr1" error clr1 "$file"
}
refuse empty                 ''
refuse only-a-comment        $'  // nothing here\n\n'
refuse no-arrow              $'S -> a S\n\n// the next line is the typo\nS b\n'
refuse nothing-left-of-arrow $'S -> a\n-> b\n'
refuse end-marker            $'S -> a\nS -> $\n'
refuse cycle                 $'S -> A\nA -> S\n'
refuse unproductive          $'S -> a\nS -> B\nB -> B b\n'
refuse two-symbol-left-side  $'S T -> a\n'
refuse unknown-variant       $'E -> E + T | T\nT -> id\n' lr2
refuse too-many-characters   "$(for (( i = 0; i < 4000; i++ )); do echo 'S -> a'; done)"
alts=""
for (( i = 0; i <= 200; i++ )); do alts+="${alts:+ | }t$i t$(( i / 7 ))"; done
refuse too-many-productions  "S -> $alts"$'\n'
alts=""
for (( i = 1; i <= 199; i++ )); do alts+="${alts:+ | }t$i"; done
refuse too-many-symbols      "S -> $alts"$'\n'

# --- run both, compare ------------------------------------------------------
echo "running ${#IDS[@]} cases through each engine" >&2
"$WORK/old-target/release/engine_identical_driver" "$MANIFEST" "$RUN/old"
"$WORK/new-target/release/engine_identical_driver" "$MANIFEST" "$RUN/new"

declare -A KIND WHAT OLD_MS NEW_MS
while IFS=$'\t' read -r id ms kind what; do
  KIND["$id"]="$kind"; WHAT["$id"]="$what"; OLD_MS["$id"]="$ms"
done < "$RUN/old/index.tsv"
while IFS=$'\t' read -r id ms _ _; do
  NEW_MS["$id"]="$ms"
done < "$RUN/new/index.tsv"

same=0
different=0
miscast=0
for id in "${IDS[@]}"; do
  kind="${KIND[$id]:-missing}"
  # A case the old engine answers differently from its listing fails the run.
  if [ "${EXPECT[$id]}" != any ] && [ "${EXPECT[$id]}" != "$kind" ]; then
    miscast=$((miscast + 1))
    printf 'WRONG %-52s listed as %s, but the old engine says: %s  %s\n' \
      "$id" "${EXPECT[$id]}" "$kind" "${WHAT[$id]:-}"
    continue
  fi
  if cmp -s "$RUN/old/$id.json" "$RUN/new/$id.json"; then
    same=$((same + 1))
    printf 'same  %-52s %-5s %s  [%s -> %s ms]\n' \
      "$id" "$kind" "${WHAT[$id]:-}" "${OLD_MS[$id]:-?}" "${NEW_MS[$id]:-?}"
  else
    different=$((different + 1))
    printf 'DIFF  %-52s %-5s %s\n' "$id" "$kind" "${WHAT[$id]:-}"
    # Where the two part, with a little of each side around that byte.
    at="$(cmp "$RUN/old/$id.json" "$RUN/new/$id.json" 2>/dev/null | sed -n 's/.*byte \([0-9]*\).*/\1/p' || true)"
    if [ -n "$at" ]; then
      from=$(( at > 60 ? at - 60 : 1 ))
      printf '        first difference at byte %s\n' "$at"
      printf '        old: ...%s...\n' "$(tail -c "+$from" "$RUN/old/$id.json" | head -c 160)"
      printf '        new: ...%s...\n' "$(tail -c "+$from" "$RUN/new/$id.json" | head -c 160)"
    else
      printf '        one output is a prefix of the other, or is missing\n'
    fi
  fi
done

echo
echo "${#IDS[@]} cases: $same identical, $different different, $miscast not the kind of case they are listed as"
if [ "$different" != 0 ] || [ "$miscast" != 0 ]; then
  if [ -n "${ENGINE_IDENTICAL_DIR:-}" ]; then
    echo "the two outputs of every case are in $RUN/old and $RUN/new"
  else
    echo "set ENGINE_IDENTICAL_DIR=<dir> and run again to keep the two outputs of every case"
  fi
  echo "ENGINE CHANGED"
  exit 1
fi
echo "ENGINE IDENTICAL"

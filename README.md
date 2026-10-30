# LR Parsing Visualizer

An interactive teaching tool for bottom-up parsing. Type a context-free
grammar, pick an LR variant, and step through the whole analysis one click at a
time: FIRST and FOLLOW sets, the canonical collection of item sets drawn as a
growing automaton, the ACTION/GOTO table filled cell by cell with conflicts
flagged, and a stack-and-input trace of a test string ending in ACCEPT or
REJECT.

Every single step carries a plain-English explanation written to be read aloud.

> **Download `index.html` and open it. Nothing to install, no server, no
> network connection, no build.** It is one self-contained file — the
> stylesheet, the JavaScript and the WebAssembly engine are all inlined — so
> double-clicking it works, and so does handing a class a copy on a memory
> stick. Get it from the GitHub Pages deployment (use your browser's *Save
> Page As*, or right-click → *Save link as* on the Pages link in the repository
> sidebar), or build it yourself with `./build.sh`, which writes
> `dist/index.html`. `dist/` is not checked in; it is generated.

**All four variants are implemented: LR(0), SLR(1), LALR(1) and CLR(1).**

### Download / checksum

Every deployment publishes the page's SHA-256 next to it, as
`index.html.sha256` at the same address. To check a downloaded copy, put the
two files in one folder and run `sha256sum -c index.html.sha256`. A build you
make yourself will not necessarily match it byte for byte.

## What it is for

The four methods differ from each other in exactly two places, and the tool is
built to make both visible.

**Where reductions go.** LR(0) writes a completed item `A -> α •` into every
terminal column. SLR(1) writes it only into the columns of `FOLLOW(A)`. CLR(1)
and LALR(1) write it only into the columns the item itself carries. Everything
else about table construction — the shifts, the GOTO entries, conflict
detection, the verdict — is shared, so switching the variant selector changes
one thing and you can see what it was.

**What an item is.** LR(0) and SLR(1) share one automaton, item for item and
number for number. CLR(1) is the first method whose item sets change: an item
becomes `[A -> α • β, a]`, carrying the terminals that may follow `A` *in the
context that particular state is reached in*, and two states are equal only when
their lookaheads match as well as their cores. That precision costs states — 22
against 12 on the classic expression grammar — and the tool always shows the
price next to the benefit.

**LALR(1) is the bargain between them, and the tool shows it being struck.** It
builds the CLR(1) automaton in full, then merges every group of states that
differ only in lookahead, unioning the lookahead sets. The result has exactly
one state per core, which is exactly as many states as LR(0) builds. The Item
Sets tab shows this as a *stage*, not a result: the canonical automaton first,
then one merge at a time, each absorbed state drawn faded in place with a
`merges into` arrow to the state that swallowed it while the edges redirect,
with a running count overhead —

```
CLR(1): 10 states  →  LALR(1): 7 states        2 of 3 merges applied — 8 states so far
```

Merging can cost something. When a conflict in the LALR(1) table does not exist
in the CLR(1) one, the tool says so, in as many words, on the cell itself:

> This reduce-reduce conflict did not exist in CLR(1). It appeared because
> states I6 and I9 were merged: separately each had a single lookahead, but the
> union means both reductions are now possible on `` `d` ``. This is the price
> LALR(1) pays for its smaller table, and it is why some grammars are CLR(1) but
> not LALR(1).

That claim is established by building the CLR(1) table and comparing, never by
reasoning about the shape of a conflict. Merging can never cause a
*shift-reduce* conflict — the shifts follow the core, and the core is exactly
what merging preserves — and the test suite asserts it never does.

## Using it in a lecture

1. **Open `dist/index.html`.** It starts on the classic expression grammar under
   LR(0), already analysed. That table has conflicts, so under LR(0) the
   walkthrough stops at the table and the String Parse tab is disabled, with the
   reason shown. Switch to SLR(1) and the string parses.
2. **Pick a method and a grammar** in the left panel. The example menu is
   grouped by the weakest method that handles each grammar: *Works from LR(0)*,
   *Needs SLR(1)*, *Needs LALR(1)*, *Needs CLR(1)* and *Conflicts in every
   method*. Changing the method re-runs the same grammar and returns to the
   first step.
3. **Walk the five phase tabs in order** with `→` or the Next button: the
   grammar as understood, FIRST/FOLLOW, the item sets, the table, and the parse
   of the test string. Under the player, each step has a one-line summary; the
   engine's full explanation, written to be read out, is behind **Show full
   explanation** and stays open while you step. **Show grammar** (or `G`) lays
   the numbered productions over the stage, production 0 being the augmented
   one, without moving the step.
4. **Click a filled cell of the table** to jump to the state it names in the Item
   Sets tab, with the item that produced the entry highlighted. When a table has
   conflicts and a stronger method that was run is clean, the verdict box on the
   Parsing Table tab says so ("This grammar is SLR(1), though - try that."), and the
   method's name is a button that re-runs the grammar under it.
5. **"Compare all four"** is the sixth tab: one table, four rows, computed for
   the grammar on screen. State count, conflict count, conflict kinds, verdict.
   A lecturer can paste a grammar in and get the whole comparison in one screen;
   each method's name in it is a button that switches to that method.
6. **Hand out `examples/`.** Thirteen `.txt` grammars matching the presets, each
   with a comment header saying what it demonstrates and the numbers it
   produces. They open in the tool by copy-paste, and the test suite checks the
   numbers in those headers still match what the engine measures.

A suggested order for a single lecture:

| Show | Grammar | The point |
| --- | --- | --- |
| LR(0), then SLR(1) | `01-expression-grammar` | One symbol of lookahead, used in one place, clears both conflicts. The automaton does not change at all. |
| SLR(1), then CLR(1) | `08-pointers-and-assignment` | SLR(1) conflicts on `=` because `=` is in FOLLOW(R). Per-state lookaheads do not. |
| CLR(1), then LALR(1) | `03-two-halves` | Dragon book 4.42. Ten states fold back to seven, and nothing is lost. Step through the merging stage. |
| CLR(1), then LALR(1) | `09-merge-casualty` | The bill. CLR(1) has no conflicts; LALR(1) has two, and both are the merge's doing. |
| Any, all four | `05-dangling-else` | No LR method handles this grammar. The conflict survives all four. The grammar is known to be ambiguous (the preset says so); the conflict alone would not prove it. |

## Running from source

You need:

| Tool | For | Install |
| --- | --- | --- |
| Rust (stable) + `wasm32-unknown-unknown` + clippy | the engine | [rustup](https://rustup.rs); `rust-toolchain.toml` adds the target and clippy on first use |
| `wasm-bindgen-cli`, **exactly** the version pinned in `engine/Cargo.toml` | the WASM glue | `cargo install wasm-bindgen-cli --version 0.2.127 --locked` |
| `wasm-opt` | shrinking the `.wasm` | `binaryen` from your package manager, or a [release](https://github.com/WebAssembly/binaryen/releases) |
| Python 3, Playwright, GNU Bison | the test suites only | `pip install -r tests/requirements.txt`, then `python3 -m playwright install chromium firefox`; `bison` from your package manager |

```
./build.sh                             # builds web/pkg/ and dist/index.html
python3 -m http.server -d web 8000     # serve web/ with the files kept separate
cd engine && cargo test                # engine test suite
cargo run --bin dump                   # text dump of every step, no UI
cargo run --bin dump -- --variant=lalr1 ../examples/03-two-halves.txt "c d d"
```

`build.sh` runs clippy first, builds with the locked dependency versions, and
checks that the `wasm-bindgen` crate and CLI versions agree. It then assembles
the bundle in a staging directory and runs `scripts/check-dist.sh` on it, which
refuses a bundle that contains an HTML-injection sink, an external reference, a
storage call or a network call, or whose content security policy is not the
exact strict one. Only a bundle that passes replaces `dist/index.html`. A
published copy is built and deployed by `.github/workflows/pages.yml` on every
push to the default branch, after the engine tests and clippy pass.

### Continuous integration

| Workflow | Runs | What it does |
| --- | --- | --- |
| `ci.yml` | every push and pull request | `./run-tests.sh`: every suite below, in Chromium and Firefox |
| `security.yml` | push, pull request, weekly | Trivy, Semgrep, cargo-deny and zizmor ([SECURITY.md](SECURITY.md)) |
| `codeql.yml` | push, pull request, weekly | CodeQL over the JavaScript, Python, Rust and workflows |
| `pages.yml` | push to the default branch | builds, gates and publishes `dist/index.html` with its checksum |

Dependabot keeps the crates, actions and Python test dependency current. It
skips `wasm-bindgen`, whose CLI must be bumped together with the crate.

### Tests

```
./run-tests.sh                 # everything below, one PASS/FAIL line per suite
./run-tests.sh --no-build      # reuse the existing dist/index.html
cd engine && cargo test        # the engine suite on its own
```

`run-tests.sh` runs these suites, cheapest first, and writes each one's output
to `test-logs/<suite>.log`:

| Suite | What it checks |
| --- | --- |
| `build` | `./build.sh`: clippy, the WASM build, and the `dist/index.html` gate |
| `gate` | `scripts/test-check-dist.sh`: the gate refuses a faulty copy of the bundle for each of its rules |
| `engine` | `cargo test`: the engine's behaviour, preset by preset, plus textbook facts, malformed input, resource limits and random grammars |
| `clippy` | the lints that forbid `unwrap`, `expect`, indexing and `panic!` in the engine |
| `bison` | `tests/bison/compare.py`: state and conflict counts against GNU Bison |
| `browser` | `tests/browser/run.py`: the built page driven in Chromium and Firefox |

The `bison` and `browser` suites need the test tools from the table above.

### Benchmark

```
python3 tests/perf/run.py
```

This builds the page, then times the engine natively and the built
`dist/index.html` in Chromium and Firefox over the grammars in
`tests/perf/grammars/` (hand-written ones and generated ones), checks that the
browser and native runs agree on every count, and joins the two sets of rows
into `tests/perf/out/performance.csv` (or `--out FILE`). A full run takes about
15 minutes; the per-environment `native.csv` and `browser.csv` are left beside
it. `--only gen-020` runs a single grammar and writes
`tests/perf/out/performance-partial.csv`, leaving the full result alone. Other
options: `--browsers`, `--runs`, `--skip-native`, `--skip-browser`,
`--no-build`.

## Writing a grammar

```
E -> E + T | T
T -> T * F | F
F -> ( E ) | id
```

- One production per line. Blank lines and lines starting `//` are ignored.
- The arrow may be `->`, `→` or `::=`; `|` separates alternatives.
- Epsilon is `ε`, `#`, `eps`, or simply an empty right-hand side.
- The start symbol is the left-hand side of the first production. The tool
  augments the grammar with `S' -> S` as production 0.
- `$` is reserved as the end-of-input marker.

**Symbols are tokenized in one of two modes, detected automatically.** If any
token anywhere is longer than one character, the whole grammar is split on
whitespace; otherwise every character is its own symbol. The chosen mode is
always stated in the first step and in the left panel, and the parsed symbol
list is shown so you can see exactly what was understood.

This matters: in space-separated mode `F -> (E) | id` reads `(E)` as a single
symbol. The tool detects that and warns, suggesting `( E )`.

**Left recursion is fine.** `E -> E + T` is correct and normal for LR parsing.
The tool says so explicitly, because students taught LL parsing first are
trained to treat it as an error.

## Reading the output

Use the player bar, or the keyboard: `→`/space next, `←` previous,
`Home`/`End` first/last, `p` play/pause, `g` show/hide the grammar (`Esc`
closes it). The Reset button returns to the first step, and the slider
scrubs. Play runs at 5 s, 2.5 s (the default), 1 s or 0.5 s per step. Inside
the tab strip the arrow keys, `Home` and `End` move between tabs. The
diagram's states and the table's filled cells are focusable and open from the
keyboard (`Enter`), and the table and the diagram both have a print stylesheet
that drops the colour and keeps the meaning: a conflict cell carries an `S/R`
or `R/R` tag as text.

Stepping backwards is instant — the engine records every move up front and the
page simply replays them.

A conflict means **this method** cannot parse **this grammar**. It is not
evidence that the grammar is ambiguous; it may be perfectly unambiguous and
simply need a more powerful parser, and ambiguity is undecidable in general.
The tool never claims otherwise. Where a preset *is* ambiguous, the preset
says so as knowledge supplied by its author, never derived from a conflict.

Because a conflicted table proves nothing about the grammar, the walkthrough
stops at the table when it has conflicts: the Parsing Table tab shows the
complete table and its conflicts, and the String Parse tab is disabled with the
reason. *Compare all four* is unaffected.

The FIRST/FOLLOW tab captions itself to match the method: under SLR(1) it says
the FOLLOW sets are what place the reduce entries, and clicking a FOLLOW set
jumps to the first table cell it produced; under CLR(1) and LALR(1) it says
FOLLOW is shown for reference and is not used to build the table.

The theme follows the system and can be switched with the theme button at the top of
the left panel; the choice is not remembered, because the page stores
nothing. Below 1100 px wide the left panel stacks above the stage, and it can
be collapsed to a rail.

## Presets

Thirteen example grammars are built in and each is mirrored as a handout in
[`examples/`](examples/). The example menu groups them by the weakest method
that handles the grammar; the grouping is the author's, and the verdict on
screen is always the one the engine computes.

| # | Grammar | Menu group | LR(0) | SLR(1) | LALR(1) | CLR(1) |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | Expression grammar (classic) | Needs SLR(1) | 2 / 12 | 0 / 12 | 0 / 12 | 0 / 22 |
| 2 | Balanced pairs | Works from LR(0) | 0 / 6 | 0 / 6 | 0 / 6 | 0 / 10 |
| 3 | Two halves (dragon book 4.42) | Works from LR(0) | 0 / 7 | 0 / 7 | 0 / 7 | 0 / 10 |
| 4 | Epsilon | Needs SLR(1) | 1 / 6 | 0 / 6 | 0 / 6 | 0 / 6 |
| 5 | Dangling else | Conflicts in every method | 1 / 7 | 1 / 7 | 1 / 7 | 1 / 12 |
| 6 | Ambiguous expression | Conflicts in every method | 4 / 7 | 4 / 7 | 4 / 7 | 4 / 7 |
| 7 | Reduce-reduce | Conflicts in every method | 2 / 5 | 1 / 5 | 1 / 5 | 1 / 5 |
| 8 | Pointers and assignment | Needs LALR(1) | 1 / 10 | 1 / 10 | **0** / 10 | 0 / 14 |
| 9 | Merge casualty | Needs CLR(1) | 6 / 13 | 2 / 13 | **2** / 13 | **0** / 14 |
| 10 | Named tokens | Works from LR(0) | 0 / 11 | 0 / 11 | 0 / 11 | 0 / 13 |
| 11 | Nested parentheses | Works from LR(0) | 0 / 6 | 0 / 6 | 0 / 6 | 0 / 10 |
| 12 | Optional pairs | Needs SLR(1) | 2 / 5 | 0 / 5 | 0 / 5 | 0 / 8 |
| 13 | Two empty non-terminals | Needs LALR(1) | 3 / 10 | 2 / 10 | **0** / 10 | 0 / 10 |

Read as *conflicts / states*. Measured, not asserted: these are the numbers the
engine produces, and a test fails if they drift. A conflict is counted per
colliding action, so a cell holding two actions is one conflict; for every
preset that is also the number of conflicted cells.

Row 8 is why real parser generators build LALR(1) — it is the strength of
CLR(1) at the size of LR(0). Row 13 is the same point built from nothing but
epsilon. Row 9 is the one case where that fails, and the only preset where
LALR(1) and CLR(1) part company.

## Accuracy

State numbering follows the textbooks: GOTO is computed over symbols in order
of first appearance in the source, so `I0`–`I11` for the expression grammar
match the dragon book state for state. The test suite checks the automaton
size, FIRST and FOLLOW sets, state ordering, conflict counts and kinds against
textbook-verified values, and asserts that the verdict never claims ambiguity.
`tests/bison/` checks the LALR(1) and CLR(1) state and conflict counts of all
thirteen presets against GNU Bison, and records where and why the two count
differently.

It also pins the *contrasts*, which is what the tool is really for:

- **SLR(1)** — the automaton is byte-identical to the LR(0) one; the cells LR(0)
  conflicts in are exactly the ones FOLLOW clears; the pointer/assignment
  grammar conflicts on `=` and nowhere else.
- **CLR(1)** — the two-halves grammar has ten LR(1) states against seven LR(0)
  ones (dragon book 4.42) and the three extras are named; the collection differs
  from the LR(0) one for every preset and is never smaller.
- **LALR(1)** — the merged collection has exactly one state per distinct CLR(1)
  core, for every preset, which pins it between the two: never larger than
  CLR(1)'s, never smaller than LR(0)'s, and in fact always equal to LR(0)'s.
  Merging never introduces a shift-reduce conflict. LALR(1) and CLR(1) agree on
  every preset but the ninth, where the merge-induced message is asserted word
  for word.

## Layout

```
engine/                  Rust analysis engine, compiled to WebAssembly (src/, tests/)
web/                     the page, as separate files for development
examples/                thirteen grammars as .txt handouts
scripts/                 the check on dist/index.html, the strict CSP it demands, and
                         the script that compares two engine builds byte for byte
tests/browser/           the page driven in Chromium and Firefox
tests/bison/             the cross-check against GNU Bison
tests/perf/              the benchmark and its grammar corpus
tests/requirements.txt   Python packages for the test suites
build.sh                 clippy, WASM build, gate, then dist/index.html
run-tests.sh             every test suite, one command
rust-toolchain.toml      the Rust toolchain rustup installs
.github/                 CI, security scans, Dependabot, and the Pages publish
dist/                    the single-file distributable (generated, not checked in)
```

[SECURITY.md](SECURITY.md) states the security rules for a reader who has been
emailed a grammar file by a student and wants to know what opening it can do.
The short answer is nothing: no `eval`, no `innerHTML`, no network, no storage,
and a Rust engine that cannot panic across the WebAssembly boundary.

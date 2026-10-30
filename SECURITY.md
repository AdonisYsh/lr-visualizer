# Security properties

This tool exists so a student can send a grammar to a lecturer, or a lecturer
can hand a file to a class. **Opening it must be incapable of harming the
machine it is opened on.** This document states what that means concretely, so
you can decide the file is safe without reading all of the source.

`dist/index.html` is one self-contained file. It has no installer, no
dependencies, and never contacts a server.

## The grammar is data, never code

Nothing in the pipeline evaluates the grammar you type or open. It is
tokenized, validated and analysed by a Rust program compiled to WebAssembly.
There is no `eval`, no `new Function`, no `setTimeout`/`setInterval` with a
string argument, and no `document.write` anywhere in the project.

## No HTML injection

The JavaScript never uses `innerHTML`, `outerHTML` or `insertAdjacentHTML` —
not even for content believed to be static. Every element is created with
`document.createElement` or `document.createElementNS`, and every piece of text
is set with `textContent` or `document.createTextNode`.

A grammar containing `<img src=x onerror=alert(1)>` therefore appears on screen
as those literal characters, in the production list, in the symbol chips and in
the explanation text. No element is created from it and nothing runs. A symbol
containing a right-to-left letter or a direction-control character is drawn in
an element of its own, so it cannot reorder the text around it.

The build checks the assembled file for these sinks and refuses to produce a
release containing one (see "One gate, in one place" below).

## No network access, at any point

There is no `fetch`, no `XMLHttpRequest`, no WebSocket, no CDN, no external
font, no analytics and no telemetry. The page works with the machine's Wi-Fi
switched off, and nothing you type ever leaves your computer.

Two independent measures enforce this:

1. The bundled WebAssembly is embedded in the page as base64 and instantiated
   from those bytes. The build additionally neuters the one unused code path in
   the generated glue that could have fetched it, so the shipped file contains
   no network call at all — verifiable with `grep`.
2. A Content-Security-Policy meta tag sets `connect-src 'none'`, so the browser
   would block a request even if one somehow existed.

The policy of the shipped file is:

```
default-src 'none'; script-src 'unsafe-inline' 'wasm-unsafe-eval';
style-src 'unsafe-inline'; img-src data:; font-src data:;
connect-src 'none'; form-action 'none'; base-uri 'none'
```

(In the file it is a single `<meta http-equiv="Content-Security-Policy">` tag;
its one copy in the repository is `scripts/dist-csp.txt`.)

`'unsafe-inline'` is required because the single-file build inlines the script
and stylesheet; `'wasm-unsafe-eval'` is required to instantiate the engine.
Neither permits loading anything from outside the file.

### The development policy is a different, looser one

`web/index.html`, the page used with a local development server, loads its
stylesheet, scripts and engine by URL, which the shipped policy would forbid.
It therefore carries a development policy: the shipped one with `'self'` added
to `script-src`, `style-src` and `connect-src`, and nothing else. `build.sh`
replaces that policy with the shipped one when it assembles
`dist/index.html`, and the gate (below) demands the shipped policy back,
character for character. The development policy never reaches the file you are
sent.

## No storage

No cookies, no `localStorage`, no `sessionStorage`, no IndexedDB. Closing the
tab leaves nothing behind. Nothing persists between sessions, by design.

A symbol in the grammar is user text, so the page never uses one as a key in an
ordinary object: in space-separated mode `constructor`, `toString` and
`__proto__` are legal symbol names, and on a plain object they are inherited
members. The tables that map symbols to values in `web/table.js` and
`web/sets.js` are created with `Object.create(null)`, and
`engine/tests/hostile_symbols.rs` runs a grammar made of such names under all
four methods.

## One gate, in one place

`scripts/check-dist.sh` is the mechanical check of the rules above, run on the
file that ships. It refuses the file unless all of these hold:

- **The exact strict policy appears once.** It is compared with
  `scripts/dist-csp.txt` line for line, is the only policy `<meta>` in the
  file, sits inside `<head>`, and comes before every `<script>`, `<style>` and
  `<link>`.
- **The build is complete:** the page scripts and the engine loader are
  present, the file ends with `</html>`, and no build marker is left over.
- **Nothing is loaded from outside:** no `<link>`, no `<script src=...>`, no
  `<iframe>`, `<frame>`, `<object>`, `<embed>` or `<base>`, no CSS `@import`,
  no `import` statement or `import()` call, no `javascript:` URL, no CSS
  `url()` other than a `#fragment` or a `data:` URI, and no absolute URL other
  than the SVG namespace name.
- **No injection sink:** `.innerHTML`, `.outerHTML`, `.insertAdjacentHTML`,
  `document.write`, `eval(`, `new Function`, `setTimeout` / `setInterval` with
  a string, `DOMParser`, `createContextualFragment`, `srcdoc`, and
  `setHTMLUnsafe` / `parseHTMLUnsafe`.
- **No storage:** `localStorage`, `sessionStorage`, `indexedDB`, cookies.
- **No network call:** `fetch(`, `XMLHttpRequest`, `WebSocket`, `EventSource`,
  `sendBeacon`, `importScripts`.

The rules match usage (`.innerHTML`, `eval(`), not the bare word, because a
comment in the source states the rule in prose. The script matches text, so it
is a tripwire for accidents, not a proof. The enforcement is the
Content-Security-Policy, which is why the gate checks the policy first and
exactly.

`build.sh` assembles the bundle in a staging directory, runs the gate on it,
and only then moves it to `dist/index.html`. A build that fails the gate leaves
the previous `dist/index.html` untouched, so a bundle with a sink in it cannot
be produced by accident. The GitHub Actions workflow runs the same script on
the file it is about to publish. `scripts/test-check-dist.sh` proves the gate
can fail: it makes a faulty copy of the bundle for each rule and requires that
the rule refuses it (77 cases at the time of the audit below).

## Automated scanning

Every push and pull request, and a weekly schedule, runs these in GitHub
Actions; any finding fails the run, and on the default branch the results are
uploaded to the repository's Security tab:

- **Trivy** — known vulnerabilities in `engine/Cargo.lock` and
  `tests/requirements.txt`, and committed secrets.
- **Semgrep** — static analysis of the Rust, JavaScript, Python and workflow
  files.
- **cargo-deny** — RustSec advisories, yanked crates, licences, and crate
  sources (crates.io only).
- **CodeQL** — GitHub's semantic analysis of the JavaScript, Python, Rust and
  workflows.
- **zizmor** — the workflows themselves: permissions, injection, pinning.

Every third-party action is pinned to a full commit SHA, every job starts with
no token permissions and asks only for what it needs, and Dependabot proposes
updates for the crates, the actions and the Python test dependency once a
release is a week old.

## No crashes, no hangs

The engine is written so that every fallible path returns a result rather than
aborting. It is compiled with these lints denied, and they are fixed rather
than suppressed:

```rust
#![deny(clippy::unwrap_used, clippy::expect_used,
        clippy::indexing_slicing, clippy::panic)]
```

Those lints exist only while clippy runs, so `build.sh` and the GitHub Actions
workflow both run `cargo clippy --locked --all-targets -- -D warnings` before
building: a warning stops the build.

Integer overflow checks stay enabled in release builds (`overflow-checks =
true` in `engine/Cargo.toml`), so a bad index can never silently wrap into
corrupt output.

Every loop that could otherwise run away is capped, and hitting a cap produces
an explanation naming the limit rather than a frozen tab. All caps live in one
file, `engine/src/limits.rs`. They fall into three kinds: what is read, what is
shown, and what is spent.

**Caps on what is read**

| Limit | Constant | Value |
|---|---|---|
| Grammar source text | `MAX_SOURCE_CHARS` | 20,000 characters |
| Productions (after expanding `\|`) | `MAX_PRODUCTIONS` | 200 |
| Symbols on one right-hand side | `MAX_RHS_LEN` | 50 |
| Distinct symbols | `MAX_SYMBOLS` | 200 |
| Length of one symbol name | `MAX_SYMBOL_LEN` | 32 characters |
| Test string length | `MAX_TEST_SYMBOLS` | 500 symbols |

**Caps on what is shown**

| Limit | Constant | Value |
|---|---|---|
| Warnings listed (the rest collapse into one "+N more" line) | `MAX_WARNINGS` | 50 |
| States in the canonical collection | `MAX_STATES` | 500 |
| Total steps emitted | `MAX_STEPS` | 20,000 |
| String-parse steps (four per test-string symbol, plus 100) | `MAX_PARSE_STEPS` | 2,100 |
| Depth of the parse stack (twice the longest test string) | `MAX_PARSE_STACK` | 1,000 |

**Cap on what is spent**

The caps above bound the result, not the time taken to reach it, so one more
bounds the work itself. A meter counts one unit for every item or lookahead
that the FIRST and FOLLOW fixpoints, closure, GOTO and merging by core handle,
and twenty units for each one that is placed in a step. It counts operations,
never milliseconds, so the same grammar is refused on every machine or on none.

| Limit | Constant | Value |
|---|---|---|
| Work one analysis may spend | `MAX_WORK` | 120,000,000 units |
| Work for a collection built only to fill a comparison row | `MAX_COMPARISON_WORK` | 60,000,000 units |

`MAX_COMPARISON_WORK` is the smaller budget for the other methods' collections
built on the side for the "Compare all four" view. When one is over budget, its
row is omitted and the analysis you asked for is unaffected.

One last constant, `MAX_FIXPOINT_ROUNDS` (10,000), is a safety valve on the
fixpoint loops, which converge in far fewer rounds; reaching it would mean a
bug, and it too ends in an error rather than a hang.

The test suite includes hostile inputs — symbols made of `<script>` markup,
byte-order marks and direction-control characters, symbols named like
JavaScript's inherited object members, a grammar thousands of lines long, and
grammars built to exceed each cap — with one test per constant in `limits.rs`.
Each asserts that the input is answered with a clean, explained error, or is
analysed within bounds, instead of panicking or hanging.

## The audit, run and recorded

Every claim above is a `grep` away. Here is that grep, and its result, run on
this tree after the last change to it.

**Date of this audit: 30 October 2026.** Re-run it before every release; the
commands are short enough to paste.

### The page

```
$ grep -rnE "\.(inner|outer)HTML|\.insertAdjacentHTML|document\.write *\(|\beval *\(|new Function|\bfetch *\(|XMLHttpRequest|localStorage|sessionStorage|indexedDB|new WebSocket|document\.cookie" web/*.js web/*.html web/*.css
$ echo $?
1
```

**Zero hits.** Two places in `web/` mention these words in prose rather than
use them: the comment at the top of `web/app.js`, which says the page never
uses `innerHTML`, `outerHTML` or `insertAdjacentHTML`, and the
`wasm-unsafe-eval` keyword inside the Content-Security-Policy in
`web/index.html`. There is no call site of any of them.

### The engine

```
$ grep -rn "unwrap(\|expect(\|panic!\|unreachable!\|todo!\|unimplemented!" engine/src/
$ echo $?
1
```

**Zero hits**, in the library and in the `dump` binary alike. `unwrap_or`,
`unwrap_or_else` and `unwrap_or_default` are used freely: they are total
functions and cannot panic. The test suite uses `unwrap` and `panic!` in plenty
of places, which is correct - a test that cannot fail loudly is not a test - and
those files are a separate crate that is never compiled into the page.

`cargo clippy --locked --all-targets -- -D warnings` reports no warnings. One
`#[allow]` exists in the whole engine, `clippy::too_many_arguments` on
`automaton::create_state`; it is a style lint, not one of the four safety lints
above, and none of those four is suppressed anywhere.

### The shipped file

```
$ grep -cE "\.innerHTML|\.outerHTML|insertAdjacentHTML *\(|document\.write *\(|\beval *\(|new Function|\bfetch *\(|XMLHttpRequest|localStorage|sessionStorage|indexedDB|new WebSocket|document\.cookie" dist/index.html
0
$ grep -oE "https?://[^\"' )]+" dist/index.html | sort -u
http://www.w3.org/2000/svg
$ grep -c "<script src=\|<link rel=\"stylesheet\"" dist/index.html
0
$ scripts/check-dist.sh dist/index.html
check-dist.sh: dist/index.html is clean (573945 bytes): strict CSP, splice complete, no outside reference, no injection sink, no storage, no network call.
```

The one URL in the whole file is the SVG namespace identifier. It is a name,
not an address: nothing is ever fetched from it, and `connect-src 'none'` would
stop the attempt if anything tried.

The bare words survive in the shipped file in the same two places as in `web/`
(the comment and the `wasm-unsafe-eval` keyword), plus the neutered code path
in the generated glue, whose error message reads "must never fetch".

`build.sh` runs the gate on every bundle before it is written, and the GitHub
Actions workflow re-runs it on the artifact it is about to publish.

### Opening it offline

The acceptance check for every release, performed on the built file:

```
$ firefox --new-window "file://$PWD/dist/index.html"       # with networking disabled
```

The page loads, analyses the default grammar and steps through all five phase
tabs and the "Compare all four" view with no server running and no network
interface up. This is not a nicety: browsers refuse to instantiate WebAssembly
over `file://` unless it arrives as bytes rather than as a fetch, which is
exactly why the engine is inlined as base64 and handed to `initSync`.

## What this document does not claim

It does not claim the analysis is free of bugs. It claims that a malformed or
malicious *input* cannot escape being treated as text. Correctness of the
parsing tables is a separate matter, addressed by the test suite in
`engine/tests/known_grammars.rs`, which checks the output of all four methods
against textbook-verified values, and `engine/tests/examples.rs`, which checks
that the handouts in `examples/` still state the numbers the engine produces.

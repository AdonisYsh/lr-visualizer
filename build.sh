#!/usr/bin/env bash
#
# Builds the engine to WASM and produces:
#   web/pkg/         the engine, for `python3 -m http.server -d web`
#   dist/index.html  one self-contained file: CSS, JS and the .wasm as base64
#
# Inlining is required: browsers refuse to load WebAssembly over file://.
# dist/index.html is only replaced by a bundle that passed scripts/check-dist.sh;
# a failed build leaves the previous one untouched.

set -euo pipefail

cd "$(dirname "$0")"
ROOT="$(pwd)"
CRATE="lr_engine"

say() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
die() { printf '\n\033[1;31mERROR: %s\033[0m\n' "$*" >&2; exit 1; }

# --------------------------------------------------------------- toolchain

for tool in cargo wasm-bindgen wasm-opt; do
  command -v "$tool" >/dev/null 2>&1 || die "$tool is not on PATH; see README.md, \"Running from source\"."
done
cargo clippy --version >/dev/null 2>&1 || die "cargo clippy is not available (rustup component add clippy)."

say "Checking the wasm-bindgen version pin"
# The CLI and the crate must be the same version, or bindgen fails with
# the opaque "invalid schema version".
CLI_VERSION="$(wasm-bindgen --version | awk '{print $2}')"
CRATE_VERSION="$(sed -n 's/^wasm-bindgen *= *"=\([0-9.]*\)".*/\1/p' engine/Cargo.toml | head -1)"
[ -n "$CRATE_VERSION" ] || die "Could not read the pinned wasm-bindgen version from engine/Cargo.toml."
if [ "$CLI_VERSION" != "$CRATE_VERSION" ]; then
  die "wasm-bindgen version mismatch.
    wasm-bindgen-cli (on PATH):             $CLI_VERSION
    wasm-bindgen crate (engine/Cargo.toml): $CRATE_VERSION
  These must be identical. Install the matching CLI with
    cargo install wasm-bindgen-cli --version $CRATE_VERSION --locked"
fi
printf '    wasm-bindgen %s on both sides. Good.\n' "$CLI_VERSION"

# -------------------------------------------------------------------- lint

say "Running clippy (the no-panic rules)"
# The no-panic lints in lib.rs are only enforced while clippy runs, so lint
# before building. --locked: use the versions in Cargo.lock or stop.
( cd engine && cargo clippy --locked --all-targets -- -D warnings )

# ------------------------------------------------------------------- build

say "Compiling the engine to wasm32-unknown-unknown (release)"
( cd engine && cargo build --locked --lib --release --target wasm32-unknown-unknown )

WASM_IN="engine/target/wasm32-unknown-unknown/release/${CRATE}.wasm"
[ -f "$WASM_IN" ] || die "Expected $WASM_IN to exist after the build."

# STAGE holds intermediate pieces; BUNDLE_STAGE holds the finished bundle while
# it is checked, inside dist/ so the final `mv` is an atomic rename.
# Both are removed on exit.
STAGE=""
BUNDLE_STAGE=""
cleanup() {
  [ -z "$STAGE" ] || rm -rf "$STAGE"
  [ -z "$BUNDLE_STAGE" ] || rm -rf "$BUNDLE_STAGE"
}
trap cleanup EXIT
mkdir -p dist
STAGE="$(mktemp -d)"
BUNDLE_STAGE="$(mktemp -d "$ROOT/dist/.stage.XXXXXX")"
BUNDLE="$BUNDLE_STAGE/index.html"

say "Running wasm-bindgen (--target web)"
wasm-bindgen --target web --no-typescript --out-dir "$STAGE" "$WASM_IN"

say "Optimising with wasm-opt -Oz"
wasm-opt -Oz -o "$STAGE/${CRATE}_opt.wasm" "$STAGE/${CRATE}_bg.wasm"
mv "$STAGE/${CRATE}_opt.wasm" "$STAGE/${CRATE}_bg.wasm"

say "Refreshing web/pkg for local development"
rm -rf web/pkg
mkdir -p web/pkg
cp "$STAGE/${CRATE}.js" "$STAGE/${CRATE}_bg.wasm" web/pkg/

# ------------------------------------------------- single-file distributable

say "Assembling the bundle (staged, not yet in dist/)"

WASM_B64="$STAGE/wasm.b64"
base64 -w0 "$STAGE/${CRATE}_bg.wasm" > "$WASM_B64"

# The glue is an ES module; inlined in a <script type="module"> it still works,
# since initSync is handed the bytes directly.
GLUE="$STAGE/glue.js"
cp "$STAGE/${CRATE}.js" "$GLUE"

# Neuter the glue's async fetch path (never taken) so the shipped file
# provably makes no network call.
sed -i 's|module_or_path = fetch(module_or_path);|throw new Error("This build is self-contained and must never fetch.");|' "$GLUE"

LOADER="$STAGE/loader.html"
{
  printf '<script type="module">\n'
  cat "$GLUE"
  printf '\n// ---- the engine itself, inlined so file:// works ----\n'
  printf 'const B64 = "'
  cat "$WASM_B64"
  printf '";\n'
  cat <<'LOADER_JS'

// ---- start-up: the engine failing to load and the page failing to start
// are different faults, each with its own try and its own message. ----

// Fallback for when app.js, which normally words these messages, is missing.
function startupMessage(tag, body, hint) {
  const box = document.getElementById('messages');
  if (!box) return;
  const msg = document.createElement('div');
  msg.className = 'msg error';
  const label = document.createElement('span');
  label.className = 'tag';
  label.textContent = tag + ': ';
  msg.appendChild(label);
  msg.appendChild(document.createTextNode(body));
  if (hint) {
    const more = document.createElement('span');
    more.className = 'hint';
    more.textContent = hint;
    msg.appendChild(more);
  }
  box.insertBefore(msg, box.firstChild);
}

// 1. The engine.
let engineLoaded = false;
try {
  const bin = atob(B64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  initSync({ module: bytes });
  engineLoaded = true;
} catch (e) {
  if (window.LRV && typeof window.LRV.bootFailed === 'function') {
    window.LRV.bootFailed(String(e));
  } else {
    startupMessage('Engine', 'The analysis engine could not be loaded.', String(e));
  }
}

// 2. The page. The engine is running, so anything thrown now is a bug in the
//    visualizer, reported through app.js when it is there.
if (engineLoaded) {
  try {
    window.LRV.boot(analyze);
  } catch (e) {
    if (window.LRV && typeof window.LRV.reportRenderError === 'function') {
      window.LRV.reportRenderError(e);
    } else {
      if (window.console && typeof window.console.error === 'function') {
        window.console.error('LR visualizer: internal rendering error', e);
      }
      startupMessage('Internal rendering error', (e && e.message) ? e.message : String(e),
        'This is a bug in the visualizer, not a problem with your grammar.');
    }
  }
}
LOADER_JS
  printf '</script>\n'
} > "$LOADER"

STYLE_BLOCK="$STAGE/style.html"
{ printf '<style>\n'; cat web/style.css; printf '\n</style>\n'; } > "$STYLE_BLOCK"

SCRIPT_BLOCK="$STAGE/scripts.html"
{
  printf '<script>\n'
  for f in app.js grammars.js sets.js table.js trace.js diagram.js compare.js; do
    printf '\n/* ===== web/%s ===== */\n' "$f"
    cat "web/$f"
  done
  printf '\n</script>\n'
} > "$SCRIPT_BLOCK"

# The bundle gets the strict CSP (scripts/dist-csp.txt, also read by
# scripts/check-dist.sh) instead of web/index.html's development one.
CSP_BLOCK="$STAGE/csp.html"
[ -s scripts/dist-csp.txt ] || die "scripts/dist-csp.txt is missing or empty; it holds the policy the bundle ships with."
{
  cat <<'CSP_COMMENT'
<!-- The grammar is data, never code. connect-src 'none' means this page cannot
     make a network request even if something went wrong. -->
CSP_COMMENT
  cat scripts/dist-csp.txt
} > "$CSP_BLOCK"

# Splice the four marked regions of web/index.html.
awk -v csp="$CSP_BLOCK" -v style="$STYLE_BLOCK" -v scripts="$SCRIPT_BLOCK" -v loader="$LOADER" '
  /<!-- CSP-START -->/     { skip=1; while ((getline line < csp)    > 0) print line; close(csp);    next }
  /<!-- CSP-END -->/       { skip=0; next }
  /<!-- STYLE-START -->/   { skip=1; while ((getline line < style)  > 0) print line; close(style);  next }
  /<!-- STYLE-END -->/     { skip=0; next }
  /<!-- SCRIPTS-START -->/ { skip=1; while ((getline line < scripts)> 0) print line; close(scripts); next }
  /<!-- SCRIPTS-END -->/   { skip=0; next }
  /<!-- WASM-START -->/    { skip=1; while ((getline line < loader) > 0) print line; close(loader);  next }
  /<!-- WASM-END -->/      { skip=0; next }
  skip != 1 { print }
' web/index.html > "$BUNDLE"

# ------------------------------------------------------------------ checks

say "Verifying the staged bundle"
# scripts/check-dist.sh, shared with the Pages workflow. Nothing is written to
# dist/index.html unless it passes.
scripts/check-dist.sh "$BUNDLE" \
  || die "The staged bundle failed scripts/check-dist.sh. dist/index.html has NOT been touched."

say "Installing dist/index.html"
mv -f "$BUNDLE" dist/index.html

SIZE_BYTES=$(wc -c < dist/index.html)

say "Done"
printf '    web/pkg/         engine for local development\n'
printf '    dist/index.html  %s bytes (%s)\n' \
  "$SIZE_BYTES" "$(du -h dist/index.html | cut -f1)"
printf '\n    Open it directly:  file://%s/dist/index.html\n' "$ROOT"
printf '    Or develop with:   python3 -m http.server -d web 8000\n\n'

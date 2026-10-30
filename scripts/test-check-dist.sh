#!/usr/bin/env bash
#
# Proves that scripts/check-dist.sh can fail.
#
#   scripts/test-check-dist.sh [path-to-index.html]     default: dist/index.html
#
#   1. the real bundle must pass;
#   2. legitimate prose appended must still pass (rules match usage, not words);
#   3. one copy per rule with one offending line appended must be refused:
#      exit 1, naming that rule, and no other rule firing;
#   4. one copy per structural fault (cut short, wrong or misplaced policy,
#      unspliced region) must be refused the same way.
#
# Every case works on a temporary copy; the bundle is never touched.

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
CHECK="$HERE/check-dist.sh"
SRC="${1:-$ROOT/dist/index.html}"

if [ ! -f "$SRC" ]; then
  echo "$SRC does not exist. Run ./build.sh first." >&2
  exit 2
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
COPY="$WORK/index.html"

PASSED=0
FAILED=0

ok() {
  PASSED=$((PASSED + 1))
  printf 'PASS  %s\n' "$1"
}

bad() {
  FAILED=$((FAILED + 1))
  printf 'FAIL  %s\n' "$1"
  shift
  printf '        %s\n' "$@"
}

# Run the gate on a file; leaves its output in OUT and its status in STATUS.
OUT=""
STATUS=0
gate() {
  STATUS=0
  OUT="$("$CHECK" "$@" 2>&1)" || STATUS=$?
}

# accepted <name>: the gate must pass $COPY.
accepted() {
  gate "$COPY"
  if [ "$STATUS" -eq 0 ]; then
    ok "accepted: $1"
  else
    bad "accepted: $1" "expected exit 0, got $STATUS:" "$OUT"
  fi
}

# refused <name> <text the refusal must contain> [number of findings, default 1;
# 'any' when the fault legitimately trips more than one rule].
refused() {
  local name="$1" want="$2" count="${3:-1}"
  gate "$COPY"
  if [ "$STATUS" -ne 1 ]; then
    bad "refused: $name" "expected exit 1, got $STATUS:" "$OUT"
  elif ! grep -qF -e "$want" <<< "$OUT"; then
    bad "refused: $name" "it was refused, but the refusal does not say: $want" "$OUT"
  elif [ "$count" != any ] && ! grep -qF -e "check-dist.sh: $count finding(s)" <<< "$OUT"; then
    bad "refused: $name" "expected exactly $count finding(s):" "$OUT"
  else
    ok "refused: $name"
  fi
}

fresh() { cp "$SRC" "$COPY"; }

# clean <line>: the bundle plus this line is still fine.
clean() {
  fresh
  printf '%s\n' "$1" >> "$COPY"
  accepted "$1"
}

# dirty <text the refusal must contain> <line>: the bundle plus this line is refused.
dirty() {
  fresh
  printf '%s\n' "$2" >> "$COPY"
  refused "$2" "$1"
}

# ------------------------------------------------------- 1. the real bundle

fresh
accepted "the bundle as built ($SRC)"

# ------------------------------------------- 2. prose and legitimate text pass

clean '// innerHTML / outerHTML / insertAdjacentHTML appear nowhere in web/.'
clean '// Nothing here evaluates the grammar, and this build must never fetch.'
clean '// No cookies, no storage of any kind, no document writing.'
clean "var ns = 'http://www.w3.org/2000/svg';"
clean 'var ns = "http://www.w3.org/2000/svg";'
clean ".edge { marker-end: url(#lrv-arrow); background: url(data:image/png;base64,AAAA); }"
clean 'window.setInterval(function () { step(1); }, 500);'
clean 'if (typeof Response === "function") { imports.wbg = {}; }'

# ------------------------------------------------------- 3. one line per rule

SINK='injection sink:'
dirty "$SINK .innerHTML"               'box.innerHTML = text;'
dirty "$SINK .innerHTML"               'box.outerHTML = text;'
dirty "$SINK .innerHTML"               "box.insertAdjacentHTML('beforeend', text);"
dirty "$SINK .innerHTML"               "box['innerHTML'] = text;"
dirty "$SINK document.write("          'document.write(text);'
dirty "$SINK document.write("          'document.writeln (text);'
dirty "$SINK eval("                    'eval(text);'
dirty "$SINK eval("                    'window.eval (text);'
dirty "$SINK eval("                    'window["eval"](text);'
dirty "$SINK new Function"             "var f = new Function('return 1');"
dirty "$SINK new Function"             "var f = Function('return 1');"
dirty "$SINK setTimeout / setInterval" 'setTimeout("go()", 10);'
dirty "$SINK setTimeout / setInterval" "window.setInterval('go()', 10);"
dirty "$SINK DOMParser"                'var p = new DOMParser();'
dirty "$SINK createContextualFragment" 'range.createContextualFragment(text);'
dirty "$SINK srcdoc"                   'frame.srcdoc = text;'
dirty "$SINK setHTMLUnsafe"            'box.setHTMLUnsafe(text);'
dirty "$SINK setHTMLUnsafe"            'Document.parseHTMLUnsafe(text);'

dirty 'storage: localStorage'          "localStorage.setItem('grammar', text);"
dirty 'storage: sessionStorage'        'var s = window.sessionStorage;'
dirty 'storage: indexedDB'             "indexedDB.open('lrv');"
dirty 'storage: document.cookie'       "document.cookie = 'seen=1';"
dirty 'storage: document.cookie'       "cookieStore.set('seen', '1');"

dirty 'network: fetch('                "fetch('grammar.txt');"
dirty 'network: fetch('                "window.fetch ('grammar.txt');"
dirty 'network: fetch('                "window['fetch']('grammar.txt');"
dirty 'network: XMLHttpRequest'        'var x = new XMLHttpRequest();'
dirty 'network: WebSocket'             'var w = new WebSocket(where);'
dirty 'network: EventSource'           'var e = new EventSource(where);'
dirty 'network: sendBeacon'            'navigator.sendBeacon(where, text);'
dirty 'network: importScripts'         "importScripts('worker.js');"

URL='network: an http://'
dirty "$URL"                           "var cdn = 'https://example.com/lib.js';"
dirty "$URL"                           '/* see http://example.com/ */'
dirty "$URL"                           "var feed = 'wss://example.com/feed';"
dirty "$URL"                           "var xhtml = 'http://www.w3.org/1999/xhtml';"
dirty "$URL"                           "var near = 'http://www.w3.org/2000/svg/extra';"
dirty "$URL"                           "var near = 'http://www.w3.org/2000/svgz';"

REF='external reference:'
dirty "$REF a <link>"                  '<link rel="stylesheet" href="style.css">'
dirty "$REF a <script> with a src"     '<script src="app.js"></script>'
dirty "$REF a <script> with a src"     '<SCRIPT type="module" SRC="app.js"></SCRIPT>'
dirty "$REF an element that loads"     '<iframe title="x"></iframe>'
dirty "$REF an element that loads"     '<base href="/">'
dirty "$REF an element that loads"     '<object data="x.swf"></object>'
dirty "$REF a CSS @import"             '@import "theme.css";'
dirty "$REF a static import"           "import init, { analyze } from './pkg/lr_engine.js';"
dirty "$REF a static import"           "  import './side-effect.js';"
dirty "$REF a dynamic import()"        "var m = import('./pkg/lr_engine.js');"
dirty "$REF a javascript: URL"         '<a href="javascript:void 0">x</a>'
dirty "$REF a CSS url()"               'body { background: url(paper.png); }'
dirty 'splice incomplete: a build marker' '<!-- WASM-START -->'
dirty 'splice incomplete: a build marker' '<!-- CSP-END -->'

# ------------------------------------------------------ 4. structural faults

CSP_LINE="$(cat "$HERE/dist-csp.txt")"
CSP='Content-Security-Policy:'

# Cut short: the last line, then a cut in the middle of the base64 engine.
fresh; head -n -1 "$SRC" > "$COPY"
refused 'the last line (</html>) is missing' "no '</html>' line"
head -c 300000 "$SRC" > "$COPY"
refused 'cut off after 300,000 bytes' "no '</html>' line" any
: > "$COPY"
refused 'an empty file' 'is empty' any
gate "$WORK/no-such-file.html"
if [ "$STATUS" -eq 1 ]; then ok 'refused: a file that does not exist'
else bad 'refused: a file that does not exist' "expected exit 1, got $STATUS:" "$OUT"; fi
gate
if [ "$STATUS" -eq 2 ]; then ok 'refused: called with no argument (exit 2)'
else bad 'refused: called with no argument' "expected exit 2, got $STATUS:" "$OUT"; fi

# The policy: loosened by one word, swapped for the development one, absent,
# doubled, joined by a second <meta>, and placed after the scripts it governs.
sed "s/connect-src 'none'/connect-src 'self'/" "$SRC" > "$COPY"
refused "the policy with connect-src 'self'" "$CSP the exact strict policy"
sed "s/script-src 'unsafe-inline'/script-src 'self' 'unsafe-inline'/" "$SRC" > "$COPY"
refused "the policy with 'self' in script-src" "$CSP the exact strict policy"

DEV_LINE="$(grep -F -e 'http-equiv="Content-Security-Policy"' "$ROOT/web/index.html" || true)"
if [ -z "$DEV_LINE" ] || [ "$DEV_LINE" = "$CSP_LINE" ]; then
  bad "the development policy from web/index.html" \
    "web/index.html should carry a development policy that differs from scripts/dist-csp.txt"
else
  grep -vFx -e "$CSP_LINE" "$SRC" > "$WORK/no-csp.html"
  { head -n 5 "$WORK/no-csp.html"; printf '%s\n' "$DEV_LINE"; tail -n +6 "$WORK/no-csp.html"; } > "$COPY"
  refused "the development policy from web/index.html, unsubstituted" "$CSP the exact strict policy"
fi

grep -vFx -e "$CSP_LINE" "$SRC" > "$COPY"
refused 'no policy at all' "$CSP the strict policy from scripts/dist-csp.txt is missing"
fresh; printf '%s\n' "$CSP_LINE" >> "$COPY"
refused 'the strict policy twice' "$CSP the exact strict policy"
fresh; printf '%s\n' '<meta http-equiv="refresh" content="0">' >> "$COPY"
refused 'a second http-equiv meta' "$CSP the strict policy is there, but so is another"
{ grep -vFx -e "$CSP_LINE" "$SRC"; printf '%s\n' "$CSP_LINE"; } > "$COPY"
refused 'the policy moved to after the scripts' 'after the first <script>/<style>/<link>' 2
# Before every script but inside <body>, where a browser ignores it. The
# inlined <style> is removed first so only the "outside <head>" rule fires.
sed '/^<style>$/,/^<\/style>$/d' "$SRC" | grep -vFx -e "$CSP_LINE" > "$WORK/no-style.html"
BODY_AT="$(grep -n -m1 -x -e '<body>' "$WORK/no-style.html" | cut -d: -f1)"
if [ -z "$BODY_AT" ]; then
  bad 'the policy placed inside <body>' 'could not find the <body> line in the bundle'
else
  { head -n "$BODY_AT" "$WORK/no-style.html"; printf '%s\n' "$CSP_LINE"
    tail -n +"$((BODY_AT + 1))" "$WORK/no-style.html"; } > "$COPY"
  refused 'the policy placed inside <body>' 'outside <head>'
fi

# The two names that prove the scripts and the loader were inlined.
sed 's/LRV\.boot/LRV.b00t/g' "$SRC" > "$COPY"
refused "no 'LRV.boot' anywhere" "'LRV.boot' is missing"
sed 's/initSync/initSink/g' "$SRC" > "$COPY"
refused "no 'initSync' anywhere" "'initSync' is missing"

# A file the gate cannot read is not a file it has checked.
if [ "$(id -u)" -ne 0 ]; then
  fresh; chmod 000 "$COPY"
  gate "$COPY"
  if [ "$STATUS" -ne 0 ]; then ok 'refused: an unreadable file'
  else bad 'refused: an unreadable file' "expected a non-zero exit, got 0:" "$OUT"; fi
  chmod 600 "$COPY"
fi

# A grep that *breaks* (status 2) must not be read as "found nothing": make
# every grep fail on a bundle that is in fact clean.
mkdir "$WORK/bin"
printf '#!/usr/bin/env bash\nexit 2\n' > "$WORK/bin/grep"
chmod +x "$WORK/bin/grep"
fresh
STATUS=0
OUT="$(PATH="$WORK/bin:$PATH" "$CHECK" "$COPY" 2>&1)" || STATUS=$?
if [ "$STATUS" -ge 2 ]; then ok "refused: grep itself failing is not read as 'clean' (exit $STATUS)"
else bad 'refused: grep itself failing' "expected an exit status of 2 or more, got $STATUS:" "$OUT"; fi

# -------------------------------------------------------------------- summary

echo
if [ "$FAILED" -eq 0 ]; then
  echo "test-check-dist.sh: OK, $PASSED cases - the gate passes the real bundle and refuses every fault."
else
  echo "test-check-dist.sh: FAILED, $FAILED of $((PASSED + FAILED)) cases."
  exit 1
fi

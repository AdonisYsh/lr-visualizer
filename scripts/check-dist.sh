#!/usr/bin/env bash
#
# Security gate for the shipped page (dist/index.html).
#
#   scripts/check-dist.sh dist/index.html
#
# Exit 0: clean. 1: findings (printed with their lines). 2: bad usage. Any
# other status is the script itself failing, which is also a refusal.
# build.sh runs it on the staged bundle and CI on the published file;
# scripts/test-check-dist.sh proves each rule can fail.
#
# A tripwire, not a proof: it matches text and would miss `window['fe' + 'tch']`.
# The enforcement is the CSP, so the exact policy is asserted and must come
# before every script.
#
# Rules match usage (`.innerHTML`, `eval(`), not the bare word: app.js's
# security comment says "innerHTML ... appear nowhere" and the neutered glue
# says "must never fetch". Names with no legitimate mention (localStorage,
# DOMParser, XMLHttpRequest ...) are matched bare.

set -euo pipefail
trap 'printf "check-dist.sh: internal error near line %s; treating the file as NOT checked.\n" "$LINENO" >&2' ERR

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# The shipped policy's single copy; build.sh splices it in, this demands it back.
CSP_FILE="$HERE/dist-csp.txt"

# The only absolute URL allowed: the SVG namespace, used as a name, never loaded.
ALLOWED_URLS=('http://www.w3.org/2000/svg')

if [ "$#" -ne 1 ]; then
  echo "usage: scripts/check-dist.sh <path-to-index.html>" >&2
  exit 2
fi
FILE="$1"

FINDINGS=0

# ------------------------------------------------------------------ helpers

# A finding: what is wrong, then the evidence (already formatted, may be empty).
finding() {
  FINDINGS=$((FINDINGS + 1))
  printf 'FAIL  %s\n' "$1"
  if [ -n "${2:-}" ]; then
    printf '%s\n' "$2"
  fi
}

# grep exits 0 (match), 1 (none) or >1 (grep itself failed). Reading >1 as
# "nothing found" would pass every bundle, so all searches go through here
# and status >1 stops the script.
SEARCH_OUT=""
search() {
  local status=0
  SEARCH_OUT="$(LC_ALL=C grep -a "$@" -- "$FILE")" || status=$?
  if [ "$status" -gt 1 ]; then
    echo "check-dist.sh: grep failed (status $status) on: grep -a $*" >&2
    exit 3
  fi
  return "$status"
}

# Up to five `line:text` matches, indented, each cut short: the base64 engine
# is one line of several hundred kilobytes.
evidence() {
  local shown=0 line out=""
  while IFS= read -r line; do
    shown=$((shown + 1))
    if [ "$shown" -gt 5 ]; then
      out+="        ... and more"$'\n'
      break
    fi
    out+="        line ${line%%:*}: ${line#*:}"$'\n'
  done <<< "$1"
  printf '%s' "${out%$'\n'}"
}

# forbid <what> <extended regex> [extra grep flags]: the pattern must not occur.
forbid() {
  local what="$1" pattern="$2"
  shift 2
  if search -noE "$@" -e "$pattern"; then
    finding "$what" "$(evidence "$(printf '%s\n' "$SEARCH_OUT" | cut -c1-120)")"
  fi
}

# require <what> <fixed string> [extra grep flags]: the string must occur.
require() {
  local what="$1" text="$2"
  shift 2
  if ! search -qF "$@" -e "$text"; then
    finding "$what"
  fi
}

# How many lines match (grep -c prints 0 and returns 1 when none do).
count_lines() {
  search -c "$@" || true
  printf '%s' "$SEARCH_OUT"
}

# The number of the first matching line, or nothing.
first_line() {
  if search -m1 -n "$@"; then
    printf '%s' "${SEARCH_OUT%%:*}"
  fi
}

# ------------------------------------------------------------ the file itself

if [ ! -f "$FILE" ] || [ ! -r "$FILE" ]; then
  echo "FAIL  $FILE does not exist or cannot be read."
  exit 1
fi
if [ ! -s "$FILE" ]; then
  echo "FAIL  $FILE is empty."
  exit 1
fi
if [ ! -s "$CSP_FILE" ]; then
  echo "check-dist.sh: $CSP_FILE is missing or empty; there is no policy to check against." >&2
  exit 3
fi
if [ "$(wc -l < "$CSP_FILE")" -ne 1 ]; then
  echo "check-dist.sh: $CSP_FILE must hold exactly one line, the shipped <meta> tag." >&2
  exit 3
fi
CSP_LINE="$(cat "$CSP_FILE")"

# Character classes used below. NAME is what may sit inside a JavaScript
# identifier, so [^NAME] before a word means "this is the whole name".
Q="[\"'\`]"
NAME='A-Za-z0-9_'
S='[[:space:]]'

# ---------------------------------------------------------- 1. the CSP, exactly

# The exact strict policy, once, and no other <meta> that sets a policy or redirects.
csp_count="$(count_lines -Fx -e "$CSP_LINE")"
if [ "$csp_count" -ne 1 ]; then
  if search -n -i -e 'http-equiv'; then
    finding "Content-Security-Policy: the exact strict policy from scripts/dist-csp.txt must appear exactly once (found $csp_count). The file carries this instead:" \
      "$(evidence "$(printf '%s\n' "$SEARCH_OUT" | cut -c1-320)")"
  else
    finding "Content-Security-Policy: the strict policy from scripts/dist-csp.txt is missing, and the file has no http-equiv meta at all."
  fi
else
  meta_count="$(count_lines -i -e 'http-equiv')"
  if [ "$meta_count" -ne 1 ]; then
    search -n -i -e 'http-equiv' || true
    finding "Content-Security-Policy: the strict policy is there, but so is another http-equiv line ($meta_count in all). Exactly one is allowed." \
      "$(evidence "$(printf '%s\n' "$SEARCH_OUT" | cut -c1-320)")"
  fi
  # A <meta> policy only governs what the parser reaches after it, and a
  # browser ignores one that is not inside <head> altogether.
  csp_at="$(first_line -Fx -e "$CSP_LINE")"
  active_at="$(first_line -i -E -e "<(script|style|link)($S|[>/]|\$)")"
  if [ -n "$active_at" ] && [ "$csp_at" -gt "$active_at" ]; then
    finding "Content-Security-Policy: the policy is on line $csp_at, after the first <script>/<style>/<link> on line $active_at. It must come first, or it does not cover them."
  fi
  head_end="$(first_line -i -E -e "</head>|<body($S|>|\$)")"
  if [ -n "$head_end" ] && [ "$csp_at" -gt "$head_end" ]; then
    finding "Content-Security-Policy: the policy is on line $csp_at, outside <head>, which ends on line $head_end. A browser ignores a policy <meta> placed there."
  fi
fi

# ----------------------------------------------------- 2. the splice completed

require "splice incomplete: 'LRV.boot' is missing, so the page scripts were not inlined." 'LRV.boot'
require "splice incomplete: 'initSync' is missing, so the engine loader was not inlined." 'initSync'
require "splice incomplete: no '</html>' line, so the file is cut short." '</html>' -x
forbid "splice incomplete: a build marker is still in the file, so a region of web/index.html was not replaced" \
  '<!-- *(CSP|STYLE|SCRIPTS|WASM)-(START|END) *-->'

# ------------------------------------------------------ 3. nothing from outside

# The file must be self-contained: everything it needs is inside it.
forbid "external reference: a <link> element (the stylesheet must be inlined)" \
  "<link($S|[>/]|\$)" -i
forbid "external reference: a <script> with a src attribute (every script must be inlined)" \
  "<script[^>]*${S}src$S*=" -i
forbid "external reference: an element that loads a document or sets the base URL (<iframe>, <frame>, <object>, <embed>, <base>)" \
  "<(iframe|frame|object|embed|base)($S|[>/]|\$)" -i
forbid "external reference: a CSS @import" '@import' -i
forbid "external reference: a static import statement (the dev loader's \`import ... from './pkg/...'\` must not ship)" \
  "^$S*import($S+[A-Za-z_\$*{\"'\`]|$S*[\"'\`{*])"
forbid "external reference: a dynamic import()" "(^|[^$NAME.\$])import$S*\\("
forbid "external reference: a javascript: URL" 'javascript:' -i

# Each absolute URL is compared whole against ALLOWED_URLS, so the SVG
# namespace passes but 'http://www.w3.org/2000/svg/x' does not.
if search -noiE -e "(https?|wss?|ftp)://[^[:space:]\"'\`<>()]*"; then
  bad_urls=""
  while IFS= read -r hit; do
    url="${hit#*:}"
    allowed=0
    for ok in "${ALLOWED_URLS[@]}"; do
      if [ "$url" = "$ok" ]; then allowed=1; fi
    done
    if [ "$allowed" -eq 0 ]; then bad_urls+="$hit"$'\n'; fi
  done <<< "$SEARCH_OUT"
  if [ -n "$bad_urls" ]; then
    # nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket
    finding "network: an http://, https://, ws://, wss:// or ftp:// URL other than the SVG namespace" \
      "$(evidence "$(printf '%s' "$bad_urls" | cut -c1-200)")"
  fi
fi

# CSS url(...) may only be a #fragment or a data: URI. Lower case only:
# the glue's `new URL(...)` builds an address and loads nothing.
if search -noE -e 'url\([^)]*\)?'; then
  bad_refs=""
  while IFS= read -r hit; do
    case "${hit#*:}" in
      'url(#'*|'url("#'*|"url('#"*|'url(data:'*|'url("data:'*|"url('data:"*) ;;
      *) bad_refs+="$hit"$'\n' ;;
    esac
  done <<< "$SEARCH_OUT"
  if [ -n "$bad_refs" ]; then
    finding "external reference: a CSS url() that is neither a #fragment nor a data: URI" \
      "$(evidence "$(printf '%s' "$bad_refs" | cut -c1-200)")"
  fi
fi

# ------------------------------------------------------------ 4. injection sinks

forbid "injection sink: .innerHTML / .outerHTML / .insertAdjacentHTML" \
  "\\.(innerHTML|outerHTML|insertAdjacentHTML)|$Q(innerHTML|outerHTML|insertAdjacentHTML)$Q"
forbid "injection sink: document.write( / document.writeln(" \
  "document$S*\\.$S*write(ln)?$S*\\("
forbid "injection sink: eval(" "(^|[^$NAME])eval$S*\\(|${Q}eval$Q"
forbid "injection sink: new Function / Function(" \
  "new$S+Function([^$NAME]|\$)|(^|[^$NAME])Function$S*\\("
forbid "injection sink: setTimeout / setInterval with a string argument" \
  "set(Timeout|Interval)$S*\\($S*$Q"
forbid "injection sink: DOMParser" 'DOMParser'
forbid "injection sink: createContextualFragment" 'createContextualFragment'
forbid "injection sink: srcdoc" 'srcdoc' -i
forbid "injection sink: setHTMLUnsafe / parseHTMLUnsafe" '(set|parse)HTMLUnsafe'

# -------------------------------------------------------------------- 5. storage

forbid "storage: localStorage" 'localStorage'
forbid "storage: sessionStorage" 'sessionStorage'
forbid "storage: indexedDB" 'indexedDB'
forbid "storage: document.cookie / cookieStore" "\\.cookie([^$NAME]|\$)|cookieStore"

# -------------------------------------------------------------------- 6. network

# build.sh neuters the one fetch( in wasm-bindgen's unused async init path;
# if a new version words it differently, this rule refuses the bundle.
forbid "network: fetch(" "(^|[^$NAME])fetch$S*\\(|${Q}fetch$Q"
forbid "network: XMLHttpRequest" 'XMLHttpRequest'
forbid "network: WebSocket" 'WebSocket'
forbid "network: EventSource" 'EventSource'
forbid "network: sendBeacon" 'sendBeacon'
forbid "network: importScripts" 'importScripts'

# ------------------------------------------------------------------ the verdict

if [ "$FINDINGS" -ne 0 ]; then
  printf '\ncheck-dist.sh: %s finding(s) in %s. This file must not ship.\n' "$FINDINGS" "$FILE"
  exit 1
fi
printf 'check-dist.sh: %s is clean (%s bytes): strict CSP, splice complete, no outside reference, no injection sink, no storage, no network call.\n' \
  "$FILE" "$(wc -c < "$FILE" | tr -d '[:space:]')"

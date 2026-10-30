"""Shared Playwright harness for the browser test suites.

Drives the built `dist/index.html` over `file://` in headless Chromium and Firefox
with the network blocked and watched. Each page records requests, console errors,
dialogs and injected markup (see `PageLog`); the runner fails a test that shows any.
"""

import argparse
import re
import sys
import time
import traceback
from dataclasses import dataclass, field
from pathlib import Path

from playwright.sync_api import Error as PlaywrightError
from playwright.sync_api import sync_playwright

# ------------------------------------------------------------------ constants

ROOT = Path(__file__).resolve().parents[2]
DIST = ROOT / 'dist' / 'index.html'
DIST_URL = DIST.as_uri()
# Screenshots and failure captures; created on first use.
OUT_DIR = Path(__file__).resolve().parent / 'out'

BROWSERS = ('chromium', 'firefox')
LOCAL_SCHEMES = ('file:', 'data:', 'blob:')
VARIANTS = ('lr0', 'slr1', 'lalr1', 'clr1')
# Tab ids as app.js builds them (`#tab-<id>`).
PHASE_TABS = ('grammar', 'sets', 'items', 'table', 'parse')
TABS = PHASE_TABS + ('compare',)
# The engine's `phase` field in emission order, and the tab that shows each.
PHASE_ORDER = ('grammar', 'sets', 'automaton', 'table', 'parse')
PHASE_OF_TAB = dict(zip(PHASE_TABS, PHASE_ORDER))

# What app.js's renderView draws in place of a view that threw.
RENDER_FAILURE_TEXT = 'This view could not be drawn at this step.'

DEFAULT_TIMEOUT_MS = 15_000
DEFAULT_VIEWPORT = {'width': 1400, 'height': 900}

# ------------------------------------------------------------- instrumentation

# Injected before any page script runs; observes only.
INIT_SCRIPT = r"""
(() => {
  if (window.__lrvTestLog) return;
  const log = { suspicious: [], csp: [], counterWrites: 0 };
  Object.defineProperty(window, '__lrvTestLog', { value: log });

  // Elements that can run code or fetch something. None of them is ever
  // created by the app after load, so any appearance is user text that was
  // parsed as markup somewhere.
  const RISKY = new Set(['script', 'img', 'image', 'iframe', 'frame', 'frameset',
    'object', 'embed', 'link', 'base', 'meta', 'form', 'audio', 'video',
    'source', 'track', 'style', 'template', 'foreignobject', 'animate', 'set',
    'use', 'portal', 'math']);

  function record(el, why) {
    let html = '';
    try { html = el.outerHTML.slice(0, 200); } catch (e) { html = String(el); }
    log.suspicious.push({
      tag: el.localName, why: why, readyState: document.readyState, html: html
    });
  }

  function inspect(el) {
    if (!el || el.nodeType !== 1) return;
    const tag = String(el.localName || '').toLowerCase();
    if (RISKY.has(tag)) record(el, 'risky element <' + tag + '>');
    for (const a of Array.from(el.attributes || [])) {
      if (/^on/i.test(a.name)) record(el, 'event-handler attribute ' + a.name);
      if (/^(href|src|xlink:href|action|formaction|srcdoc)$/i.test(a.name) &&
          /^\s*(javascript|vbscript|data:text\/html)/i.test(a.value)) {
        record(el, 'script URL in ' + a.name);
      }
    }
  }

  new MutationObserver(function (records) {
    for (const r of records) {
      if (r.type === 'childList') {
        for (const n of r.addedNodes) {
          if (n.nodeType !== 1) continue;
          inspect(n);
          if (n.querySelectorAll) n.querySelectorAll('*').forEach(inspect);
        }
      } else if (r.type === 'attributes') {
        // The step counter is the slider's aria-valuetext (renderPlayer sets
        // it on every render, so it moves on every run, even an error).
        if (r.attributeName === 'aria-valuetext' && r.target && r.target.id === 'scrub') {
          log.counterWrites++;
        }
        if (/^on/i.test(r.attributeName || '')) {
          record(r.target, 'event-handler attribute set: ' + r.attributeName);
        }
      }
    }
  }).observe(document, { childList: true, subtree: true, attributes: true });

  document.addEventListener('securitypolicyviolation', function (e) {
    log.csp.push({ directive: e.violatedDirective, blocked: e.blockedURI,
                   sample: e.sample, source: e.sourceFile });
  }, true);
})();
"""


def is_local_url(url):
    """True for URLs the page may load: file:, data: and blob: only."""
    return url.startswith(LOCAL_SCHEMES) or url in ('about:blank', 'about:srcdoc')


@dataclass
class PageLog:
    """Everything one page did that a test might need to fail on."""

    requests: list = field(default_factory=list)      # every `request` event
    blocked: list = field(default_factory=list)       # non-local URLs the route aborted
    failed: list = field(default_factory=list)        # (url, failure text) per `requestfailed`
    responses: list = field(default_factory=list)     # (url, status) per `response`
    console_errors: list = field(default_factory=list)
    console_all: list = field(default_factory=list)
    page_errors: list = field(default_factory=list)
    dialogs: list = field(default_factory=list)
    popups: list = field(default_factory=list)
    websockets: list = field(default_factory=list)

    def non_local_requests(self):
        """Every attempted non-local request, deduplicated (CSP-refused ones count too)."""
        seen = []
        for url in self.requests + self.blocked + self.websockets:
            if not is_local_url(url) and url not in seen:
                seen.append(url)
        return seen

    def past_csp(self):
        """Non-local requests that got past the CSP (Chromium reports CSP refusals as failure 'csp')."""
        csp_blocked = {u for u, why in self.failed if re.search(r'csp', why or '', re.I)}
        out = []
        for url in self.non_local_requests():
            if url in csp_blocked and url not in self.blocked:
                continue
            out.append(url)
        for url, status in self.responses:
            if not is_local_url(url) and url not in out:
                out.append(url)
        return out

    def problems(self, allow_console=(), allow_network=False):
        """Human-readable list of everything unexpected; empty means clean."""
        out = []
        if not allow_network:
            for url in self.non_local_requests():
                out.append('network request attempted: ' + url)
        for err in self.page_errors:
            out.append('pageerror: ' + err)
        for text in self.console_errors:
            if not any(re.search(p, text) for p in allow_console):
                out.append('console error: ' + text)
        for kind, msg in self.dialogs:
            out.append('dialog fired: %s(%r)' % (kind, msg))
        for url in self.popups:
            out.append('popup opened: ' + url)
        return out


@dataclass
class Outcome:
    """What the page shows after an analysis: success, or the error text."""

    ok: bool
    errors: list
    messages: str
    counter: str
    total: int
    seconds: float


class TestFailure(AssertionError):
    """Raised by `expect`; unlike `assert`, it survives `python -O`."""


def expect(condition, message):
    """Fail the current test with `message` unless `condition` holds."""
    if not condition:
        raise TestFailure(message)


# ------------------------------------------------------------------ the app


class App:
    """One open copy of dist/index.html, with helpers that drive it like a user."""

    def __init__(self, page, context, log, browser_name):
        self.page = page
        self.context = context
        self.log = log
        self.browser_name = browser_name
        self.notes = []
        self.script_baseline = 0
        self.suspicious_baseline = 0

    # -- lifecycle ---------------------------------------------------------

    def close(self):
        """Close this page's browser context (and with it the page)."""
        try:
            self.context.close()
        except PlaywrightError:
            pass

    def note(self, text):
        """Attach an informational line to this test's output."""
        self.notes.append(text)

    def wait_ready(self, timeout=DEFAULT_TIMEOUT_MS):
        """Wait until the engine has booted and its first analysis is drawn."""
        self.page.wait_for_function(
            """() => !!(window.LRV && window.__lrvTestLog &&
                  document.getElementById('variant-lr0') &&
                  (/^Step \\d+ of \\d+$/.test(document.getElementById('scrub').getAttribute('aria-valuetext')) ||
                   document.querySelector('#messages .msg.error')))""",
            timeout=timeout)
        boot_errors = self.errors()
        expect(not boot_errors, 'page did not boot cleanly: %s' % boot_errors)
        self.script_baseline = self.script_count()
        self.suspicious_baseline = self.page.evaluate(
            '() => window.__lrvTestLog.suspicious.length')

    # -- the form ----------------------------------------------------------

    def set_grammar(self, text, paste=None):
        """Replace the grammar textarea's contents (does not analyze).

        Long text is pasted, not typed: simulated typing is quadratic in line count."""
        self._set_field('#grammar', text, paste)

    def set_test_string(self, text, paste=None):
        """Replace the test-string input's contents (does not analyze)."""
        self._set_field('#teststring', text, paste)

    def _set_field(self, selector, text, paste):
        if paste is None:
            paste = len(text) > 2000
        if not paste:
            self.page.fill(selector, text)
            return
        self.page.evaluate(
            """([sel, text]) => { const f = document.querySelector(sel); f.value = text;
                                  f.dispatchEvent(new Event('input', { bubbles: true })); }""",
            [selector, text])

    def grammar(self):
        """The grammar textarea's current value."""
        return self.page.input_value('#grammar')

    def test_string(self):
        """The test-string input's current value."""
        return self.page.input_value('#teststring')

    def variant(self):
        """The id of the checked variant radio (lr0, slr1, lalr1, clr1)."""
        return self.page.evaluate(
            "() => (document.querySelector('#variants input:checked') || {}).value || null")

    def choose_variant(self, variant, timeout=DEFAULT_TIMEOUT_MS):
        """Select a variant radio; the page re-runs the analysis. Returns the Outcome."""
        if self.variant() == variant:
            return self.outcome()
        return self._run(lambda: self.page.check('#variant-' + variant), timeout)

    def preset_names(self):
        """The example menu's preset names, in menu order (placeholder excluded)."""
        return self.page.evaluate(
            """() => Array.from(document.querySelectorAll('#preset option'))
                 .filter(o => o.value !== '').map(o => o.textContent)""")

    def choose_preset(self, preset, timeout=DEFAULT_TIMEOUT_MS):
        """Pick an example by name or by index; the page analyzes it. Returns the Outcome."""
        if isinstance(preset, int):
            option = {'value': str(preset)}
        else:
            option = {'label': preset}
        return self._run(lambda: self.page.select_option('#preset', **option), timeout)

    def analyze(self, timeout=DEFAULT_TIMEOUT_MS):
        """Click Analyze and wait until the result (or error) is drawn."""
        return self._run(lambda: self.page.click('#analyze'), timeout)

    def _run(self, trigger, timeout):
        """Fire a control that re-runs the engine, then wait for the redraw.

        Every run rewrites the scrub's aria-valuetext, so the init script's write counter moving means it is on screen."""
        before = self.page.evaluate('() => window.__lrvTestLog.counterWrites')
        t0 = time.monotonic()
        trigger()
        self.page.wait_for_function(
            'n => window.__lrvTestLog.counterWrites > n', arg=before, timeout=timeout)
        out = self.outcome()
        out.seconds = time.monotonic() - t0
        return out

    def outcome(self):
        """The current analysis state as an Outcome (ok iff steps and no error)."""
        data = self.page.evaluate(
            """() => ({
                 errors: Array.from(document.querySelectorAll('#messages .msg.error'))
                           .map(e => e.textContent),
                 messages: document.getElementById('messages').textContent,
                 counter: document.getElementById('scrub').getAttribute('aria-valuetext') })""")
        pos = parse_counter(data['counter'])
        return Outcome(ok=pos is not None and not data['errors'], errors=data['errors'],
                       messages=data['messages'], counter=data['counter'],
                       total=pos[1] if pos else 0, seconds=0.0)

    # -- messages ----------------------------------------------------------

    def messages(self):
        """The full text of `#messages` (errors, warnings and notes)."""
        return self.page.text_content('#messages') or ''

    def errors(self):
        """The text of each error box in `#messages`."""
        return self.page.evaluate(
            """() => Array.from(document.querySelectorAll('#messages .msg.error'))
                 .map(e => e.textContent)""")

    def warnings(self):
        """The text of each warning or note box in `#messages`."""
        return self.page.evaluate(
            """() => Array.from(document.querySelectorAll('#messages .msg.warning, #messages .msg.note'))
                 .map(e => e.textContent)""")

    def symbols_text(self):
        """The side panel's 'Read as / Non-terminals / Terminals' text."""
        return self.page.text_content('#symbols') or ''

    # -- tabs --------------------------------------------------------------

    def current_tab(self):
        """The id of the selected tab (grammar, sets, items, table, parse, compare)."""
        return self.page.evaluate(
            """() => { const b = document.querySelector('#tabs [aria-selected="true"]');
                       return b ? b.id.replace(/^tab-/, '') : null; }""")

    def tab_available(self, tab):
        """Whether a tab's button is enabled right now."""
        return self.page.is_enabled('#tab-' + tab)

    def available_tabs(self):
        """The ids of every enabled tab, in strip order."""
        return [t for t in TABS if self.tab_available(t)]

    def tab(self, tab):
        """Click a tab if it is enabled; returns False (and does nothing) if not."""
        if not self.tab_available(tab):
            return False
        self.page.click('#tab-' + tab)
        self.page.wait_for_selector('#tab-%s[aria-selected="true"]' % tab, timeout=DEFAULT_TIMEOUT_MS)
        return True

    # -- the player --------------------------------------------------------

    def position(self):
        """(zero-based step index, total steps) from the scrub's aria-valuetext, or None."""
        return parse_counter(self.page.get_attribute('#scrub', 'aria-valuetext') or '')

    def _player(self, button):
        if not self.page.is_enabled(button):
            return None
        self.page.click(button)
        return self.position()

    def next(self):
        """Click Next; returns the new position, or None if Next is disabled."""
        return self._player('#btn-next')

    def prev(self):
        """Click Prev; returns the new position, or None if Prev is disabled."""
        return self._player('#btn-prev')

    def first(self):
        """Click First; returns the new position, or None if already there."""
        return self._player('#btn-first')

    def last(self):
        """Click Last; returns the new position, or None if already there."""
        return self._player('#btn-last')

    def press(self, key):
        """Press a player key (ArrowRight, ArrowLeft, Home, End, Space) on the stage."""
        self.page.focus('#view')
        self.page.keyboard.press(key)
        return self.position()

    def scrub_to(self, index):
        """Drag the step slider to `index`; returns the position.

        The page draws the stage in the next animation frame, so wait one before returning."""
        self.page.evaluate(
            """i => new Promise(resolve => {
                   const s = document.getElementById('scrub'); s.value = String(i);
                   s.dispatchEvent(new Event('input', { bubbles: true }));
                   requestAnimationFrame(() => resolve()); })""", index)
        return self.position()

    def walk(self, on_step=None, limit=25_000):
        """Click First, then Next to the end, calling `on_step(app, index)` at each step; returns a Walk."""
        result = Walk()
        self.first()
        expect(self.position() is not None, 'walk(): no analysis on screen')
        while True:
            seen = self.page.evaluate(
                """() => { const sel = document.querySelector('#tabs [aria-selected="true"]');
                           return { counter: document.getElementById('scrub').getAttribute('aria-valuetext'),
                                    tab: sel ? sel.id.replace(/^tab-/, '') : null,
                                    view: document.getElementById('view').textContent,
                                    explain: document.getElementById('explain').textContent }; }""")
            pos = parse_counter(seen['counter'])
            expect(pos is not None, 'walk(): counter reads %r' % seen['counter'])
            index, total = pos
            expect(index == len(result.tabs),
                   'walk(): expected step %d, the counter says %d of %d'
                   % (len(result.tabs), index, total))
            result.tabs.append(seen['tab'])
            result.text.setdefault(seen['tab'], []).append(seen['view'])
            result.explains.append(seen['explain'])
            if on_step:
                on_step(self, index)
            if index >= total - 1:
                break
            expect(len(result.tabs) <= limit, 'walk(): more than %d steps' % limit)
            # force=True skips Playwright's wait-for-stable; the counter check above proves each click landed.
            self.page.click('#btn-next', force=True)
        result.total = total
        return result

    def show_full_explanation(self, show=True):
        """Open (or close) the 'Show full explanation' disclosure under the player."""
        expanded = self.page.get_attribute('#explain-toggle', 'aria-expanded') == 'true'
        if expanded != show and self.page.is_visible('#explain-toggle'):
            self.page.click('#explain-toggle')

    def step_phase(self):
        """The current step's phase as the player labels it (grammar ... parse)."""
        return (self.page.text_content('#explain-phase') or '').strip()

    def end_of_phase(self, tab):
        """Jump to the last step of a phase tab's phase (binary search); None if the tab is disabled."""
        if not self.tab(tab):
            return None
        target = PHASE_ORDER.index(PHASE_OF_TAB[tab])
        lo, total = self.position()
        hi = total - 1
        while lo < hi:
            mid = (lo + hi + 1) // 2
            self.scrub_to(mid)
            if PHASE_ORDER.index(self.step_phase()) <= target:
                lo = mid
            else:
                hi = mid - 1
        self.scrub_to(lo)
        expect(self.current_tab() == tab,
               'end_of_phase(%s): landed on tab %s' % (tab, self.current_tab()))
        return self.position()

    # -- reading the stage -------------------------------------------------

    def view_text(self):
        """The current view's textContent (SVG text included)."""
        return self.page.evaluate("() => document.getElementById('view').textContent")

    def stage_html(self):
        """`#stage` outerHTML, read through evaluate (the ban is on web/, not tests)."""
        return self.page.evaluate("() => document.getElementById('stage').outerHTML")

    def snapshot(self):
        """Everything the player shows, as plain data, for before/after comparison."""
        return self.page.evaluate(
            """() => {
                 const q = s => document.querySelector(s);
                 const sel = q('#tabs [aria-selected="true"]');
                 return {
                   counter: q('#scrub').getAttribute('aria-valuetext'),
                   tab: sel ? sel.id.replace(/^tab-/, '') : null,
                   tabs: Array.from(document.querySelectorAll('#tabs button'))
                           .map(b => [b.id, b.disabled]),
                   view: q('#view').outerHTML,
                   explainSummary: q('#explain-summary').textContent,
                   explainFull: q('#explain-full').textContent,
                   messages: q('#messages').textContent,
                   buttons: ['first', 'prev', 'next', 'last', 'play', 'reset']
                              .map(b => [b, q('#btn-' + b).disabled])
                 };
               }""")

    def screenshot(self, name, selector=None, full_page=False):
        """Save a PNG to tests/browser/out/<name>-<browser>.png; returns its path."""
        OUT_DIR.mkdir(parents=True, exist_ok=True)
        path = OUT_DIR / ('%s-%s.png' % (name, self.browser_name))
        if selector:
            self.page.locator(selector).screenshot(path=str(path))
        else:
            self.page.screenshot(path=str(path), full_page=full_page)
        return path

    # -- injection checks --------------------------------------------------

    def script_count(self):
        """How many <script> elements the document has right now."""
        return self.page.evaluate('() => document.scripts.length')

    def injection_report(self):
        """Markup that should never exist, now or since load; empty means none."""
        found = self.page.evaluate(
            """() => {
                 const out = [];
                 const risky = document.querySelectorAll(
                   'img, iframe, frame, object, embed, form, audio, video, ' +
                   'img[src="x"], [onerror], [onload], foreignObject');
                 risky.forEach(e => out.push('present: ' + e.outerHTML.slice(0, 160)));
                 document.querySelectorAll('*').forEach(e => {
                   for (const a of Array.from(e.attributes)) {
                     if (/^on/i.test(a.name)) {
                       out.push('handler attribute ' + a.name + ' on <' + e.localName + '>');
                     }
                   }
                 });
                 return out;
               }""")
        if self.script_count() != self.script_baseline:
            found.append('script count changed: %d at boot, %d now'
                         % (self.script_baseline, self.script_count()))
        seen = self.page.evaluate(
            'n => window.__lrvTestLog.suspicious.slice(n)', self.suspicious_baseline)
        for entry in seen:
            found.append('appeared after load: %s (%s)' % (entry['why'], entry['html']))
        return found

    def csp_violations(self):
        """Every CSP securitypolicyviolation event since the page loaded."""
        return self.page.evaluate('() => window.__lrvTestLog.csp.slice()')


@dataclass
class Walk:
    """The record of one `App.walk()`: the tab at each step, view text per tab."""

    tabs: list = field(default_factory=list)
    text: dict = field(default_factory=dict)
    explains: list = field(default_factory=list)
    total: int = 0

    def text_of(self, tab):
        """All view text seen on `tab` during the walk, concatenated."""
        return '\n'.join(self.text.get(tab, []))

    def explain_text(self):
        """Every step's summary line and full explanation, concatenated."""
        return '\n'.join(self.explains)


def parse_counter(text):
    """'Step 3 of 41' -> (2, 41); anything else -> None."""
    m = re.match(r'^Step (\d+) of (\d+)$', (text or '').strip())
    return (int(m.group(1)) - 1, int(m.group(2))) if m else None


# --------------------------------------------------------------- page set-up


def launch(playwright, name):
    """Launch a headless browser by name ('chromium' or 'firefox')."""
    return getattr(playwright, name).launch(headless=True)


def open_app(browser, browser_name, viewport=None, color_scheme=None, device_scale_factor=None):
    """Open dist/index.html in a fresh, network-blocked context; returns a ready App.

    `device_scale_factor` can only be set when a context is made, hence a parameter."""
    expect(DIST.is_file(), '%s is missing - run ./build.sh first' % DIST)
    extra = {} if device_scale_factor is None else {'device_scale_factor': device_scale_factor}
    context = browser.new_context(
        viewport=viewport or DEFAULT_VIEWPORT,
        color_scheme=color_scheme or 'light',
        service_workers='block',
        offline=True,
        **extra,
    )
    log = PageLog()

    def route(route_, request):
        url = request.url
        if is_local_url(url):
            route_.continue_()
        else:
            log.blocked.append(url)
            route_.abort('blockedbyclient')

    context.route('**/*', route)
    context.add_init_script(script=INIT_SCRIPT)
    page = context.new_page()
    app = App(page, context, log, browser_name)
    watch(page, log, main=True)
    context.on('page', lambda p: (log.popups.append(p.url or '(blank)'), watch(p, log)))
    page.set_default_timeout(DEFAULT_TIMEOUT_MS)
    page.goto(DIST_URL)
    app.wait_ready()
    return app


def watch(page, log, main=False):
    """Attach the recording listeners for one page to `log`."""
    def on_console(msg):
        text = '%s: %s' % (msg.type, msg.text)
        log.console_all.append(text)
        if msg.type == 'error':
            log.console_errors.append(msg.text)

    def on_dialog(dialog):
        log.dialogs.append((dialog.type, dialog.message))
        try:
            dialog.dismiss()
        except PlaywrightError:
            pass

    page.on('request', lambda r: log.requests.append(r.url))
    page.on('requestfailed', lambda r: log.failed.append((r.url, r.failure)))
    page.on('response', lambda r: log.responses.append((r.url, r.status)))
    page.on('websocket', lambda ws: log.websockets.append(ws.url))
    page.on('console', on_console)
    page.on('pageerror', lambda e: log.page_errors.append(str(e)))
    page.on('crash', lambda _p: log.page_errors.append('page crashed'))
    page.on('dialog', on_dialog)


# -------------------------------------------------------------------- runner


@dataclass
class Registered:
    """One registered test and the checks it opted out of."""

    name: str
    fn: object
    browsers: tuple
    allow_console: tuple
    allow_network: bool


TESTS = []


def test(browsers=BROWSERS, allow_console=(), allow_network=False):
    """Decorator registering `fn(app)` to run once per browser in a fresh page.

    `allow_console` lists regexes of expected console errors; `allow_network=True` exempts the no-network check."""
    def register(fn):
        TESTS.append(Registered(fn.__name__, fn, tuple(browsers),
                                tuple(allow_console), allow_network))
        return fn
    return register


@dataclass
class RunTotals:
    """Whole-run tallies, printed after the PASS/FAIL lines."""

    passed: int = 0
    failed: int = 0
    requests_attempted: int = 0
    non_local: list = field(default_factory=list)
    deliberate: int = 0
    past_csp: list = field(default_factory=list)
    page_errors: int = 0
    console_errors: int = 0
    console_errors_allowed: int = 0
    dialogs: int = 0
    per_browser: dict = field(default_factory=dict)


def where_failed(error):
    """'at test_x.py:42 in fn' for the innermost suite frame of a failure."""
    here = Path(__file__).resolve().parent
    frames = [f for f in traceback.extract_tb(error.__traceback__)
              if Path(f.filename).resolve().parent == here and f.name != 'expect']
    if not frames:
        return 'at ' + (traceback.format_exc().strip().splitlines() or ['?'])[0]
    f = frames[-1]
    return 'at %s:%d in %s' % (Path(f.filename).name, f.lineno, f.name)


def run_one(browser, browser_name, reg, totals):
    """Run one registered test in one browser; prints its PASS/FAIL line."""
    reasons = []
    app = None
    t0 = time.monotonic()
    try:
        app = open_app(browser, browser_name)
        reg.fn(app)
    except TestFailure as e:
        reasons.append(str(e))
        reasons.append(where_failed(e))
    except Exception as e:  # noqa: BLE001 - any error is a test failure
        reasons.append('%s: %s' % (type(e).__name__, e))
        reasons.append(where_failed(e))
    finally:
        if app is not None:
            reasons.extend(app.log.problems(reg.allow_console, reg.allow_network))
            log = app.log
            if reg.allow_network:
                totals.deliberate += len(log.non_local_requests())
                totals.past_csp.extend(log.past_csp())
            else:
                totals.requests_attempted += len(log.requests)
                totals.non_local.extend(log.non_local_requests())
            totals.page_errors += len(log.page_errors)
            allowed = [t for t in log.console_errors
                       if any(re.search(p, t) for p in reg.allow_console)]
            totals.console_errors_allowed += len(allowed)
            totals.console_errors += len(log.console_errors) - len(allowed)
            totals.dialogs += len(log.dialogs)
            if reasons:
                try:
                    app.screenshot('fail-' + reg.name)
                except Exception:  # noqa: BLE001 - best effort only
                    pass
            app.close()
    seconds = time.monotonic() - t0
    ok = not reasons
    print('%s %s [%s] (%.1fs)' % ('PASS' if ok else 'FAIL', reg.name, browser_name, seconds))
    for line in (app.notes if app else []):
        print('    info: ' + line)
    for line in reasons:
        print('    ' + line)
    sys.stdout.flush()
    stats = totals.per_browser.setdefault(browser_name, [0, 0])
    if ok:
        totals.passed += 1
        stats[0] += 1
    else:
        totals.failed += 1
        stats[1] += 1
    return ok


def main(argv=None):
    """Run every registered test in each browser; returns the exit code (0 or 1)."""
    parser = argparse.ArgumentParser(description='Run the browser test suite.')
    parser.add_argument('--browser', action='append', choices=BROWSERS,
                        help='run in this browser only (repeatable)')
    parser.add_argument('-k', dest='pattern', help='only tests whose name contains this')
    parser.add_argument('--list', action='store_true', help='list tests and exit')
    args = parser.parse_args(argv)

    selected = [t for t in TESTS if not args.pattern or args.pattern in t.name]
    if args.list:
        for t in selected:
            print('%s  [%s]' % (t.name, ', '.join(t.browsers)))
        return 0
    if not selected:
        print('no tests selected')
        return 1

    browsers = tuple(args.browser) if args.browser else BROWSERS
    totals = RunTotals()
    t0 = time.monotonic()
    print('dist: %s' % DIST_URL)
    with sync_playwright() as pw:
        for name in browsers:
            tests = [t for t in selected if name in t.browsers]
            if not tests:
                continue
            browser = launch(pw, name)
            print('--- %s %s ---' % (name, browser.version))
            try:
                for reg in tests:
                    run_one(browser, name, reg, totals)
            finally:
                browser.close()
    seconds = time.monotonic() - t0

    print('--- summary ---')
    for name, (p, f) in totals.per_browser.items():
        print('%s: %d passed, %d failed' % (name, p, f))
    print('requests attempted: %d, of which non-local (not file:/data:/blob:): %d%s'
          % (totals.requests_attempted, len(totals.non_local),
             (' ' + ', '.join(totals.non_local)) if totals.non_local else ''))
    if totals.deliberate or totals.past_csp:
        print('deliberate non-local attempts by allow_network tests: %d, got past the CSP: %d'
              % (totals.deliberate, len(totals.past_csp)))
    print('page errors: %d, console errors: %d (plus %d explicitly allowed), dialogs: %d'
          % (totals.page_errors, totals.console_errors, totals.console_errors_allowed,
             totals.dialogs))
    print('%s: %d passed, %d failed in %.1fs'
          % ('OK' if not totals.failed else 'FAILED', totals.passed, totals.failed, seconds))
    return 0 if totals.failed == 0 else 1

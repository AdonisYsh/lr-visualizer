#!/usr/bin/env python3
"""Browser timings of dist/index.html over the performance corpus, as CSV.

    python3 tests/perf/render_bench.py [--out <file>] [--only <substring>]
                         [--browsers chromium,firefox] [--runs <n>] [--samples <n>]
                         [--timeout-secs <s>]

Six metrics per grammar x variant, timed in-page with performance.now(): click
to next frame (analyze, tab renders, single steps) and the bare WASM call.
"""

import argparse
import csv
import math
import statistics
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path

sys.dont_write_bytecode = True  # leave no __pycache__ behind in the tree
ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'tests' / 'browser'))

import harness  # noqa: E402 - the path tweak above must come first
from playwright.sync_api import Error as PlaywrightError  # noqa: E402
from playwright.sync_api import TimeoutError as PlaywrightTimeout  # noqa: E402
from playwright.sync_api import sync_playwright  # noqa: E402

# ------------------------------------------------------------------ constants

# Column layout of engine/src/bin/bench.rs; the two CSVs are concatenated, so do not reorder.
HEADER = ('grammar,family,productions,terminals,nonterminals,variant,environment,metric,'
          'runs,median_ms,min_ms,max_ms,outcome,states,steps,conflicts,json_bytes,detail')
COLUMNS = HEADER.split(',')

DEFAULT_OUT = ROOT / 'tests' / 'perf' / 'out' / 'browser.csv'

VARIANTS = harness.VARIANTS
# Row order within one grammar x variant.
METRICS = ('analyze_click', 'analyze_wasm', 'render_diagram_full', 'render_table_full',
           'render_step_items', 'render_step_table')
VIEWPORT = {'width': 1920, 'height': 1080}

DEFAULT_RUNS = 5
DEFAULT_SAMPLES = 24
DEFAULT_TIMEOUT_SECS = 120
# A measurement whose warm-up exceeds this gets SLOW_RUNS runs, the warm-up being the first.
SLOW_RUN_MS = 5_000.0
SLOW_RUNS = 2
# Single-step sampling thins out as one move gets slower: (warm-up ms, samples).
SAMPLE_STEPS = ((500.0, None), (1_500.0, 12), (5_000.0, 6), (math.inf, 3))
# An engine call shorter than this is repeated until a run lasts this long.
BATCH_MS = 50.0
MAX_CALLS_PER_RUN = 200
# How often the wait for an in-page job looks at the page.
POLL_MS = 20

# The two step-driven tabs that are timed, and the engine phase each one shows.
TAB_OF_METRIC = {'render_diagram_full': 'items', 'render_table_full': 'table',
                 'render_step_items': 'items', 'render_step_table': 'table'}

# ------------------------------------------------------------- instrumentation
# Injected into every page after boot. Jobs are polled, not awaited, because
# an evaluate on a page stuck in the engine could not be given a deadline.
LIB = r"""
(config) => {
  if (window.__lrvPerf) return;
  const $ = id => document.getElementById(id);
  const now = () => performance.now();
  const ORDER = config.phaseOrder;

  function need(condition, message) { if (!condition) throw new Error(message); }

  // Resolves once a frame has been produced: in a zero-delay task queued from
  // a requestAnimationFrame callback, which cannot run before the browser has
  // finished that frame's style, layout and paint. Resolves with the time the
  // callback started, so the frame's own work can be told from the wait for it.
  const afterFrame = () => new Promise(resolve => {
    requestAnimationFrame(() => {
      const begun = now();
      const channel = new MessageChannel();
      channel.port1.onmessage = () => { channel.port1.close(); resolve(begun); };
      channel.port2.postMessage(null);
    });
  });

  // The one interval every interaction metric uses; see the module docstring.
  async function timed(action) {
    await afterFrame();
    const t0 = now();
    action();
    const t1 = now();
    const begun = await afterFrame();
    const t2 = now();
    return { ms: t2 - t0, js: t1 - t0, frame: t2 - begun };
  }

  function position() {
    const m = /^Step (\d+) of (\d+)$/.exec(($('scrub').getAttribute('aria-valuetext') || '').trim());
    return m ? [Number(m[1]) - 1, Number(m[2])] : null;
  }
  function shownTab() {
    const b = document.querySelector('#tabs [aria-selected="true"]');
    return b ? b.id.replace(/^tab-/, '') : null;
  }
  function pageErrors() {
    return Array.from(document.querySelectorAll('#messages .msg.error')).map(e => e.textContent);
  }
  function setField(id, text) {
    const f = $(id);
    f.value = text;
    f.dispatchEvent(new Event('input', { bubbles: true }));
  }
  // What dragging the slider to `i` does: its value, then its input event.
  function scrub(i) {
    const s = $('scrub');
    s.value = String(i);
    s.dispatchEvent(new Event('input', { bubbles: true }));
  }
  // The slider draws the stage in the next animation frame (app.js
  // `renderStageNextFrame`), not inside the event, so a caller that has just
  // scrubbed waits two frames before it times anything: the page's own frame
  // callback was requested first, and the second tick proves the draw is done.
  const twoFrames = () => new Promise(resolve => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
  });
  function openTab(tab) {
    const b = $('tab-' + tab);
    need(b && !b.disabled, 'the ' + tab + ' tab is not available');
    b.click();
  }
  function at(index, tab) {
    const p = position();
    return !!p && p[0] === index && shownTab() === tab;
  }
  function where() {
    const p = position();
    return 'step ' + (p ? p[0] : '?') + ' on the ' + shownTab() + ' tab';
  }
  function checkDrawn() {
    need($('view').textContent.indexOf(config.failText) < 0,
         'the view reported that it could not be drawn at ' + where());
    need(pageErrors().length === 0, 'the page shows an error: ' + pageErrors().join(' | '));
  }
  // Counts of what the view put on screen, as evidence the whole thing is there.
  function drawn(tab) {
    const count = sel => document.querySelectorAll('#view ' + sel).length;
    if (tab === 'items') return { nodes: count('svg g.node[tabindex]'), edges: count('svg g.edge') };
    if (tab === 'table') return { rows: count('tbody tr'), filled: count('tbody td:not(.empty)') };
    return {};
  }

  const jobs = {};

  // Smallest gap performance.now() can report, over a 100 ms spin.
  jobs.resolution = () => {
    let last = now(), least = Infinity, t;
    const end = last + 100;
    while ((t = now()) < end) { if (t !== last) { least = Math.min(least, t - last); last = t; } }
    return least;
  };

  // Replay the opening example once so the page's own code has been compiled
  // and run before anything is timed.
  jobs.warmUi = () => {
    for (let n = 0; n < 3; n++) $('analyze').click();
    const p = position();
    need(p, 'the page opened without an analysis on screen');
    for (let i = 0; i < p[1]; i++) scrub(i);
    scrub(0);
    return p[1];
  };

  // Select the variant while a one-line grammar is in the box (selecting
  // re-runs the analysis, and that run should cost nothing), then put the
  // real grammar in. Nothing is analyzed yet.
  jobs.setup = ({ variant, grammar }) => {
    setField('grammar', 'S -> a');
    setField('teststring', '');
    const radio = $('variant-' + variant);
    need(radio, 'no radio button for ' + variant);
    if (!radio.checked) radio.click();
    const checked = document.querySelector('#variants input:checked');
    need(checked && checked.value === variant, 'the ' + variant + ' radio did not take');
    setField('grammar', grammar);
    return true;
  };

  jobs.analyzeClick = async () => {
    const t = await timed(() => $('analyze').click());
    t.errors = pageErrors();
    t.position = position();
    t.tab = shownTab();
    return t;
  };

  // The sizes the page states for the analysis on screen. Valid right after
  // Analyze: the Grammar tab at step 0 lists the productions as written,
  // before the augmented one is added.
  jobs.facts = () => {
    const facts = {};
    const p = position();
    if (p) facts.steps = p[1];
    document.querySelectorAll('#symbols dt').forEach(dt => {
      const m = /^Terminals \((\d+)\)$/.exec(dt.textContent);
      if (m) facts.terminals = Number(m[1]);
    });
    if (shownTab() === 'grammar' && p && p[0] === 0) {
      facts.productions = document.querySelectorAll('#view .prod-list li:not(.augmented)').length;
      // The side panel's list includes the start symbol the tool added; the
      // native bench counts the grammar's own. The Grammar tab names the one
      // to leave out.
      let start = null;
      document.querySelectorAll('#view dl.symbols dt').forEach(dt => {
        if (dt.textContent === 'Start symbol' && dt.nextElementSibling) {
          start = dt.nextElementSibling.textContent;
        }
      });
      const listed = Array.from(document.querySelectorAll('#symbols .chip.nt')).map(c => c.textContent);
      if (start !== null && listed.length) {
        facts.nonterminals = listed.filter(n => n !== start).length;
      }
    }
    const mine = document.querySelector('#summary .summary-one.current-variant .counts');
    const m = mine && /^(\d+)(?: conflicts?)? \/ (\d+)(?: states)?$/.exec(mine.textContent);
    if (m) { facts.conflicts = Number(m[1]); facts.states = Number(m[2]); }
    return facts;
  };

  // First and last step of every phase, by bisecting the slider on the phase
  // label the player shows. Done from the Compare tab, which the player never
  // leaves, so each probe redraws that small summary and not a phase view.
  jobs.ranges = () => {
    const p = position();
    need(p, 'no analysis on screen');
    const total = p[1];
    openTab('compare');
    need(shownTab() === 'compare', 'could not park on the Compare tab');
    const rank = i => {
      scrub(i);
      const r = ORDER.indexOf($('explain-phase').textContent.trim());
      need(r >= 0, 'step ' + i + ' has an unknown phase label');
      return r;
    };
    // The first step whose phase is `k` or later; `total` when there is none.
    const lower = k => {
      let lo = 0, hi = total;
      while (lo < hi) {
        const mid = (lo + hi) >> 1;
        if (rank(mid) >= k) hi = mid; else lo = mid + 1;
      }
      return lo;
    };
    const starts = ORDER.map((_, k) => lower(k));
    starts.push(total);
    const ranges = {};
    ORDER.forEach((phase, k) => {
      if (starts[k + 1] > starts[k]) ranges[phase] = [starts[k], starts[k + 1] - 1];
    });
    scrub(0);
    return { total: total, ranges: ranges };
  };

  // Park the player on `index` behind the Compare tab, then time the click
  // that opens `tab` there.
  jobs.fullRender = async ({ tab, index }) => {
    openTab('compare');
    scrub(index);
    need(at(index, 'compare'), 'could not park on step ' + index + ': at ' + where());
    const t = await timed(() => $('tab-' + tab).click());
    need(at(index, tab), 'the tab click landed on ' + where());
    checkDrawn();
    t.drawn = drawn(tab);
    return t;
  };

  // For each index: put the player on the step before it, then time Next.
  jobs.stepMoves = async ({ tab, indices }) => {
    openTab(tab);
    const out = [];
    for (const i of indices) {
      scrub(i - 1);
      await twoFrames();
      need(at(i - 1, tab), 'could not park on step ' + (i - 1) + ': at ' + where());
      const t = await timed(() => $('btn-next').click());
      need(at(i, tab), 'Next from step ' + (i - 1) + ' landed on ' + where());
      checkDrawn();
      t.index = i;
      out.push(t);
    }
    return out;
  };

  // ---- a second instance of the page's own engine -------------------------
  let engine = null;

  jobs.engineInit = () => {
    const loader = document.querySelector('script[type="module"]');
    const m = loader && /const B64 = "([A-Za-z0-9+\/=]+)"/.exec(loader.textContent);
    need(m, 'the inlined engine was not found in the page');
    const bin = atob(m[1]);
    const bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    const module = new WebAssembly.Module(bytes);
    // wasm-bindgen's glue for this engine supplies exactly one import. If a
    // rebuild ever adds another, refuse rather than guess what it should do.
    const wanted = WebAssembly.Module.imports(module);
    need(wanted.length === 1 && wanted[0].name === '__wbindgen_init_externref_table',
         'unexpected engine imports: ' + wanted.map(w => w.name).join(', '));
    let wasm = null;
    const imports = {};
    imports[wanted[0].module] = {
      __wbindgen_init_externref_table: function () {
        const table = wasm.__wbindgen_externrefs;
        const offset = table.grow(4);
        table.set(0, undefined);
        table.set(offset + 0, undefined);
        table.set(offset + 1, null);
        table.set(offset + 2, true);
        table.set(offset + 3, false);
      }
    };
    wasm = new WebAssembly.Instance(module, imports).exports;
    ['analyze', '__wbindgen_malloc', '__wbindgen_free', '__wbindgen_start', 'memory'].forEach(name => {
      need(wasm[name], 'the engine has no export named ' + name);
    });
    wasm.__wbindgen_start();

    const encoder = new TextEncoder();
    const decoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
    const pass = text => {
      const buf = encoder.encode(text);
      const ptr = wasm.__wbindgen_malloc(buf.length, 1) >>> 0;
      new Uint8Array(wasm.memory.buffer).subarray(ptr, ptr + buf.length).set(buf);
      return [ptr, buf.length];
    };
    // String in, string out: the same four steps as wasm-bindgen's `analyze`.
    engine = (text, variant, test) => {
      const a = pass(text), b = pass(variant), c = pass(test);
      const ret = wasm.analyze(a[0], a[1], b[0], b[1], c[0], c[1]);
      try {
        const ptr = ret[0] >>> 0;
        return [decoder.decode(new Uint8Array(wasm.memory.buffer).subarray(ptr, ptr + ret[1])), ret[1]];
      } finally {
        wasm.__wbindgen_free(ret[0], ret[1], 1);
      }
    };
    // Warm it on the page's opening example, under every method.
    const text = $('grammar').value;
    for (let n = 0; n < 10; n++) config.variants.forEach(v => engine(text, v, ''));
    return bytes.length;
  };

  // `calls` back-to-back engine calls; the mean per call. `inspect` also
  // reads the answer (outside the timed interval) for the row's counts.
  jobs.engineCall = ({ text, variant, calls, inspect }) => {
    need(engine, 'the engine instance was not created');
    let out = null;
    const t0 = now();
    for (let n = 0; n < calls; n++) out = engine(text, variant, '');
    const t1 = now();
    const result = { ms: (t1 - t0) / calls, bytes: out[1] };
    if (inspect) {
      const parsed = JSON.parse(out[0]);
      result.ok = parsed.ok !== false;
      if (!result.ok) {
        result.message = (parsed.error && parsed.error.message) || 'the engine returned an error';
      } else {
        result.steps = (parsed.steps || []).length;
        const rows = (parsed.verdict && parsed.verdict.comparison) || [];
        const mine = rows.filter(r => r.variant === variant)[0];
        if (mine) { result.states = mine.states; result.conflicts = mine.conflicts; }
      }
    }
    return result;
  };

  const perf = { job: null };
  perf.start = (name, arg) => {
    const job = { done: false };
    perf.job = job;
    setTimeout(async () => {
      try {
        job.value = await jobs[name](arg);
      } catch (e) {
        job.error = String((e && e.message) || e);
      }
      job.done = true;
    }, 0);
  };
  Object.defineProperty(window, '__lrvPerf', { value: perf });
}
"""

# ------------------------------------------------------------------ the corpus


@dataclass
class Entry:
    """One grammar file of the corpus."""

    stem: str      # the file stem, which is the grammar's name in the CSV
    family: str
    path: Path


def corpus(only=None):
    """The corpus in the native bench's order, filtered by `--only`; a missing family is an error."""
    perf = ROOT / 'tests' / 'perf' / 'grammars'
    entries = []
    for family, directory, prefix in (('textbook', ROOT / 'examples', ''),
                                      ('generated', perf, 'gen-'),
                                      ('pascal', perf, 'pascal-subset')):
        files = sorted(p for p in directory.glob('*.txt') if p.name.startswith(prefix))
        if not files:
            raise SystemExit('render_bench: no `%s*.txt` in %s' % (prefix, directory))
        entries.extend(Entry(p.stem, family, p) for p in files)
    return [e for e in entries if not only or only in e.stem]


# ------------------------------------------------------------------- one row


@dataclass
class Row:
    """One CSV row: a grammar x variant x metric in one browser."""

    entry: Entry
    variant: str
    environment: str
    metric: str
    samples: list = field(default_factory=list)
    outcome: str = 'ok'
    facts: dict = field(default_factory=dict)
    detail: str = ''

    def cells(self):
        """The row's eighteen fields, in HEADER order."""
        f = self.facts
        times = ['', '', '']
        if self.samples:
            times = ['%.3f' % statistics.median(self.samples),
                     '%.3f' % min(self.samples), '%.3f' % max(self.samples)]

        def count(name):
            return '' if f.get(name) is None else str(f[name])

        return [self.entry.stem, self.entry.family, count('productions'), count('terminals'),
                count('nonterminals'), self.variant, self.environment, self.metric,
                str(len(self.samples))] + times + [
                self.outcome, count('states'), count('steps'), count('conflicts'), '', self.detail]

    def progress_line(self):
        """One line for stderr, so a long run can be watched."""
        c = self.cells()
        if self.samples:
            times = 'median %9s ms  (min %s, max %s, n=%s)' % (c[9], c[10], c[11], c[8])
        else:
            times = 'no timing'
        return '%-14s %-26s %-5s %-19s %-7s %s%s' % (
            self.environment, self.entry.stem, self.variant, self.metric, self.outcome, times,
            ('  ' + self.detail) if self.outcome != 'ok' else '')


class Failed(Exception):
    """A measurement that ended as a row with outcome `timeout` or `error`."""

    def __init__(self, outcome, detail):
        super().__init__(detail)
        self.outcome = outcome
        self.detail = detail


def med(values):
    """Median, formatted the way the `detail` column carries it."""
    return '%.3f' % statistics.median(values)


def spread(first, last, samples):
    """The steps whose single-step move is timed: every step, or an even spread of `samples`."""
    span = last - first
    if span <= 0:
        return []
    if span <= samples:
        return list(range(first + 1, last + 1))
    picked = {first + math.ceil(k * span / samples) for k in range(1, samples + 1)}
    picked.add(last - 1)
    return sorted(picked)


def samples_for(warm_ms, default):
    """How many single-step moves to time, given how long the warm-up one took."""
    for limit, count in SAMPLE_STEPS:
        if warm_ms <= limit:
            return default if count is None else min(default, count)
    return default


# --------------------------------------------------------------- one browser


class Bench:
    """One browser, measured over the corpus one grammar at a time."""

    def __init__(self, playwright, name, opts):
        self.playwright = playwright
        self.name = name
        self.environment = name + '-wasm'
        self.opts = opts
        self.browser = None
        self.app = None
        # Kind of the open page: 'ui' or 'engine'.
        self.kind = None
        # (stem, variant) whose analysis is on screen, with its phase ranges.
        self.shown = None
        self.ranges = {}
        self.unexpected = []
        self.resolution = None

    # -- pages -------------------------------------------------------------

    def launch(self):
        """Start the browser, as the harness starts it for the test suites."""
        self.browser = harness.launch(self.playwright, self.name)
        return self.browser.version

    def quit(self):
        """Close the open page (checking its log) and the browser."""
        self.close_page()
        if self.browser is not None:
            try:
                self.browser.close()
            except PlaywrightError:
                pass
            self.browser = None

    def open_page(self, kind):
        """Make sure a healthy page of `kind` is open, warmed up and instrumented."""
        if self.app is not None and self.kind == kind:
            return
        self.close_page()
        try:
            app = harness.open_app(self.browser, self.name, viewport=VIEWPORT)
        except (PlaywrightError, harness.TestFailure) as first:
            # The browser itself may have gone down with the last page.
            try:
                self.browser.close()
            except PlaywrightError:
                pass
            try:
                self.launch()
                app = harness.open_app(self.browser, self.name, viewport=VIEWPORT)
            except (PlaywrightError, harness.TestFailure) as second:
                raise Failed('error', 'could not open the page: %s (after: %s)'
                             % (one_line(second), one_line(first)))
        app.page.evaluate(LIB, {'phaseOrder': list(harness.PHASE_ORDER),
                                'failText': harness.RENDER_FAILURE_TEXT,
                                'variants': list(VARIANTS)})
        self.app, self.kind, self.shown, self.ranges = app, kind, None, {}
        if self.resolution is None:
            self.resolution = self.job('resolution')
        self.job('warmUi' if kind == 'ui' else 'engineInit')

    def close_page(self):
        """Close a page that is believed healthy; anything in its log is unexpected."""
        app, self.app, self.kind, self.shown = self.app, None, None, None
        if app is None:
            return
        problems = app.log.problems()
        try:
            for v in app.csp_violations():
                problems.append('CSP violation: %s blocked %s' % (v.get('directive'), v.get('blocked')))
        except PlaywrightError as e:
            problems.append('could not read the page log: ' + one_line(e))
        self.unexpected.extend('[%s] %s' % (self.name, p) for p in problems)
        app.close()

    def discard_page(self):
        """Close a page a measurement failed on; returns its log for the row's `detail`."""
        app, self.app, self.kind, self.shown = self.app, None, None, None
        if app is None:
            return ''
        problems = app.log.problems()
        app.close()
        return '; '.join(problems)

    def job(self, name, arg=None, timeout=None):
        """Run one in-page job and return its value.

        Raises Failed('timeout') or Failed('error'); the page is closed either way."""
        limit = self.opts.timeout if timeout is None else timeout
        page = self.app.page
        try:
            page.evaluate('([name, arg]) => window.__lrvPerf.start(name, arg)', [name, arg])
            page.wait_for_function('() => window.__lrvPerf.job.done',
                                   timeout=limit * 1000, polling=POLL_MS)
            result = page.evaluate('() => window.__lrvPerf.job')
        except PlaywrightTimeout:
            said = self.discard_page()
            raise Failed('timeout', join_detail(
                '%s did not finish within %g s; the page was closed' % (name, limit), said))
        except PlaywrightError as e:
            said = self.discard_page()
            raise Failed('error', join_detail('%s: the page died: %s' % (name, one_line(e)), said))
        if 'error' in result:
            said = self.discard_page()
            raise Failed('error', join_detail('%s: %s' % (name, result['error']), said))
        return result.get('value')

    # -- one grammar -------------------------------------------------------

    def run_grammar(self, entry):
        """All 24 rows of one grammar in this browser, in CSV order."""
        text = entry.path.read_text(encoding='utf-8')
        rows = {}
        grammar_facts = {}

        # Interface first, then the engine on a separate page, so neither sees the other's garbage.
        for variant in VARIANTS:
            for row in self.measure_ui(entry, text, variant):
                rows[(variant, row.metric)] = row
                for name in ('productions', 'terminals', 'nonterminals'):
                    if row.facts.get(name) is not None:
                        grammar_facts[name] = row.facts[name]
        self.close_page()
        for variant in VARIANTS:
            rows[(variant, 'analyze_wasm')] = self.measure_wasm(entry, text, variant)
        self.close_page()

        ordered = []
        for variant in VARIANTS:
            for metric in METRICS:
                row = rows[(variant, metric)]
                for name, value in grammar_facts.items():
                    row.facts.setdefault(name, value)
                ordered.append(row)
        return ordered

    def row(self, entry, variant, metric):
        return Row(entry, variant, self.environment, metric)

    def timed_runs(self, warm, again, keep):
        """Take the timed runs after warm-up reading `warm`, storing each via `keep` as it arrives.

        A slow measurement gets SLOW_RUNS in all, the warm-up being the first."""
        more = self.opts.runs
        if warm['ms'] > SLOW_RUN_MS:
            keep(warm)
            more = min(more, SLOW_RUNS - 1)
        for _ in range(more):
            keep(again())

    # -- the engine alone --------------------------------------------------

    def measure_wasm(self, entry, text, variant):
        """`analyze_wasm` for one grammar x variant."""
        row = self.row(entry, variant, 'analyze_wasm')
        try:
            self.open_page('engine')
            call = {'text': text, 'variant': variant, 'calls': 1, 'inspect': True}
            warm = self.job('engineCall', call)
            for name in ('states', 'steps', 'conflicts'):
                row.facts[name] = warm.get(name)
            calls = 1
            if warm['ms'] < BATCH_MS:
                calls = min(MAX_CALLS_PER_RUN, math.ceil(BATCH_MS / max(warm['ms'], 0.05)))
            call.update(calls=calls, inspect=False)
            row.detail = 'result_bytes=%d;calls_per_run=%d' % (warm['bytes'], calls)
            if not warm['ok']:
                # The times are how long it took to give up.
                row.outcome = 'error'
                row.detail = join_detail(warm['message'], row.detail)
            self.timed_runs(warm, lambda: self.job('engineCall', call),
                            lambda reading: row.samples.append(reading['ms']))
        except Failed as f:
            row.outcome, row.detail = f.outcome, join_detail(f.detail, row.detail)
        return row

    # -- the interface -----------------------------------------------------

    def measure_ui(self, entry, text, variant):
        """The five interaction rows of one grammar x variant, in METRICS order."""
        key = (entry.stem, variant)
        click = self.row(entry, variant, 'analyze_click')
        facts = {}
        runs = []
        blocked = None   # why the render metrics cannot be measured, if they cannot
        try:
            self.open_page('ui')
            self.job('setup', {'variant': variant, 'grammar': text})
            warm = self.analyze_once()
            facts = self.job('facts')
            if warm['errors']:
                click.outcome = 'error'
                click.detail = 'the page shows: ' + ' | '.join(warm['errors'])
                blocked = Failed('error', 'not measured: the analysis ended in an error')
            self.timed_runs(warm, self.analyze_once, runs.append)
            if click.outcome == 'ok':
                click.detail = halves(runs)
                self.ranges = self.job('ranges')['ranges']
                self.shown = key
        except Failed as f:
            click.outcome, click.detail = f.outcome, f.detail
            blocked = Failed(f.outcome, 'not measured: Analyze did not get that far (%s)' % f.detail)
        click.samples = [r['ms'] for r in runs]
        click.facts = dict(facts)
        rows = [click]

        for metric in METRICS:
            if metric not in TAB_OF_METRIC:
                continue
            row = self.row(entry, variant, metric)
            row.facts = {k: v for k, v in facts.items() if k in ('states', 'steps', 'conflicts')}
            try:
                if blocked:
                    raise blocked
                self.show(key, text)
                if metric.startswith('render_step_'):
                    self.measure_steps(row, TAB_OF_METRIC[metric])
                else:
                    self.measure_full(row, TAB_OF_METRIC[metric])
            except Failed as f:
                row.outcome, row.detail = f.outcome, join_detail(f.detail, row.detail)
            rows.append(row)
        return rows

    def analyze_once(self):
        """One timed click on Analyze; the reading, with what the page then shows."""
        return self.job('analyzeClick')

    def show(self, key, text):
        """Bring `key`'s analysis back on screen if a failure cost us the page."""
        if self.app is not None and self.kind == 'ui' and self.shown == key:
            return
        self.open_page('ui')
        self.job('setup', {'variant': key[1], 'grammar': text})
        again = self.analyze_once()
        if again['errors']:
            raise Failed('error', 'not measured: the analysis ended in an error')
        self.ranges = self.job('ranges')['ranges']
        self.shown = key

    def phase_range(self, tab):
        """(first, last) step of the phase `tab` shows, as found on the page."""
        phase = harness.PHASE_OF_TAB[tab]
        if phase not in self.ranges:
            raise Failed('error', 'not measured: the analysis has no %s phase' % phase)
        return self.ranges[phase]

    def measure_full(self, row, tab):
        """`render_*_full`: open `tab` on the last step of its phase, from Compare."""
        _first, last = self.phase_range(tab)
        arg = {'tab': tab, 'index': last}
        warm = self.job('fullRender', arg)
        runs = []
        try:
            self.timed_runs(warm, lambda: self.job('fullRender', arg), runs.append)
        finally:
            row.samples = [r['ms'] for r in runs]
        drawn = ';'.join('%s=%d' % kv for kv in sorted(warm['drawn'].items()))
        row.detail = join_detail(halves(runs), 'step=%d' % last, drawn)

    def measure_steps(self, row, tab):
        """`render_step_*`: time Next over a spread of the phase `tab` shows."""
        first, last = self.phase_range(tab)
        if last <= first:
            raise Failed('error', 'not measured: the phase has a single step, so no move within it')
        warm = self.job('stepMoves', {'tab': tab, 'indices': [last]})[0]
        indices = spread(first, last, samples_for(warm['ms'], self.opts.samples))
        # Each sample is two renders (park, then move).
        budget = max(self.opts.timeout, 30 + 4 * len(indices) * warm['ms'] / 1000)
        moves = self.job('stepMoves', {'tab': tab, 'indices': indices}, timeout=budget)
        row.samples = [m['ms'] for m in moves]
        worst = max(moves, key=lambda m: m['ms'])
        row.detail = join_detail(halves(moves), 'range=%d-%d' % (first, last),
                                 'max_at_step=%d' % worst['index'])
        # Re-time the slowest move to tell a slow move from a busy browser.
        again = self.job('stepMoves', {'tab': tab, 'indices': [worst['index']]})[0]
        row.detail = join_detail(row.detail, 'max_again_ms=%.3f' % again['ms'])


def halves(readings):
    """`js_ms=..;frame_ms=..`: the medians of the two working parts of the interval."""
    if not readings:
        return ''
    return 'js_ms=%s;frame_ms=%s' % (med([r['js'] for r in readings]),
                                     med([r['frame'] for r in readings]))


def join_detail(*parts):
    """Non-empty parts joined with `;`, on one line."""
    return ';'.join(one_line(p) for p in parts if p)


def one_line(text):
    """An exception or message as a single line, for a CSV field."""
    return ' '.join(str(text).split())[:400]


# ---------------------------------------------------------------------- main


def parse_browsers(value):
    names = [b.strip() for b in value.split(',') if b.strip()]
    for name in names:
        if name not in harness.BROWSERS:
            raise argparse.ArgumentTypeError('unknown browser `%s` (choose from %s)'
                                             % (name, ', '.join(harness.BROWSERS)))
    if not names:
        raise argparse.ArgumentTypeError('no browser named')
    # Fixed order, whatever order was asked for.
    return [b for b in harness.BROWSERS if b in names]


def positive(value):
    n = int(value)
    if n <= 0:
        raise argparse.ArgumentTypeError('needs a whole number above zero')
    return n


def main(argv=None):
    """Measure the corpus in each browser and write the CSV; returns the exit code."""
    parser = argparse.ArgumentParser(
        description='Browser timings of dist/index.html over the performance corpus, as CSV.')
    parser.add_argument('--out', type=Path, default=DEFAULT_OUT,
                        help='where to write the CSV (default: tests/perf/out/browser.csv)')
    parser.add_argument('--only', help='only grammars whose name contains this')
    parser.add_argument('--browsers', type=parse_browsers, default=list(harness.BROWSERS),
                        help='comma-separated: chromium,firefox (default: both)')
    parser.add_argument('--runs', type=positive, default=DEFAULT_RUNS,
                        help='timed runs per metric after the warm-up (default %d)' % DEFAULT_RUNS)
    parser.add_argument('--samples', type=positive, default=DEFAULT_SAMPLES,
                        help='single-step moves timed per phase (default %d)' % DEFAULT_SAMPLES)
    parser.add_argument('--timeout-secs', dest='timeout', type=positive,
                        default=DEFAULT_TIMEOUT_SECS,
                        help='deadline for one timed action (default %d)' % DEFAULT_TIMEOUT_SECS)
    opts = parser.parse_args(argv)

    if not harness.DIST.is_file():
        print('render_bench: %s is missing - run ./build.sh first' % harness.DIST, file=sys.stderr)
        return 2
    entries = corpus(opts.only)
    if not entries:
        print('render_bench: --only matched no grammar in the corpus', file=sys.stderr)
        return 2

    opts.out.parent.mkdir(parents=True, exist_ok=True)
    unexpected = []
    counts = {}
    t0 = time.monotonic()
    with opts.out.open('w', encoding='utf-8', newline='') as handle, sync_playwright() as pw:
        # Bare newlines, as in the native bench.
        writer = csv.writer(handle, lineterminator='\n')
        handle.write(HEADER + '\n')
        for name in opts.browsers:
            bench = Bench(pw, name, opts)
            version = bench.launch()
            started = time.monotonic()
            try:
                for entry in entries:
                    for row in bench.run_grammar(entry):
                        print(row.progress_line(), file=sys.stderr, flush=True)
                        writer.writerow(row.cells())
                        counts[row.outcome] = counts.get(row.outcome, 0) + 1
                    # Flush per grammar so an interrupted run leaves a valid CSV.
                    handle.flush()
            finally:
                bench.quit()
            unexpected.extend(bench.unexpected)
            print('render_bench: %s %s done in %.0f s; performance.now() resolution %s ms'
                  % (name, version, time.monotonic() - started,
                     '?' if bench.resolution is None else '%g' % bench.resolution),
                  file=sys.stderr, flush=True)

    print('render_bench: %d rows (%s) in %.0f s -> %s'
          % (sum(counts.values()),
             ', '.join('%d %s' % (n, o) for o, n in sorted(counts.items())),
             time.monotonic() - t0, opts.out), file=sys.stderr)
    if unexpected:
        print('render_bench: FAILED - a healthy page did something it must never do:',
              file=sys.stderr)
        for line in unexpected:
            print('    ' + line, file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    raise SystemExit(main())

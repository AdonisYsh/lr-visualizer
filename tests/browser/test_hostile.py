"""Hostile input in a real browser: the grammar is data, never code.

Markup, absurd sizes, invisible characters and Object.prototype names go into
dist/index.html; no dialog, injected element, network call or console error
may result, and the CSP must be exactly the pinned one.

Run:  python3 tests/browser/test_hostile.py [--browser firefox] [-k name]
"""

import sys
import time
from pathlib import Path

sys.dont_write_bytecode = True  # leave no __pycache__ behind in the tree
sys.path.insert(0, str(Path(__file__).resolve().parent))

from harness import (  # noqa: E402 - the path tweak above must come first
    PHASE_TABS, RENDER_FAILURE_TEXT, TABS, VARIANTS, expect, main, test)

# The exact policy shipped in dist/index.html. Pinned on purpose: loosening a
# directive must fail this suite.
PINNED_CSP = ("default-src 'none'; script-src 'unsafe-inline' 'wasm-unsafe-eval'; "
              "style-src 'unsafe-inline'; img-src data:; font-src data:; "
              "connect-src 'none'; form-action 'none'; base-uri 'none'")


# ------------------------------------------------------------------- helpers

def check_no_injection(app, where):
    """No dialog so far, and nothing from the user's text became markup."""
    expect(not app.log.dialogs, '%s: dialog fired: %r' % (where, app.log.dialogs))
    found = app.injection_report()
    expect(not found, '%s: markup was injected:\n      %s' % (where, '\n      '.join(found)))


def check_no_render_failure(app, where):
    """No 'Internal rendering error' box and no view that failed to draw."""
    errors = [e for e in app.errors() if 'Internal rendering error' in e]
    expect(not errors, '%s: %s' % (where, errors))
    expect(RENDER_FAILURE_TEXT not in app.view_text(),
           '%s: the view could not be drawn' % where)


PARSE_STOPS_REASON = 'The table has conflicts, so the walkthrough stops at the table'
PARSE_OFF = '(parse tab disabled)'     # key in the texts dict: its title, when disabled


def visit_every_tab(app, where):
    """Click every enabled tab (Compare all four included); returns their texts."""
    texts = {}
    for tab in app.available_tabs():
        expect(app.tab(tab), '%s: could not open tab %s' % (where, tab))
        check_no_render_failure(app, '%s, tab %s' % (where, tab))
        texts[tab] = app.view_text()
    expect('compare' in texts, '%s: Compare all four was not available' % where)
    return texts


def open_a_table_cell_and_a_node(app, where):
    """Enter a filled table cell (jumps to Item Sets), then select a diagram node."""
    expect(app.end_of_phase('table') is not None, '%s: no table phase' % where)
    cells = app.page.locator('#view td.opens-state')
    expect(cells.count() > 0, '%s: the finished table has no filled cells' % where)
    cells.first.click()
    expect(app.current_tab() == 'items',
           '%s: a table cell did not open Item Sets (tab is %s)' % (where, app.current_tab()))
    check_no_render_failure(app, where + ', after opening a table cell')
    nodes = app.page.locator('#view g.node[tabindex="0"]')
    if nodes.count():
        nodes.first.click()
        check_no_render_failure(app, where + ', after selecting a diagram node')


def exercise_every_variant(app, where, variants=VARIANTS):
    """Analyze under each variant, walk every step, then poke every view.

    Returns {variant: (Outcome, Walk or None, {tab: text})}."""
    runs = {}
    for n, variant in enumerate(variants):
        here = '%s under %s' % (where, variant)
        if n == 0:
            app.page.check('#variant-' + variant)
            outcome = app.analyze()
        else:
            outcome = app.choose_variant(variant)
        expect(app.variant() == variant, '%s: variant radio not selected' % here)
        if not outcome.ok:
            runs[variant] = (outcome, None, {})
            continue
        app.show_full_explanation(True)
        walk = app.walk()
        check_no_injection(app, here + ', after walking every step')
        texts = visit_every_tab(app, here)
        # A conflicted table disables the Parse tab, for that reason only.
        if 'parse' not in texts:
            title = app.page.get_attribute('#tab-parse', 'title') or ''
            expect(PARSE_STOPS_REASON in title,
                   '%s: the Parse tab is disabled for another reason: %r' % (here, title))
            texts[PARSE_OFF] = title
        else:
            expect(PARSE_STOPS_REASON not in (app.page.get_attribute('#tab-parse', 'title') or ''),
                   '%s: the Parse tab is enabled but says the table has conflicts' % here)
        open_a_table_cell_and_a_node(app, here)
        check_no_injection(app, here + ', after visiting every view')
        runs[variant] = (outcome, walk, texts)
    return runs


# ------------------------------------------------------- markup as grammar

# `literal` must appear as text in the Grammar view; `marker` in every phase's
# view and the explanations. Every case also puts the markup in the test string.
MARKUP_CASES = [
    {
        'name': 'script_tag',
        'grammar': 'S -> <script>alert(1)</script> | x',
        'test': '<script>alert(1)</script>',
        'literal': 'S → <script>alert(1)</script>',
        'marker': '<script>alert(1)</script>',
    },
    {
        'name': 'img_onerror',
        'grammar': 'S -> <img src=x onerror=alert(1)> | x',
        'test': '<img src=x onerror=alert(1)>',
        'literal': 'S → <img src=x onerror=alert(1)>',
        'marker': '<img',
    },
    {
        'name': 'svg_onload_attribute_breakout',
        'grammar': 'S -> "><svg onload=alert(1)> | x',
        'test': '"><svg onload=alert(1)>',
        'literal': 'S → "><svg onload=alert(1)>',
        'marker': '"><svg',
    },
    {
        # Markup as non-terminal names too (GOTO columns, FIRST/FOLLOW rows,
        # parse stack, augmented start symbol).
        'name': 'markup_nonterminals',
        'grammar': ('<svg/onload=alert(1)> -> <img/src=x/onerror=alert(1)> <b>T</b> | x\n'
                    '<b>T</b> -> <i>y</i> | ε'),
        'test': '<img/src=x/onerror=alert(1)> <i>y</i>',
        'literal': '<svg/onload=alert(1)> → <img/src=x/onerror=alert(1)> <b>T</b>',
        'marker': '<b>T</b>',
    },
]


def markup_test(case):
    """Build the test function for one MARKUP_CASES entry."""
    def run(app):
        app.set_grammar(case['grammar'])
        app.set_test_string(case['test'])
        runs = exercise_every_variant(app, case['name'])
        for variant, (outcome, walk, texts) in runs.items():
            here = '%s under %s' % (case['name'], variant)
            expect(outcome.ok, '%s: expected the engine to accept it, got %r'
                   % (here, outcome.errors or outcome.counter))
            expect(case['literal'] in texts['grammar'],
                   '%s: %r not shown as text in the Grammar view' % (here, case['literal']))
            for tab in PHASE_TABS:
                if tab == 'parse' and PARSE_OFF in texts:
                    continue        # conflicted table; the reason was checked in the walk
                expect(case['marker'] in walk.text_of(tab),
                       '%s: %r never shown as text on tab %s' % (here, case['marker'], tab))
            expect(case['marker'] in walk.explain_text(),
                   '%s: %r not in any explanation' % (here, case['marker']))
            expect(case['marker'] in app.symbols_text(),
                   '%s: %r not in the side panel symbol list' % (here, case['marker']))
            if PARSE_OFF not in texts:
                parse_text = walk.text_of('parse')
                expect('accept' in parse_text.lower(),
                       '%s: the markup test string was not accepted' % here)
        app.note('accepted under %s; %s steps walked; no dialog, no element from user text'
                 % (', '.join(runs), '/'.join(str(r[1].total) for r in runs.values())))
    run.__name__ = 'markup_' + case['name'] if not case['name'].startswith('markup') else case['name']
    return run


for _case in MARKUP_CASES:
    test()(markup_test(_case))


@test()
def markup_test_string_rejected_by_the_parse(app):
    """Markup tokens that are terminals, in an order the grammar rejects."""
    app.set_grammar('S -> <script>alert(1)</script> | x')
    app.set_test_string('<script>alert(1)</script> <script>alert(1)</script>')
    runs = exercise_every_variant(app, 'rejected markup string', variants=('lr0', 'clr1'))
    for variant, (outcome, walk, _texts) in runs.items():
        expect(outcome.ok, '%s: expected an analysis, got %r' % (variant, outcome.errors))
        parse_text = walk.text_of('parse')
        expect('<script>alert(1)</script>' in parse_text,
               '%s: the rejected markup lookahead is not shown as text' % variant)
        expect('reject' in (parse_text + walk.explain_text()).lower(),
               '%s: the parse did not report a rejection' % variant)


@test()
def markup_test_string_that_is_not_a_terminal(app):
    """The engine refuses the test string; the refusal quotes it, as text."""
    app.set_grammar('S -> id')
    app.set_test_string('<img src=x onerror=alert(1)>')
    out = app.analyze()
    expect(not out.ok, 'expected an error, got %r' % out.counter)
    expect(len(out.errors) == 1, 'expected one error box, got %r' % out.errors)
    expect('`<img` is not a terminal in this grammar' in out.errors[0],
           'unexpected error text: %r' % out.errors[0])
    check_no_injection(app, 'not-a-terminal test string')
    app.note('error shown: %r' % out.errors[0])


# ----------------------------------------------------------- bad input sizes

@test()
def empty_grammar_is_a_clean_error(app):
    """An empty box, and a box with only blank lines and a comment."""
    for text in ('', '   \n\n// only a comment\n   '):
        app.set_grammar(text)
        app.set_test_string('')
        out = app.analyze()
        expect(not out.ok, 'expected an error for %r' % text)
        expect(out.errors and 'The grammar is empty.' in out.errors[0],
               'unexpected message for %r: %r' % (text, out.errors))
        expect(app.position() is None, 'the player still shows steps after an error')
        expect(app.available_tabs() == [], 'tabs still enabled after an error: %r'
               % app.available_tabs())
    app.note('message: %r' % out.errors[0])


@test()
def line_without_an_arrow_is_a_clean_error(app):
    """A line with no ->, → or ::= is reported with its line number."""
    app.set_grammar('S -> a S b | c\nS a b')
    out = app.analyze()
    expect(not out.ok, 'expected an error')
    expect(out.errors and out.errors[0].startswith('Line 2:')
           and 'No arrow on this line: `S a b`' in out.errors[0],
           'unexpected message: %r' % out.errors)
    app.note('message: %r' % out.errors[0])


@test()
def five_thousand_line_grammar_hits_the_limit(app):
    """5,000 productions: a limit error, quickly, and the page stays usable."""
    big = '\n'.join('S -> a%d' % i for i in range(5000))
    app.set_grammar(big)
    app.set_test_string('')
    out = app.analyze()
    expect(not out.ok, 'a 5,000-line grammar was accepted')
    expect(out.errors and 'over the 20000 character limit' in out.errors[0],
           'expected the source-size limit error, got %r' % out.errors)
    expect(out.seconds < 3.0, 'Analyze took %.2fs on 5,000 lines' % out.seconds)

    # Still responsive: a trivial script round trip, then a real analysis.
    t0 = time.monotonic()
    app.page.evaluate('() => 1 + 1')
    ping = time.monotonic() - t0
    expect(ping < 1.0, 'page took %.2fs to answer after the big grammar' % ping)
    again = app.choose_preset('Expression grammar (classic)')
    expect(again.ok and again.seconds < 3.0,
           'the next analysis failed or was slow: %r in %.2fs' % (again.errors, again.seconds))
    app.note('5,000 lines (%d chars): limit error in %.3fs; next analysis ok in %.3fs; %r'
             % (len(big), out.seconds, again.seconds, out.errors[0]))


@test()
def five_thousand_mostly_blank_lines_are_fine(app):
    """5,000 lines under the size cap (blank and comment lines) are just read."""
    lines = ['S -> a S b | c'] + ['' if i % 2 else '//' for i in range(4999)]
    app.set_grammar('\n'.join(lines))
    app.set_test_string('a c b')
    out = app.analyze()
    expect(out.ok, 'expected success, got %r' % out.errors)
    expect(out.seconds < 3.0, 'Analyze took %.2fs' % out.seconds)
    app.note('ok in %.3fs, %d steps' % (out.seconds, out.total))


@test()
def other_limits_are_clean_errors(app):
    """Too many productions, too long a symbol, too long a test string."""
    cases = [
        ('S -> ' + ' | '.join('t%d' % i for i in range(250)), '',
         'more than 200 productions'),
        ('S -> ' + 'a' * 40, '', 'is 40 characters long, over the 32 character limit'),
        ('S -> a', ' '.join(['a'] * 600), 'over the 500 symbol limit'),
    ]
    for grammar, test_string, wanted in cases:
        app.set_grammar(grammar)
        app.set_test_string(test_string)
        out = app.analyze()
        expect(not out.ok and out.errors and wanted in out.errors[0],
               'expected %r, got %r' % (wanted, out.errors or out.counter))
        expect(out.seconds < 3.0, '%r took %.2fs' % (wanted, out.seconds))


# --------------------------------------------------------- unusual symbols

# U+202E inside a symbol, U+200B inside one and as a whole symbol, a lookalike
# of `yz`, and some ordinary non-ASCII.
RLO, ZWSP = '‮', '​'
UNICODE_GRAMMAR = ('S -> ab%scd T | yz | y%sz | %s\n' % (RLO, ZWSP, ZWSP)) + 'T -> é | 名前'
UNICODE_TEST = 'ab%scd 名前' % RLO


def escaped(text):
    """Make invisible and direction characters visible in the report."""
    return text.encode('ascii', 'backslashreplace').decode('ascii')


# Wherever `symbol` is followed by more text on its line (document order, line =
# nearest block/cell/button/SVG <text>), the next visible character must be
# drawn right of the symbol's edge; otherwise the override has leaked.
# Zero-width characters are skipped on both sides.
BIDI_LEAK_JS = r"""
([selector, symbol]) => {
  const root = document.querySelector(selector);
  const out = [];
  if (!root) return out;
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const nodes = [];
  for (let n = walker.nextNode(); n; n = walker.nextNode()) nodes.push(n);
  const rect = (node, i) => {
    const r = document.createRange(); r.setStart(node, i); r.setEnd(node, i + 1);
    const b = r.getBoundingClientRect(); return b.width > 0 ? b : null;
  };
  const LINE = 'li, td, th, p, dd, dt, h2, h3, div, button, text';
  nodes.forEach((node, k) => {
    let from = 0;
    for (;;) {
      const at = node.data.indexOf(symbol, from);
      if (at < 0) break;
      from = at + symbol.length;
      let right = -Infinity, top = null;
      for (let i = at; i < from; i++) {
        const b = rect(node, i);
        if (!b) continue;
        right = Math.max(right, b.right);
        if (top === null) top = b.top;
      }
      if (top === null) continue;                    // not drawn at all
      const line = node.parentElement.closest(LINE);
      let next = null, nk = k, ni = from;
      while (line && nk < nodes.length && line.contains(nodes[nk])) {
        const data = nodes[nk].data;
        while (ni < data.length && /\s/.test(data[ni])) ni++;
        if (ni >= data.length) { nk++; ni = 0; continue; }
        next = rect(nodes[nk], ni);
        if (next) break;
        ni++;
      }
      if (!next || Math.abs(next.top - top) > 6) continue;   // nothing after it on this line
      out.push({ text: node.data.slice(0, 40), then: nodes[nk].data.slice(ni, ni + 12),
                 isolated: !!node.parentElement.closest('.sym, .chip, .set-member'),
                 leaked: next.left < right - 0.5 });
    }
  });
  return out;
}
"""


@test()
def bidi_and_zero_width_symbols(app):
    """Invisible and direction-overriding symbols: kept verbatim, shown as text."""
    app.set_grammar(UNICODE_GRAMMAR)
    app.set_test_string(UNICODE_TEST)
    out = app.analyze()
    expect(out.ok, 'expected the engine to accept it, got %r' % out.errors)
    app.note('messages: %s' % escaped(app.messages()))

    chips = app.page.evaluate(
        "() => Array.from(document.querySelectorAll('#symbols .chip')).map(c => c.textContent)")
    app.note('symbol chips: %s' % escaped(repr(chips)))
    for sym in ('ab%scd' % RLO, 'y%sz' % ZWSP, ZWSP, 'yz', 'é', '名前'):
        expect(sym in chips, 'symbol %s not listed verbatim in the side panel' % escaped(sym))

    walk = app.walk()
    for tab in PHASE_TABS:
        expect(RLO in walk.text_of(tab), 'U+202E missing from tab %s' % tab)
    check_no_injection(app, 'unicode symbols')

    style = app.page.evaluate(
        """() => { const c = document.querySelector('#symbols .chip');
                   const cs = getComputedStyle(c);
                   return cs.display + ', unicode-bidi: ' + cs.unicodeBidi +
                          ', direction: ' + cs.direction; }""")
    app.note('chip style: %s' % style)

    # The override must stop at its own symbol's edge in every view, the summary
    # line and the side panel. `checked` guards against passing over nothing.
    symbol = 'ab' + RLO + 'cd'
    shots = [app.screenshot('unicode-panel', selector='#panel')]
    checked = {}
    prose = {}
    app.show_full_explanation(True)
    for tab in PHASE_TABS:
        expect(app.end_of_phase(tab) is not None, 'tab %s unavailable' % tab)
        shots.append(app.screenshot('unicode-' + tab, selector='#view'))
        for where in ('#view', '#explain-summary', '#symbols'):
            lines = app.page.evaluate(BIDI_LEAK_JS, [where, symbol])
            checked['%s %s' % (tab, where)] = len(lines)
            leaked = [line for line in lines if line['leaked']]
            expect(not leaked,
                   'U+202E inside a symbol reorders the text after it on the %s tab, in %s: '
                   "'%s' is drawn before the end of '%s' (%d of %d places)"
                   % (tab, where, escaped(leaked[0]['then']) if leaked else '',
                      escaped(leaked[0]['text']) if leaked else '', len(leaked), len(lines)))
            bare = [line for line in lines if not line['isolated']]
            expect(not bare, 'on the %s tab, in %s, the symbol is joined into running text: '
                   "'%s'" % (tab, where, escaped(bare[0]['text']) if bare else ''))
    # The summary line on the two steps that put the symbol mid-line.
    app.first()
    expect(app.tab('parse'), 'the parse tab is unavailable')
    for _ in range(2):
        said = app.page.text_content('#explain-summary') or ''
        lines = app.page.evaluate(BIDI_LEAK_JS, ['#explain-summary', symbol])
        expect(lines, 'the summary line %r does not hold the symbol with text after it'
               % escaped(said))
        expect(not any(line['leaked'] for line in lines),
               'U+202E reorders the summary line %r' % escaped(said))
        checked['summary line'] = checked.get('summary line', 0) + len(lines)
        # The engine's explain is one string, so the page cannot isolate the
        # symbol there. Reported, not failed.
        full = app.page.evaluate(BIDI_LEAK_JS, ['#explain-full', symbol])
        prose[said.split(' ')[0]] = sum(1 for line in full if line['leaked'])
        app.next()
    for place in ('grammar #view', 'sets #view', 'items #view', 'parse #view',
                  'grammar #symbols'):
        expect(checked.get(place, 0) > 0,
               'nothing was checked in %s: the symbol is never followed by text there, '
               'or its characters could not be measured' % place)
    visit_every_tab(app, 'unicode symbols')
    app.note('U+202E stays inside its symbol in %d places where text follows it (%s)'
             % (sum(checked.values()),
                ', '.join('%s x%d' % (k, v) for k, v in checked.items() if v)))
    app.note('the engine\'s full explanation, one plain string, is still reordered after the '
             'symbol: %s' % ', '.join('"%s ..." step x%d' % kv for kv in prose.items()))
    app.note('screenshots: %s' % ', '.join(str(p) for p in shots))


@test()
def prototype_member_symbols_under_every_variant(app):
    """`constructor`, `toString`, `__proto__`... as symbols: no throw."""
    # Same grammar as engine/tests/hostile_symbols.rs.
    app.set_grammar('constructor -> toString __proto__ | valueOf\n'
                    'toString -> toString hasOwnProperty | hasOwnProperty\n'
                    '__proto__ -> valueOf | ε')
    app.set_test_string('hasOwnProperty hasOwnProperty valueOf')
    runs = exercise_every_variant(app, 'prototype names')
    for variant, (outcome, walk, texts) in runs.items():
        expect(outcome.ok, '%s: expected success, got %r' % (variant, outcome.errors))
        for tab in PHASE_TABS:
            if tab == 'parse' and PARSE_OFF in texts:
                continue            # conflicted table; the reason was checked in the walk
            expect(len(walk.text_of(tab)) > 0, '%s: tab %s never drew anything' % (variant, tab))
        for name in ('constructor', 'toString', '__proto__', 'hasOwnProperty', 'valueOf'):
            expect(name in walk.text_of('table') + walk.text_of('sets'),
                   '%s: %s missing from the sets/table views' % (variant, name))
        expect('__proto__' in texts['grammar'], '%s: __proto__ not in the Grammar view' % variant)
        expect(app.tab('compare'), '%s: Compare all four is unavailable' % variant)
        rows = app.page.locator('#view table.compare tbody tr').count()
        expect(rows == 4, '%s: Compare all four lists %d methods, not 4' % (variant, rows))
    app.note('steps per variant: %s' % ', '.join(
        '%s %d' % (v, r[1].total) for v, r in runs.items()))


# ------------------------------------------------------------------- the CSP

@test()
def csp_meta_is_pinned(app):
    """One CSP <meta>, in <head>, before every script, exactly the pinned string."""
    info = app.page.evaluate(
        """() => {
             const metas = Array.from(document.querySelectorAll(
               'meta[http-equiv="Content-Security-Policy" i]'));
             const m = metas[0];
             const firstScript = document.scripts[0];
             return {
               count: metas.length,
               content: m ? m.getAttribute('content') : null,
               inHead: m ? m.parentNode === document.head : false,
               beforeScripts: !!(m && firstScript &&
                 (m.compareDocumentPosition(firstScript) & Node.DOCUMENT_POSITION_FOLLOWING)),
               externalScripts: document.querySelectorAll('script[src]').length,
               externalLinks: document.querySelectorAll('link[href]').length,
               base: document.querySelectorAll('base').length
             };
           }""")
    expect(info['count'] == 1, 'expected one CSP meta, found %d' % info['count'])
    expect(info['content'] == PINNED_CSP,
           'CSP drifted from the pinned value:\n      got  %r\n      want %r'
           % (info['content'], PINNED_CSP))
    expect("connect-src 'none'" in info['content'], "connect-src 'none' missing")
    expect(info['inHead'], 'the CSP meta is not in <head>')
    expect(info['beforeScripts'], 'the CSP meta does not precede every script')
    expect(info['externalScripts'] == 0, 'a <script src> exists')
    expect(info['externalLinks'] == 0, 'a <link href> exists')
    expect(info['base'] == 0, 'a <base> element exists')


# The browsers' own CSP reports for the attempts the test below makes on purpose.
CSP_CONSOLE = (r'Content Security Policy', r'Content-Security-Policy', r'Refused to',
               r'Fetch API cannot load', r'violates the')


@test(allow_network=True, allow_console=CSP_CONSOLE)
def csp_is_enforced_not_just_present(app):
    """Ask the page to reach the network; CSP must stop every attempt before the network layer."""
    results = app.page.evaluate(
        """async () => {
             const base = 'https://example.invalid/';
             const out = {};
             const settle = p => Promise.race([p, new Promise(r => setTimeout(() => r('pending'), 1500))]);
             try { out.fetch = await settle(fetch(base + 'fetch').then(() => 'loaded', e => 'blocked: ' + e.name)); }
             catch (e) { out.fetch = 'threw: ' + e.name; }
             try {
               out.xhr = await settle(new Promise(r => {
                 const x = new XMLHttpRequest(); x.open('GET', base + 'xhr');
                 x.onload = () => r('loaded'); x.onerror = () => r('blocked: error event');
                 x.send(); }));
             } catch (e) { out.xhr = 'threw: ' + e.name; }
             try {
               out.websocket = await settle(new Promise(r => {
                 const w = new WebSocket('wss://example.invalid/ws');
                 w.onopen = () => r('loaded'); w.onerror = () => r('blocked: error event'); }));
             } catch (e) { out.websocket = 'threw: ' + e.name; }
             try {
               out.image = await settle(new Promise(r => {
                 const i = new Image(); i.onload = () => r('loaded');
                 i.onerror = () => r('blocked: error event'); i.src = base + 'img.png'; }));
             } catch (e) { out.image = 'threw: ' + e.name; }
             try { out.beacon = navigator.sendBeacon(base + 'beacon', 'x') ? 'queued' : 'blocked: refused'; }
             catch (e) { out.beacon = 'threw: ' + e.name; }
             try { out.script = await settle(import(base + 'm.js').then(() => 'loaded', e => 'blocked: ' + e.name)); }
             catch (e) { out.script = 'threw: ' + e.name; }
             await new Promise(r => setTimeout(r, 300));
             return out;
           }""")
    app.note('attempts: %s' % ', '.join('%s=%s' % kv for kv in sorted(results.items())))
    directives = sorted({v['directive'].split(' ')[0] for v in app.csp_violations()})
    app.note('CSP violations reported for: %s' % ', '.join(directives))
    if app.log.non_local_requests():
        app.note('seen as requests, then failed by CSP before the network: %s'
                 % ', '.join('%s (%s)' % f for f in app.log.failed))
    for api, result in results.items():
        expect(result != 'loaded', '%s reached the network: %s' % (api, result))
    reached = app.log.past_csp()
    expect(not reached, 'requests got past the CSP to the network layer: %r' % reached)
    expect('connect-src' in directives, 'no connect-src violation was reported')
    expect('img-src' in directives, 'no img-src violation was reported')
    check_no_injection(app, 'CSP enforcement')


# ------------------------------------------------------- the checks can fail

@test(allow_console=CSP_CONSOLE)
def harness_catches_a_real_injection(app):
    """Negative control: the test injects markup itself and the checks must fire."""
    app.page.evaluate(
        """() => { const t = document.createElement('template');
                   t.innerHTML = '<img src=x onerror=alert(1)>';
                   document.getElementById('view').appendChild(t.content); }""")
    app.page.wait_for_timeout(500)
    found = app.injection_report()
    expect(app.log.dialogs == [('alert', '1')],
           'the deliberate alert() was not seen as a dialog: %r' % app.log.dialogs)
    expect(any('<img' in f for f in found), 'injection_report missed the <img>: %r' % found)
    expect(any('onerror' in f for f in found), 'injection_report missed onerror: %r' % found)
    app.note('deliberate injection seen: dialog %r, %d report lines'
             % (app.log.dialogs[0], len(found)))
    # Expected here only; cleared so the global dialog check still means "none from the app".
    app.log.dialogs.clear()


if __name__ == '__main__':
    raise SystemExit(main())

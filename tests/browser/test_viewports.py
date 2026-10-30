"""The page at the window sizes a classroom actually has.

Measures the built dist/index.html over file:// at 1920x1080, 1366x768, 1024x768
and 150% zoom (emulated as 1280x720 at scale 1.5): no sideways scroll, reachable
controls, diagram and table fit or scroll in their box, panel layout.

Run:  python3 tests/browser/test_viewports.py [--browser firefox] [-k name]
"""

import sys
from pathlib import Path

sys.dont_write_bytecode = True  # leave no __pycache__ behind in the tree
sys.path.insert(0, str(Path(__file__).resolve().parent))

from harness import (PHASE_OF_TAB, PHASE_TABS, expect, main,  # noqa: E402
                     open_app, test)

EXPRESSION = 'Expression grammar (classic)'
CASUALTY = 'Merge casualty'
# (grammar, variant): the small matrix. A conflicted table disables the Parse
# tab; Merge casualty under LALR(1) is conflicted on purpose.
CONFLICTED = {(CASUALTY, 'lalr1')}
PARSE_STOPS_REASON = 'The table has conflicts, so the walkthrough stops at the table'
MATRIX = ((EXPRESSION, 'slr1'), (EXPRESSION, 'clr1'), (CASUALTY, 'lalr1'))

# (label, width, height, device scale factor or None for the browser's own)
VIEWPORTS = (
    ('1920x1080', 1920, 1080, None),
    ('1366x768', 1366, 768, None),
    ('1024x768', 1024, 768, None),
    ('zoom150-1280x720', 1280, 720, 1.5),
)

BREAKPOINT = 1100          # style.css: the panel stacks at or below this width
MIN_SCALE_PERCENT = 85     # diagram.js MIN_SCALE, as the toolbar reads it out
READABLE_PX = 14 * 0.85    # 14px node text, never zoomed below 85%
MAX_REPORTED = 40          # lines of failures printed per test

FIT = '#diagram-tools button[aria-label="Fit the whole automaton in view"]'
FOLLOW = '#diagram-tools button[aria-label="Follow the newest state"]'

CONTROLS = (('First', '#btn-first'), ('Prev', '#btn-prev'), ('Next', '#btn-next'),
            ('Last', '#btn-last'), ('Play', '#btn-play'), ('Reset', '#btn-reset'),
            ('show grammar', '#btn-grammar'), ('speed menu', '#speed-input'),
            ('slider', '#scrub'))

# ------------------------------------------------------------ in-page scripts

# The phase label of every step, read by moving the slider across the whole log.
PHASES_JS = r"""
() => {
  const scrub = document.getElementById('scrub');
  const total = Number(scrub.max) + 1;
  const out = [];
  for (let i = 0; i < total; i++) {
    scrub.value = String(i);
    scrub.dispatchEvent(new Event('input', { bubbles: true }));
    out.push(document.getElementById('explain-phase').textContent.trim());
  }
  return new Promise(resolve => requestAnimationFrame(() => resolve(out)));
}
"""

FRAMES_JS = r"""
n => new Promise(resolve => {
  const tick = k => k <= 0 ? resolve() : requestAnimationFrame(() => tick(k - 1));
  tick(n);
})
"""

# Scroll the stage to the top of the window (below the breakpoint the panel sits above it).
SCROLL_TO_STAGE_JS = r"""
() => { const s = document.getElementById('stage');
        window.scrollTo(0, s.getBoundingClientRect().top + window.scrollY);
        return window.scrollY; }
"""

PAGE_JS = r"""
() => {
  const R = e => {
    if (!e) return null;
    const r = e.getBoundingClientRect();
    return { left: r.left, top: r.top, right: r.right, bottom: r.bottom,
             width: r.width, height: r.height };
  };
  const de = document.documentElement;
  const hit = e => {
    const r = e.getBoundingClientRect();
    const t = document.elementFromPoint((r.left + r.right) / 2, (r.top + r.bottom) / 2);
    return t === e || e.contains(t) ? null : (t ? t.localName + (t.id ? '#' + t.id : '') : 'nothing');
  };
  const ctl = (name, sel) => {
    const e = document.querySelector(sel);
    return e ? { name: name, rect: R(e), coveredBy: hit(e) } : { name: name, rect: null };
  };
  const tabs = Array.from(document.querySelectorAll('#tabs button')).map(b =>
    ({ name: 'tab "' + b.textContent.trim() + '"', rect: R(b), coveredBy: hit(b) }));
  return {
    vw: window.innerWidth, vh: window.innerHeight,
    scrollWidth: de.scrollWidth, bodyScrollWidth: document.body.scrollWidth,
    controls: JSON.parse(JSON.stringify(CONTROLS_PLACEHOLDER)).map(c => ctl(c[0], c[1])).concat(tabs),
    tabbar: R(document.getElementById('tabs'))
  };
}
"""

LAYOUT_JS = r"""
() => {
  const R = e => { const r = e.getBoundingClientRect();
    return { left: r.left, top: r.top, right: r.right, bottom: r.bottom,
             width: r.width, height: r.height }; };
  const toggle = document.getElementById('panel-toggle');
  return { vw: window.innerWidth, vh: window.innerHeight,
           scrollWidth: document.documentElement.scrollWidth,
           panel: R(document.getElementById('panel')),
           stage: R(document.getElementById('stage')),
           view: R(document.getElementById('view')),
           expanded: toggle.getAttribute('aria-expanded'),
           bodyHidden: document.getElementById('panel-body').hidden };
}
"""

ITEMS_JS = r"""
() => {
  const R = e => { if (!e) return null; const r = e.getBoundingClientRect();
    return { left: r.left, top: r.top, right: r.right, bottom: r.bottom,
             width: r.width, height: r.height }; };
  const svg = document.querySelector('#diagram-scroll svg');
  if (!svg) return null;
  const vb = svg.viewBox.baseVal;
  const scale = svg.getBoundingClientRect().width / vb.width;
  const small = [];
  let nodeTexts = 0;
  for (const t of svg.querySelectorAll('g.node text, g.node tspan')) {
    if (!t.textContent.trim()) continue;
    nodeTexts++;
    const px = parseFloat(getComputedStyle(t).fontSize) * scale;
    small.push({ what: t.localName + (t.getAttribute('class') ? '.' + t.getAttribute('class') : ''),
                 text: t.textContent.slice(0, 20), px: px });
  }
  const zoom = document.querySelector('#diagram-tools .zoom-readout');
  const cur = document.querySelector('#diagram-scroll g.node.current rect.frame');
  return { svg: R(svg), scale: scale, nodeTexts: nodeTexts, texts: small,
           scroller: R(document.getElementById('diagram-scroll')),
           frame: R(document.getElementById('diagram-frame')),
           view: R(document.getElementById('view')),
           zoom: zoom ? zoom.textContent : null, current: R(cur) };
}
"""

# Waits for Follow's smooth scroll: true once the current state is in its box, or after `ms`.
FOLLOW_SETTLED_JS = r"""
() => {
  const sc = document.getElementById('diagram-scroll');
  const n = document.querySelector('#diagram-scroll g.node.current rect.frame');
  if (!sc || !n) return true;
  const a = sc.getBoundingClientRect(), b = n.getBoundingClientRect();
  return b.left >= a.left - 2 && b.right <= a.right + 2 &&
         b.top >= a.top - 2 && b.bottom <= a.bottom + 2;
}
"""

TABLE_JS = r"""
() => {
  const R = e => { if (!e) return null; const r = e.getBoundingClientRect();
    return { left: r.left, top: r.top, right: r.right, bottom: r.bottom,
             width: r.width, height: r.height }; };
  const sc = document.querySelector('#view .scroller');
  if (!sc) return null;
  const cell = document.querySelector('#view table.grid td.current');
  return { scroller: R(sc), view: R(document.getElementById('view')),
           table: R(sc.querySelector('table')),
           scrollW: sc.scrollWidth, clientW: sc.clientWidth,
           overflowX: getComputedStyle(sc).overflowX,
           cell: R(cell), cellText: cell ? cell.textContent : null,
           vw: window.innerWidth };
}
"""


def _controls_js():
    items = ', '.join("['%s', '%s']" % c for c in CONTROLS)
    return PAGE_JS.replace('JSON.parse(JSON.stringify(CONTROLS_PLACEHOLDER))', '[%s]' % items)


PAGE_SCRIPT = _controls_js()

# ------------------------------------------------------------------- helpers


def frames(app, n=2):
    """Wait `n` animation frames: a resize or a Fit is acted on in the next one."""
    app.page.evaluate(FRAMES_JS, n)


def inside(rect, vw, vh, slack=0.5):
    """Whether a {left, top, right, bottom} box lies wholly in the window."""
    return (rect['left'] >= -slack and rect['top'] >= -slack and
            rect['right'] <= vw + slack and rect['bottom'] <= vh + slack)


def within(inner, outer, slack=1.0):
    """Whether `inner` lies inside `outer`, give or take a pixel."""
    return (inner['left'] >= outer['left'] - slack and inner['right'] <= outer['right'] + slack and
            inner['top'] >= outer['top'] - slack and inner['bottom'] <= outer['bottom'] + slack)


class Report:
    """The failures of one test, each with the full address of where it happened."""

    def __init__(self, app, window):
        self.app = app          # the registered app: notes go on it
        self.window = window
        self.problems = []
        self.where = ''
        self.counts = {}        # what was actually checked, for the closing note
        self.shots = 0

    def at(self, grammar, variant, tab, step, total):
        """Set the address the next failures are reported at."""
        self.where = '[%s %s | %s %s | %s tab | step %d of %d]' % (
            self.app.browser_name, self.window, grammar, variant, tab, step + 1, total)

    def count(self, what, n=1):
        """Tally something that was checked."""
        self.counts[what] = self.counts.get(what, 0) + n

    def fail(self, current, message, shot=None):
        """Record a failure; keep one screenshot of the first few."""
        self.problems.append('%s %s' % (self.where, message))
        if shot and self.shots < 6:
            self.shots += 1
            try:
                path = current.screenshot('viewport-fail-%s-%s' % (self.window, shot))
                self.problems.append('    screenshot: %s' % path)
            except Exception:  # noqa: BLE001 - best effort only
                pass


# ---------------------------------------------------------------- the checks


def check_page(current, report):
    """No sideways scroll; the player and the tab bar are reachable."""
    current.page.evaluate(SCROLL_TO_STAGE_JS)
    m = current.page.evaluate(PAGE_SCRIPT)
    report.count('page measurements')
    if m['scrollWidth'] > m['vw']:
        report.fail(current, 'the page scrolls horizontally: scrollWidth %d > window %d (+%d px)'
                    % (m['scrollWidth'], m['vw'], m['scrollWidth'] - m['vw']), 'hscroll')
    for c in m['controls']:
        if c['rect'] is None:
            report.fail(current, '%s is not on the page' % c['name'])
        elif c['rect']['width'] < 1 or c['rect']['height'] < 1:
            report.fail(current, '%s has no size' % c['name'], 'control')
        elif not inside(c['rect'], m['vw'], m['vh']):
            r = c['rect']
            report.fail(current, '%s is outside the window (%dx%d): left %.0f right %.0f '
                        'top %.0f bottom %.0f' % (c['name'], m['vw'], m['vh'], r['left'],
                                                  r['right'], r['top'], r['bottom']), 'control')
        elif c.get('coveredBy'):
            report.fail(current, '%s is covered by <%s> at its centre'
                        % (c['name'], c['coveredBy']), 'control')
    bar = m['tabbar']
    if bar is None or not inside(bar, m['vw'], m['vh']):
        report.fail(current, 'the tab bar is outside the window: %r' % bar, 'control')


def check_items(current, report, follow=True):
    """Item Sets: Fit fits or sits at the minimum zoom; Follow keeps the newest state in view."""
    current.page.evaluate(SCROLL_TO_STAGE_JS)
    if current.page.query_selector('#diagram-scroll svg') is None:
        report.count('Item Sets steps with nothing drawn yet')
        return
    current.page.click(FIT)
    frames(current, 3)
    m = current.page.evaluate(ITEMS_JS)
    report.count('Fit views')
    percent = int((m['zoom'] or '0%').rstrip('%'))
    if percent < MIN_SCALE_PERCENT:
        report.fail(current, 'zoom reads %d%%, below the %d%% minimum' % (percent, MIN_SCALE_PERCENT),
                    'zoom')
    if m['scale'] < MIN_SCALE_PERCENT / 100 - 0.01:
        report.fail(current, 'the drawing is rendered at %.0f%%, below the %d%% minimum'
                    % (m['scale'] * 100, MIN_SCALE_PERCENT), 'zoom')
    box = m['scroller']
    if not within(m['frame'], m['view']) or not within(box, m['frame']):
        report.fail(current, 'the diagram frame is not inside the stage: frame %r, stage %r'
                    % (m['frame'], m['view']), 'frame')
    if not within(m['svg'], box):
        # At the 85% floor Fit may leave the drawing larger than its box, which scrolls.
        if percent > MIN_SCALE_PERCENT:
            report.fail(current, 'Fit at %d%% left the drawing outside its box by '
                        '%.0f px right / %.0f px bottom' % (
                            percent, max(0, m['svg']['right'] - box['right']),
                            max(0, m['svg']['bottom'] - box['bottom'])), 'fit')
        else:
            report.count('Fit views held at the 85% floor')
    for t in m['texts']:
        if t['px'] >= READABLE_PX - 0.05:
            continue
        # Every piece of text in a node, lookaheads and "+N more" included.
        report.fail(current, 'node text %s "%s" is %.2f px on screen (floor %.2f px)'
                    % (t['what'], t['text'], t['px'], READABLE_PX), 'text')
        break
    report.count('node texts', m['nodeTexts'])

    if not follow:
        return
    current.page.click(FOLLOW)
    try:
        current.page.wait_for_function(FOLLOW_SETTLED_JS, timeout=3000)
    except Exception:  # noqa: BLE001 - reported below with numbers
        pass
    frames(current, 2)
    m = current.page.evaluate(ITEMS_JS)
    if m['current'] is None:
        report.count('Follow views with no current state')
        return
    report.count('Follow views')
    if not within(m['current'], m['scroller'], slack=2):
        report.fail(current, 'Follow: the newest state is not in view: state %r, box %r'
                    % (m['current'], m['scroller']), 'follow')
    if m['scale'] < MIN_SCALE_PERCENT / 100 - 0.01:
        report.fail(current, 'Follow renders at %.0f%%, below the minimum' % (m['scale'] * 100),
                    'zoom')


def check_table(current, report):
    """Parsing Table: fits or scrolls in its own box; the current cell is in view."""
    current.page.evaluate(SCROLL_TO_STAGE_JS)
    m = current.page.evaluate(TABLE_JS)
    if m is None:
        report.fail(current, 'no table is drawn')
        return
    report.count('table views')
    fits = m['scrollW'] <= m['clientW'] + 1
    if not fits:
        report.count('tables wider than their box')
        if m['overflowX'] not in ('auto', 'scroll'):
            report.fail(current, 'the table is wider than its box (%d > %d) and the box does not '
                        'scroll (overflow-x: %s)' % (m['scrollW'], m['clientW'], m['overflowX']),
                        'table')
    if m['scroller']['right'] > m['vw'] + 0.5:
        report.fail(current, 'the table box ends %.0f px right of the window'
                    % (m['scroller']['right'] - m['vw']), 'table')
    cell = m['cell']
    if cell is None:
        report.count('table steps with no current cell')
        return
    report.count('current cells')
    box, view = m['scroller'], m['view']
    if cell['left'] < box['left'] - 1 or cell['right'] > box['right'] + 1:
        report.fail(current, 'the current cell "%s" is scrolled out of the table box '
                    'horizontally (cell %.0f..%.0f, box %.0f..%.0f)'
                    % (m['cellText'], cell['left'], cell['right'], box['left'], box['right']),
                    'cell')
    elif cell['top'] < view['top'] - 1 or cell['bottom'] > view['bottom'] + 1:
        # The stage scrolls vertically so the current cell is inside it.
        report.fail(current, 'the current cell "%s" is %.0f px %s the visible stage (cell %.0f..%.0f, '
                    'stage %.0f..%.0f, table %.0f px tall)'
                    % (m['cellText'],
                       max(view['top'] - cell['top'], cell['bottom'] - view['bottom']),
                       'above' if cell['top'] < view['top'] else 'below',
                       cell['top'], cell['bottom'], view['top'], view['bottom'],
                       m['table']['height']), 'cell')


def check_layout(current, report, width):
    """Panel beside or above the stage; collapse and reopen keep the stage usable."""
    stacked = width <= BREAKPOINT
    current.page.evaluate('() => window.scrollTo(0, 0)')
    m = current.page.evaluate(LAYOUT_JS)
    p, s = m['panel'], m['stage']
    report.count('layout checks')
    if stacked:
        if p['bottom'] > s['top'] + 1:
            report.fail(current, 'below %dpx the panel should be above the stage but ends at %.0f '
                        'while the stage starts at %.0f' % (BREAKPOINT, p['bottom'], s['top']),
                        'layout')
    else:
        if p['right'] > s['left'] + 1 or abs(p['top'] - s['top']) > 1:
            report.fail(current, 'at %dpx the panel should be beside the stage: panel %r, stage %r'
                        % (width, p, s), 'layout')
    open_width = s['width']
    for state in ('collapsed', 'reopened'):
        current.page.click('#panel-toggle')
        frames(current, 3)
        current.page.evaluate('() => window.scrollTo(0, 0)')
        m = current.page.evaluate(LAYOUT_JS)
        want_expanded = 'false' if state == 'collapsed' else 'true'
        if m['expanded'] != want_expanded or m['bodyHidden'] != (state == 'collapsed'):
            report.fail(current, 'panel %s: aria-expanded=%s, body hidden=%s'
                        % (state, m['expanded'], m['bodyHidden']), 'panel')
        st = m['stage']
        if st['width'] < 300 or st['height'] < 300:
            report.fail(current, 'panel %s: the stage is only %.0fx%.0f' % (state, st['width'],
                                                                              st['height']), 'panel')
        if st['left'] < -0.5 or st['right'] > m['vw'] + 0.5:
            report.fail(current, 'panel %s: the stage (%.0f..%.0f) is outside the window width %d'
                        % (state, st['left'], st['right'], m['vw']), 'panel')
        if m['scrollWidth'] > m['vw']:
            report.fail(current, 'panel %s: the page scrolls horizontally (%d > %d)'
                        % (state, m['scrollWidth'], m['vw']), 'panel')
        if state == 'collapsed' and not stacked and st['width'] <= open_width + 100:
            report.fail(current, 'collapsing the panel gave the stage no room: %.0f -> %.0f px'
                        % (open_width, st['width']), 'panel')
        if state == 'collapsed':
            check_page(current, report)     # the player is still reachable collapsed
        elif abs(st['width'] - open_width) > 1:
            report.fail(current, 'reopening the panel left the stage %.0f px wide, was %.0f'
                        % (st['width'], open_width), 'panel')


# ------------------------------------------------------------------ the driver


def phase_ranges(phases):
    """{phase label: (first step, last step)} from the per-step labels."""
    out = {}
    for i, label in enumerate(phases):
        lo, _ = out.get(label, (i, i))
        out[label] = (lo, i)
    return out


def sample(lo, hi):
    """The first, a middle and the last step of a range, without repeats."""
    return sorted({lo, (lo + hi) // 2, hi})


def run_matrix(current, report, width):
    """Every grammar / variant of the matrix, every tab, three steps each."""
    for grammar, variant in MATRIX:
        out = current.choose_variant(variant)
        expect(out.ok, 'analysis failed: %s' % (out.errors or out.messages))
        out = current.choose_preset(grammar)
        expect(out.ok, '%s under %s: analysis failed' % (grammar, variant))
        conflicted = (grammar, variant) in CONFLICTED
        title = current.page.get_attribute('#tab-parse', 'title') or ''
        if conflicted:
            expect(not current.tab_available('parse') and PARSE_STOPS_REASON in title,
                   '%s under %s: expected the Parse tab disabled because the table has '
                   'conflicts, got enabled=%s title=%r'
                   % (grammar, variant, current.tab_available('parse'), title))
        else:
            expect(current.tab_available('parse'),
                   '%s under %s: the Parse tab is disabled (title %r); the table should be '
                   'clean and the preset carries a test string' % (grammar, variant, title))
        total = out.total
        phases = current.page.evaluate(PHASES_JS)
        expect(len(phases) == total, 'scanned %d steps, the counter says %d' % (len(phases), total))
        ranges = phase_ranges(phases)

        report.at(grammar, variant, 'layout', 0, total)
        current.tab('items')
        current.scrub_to(total // 2)
        frames(current, 2)
        check_layout(current, report, width)

        for tab in PHASE_TABS + ('compare',):
            if tab == 'parse' and conflicted:
                continue
            if tab == 'compare':
                lo, hi = 0, total - 1
            else:
                lo, hi = ranges[PHASE_OF_TAB[tab]]
            expect(current.tab(tab), '%s under %s: tab %s is disabled' % (grammar, variant, tab))
            for step in sample(lo, hi):
                current.scrub_to(step)
                frames(current, 2)
                report.at(grammar, variant, tab, step, total)
                expect(current.current_tab() == tab,
                       '%s: scrubbing to step %d left the %s tab' % (report.where, step + 1, tab))
                check_page(current, report)
                if tab == 'items':
                    check_items(current, report)
                elif tab == 'table':
                    check_table(current, report)


def run_viewport(app, label, width, height, scale):
    """One window size, the whole matrix, in the browser `app` is in."""
    report = Report(app, label)
    if scale is None:
        current = app
        app.page.set_viewport_size({'width': width, 'height': height})
        frames(app, 3)
    else:
        current = open_app(app.context.browser, app.browser_name,
                           viewport={'width': width, 'height': height},
                           device_scale_factor=scale)
    try:
        run_matrix(current, report, width)
    finally:
        if current is not app:
            report.problems.extend('%s [own page log] %s' % (label, p) for p in current.log.problems())
            current.close()
    summary = ', '.join('%s %d' % (k, v) for k, v in sorted(report.counts.items()))
    app.note('%s: %s' % (label, summary))
    if report.problems:
        shown = report.problems[:MAX_REPORTED]
        more = len(report.problems) - len(shown)
        text = '%d problems:\n    ' % len(report.problems) + '\n    '.join(shown)
        if more:
            text += '\n    ... and %d more' % more
        expect(False, text)


def _register(label, width, height, scale):
    def viewport_test(app):
        run_viewport(app, label, width, height, scale)
    viewport_test.__name__ = 'viewport_' + label.replace('-', '_')
    viewport_test.__doc__ = 'The whole matrix at %s.' % label
    test()(viewport_test)


for _vp in VIEWPORTS:
    _register(*_vp)

if __name__ == '__main__':
    raise SystemExit(main())

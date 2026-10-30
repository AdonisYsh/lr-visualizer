"""The file:// smoke test: every preset under every variant, every tab, both browsers.

Checks that analysis succeeds, Last lands on the right phase, every enabled tab
draws, and the conflict and state counts match the measured table below.

Run:  python3 tests/browser/test_smoke.py [--browser firefox] [-k epsilon]
"""

import re
import sys
from pathlib import Path

sys.dont_write_bytecode = True  # leave no __pycache__ behind in the tree
sys.path.insert(0, str(Path(__file__).resolve().parent))

import statehash  # noqa: E402 - the path tweak above must come first
from harness import (  # noqa: E402
    PHASE_TABS, RENDER_FAILURE_TEXT, TABS, VARIANTS, expect, main, test)

PARSE_STOPPED = 'The table has conflicts, so the walkthrough stops at the table'
LABEL = {'lr0': 'LR(0)', 'slr1': 'SLR(1)', 'lalr1': 'LALR(1)', 'clr1': 'CLR(1)'}

# Measured counts, keyed by the example menu's names. `states` is (LR(0), CLR(1));
# `conflicts` is per variant in VARIANTS order, counted per cell.
MEASURED = {
    'Expression grammar (classic)': {'states': (12, 22), 'conflicts': (2, 0, 0, 0)},
    'Balanced pairs':               {'states': (6, 10),  'conflicts': (0, 0, 0, 0)},
    'Two halves':                   {'states': (7, 10),  'conflicts': (0, 0, 0, 0)},
    'Epsilon':                      {'states': (6, 6),   'conflicts': (1, 0, 0, 0)},
    'Dangling else':                {'states': (7, 12),  'conflicts': (1, 1, 1, 1)},
    'Ambiguous expression':         {'states': (7, 7),   'conflicts': (4, 4, 4, 4)},
    'Reduce-reduce':                {'states': (5, 5),   'conflicts': (2, 1, 1, 1)},
    'Pointers and assignment':      {'states': (10, 14), 'conflicts': (1, 1, 0, 0)},
    'Merge casualty':               {'states': (13, 14), 'conflicts': (6, 2, 2, 0)},
    'Named tokens':                 {'states': (11, 13), 'conflicts': (0, 0, 0, 0)},
    'Nested parentheses':           {'states': (6, 10),  'conflicts': (0, 0, 0, 0)},
    'Optional pairs (ε)':           {'states': (5, 8),   'conflicts': (2, 0, 0, 0)},
    'Two empty non-terminals (ε)':  {'states': (10, 10), 'conflicts': (3, 2, 0, 0)},
}

# The only case where `merge_induced` is true.
MERGE_INDUCED = {('Merge casualty', 'lalr1')}


def expected(preset, variant):
    """(conflicts, states) measured for this preset under this variant."""
    row = MEASURED[preset]
    states = row['states'][1] if variant == 'clr1' else row['states'][0]
    return row['conflicts'][VARIANTS.index(variant)], states


def slug(name):
    """'Optional pairs (ε)' -> 'optional_pairs', for test names and -k."""
    return re.sub(r'[^a-z0-9]+', '_', name.lower()).strip('_')


class Problems:
    """Collects every failure across a preset's four variants, then fails once."""

    def __init__(self):
        self.lines = []

    def check(self, condition, message):
        """Record `message` unless `condition` holds; returns the condition."""
        if not condition:
            self.lines.append(message)
        return condition

    def fail_if_any(self, where):
        """Raise one TestFailure listing everything recorded."""
        expect(not self.lines, '%s: %d problem(s):\n      %s'
               % (where, len(self.lines), '\n      '.join(self.lines)))


# ------------------------------------------------------------ reading the page

# Verdict headline, e.g. "This grammar is SLR(1)" or
# "This grammar is NOT LR(0) - 2 conflicts (2 shift-reduce, 0 reduce-reduce)".
HEADLINE = re.compile(
    r'^This grammar is (NOT )?(\S+)'
    r'(?: - (\d+) conflicts? \((\d+) shift-reduce, (\d+) reduce-reduce\))?$')

READ_TABLE_JS = r"""
() => {
  const h = document.querySelector('#view .verdict .headline');
  return {
    headline: h ? h.textContent.trim() : null,
    conflictCells: document.querySelectorAll('#view table td.conflict').length,
    mergeCells: document.querySelectorAll('#view table td.merge-conflict').length
  };
}
"""

# One entry per row of Compare all four; the label is the row header's own text node.
READ_COMPARE_JS = r"""
() => Array.from(document.querySelectorAll('#view table.compare tbody tr')).map(tr => {
  const th = tr.querySelector('th');
  const label = Array.from(th.childNodes).filter(n => n.nodeType === 3)
    .map(n => n.textContent).join('').trim();
  const nums = Array.from(tr.querySelectorAll('td.num')).map(td => td.textContent.trim());
  return { label: label, states: nums[0], conflicts: nums[1],
           showing: tr.classList.contains('showing') };
})
"""

def view_problem(app):
    """Why the current view does not count as drawn, or None if it does."""
    data = app.page.evaluate(
        """() => { const v = document.getElementById('view');
                   return { text: v.textContent.trim(), kids: v.childElementCount,
                            internal: Array.from(document.querySelectorAll('#messages .msg.error'))
                              .some(e => /Internal rendering error/.test(e.textContent)) }; }""")
    if data['internal']:
        return 'an "Internal rendering error" box is showing'
    if not data['kids'] or not data['text']:
        return 'the view is empty'
    if RENDER_FAILURE_TEXT in data['text']:
        return 'the view could not be drawn'
    return None


def check_counts(app, preset, variant, problems):
    """The conflict count from the verdict, the conflicted cells and Compare all four must match MEASURED."""
    where = '%s under %s' % (preset, variant)
    want_conflicts, want_states = expected(preset, variant)
    label = LABEL[variant]

    # 1. The verdict banner and the cells, at the last step of the table phase.
    if not problems.check(app.end_of_phase('table') is not None, where + ': no table phase'):
        return
    table = app.page.evaluate(READ_TABLE_JS)
    m = HEADLINE.match(table['headline'] or '')
    if problems.check(m is not None, '%s: unreadable verdict headline %r'
                      % (where, table['headline'])):
        negated, named, count, sr, rr = m.groups()
        count = int(count) if count else 0
        problems.check(named == label, '%s: the verdict names %s' % (where, named))
        problems.check(count == want_conflicts, '%s: the verdict says %d conflicts, '
                       'expected %d (%r)' % (where, count, want_conflicts, table['headline']))
        problems.check(bool(negated) == (count > 0), '%s: verdict "NOT" disagrees with its '
                       'count: %r' % (where, table['headline']))
        if count:
            problems.check(int(sr) + int(rr) == count, '%s: %s + %s kinds != %d conflicts'
                           % (where, sr, rr, count))
    problems.check(table['conflictCells'] == want_conflicts,
                   '%s: %d conflicted cells in the table, expected %d'
                   % (where, table['conflictCells'], want_conflicts))
    merge_expected = (preset, variant) in MERGE_INDUCED
    problems.check((table['mergeCells'] > 0) == merge_expected,
                   '%s: %d merge-induced cells, expected %s'
                   % (where, table['mergeCells'], 'some' if merge_expected else 'none'))

    # 2. Compare all four: this variant's row, and that it is the one marked.
    app.tab('compare')
    rows = {r['label']: r for r in app.page.evaluate(READ_COMPARE_JS)}
    problems.check(sorted(rows) == sorted(LABEL.values()),
                   '%s: Compare all four has rows %s' % (where, sorted(rows)))
    for v in VARIANTS:
        row = rows.get(LABEL[v])
        if not row:
            continue
        c, s = expected(preset, v)
        problems.check((row['conflicts'], row['states']) == (str(c), str(s)),
                       '%s: Compare all four says %s has %s conflicts / %s states, '
                       'expected %d / %d' % (where, LABEL[v], row['conflicts'],
                                                    row['states'], c, s))
        problems.check(row['showing'] == (v == variant),
                       '%s: Compare all four marks the wrong row as shown (%s)' % (where, LABEL[v]))


# --------------------------------------------------------------- the presets


def run_preset(app, preset):
    """Every variant of one preset: analyze, Last, every tab, the counts."""
    problems = Problems()
    for n, variant in enumerate(VARIANTS):
        where = '%s under %s' % (preset, variant)
        if n == 0:
            # A fresh page is on LR(0); picking the preset analyzes it there.
            expect(app.variant() == variant, 'fresh page is on %s' % app.variant())
            outcome = app.choose_preset(preset)
        else:
            outcome = app.choose_variant(variant)
        if not problems.check(outcome.ok and not outcome.errors,
                              '%s: analysis failed: %s' % (where, outcome.errors or outcome.messages)):
            continue
        problems.check(app.variant() == variant, '%s: radio shows %s' % (where, app.variant()))

        want_conflicts, _ = expected(preset, variant)
        app.last()
        problems.check(app.position() == (outcome.total - 1, outcome.total),
                       '%s: Last landed on %s' % (where, app.position()))
        if want_conflicts:
            # A conflicted table stops the walkthrough: no parse tab, and the
            # last step is the end of the table phase.
            problems.check(app.step_phase() == 'table',
                           '%s: the last step is in phase %r, not table' % (where, app.step_phase()))
            problems.check(app.available_tabs() == [t for t in TABS if t != 'parse'],
                           '%s: enabled tabs are %s' % (where, app.available_tabs()))
            why = app.page.get_attribute('#tab-parse', 'title')
            problems.check(why == PARSE_STOPPED,
                           '%s: the disabled parse tab says %r' % (where, why))
        else:
            problems.check(app.step_phase() == 'parse',
                           '%s: the last step is in phase %r' % (where, app.step_phase()))
            problems.check(app.available_tabs() == list(TABS),
                           '%s: enabled tabs are %s' % (where, app.available_tabs()))
        for tab in app.available_tabs():
            app.tab(tab)
            why = view_problem(app)
            problems.check(why is None, '%s, tab %s: %s' % (where, tab, why))

        check_counts(app, preset, variant, problems)

        found = app.injection_report()
        problems.check(not found, '%s: markup appeared: %s' % (where, found))
    problems.fail_if_any(preset)
    app.note('%s: 4 variants x %d tabs drawn; conflicts %s match the expected counts'
             % (preset, len(TABS), '/'.join(str(c) for c in MEASURED[preset]['conflicts'])))


def register(preset):
    """One registered test per preset, named for -k filtering."""
    def run(app):
        run_preset(app, preset)
    run.__name__ = 'smoke_' + slug(preset)
    test()(run)


@test()
def smoke_preset_menu_is_the_measured_thirteen(app):
    """The menu and MEASURED describe the same presets."""
    names = app.preset_names()
    expect(sorted(names) == sorted(MEASURED),
           'the example menu and the expected-count table differ:\n      menu only: %s\n'
           '      table only: %s' % (sorted(set(names) - set(MEASURED)),
                                     sorted(set(MEASURED) - set(names))))
    expect(len(names) == 13, '%d presets in the menu' % len(names))


for _preset in MEASURED:
    register(_preset)


# ------------------------------------------------------- theme and the panel

THEME_JS = r"""
() => ({ theme: document.documentElement.getAttribute('data-theme'),
         pressed: document.getElementById('theme-toggle').getAttribute('aria-pressed'),
         bg: getComputedStyle(document.body).backgroundColor })
"""
PANEL_JS = r"""
() => ({ collapsed: document.getElementById('app').classList.contains('panel-collapsed'),
         hidden: document.getElementById('panel-body').hidden,
         expanded: document.getElementById('panel-toggle').getAttribute('aria-expanded') })
"""


def form_state(app):
    """What the user typed and chose: none of it may move."""
    return (app.grammar(), app.test_string(), app.variant(), app.position(), app.current_tab())


def chrome_round_trip(app, where, problems):
    """Theme there and back, then panel there and back, each read against the start."""
    start = statehash.read(app)
    form = form_state(app)

    def same(what):
        diff = statehash.differing(statehash.read(app), start)
        problems.check(not diff, '%s, after %s: changed %s' % (where, what, ', '.join(diff)))
        problems.check(form_state(app) == form, '%s, after %s: form/position moved from %s to %s'
                       % (where, what, form, form_state(app)))

    t0 = app.page.evaluate(THEME_JS)
    dark0 = t0['pressed'] == 'true'
    app.page.click('#theme-toggle')
    t1 = app.page.evaluate(THEME_JS)
    problems.check(t1['theme'] == ('light' if dark0 else 'dark') and
                   t1['pressed'] == ('false' if dark0 else 'true'),
                   '%s: the theme toggle did not flip (%s -> %s)' % (where, t0, t1))
    problems.check(t1['bg'] != t0['bg'], '%s: the page background stayed %s' % (where, t0['bg']))
    same('the theme toggle')
    app.page.click('#theme-toggle')
    t2 = app.page.evaluate(THEME_JS)
    problems.check(t2['bg'] == t0['bg'] and t2['pressed'] == t0['pressed'],
                   '%s: toggling twice did not restore the theme (%s -> %s)' % (where, t0, t2))
    same('the theme toggle, twice')

    app.page.click('#panel-toggle')
    p1 = app.page.evaluate(PANEL_JS)
    problems.check(p1 == {'collapsed': True, 'hidden': True, 'expanded': 'false'},
                   '%s: the panel did not collapse: %s' % (where, p1))
    same('collapsing the panel')
    app.page.click('#panel-toggle')
    p2 = app.page.evaluate(PANEL_JS)
    problems.check(p2 == {'collapsed': False, 'hidden': False, 'expanded': 'true'},
                   '%s: the panel did not reopen: %s' % (where, p2))
    same('reopening the panel')


@test()
def smoke_theme_and_panel_keep_the_analysis(app):
    """From a light and a dark system theme, on every tab, theme and panel toggles change nothing.

    Run on the LALR(1) merging showcase and the largest automaton (CLR(1) expression)."""
    problems = Problems()
    for scheme in ('light', 'dark'):
        # Forget the earlier explicit choice, as a reload would, then switch
        # the system theme; the toggle must track it.
        app.page.evaluate("() => document.documentElement.removeAttribute('data-theme')")
        app.page.emulate_media(color_scheme=scheme)
        app.page.wait_for_function(
            "want => document.getElementById('theme-toggle').getAttribute('aria-pressed') === want",
            arg='true' if scheme == 'dark' else 'false')
        for preset, variant in (('Two halves', 'lalr1'),
                                ('Expression grammar (classic)', 'clr1')):
            app.choose_variant(variant)
            outcome = app.choose_preset(preset)
            expect(outcome.ok, '%s under %s: analysis failed' % (preset, variant))
            for tab in PHASE_TABS + ('compare',):
                if tab == 'compare':
                    app.tab(tab)
                else:
                    app.end_of_phase(tab)
                chrome_round_trip(app, '%s system theme, %s under %s, tab %s'
                                  % (scheme, preset, variant, tab), problems)
    problems.fail_if_any('theme and panel')
    app.note('theme toggle and panel collapse: 2 system themes x 2 analyses x 6 tabs, '
             'stage unchanged every time')


if __name__ == '__main__':
    raise SystemExit(main())

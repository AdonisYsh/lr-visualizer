"""Every view is a function of the step: any route to step i draws the same stage.
Runs `dist/index.html` over file:// (Chromium and Firefox) for each variant on three presets. Passes A-H reach every step by Next, Prev, keys, slider, jump keys, phase tabs and Compare all four, and compare each read with the forward walk (statehash.py).
Run:  python3 tests/browser/test_determinism.py [--browser firefox] [-k expression]
"""

import random
import sys
from pathlib import Path

sys.dont_write_bytecode = True  # leave no __pycache__ behind in the tree
sys.path.insert(0, str(Path(__file__).resolve().parent))

import statehash  # noqa: E402 - the path tweak above must come first
from harness import (  # noqa: E402
    PHASE_OF_TAB, PHASE_TABS, VARIANTS, expect, main, parse_counter, test)

# (test-name slug, preset name exactly as the example menu shows it)
PRESETS = (
    ('expression', 'Expression grammar (classic)'),
    ('merge_casualty', 'Merge casualty'),
    ('epsilon', 'Epsilon'),
)

# Stop listing mismatches after this many; the count is still reported.
MAX_REPORTED = 12
# Pass H's jumps back on Compare: every k-th step plus the last one.
H_JUMP_EVERY = 7
# (preset slug, variant) pairs whose table has conflicts; there the parse tab is
# disabled. Fixed on purpose: wrongly hiding the parse on a clean table must fail.
CONFLICTED = {
    ('expression', 'lr0'),
    ('merge_casualty', 'lr0'), ('merge_casualty', 'slr1'), ('merge_casualty', 'lalr1'),
    ('epsilon', 'lr0'),
}
PARSE_DISABLED_TITLE = 'The table has conflicts, so the walkthrough stops at the table'
TAB_OF_PHASE = {phase: tab for tab, phase in PHASE_OF_TAB.items()}


class Mismatches:
    """Collects differences between a read and the reference, then fails once."""

    def __init__(self, where):
        self.where = where
        self.lines = []
        self.count = 0

    def check(self, how, index, got, want, parts=statehash.PARTS):
        """Record a mismatch if any of `parts` differs; returns True if all match."""
        diff = statehash.differing(got, want, parts)
        if not diff:
            return True
        self.count += 1
        if len(self.lines) < MAX_REPORTED:
            self.lines.append('%s: step %d [%s] "%s" differs in: %s'
                              % (how, index, want['phase'], want['line'][:70],
                                 ', '.join(diff)))
        return False

    def fail_if_any(self):
        """Raise one TestFailure listing every recorded mismatch."""
        if not self.count:
            return
        more = '' if self.count <= MAX_REPORTED else (
            '\n      ... and %d more' % (self.count - MAX_REPORTED))
        expect(False, '%s: %d reads did not match the forward Next walk:\n      %s%s'
               % (self.where, self.count, '\n      '.join(self.lines), more))


def at(app, index, total, how):
    """Read the stage; fail at once if the player is not at `index`."""
    got = statehash.read(app)
    pos = parse_counter(got['counter'])
    expect(pos == (index, total),
           '%s: expected step %d of %d, the counter reads %r'
           % (how, index + 1, total, got['counter']))
    return got


def click(app, selector):
    # A real click, minus the wait-for-stable that would triple the cost of a
    # step (see harness.App.walk); `at` proves each one landed.
    app.page.click(selector, force=True)


def run_determinism(app, preset, variant, conflicted=False):
    """Passes A-H from the module docstring for one preset under one variant."""
    where = '%s under %s' % (preset, variant)
    app.choose_variant(variant)
    outcome = app.choose_preset(preset)
    expect(outcome.ok, '%s: analysis failed: %s' % (where, outcome.errors or outcome.messages))
    expect(app.variant() == variant, '%s: the variant radio is %s' % (where, app.variant()))
    phase_tabs = PHASE_TABS
    if conflicted:
        expect(not app.tab_available('parse'), '%s: conflicted table but the parse tab is enabled' % where)
        title = app.page.get_attribute('#tab-parse', 'title') or ''
        expect(PARSE_DISABLED_TITLE in title,
               '%s: the parse tab reason is %r, expected %r' % (where, title, PARSE_DISABLED_TITLE))
        phase_tabs = tuple(t for t in PHASE_TABS if t != 'parse')
    else:
        expect(app.tab_available('parse'), '%s: no parse phase - is the test string set?' % where)
    total = outcome.total
    expect(total > 1, '%s: only %d steps' % (where, total))
    bad = Mismatches(where)

    # A - the reference: step 0, then Next to the end.
    expect(app.position() == (0, total), '%s: a fresh analysis is not at step 1' % where)
    ref = [at(app, 0, total, 'A start')]
    for i in range(1, total):
        click(app, '#btn-next')
        ref.append(at(app, i, total, 'A Next'))
    expect(not app.page.is_enabled('#btn-next'), '%s: Next still enabled at the end' % where)
    for i, r in enumerate(ref):
        expect(r['tab'] == TAB_OF_PHASE.get(r['phase']),
               '%s: step %d is in phase %r but the %r tab is showing'
               % (where, i, r['phase'], r['tab']))
    seen = {r['tab'] for r in ref}
    if conflicted:
        expect(ref[-1]['phase'] == 'table',
               '%s: conflicted table but the last step is in phase %r' % (where, ref[-1]['phase']))
    expect(seen == set(phase_tabs),
           '%s: the walk only reached tabs %s' % (where, sorted(seen)))

    # B - Prev all the way back; the last read is the step-0 snapshot.
    for i in range(total - 2, -1, -1):
        click(app, '#btn-prev')
        bad.check('B Prev', i, at(app, i, total, 'B Prev'), ref[i])
    expect(not app.page.is_enabled('#btn-prev'), '%s: Prev still enabled at step 1' % where)

    # C, D - the keyboard, on the stage, as a user would after clicking it.
    app.page.focus('#view')
    for i in range(1, total):
        key = 'Space' if i % 5 == 0 else 'ArrowRight'
        app.page.keyboard.press(key)
        got = at(app, i, total, 'C ' + key)
        expect(got['active'] == 'view',
               '%s: after %s at step %d focus is on %r, not the stage'
               % (where, key, i, got['active']))
        bad.check('C ' + key, i, got, ref[i])
    for i in range(total - 2, -1, -1):
        app.page.keyboard.press('ArrowLeft')
        bad.check('D ArrowLeft', i, at(app, i, total, 'D ArrowLeft'), ref[i])

    # E - direct jumps with the slider, in an order that never walks.
    order = list(range(total))
    random.Random('%s/%s' % (preset, variant)).shuffle(order)
    for i in order:
        app.scrub_to(i)
        bad.check('E slider', i, at(app, i, total, 'E slider'), ref[i])

    # F - the jump keys and buttons, each from the middle of the log.
    mid = total // 2
    for how, act, want in (
            ('F Home', lambda: app.page.keyboard.press('Home'), 0),
            ('F End', lambda: app.page.keyboard.press('End'), total - 1),
            ('F First', lambda: click(app, '#btn-first'), 0),
            ('F Last', lambda: click(app, '#btn-last'), total - 1)):
        app.scrub_to(mid)
        app.page.focus('#view')
        act()
        bad.check(how, want, at(app, want, total, how), ref[want])

    # G - entering each phase through its tab, from the last step.
    for tab in phase_tabs:
        app.scrub_to(total - 1)
        app.tab(tab)
        index, _ = app.position()
        phase = PHASE_OF_TAB[tab]
        first = min(i for i, r in enumerate(ref) if r['phase'] == phase)
        last = max(i for i, r in enumerate(ref) if r['phase'] == phase)
        want = last if last == total - 1 else first   # already inside: stays put
        expect(index == want,
               '%s: the %s tab opened step %d, expected %d (its phase runs %d-%d)'
               % (where, tab, index, want, first, last))
        bad.check('G tab ' + tab, index, at(app, index, total, 'G tab ' + tab), ref[index])

    # H - parked on Compare all four. Everything but the tab strip and the view
    # must match the reference; the view must not move at all.
    app.scrub_to(0)
    app.tab('compare')
    shared = ('counter', 'explain', 'player', 'messages')
    cmp_ref = [at(app, 0, total, 'H start')]
    bad.check('H compare', 0, cmp_ref[0], ref[0], shared)
    for i in range(1, total):
        click(app, '#btn-next')
        got = at(app, i, total, 'H Next')
        expect(got['tab'] == 'compare',
               '%s: stepping to %d on Compare all four switched to %r' % (where, i, got['tab']))
        bad.check('H compare Next', i, got, ref[i], shared)
        bad.check('H compare view', i, got, cmp_ref[0], ('view', 'tabs'))
        cmp_ref.append(got)
    # Back on Compare by direct jumps rather than a second Prev walk, which
    # would repeat pass B on another tab for a third of the suite's runtime.
    sample = sorted(set(range(0, total, H_JUMP_EVERY)) | {total - 1})
    random.Random('compare/%s/%s' % (preset, variant)).shuffle(sample)
    for i in sample:
        app.scrub_to(i)
        bad.check('H compare slider', i, at(app, i, total, 'H slider'), cmp_ref[i])

    bad.fail_if_any()
    app.note('%s: %d steps; each reached by Next, Prev, ArrowRight/Space, ArrowLeft and '
             'the slider, plus Compare at every step - all identical' % (where, total))


def register(slug, preset, variant):
    """One registered test per (preset, variant), named for -k filtering."""
    def run(app):
        run_determinism(app, preset, variant, (slug, variant) in CONFLICTED)
    run.__name__ = 'determinism_%s_%s' % (slug, variant)
    test()(run)


for _slug, _preset in PRESETS:
    for _variant in VARIANTS:
        register(_slug, _preset, _variant)

if __name__ == '__main__':
    raise SystemExit(main())

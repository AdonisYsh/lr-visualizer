"""Things the page says, or must not say, around an analysis.

Run in the built `dist/index.html` over file://: the conflicted-table stop,
tab and menu state after errors, the stage's focus ring, the slider's single
draw path, and unusual engine results (null-line warnings, `S''`, a missing method).

Run:  python3 tests/browser/test_followups.py [--browser firefox] [-k name]
"""

import re
import sys
from pathlib import Path

sys.dont_write_bytecode = True  # leave no __pycache__ behind in the tree
sys.path.insert(0, str(Path(__file__).resolve().parent))

import statehash  # noqa: E402 - the path tweak above must come first
from harness import RENDER_FAILURE_TEXT, expect, main, test  # noqa: E402

EXPRESSION = 'Expression grammar (classic)'

# ------------------------------------------------- a parse on a conflicted table

PARSE_JS = r"""
() => {
  const box = document.querySelector('#view .verdict');
  const rows = document.querySelectorAll('#view table.trace tbody tr');
  const last = rows.length ? rows[rows.length - 1] : null;
  const style = box ? getComputedStyle(box) : null;
  return {
    headline: box ? box.querySelector('.headline').textContent : null,
    classes: box ? box.className : null,
    caveat: box && box.querySelector('.conflict-caveat')
      ? box.querySelector('.conflict-caveat').textContent : null,
    border: style ? style.borderTopStyle : null,
    background: style ? style.backgroundColor : null,
    lastAction: last ? last.cells[last.cells.length - 1].textContent : null,
    summary: document.getElementById('explain-summary').textContent,
    full: document.getElementById('explain-full').textContent
  };
}
"""

# The rule as the engine words it (engine/src/parser.rs).
RULE = 'In a conflicted cell the parser takes the first action it finds'


def parse_result(app, where):
    """Jump to the last step, which ends the parse, and read how it is shown."""
    app.last()
    expect(app.current_tab() == 'parse', '%s: the last step is not on the parse tab' % where)
    return app.page.evaluate(PARSE_JS)


@test()
def conflicted_table_stops_the_walkthrough_at_the_table(app):
    """On a conflicted table the player ends at the table; a clean one still parses."""
    where = 'Dangling else under lr0'
    app.choose_variant('lr0')
    expect(app.choose_preset('Dangling else').ok, '%s: analysis failed' % where)
    app.last()
    expect(app.current_tab() == 'table',
           '%s: the last step is on %r, not the table' % (where, app.current_tab()))
    summary = app.page.evaluate("() => document.getElementById('explain-summary').textContent")
    expect('the walkthrough stops here' in summary,
           '%s: the summary line says %r' % (where, summary))
    expect(not app.tab_available('parse'), '%s: the parse tab is enabled' % where)
    title = app.page.get_attribute('#tab-parse', 'title')
    expect(title == 'The table has conflicts, so the walkthrough stops at the table',
           '%s: the parse tab title is %r' % (where, title))

    where = '%s under slr1' % EXPRESSION
    app.choose_variant('slr1')
    expect(app.choose_preset(EXPRESSION).ok, '%s: analysis failed' % where)
    expect(app.tab_available('parse'), '%s: the parse tab is disabled' % where)
    seen = parse_result(app, where)
    expect(seen['headline'] == 'ACCEPT', '%s: the banner says %r' % (where, seen['headline']))
    expect(seen['lastAction'] == 'ACCEPT' and seen['summary'] == 'Accept',
           '%s: trace says %r, summary %r' % (where, seen['lastAction'], seen['summary']))
    app.note('conflicted Dangling else stops at the table with the parse tab disabled; '
             'clean Expression grammar still accepts')


# ------------------------------------------------ after an error; the menu

MENU_JS = r"""
() => {
  const menu = document.getElementById('preset');
  const shown = menu.options[menu.selectedIndex];
  return { value: menu.value, shown: shown ? shown.textContent : null,
           noteGone: document.getElementById('preset-note') === null };
}
"""
TABS_JS = r"""
() => Array.from(document.querySelectorAll('#tabs button')).map(b => ({
  id: b.id, disabled: b.disabled, selected: b.getAttribute('aria-selected'),
  weight: getComputedStyle(b).fontWeight }))
"""


@test()
def error_leaves_no_tab_selected_and_edits_unname_the_example(app):
    """A disabled tab is never the selected one; the menu names what is in the box."""
    start = app.page.evaluate(MENU_JS)
    expect(start['shown'] == EXPRESSION and start['noteGone'],
           'the page did not open on the expression grammar: %r' % start)
    original = app.grammar()

    # Editing the text makes it the user's own grammar, at once.
    app.set_grammar(original + '\nF -> num')
    edited = app.page.evaluate(MENU_JS)
    expect(edited['value'] == '' and 'your own grammar' in edited['shown'],
           'after an edit the menu still shows %r' % edited['shown'])
    expect(app.analyze().ok, 'the edited grammar did not analyze')
    expect(app.page.evaluate(MENU_JS) == edited, 'Analyze changed the menu back')

    # Putting the text back exactly brings the example back.
    app.set_grammar(original)
    expect(app.page.evaluate(MENU_JS) == start,
           'restoring the text did not restore the menu: %r' % app.page.evaluate(MENU_JS))

    # An empty box is not "your own grammar" either.
    app.set_grammar('')
    empty = app.page.evaluate(MENU_JS)
    expect(empty['value'] == '' and 'choose an example' in empty['shown'],
           'with an empty box the menu shows %r' % empty['shown'])

    # A validation error from the Item Sets tab: no tab may stay selected.
    app.set_grammar(original)
    expect(app.analyze().ok, 'analysis failed')
    app.tab('items')
    app.set_grammar('S -> a S b | c\nS a b')
    out = app.analyze()
    expect(not out.ok and out.errors, 'expected a validation error')
    tabs = app.page.evaluate(TABS_JS)
    expect(all(t['disabled'] for t in tabs), 'tabs still enabled after an error: %r' % tabs)
    chosen = [t['id'] for t in tabs if t['selected'] == 'true']
    expect(not chosen, 'after an error %s is still marked selected' % chosen)
    bold = [t['id'] for t in tabs if int(t['weight']) >= 600]
    expect(not bold, 'after an error %s is still drawn as selected' % bold)
    menu = app.page.evaluate(MENU_JS)
    expect(menu['value'] == '' and menu['noteGone'],
           'after an error the menu still names %r' % menu['shown'])

    # And the page recovers: pick an example, one tab is selected again.
    expect(app.choose_preset('Two halves').ok, 'could not analyze after the error')
    tabs = app.page.evaluate(TABS_JS)
    expect([t['id'] for t in tabs if t['selected'] == 'true'] == ['tab-grammar'],
           'after recovering, selected tabs are %r' % tabs)
    menu = app.page.evaluate(MENU_JS)
    expect(menu['shown'] == 'Two halves' and menu['noteGone'], 'menu after recovering: %r' % menu)


# ------------------------------------------------------- the stage's focus ring

FOCUS_JS = r"""
() => {
  const view = document.getElementById('view');
  const a = document.activeElement;
  const style = getComputedStyle(view);
  return { active: a ? (a.id || a.localName) : null,
           visible: view.matches(':focus-visible'),
           outline: style.outlineStyle, tabindex: view.getAttribute('tabindex') };
}
"""


@test()
def fresh_load_does_not_ring_the_stage(app):
    """No ring on load or after a mouse click; a ring when the keyboard gets there."""
    fresh = app.page.evaluate(FOCUS_JS)
    expect(fresh['active'] != 'view', 'a fresh page has moved focus to the stage')
    expect(not fresh['visible'] and fresh['outline'] == 'none',
           'a fresh page rings the stage: %r' % fresh)
    expect(fresh['tabindex'] == '0', 'the stage is no longer focusable')
    # The player's keys work without it: they are bound to the document.
    app.page.keyboard.press('ArrowRight')
    expect(app.position()[0] == 1, 'ArrowRight on a fresh page did not step the player')

    # Analyze with the mouse still moves focus to the result, without a ring.
    app.page.click('#analyze')
    clicked = app.page.evaluate(FOCUS_JS)
    expect(clicked['active'] == 'view', 'Analyze did not move focus to the stage: %r' % clicked)
    expect(not clicked['visible'] and clicked['outline'] == 'none',
           'a mouse click on Analyze rings the stage: %r' % clicked)

    # The keyboard: Tab to the stage from the top of the page. The click on
    # the heading only says where Tab starts from; it focuses nothing.
    app.page.click('#panel h1')
    expect(app.page.evaluate("() => document.activeElement === document.body"),
           'clicking the heading focused something')
    reached = None
    for presses in range(1, 61):
        app.page.keyboard.press('Tab')
        if app.page.evaluate("() => document.activeElement.id") == 'view':
            reached = presses
            break
    expect(reached, 'Tab never reached the stage in 60 presses')
    tabbed = app.page.evaluate(FOCUS_JS)
    expect(tabbed['visible'] and tabbed['outline'] == 'solid',
           'the keyboard reached the stage and it is not ringed: %r' % tabbed)
    before = app.position()[0]
    app.page.keyboard.press('ArrowRight')
    expect(app.position()[0] == before + 1, 'ArrowRight on the focused stage did not step')

    # Analyze from the keyboard: focus goes to the stage, and it is ringed.
    app.page.focus('#analyze')
    app.page.keyboard.press('Enter')
    keyed = app.page.evaluate(FOCUS_JS)
    expect(keyed['active'] == 'view' and keyed['visible'] and keyed['outline'] == 'solid',
           'Analyze from the keyboard: %r' % keyed)
    app.note('no ring on load or after a mouse click; ringed after %d Tabs and after Enter '
             'on Analyze' % reached)


# -------------------------------------------------------- the slider's one path

# The drawing inside #view (without #view's own attributes) plus the player's position.
DRAWN_JS = r"""
() => ({ position: document.getElementById('scrub').getAttribute('aria-valuetext'),
         drawn: document.getElementById('view').innerHTML })
"""
# Dispatch a slider `input` per index and read the page in one task, so no frame runs between.
SCRUB_MANY_JS = r"""
indices => {
  const scrub = document.getElementById('scrub');
  for (const i of indices) {
    scrub.value = String(i);
    scrub.dispatchEvent(new Event('input', { bubbles: true }));
  }
  const sel = document.querySelector('#tabs [aria-selected="true"]');
  return { position: document.getElementById('scrub').getAttribute('aria-valuetext'),
           drawn: document.getElementById('view').innerHTML,
           tab: sel ? sel.id : null,
           explain: document.getElementById('explain').textContent,
           slider: scrub.value };
}
"""
FRAME_JS = "() => new Promise(resolve => requestAnimationFrame(() => resolve()))"


@test()
def slider_moves_the_position_now_and_the_stage_next_frame(app):
    """A scripted `input` is treated like a real drag: one path, one draw per frame."""
    # SLR(1): clean table, so the walkthrough runs on to the parse tab.
    app.choose_variant('slr1')
    expect(app.choose_preset(EXPRESSION).ok, 'analysis failed')
    total = app.position()[1]
    last = total - 1

    # References, drawn synchronously by the buttons.
    app.last()
    at_last = statehash.read(app)
    last_drawn = app.page.evaluate(DRAWN_JS)['drawn']
    last_explain = app.page.text_content('#explain')
    app.first()
    at_first = statehash.read(app)
    first_drawn = app.page.evaluate(DRAWN_JS)['drawn']
    expect(first_drawn != last_drawn, 'first and last steps look the same')

    # Several events in one task, ending on the last step.
    now = app.page.evaluate(SCRUB_MANY_JS, [3, total // 2, 7, last])
    expect(now['position'] == 'Step %d of %d' % (total, total),
           'the position did not follow the slider at once: %r' % now['position'])
    expect(now['explain'] == last_explain and now['slider'] == str(last),
           'the explanation or the slider lag behind: %r' % now['explain'][:80])
    expect(now['tab'] == 'tab-parse', 'the tab strip did not follow the slider: %s' % now['tab'])
    expect(now['drawn'] != last_drawn,
           'a script-dispatched slider event drew the stage synchronously: '
           'there are two slider paths again')
    expect(now['drawn'] == first_drawn,
           'the stage changed before its frame, and not to the step the slider is on')

    app.page.evaluate(FRAME_JS)
    after = statehash.read(app)
    diff = statehash.differing(after, at_last)
    expect(not diff, 'one frame after the slider moved, %s differ from the last step'
           % ', '.join(diff))

    # A button pressed while a frame is owed draws at once and cancels it.
    said = app.page.evaluate(
        """indices => {
             const scrub = document.getElementById('scrub');
             scrub.value = String(indices[0]);
             scrub.dispatchEvent(new Event('input', { bubbles: true }));
             document.getElementById('btn-first').click();
             return document.getElementById('scrub').getAttribute('aria-valuetext'); }""", [total // 2])
    expect(said == 'Step 1 of %d' % total, 'First after a slider move landed on %r' % said)
    expect(not statehash.differing(statehash.read(app), at_first),
           'First did not draw at once while a slider frame was owed')
    app.page.evaluate(FRAME_JS)
    expect(not statehash.differing(statehash.read(app), at_first),
           'the cancelled slider frame drew anyway')

    # The harness's own scrub_to waits that frame.
    app.scrub_to(last)
    expect(not statehash.differing(statehash.read(app), at_last),
           'harness.scrub_to returned before the stage was drawn')


# ------------------------------------------- unusual engine results

@test()
def capped_warnings_renamed_start_and_missing_methods_are_drawn(app):
    """`line: null` warnings, a start symbol called S'', a comparison without a row."""
    # Seventy unreachable non-terminals: fifty warnings, then "+20 more".
    app.set_grammar('S -> a\n' + '\n'.join('U%d -> u%d' % (i, i) for i in range(70)))
    app.set_test_string('')
    out = app.analyze()
    expect(out.ok, 'analysis failed: %s' % out.errors)
    boxes = app.page.evaluate(
        """() => Array.from(document.querySelectorAll('#messages .msg.warning')).map(m => ({
                 tag: m.querySelector('.tag').textContent, text: m.textContent }))""")
    expect(len(boxes) == 51, 'expected 50 warnings and a "+N more" line, got %d' % len(boxes))
    more = boxes[-1]
    expect(more['tag'] == 'Warning: ' and '+20 more warnings not shown' in more['text'],
           'the "+N more" line is drawn as %r' % more)
    expect('null' not in more['text'] and 'undefined' not in more['text'],
           'a missing line number leaked into the text: %r' % more['text'])
    expect(all(re.match(r'^Warning(, line \d+)?: $', b['tag']) for b in boxes),
           'a warning is tagged %r' % [b['tag'] for b in boxes if not
                                       re.match(r'^Warning(, line \d+)?: $', b['tag'])][:1])

    # A grammar that already uses S': the augmented start is S''.
    app.set_grammar("S -> S' a | b\nS' -> c")
    app.set_test_string('c a')
    expect(app.analyze().ok, "the grammar that uses S' did not analyze")
    walk = app.walk()
    expect("0.S'' → S" in walk.text_of('grammar'), "S'' → S is not listed as production 0")
    expect("Start symbolS'' (augmented)" in walk.text_of('grammar'),
           "the Grammar view does not name S'' as the start symbol")
    goto = app.page.evaluate(
        """() => { document.getElementById('tab-table').click();
                   return Array.from(document.querySelectorAll('#view table.grid thead tr'))
                     .pop().textContent; }""")
    expect(goto == "abc$SS'", "table columns are %r: S' must have a GOTO column, S'' none" % goto)
    expect('accept' in walk.text_of('parse').lower(), "the parse over S' did not accept")

    # 150 alternatives: LR(0) needs too many steps and is left out of the
    # comparison of an SLR(1) run. Three rows, and a sentence saying why.
    app.choose_variant('slr1')
    app.set_grammar('S -> ' + ' | '.join('a%d' % i for i in range(1, 151)))
    app.set_test_string('a7')
    out = app.analyze(timeout=60_000)
    expect(out.ok, 'analysis failed: %s' % out.errors)
    app.tab('compare')
    rows = app.page.evaluate(
        "() => document.querySelectorAll('#view table.compare tbody tr').length")
    text = app.view_text()
    expect(rows == 3, 'Compare all four has %d rows' % rows)
    expect('A method is listed only if it could be built' in text,
           'Compare all four does not explain the missing row')
    expect(RENDER_FAILURE_TEXT not in text and not app.errors(),
           'the comparison view failed: %r' % app.errors())
    app.note('51 warning boxes ending in "+20 more"; S\'\' start symbol drawn; 3-row comparison '
             'with its note in Compare all four')


if __name__ == '__main__':
    raise SystemExit(main())

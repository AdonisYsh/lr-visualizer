"""What the page no longer says, and what it now does, in the built `dist/index.html`.
Checked over file:// in Chromium and Firefox: no write-up under the example menu, a Show grammar button that opens the productions without moving the step, no visible step counter, a conflicted table ending the player at the table, and no four-method strip.
Run:  python3 tests/browser/test_requests.py [--browser firefox] [-k name]
"""

import re
import sys
from pathlib import Path

sys.dont_write_bytecode = True  # leave no __pycache__ behind in the tree
sys.path.insert(0, str(Path(__file__).resolve().parent))

from harness import expect, main, test  # noqa: E402 - the path tweak above must come first

EXPRESSION = 'Expression grammar (classic)'


def panel(app):
    """(open?, aria-expanded) of the grammar panel."""
    return app.page.evaluate(
        """() => [!document.getElementById('grammar-panel').hidden,
                  document.getElementById('btn-grammar').getAttribute('aria-expanded')]""")


def valuetext(app):
    """The slider's spoken position."""
    return app.page.get_attribute('#scrub', 'aria-valuetext')


@test()
def no_write_up_under_the_example_menu(app):
    """#preset-note is gone and a chosen preset adds nothing beside the menu."""
    expect(app.page.query_selector('#preset-note') is None, '#preset-note still exists')
    app.choose_preset(EXPRESSION)
    kids = app.page.evaluate(
        """() => Array.from(document.getElementById('preset').parentElement.children)
                  .map(e => e.tagName.toLowerCase())""")
    expect(kids == ['label', 'select'], 'the example menu\'s parent holds %r' % kids)
    texts = app.page.evaluate(
        """() => Array.from(document.getElementById('preset').parentElement.childNodes)
                  .filter(n => n.nodeType === 3 && n.textContent.trim()).length""")
    expect(texts == 0, 'stray text beside the example menu')


@test()
def show_grammar_on_any_step(app):
    """The grammar panel opens and closes without moving the step."""
    app.choose_preset(EXPRESSION)
    for _ in range(3):
        app.next()
    before = valuetext(app)
    app.page.click('#btn-grammar')
    opened, expanded = panel(app)
    expect(opened and expanded == 'true', 'the panel did not open: %r' % [opened, expanded])
    items = app.page.eval_on_selector_all('#grammar-panel-list li', 'e => e.map(x => x.textContent)')
    expect(len(items) == 7, 'the panel lists %d productions: %r' % (len(items), items))
    expect(re.sub(r'\s+', ' ', items[0]).strip() == "E' → E", 'first production is %r' % items[0])
    expect(valuetext(app) == before, 'opening the panel moved the step')
    app.page.keyboard.press('Escape')
    expect(panel(app)[0] is False and panel(app)[1] == 'false', 'Escape did not close it')
    app.page.focus('#view')
    app.page.keyboard.press('g')
    expect(panel(app)[0], 'g did not reopen it')
    app.page.click('#btn-next')
    expect(valuetext(app) != before, 'Next did not advance while the panel was open')
    expect(panel(app)[0], 'Next closed the panel')
    app.page.click('#btn-grammar')
    expect(not panel(app)[0], 'the button did not close it')
    # Opening it while Playing stops Play.
    app.page.click('#btn-play')
    expect(app.page.get_attribute('#btn-play', 'aria-label') == 'Pause', 'Play did not start')
    app.page.click('#btn-grammar')
    expect(app.page.get_attribute('#btn-play', 'aria-label') == 'Play',
           'opening the panel left Play running')
    # Re-analysing closes it.
    expect(panel(app)[0], 'the panel is not open before re-analysing')
    app.analyze()
    expect(not panel(app)[0], 'analysing again left the panel open')
    # A grammar error disables the button.
    app.set_grammar('S ->')
    out = app.analyze()
    expect(not out.ok, 'the broken grammar was accepted')
    expect(app.page.is_disabled('#btn-grammar'), 'the button is enabled after an error')
    expect(not panel(app)[0], 'the panel is open after an error')


@test()
def no_visible_step_counter(app):
    """There is no "Step N of M" text; the slider still says it."""
    app.choose_preset(EXPRESSION)
    expect(app.page.query_selector('#counter') is None, '#counter still exists')
    text = app.page.text_content('#player') or ''
    expect(not re.search(r'Step\s+\d+\s+of\s+\d+', text), 'the player shows %r' % text)
    vt = valuetext(app)
    expect(re.match(r'^Step \d+ of \d+$', vt or ''), 'aria-valuetext is %r' % vt)


@test()
def conflicted_table_stops_at_the_table(app):
    """LR(0) on the expression grammar ends at the table; SLR(1) goes on to the parse."""
    app.choose_preset(EXPRESSION)
    app.set_test_string('id + id * id')
    out = app.choose_variant('lr0')
    expect(out.ok, 'analysis failed: %s' % out.errors)
    app.last()
    expect(app.current_tab() == 'table', 'End is on the %s tab' % app.current_tab())
    summary = app.page.text_content('#explain-summary') or ''
    expect('the walkthrough stops here' in summary, 'summary reads %r' % summary)
    expect(not app.tab_available('parse'), 'the parse tab is enabled')
    title = app.page.get_attribute('#tab-parse', 'title')
    expect(title == 'The table has conflicts, so the walkthrough stops at the table',
           'parse tab title is %r' % title)
    sug = app.page.text_content('#view .verdict .suggestion')
    expect(sug == 'This grammar is SLR(1), though - try that.', 'suggestion reads %r' % sug)
    expect(app.page.query_selector('#view .verdict .comparison') is None,
           'the per-method line is still in the verdict')
    expect(app.tab_available('compare'), 'Compare all four is unavailable')
    app.tab('compare')
    rows = app.page.evaluate("() => document.querySelectorAll('#view table.compare tbody tr').length")
    expect(rows == 4, 'Compare all four has %d rows' % rows)
    app.tab('table')
    app.last()
    app.page.click('#view .verdict .suggestion button')
    app.page.wait_for_function(
        "() => (document.querySelector('#variants input:checked') || {}).value === 'slr1'")
    out = app.outcome()
    expect(out.ok, 'analysis failed: %s' % out.errors)
    app.last()
    expect(app.current_tab() == 'parse', 'under SLR(1) End is on the %s tab' % app.current_tab())
    expect(app.page.query_selector('#view .verdict .suggestion') is None,
           'a clean table still suggests a method')
    text = app.view_text()
    expect('ACCEPT' in text, 'the SLR(1) parse does not accept')
    app.tab('table')
    expect(app.page.query_selector('#view .verdict .suggestion') is None,
           'the SLR(1) table banner still suggests a method')
    # No method works for this one, so there is nothing to suggest.
    app.choose_preset('Ambiguous expression')
    out = app.choose_variant('lr0')
    expect(out.ok, 'analysis failed: %s' % out.errors)
    app.last()
    expect(app.current_tab() == 'table', 'ambiguous: End is on the %s tab' % app.current_tab())
    expect(app.page.query_selector('#view .verdict') is not None, 'no verdict box')
    expect(app.page.query_selector('#view .verdict .suggestion') is None,
           'a suggestion is made where no method works')


@test()
def no_four_method_strip(app):
    """#summary and its per-method entries are gone."""
    app.choose_preset(EXPRESSION)
    expect(app.page.query_selector('#summary') is None, '#summary still exists')
    expect(app.page.query_selector('.summary-one') is None, '.summary-one still exists')


if __name__ == '__main__':
    raise SystemExit(main())

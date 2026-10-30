"""The Item Sets drawing: where it sits, what it may overlap, where a click lands.

Measures the drawing the page actually made (bounding boxes, points sampled along
every edge path) in the built `dist/index.html` over file://, in Chromium and Firefox.
Knows no parsing theory: it reads what the player drew.

Run:  python3 tests/browser/test_diagram.py [--browser firefox] [-k name]
"""

import re
import sys
from pathlib import Path

sys.dont_write_bytecode = True  # leave no __pycache__ behind in the tree
sys.path.insert(0, str(Path(__file__).resolve().parent))

from harness import VARIANTS, expect, main, test  # noqa: E402 - path tweak first

EXPRESSION = 'Expression grammar (classic)'
# Same-core states three columns apart, so the LALR(1) merge arrow must take a lane.
FAR_MERGE_GRAMMAR = 'S -> a A c | b b b A d\nA -> x'

FIT = '#diagram-tools button[aria-label="Fit the whole automaton in view"]'
FOLLOW = '#diagram-tools button[aria-label="Follow the newest state"]'

# ------------------------------------------------------------ in-page scripts

# Every step's summary, read by moving the slider across the whole log in one go.
SCAN_JS = r"""
() => {
  const scrub = document.getElementById('scrub');
  const total = Number(scrub.max) + 1;
  const out = [];
  for (let i = 0; i < total; i++) {
    scrub.value = String(i);
    scrub.dispatchEvent(new Event('input', { bubbles: true }));
    out.push(document.getElementById('explain-summary').textContent);
  }
  return out;
}
"""

# The drawing in its own units: state boxes, and per arrow the boxes its path crosses
# (sampled every 2 units, inset 2), its ends, long vertical x's and label box.
GEOMETRY_JS = r"""
() => {
  const svg = document.querySelector('#diagram-scroll svg');
  if (!svg) return null;
  const boxes = Array.from(svg.querySelectorAll('g.node[tabindex]')).map(g => {
    const m = /translate\(([-\d.]+),\s*([-\d.]+)\)/.exec(g.getAttribute('transform') || '');
    const f = g.querySelector('rect.frame');
    return { title: g.querySelector('text.title').textContent,
             cls: g.getAttribute('class'),
             x: Number(m[1]), y: Number(m[2]),
             w: Number(f.getAttribute('width')), h: Number(f.getAttribute('height')) };
  });
  const inside = (pt, b, by) =>
    pt.x > b.x + by && pt.x < b.x + b.w - by && pt.y > b.y + by && pt.y < b.y + b.h - by;
  const edges = Array.from(svg.querySelectorAll('g.edge')).map(g => {
    const path = g.querySelector('path');
    const length = path.getTotalLength();
    const through = new Set();
    let top = Infinity, bottom = -Infinity;
    for (let s = 0; s <= length; s += 2) {
      const pt = path.getPointAtLength(s);
      top = Math.min(top, pt.y);
      bottom = Math.max(bottom, pt.y);
      for (const b of boxes) if (inside(pt, b, 2)) through.add(b.title);
    }
    const nums = (path.getAttribute('d').match(/-?\d+(?:\.\d+)?/g) || []).map(Number);
    const pts = [];
    for (let i = 0; i + 1 < nums.length; i += 2) pts.push([nums[i], nums[i + 1]]);
    const verticals = new Set();
    for (let i = 1; i < pts.length; i++) {
      if (Math.abs(pts[i][0] - pts[i - 1][0]) < 0.01 && Math.abs(pts[i][1] - pts[i - 1][1]) > 20) {
        verticals.add(pts[i][0]);
      }
    }
    const bg = g.querySelector('rect.label-bg');
    const text = g.querySelector('text');
    const a = path.getPointAtLength(0), z = path.getPointAtLength(length);
    return {
      cls: g.getAttribute('class'),
      label: text ? text.textContent : '',
      through: Array.from(through),
      start: [a.x, a.y], end: [z.x, z.y], top: top, bottom: bottom,
      verticals: Array.from(verticals),
      labelBox: bg ? { x: Number(bg.getAttribute('x')), y: Number(bg.getAttribute('y')),
                       w: Number(bg.getAttribute('width')), h: Number(bg.getAttribute('height')) }
                   : null
    };
  });
  return { boxes: boxes, edges: edges,
           width: Number(svg.getAttribute('width')), height: Number(svg.getAttribute('height')) };
}
"""

# Bounding boxes on screen, in CSS pixels, of the first match of each selector.
RECTS_JS = r"""
selectors => {
  const out = {};
  for (const [name, selector] of Object.entries(selectors)) {
    const e = document.querySelector(selector);
    if (!e) { out[name] = null; continue; }
    const r = e.getBoundingClientRect();
    out[name] = { left: r.left, top: r.top, right: r.right, bottom: r.bottom,
                  width: r.width, height: r.height };
  }
  return out;
}
"""

# Everything the drawing holds that could sit under the toolbar.
DRAWN_RECTS_JS = r"""
() => Array.from(document.querySelectorAll(
    '#diagram-scroll svg g.edge path, #diagram-scroll svg g.edge rect, ' +
    '#diagram-scroll svg g.edge text, #diagram-scroll svg g.node')).map(e => {
  const r = e.getBoundingClientRect();
  const g = e.closest('g');
  return { what: e.localName + ' in ' + g.getAttribute('class') + ' "' +
                 (g.textContent || '').slice(0, 24) + '"',
           left: r.left, top: r.top, right: r.right, bottom: r.bottom };
})
"""

FRAMES_JS = r"""
n => new Promise(resolve => {
  const tick = k => k <= 0 ? resolve() : requestAnimationFrame(() => tick(k - 1));
  tick(n);
})
"""


# ------------------------------------------------------------------- helpers

def frames(app, n=2):
    """Wait `n` animation frames: a resize is acted on in the next one."""
    app.page.evaluate(FRAMES_JS, n)


def summaries(app):
    """Every step's summary line, in order; leaves the player on the last step."""
    lines = app.page.evaluate(SCAN_JS)
    frames(app, 1)
    return lines


def steps_matching(lines, pattern):
    """The indices of the steps whose summary matches `pattern`."""
    return [i for i, line in enumerate(lines) if re.search(pattern, line)]


def geometry(app, where):
    """The drawing's geometry, failing if there is no drawing."""
    data = app.page.evaluate(GEOMETRY_JS)
    expect(data is not None, '%s: there is no diagram on screen' % where)
    return data


def overlaps(a, b, slack=0.5):
    """Whether two {left, top, right, bottom} boxes share any area."""
    return (a['left'] < b['right'] - slack and b['left'] < a['right'] - slack and
            a['top'] < b['bottom'] - slack and b['top'] < a['bottom'] - slack)


def within(inner, outer, slack=1.0):
    """Whether `inner` lies inside `outer`, give or take a pixel."""
    return (inner['left'] >= outer['left'] - slack and inner['right'] <= outer['right'] + slack and
            inner['top'] >= outer['top'] - slack and inner['bottom'] <= outer['bottom'] + slack)


def as_rect(box):
    """A drawing-unit {x, y, w, h} box as {left, top, right, bottom}."""
    return {'left': box['x'], 'top': box['y'],
            'right': box['x'] + box['w'], 'bottom': box['y'] + box['h']}


def crossings(data):
    """'edge "label" runs through I7, I8' for every arrow that enters a box."""
    return ['%s "%s" runs through %s' % (e['cls'], e['label'], ', '.join(e['through']))
            for e in data['edges'] if e['through']]


def shared_verticals(data):
    """Each x that more than one lane is drawn along, as 'x=412 (a | b)'."""
    users = {}
    for e in data['edges']:
        if ' lane' not in ' ' + e['cls']:
            continue
        for x in e['verticals']:
            users.setdefault(round(x, 2), []).append(e['label'])
    return ['x=%s (%s)' % (x, ' | '.join(labels))
            for x, labels in sorted(users.items()) if len(labels) > 1]


def analyze_custom(app, grammar, test_string=''):
    """Type a grammar in and analyze it; fails unless the engine accepts it."""
    app.set_grammar(grammar)
    app.set_test_string(test_string)
    out = app.analyze()
    expect(out.ok, 'analysis failed: %s' % (out.errors or out.messages))
    return out


# ------------------------------------------- the counter, the stage and Fit

@test()
def diagram_ends_where_the_stage_does_under_the_merge_counter(app):
    """LALR(1): counter and caption above, and the drawing's box still in view."""
    app.choose_variant('lalr1')
    expect(app.choose_preset(EXPRESSION).ok, 'analysis failed')
    app.tab('items')
    lines = summaries(app)
    merges = steps_matching(lines, r'^Merge ')
    done = steps_matching(lines, r'^Merging done')
    expect(merges and done, 'the LALR(1) log has no merge steps')
    stages = (('canonical', merges[0] - 1), ('merging', merges[len(merges) // 2]),
              ('merged', done[0]))

    for width, height in ((1400, 900), (1920, 1080), (1366, 768), (1100, 900)):
        app.page.set_viewport_size({'width': width, 'height': height})
        frames(app)
        for stage, index in stages:
            where = '%dx%d, %s picture (step %d)' % (width, height, stage, index + 1)
            app.scrub_to(index)
            r = app.page.evaluate(RECTS_JS, {
                'view': '#view', 'counter': '#view .merge-counter',
                'caption': '#view .merge-caption', 'host': '#diagram-host',
                'scroller': '#diagram-scroll', 'inspector': '#inspector'})
            expect(r['counter'] and r['caption'], '%s: the merge counter is not shown' % where)
            expect(within(r['counter'], r['view']), '%s: the counter is outside the stage' % where)
            expect(r['counter']['bottom'] <= r['caption']['top'] + 1 and
                   r['caption']['bottom'] <= r['host']['top'] + 1,
                   '%s: counter, caption and drawing are not stacked in that order' % where)
            expect(r['host']['bottom'] <= r['view']['bottom'] + 1,
                   '%s: the drawing ends %.0f px below the stage'
                   % (where, r['host']['bottom'] - r['view']['bottom']))
            expect(r['inspector']['bottom'] <= r['view']['bottom'] + 1,
                   '%s: the inspector ends %.0f px below the stage'
                   % (where, r['inspector']['bottom'] - r['view']['bottom']))
            expect(r['scroller']['height'] >= 200,
                   '%s: only %.0f px of height is left for the drawing'
                   % (where, r['scroller']['height']))
            scroll = app.page.evaluate(
                "() => { const v = document.getElementById('view');"
                "        return v.scrollHeight - v.clientHeight; }")
            expect(scroll <= 1, '%s: the stage scrolls by %d px to show the drawing'
                   % (where, scroll))

    # Fit fits what is visible: a small automaton, a big window, the whole
    # drawing inside the scrolling box at no less than full size.
    app.page.set_viewport_size({'width': 1920, 'height': 1080})
    expect(app.choose_preset('Two halves').ok, 'analysis failed')
    app.tab('items')
    lines = summaries(app)
    app.scrub_to(steps_matching(lines, r'^Merge ')[-1])
    app.page.click(FIT)
    frames(app)
    r = app.page.evaluate(RECTS_JS, {'svg': '#diagram-scroll svg', 'scroller': '#diagram-scroll',
                                     'view': '#view'})
    expect(within(r['svg'], r['scroller']),
           'Fit left the drawing %r outside its box %r' % (r['svg'], r['scroller']))
    expect(within(r['scroller'], r['view']), 'the drawing box is not inside the stage')
    zoom = app.page.text_content('#diagram-tools .zoom-readout')
    expect(zoom and int(zoom.rstrip('%')) >= 100, 'Fit shrank a drawing that fits: %s' % zoom)
    app.note('counter + caption + drawing inside the stage at 4 window sizes x 3 pictures; '
             'Two halves fits whole at %s' % zoom)


# ------------------------------------------------------------- the toolbar

@test()
def nothing_is_drawn_under_the_diagram_toolbar(app):
    """Expression grammar at Fit: no edge, label or state under Follow / Fit / + / -."""
    app.page.set_viewport_size({'width': 1920, 'height': 1080})
    checked = 0
    for variant in ('lr0', 'clr1'):
        app.choose_variant(variant)
        expect(app.choose_preset(EXPRESSION).ok, 'analysis failed')
        app.end_of_phase('items')
        for mode, button in (('Fit', FIT), ('Follow', FOLLOW)):
            where = '%s, %s' % (variant, mode)
            app.page.click(button)
            frames(app)
            bar = app.page.evaluate(RECTS_JS, {'tools': '#diagram-tools',
                                               'scroller': '#diagram-scroll'})
            expect(bar['tools'] and bar['tools']['height'] > 20, '%s: no toolbar' % where)
            expect(not overlaps(bar['tools'], bar['scroller']),
                   '%s: the toolbar lies over the scrolling drawing' % where)
            # Only what is visible counts: the drawing scrolls inside its box.
            under = [d['what'] for d in app.page.evaluate(DRAWN_RECTS_JS)
                     if overlaps(d, bar['tools']) and overlaps(d, bar['scroller'])]
            expect(not under, '%s: drawn under the toolbar: %s' % (where, under[:4]))
            checked += 1
    app.note('toolbar clear of the drawing in %d views (LR(0) and CLR(1), Fit and Follow)'
             % checked)


# ----------------------------------------------------- table cell -> state

def selected_state(app, where):
    """The selected node: its title, and the arrows that end on its left side."""
    data = geometry(app, where)
    chosen = [b for b in data['boxes'] if ' selected' in ' ' + b['cls']]
    expect(len(chosen) == 1, '%s: %d states are selected' % (where, len(chosen)))
    box = chosen[0]
    incoming = [e for e in data['edges']
                if abs(e['end'][0] - box['x']) < 1.5 and
                box['y'] - 0.5 <= e['end'][1] <= box['y'] + box['h'] + 0.5]
    return box, incoming, data


def open_cell(app, row, where):
    """Click the first filled cell of table row `row`; returns the new position."""
    expect(app.end_of_phase('table') is not None, '%s: no table phase' % where)
    cell = app.page.locator('#view table.grid tbody tr').nth(row).locator('td.opens-state').first
    expect(cell.count() == 1, '%s: row I%d has no filled cell' % (where, row))
    total = app.position()[1]
    cell.click()
    expect(app.current_tab() == 'items', '%s: the cell did not open Item Sets' % where)
    index, now = app.position()
    expect(now == total, '%s: the click re-ran the analysis (%d steps, was %d)'
           % (where, now, total))
    return index


@test()
def table_cell_opens_its_state_already_connected(app):
    """The landing step shows the state closed *and* the edge that leads into it."""
    def summary():
        return app.page.text_content('#explain-summary') or ''

    def item_counts():
        return app.page.evaluate(
            """() => ({ node: document.querySelectorAll(
                          '#diagram-scroll g.node.selected text:not(.title)').length,
                        panel: document.querySelectorAll('#inspector .item-line .core').length })""")

    # LR(0) and CLR(1): the transition that first reaches the state.
    for variant in ('lr0', 'clr1'):
        app.choose_variant(variant)
        expect(app.choose_preset(EXPRESSION).ok, 'analysis failed')
        for row in (7, 4, 1):
            where = '%s, row I%d' % (variant, row)
            index = open_cell(app, row, where)
            expect(re.search(r' goes to I%d \(new state\)$' % row, summary()),
                   '%s: landed on step %d, "%s"' % (where, index + 1, summary()))
            box, incoming, data = selected_state(app, where)
            expect(box['title'] == 'I%d' % row, '%s: selected %s' % (where, box['title']))
            expect(len(incoming) >= 1, '%s: no drawn edge leads into the selected state' % where)
            current = [e for e in data['edges'] if ' current' in ' ' + e['cls']]
            expect(len(current) == 1 and current[0] in incoming,
                   '%s: the highlighted edge is not the one into the state' % where)
            counts = item_counts()
            expect(counts['node'] == counts['panel'] and counts['node'] > 0,
                   '%s: the node shows %d items, the inspector %d'
                   % (where, counts['node'], counts['panel']))
            expect(app.page.locator('#inspector .item-line.source-hit').count() == 1,
                   '%s: the source item is not marked' % where)
            # One step back is the state's own `stateClosed`: same state, all
            # its items, and not yet the edge.
            app.prev()
            expect(re.match(r'^I%d closed: ' % row, summary()),
                   '%s: the step before the landing step is "%s"' % (where, summary()))

        # The start state: nothing leads into it, so its own closing step.
        where = '%s, row I0' % variant
        index = open_cell(app, 0, where)
        expect(re.match(r'^I0 closed: ', summary()),
               '%s: landed on step %d, "%s"' % (where, index + 1, summary()))
        box, incoming, _ = selected_state(app, where)
        expect(box['title'] == 'I0' and not incoming,
               '%s: selected %s with %d incoming edges' % (where, box['title'], len(incoming)))

    # LALR(1): the table is numbered by merged ids, which exist from mergeDone.
    app.choose_variant('lalr1')
    expect(app.choose_preset(EXPRESSION).ok, 'analysis failed')
    for row in (7, 0):
        where = 'lalr1, row I%d' % row
        index = open_cell(app, row, where)
        expect(summary().startswith('Merging done'),
               '%s: landed on step %d, "%s"' % (where, index + 1, summary()))
        box, incoming, _ = selected_state(app, where)
        expect(re.match(r'^I%d\b' % row, box['title']), '%s: selected %s' % (where, box['title']))
        expect((len(incoming) >= 1) == (row != 0),
               '%s: %d edges lead into the selected state' % (where, len(incoming)))
    app.note('cells of I7, I4, I1 land on "goes to In (new state)" with the edge drawn and '
             'highlighted; I0 on its stateClosed; LALR(1) on mergeDone')


# ------------------------------------------------------------ merge arrows

def check_merge_steps(app, label, expect_lane=False):
    """Every merge step of the analysis on screen: arrows clear of every box."""
    app.tab('items')
    lines = summaries(app)
    merges = steps_matching(lines, r'^Merge ')
    expect(merges, '%s: no merge steps' % label)
    arrows = 0
    lanes = 0
    for index in merges:
        where = '%s, step %d "%s"' % (label, index + 1, lines[index])
        app.scrub_to(index)
        data = geometry(app, where)
        folds = [e for e in data['edges'] if 'merge-edge' in e['cls']]
        expect(folds, '%s: no merges-into arrow is drawn' % where)
        named = [e for e in folds if e['label'].startswith('merges into I')]
        expect(named, '%s: no arrow is labelled "merges into"' % where)
        bad = crossings(data)
        expect(not bad, '%s: %s' % (where, '; '.join(bad)))
        top = min(b['y'] for b in data['boxes'])
        bottom = max(b['y'] + b['h'] for b in data['boxes'])
        for e in folds:
            arrows += 1
            if e['top'] < top - 1 or e['bottom'] > bottom + 1:
                lanes += 1
            if e['labelBox']:
                covered = [b['title'] for b in data['boxes']
                           if overlaps(as_rect(e['labelBox']), as_rect(b))]
                expect(not covered, '%s: the "%s" label covers %s'
                       % (where, e['label'], ', '.join(covered)))
    if expect_lane:
        expect(lanes > 0, '%s: no merge arrow took a lane above or below the boxes' % label)
    return len(merges), arrows, lanes


@test()
def merge_arrows_go_round_the_boxes(app):
    """`merges into` never runs through a state: same column, next column, or far apart."""
    app.choose_variant('lalr1')
    report = []
    for preset in ('Merge casualty', 'Two halves', EXPRESSION, 'Pointers and assignment'):
        expect(app.choose_preset(preset).ok, '%s: analysis failed' % preset)
        steps, arrows, _ = check_merge_steps(app, preset)
        report.append('%s %d merges/%d arrows' % (preset, steps, arrows))
    analyze_custom(app, FAR_MERGE_GRAMMAR)
    steps, arrows, lanes = check_merge_steps(app, 'states three columns apart', expect_lane=True)
    report.append('far-apart grammar %d merges/%d arrows, %d by lane' % (steps, arrows, lanes))
    app.note('; '.join(report))


@test()
def merge_caption_and_arrow_name_the_same_states(app):
    """The caption's survivor is the id the arrow and the box header show."""
    app.choose_variant('lalr1')
    checked = 0
    for preset in ('Merge casualty', 'Pointers and assignment', 'Two halves', EXPRESSION):
        expect(app.choose_preset(preset).ok, '%s: analysis failed' % preset)
        app.tab('items')
        lines = summaries(app)
        merges = steps_matching(lines, r'^Merge ')
        expect(merges, '%s: no merge steps' % preset)
        for index in merges:
            where = '%s, step %d "%s"' % (preset, index + 1, lines[index])
            found = re.search(r'Merge (I\d+(?: \+ I\d+)+) into (I\d+)', lines[index])
            expect(found, '%s: caption not understood' % where)
            group = found.group(1).split(' + ')
            survivor = found.group(2)
            expect(survivor == group[0],
                   '%s: caption says the survivor is %s, the group starts at %s'
                   % (where, survivor, group[0]))
            app.scrub_to(index)
            data = geometry(app, where)
            labels = [e['label'] for e in data['edges'] if 'merge-edge' in e['cls']]
            expect('merges into ' + survivor in labels,
                   '%s: no arrow labelled "merges into %s" (have %s)' % (where, survivor, labels))
            titles = [b['title'] for b in data['boxes']]
            expect(any(t.startswith(survivor + ' + ') for t in titles),
                   '%s: no box title starts with "%s + " (have %s)' % (where, survivor, titles))
            checked += 1
    app.note('%d merge steps: caption, arrow and box header agree on the survivor' % checked)


GHOST_JS = r"""
() => {
  const svg = document.querySelector('#diagram-scroll svg');
  const title = g => (g.querySelector('text.title') || {}).textContent;
  const originals = Array.from(svg.querySelectorAll('g.node.absorbed'));
  return Array.from(svg.querySelectorAll('g.node.absorbing')).map(g => {
    const twin = originals.find(o => title(o) === title(g));
    return { title: title(g), attr: g.getAttribute('transform'),
             style: g.style.transform,
             twinAttr: twin ? twin.getAttribute('transform') : null };
  });
}
"""


@test()
def merge_ghost_fades_in_place(app):
    """The fading copy of an absorbed state is placed like the state it copies."""
    app.choose_variant('lalr1')
    expect(app.choose_preset('Merge casualty').ok, 'analysis failed')
    app.tab('items')
    lines = summaries(app)
    merges = steps_matching(lines, r'^Merge ')
    expect(merges, 'no merge steps')
    for index in merges:
        where = 'step %d "%s"' % (index + 1, lines[index])
        app.scrub_to(index)
        app.page.wait_for_timeout(800)
        ghosts = app.page.evaluate(GHOST_JS)
        expect(ghosts, '%s: no ghost is drawn' % where)
        for g in ghosts:
            expect(g['twinAttr'] is not None,
                   '%s: no absorbed original titled %s' % (where, g['title']))
            expect(not g['style'], '%s: ghost %s is placed by style.transform "%s"'
                   % (where, g['title'], g['style']))
            expect(g['attr'] == g['twinAttr'],
                   '%s: ghost %s transform attribute is %r, its original has %r'
                   % (where, g['title'], g['attr'], g['twinAttr']))
    app.note('%d merge steps: the ghost keeps its transform attribute' % len(merges))


# ------------------------------------------ every edge, every preset, lanes

@test()
def no_edge_crosses_a_box_and_no_two_lanes_share_a_line(app):
    """The finished automaton of every preset under every variant."""
    presets = app.preset_names()
    expect(len(presets) == 13, '%d presets in the menu' % len(presets))
    busiest = (0, '')
    drawings = 0
    for variant in VARIANTS:
        app.choose_variant(variant)
        for preset in presets:
            where = '%s under %s' % (preset, variant)
            expect(app.choose_preset(preset).ok, '%s: analysis failed' % where)
            expect(app.end_of_phase('items') is not None, '%s: no automaton phase' % where)
            data = geometry(app, where)
            bad = crossings(data)
            expect(not bad, '%s: %s' % (where, '; '.join(bad)))
            shared = shared_verticals(data)
            expect(not shared, '%s: lanes drawn along the same line: %s'
                   % (where, '; '.join(shared)))
            lanes = sum(1 for e in data['edges'] if ' lane' in ' ' + e['cls'])
            if lanes > busiest[0]:
                busiest = (lanes, where)
            drawings += 1
    app.note('%d finished automata: no edge through a box, every lane on its own vertical '
             '(busiest: %d lanes, %s)' % (drawings, busiest[0], busiest[1]))


# ----------------------------------------------------------- window resize

LAYOUT_JS = r"""
() => {
  const svg = document.querySelector('#diagram-scroll svg');
  return JSON.stringify([
    svg.getAttribute('viewBox'),
    Array.from(svg.querySelectorAll('g.node[tabindex]')).map(g => g.getAttribute('transform')),
    Array.from(svg.querySelectorAll('g.edge path')).map(p => p.getAttribute('d'))]);
}
"""


@test()
def diagram_refits_when_the_window_changes_size(app):
    """Fit follows the window, Follow keeps its state in view, the layout never moves."""
    big, small = {'width': 1920, 'height': 1080}, {'width': 1300, 'height': 700}
    app.page.set_viewport_size(big)
    expect(app.choose_preset('Two halves').ok, 'analysis failed')
    app.end_of_phase('items')
    app.page.click(FIT)
    frames(app)

    def read():
        r = app.page.evaluate(RECTS_JS, {'svg': '#diagram-scroll svg',
                                         'scroller': '#diagram-scroll'})
        r['zoom'] = app.page.text_content('#diagram-tools .zoom-readout')
        r['layout'] = app.page.evaluate(LAYOUT_JS)
        r['pressed'] = app.page.get_attribute(FIT, 'aria-pressed')
        return r

    at_big = read()
    expect(within(at_big['svg'], at_big['scroller']), 'the drawing does not fit at 1920x1080')

    app.page.set_viewport_size(small)
    frames(app)
    at_small = read()
    expect(at_small['scroller']['width'] < at_big['scroller']['width'] - 100,
           'the smaller window did not make the drawing box narrower')
    expect(at_small['svg']['width'] < at_big['svg']['width'] - 1,
           'Fit did not follow the window: still %s wide (zoom %s, was %s)'
           % (at_small['svg']['width'], at_small['zoom'], at_big['zoom']))
    expect(at_small['pressed'] == 'true', 'the resize left Fit mode')
    expect(at_small['layout'] == at_big['layout'], 'the resize changed the layout')

    app.page.set_viewport_size(big)
    frames(app)
    again = read()
    expect(again['zoom'] == at_big['zoom'] and
           abs(again['svg']['width'] - at_big['svg']['width']) < 1,
           'back at 1920x1080 the drawing is %s at %s, was %s at %s'
           % (again['svg']['width'], again['zoom'], at_big['svg']['width'], at_big['zoom']))
    expect(again['layout'] == at_big['layout'], 'resizing back changed the layout')

    # Follow: the state the current step is about stays in view.
    app.choose_variant('clr1')
    expect(app.choose_preset(EXPRESSION).ok, 'analysis failed')
    app.tab('items')
    lines = summaries(app)
    closing = steps_matching(lines, r'^I\d+ closed: ')
    expect(len(closing) > 4, 'too few states to follow')
    app.scrub_to(closing[-1])
    app.page.click(FOLLOW)
    app.page.wait_for_timeout(700)     # the smooth scroll that Follow starts
    frames(app)

    def followed():
        r = app.page.evaluate(RECTS_JS, {'node': '#diagram-scroll g.node.current rect.frame',
                                         'scroller': '#diagram-scroll'})
        expect(r['node'], 'no current state is drawn')
        return r

    r = followed()
    expect(within(r['node'], r['scroller'], slack=2), 'Follow did not bring the state into view')
    layout = app.page.evaluate(LAYOUT_JS)
    for size in ({'width': 1250, 'height': 640}, {'width': 1700, 'height': 1000}, small):
        app.page.set_viewport_size(size)
        frames(app, 3)
        r = followed()
        expect(within(r['node'], r['scroller'], slack=2),
               'at %dx%d the followed state is out of view: node %r, box %r'
               % (size['width'], size['height'], r['node'], r['scroller']))
        expect(app.page.get_attribute(FOLLOW, 'aria-pressed') == 'true',
               'the resize left Follow mode')
        expect(app.page.evaluate(LAYOUT_JS) == layout, 'the resize changed the layout')
    app.note('Fit: %s at 1920x1080, %s at 1300x700, %s again; Follow kept "%s" in view at 3 sizes'
             % (at_big['zoom'], at_small['zoom'], again['zoom'], lines[closing[-1]]))


if __name__ == '__main__':
    raise SystemExit(main())

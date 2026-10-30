/* SVG automaton renderer: LRV.views.diagram(steps, currentIndex, container).
 *
 * The layout is computed once from the finished automaton. States, edges and
 * closure items not yet built are not drawn, but their room is reserved.
 * Every node and edge is described as plain data and an element is reused only
 * when its description is identical, so stepping backwards removes what later
 * steps added.
 *
 * LALR(1) merging is drawn as a transition (canonical automaton, one fold at a
 * time, then the merged layout). The engine says what merged; this file only
 * redraws it. All SVG is built with createElementNS; no HTML strings.
 */
window.LRV = window.LRV || {};
window.LRV.views = window.LRV.views || {};

(function (LRV) {
  'use strict';
  var el = LRV.el;
  var svg = LRV.svg;

  // ------------------------------------------------------------- geometry
  // Sized for a projector: node text is 14px and the view never zooms below
  // MIN_SCALE, so the smallest text is about 12px. Character widths are DejaVu
  // Sans Mono's 0.602em advance and must match the font sizes in style.css.

  var CW = 8.45;          // one monospace character of an item core at 14px
  var CW_LA = 8.45;       // ...of a lookahead set, also 14px: secondary by colour, not size
  var CW_TITLE = 9.1;     // ...of a node title, 15px bold
  var CW_EDGE = 8.45;     // ...of an edge label, 14px
  var LINE_H = 20;
  var HEAD_H = 25;
  var PAD = 9;
  var MIN_NODE_W = 110;
  var H_GAP = 130;        // between columns: forward curves, lane verticals, labels
  var SIDE_GAP = 54;      // left of the first column and right of the last
  var V_GAP = 30;
  var LOOP_H = 50;        // headroom above a box that carries a self-loop
  var LANE_PAD = 26;      // first lane track's distance from the boxes
  var LANE_STEP = 24;     // between lane tracks; more than a label is tall
  var MARGIN = 16;
  var SLOT_INSET = 14;    // a lane vertical's distance from the column edge
  var SLOT_STEP = 10;
  var SLOTS = 5;          // verticals per gap side before they stand closer

  var MIN_SCALE = 0.85;
  var MAX_SCALE = 2.5;
  var FIT_MAX = 1.25;     // Fit may enlarge a small automaton, but not absurdly
  var ZOOM_STEP = 1.25;

  /* Every item is drawn; the cap only guards against pathological grammars. */
  var MAX_LINES = 200;
  /* Past this a lookahead set is elided in the node; the panel shows it whole. */
  var MAX_LOOKAHEAD_CHARS = 22;

  /* Zoom, scroll and follow mode are kept per container; reset on a new analysis. */
  var viewCache = new WeakMap();

  // ------------------------------------------------------------ step replay

  /** The finished automaton, plus the step index at which each part appears. */
  function collect(steps) {
    var states = [];
    var edges = [];
    var merges = [];
    var mergeDone = null;
    var byId = {};
    steps.forEach(function (s) {
      if (s.kind === 'stateCreated') {
        var st = {
          id: s.id,
          kernel: s.kernel || [],
          items: (s.kernel || []).slice(),
          closureAt: {},          // item key -> step index it was added
          because: {},            // item key -> the item that caused it
          fromState: s.fromState,
          onSymbol: s.onSymbol,
          bornAt: s.index
        };
        states[s.id] = st;
        byId[s.id] = st;
      } else if (s.kind === 'closureItemAdded') {
        var target = byId[s.state];
        if (!target) return;
        target.items.push(s.item);
        target.closureAt[itemKey(s.item)] = s.index;
        target.because[itemKey(s.item)] = s.becauseOf;
      } else if (s.kind === 'stateClosed') {
        var t2 = byId[s.id];
        if (t2 && s.items) t2.items = s.items;
      } else if (s.kind === 'transitionAdded') {
        edges.push({ from: s.from, to: s.to, symbol: s.symbol, bornAt: s.index });
      } else if (s.kind === 'statesMerged') {
        merges.push({
          mergedIds: s.mergedIds || [],
          newId: s.newId,
          items: s.items || [],
          bornAt: s.index
        });
      } else if (s.kind === 'mergeDone') {
        mergeDone = {
          groups: s.groups || [],
          before: s.before,
          after: s.after,
          beforeLabel: s.beforeLabel,
          afterLabel: s.afterLabel,
          bornAt: s.index
        };
      }
    });
    return {
      states: states.filter(Boolean),
      edges: edges,
      merges: merges,
      mergeDone: mergeDone
    };
  }

  /* Keyed by core alone: a core appears at most once per state. */
  function itemKey(item) {
    return item.production + ':' + item.dot;
  }

  function isKernel(state, item) {
    var k = itemKey(item);
    for (var i = 0; i < state.kernel.length; i++) {
      if (itemKey(state.kernel[i]) === k) return true;
    }
    return false;
  }

  /** Has this item been added to its state by step `index`? */
  function itemBuilt(state, item, index) {
    if (isKernel(state, item)) return true;
    var at = state.closureAt[itemKey(item)];
    return at === undefined || at <= index;
  }

  /** Parallel edges (same source and target) become one arrow. */
  function groupEdges(edges) {
    var out = [];
    var at = Object.create(null);
    edges.forEach(function (e) {
      var key = e.from + '>' + e.to;
      var grp = at[key];
      if (!grp) {
        grp = { from: e.from, to: e.to, key: key, symbols: [] };
        at[key] = grp;
        out.push(grp);
      }
      grp.symbols.push({ symbol: e.symbol, bornAt: e.bornAt });
    });
    return out;
  }

  // ---------------------------------------------------------------- sizing

  /** The two halves of an item as drawn in a node, as runs; `la` is elided past MAX_LOOKAHEAD_CHARS. */
  function nodeLine(g, item) {
    var core = LRV.itemCoreRuns(g, item);
    var la = LRV.itemLookaheadsRuns(item);
    var coreLength = LRV.flat(core).length;
    var laText = LRV.flat(la);
    if (laText.length > MAX_LOOKAHEAD_CHARS) {
      // Cut as one string; safe because the lookaheads sit in their own isolating <tspan>.
      laText = laText.slice(0, MAX_LOOKAHEAD_CHARS - 2) + '…}';
      la = [laText];
    }
    return { core: core, la: la, width: coreLength * CW + laText.length * CW_LA };
  }

  /** What one node draws: every item, up to the safety cap (kernel items kept first). */
  function nodeLines(g, state) {
    var all = state.items.map(function (it) {
      var line = nodeLine(g, it);
      line.item = it;
      line.kernel = isKernel(state, it);
      return line;
    });
    if (all.length <= MAX_LINES + 1) return { lines: all, hidden: 0 };
    var kernels = all.filter(function (l) { return l.kernel; });
    var rest = all.filter(function (l) { return !l.kernel; });
    var shown = kernels.concat(rest).slice(0, MAX_LINES);
    shown.sort(function (a, b) { return all.indexOf(a) - all.indexOf(b); });
    return { lines: shown, hidden: all.length - shown.length };
  }

  function sizeNode(g, s) {
    var drawn = nodeLines(g, s);
    // Sized from the merged contents so a merge never moves a node; `preItems` is what it shows until then.
    var pre = s.preItems ? nodeLines(g, { items: s.preItems, kernel: s.kernel }) : drawn;
    var widths = drawn.lines.map(function (l) { return l.width; });
    if (drawn.hidden) widths.push(('+' + drawn.hidden + ' more').length * CW);
    // Sized from the widest title the node will carry, so merging does not resize it.
    var widest = title({ id: s.id, absorbed: s.absorbed || s.willAbsorb, mergedFrom: s.mergedFrom });
    widths.push(widest.length * CW_TITLE);
    var rows = drawn.lines.length + (drawn.hidden ? 1 : 0);
    return {
      state: s,
      drawn: drawn,
      drawnPre: pre,
      w: Math.ceil(Math.max(MIN_NODE_W, Math.max.apply(null, widths) + PAD * 2)),
      h: HEAD_H + rows * LINE_H + PAD,
      layer: -1
    };
  }

  // ---------------------------------------------------------------- layout

  /** Node positions, from the finished automaton. `layoutEdges` fix columns and rows
   *  (the canonical edges before mergeDone, so a merge never moves a node);
   *  `reserveSets` size the lanes for the busiest routing; `into` maps absorbed
   *  states to survivors for the merge arrows. */
  function layout(g, model, layoutEdges, reserveSets, into) {
    var nodes = {};
    model.states.forEach(function (s) { nodes[s.id] = sizeNode(g, s); });

    var loops = {};
    reserveSets.forEach(function (set) {
      set.forEach(function (e) { if (e.from === e.to) loops[e.from] = true; });
    });

    // Columns: BFS distance from I0, so no forward edge jumps more than one column.
    var adjacency = {};
    var preds = {};
    layoutEdges.forEach(function (e) {
      if (e.from === e.to) return;
      (adjacency[e.from] = adjacency[e.from] || []).push(e.to);
      (preds[e.to] = preds[e.to] || []).push(e.from);
    });
    var queue = [0];
    if (nodes[0]) nodes[0].layer = 0;
    while (queue.length) {
      var id = queue.shift();
      (adjacency[id] || []).forEach(function (next) {
        if (nodes[next] && nodes[next].layer < 0) {
          nodes[next].layer = nodes[id].layer + 1;
          queue.push(next);
        }
      });
    }
    // Anything unreachable (should not happen) goes in its own final column.
    var maxLayer = 0;
    Object.keys(nodes).forEach(function (k) { maxLayer = Math.max(maxLayer, nodes[k].layer); });
    Object.keys(nodes).forEach(function (k) {
      if (nodes[k].layer < 0) nodes[k].layer = maxLayer + 1;
    });

    var layers = [];
    Object.keys(nodes).forEach(function (k) {
      var n = nodes[k];
      (layers[n.layer] = layers[n.layer] || []).push(n);
    });

    // Rows: ordered by the barycentre of already placed parents; ties by state number.
    var tallest = 0;
    layers.forEach(function (col) {
      col.forEach(function (n) {
        var ps = (preds[n.state.id] || []).filter(function (p) {
          return nodes[p] && nodes[p].rel !== undefined;
        });
        n.bary = ps.length
          ? ps.reduce(function (sum, p) { return sum + nodes[p].rel + nodes[p].h / 2; }, 0) / ps.length
          : Infinity;
      });
      col.sort(function (a, b) {
        if (a.bary !== b.bary) return a.bary < b.bary ? -1 : 1;
        return a.state.id - b.state.id;
      });
      var y = 0;
      col.forEach(function (n) {
        if (loops[n.state.id]) y += LOOP_H;
        n.top = y;
        y += n.h + V_GAP;
      });
      var height = y - V_GAP;
      col.forEach(function (n) { n.rel = n.top - height / 2; });
      tallest = Math.max(tallest, height);
    });

    var x = MARGIN + SIDE_GAP;
    var cols = [];                      // indexed by layer, like `layers`
    layers.forEach(function (col, li) {
      var widest = Math.max.apply(null, col.map(function (n) { return n.w; }));
      col.forEach(function (n) {
        n.x = x;
        n.y = n.rel + tallest / 2;      // provisional: lanes shift it below
      });
      cols[li] = { left: x, right: x + widest };
      x += widest + H_GAP;
    });
    var width = x - H_GAP + SIDE_GAP + MARGIN;

    var geom = {
      nodes: nodes,
      cols: cols,
      width: Math.ceil(width),
      boxesTop: 0,
      boxesBottom: tallest,
      loops: loops
    };

    // Reserve the lanes, then move everything down below the upper ones.
    var above = 0;
    var below = 0;
    reserveSets.forEach(function (set) {
      var r = classify(geom, groupEdges(set));
      above = Math.max(above, r.above);
      below = Math.max(below, r.below);
    });
    // Merge arrows are classified while the lanes are still being counted,
    // because one that takes a lane needs a track outside every edge's.
    var folds = into ? classifyFolds(geom, into, above, below) : null;
    if (folds) { above = folds.above; below = folds.below; }

    var shift = MARGIN + (above ? LANE_PAD + (above - 1) * LANE_STEP + 12 : 0);
    Object.keys(nodes).forEach(function (k) { nodes[k].y += shift; });
    geom.boxesTop = shift;
    geom.boxesBottom = shift + tallest;
    geom.height = Math.ceil(geom.boxesBottom + (below ? LANE_PAD + (below - 1) * LANE_STEP + 12 : 0) + MARGIN);
    geom.folds = folds ? routeFolds(geom, folds.list) : null;
    return geom;
  }

  // ---------------------------------------------------------------- routing
  // Three kinds of edge, none crossing a box:
  //   direct - to the next column: a curve across one gap;
  //   loop   - to itself: a small loop above the box;
  //   lane   - anything else: out of the right side, along a lane above or below
  //            every box, and in on the target's left.

  /** Decide each edge's kind, side and lane track. Mutates the groups. */
  function classify(geom, groups) {
    var nodes = geom.nodes;
    var mid = (geom.boxesTop + geom.boxesBottom) / 2;
    var lanes = { above: [], below: [] };
    groups.forEach(function (e) {
      var a = nodes[e.from];
      var b = nodes[e.to];
      if (!a || !b) { e.kind = 'none'; return; }
      if (a === b) { e.kind = 'loop'; return; }
      if (b.layer === a.layer + 1) { e.kind = 'direct'; return; }
      e.kind = 'lane';
      e.above = (a.y + a.h / 2 + b.y + b.h / 2) / 2 <= mid;
      e.span = Math.abs(a.layer - b.layer);
      lanes[e.above ? 'above' : 'below'].push(e);
    });
    // Short spans take the inner tracks, so they do not have to cross the
    // long ones on the way in and out.
    ['above', 'below'].forEach(function (side) {
      lanes[side].sort(function (p, q) {
        return p.span - q.span || p.from - q.from || p.to - q.to;
      });
      lanes[side].forEach(function (e, i) { e.track = i; });
    });
    return { above: lanes.above.length, below: lanes.below.length };
  }

  /** Geometry for every edge group: a path and a label anchor. */
  function route(geom, groups) {
    var nodes = geom.nodes;
    classify(geom, groups);

    // Ports: outgoing edges leave the right side, incoming ones enter the left,
    // spread in order so arrowheads do not pile up.
    var outs = {};
    var ins = {};
    function laneKey(e) { return e.above ? -1e6 - e.track : 1e6 + e.track; }
    groups.forEach(function (e) {
      if (e.kind !== 'direct' && e.kind !== 'lane') return;
      var a = nodes[e.from];
      var b = nodes[e.to];
      (outs[e.from] = outs[e.from] || []).push({ e: e, key: e.kind === 'lane' ? laneKey(e) : b.y + b.h / 2 });
      (ins[e.to] = ins[e.to] || []).push({ e: e, key: e.kind === 'lane' ? laneKey(e) : a.y + a.h / 2 });
    });
    function spread(list, n, prop) {
      list.sort(function (p, q) { return p.key - q.key || p.e.from - q.e.from || p.e.to - q.e.to; });
      var lo = n.y + Math.min(HEAD_H, n.h / 4);
      var hi = n.y + n.h - 6;
      list.forEach(function (entry, i) {
        entry.e[prop] = lo + (hi - lo) * (i + 1) / (list.length + 1);
      });
    }
    Object.keys(outs).forEach(function (k) { spread(outs[k], nodes[k], 'py'); });
    Object.keys(ins).forEach(function (k) { spread(ins[k], nodes[k], 'qy'); });

    // Lane verticals: each gap side hands out x positions in track order; past
    // SLOTS they share the width rather than wrap onto a vertical already taken.
    var lanesInOrder = groups.filter(function (e) { return e.kind === 'lane'; })
      .sort(function (p, q) { return (p.above === q.above ? 0 : p.above ? -1 : 1) || p.track - q.track; });
    var exits = { used: {}, total: {} };
    var entries = { used: {}, total: {} };
    lanesInOrder.forEach(function (e) {
      var out = nodes[e.from].layer;
      var inn = nodes[e.to].layer;
      exits.total[out] = (exits.total[out] || 0) + 1;
      entries.total[inn] = (entries.total[inn] || 0) + 1;
    });
    function slot(side, col) {
      var n = side.used[col] || 0;
      side.used[col] = n + 1;
      var step = Math.min(SLOT_STEP, SLOT_STEP * (SLOTS - 1) / Math.max(1, side.total[col] - 1));
      return SLOT_INSET + n * step;
    }
    lanesInOrder.forEach(function (e) {
      e.xv1 = geom.cols[nodes[e.from].layer].right + slot(exits, nodes[e.from].layer);
      e.xv2 = geom.cols[nodes[e.to].layer].left - slot(entries, nodes[e.to].layer);
    });

    groups.forEach(function (e) {
      var a = nodes[e.from];
      var b = nodes[e.to];
      if (e.kind === 'loop') {
        // A small loop standing on the top edge, towards the right so it
        // stays clear of the title.
        var cx = a.x + a.w - 34;
        var top = a.y;
        e.d = 'M ' + (cx + 10) + ' ' + top +
              ' C ' + (cx + 22) + ' ' + (top - 38) + ', ' +
              (cx - 22) + ' ' + (top - 38) + ', ' + (cx - 10) + ' ' + top;
        e.lx = cx;
        e.ly = top - 34;
      } else if (e.kind === 'direct') {
        var x1 = a.x + a.w;
        var x2 = b.x;
        var m = (x1 + x2) / 2;
        e.d = 'M ' + x1 + ' ' + e.py + ' C ' + m + ' ' + e.py + ', ' + m + ' ' + e.qy + ', ' + x2 + ' ' + e.qy;
        e.lx = m;
        e.ly = (e.py + e.qy) / 2 + 5;
      } else if (e.kind === 'lane') {
        var ly = e.above
          ? geom.boxesTop - LANE_PAD - e.track * LANE_STEP
          : geom.boxesBottom + LANE_PAD + e.track * LANE_STEP;
        e.d = roundedPath([
          [a.x + a.w, e.py], [e.xv1, e.py], [e.xv1, ly],
          [e.xv2, ly], [e.xv2, e.qy], [b.x, e.qy]
        ], 8);
        e.lx = (e.xv1 + e.xv2) / 2;
        e.ly = ly + 5;
      }
    });
    return groups;
  }

  /** An orthogonal polyline with its corners rounded off. */
  function roundedPath(pts, r) {
    var d = 'M ' + pts[0][0] + ' ' + pts[0][1];
    for (var i = 1; i < pts.length - 1; i++) {
      var p = pts[i - 1], c = pts[i], n = pts[i + 1];
      var l1 = Math.hypot(c[0] - p[0], c[1] - p[1]);
      var l2 = Math.hypot(n[0] - c[0], n[1] - c[1]);
      var rr = Math.min(r, l1 / 2, l2 / 2);
      if (!l1 || !l2 || rr <= 0) { d += ' L ' + c[0] + ' ' + c[1]; continue; }
      var ax = c[0] - (c[0] - p[0]) / l1 * rr, ay = c[1] - (c[1] - p[1]) / l1 * rr;
      var bx = c[0] + (n[0] - c[0]) / l2 * rr, by = c[1] + (n[1] - c[1]) / l2 * rr;
      d += ' L ' + ax + ' ' + ay + ' Q ' + c[0] + ' ' + c[1] + ' ' + bx + ' ' + by;
    }
    var last = pts[pts.length - 1];
    return d + ' L ' + last[0] + ' ' + last[1];
  }

  // ---- the `merges into` arrows (LALR(1) merging stage) --------------------
  // Header to header, never crossing a box. Routed by where the boxes sit:
  //   side   - same column: out of the left side, along the gap, back in;
  //   direct - neighbouring columns: a curve across the gap;
  //   lane   - further apart: a lane on a track outside all the edges' tracks.
  // Verticals sit half a slot off the edge lanes' so an arrow never lies on an edge.

  var FOLD_INSET = SLOT_INSET + SLOT_STEP / 2;
  var FOLD_SLOTS = SLOTS - 1;

  /** Decide how each merge arrow is routed; lane arrows get a track beyond the edges' reserved ones. */
  function classifyFolds(geom, into, above, below) {
    var nodes = geom.nodes;
    var mid = (geom.boxesTop + geom.boxesBottom) / 2;
    var sideSlots = {};
    var exitSlots = {};
    var entrySlots = {};
    function slot(table, col) {
      var n = table[col] || 0;
      table[col] = n + 1;
      return FOLD_INSET + (n % FOLD_SLOTS) * SLOT_STEP;
    }
    var list = [];
    Object.keys(into).map(Number).sort(function (p, q) { return p - q; }).forEach(function (id) {
      var a = nodes[id];
      var b = nodes[into[id]];
      if (!a || !b || a === b) return;
      var f = { from: id, to: into[id] };
      var apart = Math.abs(a.layer - b.layer);
      if (apart === 0) {
        f.kind = 'side';
        f.inset = slot(sideSlots, a.layer);
      } else if (apart === 1) {
        f.kind = 'direct';
      } else {
        f.kind = 'lane';
        f.above = (a.y + a.h / 2 + b.y + b.h / 2) / 2 <= mid;
        f.track = f.above ? above++ : below++;
        f.exit = slot(exitSlots, a.layer);
        f.entry = slot(entrySlots, b.layer);
      }
      list.push(f);
    });
    return { list: list, above: above, below: below };
  }

  /** A path and a label anchor for every merge arrow, keyed by the absorbed state's id. */
  function routeFolds(geom, list) {
    var nodes = geom.nodes;
    var out = {};
    list.forEach(function (f) {
      var a = nodes[f.from];
      var b = nodes[f.to];
      var fy = a.y + HEAD_H / 2;
      var ty = b.y + HEAD_H / 2;
      var d, lx, ly;
      var room = null;
      if (f.kind === 'side') {
        var xg = geom.cols[a.layer].left - f.inset;
        d = roundedPath([[a.x, fy], [xg, fy], [xg, ty], [b.x, ty]], 8);
        // The label sits on the vertical, in a strip that holds no box.
        lx = xg;
        ly = (ty < fy ? a.y - V_GAP / 2 : a.y + a.h + V_GAP / 2) + 5;
      } else if (f.kind === 'direct') {
        // Out of the side facing the other column; a narrower box first runs level to the column edge.
        var right = b.layer > a.layer;
        var x1 = right ? geom.cols[a.layer].right : a.x;
        var x2 = right ? b.x : geom.cols[b.layer].right;
        var m = (x1 + x2) / 2;
        d = (right && a.x + a.w < x1 ? 'M ' + (a.x + a.w) + ' ' + fy + ' L ' : 'M ') + x1 + ' ' + fy +
            ' C ' + m + ' ' + fy + ', ' + m + ' ' + ty + ', ' + x2 + ' ' + ty +
            (!right && b.x + b.w < x2 ? ' L ' + (b.x + b.w) + ' ' + ty : '');
        lx = m;
        ly = (fy + ty) / 2 + 5;
        // In the middle of the gap, and no wider than it.
        room = Math.abs(x2 - x1) - 2;
      } else {
        var lane = f.above
          ? geom.boxesTop - LANE_PAD - f.track * LANE_STEP
          : geom.boxesBottom + LANE_PAD + f.track * LANE_STEP;
        var xv1 = geom.cols[a.layer].right + f.exit;
        var xv2 = geom.cols[b.layer].left - f.entry;
        d = roundedPath([[a.x + a.w, fy], [xv1, fy], [xv1, lane], [xv2, lane], [xv2, ty], [b.x, ty]], 8);
        lx = (xv1 + xv2) / 2;
        ly = lane + 5;
      }
      out[f.from] = { to: f.to, d: d, lx: lx, ly: ly, room: room };
    });
    return out;
  }

  // ---------------------------------------------------------------- drawing

  /** A node's header; a state that has absorbed others lists all its CLR(1) numbers. */
  function title(s) {
    if (s.absorbed && s.absorbed.length) {
      return 'I' + s.id + ' + ' + s.absorbed.map(function (i) { return 'I' + i; }).join(' + ');
    }
    if (s.mergedFrom && s.mergedFrom.length > 1) {
      return 'I' + s.id + ' ← ' + s.mergedFrom.map(function (i) { return 'I' + i; }).join('+');
    }
    return 'I' + s.id;
  }

  /* Nodes and edges are built from plain-data descriptions (nodeSpec / edgeSpec /
     mergeArrowSpec), so a drawing can be kept across steps. */

  /** What one node shows at step `index`; closure items added later are left out. */
  function nodeSpec(node, index, extraClass, drawn, interactive) {
    var s = node.state;
    drawn = drawn || node.drawn;
    var lines = [];
    drawn.lines.forEach(function (line, i) {
      if (!itemBuilt(s, line.item, index)) return;
      lines.push({ row: i, kernel: line.kernel, core: line.core, la: line.la });
    });
    return {
      id: s.id,
      cls: 'node' + (extraClass ? ' ' + extraClass : '') +
           (s.absorbed || (s.mergedFrom && s.mergedFrom.length > 1) ? ' merged' : ''),
      x: node.x,
      y: node.y,
      w: node.w,
      h: node.h,
      interactive: !!interactive,
      label: interactive
        ? 'State ' + title(s) + ', ' + s.items.length + ' item' + (s.items.length === 1 ? '' : 's') +
          (s.mergedFrom && s.mergedFrom.length > 1
            ? ', merged from ' + s.mergedFrom.length + ' CLR(1) states' : '') +
          (drawn.hidden ? ', truncated - select it for the full set' : '')
        : null,
      title: title(s),
      lines: lines,
      more: drawn.hidden
        ? { row: drawn.lines.length, text: '+' + drawn.hidden + ' more' }
        : null
    };
  }

  /** One node, from its description alone. */
  function buildNode(spec) {
    var group = svg('g', {
      class: spec.cls,
      transform: 'translate(' + spec.x + ',' + spec.y + ')',
      tabindex: spec.interactive ? '0' : null,
      role: spec.interactive ? 'button' : null,
      'aria-hidden': spec.interactive ? null : 'true',
      'aria-label': spec.label
    });

    group.appendChild(svg('rect', { class: 'frame', x: 0, y: 0, width: spec.w, height: spec.h, rx: 5 }));
    group.appendChild(svg('rect', { class: 'hdr', x: 1, y: 1, width: spec.w - 2, height: HEAD_H - 1, rx: 4 }));
    group.appendChild(svg('text', { class: 'title', x: PAD, y: 18, text: spec.title }));

    spec.lines.forEach(function (line) {
      var y = HEAD_H + line.row * LINE_H;
      if (line.kernel) {
        group.appendChild(svg('rect', {
          class: 'kernel-bg', x: 1, y: y + 1, width: spec.w - 2, height: LINE_H
        }));
      }
      // Core and lookaheads are two tspans in one text node, so the set reads as secondary.
      var text = svg('text', { x: PAD, y: y + 15 });
      text.appendChild(svg('tspan', {}, LRV.rich(line.core, true)));
      if (line.la.length) text.appendChild(svg('tspan', { class: 'la' }, LRV.rich(line.la, true)));
      group.appendChild(text);
    });

    if (spec.more) {
      group.appendChild(svg('text', {
        class: 'more',
        x: PAD,
        y: HEAD_H + spec.more.row * LINE_H + 15,
        text: spec.more.text
      }));
    }
    return group;
  }

  /** A label on an arrow. `room` caps the opaque backing's width so it does not blank neighbouring boxes. */
  function edgeLabel(group, label, lx, ly, room) {
    var length = LRV.flat(label).length;
    var width = length * CW_EDGE + 8;
    if (room && width > room) width = room;
    group.appendChild(svg('rect', {
      class: 'label-bg',
      x: lx - width / 2,
      y: ly - 14,
      width: width,
      height: 19,
      rx: 3
    }));
    group.appendChild(svg('text', { x: lx, y: ly, 'text-anchor': 'middle' }, LRV.rich(label, true)));
  }

  /** One arrow for a group of parallel edges; only symbols added by the current step are labelled. */
  function edgeSpec(e, symbols, isCurrent) {
    return {
      cls: 'edge ' + e.kind + (isCurrent ? ' current' : ''),
      d: e.d,
      label: LRV.runs(LRV.symList(symbols, ', ')),
      lx: e.lx,
      ly: e.ly
    };
  }

  /** The dashed arrow from an absorbed state to its survivor, header to header; `fold` is its route. */
  function mergeArrowSpec(fold, labelled) {
    return {
      cls: 'edge merge-edge' + (labelled ? '' : ' past'),
      d: fold.d,
      label: labelled ? ['merges into I' + fold.to] : null,
      lx: fold.lx,
      ly: fold.ly,
      room: fold.room
    };
  }

  /** One arrow, of either kind, from its description alone. */
  function buildEdge(spec) {
    var group = svg('g', { class: spec.cls });
    group.appendChild(svg('path', { d: spec.d, 'marker-end': 'url(#lrv-arrow)' }));
    if (spec.label !== null) edgeLabel(group, spec.label, spec.lx, spec.ly, spec.room);
    return group;
  }

  /** Make `layer` hold exactly the elements `wanted` describes, in order, reusing an
   *  element only if its description is identical. */
  function reconcile(layer, kept, wanted, build) {
    var plan = wanted.map(function (w) {
      var sig = JSON.stringify(w.spec);
      var old = kept.get(w.key);
      return { key: w.key, sig: sig, el: old && old.sig === sig ? old.el : null, spec: w.spec };
    });
    var staying = new Set();
    plan.forEach(function (item) { if (item.el) staying.add(item.el); });

    kept.clear();
    var cursor = layer.firstChild;
    // Remove unwanted elements first so the survivors stay in order (moving one costs a re-layout).
    function dropUnwanted() {
      while (cursor && !staying.has(cursor)) {
        var after = cursor.nextSibling;
        layer.removeChild(cursor);
        cursor = after;
      }
    }
    plan.forEach(function (item) {
      dropUnwanted();
      var node = item.el || build(item.spec);
      if (node === cursor) cursor = cursor.nextSibling;
      else layer.insertBefore(node, cursor);
      kept.set(item.key, { sig: item.sig, el: node });
    });
    dropUnwanted();
  }

  // -------------------------------------------------------------- inspector

  /** One item as two spans: the core, then the lookahead set as secondary; never truncated. */
  function itemLine(g, item, extraClass) {
    var line = el('div', { class: 'item-line' + (extraClass ? ' ' + extraClass : '') });
    line.appendChild(el('span', { class: 'core' }, LRV.rich(LRV.itemCoreRuns(g, item))));
    var la = LRV.itemLookaheadsRuns(item);
    if (la.length) line.appendChild(el('span', { class: 'la' }, LRV.rich(la)));
    return line;
  }

  /** Fill the side panel for one state; `hit` is the item a table cell came from, marked. Returns that line. */
  function describeState(g, model, node, container, index, hit) {
    var s = node.state;
    LRV.clear(container);
    container.appendChild(el('h3', { text: 'State ' + title(s) }));

    if (model.into && model.into[s.id] !== undefined) {
      container.appendChild(el('p', {
        class: 'note merged-note',
        text: 'Merged into I' + model.into[s.id] + ': it had the same core. These are the items it had before the merge.'
      }));
    }
    if (s.mergedFrom && s.mergedFrom.length > 1) {
      container.appendChild(el('p', {
        class: 'note merged-note',
        text: 'Merged from CLR(1) states ' +
              s.mergedFrom.map(function (i) { return 'I' + i; }).join(', ') +
              ' — same items, same dots, different lookaheads. The sets below are the union.'
      }));
    }
    if (s.fromState === null || s.fromState === undefined) {
      container.appendChild(el('p', { class: 'note', text: 'The start state: the closure of the augmented production.' }));
    } else {
      container.appendChild(el('p', { class: 'note' },
        LRV.rich(['Reached as GOTO(I' + s.fromState + ', ', LRV.sym(s.onSymbol), ').'])));
    }
    if (node.drawn && node.drawn.hidden) {
      container.appendChild(el('p', {
        class: 'note',
        text: 'The diagram shows ' + node.drawn.lines.length + ' of these ' +
              s.items.length + ' items. All of them are here.'
      }));
    }

    var items = (model.into && model.into[s.id] !== undefined && s.preItems) ? s.preItems : s.items;
    items = items.filter(function (item) { return itemBuilt(s, item, index); });
    // The cell names its item exactly; the core is a fallback if lookahead sets are spelt differently.
    var hitText = hit ? LRV.itemText(g, hit) : null;
    var hitCore = hit ? LRV.itemCore(g, hit) : null;
    var exact = hit && items.some(function (item) { return LRV.itemText(g, item) === hitText; });
    var marked = null;

    var list = el('div');
    items.forEach(function (item) {
      var classes = isKernel(s, item) ? ['kernel'] : [];
      var isHit = hit && !marked &&
        (exact ? LRV.itemText(g, item) === hitText : LRV.itemCore(g, item) === hitCore);
      if (isHit) classes.push('source-hit');
      var line = itemLine(g, item, classes.join(' '));
      if (isHit) {
        marked = line;
        line.setAttribute('aria-label', 'Source of the selected table entry: ' + LRV.itemText(g, item));
      }
      list.appendChild(line);
      var cause = s.because[itemKey(item)];
      if (cause) {
        list.appendChild(el('span', { class: 'because' },
          LRV.rich(['added by closure, because of ', LRV.itemRuns(g, cause)])));
      }
    });
    container.appendChild(list);

    var out = model.edges.filter(function (e) { return e.from === s.id && e.bornAt <= index; });
    if (out.length) {
      container.appendChild(el('h3', { text: 'Transitions', style: { marginTop: '12px' } }));
      out.forEach(function (e) {
        container.appendChild(el('div', { class: 'item-line' },
          LRV.rich(['on ', LRV.sym(e.symbol), '  →  I' + e.to])));
      });
    }

    var legend = el('p', { class: 'legend' }, [
      el('span', { class: 'swatch kernel' }),
      'shaded = kernel item (the dot is not at the start)'
    ]);
    // Only worth saying when there are lookaheads on screen to explain.
    if (s.items.some(function (i) { return i.lookaheads && i.lookaheads.length; })) {
      legend.appendChild(el('br'));
      legend.appendChild(el('span', {
        class: 'la',
        text: '{…} = the lookaheads this state allows the item to be reduced on'
      }));
    }
    if (marked) {
      legend.appendChild(el('br'));
      legend.appendChild(el('span', {
        class: 'swatch hit'
      }));
      legend.appendChild(document.createTextNode('bold with a bar = the item the clicked table entry came from'));
    }
    container.appendChild(legend);
    return marked;
  }


  // ------------------------------------------------------- the merging stage
  // Three pictures: the canonical CLR(1) automaton; the same layout while absorbed
  // states fade and edges redirect; the merged automaton laid out afresh. The
  // `statesMerged` and `mergeDone` steps supply every fact.

  /** Redirect edges through `remap` and drop the duplicates that creates. */
  function redirect(edges, remap) {
    var out = [];
    var seen = {};
    edges.forEach(function (e) {
      var from = remap(e.from);
      var to = remap(e.to);
      var key = from + ' ' + e.symbol;
      if (seen[key]) return;
      seen[key] = true;
      out.push({ from: from, to: to, symbol: e.symbol, bornAt: e.bornAt });
    });
    return out;
  }

  /** The finished LALR(1) automaton, renumbered as the engine renumbered it. */
  function mergedModel(model) {
    var groups = model.mergeDone.groups;
    var newIdOf = [];
    groups.forEach(function (group, newId) {
      group.forEach(function (old) { newIdOf[old] = newId; });
    });
    var itemsOf = {};
    model.merges.forEach(function (m) { itemsOf[m.newId] = m.items; });
    var byId = {};
    model.states.forEach(function (s) { byId[s.id] = s; });

    var states = groups.map(function (group, newId) {
      var rep = byId[group[0]] || { items: [], kernel: [], closureAt: {}, because: {} };
      return {
        id: newId,
        kernel: rep.kernel,
        items: itemsOf[newId] || rep.items,
        closureAt: rep.closureAt,
        because: rep.because,
        fromState: rep.fromState === null || rep.fromState === undefined
          ? rep.fromState : newIdOf[rep.fromState],
        onSymbol: rep.onSymbol,
        bornAt: -1,                     // the merged automaton is shown whole
        mergedFrom: group
      };
    });
    return {
      states: states,
      edges: redirect(model.edges, function (id) { return newIdOf[id]; }),
      merges: model.merges,
      mergeDone: model.mergeDone
    };
  }

  /** Where every absorbed CLR(1) state has gone once the first `upto` merges are applied. */
  function intoAfter(model, upto) {
    var into = {};
    model.merges.forEach(function (m) {
      if (m.bornAt > upto) return;
      m.mergedIds.slice(1).forEach(function (id) { into[id] = m.mergedIds[0]; });
    });
    return into;
  }

  function redirectInto(edges, into) {
    return redirect(edges, function (id) { return into[id] === undefined ? id : into[id]; });
  }

  /** The canonical automaton with the merges so far folded in; positions never move. */
  function mergingModel(model, currentIndex) {
    var applied = model.merges.filter(function (m) { return m.bornAt <= currentIndex; });
    var into = intoAfter(model, currentIndex);
    var absorbs = {};         // surviving CLR id -> the ids it will swallow
    var mergedItems = {};     // surviving CLR id -> the unioned items
    model.merges.forEach(function (m) {
      var keep = m.mergedIds[0];
      mergedItems[keep] = m.items;
      absorbs[keep] = m.mergedIds.slice(1);
    });

    var states = model.states.map(function (s) {
      if (!mergedItems[s.id]) return s;
      var done = into[absorbs[s.id][0]] !== undefined;
      return {
        id: s.id,
        kernel: s.kernel,
        items: mergedItems[s.id],           // sizing, and what it shows once merged
        preItems: s.items,                  // what it shows until then
        closureAt: s.closureAt,
        because: s.because,
        fromState: s.fromState,
        onSymbol: s.onSymbol,
        bornAt: s.bornAt,
        // `willAbsorb` is for sizing and known from the start; `absorbed` is for the label, set when the merge happens.
        willAbsorb: absorbs[s.id],
        absorbed: done ? absorbs[s.id] : null,
        mergedFrom: done ? [s.id].concat(absorbs[s.id]) : null
      };
    });

    return {
      states: states,
      edges: redirectInto(model.edges, into),
      merges: model.merges,
      mergeDone: model.mergeDone,
      into: into,
      applied: applied
    };
  }

  // ------------------------------------------------- one model per step log
  // Everything above depends on the step log (and merges applied), never the step index.

  /* Keyed on the `steps` array itself: a new analysis is a new array. Nothing cached depends on the step index or container. */
  var models = new WeakMap();

  function modelOf(steps) {
    var m = models.get(steps);
    if (m) return m;
    var canonical = collect(steps);
    var firstBorn = Infinity;
    canonical.states.forEach(function (s) { firstBorn = Math.min(firstBorn, s.bornAt); });
    m = {
      g: LRV.grammarFrom(steps),
      variant: LRV.variantFrom(steps),
      canonical: canonical,
      firstBorn: firstBorn,       // the step that creates the first state
      // The pictures, each filled in the first time a step asks for it:
      plain: null,                // no merging stage: the automaton as built
      unmerged: null,             // LALR(1), before the first merge
      merging: null,              // LALR(1), the last partly-merged picture drawn
      merged: null                // LALR(1), after `mergeDone`
    };
    models.set(steps, m);
    return m;
  }

  /** A model with its node positions and its edge routes: one picture. */
  function settle(model, geom) {
    return { model: model, geom: geom, groups: route(geom, groupEdges(model.edges)) };
  }

  /** The layout `geom` with nodes pointing at `model`'s states; used while merging, where positions are fixed and only node contents change. */
  function rehouse(geom, model) {
    var nodes = {};
    model.states.forEach(function (s) {
      var node = geom.nodes[s.id];
      if (!node) return;
      if (node.state !== s) {
        var copy = {};
        Object.keys(node).forEach(function (k) { copy[k] = node[k]; });
        copy.state = s;
        node = copy;
      }
      nodes[s.id] = node;
    });
    var out = {};
    Object.keys(geom).forEach(function (k) { out[k] = geom[k]; });
    out.nodes = nodes;
    return out;
  }

  /** What to draw at step `currentIndex`: the picture, counter and current merge, or `{ empty: why }`. */
  function pictureAt(steps, currentIndex) {
    var m = modelOf(steps);
    if (!m.g) return { empty: 'Waiting for the grammar.' };
    if (!(m.firstBorn <= currentIndex)) {
      return { empty: 'The automaton has not been built yet. Step forward to watch it grow.' };
    }
    var canonical = m.canonical;

    // For LR(0), SLR(1) and CLR(1) this is the automaton as built. For LALR(1)
    // it is whichever of the three pictures the current step calls for.
    if (!canonical.mergeDone) {
      if (!m.plain) {
        m.plain = settle(canonical, layout(m.g, canonical, canonical.edges, [canonical.edges]));
      }
      return picture(m, 'none', 'plain', m.plain, null, null);
    }

    var done = currentIndex >= canonical.mergeDone.bornAt;
    var applied = canonical.merges.filter(function (mg) { return mg.bornAt <= currentIndex; });
    var saved = applied.reduce(function (n, mg) { return n + mg.mergedIds.length - 1; }, 0);
    var counter = {
      beforeLabel: canonical.mergeDone.beforeLabel,
      afterLabel: canonical.mergeDone.afterLabel,
      before: canonical.mergeDone.before,
      after: canonical.mergeDone.after,
      live: canonical.mergeDone.before - saved,
      applied: applied.length,
      total: canonical.merges.length,
      done: done
    };

    if (done) {
      if (!m.merged) {
        var whole = mergedModel(canonical);
        m.merged = settle(whole, layout(m.g, whole, whole.edges, [whole.edges]));
      }
      return picture(m, 'merged', 'merged', m.merged, counter, null);
    }

    if (!m.unmerged) {
      // Laid out from the canonical edges; lanes reserved for every
      // intermediate picture, so nothing moves while the merges are applied.
      var reserve = [canonical.edges];
      canonical.merges.forEach(function (mg) {
        reserve.push(redirectInto(canonical.edges, intoAfter(canonical, mg.bornAt)));
      });
      var untouched = mergingModel(canonical, -Infinity);
      m.unmerged = settle(untouched,
        layout(m.g, untouched, canonical.edges, reserve, intoAfter(canonical, Infinity)));
    }
    var staged = m.unmerged;
    if (applied.length) {
      // Merges are in log order, so "the first n" is one picture; only the last is worth keeping.
      if (!m.merging || m.merging.count !== applied.length) {
        var partly = mergingModel(canonical, currentIndex);
        m.merging = settle(partly, rehouse(m.unmerged.geom, partly));
        m.merging.count = applied.length;
      }
      staged = m.merging;
    }
    // The merge that is happening right now, if any: its absorbed nodes are
    // the ones that get to slide.
    var now = canonical.merges.filter(function (mg) { return mg.bornAt === currentIndex; })[0] || null;
    return picture(m, applied.length ? 'merging' : 'canonical', 'unmerged', staged, counter, now);
  }

  /* `layoutKey` names the layout a picture sits on; a drawing is repainted in place only on the layout it was made for. */
  function picture(m, phase, layoutKey, settled, counter, current) {
    return {
      g: m.g,
      variant: m.variant,
      phase: phase,
      layoutKey: layoutKey,
      model: settled.model,
      geom: settled.geom,
      groups: settled.groups,
      counter: counter,
      current: current
    };
  }

  /** The running side-by-side count shown on every step of an LALR(1) run. */
  function counterBar(counter) {
    var bar = el('div', { class: 'merge-counter' + (counter.done ? ' done' : '') });
    bar.appendChild(el('span', { class: 'side from' }, [
      el('span', { class: 'who', text: counter.beforeLabel + ':' }),
      el('span', { class: 'n', text: String(counter.before) }),
      ' states'
    ]));
    bar.appendChild(el('span', { class: 'arrow', text: '→', 'aria-hidden': 'true' }));
    bar.appendChild(el('span', { class: 'side to' }, [
      el('span', { class: 'who', text: counter.afterLabel + ':' }),
      el('span', { class: 'n', text: String(counter.after) }),
      ' states'
    ]));
    var progress = counter.total === 0
      ? 'nothing to merge — no two states share a core'
      : counter.done
        ? counter.total + (counter.total === 1 ? ' group merged' : ' groups merged')
        : counter.applied + ' of ' + counter.total + ' merges applied — ' +
          counter.live + ' states so far';
    bar.appendChild(el('span', { class: 'progress', text: progress }));
    return bar;
  }

  // ------------------------------------------------------------------ entry
  // Table-cell hook: app.js may set LRV.diagramFocus = { state, sourceItem }; if that
  // state is drawn it is selected and `sourceItem` marked. Consumed once. After
  // mergeDone `state` is the merged number.

  /* The drawing in each container, so `update` can repaint it. */
  var drawings = new WeakMap();

  LRV.views.diagram = function (steps, currentIndex, container) {
    var focus = LRV.diagramFocus || null;
    LRV.diagramFocus = null;
    drawings.delete(container);

    var pic = pictureAt(steps, currentIndex);
    if (pic.empty) {
      container.appendChild(el('p', { class: 'empty-state', text: pic.empty }));
      return;
    }
    var drawing = scaffold(steps, pic, container);
    drawings.set(container, drawing);
    paint(drawing, pic, currentIndex, focus);
  };

  /** Repaint the drawing already in `container` at another step; declines unless it holds this log's drawing on the layout the new step needs. */
  LRV.views.diagram.update = function (steps, currentIndex, container) {
    var drawing = drawings.get(container);
    if (!drawing || drawing.steps !== steps || drawing.frame.parentNode !== container) return false;
    var pic = pictureAt(steps, currentIndex);
    if (pic.empty || pic.layoutKey !== drawing.layoutKey) return false;
    var focus = LRV.diagramFocus || null;
    LRV.diagramFocus = null;
    paint(drawing, pic, currentIndex, focus);
    return true;
  };

  /** The step-independent parts of a drawing: frame, SVG layers, toolbar, inspector, zoom/scroll/drag.
   *  Handlers read current values off the drawing, never from the step they were made at. */
  function scaffold(steps, pic, container) {
    var scroller = el('div', { id: 'diagram-scroll' });
    var host = el('div', { id: 'diagram-host' });
    var inspector = el('div', { id: 'inspector' });
    var wrap = el('div', { id: 'diagram-wrap' }, [host, inspector]);
    // One box for the whole view: the LALR(1) counter and caption go above `wrap`, which takes the remaining height.
    var frame = el('div', { id: 'diagram-frame' }, [wrap]);

    var root = svg('svg', {
      viewBox: '0 0 ' + pic.geom.width + ' ' + pic.geom.height,
      role: 'group',
      // The engine says which method this is; the diagram never works it out.
      'aria-label': 'Canonical collection of ' + (pic.variant ? pic.variant.label : 'LR(0)') + ' item sets'
    });

    var defs = svg('defs', {}, svg('marker', {
      // Sized in drawing units, so a highlighted (thicker) edge does not get
      // a bigger arrowhead than its neighbours.
      id: 'lrv-arrow', viewBox: '0 0 10 10', refX: '9', refY: '5',
      markerUnits: 'userSpaceOnUse', markerWidth: '11', markerHeight: '11',
      orient: 'auto-start-reverse'
    }, svg('path', { d: 'M 0 0 L 10 5 L 0 10 z', fill: 'currentColor' })));
    root.appendChild(defs);

    var edgeLayer = svg('g');
    var nodeLayer = svg('g');
    var overLayer = svg('g');      // merge arrows and the sliding ghosts
    root.appendChild(edgeLayer);
    root.appendChild(nodeLayer);
    root.appendChild(overLayer);
    scroller.appendChild(root);

    var d = {
      steps: steps,
      layoutKey: pic.layoutKey,
      container: container,
      frame: frame,
      wrap: wrap,
      scroller: scroller,
      inspector: inspector,
      root: root,
      edgeLayer: edgeLayer,
      nodeLayer: nodeLayer,
      overLayer: overLayer,
      edges: new Map(),            // key -> { sig, el }, see reconcile
      nodes: new Map(),
      // Left here by each paint, for the handlers below:
      geom: pic.geom,
      view: null,                  // zoom, scroll and mode; see viewCache
      followTarget: null,          // the newest thing on screen
      reduceMotion: false,
      gliding: false,              // a smooth scroll was started; see freshen
      selectedEl: null,
      pick: function () {},        // select a state by id, as drawn right now
      drag: null
    };

    // ---- toolbar ---------------------------------------------------------
    function toolButton(label, aria, onClick) {
      return el('button', {
        type: 'button', text: label, title: aria, 'aria-label': aria, on: { click: onClick }
      });
    }
    var btnFollow = toolButton('Follow', 'Follow the newest state', function () { d.setMode('follow'); });
    var btnFit = toolButton('Fit', 'Fit the whole automaton in view', function () { d.setMode('fit'); });
    var btnIn = toolButton('+', 'Zoom in', function () { zoomBy(ZOOM_STEP); });
    var btnOut = toolButton('−', 'Zoom out', function () { zoomBy(1 / ZOOM_STEP); });
    var zoomReadout = el('span', { class: 'zoom-readout', 'aria-live': 'polite' });
    // The toolbar has a row of its own so it never covers the drawing.
    host.appendChild(el('div', { id: 'diagram-tools', role: 'group', 'aria-label': 'Diagram view' },
      [btnFollow, btnFit, btnOut, zoomReadout, btnIn]));
    host.appendChild(scroller);

    // ---- zoom, scroll and follow -------------------------------------------
    d.applyScale = function () {
      var w = String(Math.round(d.geom.width * d.view.scale));
      var h = String(Math.round(d.geom.height * d.view.scale));
      // Written only when it changes: rewriting an <svg> at the same size still relays out everything inside.
      if (root.getAttribute('width') !== w) root.setAttribute('width', w);
      if (root.getAttribute('height') !== h) root.setAttribute('height', h);
      zoomReadout.textContent = Math.round(d.view.scale * 100) + '%';
      btnIn.disabled = d.view.scale >= MAX_SCALE - 1e-9;
      btnOut.disabled = d.view.scale <= MIN_SCALE + 1e-9;
    };
    d.showMode = function () {
      btnFollow.setAttribute('aria-pressed', String(d.view.mode === 'follow'));
      btnFit.setAttribute('aria-pressed', String(d.view.mode === 'fit'));
    };
    function clampScale(s) { return Math.max(MIN_SCALE, Math.min(MAX_SCALE, s)); }
    d.fitScale = function () {
      var w = scroller.clientWidth - 16;
      var h = scroller.clientHeight - 16;
      if (w <= 0 || h <= 0) return d.view.scale;
      return clampScale(Math.min(w / d.geom.width, h / d.geom.height, FIT_MAX));
    };
    /** Where the drawing's (0, 0) sits inside the scrolled content: it is
     *  centred when smaller than the viewport. */
    function origin() {
      var sr = scroller.getBoundingClientRect();
      var rr = root.getBoundingClientRect();
      return { x: rr.left - sr.left + scroller.scrollLeft, y: rr.top - sr.top + scroller.scrollTop };
    }
    d.scrollTo = function (left, top, smooth) {
      if (smooth && !d.reduceMotion && typeof scroller.scrollTo === 'function') {
        d.gliding = true;          // may still be on its way at the next step
        scroller.scrollTo({ left: left, top: top, behavior: 'smooth' });
      } else {
        scroller.scrollLeft = left;
        scroller.scrollTop = top;
      }
    };
    /** Bring a box (in drawing units) into view; centre it if it was not. */
    d.reveal = function (box, smooth, always) {
      var o = origin();
      var s = d.view.scale;
      var x0 = o.x + (box.x - 14) * s;
      var y0 = o.y + (box.y - 14) * s;
      var x1 = o.x + (box.x + box.w + 14) * s;
      var y1 = o.y + (box.y + box.h + 14) * s;
      var vw = scroller.clientWidth;
      var vh = scroller.clientHeight;
      var inside = x0 >= scroller.scrollLeft && x1 <= scroller.scrollLeft + vw &&
                   y0 >= scroller.scrollTop && y1 <= scroller.scrollTop + vh;
      if (inside && !always) return;
      // Too big to centre whole: keep its top-left corner in view instead.
      var left = (x1 - x0) > vw ? x0 : (x0 + x1) / 2 - vw / 2;
      var top = (y1 - y0) > vh ? y0 : (y0 + y1) / 2 - vh / 2;
      d.scrollTo(Math.max(0, left), Math.max(0, top), smooth);
    };
    function zoomBy(factor, ax, ay) {
      var s0 = d.view.scale;
      var s1 = clampScale(s0 * factor);
      if (s1 === s0) return;
      // Keep the point under the anchor (the viewport centre by default) still.
      var px = ax === undefined ? scroller.clientWidth / 2 : ax;
      var py = ay === undefined ? scroller.clientHeight / 2 : ay;
      var o = origin();
      var dx = (scroller.scrollLeft + px - o.x) / s0;
      var dy = (scroller.scrollTop + py - o.y) / s0;
      d.view.scale = s1;
      d.view.mode = 'manual';
      d.applyScale();
      d.showMode();
      var o2 = origin();
      d.scrollTo(o2.x + dx * s1 - px, o2.y + dy * s1 - py, false);
    }
    d.setMode = function (mode) {
      d.view.mode = mode;
      if (mode === 'fit') {
        d.view.scale = d.fitScale();
        d.applyScale();
        d.scrollTo(0, 0, false);
      } else if (mode === 'follow') {
        if (d.view.scale < 1) { d.view.scale = 1; d.applyScale(); }
        if (d.followTarget) d.reveal(d.followTarget, true, false);
      }
      d.showMode();
    };
    function leaveFollow() {
      if (d.view.mode === 'follow' || d.view.mode === 'fit') { d.view.mode = 'manual'; d.showMode(); }
    }

    scroller.addEventListener('scroll', function () {
      // A scroll event arrives a frame late; if the drawing has left the page the scroller reads 0 and must not overwrite the remembered view.
      if (!d.view || d.frame.parentNode !== d.container) return;
      d.view.sl = scroller.scrollLeft;
      d.view.st = scroller.scrollTop;
    });

    // ---- pointer: ctrl+wheel zooms, plain wheel scrolls, drag pans --------
    scroller.addEventListener('wheel', function (ev) {
      if (ev.ctrlKey || ev.metaKey) {
        ev.preventDefault();
        var box = scroller.getBoundingClientRect();
        zoomBy(ev.deltaY > 0 ? 1 / 1.12 : 1.12, ev.clientX - box.left, ev.clientY - box.top);
        return;
      }
      if (scroller.scrollWidth > scroller.clientWidth || scroller.scrollHeight > scroller.clientHeight) {
        leaveFollow();
      }
    }, { passive: false });

    // A drag only starts once the pointer has really moved, so a plain click
    // still lands on the node under it.
    scroller.addEventListener('pointerdown', function (ev) {
      if (ev.button !== 0) return;
      d.drag = { x: ev.clientX, y: ev.clientY, sl: scroller.scrollLeft, st: scroller.scrollTop, on: false, id: ev.pointerId };
    });
    scroller.addEventListener('pointermove', function (ev) {
      var drag = d.drag;
      if (!drag) return;
      var dx = ev.clientX - drag.x;
      var dy = ev.clientY - drag.y;
      if (!drag.on) {
        if (Math.abs(dx) + Math.abs(dy) < 5) return;
        drag.on = true;
        try { scroller.setPointerCapture(drag.id); } catch (err) { /* already released */ }
        scroller.classList.add('dragging');
        leaveFollow();
      }
      scroller.scrollLeft = drag.sl - dx;
      scroller.scrollTop = drag.st - dy;
    });
    function endDrag() {
      d.drag = null;
      scroller.classList.remove('dragging');
    }
    scroller.addEventListener('pointerup', endDrag);
    scroller.addEventListener('pointercancel', endDrag);

    // ---- nodes: built here so their handlers reach the current paint -------
    d.buildShape = function (spec) {
      var shape = buildNode(spec);
      shape.addEventListener('click', function () { d.pick(spec.id); });
      shape.addEventListener('keydown', function (ev) {
        if (ev.key === 'Enter' || ev.key === ' ' || ev.key === 'Spacebar') {
          ev.preventDefault();
          // The player steps on Space and Enter too; selecting a state must
          // not also move the analysis on underneath it.
          ev.stopPropagation();
          d.pick(spec.id);
        }
      });
      return shape;
    };

    return d;
  }

  /** Put a drawing about to be repainted back in the state of a fresh one: clear selection, drag, focus and any smooth scroll. */
  function freshen(d) {
    if (d.selectedEl) {
      d.selectedEl.classList.remove('selected');
      var ring = d.selectedEl.querySelector('.sel-ring');
      if (ring) d.selectedEl.removeChild(ring);
      d.selectedEl = null;
    }
    if (d.drag && d.drag.on) {
      try { d.scroller.releasePointerCapture(d.drag.id); } catch (err) { /* already released */ }
    }
    d.drag = null;
    d.scroller.removeAttribute('class');
    var focused = document.activeElement;
    if (focused && d.frame.contains(focused) && typeof focused.blur === 'function') focused.blur();
    if (d.gliding) {
      // A new scroller is at rest; this one may still be gliding. Only a write that moves it stops the glide
      // (Firefox glides through a same-value write), so move it a pixel and put it back.
      var x = d.scroller.scrollLeft;
      var y = d.scroller.scrollTop;
      d.scroller.scrollLeft = x > 0 ? x - 1 : 1;
      d.scroller.scrollTop = y > 0 ? y - 1 : 1;
      d.scroller.scrollLeft = x;
      d.scroller.scrollTop = y;
      d.gliding = false;
    }
  }

  /** Draw step `currentIndex` into a drawing: new from `scaffold`, or another step on the same layout. */
  function paint(d, pic, currentIndex, focus) {
    var steps = d.steps;
    var container = d.container;
    var scroller = d.scroller;
    var inspector = d.inspector;
    var g = pic.g;
    var model = pic.model;
    var geom = pic.geom;
    var current = steps[currentIndex] || {};

    freshen(d);

    // ---- edges: only what has been added by this step -------------------
    var curEdge = current.kind === 'transitionAdded' ? current : null;
    var edges = [];
    pic.groups.forEach(function (e) {
      if (e.kind === 'none') return;
      var shown = e.symbols.filter(function (sym) { return sym.bornAt <= currentIndex; })
        .map(function (sym) { return String(sym.symbol); });
      if (!shown.length) return;
      var isCurrent = !!(curEdge && curEdge.from === e.from && curEdge.to === e.to);
      edges.push({ key: 'edge ' + e.key, spec: edgeSpec(e, shown, isCurrent) });
    });

    // ---- nodes: only what has been built by this step --------------------
    var nodes = [];
    var followTarget = null;         // the newest thing on screen
    var defaultSel = null;
    model.states.forEach(function (s) {
      var node = geom.nodes[s.id];
      if (!node) return;
      if (!(s.bornAt <= currentIndex || s.bornAt === -1)) return;
      var absorbed = !!(model.into && model.into[s.id] !== undefined);
      var nowAbsorbed = absorbed && pic.current && pic.current.mergedIds.indexOf(s.id) > 0;
      var isCurrent = !absorbed &&
        (current.kind === 'stateCreated' || current.kind === 'stateClosed' ||
         current.kind === 'closureItemAdded') &&
        (current.id === s.id || current.state === s.id);
      // A node shows its merged contents only once its group has merged.
      var justMerged = !!(pic.current && pic.current.mergedIds[0] === s.id);
      var lines = absorbed ? node.drawnPre : (s.absorbed ? node.drawn : node.drawnPre);
      var cls = absorbed ? 'absorbed' + (nowAbsorbed ? ' now' : '')
                         : ((isCurrent || justMerged) ? 'current' : '');
      nodes.push({ key: 'node ' + s.id, id: s.id, node: node, spec: nodeSpec(node, currentIndex, cls, lines, true) });
      if (isCurrent || justMerged) { defaultSel = s.id; followTarget = node; }
    });

    if (curEdge && geom.nodes[curEdge.from] && geom.nodes[curEdge.to]) {
      followTarget = unionBox(geom.nodes[curEdge.from], geom.nodes[curEdge.to]);
      // The step reads "Reading X in Ik ...": show Ik and its transitions.
      if (defaultSel === null) defaultSel = curEdge.from;
    }
    if (current.kind === 'mergeDone' && geom.nodes[0]) followTarget = geom.nodes[0];
    // While a group folds, follow the whole group: the arrow is the point.
    if (pic.current) {
      followTarget = null;
      pic.current.mergedIds.forEach(function (id) {
        var n = geom.nodes[id];
        if (n) followTarget = followTarget ? unionBox(followTarget, n) : n;
      });
    }

    // ---- the merging stage: absorbed states, their arrows, the ghost ------
    var over = [];
    var dissolving = [];
    if (model.into) {
      Object.keys(model.into).forEach(function (k) {
        var from = geom.nodes[k];
        var to = geom.nodes[model.into[k]];
        var fold = geom.folds ? geom.folds[k] : null;
        if (!from || !to || !fold) return;
        var now = !!(pic.current && pic.current.mergedIds.indexOf(Number(k)) > 0);
        // The merge happening now is drawn over everything, with its label;
        // earlier ones stay as a thinner record of where each state went.
        if (!now) {
          edges.push({ key: 'merge ' + k, spec: mergeArrowSpec(fold, false) });
          return;
        }
        over.push(buildEdge(mergeArrowSpec(fold, true)));
        // A copy fades out in place over the original, which stays put,
        // faded and dashed, so the arrow always has a box at both ends.
        var ghost = buildNode(nodeSpec(from, currentIndex, 'absorbing', from.drawnPre, false));
        over.push(ghost);
        dissolving.push({ node: ghost });
      });
    }

    reconcile(d.edgeLayer, d.edges, edges, buildEdge);
    reconcile(d.nodeLayer, d.nodes, nodes, d.buildShape);
    LRV.clear(d.overLayer);
    over.forEach(function (node) { d.overLayer.appendChild(node); });

    var drawnNodes = {};             // state id -> its node and <g>, for selection
    nodes.forEach(function (n) {
      drawnNodes[n.id] = { node: n.node, shape: d.nodes.get(n.key).el };
    });

    // ---- the counter and its caption, above the drawing -------------------
    while (d.frame.firstChild && d.frame.firstChild !== d.wrap) {
      d.frame.removeChild(d.frame.firstChild);
    }
    if (d.frame.parentNode !== container) container.appendChild(d.frame);
    if (pic.counter) {
      d.frame.insertBefore(counterBar(pic.counter), d.wrap);
      d.frame.insertBefore(el('p', {
        class: 'note merge-caption',
        text: pic.phase === 'canonical'
          ? 'This is the canonical LR(1) automaton, built in full. LALR(1) has not merged anything yet — step on to watch same-core states fold together.'
          : pic.phase === 'merging'
            ? 'Merging in progress. Each absorbed state stays where it was, faded and dashed, with an arrow to the state that swallows it, and every edge that pointed at it now points there instead. The surviving node shows both numbers and the union of the lookahead sets.'
            : 'The merged automaton, renumbered. A node labelled I3 ← I3+I6 was two CLR(1) states a moment ago; its lookahead sets are the union of theirs, and that union is what the table is built from.'
      }), d.wrap);
    }

    // ---- selection: the table hook, else the current state, else I0 -------
    function select(node, shape, hit) {
      if (d.selectedEl) {
        d.selectedEl.classList.remove('selected');
        var ring = d.selectedEl.querySelector('.sel-ring');
        if (ring) d.selectedEl.removeChild(ring);
      }
      d.selectedEl = shape;
      if (shape) {
        shape.classList.add('selected');
        shape.appendChild(svg('rect', {
          class: 'sel-ring', x: -5, y: -5, width: node.w + 10, height: node.h + 10, rx: 8
        }));
      }
      return describeState(g, model, node, inspector, currentIndex, hit || null);
    }
    d.pick = function (id) {
      if (drawnNodes[id]) select(drawnNodes[id].node, drawnNodes[id].shape);
    };

    LRV.clear(inspector);
    var focusHit = null;
    var focusNode = null;
    if (focus && drawnNodes[focus.state]) {
      var f = drawnNodes[focus.state];
      focusHit = select(f.node, f.shape, focus.sourceItem);
      focusNode = f.node;
    } else if (defaultSel !== null && drawnNodes[defaultSel]) {
      select(drawnNodes[defaultSel].node, drawnNodes[defaultSel].shape);
    } else if (drawnNodes[0]) {
      select(drawnNodes[0].node, drawnNodes[0].shape);
    }

    // ---- zoom, scroll and follow, remembered across re-renders -------------
    var view = viewCache.get(container);
    if (!view || view.steps !== steps) {
      view = { steps: steps, mode: 'follow', scale: 1, sl: 0, st: 0, size: '' };
    }
    var size = geom.width + 'x' + geom.height;
    var relaid = view.size !== size;
    view.size = size;
    viewCache.set(container, view);

    d.geom = geom;
    d.view = view;
    d.followTarget = followTarget;
    live = d;
    d.reduceMotion = typeof window.matchMedia === 'function' &&
      window.matchMedia('(prefers-reduced-motion: reduce)').matches;

    if (view.mode === 'fit') view.scale = d.fitScale();
    d.applyScale();
    d.showMode();
    // A new drawing's inspector starts at the top; so does a repainted one.
    // Placed after applyScale: a scroll write forces a layout, and earlier that would be one too many.
    inspector.scrollTop = 0;
    if (relaid && view.mode !== 'manual') {
      d.scrollTo(0, 0, false);
    } else {
      d.scrollTo(view.sl, view.st, false);
    }
    if (focusNode) {
      d.reveal(focusNode, false, false);
    } else if (view.mode === 'follow' && followTarget) {
      d.reveal(followTarget, true, false);
    }
    if (focusHit) {
      // The inspector scrolls on its own; keep the marked item in sight.
      var ir = inspector.getBoundingClientRect();
      var hr = focusHit.getBoundingClientRect();
      if (hr.top < ir.top || hr.bottom > ir.bottom) {
        inspector.scrollTop += hr.top - ir.top - ir.height / 3;
      }
      if (d.selectedEl && typeof d.selectedEl.focus === 'function') {
        try { d.selectedEl.focus({ preventScroll: true }); } catch (err) { /* old browser */ }
      }
    }
    view.sl = scroller.scrollLeft;
    view.st = scroller.scrollTop;

    // Ghosts fade in place, never sliding across other boxes; the layout read makes the transition start from full opacity. Reduced motion makes it a jump.
    if (dissolving.length && typeof window.requestAnimationFrame === 'function') {
      dissolving.forEach(function (item) { item.node.getBoundingClientRect(); });
      window.requestAnimationFrame(function () {
        dissolving.forEach(function (item) {
          item.node.setAttribute('class', 'node absorbing gone');
        });
      });
    }
  }

  // ---------------------------------------------------------- window resize
  // Re-fit (Fit) or re-follow on window resize, at most once per animation frame; the layout is untouched.
  var live = null;            // the drawing `paint` drew last
  var resizeFrame = 0;

  function refit() {
    resizeFrame = 0;
    var d = live;
    if (!d || !d.view || d.frame.parentNode !== d.container || !document.contains(d.frame)) return;
    if (d.view.mode === 'fit') {
      d.view.scale = d.fitScale();
      d.applyScale();
      d.scrollTo(0, 0, false);
    } else if (d.view.mode === 'follow' && d.followTarget) {
      d.reveal(d.followTarget, false, false);
    }
  }

  window.addEventListener('resize', function () {
    if (resizeFrame) return;
    if (typeof window.requestAnimationFrame !== 'function') { refit(); return; }
    resizeFrame = window.requestAnimationFrame(refit);
  });

  /** The smallest box around two nodes, for following an edge. */
  function unionBox(a, b) {
    var x = Math.min(a.x, b.x);
    var y = Math.min(a.y, b.y);
    return {
      x: x, y: y,
      w: Math.max(a.x + a.w, b.x + b.w) - x,
      h: Math.max(a.y + a.h, b.y + b.h) - y
    };
  }

})(window.LRV);

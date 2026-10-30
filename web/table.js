/* ACTION / GOTO table view.
   *
   * Cells fill in as the player advances. Conflicts keep every competing action
   * and are marked with a dashed border, a glyph and an `S/R` / `R/R` tag from
   * the engine's `conflictKind`, never colour alone (it gets printed).
   *
   * A filled cell is focusable; click, Enter or Space calls
   * `LRV.openStateFromTable` with the row's state, column and `sourceItem`.
   *
   * A cell is a function of the step log and the index only: `cellAt` reads its
   * entries, `buildCell` draws it. `update` rebuilds just the cells that can
   * have changed; the grid itself is not rebuilt per step (too many cells). */
window.LRV = window.LRV || {};
window.LRV.views = window.LRV.views || {};

(function (LRV) {
  'use strict';
  var el = LRV.el;

  var WARN = '⚠';

  /* Symbols are user text and may be named like Object.prototype members
     (__proto__, constructor), so every symbol-keyed map has no prototype. */
  function dict() { return Object.create(null); }

  /* Index-independent facts from the step log (grammar, columns, row count,
     which steps wrote which cell), cached per `steps` array: a new analysis
     is a new array, and nothing writes to one. */
  var models = new WeakMap();

  function modelOf(steps) {
    var model = models.get(steps);
    if (model) return model;

    var g = LRV.grammarFrom(steps);
    // Column lists are not in the step log, so they are read back from the
    // grammarParsed step (terminals then `$`; GOTO: non-terminals but the start).
    var actionCols = g ? g.terminals.concat(['$']) : [];
    var gotoCols = g ? g.nonterminals.filter(function (n) { return n !== g.start; }) : [];
    var allCols = actionCols.concat(gotoCols);

    // Where each symbol's column sits in a row (a list: may head several columns).
    var columnsOf = dict();
    allCols.forEach(function (col, ci) {
      if (!columnsOf[col]) columnsOf[col] = [];
      columnsOf[col].push(ci);
    });

    // `writes`: every cell-writing step, in log order; `byCell`: those indices per cell.
    var writes = [];
    var byCell = dict();
    // Cells that conflict only because of a merge, for the footnote.
    var mergeCells = [];
    var seenMerge = dict();
    steps.forEach(function (s, i) {
      if (s.kind !== 'tableCellSet' && s.kind !== 'tableConflict') return;
      var k = key(s.state, s.symbol);
      writes.push(i);
      if (!byCell[k]) byCell[k] = [];
      byCell[k].push(i);
      if (s.kind === 'tableConflict' && s.mergeInduced && !seenMerge[k]) {
        seenMerge[k] = true;
        mergeCells.push({ key: k, at: i });
      }
    });

    model = {
      g: g,
      actionCols: actionCols,
      gotoCols: gotoCols,
      allCols: allCols,
      columnsOf: columnsOf,
      total: stateCount(steps),
      writes: writes,
      byCell: byCell,
      mergeCells: mergeCells
    };
    models.set(steps, model);
    return model;
  }

  /** One cell as of step `index`: its entries, the item the engine says
   *  produced it, and how it conflicts. A replay of just the steps that wrote
   *  to this cell, so it costs the same wherever in the log `index` is. */
  function cellAt(model, steps, state, symbol, index) {
    var actions = [];
    var source = null;
    var fromMerge = false;
    var kinds = [];
    var wrote = model.byCell[key(state, symbol)] || [];
    for (var n = 0; n < wrote.length && wrote[n] <= index; n++) {
      var s = steps[wrote[n]];
      if (s.kind === 'tableCellSet') {
        put(actions, s.action);
        if (s.sourceItem) source = s.sourceItem;
      } else {
        put(actions, s.incoming);
        // Merge-induced and conflict kind are read off the step, not derived.
        if (s.mergeInduced) fromMerge = true;
        if (s.conflictKind && kinds.indexOf(s.conflictKind) < 0) kinds.push(s.conflictKind);
      }
    }
    return { actions: actions, source: source, fromMerge: fromMerge, kinds: kinds };
  }

  /** The `state symbol` key of the cell step `index` wrote to, or null. */
  function cellOfStep(steps, index) {
    var s = steps[index];
    return (s && (s.kind === 'tableCellSet' || s.kind === 'tableConflict'))
      ? key(s.state, s.symbol) : null;
  }

  function key(state, symbol) { return state + ' ' + symbol; }

  /* Short tag for a conflicted cell, keyed by the engine's `conflictKind`;
     an unknown kind is shown as the engine spelled it. */
  var KIND_TAG = { 'shift-reduce': 'S/R', 'reduce-reduce': 'R/R' };
  function kindTag(kind) {
    return Object.prototype.hasOwnProperty.call(KIND_TAG, kind) ? KIND_TAG[kind] : kind;
  }

  /** Add an entry to a cell's list, unless the cell already shows it. */
  function put(list, action) {
    var text = LRV.actionText(action);
    for (var i = 0; i < list.length; i++) {
      if (LRV.actionText(list[i]) === text) return;
    }
    list.push(action);
  }

  /** How many states the table has rows for. The last automaton-phase step
   *  says: `automatonDone`, or `mergeDone` for LALR(1). */
  function stateCount(steps) {
    var count = 0;
    steps.forEach(function (s) {
      if (s.kind === 'automatonDone') count = s.stateCount;
      else if (s.kind === 'mergeDone') count = s.after;
    });
    if (count) return count;
    var max = 0;
    steps.forEach(function (s) {
      if (s.kind === 'stateCreated') max = Math.max(max, s.id + 1);
    });
    return max;
  }

  function describeCell(g, state, symbol, actions, item, fromMerge, kinds) {
    // Named by the entry's own type tag: a GOTO cell holds only `goto` entries.
    var table = actions.length && actions[0].type === 'goto' ? 'GOTO' : 'ACTION';
    var parts = [table + '[' + state + ', ' + symbol + '] = ' +
      actions.map(LRV.actionText).join(' / ')];
    actions.forEach(function (a) {
      if (a.type === 'shift') {
        parts.push('shift: read ' + symbol + ' and go to state I' + a.state + '.');
      } else if (a.type === 'reduce') {
        parts.push('reduce ' + a.production + ': ' + LRV.productionText(g, a.production) + '.');
      } else if (a.type === 'accept') {
        parts.push('accept: the input is fully recognised.');
      } else if (a.type === 'goto') {
        parts.push('goto: after pushing ' + symbol + ', the parser is in state I' + a.state + '.');
      }
    });
    if (item) parts.push('From the item ' + LRV.itemText(g, item) + '.');
    if (actions.length > 1) {
      parts.push('This cell is in conflict' +
        (kinds && kinds.length ? ' (' + kinds.join(', ') + ')' : '') +
        ' - it holds more than one action.');
    }
    if (fromMerge) {
      parts.push('It conflicts only because two states were merged; CLR(1) has no conflict here.');
    }
    return parts.join(' ');
  }

  /* The grid drawn into each container, so `update` can find it again. */
  var drawings = new WeakMap();

  LRV.views.table = function (steps, index, container) {
    drawings.delete(container);
    var model = modelOf(steps);
    var g = model.g;
    if (!g) {
      container.appendChild(el('p', { class: 'empty-state', text: 'Waiting for the grammar.' }));
      return;
    }
    var actionCols = model.actionCols;
    var gotoCols = model.gotoCols;
    var currentCell = cellOfStep(steps, index);

    container.appendChild(el('h2', { class: 'view-title', text: 'ACTION / GOTO table' }));
    container.appendChild(el('p', {
      class: 'note',
      text: 'Rows are states. ACTION is consulted on the next input symbol; GOTO is consulted after a reduction. sN = shift and go to state N, rN = reduce by production N, acc = accept. A cell with two entries is a conflict.'
    }));

    var table = el('table', { class: 'grid' });

    var head = el('thead');
    head.appendChild(el('tr', {}, [
      el('th', { rowspan: '2', text: 'State' }),
      el('th', { class: 'group-title group', colspan: String(actionCols.length), text: 'ACTION' }),
      gotoCols.length
        ? el('th', { class: 'group-title group', colspan: String(gotoCols.length), text: 'GOTO' })
        : null
    ]));
    var colRow = el('tr');
    actionCols.forEach(function (c, i) {
      colRow.appendChild(el('th', { class: i === 0 ? 'group' : '', scope: 'col', text: c }));
    });
    gotoCols.forEach(function (c, i) {
      colRow.appendChild(el('th', { class: i === 0 ? 'group' : '', scope: 'col', text: c }));
    });
    head.appendChild(colRow);
    table.appendChild(head);

    var body = el('tbody');
    for (var s = 0; s < model.total; s++) {
      body.appendChild(buildRow(model, steps, s, index, currentCell));
    }
    table.appendChild(body);
    var scroller = el('div', { class: 'scroller' }, table);
    container.appendChild(scroller);

    appendMergeFootnote(model, index, container);
    appendVerdict(steps, index, container);

    drawings.set(container, { steps: steps, index: index, scroller: scroller, body: body });
    revealCurrentCell(container, scroller);
  };

  /* Bring the current cell into view by scrolling only `container` (vertical)
     and `scroller` (horizontal); no scrollIntoView, no focus. */
  var REVEAL_MARGIN = 8;
  function revealCurrentCell(container, scroller) {
    var cell = scroller.querySelector('td.current');
    // A container that is not laid out (hidden, detached) has nothing to measure.
    if (!cell || !container.clientHeight || !scroller.clientWidth) return;
    var c = cell.getBoundingClientRect();
    var s = scroller.getBoundingClientRect();
    var right = s.left + scroller.clientWidth;
    if (c.right > right) {
      // Off to the right: show it, and only then give up the row header.
      scroller.scrollLeft += Math.min(c.right - right + REVEAL_MARGIN, c.left - s.left);
    }
    c = cell.getBoundingClientRect();
    var v = container.getBoundingClientRect();
    var top = v.top + container.clientTop + REVEAL_MARGIN;
    var bottom = v.top + container.clientTop + container.clientHeight - REVEAL_MARGIN;
    if (c.top < top) container.scrollTop += c.top - top;
    else if (c.bottom > bottom) container.scrollTop += Math.min(c.bottom - bottom, c.top - top);
  }

  /** Move the grid in `container` from the index it shows to `index`. A cell
   *  can differ only if a step in between wrote it or it is current at either
   *  end, so only those are rebuilt (same `buildCell`); footnote and verdict
   *  are redrawn. */
  LRV.views.table.update = function (steps, index, container) {
    var drawn = drawings.get(container);
    if (!drawn || drawn.steps !== steps || drawn.scroller.parentNode !== container) return false;
    var model = modelOf(steps);
    var currentCell = cellOfStep(steps, index);

    // The cells to rebuild, as one representative step each.
    var stale = dict();
    [drawn.index, index].forEach(function (i) {
      var k = cellOfStep(steps, i);
      if (k !== null) stale[k] = steps[i];
    });
    var lo = Math.min(drawn.index, index);
    var hi = Math.max(drawn.index, index);
    // `writes` is ascending, so the steps in (lo, hi] are one run of it.
    var w = model.writes;
    var from = 0;
    var to = w.length;
    while (from < to) {
      var mid = (from + to) >> 1;
      if (w[mid] <= lo) from = mid + 1; else to = mid;
    }
    for (var n = from; n < w.length && w[n] <= hi; n++) {
      var step = steps[w[n]];
      stale[key(step.state, step.symbol)] = step;
    }

    Object.keys(stale).forEach(function (k) {
      var row = drawn.body.rows[stale[k].state];
      // A step may name a state or a symbol the grid has no row or column
      // for; a full draw shows nothing for it, so there is nothing to swap.
      if (!row) return;
      (model.columnsOf[stale[k].symbol] || []).forEach(function (ci) {
        var old = row.cells[ci + 1];    // + 1: the row's own `I3` header cell
        row.replaceChild(buildCell(model, steps, stale[k].state, ci, index, currentCell), old);
      });
    });

    while (drawn.scroller.nextSibling) container.removeChild(drawn.scroller.nextSibling);
    appendMergeFootnote(model, index, container);
    appendVerdict(steps, index, container);

    // A full draw leaves no cell focused and scrolled to the left; match that.
    // Scroll last: it forces layout.
    var focused = document.activeElement;
    if (focused && focused !== container && container.contains(focused) &&
        typeof focused.blur === 'function') {
      focused.blur();
    }
    drawn.scroller.scrollLeft = 0;

    drawn.index = index;
    revealCurrentCell(container, drawn.scroller);
    return true;
  };

  /* Named in text, not just marked: which cells conflict only because LALR(1)
     merged two states. */
  function appendMergeFootnote(model, index, container) {
    var cells = model.mergeCells
      .filter(function (c) { return c.at <= index; })
      .map(function (c) { return c.key; });
    if (!cells.length) return;
    // As runs: a column is a symbol.
    var named = [];
    cells.forEach(function (k, n) {
      var parts = k.split(' ');
      if (n) named.push(' and ');
      named.push('ACTION[' + parts[0] + ', ', LRV.sym(parts[1]), ']');
    });
    container.appendChild(el('p', { class: 'note merge-footnote' }, [
      el('span', { class: 'warn-mark', text: '⚠ ', 'aria-hidden': 'true' })
    ].concat(LRV.rich(cells.length === 1
      ? ['One cell conflicts only because merging brought two states together: ', named,
         '. CLR(1) keeps those states apart and has no conflict there. This is what LALR(1) trades for its smaller table.']
      : [cells.length + ' cells conflict only because merging brought states together: ', named,
         '. CLR(1) keeps those states apart and has no conflict in any of them. This is what LALR(1) trades for its smaller table.']
    ))));
  }

  function buildRow(model, steps, s, index, currentCell) {
    var row = el('tr');
    row.appendChild(el('th', { class: 'rowhead', scope: 'row', text: 'I' + s }));
    model.allCols.forEach(function (col, ci) {
      row.appendChild(buildCell(model, steps, s, ci, index, currentCell));
    });
    return row;
  }

  /** The cell in row `s`, column `ci`, as of step `index`; the one place a
   *  cell is drawn (full draw and `update` both use it). */
  function buildCell(model, steps, s, ci, index, currentCell) {
    var g = model.g;
    var col = model.allCols[ci];
    var at = cellAt(model, steps, s, col, index);
    var actions = at.actions;
    var conflicted = actions.length > 1;
    var fromMerge = at.fromMerge;
    var cls = [];
    if (ci === 0 || ci === model.actionCols.length) cls.push('group');
    if (!actions.length) cls.push('empty');
    if (conflicted) cls.push('conflict');
    if (fromMerge) cls.push('merge-conflict');
    if (currentCell === key(s, col)) cls.push('current');

    var item = at.source;
    var kinds = conflicted ? at.kinds : [];
    var target = actions.length ? { state: s, sourceItem: item, column: col } : null;
    var canOpen = !!target && typeof LRV.openStateFromTable === 'function';

    var title = null;
    if (actions.length) {
      title = describeCell(g, s, col, actions, item, fromMerge, kinds);
      if (canOpen) title += ' Click or press Enter to open I' + s + ' in Item Sets.';
    }

    var cell = el('td', {
      class: cls.join(' ') + (canOpen ? ' opens-state' : ''),
      tabindex: actions.length ? '0' : null,
      title: title,
      on: target ? {
        click: function () { openState(target); },
        keydown: function (ev) {
          if (ev.key !== 'Enter' && ev.key !== ' ' && ev.key !== 'Spacebar') return;
          // Keys meant for this cell must not also step the player.
          ev.preventDefault();
          ev.stopPropagation();
          openState(target);
        }
      } : null
    });
    actions.forEach(function (a, i) {
      if (i > 0) {
        cell.appendChild(document.createTextNode(' '));
        cell.appendChild(el('span', { class: 'warn-mark', text: WARN }));
        cell.appendChild(document.createTextNode(' '));
      }
      cell.appendChild(document.createTextNode(LRV.actionText(a)));
    });
    if (conflicted) kinds.forEach(function (kind) { appendKindTag(cell, kind); });
    return cell;
  }

  /* `S/R` or `R/R` as text, so the kind survives black-and-white print.
     Hidden from screen readers in favour of the words it abbreviates. */
  function appendKindTag(cell, kind) {
    cell.appendChild(document.createTextNode(' '));
    cell.appendChild(el('span', { class: 'conflict-kind', title: kind + ' conflict' }, [
      el('span', { 'aria-hidden': 'true', text: kindTag(kind) }),
      el('span', { class: 'sr-only', text: kind + ' conflict' })
    ]));
  }

  /** Hand a filled cell to the Item Sets view, if the app offers that.
   *  `sourceItem` is the item the engine says produced the entry (for a
   *  conflicted cell, the first entry's), or null when it names none. */
  function openState(target) {
    if (typeof LRV.openStateFromTable !== 'function') return;
    LRV.openStateFromTable({
      state: target.state,
      sourceItem: target.sourceItem,
      column: target.column
    });
  }

  /** The verdict banner, once the table phase has finished. The method name
   *  comes from the step's `class` field; ends with the suggestion line when
   *  the table has conflicts. */
  function appendVerdict(steps, index, container) {
    var done = null;
    for (var i = 0; i <= index && i < steps.length; i++) {
      if (steps[i].kind === 'tableDone') done = steps[i];
    }
    if (!done) return;

    var conflicts = steps.slice(0, index + 1).filter(function (x) {
      return x.kind === 'tableConflict';
    });
    var sr = conflicts.filter(function (x) { return x.conflictKind === 'shift-reduce'; }).length;
    var rr = conflicts.length - sr;

    var headline = done.isValid
      ? 'This grammar is ' + done.class
      : 'This grammar is ' + done.class.replace('not ', 'NOT ') + ' - ' +
        done.conflicts + ' conflict' + (done.conflicts === 1 ? '' : 's') +
        ' (' + sr + ' shift-reduce, ' + rr + ' reduce-reduce)';

    var box = el('div', { class: 'verdict ' + (done.isValid ? 'good' : 'bad') }, [
      el('strong', { class: 'headline', text: headline }),
      document.createTextNode(done.explain)
    ]);
    if (!done.isValid) appendSuggestion(steps, index, box);
    container.appendChild(box);
  }

  /** The "try that" line: shown only when this table has conflicts and a
   *  stronger method is clean. Scans the engine's rows (weakest first) for the
   *  first valid one after the current, so it names only methods that ran. */
  function appendSuggestion(steps, index, container) {
    var cmp = null;
    for (var i = 0; i <= index && i < steps.length; i++) {
      if (steps[i].kind === 'variantComparison') cmp = steps[i];
    }
    if (!cmp || !cmp.results) return;
    var at = -1;
    for (var j = 0; j < cmp.results.length; j++) {
      if (cmp.results[j].label === cmp.current) { at = j; break; }
    }
    if (at < 0) return;
    var pick = null;
    for (var k = at + 1; k < cmp.results.length; k++) {
      if (cmp.results[k].isValid) { pick = cmp.results[k]; break; }
    }
    if (!pick) return;
    var name = typeof LRV.selectVariant === 'function'
      ? el('button', {
          type: 'button',
          class: 'link-like',
          title: 'Walk through the ' + pick.label + ' analysis of this grammar',
          on: { click: function () { LRV.selectVariant(pick.variant); } },
          text: pick.label
        })
      : document.createTextNode(pick.label);
    container.appendChild(el('p', { class: 'suggestion' }, [
      document.createTextNode('This grammar is '), name,
      document.createTextNode(', though - try that.')
    ]));
  }

})(window.LRV);

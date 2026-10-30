/* "Compare all four": one row per method, read from the finished analysis, not
 * replayed from steps. Every number is measured by building that method. */
window.LRV = window.LRV || {};
window.LRV.views = window.LRV.views || {};

(function (LRV) {
  'use strict';
  var el = LRV.el;

  var COLUMNS = [
    { key: 'method',   label: 'Method',         hint: 'Weakest first. Each row accepts every grammar the row above it accepts, and more.' },
    { key: 'states',   label: 'States',         hint: 'The size of the canonical collection this method builds.' },
    { key: 'conflicts', label: 'Conflicts',     hint: 'Cells holding more than one action.' },
    { key: 'kinds',    label: 'Conflict types', hint: 'Shift-reduce, reduce-reduce, and - for LALR(1) - how many exist only because merging brought two states together.' },
    { key: 'verdict',  label: 'Verdict',        hint: 'What this method would say about this grammar.' }
  ];

  /** "2 shift-reduce", "1 reduce-reduce (1 caused by merging)", "—". */
  function kindsText(row) {
    var parts = [];
    if (row.shiftReduce) parts.push(row.shiftReduce + ' shift-reduce');
    if (row.reduceReduce) parts.push(row.reduceReduce + ' reduce-reduce');
    if (!parts.length) return '—';
    var text = parts.join(', ');
    if (row.mergeInduced) {
      text += row.mergeInduced === row.conflicts
        ? ' — all of them caused by merging'
        : ' — ' + row.mergeInduced + ' of them caused by merging';
    }
    return text;
  }

  LRV.views.compare = function (steps, index, container, result) {
    var rows = (result && result.verdict && result.verdict.comparison) || [];
    if (!rows.length) {
      container.appendChild(el('p', {
        class: 'empty-state',
        text: 'Analyze a grammar and this table fills in: one row per method, for the grammar you typed.'
      }));
      return;
    }

    container.appendChild(el('h2', { class: 'view-title', text: 'The same grammar, under all four methods' }));
    container.appendChild(el('p', {
      class: 'note',
      text: 'Every row below was produced by actually running that method on this grammar — building its item sets, then its table, then counting what collided. Reading down the table is the whole topic in one screen: the conflicts fall away as the methods get stronger, and the state count is what that strength costs.'
    }));

    var table = el('table', { class: 'grid compare' });
    var head = el('thead');
    var headRow = el('tr');
    COLUMNS.forEach(function (c) {
      headRow.appendChild(el('th', { scope: 'col', title: c.hint, text: c.label }));
    });
    head.appendChild(headRow);
    table.appendChild(head);

    var body = el('tbody');
    rows.forEach(function (r) {
      var showing = isCurrent(steps, r.variant);
      var tr = el('tr', { class: (r.isValid ? 'clean' : 'dirty') + (showing ? ' showing' : '') });

      var name = el('th', { class: 'rowhead', scope: 'row' }, [
        el('span', { class: 'mark', text: r.isValid ? '✓' : '⚠', 'aria-hidden': 'true' }),
        el('span', { class: 'sr-only', text: r.isValid ? 'no conflicts: ' : 'has conflicts: ' }),
        r.label
      ]);
      if (showing) {
        name.appendChild(el('span', { class: 'showing-tag', text: 'shown now' }));
      } else {
        name.appendChild(el('button', {
          type: 'button',
          class: 'switch',
          title: 'Walk through the ' + r.label + ' analysis of this grammar',
          on: { click: function () { LRV.selectVariant(r.variant); } },
          text: 'show'
        }));
      }
      tr.appendChild(name);

      tr.appendChild(el('td', { class: 'num', text: String(r.states) }));
      tr.appendChild(el('td', { class: 'num' + (r.conflicts ? ' bad' : ' good'), text: String(r.conflicts) }));
      tr.appendChild(el('td', { class: 'kinds', text: kindsText(r) }));
      tr.appendChild(el('td', { class: 'verdict-cell' }, [
        el('strong', { text: 'is ' + r.class }),
        el('span', { class: 'why', text: r.message })
      ]));
      body.appendChild(tr);
    });
    table.appendChild(body);
    container.appendChild(el('div', { class: 'scroller' }, table));

    if (rows.length < 4) {
      container.appendChild(el('p', {
        class: 'note',
        text: 'A method is listed only if it could be built on this grammar. One that exceeded the state limit is missing from this table rather than guessed at — and, for the same reason, it is never suggested as the method to try instead.'
      }));
    }

    container.appendChild(el('p', {
      class: 'note',
      text: 'A conflict shows only that the method in that row cannot parse this grammar. It is not a proof of ambiguity: the grammar may be perfectly unambiguous and simply need a more powerful parser.'
    }));
  };

  /** Which row is the analysis currently on screen, as announced by the engine. */
  function isCurrent(steps, variantId) {
    var chosen = LRV.variantFrom(steps);
    return !!chosen && chosen.variant === variantId;
  }

})(window.LRV);

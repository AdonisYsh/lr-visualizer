/* FIRST / FOLLOW view: both tables as of step `index`, emphasising the symbol
   the current step added. Everything comes out of the step log. */
window.LRV = window.LRV || {};
window.LRV.views = window.LRV.views || {};

(function (LRV) {
  'use strict';
  var el = LRV.el;

  /* Display order for set members: terminals in grammar order, then $, then ε.
     Members arrive in fixpoint order, which would differ between equal rows. */
  function ranker(g) {
    // Symbols are user text and may be named like Object.prototype members
    // (__proto__, constructor), so no prototype.
    var rank = Object.create(null);
    (g ? g.terminals : []).forEach(function (t, i) { rank[t] = i; });
    return function (symbol) {
      if (symbol === '$') return 1e6;
      if (symbol === 'ε') return 1e7;
      return rank[symbol] === undefined ? 1e5 : rank[symbol];
    };
  }

  /** Replay FirstAdd/FollowAdd up to `index` into { nt: [symbols] }. */
  function accumulate(steps, index, addKind, rankOf) {
    // Symbols are user text and may be named like Object.prototype members
    // (__proto__, constructor), so no prototype.
    var sets = Object.create(null);
    for (var i = 0; i <= index && i < steps.length; i++) {
      var s = steps[i];
      if (s.kind !== addKind) continue;
      if (!sets[s.nonterminal]) sets[s.nonterminal] = [];
      if (sets[s.nonterminal].indexOf(s.symbol) < 0) sets[s.nonterminal].push(s.symbol);
    }
    Object.keys(sets).forEach(function (nt) {
      sets[nt].sort(function (a, b) { return rankOf(a) - rankOf(b); });
    });
    return sets;
  }

  /** Rows to show: every non-terminal, so the shape of the table never jumps. */
  function rowsFor(g) {
    return g ? g.nonterminals.slice() : [];
  }

  /* `link(nt)` may return { index, count } to make that row's set clickable,
     jumping the player to the first table cell the set produced. */
  function buildTable(title, caption, rows, sets, current, link) {
    var table = el('table', { class: 'grid' });
    var thead = el('thead', {}, el('tr', {}, [
      el('th', { text: title }),
      el('th', { text: 'set' })
    ]));
    table.appendChild(thead);
    var tbody = el('tbody');
    rows.forEach(function (nt) {
      var members = sets[nt] || [];
      var cell = el('td', { class: 'set-cell' });
      var target = members.length && link ? link(nt) : null;
      // The members go inside a button when there is somewhere to go, so the
      // link is reachable by keyboard and announced as a control.
      var holder = cell;
      if (target) {
        holder = el('button', {
          class: 'set-link',
          type: 'button',
          title: 'Jump to the ' + target.count + ' table cell' +
                 (target.count === 1 ? '' : 's') + ' this set produced',
          on: { click: function () { LRV.goToStep(target.index); } }
        });
        cell.appendChild(holder);
      }
      if (!members.length) {
        cell.appendChild(el('span', { class: 'sr-only', text: 'empty so far' }));
        cell.appendChild(document.createTextNode('{ }'));
      } else {
        holder.appendChild(document.createTextNode('{ '));
        members.forEach(function (m, i) {
          var isCurrent = current && current.nonterminal === nt && current.symbol === m;
          holder.appendChild(el('span', {
            class: 'set-member' + (isCurrent ? ' current' : ''),
            text: m
          }));
          if (i < members.length - 1) holder.appendChild(document.createTextNode(', '));
        });
        holder.appendChild(document.createTextNode(' }'));
        if (target) holder.appendChild(el('span', { class: 'jump', text: ' →' }));
      }
      tbody.appendChild(el('tr', {}, [
        el('th', { class: 'rowhead', scope: 'row' }, LRV.rich([title + '(', LRV.sym(nt), ')'])),
        cell
      ]));
    });
    table.appendChild(tbody);

    return el('div', {}, [
      el('h2', { class: 'view-title', text: title }),
      el('p', { class: 'note', text: caption }),
      el('div', { class: 'scroller' }, table)
    ]);
  }

  var FOLLOW_CAPTION =
    'FOLLOW(A) is the set of terminals that can appear immediately after A in ' +
    'some derivation from the start symbol. LR(0) never consults these — SLR(1) ' +
    'builds its whole reduce rule out of them, and CLR(1) and LALR(1) go back to ' +
    'ignoring them in favour of per-state lookaheads.';

  var FIRST_CAPTION =
    'FIRST(X) is the set of terminals that can begin a string derived from X. ' +
    'ε in the set means X can also derive nothing at all.';

  /* Captions written once with the method's own name dropped in; the engine
     says which method is shown. */
  function firstCaptionFor(label) {
    return ' This is the set ' + label + ' actually uses: closing an item ' +
      '[A → α • B β, a] gives each new B item the lookaheads FIRST(β a) — FIRST ' +
      'taken over the symbols after B followed by the item\'s own lookahead.';
  }

  function followCaptionUnused(label) {
    return 'FOLLOW(A) is the set of terminals that can appear immediately after ' +
      'A in some derivation from the start symbol. It is shown here for ' +
      'reference — ' + label + ' does not use it. Reduce entries come from the ' +
      'lookaheads each item carries, worked out per state from the context that ' +
      'state is reached in, and FOLLOW(A) is the union of those over every ' +
      'context at once. Confusing the two is the usual way of mistaking ' +
      label + ' for SLR(1).';
  }

  var FOLLOW_CAPTION_DRIVES =
    'FOLLOW(A) is the set of terminals that can appear immediately after A in ' +
    'some derivation from the start symbol. These sets are what drive the ' +
    'reduce entries here: a completed item A → α • is reduced only in the ' +
    'columns of FOLLOW(A), and nowhere else. Click a set to jump to the first ' +
    'table cell it produced.';

  LRV.views.sets = function (steps, index, container) {
    var seen = LRV.upto(steps, index);
    var g = LRV.grammarFrom(seen);
    var rows = rowsFor(g);
    var rankOf = ranker(g);
    var step = steps[index];

    var firstCurrent = step && step.kind === 'firstAdd' ? step : null;
    var followCurrent = step && step.kind === 'followAdd' ? step : null;

    // The engine says whether this method reads its reduce columns from FOLLOW.
    var variant = LRV.variantFrom(steps);
    var drivesReduces = !!variant && variant.reduceRule === 'follow';
    var ignoresFollow = !!variant && variant.reduceRule === 'item-lookahead';

    var label = (variant && variant.label) || 'This method';

    container.appendChild(buildTable(
      'FIRST',
      FIRST_CAPTION + (ignoresFollow ? firstCaptionFor(label) : ''),
      rows,
      accumulate(steps, index, "firstAdd", rankOf),
      firstCurrent
    ));

    container.appendChild(buildTable(
      'FOLLOW',
      drivesReduces ? FOLLOW_CAPTION_DRIVES
        : ignoresFollow ? followCaptionUnused(label)
        : FOLLOW_CAPTION,
      rows,
      accumulate(steps, index, "followAdd", rankOf),
      followCurrent,
      drivesReduces ? function (nt) {
        var cells = LRV.reduceCellsFor(steps, nt);
        return cells.length ? { index: cells[0], count: cells.length } : null;
      } : null
    ));

    if (followCurrent && followCurrent.ruleUsed) {
      var RULES = {
        'start': 'Rule 1 — the start symbol can be followed by the end of input.',
        'beta-first': 'Rule 2 — whatever can begin what comes next can follow this non-terminal.',
        'beta-nullable': 'Rule 3 — everything after it can vanish, so what follows the left-hand side follows it too.',
        'tail': 'Rule 3 — it sits at the very end, so what follows the left-hand side follows it too.'
      };
      container.appendChild(el('p', {
        class: 'note',
        text: RULES[followCurrent.ruleUsed] || followCurrent.ruleUsed
      }));
    }
  };

})(window.LRV);

/* String parse view: a Step | Stack | Input | Action table ending in ACCEPT or REJECT.
 * A parse over a conflicted table is qualified in words and a dashed border, since this view is printed. */
window.LRV = window.LRV || {};
window.LRV.views = window.LRV.views || {};

(function (LRV) {
  'use strict';
  var el = LRV.el;

  /** `0 E 1 + 6`: the stack as the engine sent it, bottom first. */
  function stackRuns(stack) {
    var out = [];
    (stack || []).forEach(function (e, i) {
      if (i) out.push(' ');
      if (e.symbol === null || e.symbol === undefined) out.push(String(e.state));
      else out.push(LRV.sym(e.symbol), ' ' + e.state);
    });
    return out;
  }

  function inputRuns(input, pos) {
    return LRV.symList((input || []).slice(pos), ' ');
  }

  /** The Action column. `caveat` qualifies the row that ends the parse. */
  function actionRuns(g, step, caveat) {
    var qualified = caveat ? ' — ' + caveat.short : '';
    switch (step.kind) {
      case 'parseInit': return 'start';
      case 'parseShift': return ['shift ', LRV.sym(step.symbol), ', go to I' + step.toState];
      case 'parseReduce': return ['reduce ' + step.production + ' (',
        LRV.productionRuns(g, step.production), ')'];
      case 'parseAccept': return 'ACCEPT' + qualified;
      case 'parseReject': return 'REJECT' + qualified;
      default: return '';
    }
  }

  /** A caret line pointing at token `pos` of a space-joined input. */
  function caretLine(input, pos) {
    var offset = 0;
    for (var i = 0; i < pos && i < input.length; i++) {
      offset += String(input[i]).length + 1;
    }
    var pad = '';
    while (pad.length < offset) pad += ' ';
    var width = pos < input.length ? String(input[pos]).length : 1;
    var mark = '';
    while (mark.length < width) mark += '^';
    return pad + mark;
  }

  LRV.views.trace = function (steps, index, container) {
    var g = LRV.grammarFrom(steps);
    var parseSteps = steps.filter(function (s) { return s.phase === 'parse'; });
    // No parse phase without a test string; app.js disables this tab then.
    if (!parseSteps.length) return;
    var caveat = LRV.parseCaveat(steps);

    var input = parseSteps[0].input || [];
    container.appendChild(el('h2', { class: 'view-title' },
      LRV.rich(['Parsing: ', LRV.symList(input, ' ')])));

    var table = el('table', { class: 'trace' });
    table.appendChild(el('thead', {}, el('tr', {}, [
      el('th', { scope: 'col', text: 'Step' }),
      el('th', { scope: 'col', text: 'Stack' }),
      el('th', { scope: 'col', text: 'Input' }),
      el('th', { scope: 'col', text: 'Action' })
    ])));

    var body = el('tbody');
    var shown = 0;
    var pos = 0;
    var stack = [];
    var terminal = null;

    for (var j = 0; j < parseSteps.length; j++) {
      var s = parseSteps[j];
      if (s.index > index) break;
      shown++;
      if (s.stack) stack = s.stack;
      if (typeof s.inputPos === 'number') pos = s.inputPos;
      if (s.kind === 'parseAccept' || s.kind === 'parseReject') terminal = s;

      body.appendChild(el('tr', { class: s.index === index ? 'current' : '' }, [
        el('td', { text: String(shown) }),
        el('td', { class: 'stack' }, LRV.rich(stackRuns(stack))),
        el('td', { class: 'input' }, LRV.rich(inputRuns(input, pos))),
        el('td', {}, LRV.rich(actionRuns(g, s, caveat)))
      ]));
    }
    table.appendChild(body);
    container.appendChild(table);

    if (!terminal) return;

    var accepted = terminal.kind === 'parseAccept';
    var word = accepted ? 'ACCEPT' : 'REJECT';
    var box = el('div', {
      class: 'verdict ' + (accepted ? 'good' : 'bad') + (caveat ? ' qualified' : '')
    }, [
      el('strong', { class: 'headline', text: caveat ? word + ' — ' + caveat.short : word }),
      document.createTextNode(terminal.explain)
    ]);
    if (!accepted) {
      box.appendChild(el('div', { class: 'reject-marker' }, LRV.rich(LRV.symList(input, ' '))));
      box.appendChild(el('div', { class: 'reject-marker', text: caretLine(input, terminal.inputPos) }));
      if (terminal.expected && terminal.expected.length) {
        box.appendChild(el('span', { class: 'caveat' }, LRV.rich([
          'In state I' + terminal.state + ' the parser could have continued on: ',
          LRV.symList(terminal.expected, ', ')
        ])));
      }
    }
    if (caveat) {
      // Words and a mark, not colour, so it survives a photocopy.
      box.appendChild(el('span', { class: 'caveat conflict-caveat' }, [
        el('span', { class: 'warn-mark', text: '⚠ ', 'aria-hidden': 'true' }),
        caveat.rule
      ]));
    }
    container.appendChild(box);
  };

})(window.LRV);

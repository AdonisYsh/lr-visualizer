/* The step player. Contains no parsing theory: it walks the engine's array of
 * steps and asks a renderer to draw the state at index i. Going backwards just
 * changes the index.
 *
 * SECURITY: the grammar is never evaluated and no DOM node is built from a string
 * (no innerHTML / outerHTML / insertAdjacentHTML); text goes through textContent.
 */
window.LRV = window.LRV || {};

(function (LRV) {
  'use strict';

  // ------------------------------------------------------------ DOM helpers

  /** Build an element. `opts` may set class/text/attrs/on; kids are appended. */
  function el(tag, opts, kids) {
    var node = document.createElement(tag);
    apply(node, opts);
    append(node, kids);
    return node;
  }

  /** Same, in the SVG namespace. */
  function svg(tag, opts, kids) {
    var node = document.createElementNS('http://www.w3.org/2000/svg', tag);
    apply(node, opts, true);
    append(node, kids);
    return node;
  }

  function apply(node, opts, isSvg) {
    if (!opts) return;
    Object.keys(opts).forEach(function (key) {
      var value = opts[key];
      if (value === null || value === undefined) return;
      if (key === 'text') {
        node.appendChild(document.createTextNode(String(value)));
      } else if (key === 'class') {
        if (isSvg) node.setAttribute('class', String(value));
        else node.className = String(value);
      } else if (key === 'on') {
        Object.keys(value).forEach(function (evt) {
          node.addEventListener(evt, value[evt]);
        });
      } else if (key === 'style' && typeof value === 'object') {
        Object.keys(value).forEach(function (prop) { node.style[prop] = value[prop]; });
      } else if (!isSvg && (key === 'value' || key === 'checked' || key === 'disabled' ||
                            key === 'selected' || key === 'textContent')) {
        node[key] = value;
      } else {
        node.setAttribute(key, String(value));
      }
    });
  }

  function append(node, kids) {
    if (kids === null || kids === undefined) return;
    if (!Array.isArray(kids)) kids = [kids];
    kids.forEach(function (kid) {
      if (kid === null || kid === undefined || kid === false) return;
      node.appendChild(typeof kid === 'string' || typeof kid === 'number'
        ? document.createTextNode(String(kid))
        : kid);
    });
  }

  function clear(node) {
    while (node.firstChild) node.removeChild(node.firstChild);
  }

  LRV.el = el;
  LRV.svg = svg;
  LRV.clear = clear;

  // -------------------------------------------- reading the step log itself
  // Every renderer derives what it needs from the steps, so each view module
  // stays swappable without touching this file.

  /** The grammar as announced by step 0, or null. */
  LRV.grammarFrom = function (steps) {
    for (var i = 0; i < steps.length; i++) {
      if (steps[i].kind === 'grammarParsed') return steps[i];
    }
    return null;
  };

  /** The method this analysis was run under, as announced by the engine.
   *  Carries `variant`, `label` and `reduceRule` - the frontend switches on
   *  the tag, it never works out for itself what a variant does. */
  LRV.variantFrom = function (steps) {
    for (var i = 0; i < steps.length; i++) {
      if (steps[i].kind === 'variantChosen') return steps[i];
    }
    return null;
  };

  /** Step indices of the reduce cells written for non-terminal `nt`.
   *  A lookup, not a derivation: the engine decided which columns get the
   *  entry, and each cell already names the production it reduces by. */
  LRV.reduceCellsFor = function (steps, nt) {
    var g = LRV.grammarFrom(steps);
    var out = [];
    if (!g) return out;
    for (var i = 0; i < steps.length; i++) {
      var s = steps[i];
      var action = s.kind === 'tableCellSet' ? s.action
                 : s.kind === 'tableConflict' ? s.incoming
                 : null;
      if (!action || action.type !== 'reduce') continue;
      var p = g.productions[action.production];
      if (p && p.lhs === nt) out.push(i);
    }
    return out;
  };

  /* ------------------------------------------------ symbols in running text
     A symbol is user text. A right-to-left letter or direction control (U+202E)
     joined into a line would reorder the rest of it, so such symbols are drawn
     in an isolating element of their own. A line of display text is a list of
     *runs*: plain strings, and `{ sym }` for a symbol that must be isolated.
     LRV.rich turns runs into nodes, LRV.flat into one string. */

  // Right-to-left scripts, direction marks, and embedding/override/isolate controls.
  var DIRECTIONAL = /[\u0590-\u08FF\u200E\u200F\u202A-\u202E\u2066-\u2069\uFB1D-\uFDFF\uFE70-\uFEFC]|[\uD802\uD803\uD83A\uD83B][\uDC00-\uDFFF]/;

  /** A symbol, ready to go in a list of runs: the string itself when it is
   *  harmless, a `{ sym }` marker when it has to be isolated. */
  LRV.sym = function (symbol) {
    var text = String(symbol);
    return DIRECTIONAL.test(text) ? { sym: text } : text;
  };

  /** Symbols with `sep` between them, as a list of runs. */
  LRV.symList = function (symbols, sep) {
    var out = [];
    (symbols || []).forEach(function (s, i) {
      if (i) out.push(sep);
      out.push(LRV.sym(s));
    });
    return out;
  };

  /** Any nesting of strings and markers, flattened, with neighbouring
   *  strings joined: `['S → ', { sym: '...' }, ' • T']`, or just `['E → • T']`. */
  LRV.runs = function (parts) {
    var out = [];
    (function walk(p) {
      if (p === null || p === undefined || p === false) return;
      if (Array.isArray(p)) { p.forEach(walk); return; }
      if (typeof p === 'object') { out.push({ sym: String(p.sym) }); return; }
      var text = String(p);
      if (!text) return;
      if (out.length && typeof out[out.length - 1] === 'string') out[out.length - 1] += text;
      else out.push(text);
    })(parts);
    return out;
  };

  /** Runs as nodes, to hand to `el` / `svg` as children: a text node per
   *  string, an isolating element per marked symbol. */
  LRV.rich = function (parts, inSvg) {
    return LRV.runs(parts).map(function (run) {
      if (typeof run === 'string') return document.createTextNode(run);
      return inSvg
        ? svg('tspan', { class: 'sym', text: run.sym })
        : el('bdi', { class: 'sym', text: run.sym });
    });
  };

  /** Runs as one plain string. */
  LRV.flat = function (parts) {
    return LRV.runs(parts).map(function (run) {
      return typeof run === 'string' ? run : run.sym;
    }).join('');
  };

  /** The core of an item: `E → E • + T`, with the dot drawn at `item.dot` and
   *  no lookaheads. As runs; `LRV.itemCore` is the same thing as a string. */
  LRV.itemCoreRuns = function (g, item) {
    var p = g.productions[item.production];
    if (!p) return ['?'];
    var body = [];
    for (var i = 0; i < p.rhs.length; i++) {
      if (i === item.dot) body.push('•');
      body.push(LRV.sym(p.rhs[i]));
    }
    // Also the whole of an empty right-hand side: a lone dot.
    if (item.dot >= p.rhs.length) body.push('•');
    var spaced = [];
    body.forEach(function (part, n) {
      if (n) spaced.push(' ');
      spaced.push(part);
    });
    return LRV.runs([LRV.sym(p.lhs), ' → ', spaced]);
  };
  LRV.itemCore = function (g, item) { return LRV.flat(LRV.itemCoreRuns(g, item)); };

  /** The lookahead half of a CLR(1) item: `, {+, ), $}`, or nothing when the
   *  item carries none. Kept apart from the core so a renderer can draw it as
   *  secondary - the core has to stay scannable with a set hung off it. */
  LRV.itemLookaheadsRuns = function (item) {
    if (!item.lookaheads || !item.lookaheads.length) return [];
    return LRV.runs([', {', LRV.symList(item.lookaheads, ', '), '}']);
  };
  LRV.itemLookaheads = function (item) { return LRV.flat(LRV.itemLookaheadsRuns(item)); };

  /** `E → E • + T, {+, ), $}` - both halves. */
  LRV.itemRuns = function (g, item) {
    return LRV.runs([LRV.itemCoreRuns(g, item), LRV.itemLookaheadsRuns(item)]);
  };
  LRV.itemText = function (g, item) { return LRV.flat(LRV.itemRuns(g, item)); };

  /** `E → E + T`, or `A → ε`. */
  LRV.productionRuns = function (g, index) {
    var p = g.productions[index];
    if (!p) return ['?'];
    return LRV.runs([LRV.sym(p.lhs), ' → ', p.rhs.length ? LRV.symList(p.rhs, ' ') : 'ε']);
  };
  LRV.productionText = function (g, index) { return LRV.flat(LRV.productionRuns(g, index)); };

  /** The conventional short form of a table entry: s5, r3, acc, 7. */
  LRV.actionText = function (a) {
    if (!a) return '';
    if (a.type === 'shift') return 's' + a.state;
    if (a.type === 'reduce') return 'r' + a.production;
    if (a.type === 'accept') return 'acc';
    if (a.type === 'goto') return String(a.state);
    return '';
  };

  /* ---------------------------------------------------- one-line summaries
     One template per step kind, filled only from the step's fields. A kind with
     no template falls back to the first sentence of the engine's explain. */

  /** The first sentence of the engine's explain, for kinds with no template. */
  function firstSentence(text) {
    var t = String(text || '').trim();
    var m = /^[\s\S]*?[.!?](?=\s|$)/.exec(t);
    return m ? m[0] : t;
  }

  function stateName(n) { return 'I' + n; }

  /** `shift 7`, `reduce by T → F`, `accept`, `7` - a table entry in words. */
  function actionWords(g, a) {
    if (!a) return '';
    if (a.type === 'shift') return 'shift ' + a.state;
    if (a.type === 'reduce') return ['reduce by ', LRV.productionRuns(g, a.production)];
    if (a.type === 'accept') return 'accept';
    if (a.type === 'goto') return String(a.state);
    return LRV.actionText(a);
  }

  /** `ACTION[2, *]` or `GOTO[0, E]`, from the entry's own type tag. */
  function cellName(a, st, symbol) {
    return [(a && a.type === 'goto' ? 'GOTO' : 'ACTION') + '[' + st + ', ', LRV.sym(symbol), ']'];
  }

  /** The engine's verdict on the table, as its `tableDone` step states it
   *  (`isValid`, `conflicts`, `class`), or null before there is one. */
  LRV.tableDoneFrom = function (steps) {
    for (var i = 0; i < steps.length; i++) {
      if (steps[i].kind === 'tableDone') return steps[i];
    }
    return null;
  };

  /* What a parse is worth on a table with conflicts: the trace took the first
     action of a conflicted cell, so a bare ACCEPT would mislead. Read from the
     engine's tableDone step; returns null for a clean table. */
  LRV.parseCaveat = function (steps) {
    var done = steps ? LRV.tableDoneFrom(steps) : null;
    if (!done || done.isValid !== false) return null;
    var n = done.conflicts;
    var count = typeof n === 'number' && n > 0
      ? n + (n === 1 ? ' conflict' : ' conflicts')
      : 'conflicts';
    return {
      // For the headline and the trace's last row: "ACCEPT — but ..."
      short: 'but this table has ' + count,
      // The rule, in the engine's own terms (parser.rs).
      rule: 'In a conflicted cell the parser takes the first action it finds, so ' +
            'a trace that went through one is not a meaningful parse. The conflicted ' +
            'cells are marked in the Parsing Table tab.'
    };
  };

  /** ` — but this table has 2 conflicts; in a conflicted cell ...`, or ''. */
  function caveatClause(steps) {
    var caveat = LRV.parseCaveat(steps);
    return caveat
      ? ' — ' + caveat.short + '; in a conflicted cell the parser takes the first action it finds'
      : '';
  }

  // Display names for the engine's own tags. An unknown tag is shown as is.
  var REDUCE_RULE_WORDS = {
    'every-column': 'reduces in every column',
    'follow': 'reduces on FOLLOW of the left-hand side',
    'item-lookahead': 'reduces on each item\'s own lookaheads'
  };
  var FOLLOW_RULE_WORDS = {
    'start': 'start symbol',
    'beta-first': 'FIRST of what follows',
    'beta-nullable': 'what follows can vanish',
    'tail': 'at the end of a production'
  };

  var SUMMARIES = {
    variantChosen: function (s) {
      return 'Method: ' + s.label + ' - ' + (REDUCE_RULE_WORDS[s.reduceRule] || s.reduceRule);
    },
    grammarParsed: function (s) {
      return 'Grammar read in ' + s.mode + ' mode';
    },
    augmented: function (s) {
      var p = s.production;
      return ['Augmented with production ' + p.index + ': ', LRV.sym(p.lhs), ' → ',
        p.rhs.length ? LRV.symList(p.rhs, ' ') : 'ε'];
    },
    firstAdd: function (s, g) {
      return ['FIRST(', LRV.sym(s.nonterminal), ') gains ', LRV.sym(s.symbol), ', from ',
        LRV.productionRuns(g, s.fromProduction)];
    },
    firstDone: function () { return 'FIRST sets complete'; },
    followAdd: function (s, g) {
      var rule = FOLLOW_RULE_WORDS[s.ruleUsed] || s.ruleUsed;
      return ['FOLLOW(', LRV.sym(s.nonterminal), ') gains ', LRV.sym(s.symbol), ', from ',
        LRV.productionRuns(g, s.fromProduction), ' (' + rule + ')'];
    },
    followDone: function () { return 'FOLLOW sets complete'; },
    stateCreated: function (s, g) {
      var kernel = s.kernel.length === 1
        ? ['kernel ', LRV.itemRuns(g, s.kernel[0])]
        : s.kernel.length + ' kernel items';
      return s.fromState === null || s.fromState === undefined
        ? [stateName(s.id) + ' created as the start state, ', kernel]
        : [stateName(s.id) + ' created as GOTO(' + stateName(s.fromState) + ', ',
           LRV.sym(s.onSymbol), '), ', kernel];
    },
    closureItemAdded: function (s, g) {
      return ['Closure of ' + stateName(s.state) + ' adds ', LRV.itemRuns(g, s.item)];
    },
    stateClosed: function (s) {
      return stateName(s.id) + ' closed: ' + s.items.length +
        (s.items.length === 1 ? ' item' : ' items');
    },
    transitionAdded: function (s) {
      return [stateName(s.from) + ' on ', LRV.sym(s.symbol), ' goes to ' + stateName(s.to) +
        (s.targetIsNew ? ' (new state)' : ' (existing state)')];
    },
    statesMerged: function (s) {
      // mergedIds are the original CLR(1) ids and the drawing uses them until
      // mergeDone; newId is the post-renumbering id, so it is only a footnote.
      var keep = s.mergedIds[0];
      return 'Merge ' + s.mergedIds.map(stateName).join(' + ') + ' into ' + stateName(keep) +
        ', lookaheads unioned' +
        (s.newId !== keep ? ' (renumbered ' + stateName(s.newId) + ' at the end)' : '');
    },
    mergeDone: function (s) {
      return 'Merging done: ' + s.beforeLabel + ' ' + s.before + ' states → ' +
        s.afterLabel + ' ' + s.after + ' states';
    },
    automatonDone: function (s) {
      return 'Automaton complete: ' + s.stateCount +
        (s.stateCount === 1 ? ' state' : ' states');
    },
    tableCellSet: function (s, g) {
      return [cellName(s.action, s.state, s.symbol), ' = ', actionWords(g, s.action)];
    },
    tableConflict: function (s, g) {
      return ['Conflict (' + s.conflictKind + ') in ', cellName(s.incoming, s.state, s.symbol),
        ': ', actionWords(g, s.existing), ' vs ', actionWords(g, s.incoming),
        s.mergeInduced ? ', caused by merging' : ''];
    },
    tableDone: function (s) {
      return 'Table complete: ' + s.conflicts +
        (s.conflicts === 1 ? ' conflict' : ' conflicts') + ', ' + s.class;
    },
    // This method's own table only: the four-method comparison lives in the
    // "Compare all four" tab. With no row for this method the lookups below
    // throw, and stepSummaryRuns falls back to the engine's own sentence.
    variantComparison: function (s, g, steps) {
      var chosen = LRV.variantFrom(steps);
      var mine = s.results.filter(function (r) {
        return r.variant === chosen.variant;
      })[0];
      if (mine.conflicts === 0) return 'Parsing table complete';
      return 'Parsing table complete: ' + mine.conflicts +
        (mine.conflicts === 1 ? ' conflict' : ' conflicts') +
        ' — the walkthrough stops here';
    },
    parseInit: function (s) {
      var top = s.stack[s.stack.length - 1];
      return ['Parse starts in ' + stateName(top ? top.state : 0) + ' with input ',
        LRV.symList(s.input, ' ')];
    },
    parseShift: function (s) {
      return ['Shift ', LRV.sym(s.symbol), ', go to ' + stateName(s.toState)];
    },
    parseReduce: function (s, g) {
      return ['Reduce by ', LRV.productionRuns(g, s.production), ', then go to ' +
        stateName(s.gotoState)];
    },
    // The two steps that end a parse also say what the table they ended it
    // on is worth: see LRV.parseCaveat.
    parseAccept: function (s, g, steps) { return 'Accept' + caveatClause(steps); },
    parseReject: function (s, g, steps) {
      return ['Reject in ' + stateName(s.state) + ' on ', LRV.sym(s.lookahead),
        s.expected && s.expected.length
          ? [' (expected ', LRV.symList(s.expected, ', '), ')'] : '',
        caveatClause(steps)];
    }
  };

  /** One line for the player, as runs, from the step's fields. `steps` is
   *  the log the step came from: the two steps that end a parse read the
   *  engine's verdict on the table out of it, and nothing else looks.
   *  Any kind without a template, or any step a template cannot read, falls
   *  back to the first sentence of the engine's own explain - never to
   *  nothing. */
  LRV.stepSummaryRuns = function (step, g, steps) {
    if (!step) return [];
    var template = Object.prototype.hasOwnProperty.call(SUMMARIES, step.kind)
      ? SUMMARIES[step.kind] : null;
    if (template && g) {
      try {
        var line = LRV.runs(template(step, g, steps));
        if (line.length) return line;
      } catch (e) {
        // A step missing a field it normally has: say what the engine said.
      }
    }
    return LRV.runs(firstSentence(step.explain));
  };

  /** The same line as one string. */
  LRV.stepSummary = function (step, g, steps) {
    return LRV.flat(LRV.stepSummaryRuns(step, g, steps));
  };

  /** Steps 0..index inclusive - i.e. everything that has "happened" so far. */
  LRV.upto = function (steps, index) {
    return steps.slice(0, Math.min(index + 1, steps.length));
  };

  // ------------------------------------------------------------------ state

  // Five phase tabs, plus "Compare all four", which summarises the finished result
  // (`always`: available whenever there is a result, never jumped away from).
  var TABS = [
    { id: 'grammar',   phase: 'grammar',   label: 'Grammar' },
    { id: 'sets',      phase: 'sets',      label: 'FIRST / FOLLOW' },
    { id: 'items',     phase: 'automaton', label: 'Item Sets' },
    { id: 'table',     phase: 'table',     label: 'Parsing Table' },
    { id: 'parse',     phase: 'parse',     label: 'String Parse' },
    { id: 'compare',   phase: null,        label: 'Compare all four', always: true }
  ];

  // Weakest method first, matching the engine's ordering.
  var VARIANTS = [
    { id: 'lr0',   label: 'LR(0)' },
    { id: 'slr1',  label: 'SLR(1)' },
    { id: 'lalr1', label: 'LALR(1)' },
    { id: 'clr1',  label: 'CLR(1)' }
  ];

  var state = {
    analyze: null,
    result: null,
    steps: [],
    index: 0,
    tab: 'grammar',
    variant: 'lr0',
    playing: false,
    timer: null,
    // Milliseconds per step while playing; matches the selected option in index.html.
    speed: 2500,
    // Whether the full explain shows under the summary. In memory only.
    fullExplain: false,
    // Set once analyze() has thrown: a WASM trap leaves the instance unusable.
    engineStopped: false,
    engineStopDetail: '',
    // Whether the side panel is folded to a rail. In memory only.
    panelCollapsed: false,
    // Index into LRV.PRESETS of the example last loaded, or -1 (see syncPreset).
    preset: -1,
    // The steps array the open grammar panel was filled from, or null when closed.
    grammarPanelFor: null
  };

  var dom = {};

  // ----------------------------------------------------------------- render

  /* First and last step index of every phase, cached per steps array
     (every analysis parses a new array and nothing writes to one). */
  var phaseRanges = { steps: null, byPhase: Object.create(null) };

  function phaseRange(phase) {
    if (phaseRanges.steps !== state.steps) {
      var byPhase = Object.create(null);
      for (var i = 0; i < state.steps.length; i++) {
        var range = byPhase[state.steps[i].phase];
        if (range) range.last = i;
        else byPhase[state.steps[i].phase] = { first: i, last: i };
      }
      phaseRanges = { steps: state.steps, byPhase: byPhase };
    }
    return phaseRanges.byPhase[phase] || null;
  }

  function tabForIndex(i) {
    var step = state.steps[i];
    if (!step) return state.tab;
    for (var t = 0; t < TABS.length; t++) {
      if (TABS[t].phase === step.phase) return TABS[t].id;
    }
    return state.tab;
  }

  function tabAvailable(tab) {
    return tab.always ? !!state.result : !!phaseRange(tab.phase);
  }

  /* Tab buttons are built once and then updated: rebuilding would drop the user's focus. */
  var tabButtons = {};

  function buildTabs() {
    clear(dom.tabs);
    tabButtons = {};
    TABS.forEach(function (tab, n) {
      var button = el('button', {
        class: 'tab' + (tab.always ? ' aside' : ''),
        type: 'button',
        role: 'tab',
        id: 'tab-' + tab.id,
        'aria-controls': 'view',
        on: {
          click: function () { selectTab(tab.id); },
          keydown: tabKey
        }
      }, [
        tab.always ? null : el('span', { class: 'num', text: String(n + 1) }),
        tab.label
      ]);
      tabButtons[tab.id] = button;
      dom.tabs.appendChild(button);
    });

    // A disabled button cannot be focused, so the reason is exposed outside the tablist.
    var old = document.getElementById('tab-reasons');
    if (old && old.parentNode) old.parentNode.removeChild(old);
    var reasons = el('div', { id: 'tab-reasons', class: 'sr-only' });
    TABS.forEach(function (tab) {
      reasons.appendChild(el('span', { id: 'tab-why-' + tab.id }));
    });
    dom.tabs.parentNode.insertBefore(reasons, dom.tabs.nextSibling);
  }

  /** Why a tab is unavailable right now, in words; null when it is available. */
  function tabUnavailableReason(tab) {
    if (tabAvailable(tab)) return null;
    if (tab.always) return 'Analyze a grammar first';
    // The parse phase is missing for an empty test string, or cut on a conflicted
    // table (see playableSteps); the full log in state.result tells them apart.
    if (tab.id === 'parse' && state.result) {
      var full = state.result.steps || [];
      var hadParse = full.some(function (s) { return s.phase === 'parse'; });
      return hadParse
        ? 'The table has conflicts, so the walkthrough stops at the table'
        : 'Enter a test string to see the parse';
    }
    return 'No steps in this phase yet';
  }

  function renderTabs() {
    TABS.forEach(function (tab) {
      var button = tabButtons[tab.id];
      if (!button) return;
      var available = tabAvailable(tab);
      // A tab with nothing to show is never drawn as selected.
      var selected = available && state.tab === tab.id;
      button.disabled = !available;
      button.setAttribute('aria-selected', String(selected));
      // Roving tabindex: the whole strip is one stop in the Tab order and the
      // arrow keys move within it, which is what a tablist is expected to do.
      button.setAttribute('tabindex', selected ? '0' : '-1');
      var why = tabUnavailableReason(tab);
      var reason = document.getElementById('tab-why-' + tab.id);
      if (reason) reason.textContent = why || '';
      if (why) {
        button.setAttribute('title', why);
        button.setAttribute('aria-describedby', 'tab-why-' + tab.id);
      } else {
        button.removeAttribute('title');
        button.removeAttribute('aria-describedby');
      }
    });
    dom.view.setAttribute('aria-labelledby', 'tab-' + state.tab);
  }

  /** Arrow keys move between tabs. They are stopped here rather than allowed to
   *  bubble, because the same keys step the player everywhere else. */
  function tabKey(ev) {
    var order = TABS.filter(tabAvailable);
    if (!order.length) return;
    var ids = order.map(function (t) { return t.id; });
    var here = Math.max(0, ids.indexOf(state.tab));
    var to;
    switch (ev.key) {
      case 'ArrowRight': case 'ArrowDown': to = (here + 1) % ids.length; break;
      case 'ArrowLeft': case 'ArrowUp': to = (here - 1 + ids.length) % ids.length; break;
      case 'Home': to = 0; break;
      case 'End': to = ids.length - 1; break;
      default: return;
    }
    ev.preventDefault();
    ev.stopPropagation();
    selectTab(ids[to]);
    var next = tabButtons[ids[to]];
    if (next) next.focus();
  }

  function selectTab(id) {
    var tab = TABS.filter(function (t) { return t.id === id; })[0];
    if (!tab || !tabAvailable(tab)) return;
    state.tab = id;
    if (!tab.always) {
      // Switching to a phase tab jumps the player to that phase's first step.
      var range = phaseRange(tab.phase);
      if (state.index < range.first || state.index > range.last) {
        state.index = range.first;
      }
    }
    renderAll();
  }

  // What the stage holds now: which renderer drew it, from which steps array. Null otherwise.
  var stageHolds = null;

  /* A view may offer `render.update(steps, index, container, result)`, which redraws
     in place what it last drew and returns true, or returns false to ask for a full
     draw. It is asked only while the stage still holds that renderer's drawing of
     this same steps array, and must leave exactly the DOM a full draw would build. */
  function renderView() {
    var held = stageHolds;
    stageHolds = null;
    if (!state.result) {
      clear(dom.view);
      dom.view.appendChild(el('p', {
        class: 'empty-state',
        text: 'Pick an example or type a grammar, then press Analyze. Every step of the analysis is recorded, so you can walk forwards and backwards through it at your own pace.'
      }));
      return;
    }
    var renderers = {
      grammar: LRV.views.grammar,
      sets: LRV.views.sets,
      items: LRV.views.diagram,
      table: LRV.views.table,
      parse: LRV.views.trace,
      compare: LRV.views.compare
    };
    var render = renderers[state.tab];
    // The fourth argument is the finished result. Step-driven views ignore it;
    // the comparison view is a summary of the whole run and needs it.
    if (!render) { clear(dom.view); return; }
    try {
      var inPlace = !!held && held.render === render && held.steps === state.steps &&
        typeof render.update === 'function' &&
        render.update(state.steps, state.index, dom.view, state.result) === true;
      if (!inPlace) {
        clear(dom.view);
        render(state.steps, state.index, dom.view, state.result);
      }
      stageHolds = { render: render, steps: state.steps };
    } catch (e) {
      // A bug in one view must not take the player and the other tabs down
      // with it, and must not be mistaken for the engine failing to load.
      clear(dom.view);
      dom.view.appendChild(el('p', {
        class: 'empty-state',
        text: 'This view could not be drawn at this step. The other tabs and the player still work.'
      }));
      reportRenderError(e);
    }
  }

  function renderPlayer() {
    var total = state.steps.length;
    var has = total > 0;
    dom.scrub.max = String(Math.max(0, total - 1));
    dom.scrub.value = String(state.index);
    dom.scrub.disabled = !has;
    // There is no visible counter; the slider says where it is to a screen
    // reader instead (the explanation panel names the phase).
    dom.scrub.setAttribute('aria-valuetext', has
      ? 'Step ' + (state.index + 1) + ' of ' + total
      : 'No analysis yet');

    dom.first.disabled = !has || state.index === 0;
    dom.prev.disabled = !has || state.index === 0;
    dom.next.disabled = !has || state.index >= total - 1;
    dom.last.disabled = !has || state.index >= total - 1;
    dom.play.disabled = !has;
    dom.reset.disabled = !has;
    dom.grammarBtn.disabled = !has;
    // A new analysis, a variant switch and an error all replace the steps
    // array: close rather than ever show the grammar of an earlier one.
    if (state.grammarPanelFor !== null && state.grammarPanelFor !== state.steps) {
      closeGrammarPanel();
    }

    clear(dom.play);
    // Plain words rather than media glyphs: ⏸/⏮ are missing from a surprising
    // number of system fonts and render as tofu.
    dom.play.appendChild(document.createTextNode(state.playing ? 'Pause' : '▶ Play'));
    dom.play.setAttribute('aria-label', state.playing ? 'Pause' : 'Play');

    renderExplain();
  }

  /* The explanation under the player: a one-line summary built from the
     step's fields, and the engine's full `explain` behind a disclosure button.
     The nodes are static in index.html and only updated here - rebuilding the
     toggle on every render would drop keyboard focus off it each time. */
  function renderExplain() {
    var step = state.steps[state.index];
    dom.explainPhase.textContent = step ? step.phase : '';
    dom.explainPhase.hidden = !step;
    clear(dom.explainSummary);
    if (step) {
      append(dom.explainSummary,
        LRV.rich(LRV.stepSummaryRuns(step, LRV.grammarFrom(state.steps), state.steps)));
    }
    dom.explainFull.textContent = step ? (step.explain || '') : '';
    dom.explainFull.hidden = !step || !state.fullExplain;
    dom.explainToggle.hidden = !step;
    dom.explainToggle.setAttribute('aria-expanded', String(state.fullExplain));
    dom.explainToggle.textContent = state.fullExplain
      ? 'Hide full explanation'
      : 'Show full explanation';
  }

  function toggleFullExplain() {
    state.fullExplain = !state.fullExplain;
    renderExplain();
  }

  /** Everything around the stage: the tab strip and the
   *  player with its explanation. Cheap at any grammar size. */
  function renderChrome() {
    var current = TABS.filter(function (t) { return t.id === state.tab; })[0];
    // Playing through a phase boundary carries the view along with it -
    // unless the user has deliberately parked on a tab that is not a phase.
    if (!current || !current.always) {
      var wanted = tabForIndex(state.index);
      if (wanted !== state.tab) state.tab = wanted;
    }
    renderTabs();
    renderPlayer();
  }

  function renderAll() {
    // Drawing the stage now pays any redraw the slider still owes it.
    cancelStageFrame();
    clearRenderError();
    try {
      renderChrome();
      renderView();
    } catch (e) {
      reportRenderError(e);
    }
  }

  /* The slider's half of rendering. Index, label and explanation move on every
     input event; the stage is drawn at most once per animation frame, at the
     current index. Anything else that draws goes through renderAll, which cancels
     the pending frame. */
  var stageFrame = 0;

  function cancelStageFrame() {
    if (stageFrame) window.cancelAnimationFrame(stageFrame);
    stageFrame = 0;
  }

  function renderStageNextFrame() {
    clearRenderError();
    try {
      renderChrome();
    } catch (e) {
      reportRenderError(e);
    }
    if (stageFrame) return;
    stageFrame = window.requestAnimationFrame(function () {
      stageFrame = 0;
      try {
        renderView();
      } catch (e) {
        reportRenderError(e);
      }
    });
  }

  // ---------------------------------------------------- rendering failures
  // A bug in a renderer is ours, not the engine's and not the grammar's, and
  // it is reported as exactly that. The loader's "could not be loaded"
  // message (LRV.bootFailed) is kept for the engine genuinely failing to start.

  var renderErrorNode = null;

  function clearRenderError() {
    if (renderErrorNode && renderErrorNode.parentNode) {
      renderErrorNode.parentNode.removeChild(renderErrorNode);
    }
    renderErrorNode = null;
  }

  /** Also what the two loaders (web/index.html and the one build.sh writes)
   *  call when the page's own start-up throws, so that message is worded in
   *  exactly one place. */
  LRV.reportRenderError = reportRenderError;

  function reportRenderError(e) {
    if (window.console && typeof window.console.error === 'function') {
      window.console.error('LR visualizer: internal rendering error', e);
    }
    var box = dom.messages || document.getElementById('messages');
    if (!box) return;
    // The messages live in the side panel; a collapsed panel would hide the
    // report. Opened without a re-render, which would clear it again.
    if (state.panelCollapsed) setPanelCollapsed(false, false);
    clearRenderError();
    var why = (e && e.message) ? e.message : String(e);
    renderErrorNode = message('error', 'Internal rendering error', why,
      'This is a bug in the visualizer, not a problem with your grammar.');
    // First, above any grammar warnings: it is the most urgent thing there.
    box.insertBefore(renderErrorNode, box.firstChild);
  }

  /* Errors thrown outside renderAll - in a diagram node's click handler, a zoom
     button, a drag - never reach its try/catch. They are reported the same
     way: one "Internal rendering error" box in #messages (reportRenderError
     replaces any box already showing, so a handler that keeps failing does not
     stack them up) and a console.error. The browser's own console report is
     left alone. */
  window.addEventListener('error', function (ev) {
    reportRenderError(ev && ev.error ? ev.error : new Error((ev && ev.message) || 'Unknown error'));
  });
  window.addEventListener('unhandledrejection', function (ev) {
    var reason = ev ? ev.reason : null;
    reportRenderError(reason instanceof Error ? reason : new Error(String(reason)));
  });

  // ------------------------------------------------- panel and theme chrome
  // Both controls are pure presentation and work before the engine has
  // loaded, so they are wired as soon as this script runs (it is included
  // after the markup it touches).

  /** Fold the side panel to a rail, or open it again. Only a class and the
   *  `hidden` attribute change - the form is not rebuilt, so the grammar, test
   *  string, variant, tab and step all survive. `rerender` redraws the current
   *  view through renderAll, so the diagram re-fits (or keeps following) at the
   *  new width; the diagram only measures itself when it is drawn. */
  function setPanelCollapsed(collapsed, rerender) {
    var app = document.getElementById('app');
    var body = document.getElementById('panel-body');
    var toggle = document.getElementById('panel-toggle');
    if (!app || !body || !toggle) return;
    state.panelCollapsed = collapsed;
    app.classList.toggle('panel-collapsed', collapsed);
    body.hidden = collapsed;
    toggle.setAttribute('aria-expanded', String(!collapsed));
    toggle.textContent = collapsed ? '»' : '«';
    toggle.setAttribute('title', collapsed ? 'Show the side panel' : 'Hide the side panel');
    if (rerender && state.analyze) renderAll();
  }

  var darkQuery = typeof window.matchMedia === 'function'
    ? window.matchMedia('(prefers-color-scheme: dark)')
    : null;

  /** The theme on screen: the toggle's choice if there is one, else the
   *  system's preference. */
  function currentTheme() {
    var chosen = document.documentElement.getAttribute('data-theme');
    if (chosen === 'dark' || chosen === 'light') return chosen;
    return darkQuery && darkQuery.matches ? 'dark' : 'light';
  }

  function showTheme() {
    var button = document.getElementById('theme-toggle');
    if (!button) return;
    var dark = currentTheme() === 'dark';
    button.setAttribute('aria-pressed', String(dark));
    button.setAttribute('title', dark ? 'Switch to the light theme' : 'Switch to the dark theme');
  }

  function setupChrome() {
    var panelToggle = document.getElementById('panel-toggle');
    if (panelToggle) {
      panelToggle.addEventListener('click', function () {
        setPanelCollapsed(!state.panelCollapsed, true);
      });
    }
    var themeToggle = document.getElementById('theme-toggle');
    if (themeToggle) {
      themeToggle.addEventListener('click', function () {
        // Not saved anywhere: storage is banned, so a reload follows the system again.
        document.documentElement.dataset.theme = currentTheme() === 'dark' ? 'light' : 'dark';
        showTheme();
      });
    }
    // Until the user chooses, the button tracks the system setting live.
    if (darkQuery) {
      if (typeof darkQuery.addEventListener === 'function') darkQuery.addEventListener('change', showTheme);
      else if (typeof darkQuery.addListener === 'function') darkQuery.addListener(showTheme);
    }
    showTheme();
  }

  // ---------------------------------------------------------------- control

  function goto_(i) {
    if (!state.steps.length) return;
    var clamped = Math.max(0, Math.min(i, state.steps.length - 1));
    // Already there, and nothing to redraw - unless the slider has just moved
    // the index and the stage has not been drawn there yet (`stageFrame`). A
    // button or a key always leaves the stage showing the step it landed on.
    if (clamped === state.index && !stageFrame) { renderPlayer(); return; }
    state.index = clamped;
    renderAll();
  }

  /** The slider's goto_: the same index change, with the stage redrawn in the
   *  next animation frame rather than once per event (see
   *  renderStageNextFrame). */
  function scrubTo(i) {
    if (!state.steps.length) return;
    var clamped = Math.max(0, Math.min(i, state.steps.length - 1));
    if (clamped === state.index) { renderPlayer(); return; }
    state.index = clamped;
    renderStageNextFrame();
  }

  function step(delta) {
    if (state.playing && state.index + delta >= state.steps.length - 1) stopPlay();
    goto_(state.index + delta);
  }

  function startPlay() {
    if (!state.steps.length || state.index >= state.steps.length - 1) return;
    state.playing = true;
    // Note: a function, never a string - see the security notes above.
    state.timer = window.setInterval(function () { step(1); }, state.speed);
    renderPlayer();
  }

  function stopPlay() {
    state.playing = false;
    if (state.timer !== null) { window.clearInterval(state.timer); state.timer = null; }
    renderPlayer();
  }

  function togglePlay() { if (state.playing) stopPlay(); else startPlay(); }

  /* "Show grammar": an overlay listing the productions, read from the step log.
     It touches no index. */
  function openGrammarPanel() {
    var g = LRV.grammarFrom(state.steps);
    if (!g) return;
    stopPlay();
    clear(dom.grammarList);
    // Production 0 is the augmented S' -> S, which is what <ol start="0"> numbers.
    g.productions.forEach(function (p, i) {
      dom.grammarList.appendChild(el('li', { class: i === 0 ? 'augmented' : '' },
        LRV.rich(LRV.productionRuns(g, i))));
    });
    state.grammarPanelFor = state.steps;
    dom.grammarPanel.hidden = false;
    dom.grammarBtn.setAttribute('aria-expanded', 'true');
    dom.grammarClose.focus();
  }

  function closeGrammarPanel() {
    // Focus inside a panel that is about to be hidden would fall to the page;
    // hand it back to the button that opened it.
    var hadFocus = dom.grammarPanel.contains(document.activeElement);
    state.grammarPanelFor = null;
    dom.grammarPanel.hidden = true;
    dom.grammarBtn.setAttribute('aria-expanded', 'false');
    if (hadFocus && !dom.grammarBtn.disabled) dom.grammarBtn.focus();
  }

  function toggleGrammarPanel() {
    if (state.grammarPanelFor !== null) closeGrammarPanel(); else openGrammarPanel();
  }

  /** Jump to one step, for a view that offers a link into another phase.
   *  Still just an index: nothing is recomputed. */
  LRV.goToStep = function (index) {
    stopPlay();
    goto_(index);
  };

  /** Switch to another method, for a view that offers the comparison as a
   *  control - the "Compare all four" table does.
   *  Unlike goToStep this really does re-run the engine, because it is a
   *  different analysis and not a different point in the same one. */
  LRV.selectVariant = function (id) { selectVariant(id); };

  /** Open a parsing-table cell's state in the Item Sets view: called by
   *  table.js with `{ state, sourceItem, column }` on click/Enter/Space.
   *  Nothing is recomputed - it picks a step index and hands the diagram a
   *  one-shot focus request (`LRV.diagramFocus`). */
  LRV.openStateFromTable = function (info) {
    if (!info || !state.steps.length) return;
    stopPlay();
    var target = stateStepIndex(info.state);
    if (target < 0) return;
    // Order matters. Set the tab and index first, then the focus request, then
    // render exactly once: selectTab() or goto_() could render on their own
    // (selectTab at the phase's first step, before the state exists), and any
    // diagram render consumes LRV.diagramFocus.
    state.tab = 'items';
    state.index = target;
    LRV.diagramFocus = { state: info.state, sourceItem: info.sourceItem || null };
    renderAll();
  };

  /** The step at which state `id` (as the table numbers it) is fully drawn and
   *  joined to the automaton: mergeDone for a merging log, otherwise the later of
   *  its stateClosed and the transitionAdded leading into it. */
  function stateStepIndex(id) {
    var closed = -1;
    var joined = -1;
    for (var i = 0; i < state.steps.length; i++) {
      var s = state.steps[i];
      if (s.kind === 'mergeDone') return i;
      if (closed < 0 && s.kind === 'stateClosed' && s.id === id) closed = i;
      if (joined < 0 && s.kind === 'transitionAdded' && s.to === id) joined = i;
    }
    if (closed >= 0) return Math.max(closed, joined);
    var range = phaseRange('automaton');
    return range ? range.last : -1;
  }

  // ---------------------------------------------------------------- message

  function message(kind, tag, body, hint) {
    return el('div', { class: 'msg ' + kind }, [
      el('span', { class: 'tag', text: tag + ': ' }),
      document.createTextNode(body),
      hint ? el('span', { class: 'hint', text: hint }) : null
    ]);
  }

  function showMessages(result) {
    clear(dom.messages);
    if (!result) return;
    if (result.ok === false) {
      var e = result.error || {};
      var where = e.line ? 'Line ' + e.line : 'Error';
      dom.messages.appendChild(message('error', where, e.message || 'Unknown error', e.hint));
      return;
    }
    (result.warnings || []).forEach(function (w) {
      var kind = w.severity === 'note' ? 'note' : 'warning';
      var tag = w.severity === 'note' ? 'Note' : (w.line ? 'Warning, line ' + w.line : 'Warning');
      dom.messages.appendChild(message(kind, tag, w.message));
    });
  }

  /** After a trap: Analyze is disabled for good, and the message says why and
   *  what to do, with the trap's own text as the detail. */
  function showEngineStopped() {
    var detail = state.engineStopDetail;
    dom.analyze.disabled = true;
    dom.analyze.setAttribute('title', 'The engine has stopped. Reload the page to continue.');
    clear(dom.messages);
    dom.messages.appendChild(message('error', 'Engine',
      'The engine stopped unexpectedly. Reload the page to continue.',
      detail ? 'Details: ' + detail : null));
  }

  /** The parsed symbol list, shown so the user can always see what was read. */
  function showSymbols(result) {
    clear(dom.symbols);
    if (!result || result.ok === false) return;
    var g = result.grammar;
    var list = el('dl', { class: 'symbols' });

    list.appendChild(el('dt', { text: 'Read as' }));
    list.appendChild(el('dd', {
      text: g.mode === 'single-char'
        ? 'single-char mode — every character is a symbol'
        : 'space-separated mode — symbols are split on spaces'
    }));

    list.appendChild(el('dt', { text: 'Non-terminals (' + g.nonterminals.length + ')' }));
    var nts = el('dd');
    g.nonterminals.forEach(function (n) {
      nts.appendChild(el('span', { class: 'chip nt', text: n }));
    });
    list.appendChild(nts);

    list.appendChild(el('dt', { text: 'Terminals (' + g.terminals.length + ')' }));
    var ts = el('dd');
    g.terminals.forEach(function (t) {
      ts.appendChild(el('span', { class: 'chip', text: t }));
    });
    if (!g.terminals.length) ts.appendChild(document.createTextNode('none'));
    list.appendChild(ts);

    dom.symbols.appendChild(list);
  }

  // ----------------------------------------------------------------- driver

  /** Re-run the engine and rewind the player to step 0. The grammar and the
   *  test string are read from the form, so they survive every re-run.
   *  `keepFocus` leaves focus where it is - used when the re-run was triggered
   *  by a control the user is still on, such as the variant radios. */
  function runAnalysis(keepFocus) {
    if (!state.analyze) return;
    stopPlay();
    if (state.engineStopped) { showEngineStopped(); return; }
    var text = dom.grammar.value;
    var test = dom.teststring.value;
    // The box may have been filled without an `input` event (a script, a
    // browser restoring the form), so the menu is checked against it here too.
    syncPreset();

    var raw;
    try {
      raw = state.analyze(text, state.variant, test);
    } catch (e) {
      // The engine is written not to panic and returns every error as JSON,
      // so a throw here means a WebAssembly trap (an abort, or running out of
      // memory). The instance is not safe to call again after that, so the
      // engine is retired for the rest of this page's life.
      if (window.console && typeof window.console.error === 'function') {
        window.console.error('LR visualizer: the engine trapped', e);
      }
      state.engineStopped = true;
      state.result = null;
      state.steps = [];
      state.index = 0;
      showSymbols(null);
      renderAll();
      state.engineStopDetail = String(e);
      showEngineStopped();
      return;
    }

    var parsed;
    try {
      parsed = JSON.parse(raw);
    } catch (e) {
      parsed = { ok: false, error: { message: 'The engine returned something unreadable.' } };
    }

    showMessages(parsed);
    if (parsed.ok === false) {
      state.result = null;
      state.steps = [];
      state.index = 0;
      showSymbols(null);
      renderAll();
      return;
    }

    state.result = parsed;
    state.steps = playableSteps(parsed.steps || []);
    state.index = 0;
    state.tab = 'grammar';
    showSymbols(parsed);
    renderAll();
    if (!keepFocus) dom.view.focus();
  }

  /* The steps the player walks. On a table with conflicts the parse phase is
     cut off (it would prove nothing); state.result keeps the full log. A cut log
     is a new array, so per-steps caches treat it as a new analysis. */
  function playableSteps(steps) {
    var done = LRV.tableDoneFrom(steps);
    if (!done || done.isValid !== false) return steps;
    for (var i = 0; i < steps.length; i++) {
      if (steps[i].phase === 'parse') return steps.slice(0, i);
    }
    return steps;
  }

  /** Switching variant re-runs the analysis over the same grammar and test
   *  string and resets the player to step 0. */
  function selectVariant(id) {
    if (state.variant === id) return;
    if (state.engineStopped) {
      // Nothing can be re-run, so keep the radios saying what was last shown.
      var shown = document.getElementById('variant-' + state.variant);
      if (shown) shown.checked = true;
      showEngineStopped();
      return;
    }
    state.variant = id;
    // Keep the radio group in step when the change came from somewhere else,
    // such as the comparison strip or the compare view.
    var radio = document.getElementById('variant-' + id);
    if (radio) radio.checked = true;
    runAnalysis(true);
  }

  // -------------------------------------------------------------- assembly

  function buildVariants() {
    clear(dom.variants);
    VARIANTS.forEach(function (v) {
      var input = el('input', {
        type: 'radio',
        name: 'variant',
        id: 'variant-' + v.id,
        value: v.id,
        checked: state.variant === v.id,
        on: { change: function () { selectVariant(v.id); } }
      });
      dom.variants.appendChild(el('label', { for: 'variant-' + v.id }, [
        input,
        el('span', { text: v.label })
      ]));
    });
  }

  /* The example menu names an example only while the grammar box still holds its
     text; after an edit it reads "your own grammar". */
  var NO_PRESET = '— choose an example —';
  var CUSTOM_PRESET = '— your own grammar —';

  function syncPreset() {
    var p = (LRV.PRESETS || [])[state.preset];
    var text = dom.grammar.value;
    var same = !!p && p.grammar === text;
    dom.preset.value = same ? String(state.preset) : '';
    if (dom.presetNone) {
      dom.presetNone.textContent = !same && text.trim() ? CUSTOM_PRESET : NO_PRESET;
    }
  }

  function buildPresets() {
    var presets = LRV.PRESETS || [];
    clear(dom.preset);
    dom.presetNone = el('option', { value: '', text: NO_PRESET });
    dom.preset.appendChild(dom.presetNone);

    // One <optgroup> per entry of PRESET_GROUPS, in that order. An option's
    // value stays its index into PRESETS, so the change handler below does not
    // care how the menu is grouped. A preset whose group is not listed lands in
    // a trailing "Other" group rather than silently vanishing from the menu.
    var groups = LRV.PRESET_GROUPS || [];
    var known = {};
    groups.forEach(function (g) { known[g.id] = true; });
    function addGroup(label, members) {
      if (members.length === 0) return;
      dom.preset.appendChild(el('optgroup', { label: label }, members.map(function (m) {
        return el('option', { value: String(m.index), text: m.preset.name });
      })));
    }
    function membersWhere(test) {
      var out = [];
      presets.forEach(function (p, i) { if (test(p)) out.push({ preset: p, index: i }); });
      return out;
    }
    groups.forEach(function (g) {
      addGroup(g.label, membersWhere(function (p) { return p.group === g.id; }));
    });
    addGroup('Other', membersWhere(function (p) { return !known[p.group]; }));
    dom.preset.addEventListener('change', function () {
      // The valueless first entry is not an example: nothing to load.
      var p = dom.preset.value === '' ? null : presets[Number(dom.preset.value)];
      if (!p) { syncPreset(); return; }
      state.preset = Number(dom.preset.value);
      dom.grammar.value = p.grammar;
      dom.teststring.value = p.testString || '';
      syncPreset();
      runAnalysis(false);
    });
    dom.grammar.addEventListener('input', syncPreset);
  }

  function bindKeys() {
    document.addEventListener('keydown', function (ev) {
      var t = ev.target;
      // Escape closes the grammar panel from anywhere, even from its own
      // (button) close control, which the guard below would otherwise skip.
      if (ev.key === 'Escape' && state.grammarPanelFor !== null &&
          !ev.metaKey && !ev.ctrlKey && !ev.altKey) {
        closeGrammarPanel();
        ev.preventDefault();
        return;
      }
      // Never hijack typing - and never hijack a focused control either. Space
      // on a focused button must press that button, not step the player, or
      // tabbing to Reset and pressing space would silently do something else.
      if (t && (t.tagName === 'TEXTAREA' || t.tagName === 'INPUT' ||
                t.tagName === 'SELECT' || t.tagName === 'BUTTON')) return;
      if (ev.metaKey || ev.ctrlKey || ev.altKey) return;
      var handled = true;
      switch (ev.key) {
        case 'ArrowRight': case ' ': case 'Spacebar': step(1); break;
        case 'ArrowLeft': step(-1); break;
        case 'Home': goto_(0); break;
        case 'End': goto_(state.steps.length - 1); break;
        case 'p': case 'P': togglePlay(); break;
        case 'g': case 'G':
          // Only with an analysis to show; the button is disabled without one.
          if (state.steps.length) toggleGrammarPanel();
          break;
        default: handled = false;
      }
      if (handled) ev.preventDefault();
    });
  }

  function collectDom() {
    dom.variants = document.getElementById('variants');
    dom.preset = document.getElementById('preset');
    dom.grammar = document.getElementById('grammar');
    dom.teststring = document.getElementById('teststring');
    dom.analyze = document.getElementById('analyze');
    dom.messages = document.getElementById('messages');
    dom.symbols = document.getElementById('symbols');
    dom.tabs = document.getElementById('tabs');
    dom.view = document.getElementById('view');
    dom.scrub = document.getElementById('scrub');
    dom.explain = document.getElementById('explain');
    dom.explainPhase = document.getElementById('explain-phase');
    dom.explainSummary = document.getElementById('explain-summary');
    dom.explainToggle = document.getElementById('explain-toggle');
    dom.explainFull = document.getElementById('explain-full');
    dom.first = document.getElementById('btn-first');
    dom.prev = document.getElementById('btn-prev');
    dom.next = document.getElementById('btn-next');
    dom.last = document.getElementById('btn-last');
    dom.play = document.getElementById('btn-play');
    dom.reset = document.getElementById('btn-reset');
    dom.speed = document.getElementById('speed-input');
    dom.grammarBtn = document.getElementById('btn-grammar');
    dom.grammarPanel = document.getElementById('grammar-panel');
    dom.grammarClose = document.getElementById('grammar-panel-close');
    dom.grammarList = document.getElementById('grammar-panel-list');
  }

  /** Called by the loader once the WASM engine is ready. By then the engine
   *  has loaded, so nothing thrown from here on may reach the loader's catch,
   *  which would mislabel a UI bug as "the engine could not be loaded". */
  LRV.boot = function (analyze) {
    try {
      bootUi(analyze);
    } catch (e) {
      reportRenderError(e);
    }
  };

  function bootUi(analyze) {
    state.analyze = analyze;
    collectDom();
    buildTabs();
    buildVariants();
    buildPresets();

    dom.explainToggle.addEventListener('click', toggleFullExplain);
    // The speed menu's selected option is the one source of the default.
    state.speed = Number(dom.speed.value) || state.speed;

    // Wrapped, so the click event is never passed in as `keepFocus`.
    dom.analyze.addEventListener('click', function () { runAnalysis(false); });
    dom.first.addEventListener('click', function () { stopPlay(); goto_(0); });
    dom.prev.addEventListener('click', function () { stopPlay(); step(-1); });
    dom.next.addEventListener('click', function () { stopPlay(); step(1); });
    dom.last.addEventListener('click', function () { stopPlay(); goto_(state.steps.length - 1); });
    dom.play.addEventListener('click', togglePlay);
    dom.grammarBtn.addEventListener('click', toggleGrammarPanel);
    dom.grammarClose.addEventListener('click', closeGrammarPanel);
    dom.reset.addEventListener('click', function () { stopPlay(); goto_(0); });
    dom.scrub.addEventListener('input', function () {
      // Read the slider BEFORE stopping: stopPlay() re-renders the player,
      // which snaps the slider back to the current index, and we would then
      // read that instead of where the user just dragged to.
      var target = Number(dom.scrub.value);
      stopPlay();
      // The index, the slider's label and the explanation move now; the stage is
      // drawn once in the next animation frame (see renderStageNextFrame).
      scrubTo(target);
    });
    dom.speed.addEventListener('change', function () {
      state.speed = Number(dom.speed.value) || 2500;
      if (state.playing) { stopPlay(); startPlay(); }
    });
    bindKeys();

    // Open on the classic expression grammar so the tool is useful immediately.
    var presets = LRV.PRESETS || [];
    if (presets.length) {
      state.preset = 0;
      dom.grammar.value = presets[0].grammar;
      dom.teststring.value = presets[0].testString || '';
      syncPreset();
    }
    renderAll();
    // `true`: focus stays where the browser put it. Moving it to the stage is
    // right after the user presses Analyze - that is where the result is - but
    // on a page that has only just loaded nobody has asked for anything, and
    // a browser draws a script's first focus with the keyboard ring: the whole
    // stage came up outlined. Tab still reaches the stage, and rings it.
    runAnalysis(true);
  }

  /** Called by the loader if the engine could not start at all. */
  LRV.bootFailed = function (why) {
    var box = document.getElementById('messages');
    if (!box) return;
    clear(box);
    box.appendChild(message('error', 'Engine',
      'The analysis engine could not be loaded.', why));
  };

  LRV.views = LRV.views || {};

  // --------------------------------------------------------- grammar view

  LRV.views.grammar = function (steps, index, container) {
    var seen = LRV.upto(steps, index);
    var g = LRV.grammarFrom(seen);
    if (!g) {
      container.appendChild(el('p', { class: 'empty-state', text: 'Waiting for the grammar to be read.' }));
      return;
    }
    var augmented = seen.some(function (s) { return s.kind === 'augmented'; });

    container.appendChild(el('h2', { class: 'view-title', text: 'Productions' }));
    var list = el('ul', { class: 'prod-list' });
    g.productions.forEach(function (p) {
      if (p.index === 0 && !augmented) return;
      list.appendChild(el('li', { class: p.index === 0 ? 'augmented' : '' },
        [el('span', { class: 'idx', text: p.index + '.' })]
          .concat(LRV.rich(LRV.productionRuns(g, p.index)))));
    });
    container.appendChild(list);

    container.appendChild(el('h2', { class: 'view-title', text: 'Symbols as understood' }));
    var dl = el('dl', { class: 'symbols' });
    dl.appendChild(el('dt', { text: 'Tokenization mode' }));
    dl.appendChild(el('dd', { text: g.mode }));
    dl.appendChild(el('dt', { text: 'Start symbol' }));
    dl.appendChild(el('dd', {}, LRV.rich([LRV.sym(g.start), augmented ? ' (augmented)' : ''])));
    dl.appendChild(el('dt', { text: 'Non-terminals' }));
    var a = el('dd');
    g.nonterminals.forEach(function (n) { a.appendChild(el('span', { class: 'chip nt', text: n })); });
    dl.appendChild(a);
    dl.appendChild(el('dt', { text: 'Terminals' }));
    var b = el('dd');
    g.terminals.forEach(function (t) { b.appendChild(el('span', { class: 'chip', text: t })); });
    dl.appendChild(b);
    container.appendChild(dl);
  };

  try {
    setupChrome();
  } catch (e) {
    reportRenderError(e);
  }

})(window.LRV);

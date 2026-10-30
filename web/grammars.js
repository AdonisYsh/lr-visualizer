/* Preset grammar library: pure data. */
window.LRV = window.LRV || {};

window.LRV.PRESETS = [
  {
    name: 'Expression grammar (classic)',
    group: 'slr1',
    grammar: 'E -> E + T | T\nT -> T * F | F\nF -> ( E ) | id',
    testString: 'id + id * id'
  },
  {
    name: 'Balanced pairs',
    group: 'lr0',
    grammar: 'S -> a S b | c',
    testString: 'a a c b b'
  },
  {
    name: 'Two halves',
    group: 'lr0',
    grammar: 'S -> C C\nC -> c C | d',
    testString: 'c d d'
  },
  {
    name: 'Epsilon',
    group: 'slr1',
    grammar: 'S -> A B\nA -> a | ε\nB -> b',
    testString: 'b'
  },
  {
    name: 'Dangling else',
    group: 'none',
    grammar: 'S -> i S e S | i S | a',
    testString: 'i i a e a'
  },
  {
    name: 'Ambiguous expression',
    group: 'none',
    grammar: 'E -> E + E | E * E | id',
    testString: 'id + id * id'
  },
  {
    name: 'Reduce-reduce',
    group: 'none',
    grammar: 'S -> A | B\nA -> a\nB -> a',
    testString: 'a'
  },
  {
    name: 'Pointers and assignment',
    group: 'lalr1',
    grammar: 'S -> L = R | R\nL -> * R | id\nR -> L',
    testString: 'id = id'
  },
  {
    name: 'Merge casualty',
    group: 'clr1',
    grammar: 'S -> a A d | b B d | a B e | b A e\nA -> c\nB -> c',
    testString: 'a c d'
  },
  {
    name: 'Named tokens',
    group: 'lr0',
    grammar: 'stmt -> if expr then stmt | id = expr\nexpr -> id | num',
    testString: 'if id then id = num'
  },
  {
    name: 'Nested parentheses',
    group: 'lr0',
    grammar: 'S -> ( S ) | a',
    testString: '( ( a ) )'
  },
  {
    name: 'Optional pairs (ε)',
    group: 'slr1',
    grammar: 'S -> a S b | ε',
    testString: 'a a b b'
  },
  {
    name: 'Two empty non-terminals (ε)',
    group: 'lalr1',
    grammar: 'S -> A a A b | B b B a\nA -> ε\nB -> ε',
    testString: 'a b'
  }
];

/* Example menu groups, by the weakest method that handles each grammar (author-supplied). */
window.LRV.PRESET_GROUPS = [
  { id: 'lr0',   label: 'Works from LR(0)' },
  { id: 'slr1',  label: 'Needs SLR(1)' },
  { id: 'lalr1', label: 'Needs LALR(1)' },
  { id: 'clr1',  label: 'Needs CLR(1)' },
  { id: 'none',  label: 'Conflicts in every method' }
];

//! Textbook facts about bottom-up parsing, one test per fact.
//!
//! Citations are to Aho, Lam, Sethi and Ullman (the dragon book, 2nd edition).

use serde_json::Value;

// --- grammars ----------------------------------------------------------------

const EXPRESSION: &str = "E -> E + T | T\nT -> T * F | F\nF -> ( E ) | id";
const TWO_HALVES: &str = "S -> C C\nC -> c C | d";
const POINTERS: &str = "S -> L = R | R\nL -> * R | id\nR -> L";
const MERGE_CASUALTY: &str = "S -> a A d | b B d | a B e | b A e\nA -> c\nB -> c";
const AMBIGUOUS_EXPR: &str = "E -> E + E | E * E | id";
const NESTED_PARENS: &str = "S -> ( S ) | a";
// `ε`, `#`, `eps`, `epsilon` and `ϵ` are all epsilon spellings.
const EPSILON_G: &str = "S -> A B\nA -> a | ε\nB -> b";
const OPTIONAL_PAIRS: &str = "S -> a S b | ε";
const TWO_EMPTY: &str = "S -> A a A b | B b B a\nA -> ε\nB -> ε";

const VARIANTS: [&str; 4] = ["lr0", "slr1", "lalr1", "clr1"];

// --- helpers -----------------------------------------------------------------

/// Runs one analysis and insists it succeeded; the grammars here are all valid
/// input, so an error result is itself a failure.
fn run(variant: &str, grammar: &str, test: &str) -> Value {
    let json = lr_engine::analyze(grammar, variant, test);
    let v: Value = serde_json::from_str(&json).expect("analyze must return valid JSON");
    assert_eq!(
        v["ok"],
        Value::Bool(true),
        "expected a successful analysis under {variant}, got: {}",
        v["error"]
    );
    v
}

/// The number of conflicted ACTION cells in the finished table.
fn conflict_count(v: &Value) -> usize {
    v["table"]["conflicts"].as_array().map_or(usize::MAX, Vec::len)
}

/// The number of states in the automaton the table was built from.
fn state_count(v: &Value) -> u64 {
    v["table"]["states"].as_u64().expect("table.states is a number")
}

/// The engine's own verdict on whether this method handles the grammar.
fn is_valid(v: &Value) -> bool {
    v["verdict"]["isValid"].as_bool().expect("verdict.isValid is a bool")
}

/// Every conflict in the finished table, as (state, symbol, kind, mergeInduced).
fn conflicts(v: &Value) -> Vec<(u64, String, String, bool)> {
    v["table"]["conflicts"]
        .as_array()
        .expect("table.conflicts is an array")
        .iter()
        .map(|c| {
            (
                c["state"].as_u64().expect("conflict.state"),
                c["symbol"].as_str().expect("conflict.symbol").to_string(),
                c["kind"].as_str().expect("conflict.kind").to_string(),
                c["mergeInduced"].as_bool().expect("conflict.mergeInduced"),
            )
        })
        .collect()
}

fn steps_of_kind<'a>(v: &'a Value, kind: &str) -> Vec<&'a Value> {
    v["steps"]
        .as_array()
        .expect("steps is an array")
        .iter()
        .filter(|s| s["kind"] == kind)
        .collect()
}

/// Asserts a method's conflict count, state count and verdict in one go.
fn assert_method(grammar: &str, variant: &str, conflicts: usize, states: u64) {
    let v = run(variant, grammar, "");
    assert_eq!(conflict_count(&v), conflicts, "{variant}: conflicted cells");
    assert_eq!(state_count(&v), states, "{variant}: states");
    assert_eq!(is_valid(&v), conflicts == 0, "{variant}: verdict.isValid");
}

// --- the expression grammar --------------------------------------------------

// Dragon book Fig. 4.31: the LR(0) automaton has I0..I11.
#[test]
fn expression_grammar_has_twelve_lr0_states() {
    let v = run("lr0", EXPRESSION, "");
    assert_eq!(state_count(&v), 12);
}

// Dragon book Fig. 4.31: I2 and I9 each pair a reduce with a shift on `*`.
#[test]
fn expression_grammar_is_not_lr0() {
    let v = run("lr0", EXPRESSION, "");
    assert!(conflict_count(&v) > 0, "LR(0) must have at least one conflict");
    assert!(!is_valid(&v));
    assert_eq!(conflict_count(&v), 2, "I2 and I9, both on `*`");
}

// Dragon book Fig. 4.37: the SLR(1) table has no conflicts.
#[test]
fn expression_grammar_is_slr1() {
    let v = run("slr1", EXPRESSION, "");
    assert_eq!(conflict_count(&v), 0);
    assert!(is_valid(&v));
    assert_eq!(state_count(&v), 12, "SLR(1) uses the LR(0) automaton");
}

// id + id * id is a sentence of the grammar (dragon book Fig. 4.38).
#[test]
fn slr1_accepts_id_plus_id_times_id() {
    let v = run("slr1", EXPRESSION, "id + id * id");
    assert_eq!(steps_of_kind(&v, "parseAccept").len(), 1, "exactly one accept");
    assert!(steps_of_kind(&v, "parseReject").is_empty(), "and no reject");
}

// An LR parser rejects as soon as no ACTION entry exists: here, at `*`.
#[test]
fn slr1_rejects_id_plus_times_id() {
    let v = run("slr1", EXPRESSION, "id + * id");
    assert!(steps_of_kind(&v, "parseAccept").is_empty(), "must not accept");
    let rejects = steps_of_kind(&v, "parseReject");
    assert_eq!(rejects.len(), 1, "exactly one reject");
    assert_eq!(rejects[0]["lookahead"], "*", "the error is detected at `*`");
}

// --- S -> C C, C -> c C | d --------------------------------------------------

// Dragon book Fig. 4.41: the canonical LR(1) collection has I0..I9.
#[test]
fn two_halves_has_ten_clr1_states() {
    let v = run("clr1", TWO_HALVES, "");
    assert_eq!(state_count(&v), 10);
    assert_eq!(conflict_count(&v), 0);
}

// Dragon book Fig. 4.43: merging leaves seven LALR(1) states.
#[test]
fn two_halves_has_seven_lalr1_states() {
    let v = run("lalr1", TWO_HALVES, "");
    assert_eq!(state_count(&v), 7);
}

// Dragon book Fig. 4.43: the LALR(1) table has no conflicts.
#[test]
fn two_halves_is_lalr1() {
    let v = run("lalr1", TWO_HALVES, "");
    assert_eq!(conflict_count(&v), 0);
    assert!(is_valid(&v));
}

// --- S -> L = R | R, L -> * R | id, R -> L ------------------------------------

// Dragon book Example 4.48: `=` is in FOLLOW(R), so I2 has a shift-reduce conflict on it.
#[test]
fn pointers_grammar_is_not_slr1_with_a_shift_reduce_conflict_on_equals() {
    let v = run("slr1", POINTERS, "");
    assert!(!is_valid(&v));
    let found = conflicts(&v);
    assert!(
        found.iter().any(|(_, sym, kind, _)| sym == "=" && kind == "shift-reduce"),
        "expected a shift-reduce conflict on `=`, got {found:?}"
    );
}

// Dragon book Example 4.48: grammar (4.49) is LALR(1).
#[test]
fn pointers_grammar_is_lalr1() {
    let v = run("lalr1", POINTERS, "");
    assert_eq!(conflict_count(&v), 0);
    assert!(is_valid(&v));
    assert_eq!(state_count(&v), 10, "LALR(1) keeps the LR(0) state count");
}

// Every LALR(1) grammar is LR(1), and this one is canonical LR(1) too.
#[test]
fn pointers_grammar_is_clr1() {
    let v = run("clr1", POINTERS, "");
    assert_eq!(conflict_count(&v), 0);
    assert!(is_valid(&v));
}

// --- the merge casualty ------------------------------------------------------

// Dragon book Example 4.58: the canonical LR(1) table has no conflicts.
#[test]
fn merge_casualty_is_clr1() {
    let v = run("clr1", MERGE_CASUALTY, "");
    assert_eq!(conflict_count(&v), 0);
    assert!(is_valid(&v));
}

// Dragon book Example 4.58: merging the two halves yields reduce-reduce conflicts.
#[test]
fn merge_casualty_is_not_lalr1_because_merging_causes_a_reduce_reduce_conflict() {
    let v = run("lalr1", MERGE_CASUALTY, "");
    assert!(!is_valid(&v));

    let found = conflicts(&v);
    assert!(!found.is_empty(), "LALR(1) must conflict on this grammar");
    for (state, sym, kind, merge_induced) in &found {
        assert_eq!(kind, "reduce-reduce", "conflict in I{state} on {sym}");
        assert!(merge_induced, "conflict in I{state} on {sym} must be flagged merge-induced");
    }

    // The same facts, read from the step log rather than the table.
    let steps = steps_of_kind(&v, "tableConflict");
    assert!(!steps.is_empty());
    for s in steps {
        assert_eq!(s["conflictKind"], "reduce-reduce");
        assert_eq!(s["mergeInduced"], true);
    }
}

// Shifts depend only on the core, so merging adds no shift-reduce conflict (dragon book 4.7.4).
#[test]
fn merge_casualty_merge_introduces_no_shift_reduce_conflict() {
    let v = run("lalr1", MERGE_CASUALTY, "");
    assert!(conflicts(&v).iter().all(|(_, _, kind, _)| kind != "shift-reduce"));
}

// --- an ambiguous grammar ----------------------------------------------------

// Dragon book 4.8.1: an ambiguous grammar is not LR(k) for any method.
#[test]
fn ambiguous_expression_grammar_fails_all_four_methods() {
    for variant in VARIANTS {
        let v = run(variant, AMBIGUOUS_EXPR, "");
        assert!(conflict_count(&v) > 0, "{variant} must have conflicts");
        assert!(!is_valid(&v), "{variant} must not be valid");
    }
    // The comparison row agrees: no method is suggested.
    let v = run("lr0", AMBIGUOUS_EXPR, "");
    let rows = v["verdict"]["comparison"].as_array().expect("comparison rows");
    assert_eq!(rows.len(), 4, "all four methods were run");
    for row in rows {
        assert_eq!(row["isValid"], false, "{} must not be valid", row["label"]);
    }
}

// --- an LR(0) grammar --------------------------------------------------------

// A completed item stands alone in every state, so there is no conflict.
#[test]
fn nested_parentheses_is_lr0_with_six_states() {
    let v = run("lr0", NESTED_PARENS, "");
    assert_eq!(conflict_count(&v), 0);
    assert!(is_valid(&v));
    assert_eq!(state_count(&v), 6);
}

// --- ε-productions -----------------------------------------------------------

// LR(0) reduces an ε-production on every column; FOLLOW(A) = { b } fixes it.
#[test]
fn epsilon_grammar_is_not_lr0_but_is_slr1() {
    assert_method(EPSILON_G, "lr0", 1, 6);
    assert_method(EPSILON_G, "slr1", 0, 6);
}

// LR(0) reduces S -> ε against the shift of `a`; FOLLOW(S) = { b, $ } separates them.
#[test]
fn optional_pairs_is_not_lr0_but_is_slr1() {
    assert_method(OPTIONAL_PAIRS, "lr0", 2, 5);
    assert_method(OPTIONAL_PAIRS, "slr1", 0, 5);
}

// Equal FOLLOW sets defeat SLR(1); the LR(1) lookaheads differ, so LALR(1) and CLR(1) succeed.
#[test]
fn two_empty_nonterminals_need_lalr1() {
    assert_method(TWO_EMPTY, "lr0", 3, 10);
    assert_method(TWO_EMPTY, "slr1", 2, 10);
    assert_method(TWO_EMPTY, "lalr1", 0, 10);
    assert_method(TWO_EMPTY, "clr1", 0, 10);
}

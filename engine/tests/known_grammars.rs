//! Preset-by-preset checks of the engine for all four methods. Numbers come
//! from the dragon book; where the code disagrees, the code is wrong.
//! SLR(1) is checked for contrast with LR(0), not only for its own result.

use serde_json::Value;

// --- the ten preset grammars, kept in step with web/grammars.js -------------

const EXPRESSION: &str = "E -> E + T | T\nT -> T * F | F\nF -> ( E ) | id";
const BALANCED: &str = "S -> a S b | c";
const TWO_HALVES: &str = "S -> C C\nC -> c C | d";
const EPSILON_G: &str = "S -> A B\nA -> a | ε\nB -> b";
const DANGLING_ELSE: &str = "S -> i S e S | i S | a";
const AMBIGUOUS_EXPR: &str = "E -> E + E | E * E | id";
const REDUCE_REDUCE: &str = "S -> A | B\nA -> a\nB -> a";
const POINTERS: &str = "S -> L = R | R\nL -> * R | id\nR -> L";
const MERGE_CASUALTY: &str = "S -> a A d | b B d | a B e | b A e\nA -> c\nB -> c";
const NAMED_TOKENS: &str = "stmt -> if expr then stmt | id = expr\nexpr -> id | num";

const PRESETS: [(&str, &str, &str); 10] = [
    ("Expression grammar", EXPRESSION, "id + id * id"),
    ("Balanced pairs", BALANCED, "a a c b b"),
    ("Two halves", TWO_HALVES, "c d d"),
    ("Epsilon", EPSILON_G, "b"),
    ("Dangling else", DANGLING_ELSE, "i i a e a"),
    ("Ambiguous expression", AMBIGUOUS_EXPR, "id + id * id"),
    ("Reduce-reduce", REDUCE_REDUCE, "a"),
    ("Pointers and assignment", POINTERS, "id = id"),
    ("Merge casualty", MERGE_CASUALTY, "a c d"),
    ("Named tokens", NAMED_TOKENS, "if id then id = num"),
];

// --- helpers ---------------------------------------------------------------

fn analyze_with(variant: &str, grammar: &str, test: &str) -> Value {
    let json = lr_engine::analyze(grammar, variant, test);
    serde_json::from_str(&json).expect("analyze must always return valid JSON")
}

fn analyze(grammar: &str, test: &str) -> Value {
    analyze_with("lr0", grammar, test)
}

fn ok_with(variant: &str, grammar: &str, test: &str) -> Value {
    let v = analyze_with(variant, grammar, test);
    assert_eq!(
        v["ok"], Value::Bool(true),
        "expected success under {variant}, got error: {}", v["error"]
    );
    v
}

fn ok(grammar: &str, test: &str) -> Value {
    ok_with("lr0", grammar, test)
}

/// Every conflict in the finished table, as (state, symbol, kind).
fn conflicts(v: &Value) -> Vec<(u64, String, String)> {
    v["table"]["conflicts"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|c| {
            (
                c["state"].as_u64().unwrap_or(u64::MAX),
                c["symbol"].as_str().unwrap_or("?").to_string(),
                c["kind"].as_str().unwrap_or("?").to_string(),
            )
        })
        .collect()
}

/// One ACTION cell in the conventional short form: `["s7"]`, `["s6", "r5"]`.
fn action_cell(v: &Value, state: usize, symbol: &str) -> Vec<String> {
    v["table"]["action"][state.to_string()][symbol]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|a| match a["type"].as_str().unwrap_or("") {
            "shift" => format!("s{}", a["state"]),
            "reduce" => format!("r{}", a["production"]),
            "accept" => "acc".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// The steps of one phase, as JSON text, for comparing two runs byte for byte.
fn phase_json(v: &Value, phase: &str) -> String {
    let only: Vec<&Value> = steps(v).iter().filter(|s| s["phase"] == phase).collect();
    serde_json::to_string(&only).expect("steps re-serialize")
}

fn err(grammar: &str, test: &str) -> Value {
    let v = analyze(grammar, test);
    assert_eq!(v["ok"], Value::Bool(false), "expected an error, got success");
    v
}

fn steps(v: &Value) -> &Vec<Value> {
    v["steps"].as_array().expect("steps must be an array")
}

fn steps_of_kind<'a>(v: &'a Value, kind: &str) -> Vec<&'a Value> {
    steps(v).iter().filter(|s| s["kind"] == kind).collect()
}

/// FIRST or FOLLOW as (nonterminal, set) pairs, from the phase's Done step.
fn set_table(v: &Value, kind: &str, nt: &str) -> Vec<String> {
    let done = steps_of_kind(v, kind);
    let last = done.last().unwrap_or_else(|| panic!("no {kind} step"));
    last["table"]
        .as_array()
        .unwrap_or_else(|| panic!("{kind} has no table"))
        .iter()
        .find(|row| row[0] == nt)
        .unwrap_or_else(|| panic!("{kind} has no row for {nt}"))[1]
        .as_array()
        .unwrap_or_else(|| panic!("{kind} row for {nt} is not an array"))
        .iter()
        .filter_map(|s| s.as_str().map(str::to_string))
        .collect()
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

fn set_of(items: &[&str]) -> Vec<String> {
    sorted(items.iter().map(|s| (*s).to_string()).collect())
}

fn warnings(v: &Value) -> Vec<String> {
    v["warnings"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|w| w["message"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

// ===========================================================================
// The expression grammar: E -> E + T | T / T -> T * F | F / F -> ( E ) | id
// ===========================================================================

#[test]
fn expression_grammar_has_twelve_states() {
    let v = ok(EXPRESSION, "");
    let done = steps_of_kind(&v, "automatonDone");
    assert_eq!(done.len(), 1);
    assert_eq!(
        done[0]["stateCount"], 12,
        "the augmented LR(0) canonical collection has states I0 to I11"
    );
    assert_eq!(v["table"]["states"], 12);
}

#[test]
fn expression_grammar_first_sets() {
    let v = ok(EXPRESSION, "");
    let expected = set_of(&["(", "id"]);
    for nt in ["E", "T", "F"] {
        assert_eq!(sorted(set_table(&v, "firstDone", nt)), expected, "FIRST({nt})");
    }
}

#[test]
fn expression_grammar_follow_sets() {
    let v = ok(EXPRESSION, "");
    assert_eq!(
        sorted(set_table(&v, "followDone", "E")),
        set_of(&["+", ")", "$"]),
        "FOLLOW(E)"
    );
    for nt in ["T", "F"] {
        assert_eq!(
            sorted(set_table(&v, "followDone", nt)),
            set_of(&["+", "*", ")", "$"]),
            "FOLLOW({nt})"
        );
    }
}

/// Textbooks number states by the order GOTO is taken over symbols as they
/// first appear in the source. A lecturer compares these numbers directly, so
/// an identical automaton numbered differently still reads as wrong.
#[test]
fn expression_grammar_states_come_out_in_textbook_order() {
    let v = ok(EXPRESSION, "");
    let created = steps_of_kind(&v, "stateCreated");
    let order: Vec<(u64, Option<u64>, Option<&str>)> = created
        .iter()
        .map(|s| {
            (
                s["id"].as_u64().unwrap_or(u64::MAX),
                s["fromState"].as_u64(),
                s["onSymbol"].as_str(),
            )
        })
        .collect();

    assert_eq!(order.first(), Some(&(0, None, None)), "I0 is closure(E' -> . E)");
    assert_eq!(order.get(1), Some(&(1, Some(0), Some("E"))));
    assert_eq!(order.get(2), Some(&(2, Some(0), Some("T"))));
    assert_eq!(order.get(3), Some(&(3, Some(0), Some("F"))));
    assert_eq!(order.get(4), Some(&(4, Some(0), Some("("))));
    assert_eq!(order.get(5), Some(&(5, Some(0), Some("id"))));
    // The remainder of the dragon-book collection, for completeness.
    assert_eq!(order.get(6), Some(&(6, Some(1), Some("+"))));
    assert_eq!(order.get(7), Some(&(7, Some(2), Some("*"))));
    assert_eq!(order.get(8), Some(&(8, Some(4), Some("E"))));
    assert_eq!(order.get(9), Some(&(9, Some(6), Some("T"))));
    assert_eq!(order.get(10), Some(&(10, Some(7), Some("F"))));
    assert_eq!(order.get(11), Some(&(11, Some(8), Some(")"))));
}

#[test]
fn expression_grammar_is_not_lr0() {
    let v = ok(EXPRESSION, "");
    let conflicts = v["table"]["conflicts"].as_array().cloned().unwrap_or_default();
    assert!(!conflicts.is_empty(), "LR(0) must conflict on this grammar");
    // Every state holding a completed item alongside a shift collides, because
    // LR(0) writes the reduction into every column including the shift's.
    for c in &conflicts {
        assert_eq!(c["kind"], "shift-reduce", "conflict {c} should be shift-reduce");
    }
    assert_eq!(v["verdict"]["isValid"], Value::Bool(false));
    assert_eq!(v["verdict"]["class"], "not LR(0)");
}

#[test]
fn expression_grammar_parses_its_test_string() {
    let v = ok(EXPRESSION, "id + id * id");
    assert_eq!(steps_of_kind(&v, "parseAccept").len(), 1);
    assert_eq!(steps_of_kind(&v, "parseReject").len(), 0);
}

// ===========================================================================
// The other presets, against their known behaviour
// ===========================================================================

#[test]
fn balanced_pairs_is_conflict_free_under_lr0() {
    let v = ok(BALANCED, "a a c b b");
    assert_eq!(
        v["table"]["conflicts"].as_array().map(Vec::len),
        Some(0),
        "S -> a S b | c is LR(0)"
    );
    assert_eq!(v["verdict"]["isValid"], Value::Bool(true));
    assert_eq!(v["verdict"]["class"], "LR(0)");
    assert_eq!(steps_of_kind(&v, "parseAccept").len(), 1);
}

/// Smallest reduce-reduce conflict: I4 = {A -> a •, B -> a •}. One colliding
/// item pair but two conflicted cells (`a` and `$`): LR(0) writes the completed
/// item into every column. SLR(1) leaves only `$`.
#[test]
fn reduce_reduce_grammar_conflicts_only_reduce_reduce_in_one_state() {
    let v = ok(REDUCE_REDUCE, "a");
    let conflicts = v["table"]["conflicts"].as_array().cloned().unwrap_or_default();
    assert!(!conflicts.is_empty(), "this grammar must conflict");
    for c in &conflicts {
        assert_eq!(c["kind"], "reduce-reduce", "no shift can be involved: {c}");
    }

    let states: Vec<&Value> = {
        let mut s: Vec<&Value> = conflicts.iter().map(|c| &c["state"]).collect();
        s.dedup();
        s
    };
    assert_eq!(states.len(), 1, "all of it happens in the single state holding both items");

    let columns: Vec<String> = conflicts
        .iter()
        .filter_map(|c| c["symbol"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        sorted(columns),
        set_of(&["a", "$"]),
        "the blanket LR(0) reduce rule collides in every column, not just $"
    );
}

#[test]
fn dangling_else_conflicts_on_e() {
    let v = ok(DANGLING_ELSE, "i i a e a");
    let conflicts = v["table"]["conflicts"].as_array().cloned().unwrap_or_default();
    let on_e: Vec<&Value> = conflicts
        .iter()
        .filter(|c| c["symbol"] == "e" && c["kind"] == "shift-reduce")
        .collect();
    assert!(
        !on_e.is_empty(),
        "the shift-reduce conflict on `e` is the whole point of this grammar; got {conflicts:?}"
    );
}

#[test]
fn every_preset_analyses_cleanly_with_explanations_on_every_step() {
    for (name, grammar, test) in PRESETS {
        let v = ok(grammar, test);
        let all = steps(&v);
        assert!(!all.is_empty(), "{name}: no steps emitted");
        assert!(
            v["table"]["states"].as_u64().unwrap_or(0) <= lr_engine::limits::MAX_STATES as u64,
            "{name}: state count over the limit"
        );
        for s in all {
            let explain = s["explain"].as_str().unwrap_or("");
            assert!(
                !explain.trim().is_empty(),
                "{name}: step {} ({}) has no explanation",
                s["index"],
                s["kind"]
            );
            assert!(s["phase"].is_string(), "{name}: step missing phase");
            assert!(s["index"].is_u64(), "{name}: step missing index");
        }
    }
}

#[test]
fn named_tokens_grammar_uses_space_separated_mode() {
    let v = ok(NAMED_TOKENS, "if id then id = num");
    assert_eq!(v["grammar"]["mode"], "space-separated");
    assert_eq!(v["grammar"]["originalStart"], "stmt");
    // "uppercase means non-terminal" would get every one of these backwards.
    let nts = v["grammar"]["nonterminals"].as_array().cloned().unwrap_or_default();
    assert!(nts.iter().any(|n| n == "stmt"));
    assert!(nts.iter().any(|n| n == "expr"));
}

#[test]
fn single_character_mode_is_chosen_when_nothing_is_longer_than_one_char() {
    let v = ok(BALANCED, "");
    assert_eq!(v["grammar"]["mode"], "single-char");
    // ...and the chosen mode is stated in the very first step.
    let parsed = steps_of_kind(&v, "grammarParsed");
    let explain = parsed[0]["explain"].as_str().unwrap_or("");
    assert!(explain.contains("single-character mode"), "got: {explain}");
}

// ===========================================================================
// Verdict wording
// ===========================================================================

/// A conflict proves only that *this method* cannot parse the grammar. Claiming
/// ambiguity from a conflict is the single most common error in tools of this
/// kind, and a lecturer will catch it immediately.
#[test]
fn conflicted_verdict_never_claims_ambiguity() {
    for (variant, label) in [("lr0", "LR(0)"), ("slr1", "SLR(1)"), ("clr1", "CLR(1)")] {
        for (name, grammar, _) in PRESETS {
            let v = ok_with(variant, grammar, "");
            if v["verdict"]["isValid"] == Value::Bool(true) {
                continue;
            }
            let message = v["verdict"]["message"].as_str().unwrap_or("");
            assert!(
                message.contains(&format!("not {label}")),
                "{name} under {variant}: verdict was `{message}`"
            );
            assert!(
                !message.to_lowercase().contains("ambiguous"),
                "{name} under {variant}: the verdict must not call the grammar ambiguous: `{message}`"
            );
        }
    }
}

#[test]
fn nothing_anywhere_in_the_output_asserts_ambiguity() {
    for variant in ["lr0", "slr1", "clr1"] {
        for (name, grammar, test) in PRESETS {
            let json = lr_engine::analyze(grammar, variant, test).to_lowercase();
            for claim in [
                "this grammar is ambiguous",
                "the grammar is ambiguous",
                "grammar is ambiguous",
            ] {
                assert!(!json.contains(claim), "{name} under {variant}: output contains `{claim}`");
            }
        }
    }
}

// ===========================================================================
// Grammar sanity checks
// ===========================================================================

#[test]
fn glued_tokens_in_space_separated_mode_are_flagged() {
    // `(E)` reads as one symbol here. Without the warning the tool would
    // silently analyse a grammar the user never wrote.
    let v = ok("E -> E + T | T\nT -> T * F | F\nF -> (E) | id", "");
    let found = warnings(&v)
        .into_iter()
        .find(|w| w.contains("(E)"))
        .unwrap_or_default();
    assert!(
        found.contains("was read as one symbol") && found.contains("( E )"),
        "expected a glued-token warning naming the fix, got: {:?}",
        warnings(&v)
    );
}

#[test]
fn unknown_test_string_symbol_names_the_real_terminals() {
    let e = err(EXPRESSION, "id + foo");
    let message = e["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("`foo` is not a terminal"), "got: {message}");
    for terminal in ["+", "*", "(", ")", "id"] {
        assert!(message.contains(terminal), "terminal {terminal} missing from: {message}");
    }
}

#[test]
fn a_cycle_is_an_error_not_a_hang() {
    let e = err("A -> B\nB -> A", "");
    let message = e["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("cycle"), "got: {message}");
}

#[test]
fn duplicate_productions_are_deduplicated_with_a_warning() {
    let v = ok("S -> a S b | c | c", "c");
    let count = v["grammar"]["productions"]
        .as_array()
        .map(Vec::len)
        .unwrap_or(0);
    // S' -> S, S -> a S b, S -> c. The repeat is gone.
    assert_eq!(count, 3, "the duplicate should have been merged");
    assert!(
        warnings(&v).iter().any(|w| w.contains("Duplicate production")),
        "got: {:?}",
        warnings(&v)
    );
}

#[test]
fn left_recursion_gets_a_reassuring_note_and_analyses_normally() {
    let v = ok(EXPRESSION, "id + id");
    let note = warnings(&v)
        .into_iter()
        .find(|w| w.contains("left-recursive"))
        .unwrap_or_default();
    assert!(note.contains("fine for LR parsers"), "got: {note}");
    assert!(note.contains("LL"), "the note should say why students think it is an error");
    assert_eq!(v["table"]["states"], 12, "and the analysis proceeds as normal");
}

#[test]
fn non_productive_nonterminals_are_an_error() {
    let e = err("S -> A\nA -> A a", "");
    let message = e["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("never derive"), "got: {message}");
}

#[test]
fn unreachable_nonterminals_are_a_warning_not_an_error() {
    let v = ok("S -> a\nZ -> b", "a");
    assert!(
        warnings(&v).iter().any(|w| w.contains("Z") && w.contains("cannot be reached")),
        "got: {:?}",
        warnings(&v)
    );
}

#[test]
fn the_reserved_end_marker_is_rejected_with_its_line_number() {
    let e = err("S -> a\nS -> $ b", "");
    assert_eq!(e["error"]["line"], 2);
    let message = e["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("reserved"), "got: {message}");
}

#[test]
fn a_missing_arrow_names_the_line() {
    let e = err("S -> a S b\nS c\nS -> c", "");
    assert_eq!(e["error"]["line"], 2);
    assert!(e["error"]["hint"].is_string(), "errors should carry a hint");
}

#[test]
fn a_multi_symbol_left_hand_side_is_rejected() {
    let e = err("A B -> c\nA -> a", "");
    assert_eq!(e["error"]["line"], 1);
}

// ===========================================================================
// Variants
// ===========================================================================

#[test]
fn all_four_variants_run_and_anything_else_is_a_named_refusal() {
    for variant in ["lr0", "slr1", "lalr1", "clr1"] {
        let v = ok_with(variant, EXPRESSION, "");
        assert_eq!(steps_of_kind(&v, "variantChosen")[0]["variant"], variant);
    }
    let v: Value = serde_json::from_str(&lr_engine::analyze(EXPRESSION, "lr9", "")).expect("valid JSON");
    assert_eq!(v["ok"], Value::Bool(false), "an unknown variant must not quietly run as LR(0)");
    let message = v["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("Unknown parser variant"), "got: {message}");
    let hint = v["error"]["hint"].as_str().unwrap_or("");
    assert!(hint.contains("lalr1"), "the hint lists what is valid: {hint}");
}

// ===========================================================================
// SLR(1)
//
// SLR(1) reduces A -> alpha . only on FOLLOW(A); items, GOTO and numbering are
// untouched, and the tests pin both halves.
// ===========================================================================

#[test]
fn expression_grammar_is_slr1_with_the_same_twelve_states() {
    let v = ok_with("slr1", EXPRESSION, "");
    assert_eq!(v["table"]["states"], 12, "SLR(1) must not change the state count");
    assert_eq!(
        steps_of_kind(&v, "automatonDone")[0]["stateCount"], 12,
        "the canonical collection is the LR(0) one"
    );
    assert_eq!(conflicts(&v), vec![], "the expression grammar is SLR(1)");
    assert_eq!(v["verdict"]["isValid"], Value::Bool(true));
    assert_eq!(v["verdict"]["class"], "SLR(1)");
}

/// The headline contrast, regression-tested in both directions: the cell that
/// makes the expression grammar fail LR(0) is exactly the cell FOLLOW clears.
#[test]
fn follow_keeps_the_star_column_free_where_lr0_conflicted() {
    let lr0 = ok(EXPRESSION, "");
    let slr1 = ok_with("slr1", EXPRESSION, "");

    // LR(0): I2 holds E -> T . and reduces in every column, including `*`,
    // where I2 also wants to shift.
    assert_eq!(action_cell(&lr0, 2, "*"), vec!["s7", "r2"], "LR(0) conflicts on *");
    assert!(
        conflicts(&lr0).iter().any(|(state, sym, kind)| *state == 2
            && sym == "*"
            && kind == "shift-reduce"),
        "the LR(0) run must still report the conflict: {:?}",
        conflicts(&lr0)
    );
    assert_eq!(
        conflicts(&lr0).len(),
        2,
        "I2 and I9, both on `*`: {:?}",
        conflicts(&lr0)
    );

    // SLR(1): `*` is not in FOLLOW(E) = { +, ), $ }, so r2 never reaches the
    // cell and the shift stands alone.
    assert_eq!(action_cell(&slr1, 2, "*"), vec!["s7"], "SLR(1) leaves ACTION[2, *] to the shift");
    assert_eq!(action_cell(&slr1, 9, "*"), vec!["s7"], "and the same at I9");
    // The reduction did happen - just only where FOLLOW(E) allows it.
    for column in ["+", ")", "$"] {
        assert_eq!(
            action_cell(&slr1, 2, column),
            vec!["r2"],
            "r2 belongs in the FOLLOW(E) column {column}"
        );
    }
}

/// The explanation is the product. It has to name the set, the columns it
/// filled, and the column LR(0) would have filled but this method skipped.
#[test]
fn slr1_reduce_cells_explain_themselves_in_terms_of_follow() {
    let v = ok_with("slr1", EXPRESSION, "");
    let explain = steps_of_kind(&v, "tableCellSet")
        .into_iter()
        .filter_map(|s| s["explain"].as_str())
        .find(|e| e.starts_with("ACTION[2, `+`]"))
        .unwrap_or_default();

    assert!(explain.contains("completed item E → T •"), "got: {explain}");
    assert!(explain.contains("can legally follow E"), "got: {explain}");
    assert!(explain.contains("FOLLOW(E) = { +, ), $ }"), "got: {explain}");
    assert!(explain.contains("r2 goes in those three columns"), "got: {explain}");
    assert!(
        explain.contains("unlike LR(0), not under `*`"),
        "the contrast with LR(0) is the whole lesson of SLR: {explain}"
    );
}

/// Preset 8, the standard grammar that is LALR(1) but not SLR(1). `=` is in
/// FOLLOW(R), so R -> L . reduces into the very column I2 wants to shift on.
#[test]
fn pointers_grammar_is_not_slr1_and_the_conflict_lands_on_equals() {
    let v = ok_with("slr1", POINTERS, "");
    let found = conflicts(&v);
    assert_eq!(found.len(), 1, "exactly one conflicted cell, got {found:?}");

    let (state, symbol, kind) = found[0].clone();
    assert_eq!(symbol, "=", "the conflict must land on `=`, not elsewhere");
    assert_eq!(kind, "shift-reduce");
    assert_eq!(
        action_cell(&v, state as usize, "="),
        vec!["s6", "r5"],
        "the cell holds the shift and the reduction by R -> L"
    );

    assert_eq!(v["verdict"]["class"], "not SLR(1)");
    let message = v["verdict"]["message"].as_str().unwrap_or("");
    assert!(message.contains("1 conflict"), "the count is named: {message}");
    assert!(
        message.contains(&format!("first at state {state} on =")),
        "the first offending cell is named: {message}"
    );
    // FOLLOW is what put it there, and FOLLOW is why SLR cannot get it out.
    assert_eq!(sorted(set_table(&v, "followDone", "R")), set_of(&["=", "$"]), "FOLLOW(R)");
}

/// Preset 7. FOLLOW does narrow the reduction - LR(0) conflicts in `a` and in
/// `$`, SLR(1) only in `$` - but two items whose FOLLOW sets coincide still
/// collide, and no amount of one-symbol lookahead separates them.
#[test]
fn reduce_reduce_preset_keeps_exactly_one_conflict_under_slr1() {
    let v = ok_with("slr1", REDUCE_REDUCE, "a");
    let found = conflicts(&v);
    assert_eq!(found.len(), 1, "one conflicted cell, got {found:?}");
    assert_eq!(found[0].1, "$", "FOLLOW(A) = FOLLOW(B) = {{ $ }}, so it is the $ column");
    assert_eq!(found[0].2, "reduce-reduce");
    // ...and the `a` column that LR(0) also conflicted in is now clean.
    assert_eq!(action_cell(&v, found[0].0 as usize, "a"), Vec::<String>::new());
    assert_eq!(conflicts(&ok(REDUCE_REDUCE, "a")).len(), 2, "LR(0) still conflicts twice");
}

/// Preset 5. Ambiguity is not something a stronger method can fix, and this is
/// the grammar that shows it: `e` is in FOLLOW(S), so the conflict survives.
#[test]
fn dangling_else_still_conflicts_on_e_under_slr1() {
    let v = ok_with("slr1", DANGLING_ELSE, "i i a e a");
    assert!(
        conflicts(&v).iter().any(|(_, sym, kind)| sym == "e" && kind == "shift-reduce"),
        "got {:?}",
        conflicts(&v)
    );
    assert_eq!(v["verdict"]["isValid"], Value::Bool(false));
}

/// The claim that SLR(1) reuses the LR(0) automaton, asserted directly: the
/// automaton phase of the two runs is the same text, step for step.
#[test]
fn the_automaton_is_byte_identical_under_lr0_and_slr1() {
    for (name, grammar, _) in PRESETS {
        let lr0 = ok(grammar, "");
        let slr1 = ok_with("slr1", grammar, "");
        assert_eq!(
            phase_json(&lr0, "automaton"),
            phase_json(&slr1, "automaton"),
            "{name}: SLR(1) must reuse the LR(0) canonical collection unchanged"
        );
        // The sets phase is shared too - SLR(1) only starts *consulting* it.
        assert_eq!(phase_json(&lr0, "sets"), phase_json(&slr1, "sets"), "{name}: sets differ");
    }
}

#[test]
fn parsing_id_equals_id_under_slr1_says_the_conflict_was_resolved_arbitrarily() {
    let v = ok_with("slr1", POINTERS, "id = id");
    let noted: Vec<&str> = steps(&v)
        .iter()
        .filter(|s| s["phase"] == "parse")
        .filter_map(|s| s["explain"].as_str())
        .filter(|e| e.contains("taken arbitrarily"))
        .collect();
    assert!(
        !noted.is_empty(),
        "the trace must say the conflicted cell was resolved arbitrarily"
    );
    assert!(
        noted[0].contains("s6 / r5"),
        "and name both competing actions: {}",
        noted[0]
    );
    assert!(
        noted[0].contains("nothing after this point is a meaningful parse"),
        "got: {}",
        noted[0]
    );
}

/// The comparison line: "LR(0): 2 conflicts, SLR(1): 0". Measured by running
/// both, never inferred, and offered whichever variant is being shown.
#[test]
fn every_analysis_carries_a_measured_variant_comparison() {
    // One row per implemented variant, ordered weakest first - which is also
    // what makes "is there a stronger method that works?" a forward scan.
    let expected: [(&str, u64, bool, u64); 4] = [
        ("lr0", 2, false, 12),
        ("slr1", 0, true, 12),
        ("lalr1", 0, true, 12),
        ("clr1", 0, true, 22),
    ];
    for variant in ["lr0", "slr1", "lalr1", "clr1"] {
        let v = ok_with(variant, EXPRESSION, "");
        let comparison = v["verdict"]["comparison"].as_array().cloned().unwrap_or_default();
        assert_eq!(comparison.len(), 4, "one row per implemented variant");
        for (n, (id, conflicts, valid, states)) in expected.iter().enumerate() {
            let row = &comparison[n];
            assert_eq!(row["variant"], *id, "row {n} of the comparison");
            assert_eq!(row["conflicts"], *conflicts, "{id} conflicts");
            assert_eq!(row["isValid"], Value::Bool(*valid), "{id} validity");
            // The state count is measured the same way the conflict count is:
            // by building that method's own collection, even when it is not the
            // one being shown.
            assert_eq!(row["states"], *states, "{id} states");
            // Everything the "Compare all four" view needs is on the row.
            assert!(row["class"].is_string(), "{id} has a class");
            assert!(row["message"].is_string(), "{id} has a verdict sentence");
            assert_eq!(
                row["shiftReduce"].as_u64().unwrap_or(0) + row["reduceReduce"].as_u64().unwrap_or(0),
                *conflicts,
                "{id}: the kind counts add up to the total"
            );
        }

        let step = steps_of_kind(&v, "variantComparison");
        assert_eq!(step.len(), 1, "the step log carries it too, in the table phase");
        assert_eq!(step[0]["phase"], "table");
        let explain = step[0]["explain"].as_str().unwrap_or("");
        assert!(explain.contains("LR(0) - 2 conflicts"), "got: {explain}");
        assert!(explain.contains("SLR(1) - no conflicts"), "got: {explain}");
        assert!(explain.contains("LALR(1) - no conflicts"), "got: {explain}");
        assert!(explain.contains("CLR(1) - no conflicts"), "got: {explain}");
        assert!(explain.contains("22 in CLR(1)"), "the price is named too: {explain}");
        assert!(
            explain.contains("in 12 states rather than 22"),
            "and what merging bought is named: {explain}"
        );
    }
}

/// A conflicted verdict names a variant that would work, and only one that was run.
#[test]
fn an_lr0_verdict_points_at_slr1_when_slr1_would_work() {
    let expression = ok(EXPRESSION, "");
    let message = expression["verdict"]["message"].as_str().unwrap_or("");
    assert!(message.contains("This grammar is SLR(1), though - try that."), "got: {message}");

    // ...and says nothing of the sort when SLR(1) fails too.
    let dangling = ok(DANGLING_ELSE, "");
    let message = dangling["verdict"]["message"].as_str().unwrap_or("");
    assert!(!message.contains("try that"), "nothing to suggest here: {message}");
}

#[test]
fn every_preset_analyses_cleanly_under_slr1_too() {
    for (name, grammar, test) in PRESETS {
        let v = ok_with("slr1", grammar, test);
        for s in steps(&v) {
            assert!(
                !s["explain"].as_str().unwrap_or("").trim().is_empty(),
                "{name}: step {} ({}) has no explanation",
                s["index"],
                s["kind"]
            );
        }
        // The opening step states the method and the rule it will use.
        let chosen = steps_of_kind(&v, "variantChosen");
        assert_eq!(chosen.len(), 1, "{name}: exactly one variantChosen step");
        assert_eq!(chosen[0]["phase"], "grammar", "{name}: it belongs to the grammar phase");
        assert_eq!(chosen[0]["variant"], "slr1");
        assert_eq!(chosen[0]["reduceRule"], "follow");
    }
}

// ===========================================================================
// Hostile input: a grammar may arrive by email. Opening it must be harmless.
// ===========================================================================

#[test]
fn a_script_tag_is_data_not_code() {
    // The engine must not choke on it, and whatever it decides, it must say so
    // in a structured result rather than panicking.
    let json = lr_engine::analyze("S -> <script>alert(1)</script>", "lr0", "");
    let v: Value = serde_json::from_str(&json).expect("must still be valid JSON");
    assert!(v["ok"].is_boolean());
    // If it parsed, the tag characters are ordinary terminals and nothing else.
    if v["ok"] == Value::Bool(true) {
        let terminals = v["grammar"]["terminals"].to_string();
        assert!(terminals.contains("script") || terminals.contains('<'));
    }
}

#[test]
fn an_oversized_grammar_is_refused_cleanly() {
    let huge = "S -> a\n".repeat(4_000); // well over 20,000 characters
    assert!(huge.len() > lr_engine::limits::MAX_SOURCE_CHARS);
    let e = err(&huge, "");
    let message = e["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("20000") || message.contains("character limit"), "got: {message}");
}

#[test]
fn a_grammar_that_would_explode_the_state_count_is_refused_cleanly() {
    // One long production over many distinct non-terminals, each with two
    // alternatives: the widest grammar of this shape a right-hand side may hold
    // (`MAX_RHS_LEN` symbols). The caps themselves are tested one by one in
    // `limits.rs`; this is the shape that first asked for them.
    let mut src = String::from("S -> ");
    for i in 0..lr_engine::limits::MAX_RHS_LEN {
        src.push_str(&format!("A{i} "));
    }
    src.push('\n');
    for i in 0..lr_engine::limits::MAX_RHS_LEN {
        src.push_str(&format!("A{i} -> a{i} | b{i}\n"));
    }
    let v = analyze(&src, "");
    // Either it fits within the caps or it is refused - never a hang, never a panic.
    if v["ok"] == Value::Bool(false) {
        assert!(v["error"]["message"].is_string());
    } else {
        assert!(v["table"]["states"].as_u64().unwrap_or(0) <= lr_engine::limits::MAX_STATES as u64);
    }
}

#[test]
fn an_over_long_test_string_is_refused_cleanly() {
    let long = "a ".repeat(lr_engine::limits::MAX_TEST_SYMBOLS + 10);
    let e = err(EXPRESSION, &long);
    assert!(e["error"]["message"].is_string());
}

#[test]
fn an_empty_grammar_is_refused_cleanly() {
    let e = err("   \n// just a comment\n", "");
    assert!(e["error"]["message"].as_str().unwrap_or("").contains("empty"));
}

// ===========================================================================
// Step-log round trip
// ===========================================================================

#[test]
fn the_step_list_round_trips_through_json_with_phase_and_index() {
    let v = ok(EXPRESSION, "id + id * id");
    let reparsed: Value =
        serde_json::from_str(&serde_json::to_string(&v["steps"]).expect("serializable"))
            .expect("re-parses");
    let all = reparsed.as_array().cloned().unwrap_or_default();
    assert!(!all.is_empty());

    let valid_phases = ["grammar", "sets", "automaton", "table", "parse"];
    for (i, s) in all.iter().enumerate() {
        assert_eq!(s["index"].as_u64(), Some(i as u64), "indices must be sequential from 0");
        let phase = s["phase"].as_str().unwrap_or("");
        assert!(valid_phases.contains(&phase), "unexpected phase `{phase}`");
        assert!(s["kind"].is_string(), "every step is tagged with its kind");
        assert!(!s["explain"].as_str().unwrap_or("").is_empty());
    }

    // Phases appear in order and never interleave.
    let seen: Vec<&str> = all
        .iter()
        .filter_map(|s| s["phase"].as_str())
        .fold(Vec::new(), |mut acc, p| {
            if acc.last() != Some(&p) {
                acc.push(p);
            }
            acc
        });
    assert_eq!(seen, valid_phases, "phases must run in order, each contiguous");
}

// ===========================================================================
// CLR(1)
//
// CLR(1) differs in its item sets, so the automaton is checked as hard as the
// table. Numbers come from the dragon book, section 4.7.
// ===========================================================================

/// The size of the canonical collection actually built for a variant, taken
/// from the automaton phase rather than from the comparison line, so the two
/// can be checked against each other.
fn state_count(v: &Value) -> u64 {
    steps_of_kind(v, "automatonDone")
        .last()
        .and_then(|s| s["stateCount"].as_u64())
        .unwrap_or_else(|| panic!("no automatonDone step"))
}

/// One row of `verdict.comparison`, by variant id.
fn comparison_row<'a>(v: &'a Value, variant: &str) -> &'a Value {
    v["verdict"]["comparison"]
        .as_array()
        .unwrap_or_else(|| panic!("no comparison"))
        .iter()
        .find(|r| r["variant"] == variant)
        .unwrap_or_else(|| panic!("no comparison row for {variant}"))
}

/// Preset 3, dragon book 4.42: S -> C C, C -> c C | d. Ten CLR(1) states against
/// seven LR(0): the pairs with equal cores but different lookaheads split.
#[test]
fn two_halves_has_ten_clr1_states_against_seven_for_lr0() {
    let clr1 = ok_with("clr1", TWO_HALVES, "c d d");
    let slr1 = ok_with("slr1", TWO_HALVES, "c d d");

    assert_eq!(state_count(&slr1), 7, "the LR(0) collection for S -> C C");
    assert_eq!(state_count(&clr1), 10, "the LR(1) collection is dragon book 4.42");
    assert_eq!(clr1["table"]["states"], 10, "and the table is built over all ten");

    // Both are conflict-free; the point of this grammar is the size, not the
    // strength. The comparison line has to carry that.
    assert_eq!(conflicts(&clr1), vec![]);
    assert_eq!(comparison_row(&clr1, "slr1")["states"], 7);
    assert_eq!(comparison_row(&clr1, "clr1")["states"], 10);
}

/// The three splits named, so a wrong-but-same-sized collection cannot pass.
/// I3/I6, I4/I7 and I8/I9 are the pairs: same items, same dots, different
/// lookaheads. The engine says so in the step that creates the second of each.
#[test]
fn the_extra_clr1_states_are_the_ones_whose_cores_already_existed() {
    let v = ok_with("clr1", TWO_HALVES, "");
    let split: Vec<(u64, String)> = steps_of_kind(&v, "stateCreated")
        .iter()
        .filter_map(|s| {
            let explain = s["explain"].as_str()?;
            explain
                .contains("already holds the same items with the dots in the same places")
                .then(|| (s["id"].as_u64().unwrap_or(u64::MAX), explain.to_string()))
        })
        .collect();

    let ids: Vec<u64> = split.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, vec![6, 7, 9], "exactly three states are core duplicates");
    assert!(split[0].1.contains("I3 already holds"), "got: {}", split[0].1);
    assert!(split[1].1.contains("I4 already holds"), "got: {}", split[1].1);
    assert!(split[2].1.contains("I8 already holds"), "got: {}", split[2].1);

    // ...and the automaton's closing step counts them rather than asserting.
    let done = steps_of_kind(&v, "automatonDone")[0]["explain"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(done.contains("only 7 distinct cores"), "got: {done}");
    assert!(done.contains("3 of them duplicate"), "got: {done}");
}

/// Preset 8: CLR(1) is strictly stronger than SLR(1). The R -> L . item reached
/// before `=` carries it as lookahead; the one reached elsewhere does not.
#[test]
fn pointers_grammar_is_clr1_where_it_was_not_slr1() {
    let slr1 = ok_with("slr1", POINTERS, "id = id");
    let clr1 = ok_with("clr1", POINTERS, "id = id");

    let slr_conflicts = conflicts(&slr1);
    assert_eq!(slr_conflicts.len(), 1, "SLR(1) conflicts: {slr_conflicts:?}");
    assert_eq!(slr_conflicts[0].1, "=", "on `=`");
    assert_eq!(slr_conflicts[0].2, "shift-reduce");

    assert_eq!(conflicts(&clr1), vec![], "CLR(1) clears it");
    assert_eq!(clr1["verdict"]["isValid"], Value::Bool(true));
    assert_eq!(clr1["verdict"]["class"], "CLR(1)");

    // FOLLOW(R) still contains `=` - CLR(1) simply never asks it.
    assert_eq!(sorted(set_table(&clr1, "followDone", "R")), set_of(&["=", "$"]));

    // And the test string now parses, where under SLR(1) the conflicted cell
    // had to be resolved arbitrarily.
    assert_eq!(steps_of_kind(&clr1, "parseAccept").len(), 1);
    let arbitrary = steps(&clr1)
        .iter()
        .filter_map(|s| s["explain"].as_str())
        .any(|e| e.contains("taken arbitrarily"));
    assert!(!arbitrary, "nothing is arbitrary in a conflict-free table");
}

/// Preset 9. CLR(1) keeps `A -> c .` after `a` apart from `A -> c .` after `b`,
/// because the two are reached with different lookaheads.
#[test]
fn merge_casualty_is_clr1() {
    let clr1 = ok_with("clr1", MERGE_CASUALTY, "a c d");
    assert_eq!(conflicts(&clr1), vec![], "CLR(1) handles it");
    assert_eq!(clr1["verdict"]["isValid"], Value::Bool(true));
    assert_eq!(steps_of_kind(&clr1, "parseAccept").len(), 1);

    // The weaker methods do not, and the conflicts they get are reduce-reduce:
    // A -> c . and B -> c . land in the same cell.
    for variant in ["lr0", "slr1"] {
        let weaker = ok_with(variant, MERGE_CASUALTY, "");
        let found = conflicts(&weaker);
        assert!(!found.is_empty(), "{variant} must conflict here: {found:?}");
        for (_, _, kind) in &found {
            assert_eq!(kind, "reduce-reduce", "{variant}: {found:?}");
        }
    }
}

/// Presets 5 and 6. Ambiguity is a property of the grammar, and no LR method
/// removes it - CLR(1) included. The tool must never suggest otherwise.
#[test]
fn the_ambiguous_presets_still_conflict_under_clr1() {
    for (name, grammar, test) in [
        ("Dangling else", DANGLING_ELSE, "i i a e a"),
        ("Ambiguous expression", AMBIGUOUS_EXPR, "id + id * id"),
    ] {
        let v = ok_with("clr1", grammar, test);
        let found = conflicts(&v);
        assert!(!found.is_empty(), "{name}: CLR(1) must still conflict");
        assert_eq!(v["verdict"]["isValid"], Value::Bool(false), "{name}");
        assert_eq!(v["verdict"]["class"], "not CLR(1)", "{name}");

        // Every implemented method fails, so nothing may be suggested.
        let message = v["verdict"]["message"].as_str().unwrap_or("");
        assert!(!message.contains("try that"), "{name}: nothing to suggest: {message}");
        for row in v["verdict"]["comparison"].as_array().cloned().unwrap_or_default() {
            assert_eq!(row["isValid"], Value::Bool(false), "{name}: {row}");
        }
    }
    // ...and the dangling-else conflict is still the one on `e`.
    let v = ok_with("clr1", DANGLING_ELSE, "");
    assert!(
        conflicts(&v).iter().any(|(_, sym, kind)| sym == "e" && kind == "shift-reduce"),
        "got {:?}",
        conflicts(&v)
    );
}

/// CLR(1) never merges states, so its collection is at least as large as the
/// LR(0) one for every grammar - and the comparison line says the same numbers
/// the automaton phase does.
#[test]
fn clr1_never_has_fewer_states_than_slr1() {
    for (name, grammar, test) in PRESETS {
        let slr1 = ok_with("slr1", grammar, test);
        let clr1 = ok_with("clr1", grammar, test);
        let (small, large) = (state_count(&slr1), state_count(&clr1));
        assert!(
            large >= small,
            "{name}: CLR(1) has {large} states against SLR(1)'s {small}; splitting by lookahead can never merge two states"
        );
        assert_eq!(
            comparison_row(&clr1, "slr1")["states"].as_u64(),
            Some(small),
            "{name}: the comparison line disagrees with the SLR(1) automaton"
        );
        assert_eq!(
            comparison_row(&slr1, "clr1")["states"].as_u64(),
            Some(large),
            "{name}: the comparison line disagrees with the CLR(1) automaton"
        );
    }
}

/// Item display: `[A -> alpha . beta, {a, b}]`, one line per core with the
/// lookaheads merged into a set. Duplicate items, one per lookahead, would be
/// correct and unreadable.
#[test]
fn clr1_items_carry_merged_lookahead_sets() {
    let v = ok_with("clr1", TWO_HALVES, "");
    let closed = steps_of_kind(&v, "stateClosed");
    let i0 = closed
        .iter()
        .find(|s| s["id"] == 0)
        .unwrap_or_else(|| panic!("no stateClosed for I0"));
    let items = i0["items"].as_array().cloned().unwrap_or_default();

    // S' -> . S {$}, S -> . C C {$}, C -> . c C {c, d}, C -> . d {c, d}.
    assert_eq!(items.len(), 4, "one item per core, not one per lookahead");
    for item in &items {
        assert!(item["lookaheads"].is_array(), "every CLR(1) item carries a set: {item}");
    }
    let widest = items
        .iter()
        .filter_map(|i| i["lookaheads"].as_array().map(Vec::len))
        .max();
    assert_eq!(widest, Some(2), "C -> . c C carries both c and d on one line");

    // ...and the LR(0)/SLR(1) items still carry none at all.
    let lr0 = ok_with("lr0", TWO_HALVES, "");
    for s in steps_of_kind(&lr0, "stateClosed") {
        for item in s["items"].as_array().cloned().unwrap_or_default() {
            assert!(item["lookaheads"].is_null(), "LR(0) items carry no lookahead: {item}");
        }
    }
}

/// The explanation is the product. A CLR(1) reduce cell has to name the item's
/// own lookaheads and say plainly that FOLLOW is not what put it there -
/// confusing the two is the single most common misreading of the method.
#[test]
fn clr1_reduce_cells_say_they_did_not_consult_follow() {
    let v = ok_with("clr1", TWO_HALVES, "");
    let explain = steps_of_kind(&v, "tableCellSet")
        .into_iter()
        .filter_map(|s| s["explain"].as_str())
        .find(|e| e.starts_with("ACTION[7, $]"))
        .unwrap_or_default();

    assert!(explain.contains("completed item C → d •, {$}"), "got: {explain}");
    assert!(explain.contains("lookaheads written into the item itself"), "got: {explain}");
    assert!(explain.contains("FOLLOW(C) is not consulted here at all"), "got: {explain}");
    assert!(
        explain.contains("unlike LR(0), not under `c` or `d`"),
        "the contrast with the blanket rule survives: {explain}"
    );

    // The same state under SLR(1) - I4, the unsplit one - would have reduced on
    // c and d as well, because FOLLOW(C) holds all three.
    let slr1 = ok_with("slr1", TWO_HALVES, "");
    assert_eq!(sorted(set_table(&slr1, "followDone", "C")), set_of(&["c", "d", "$"]));
}

/// The opening step names the method and the rule, and the rule is a new tag
/// that the FIRST/FOLLOW view switches on to caption itself correctly.
#[test]
fn every_preset_analyses_cleanly_under_clr1_too() {
    for (name, grammar, test) in PRESETS {
        let v = ok_with("clr1", grammar, test);
        for s in steps(&v) {
            assert!(
                !s["explain"].as_str().unwrap_or("").trim().is_empty(),
                "{name}: step {} ({}) has no explanation",
                s["index"],
                s["kind"]
            );
        }
        let chosen = steps_of_kind(&v, "variantChosen");
        assert_eq!(chosen.len(), 1, "{name}: exactly one variantChosen step");
        assert_eq!(chosen[0]["phase"], "grammar", "{name}");
        assert_eq!(chosen[0]["variant"], "clr1");
        assert_eq!(chosen[0]["reduceRule"], "item-lookahead");
        assert!(
            chosen[0]["explain"].as_str().unwrap_or("").contains("FOLLOW is not consulted"),
            "{name}: the opening step has to head off the SLR(1) confusion"
        );
        // LALR(1) is the only variant that merges states. CLR(1) never does -
        // keeping the states apart is the whole of the method.
        assert_eq!(steps_of_kind(&v, "statesMerged").len(), 0, "{name}");
        assert_eq!(steps_of_kind(&v, "mergeDone").len(), 0, "{name}");
    }
}

/// CLR(1) changes the automaton, so - unlike SLR(1) - its automaton phase must
/// differ from LR(0)'s. Asserted so a regression that silently fell back to the
/// LR(0) collection would be caught even where the conflict counts agree.
#[test]
fn the_clr1_automaton_is_not_the_lr0_one() {
    let differing: Vec<&str> = PRESETS
        .iter()
        .filter(|(_, grammar, _)| {
            let lr0 = ok(grammar, "");
            let clr1 = ok_with("clr1", grammar, "");
            phase_json(&lr0, "automaton") != phase_json(&clr1, "automaton")
        })
        .map(|(name, _, _)| *name)
        .collect();
    assert_eq!(differing.len(), PRESETS.len(), "every preset's automaton phase differs");

    // The sets phase, by contrast, is shared by all three: FIRST and FOLLOW are
    // computed once for everyone, and CLR(1) simply uses FIRST rather than FOLLOW.
    for (name, grammar, _) in PRESETS {
        assert_eq!(
            phase_json(&ok(grammar, ""), "sets"),
            phase_json(&ok_with("clr1", grammar, ""), "sets"),
            "{name}: the sets phase must be identical under every variant"
        );
    }
}

// ===========================================================================
// FIRST over a sequence
//
// FIRST over a *string* of symbols, as CLR(1) closure needs it.
// ===========================================================================

/// FIRST(sequence) over `grammar`, as (members, whole-sequence-nullable).
fn first_of(grammar: &str, sequence: &[&str]) -> (Vec<String>, bool) {
    let parsed = lr_engine::grammar::parse(grammar).expect("grammar parses");
    let mut rec = lr_engine::trace::Recorder::new();
    let sets = lr_engine::sets::compute(&parsed.grammar, &mut rec).expect("sets compute");
    let seq: Vec<String> = sequence.iter().map(|s| (*s).to_string()).collect();
    let (members, nullable) = sets.first_of_sequence(&parsed.grammar, &seq);
    (members, nullable)
}

#[test]
fn first_of_sequence_stops_at_the_first_symbol_that_cannot_vanish() {
    // FIRST(E) = FIRST(T) = FIRST(F) = { (, id }, and none of them is nullable.
    assert_eq!(first_of(EXPRESSION, &["E"]), (set_of(&["(", "id"]), false));
    // The `+` is never reached: E cannot vanish.
    assert_eq!(first_of(EXPRESSION, &["E", "+", "T"]), (set_of(&["(", "id"]), false));
    // A terminal first is the whole answer.
    assert_eq!(first_of(EXPRESSION, &["+", "E"]), (vec!["+".to_string()], false));
}

#[test]
fn first_of_sequence_of_nothing_is_empty_and_nullable() {
    // The empty sequence derives the empty string and nothing else. This is the
    // case CLR(1) hits whenever the dot is before the last symbol of a
    // production, and it is why the lookahead is appended before the call.
    assert_eq!(first_of(EXPRESSION, &[]), (Vec::<String>::new(), true));
}

#[test]
fn first_of_sequence_that_is_entirely_nullable_reports_it() {
    // A and B can both vanish; C cannot.
    let g = "S -> A B C\nA -> a | ε\nB -> b | ε\nC -> c";
    assert_eq!(first_of(g, &["A"]), (vec!["a".to_string()], true));
    assert_eq!(first_of(g, &["A", "B"]), (set_of(&["a", "b"]), true));
    // Once C is in the sequence the whole thing can no longer vanish...
    assert_eq!(first_of(g, &["A", "B", "C"]), (set_of(&["a", "b", "c"]), false));
    // ...and the returned set never contains ε; that is what the flag is for.
    let (members, nullable) = first_of(g, &["A", "B"]);
    assert!(!members.iter().any(|m| m == "ε"), "got {members:?}");
    assert!(nullable);
}

#[test]
fn first_of_sequence_accepts_the_end_marker_as_a_lookahead() {
    // CLR(1) calls this with FIRST(beta a) where `a` may be `$`. `$` is not a
    // symbol of the grammar, so it has to be handled explicitly.
    let g = "S -> A B C\nA -> a | ε\nB -> b | ε\nC -> c";
    assert_eq!(first_of(g, &["$"]), (vec!["$".to_string()], false));
    // Members come back in the canonical display order - grammar-appearance
    // order, with `$` pushed to the end - so an item renders the same way twice.
    assert_eq!(
        first_of(g, &["A", "B", "$"]),
        (vec!["a".to_string(), "b".to_string(), "$".to_string()], false)
    );
    // Which is exactly the shape the closure relies on: appending the lookahead
    // makes the sequence un-nullable, so the flag is always false in that call.
    assert!(!first_of(EXPRESSION, &["E", "$"]).1);
}

/// If CLR(1) exceeds the state cap, the requested analysis still succeeds and
/// the comparison omits that row.
#[test]
fn a_clr1_collection_over_the_cap_does_not_break_the_lr0_analysis() {
    // Many distinct right-contexts around one shared non-terminal: LR(0)
    // collapses them into one state, CLR(1) keeps every one apart.
    let mut src = String::from("S -> ");
    let alts: Vec<String> = (0..80).map(|i| format!("x{i} A y{i}")).collect();
    src.push_str(&alts.join(" | "));
    src.push_str("\nA -> B | c\nB -> d A | e\n");

    let over = ok(&src, "");
    assert!(
        over["table"]["states"].as_u64().unwrap_or(0) < lr_engine::limits::MAX_STATES as u64,
        "the LR(0) collection must be well within the cap for this test to mean anything"
    );
    let rows: Vec<String> = over["verdict"]["comparison"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|r| r["variant"].as_str().map(str::to_string))
        .collect();
    assert_eq!(rows, vec!["lr0", "slr1"], "the CLR(1) row is dropped, not faked");

    // Asking for CLR(1) directly on the same grammar is a clean, named refusal.
    let refused = analyze_with("clr1", &src, "");
    assert_eq!(refused["ok"], Value::Bool(false));
    let message = refused["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("more than 500 states"), "got: {message}");
    assert!(refused["error"]["hint"].is_string());
}

// ===========================================================================
// LALR(1)
//
// LALR(1) is built the textbook way (full CLR(1), then merge same-core states).
// Pinned: one merged state per core; merging never adds a shift-reduce conflict.
// ===========================================================================

/// The merged collection's size, read off the step that closes the merging
/// stage. `state_count` would give the CLR(1) count an LALR(1) run starts from.
fn lalr_state_count(v: &Value) -> u64 {
    steps_of_kind(v, "mergeDone")
        .last()
        .and_then(|s| s["after"].as_u64())
        .unwrap_or_else(|| panic!("no mergeDone step - was this an LALR(1) run?"))
}

/// The renumbering: new state id -> the CLR(1) states it was merged from.
fn merge_groups(v: &Value) -> Vec<Vec<u64>> {
    steps_of_kind(v, "mergeDone")
        .last()
        .and_then(|s| s["groups"].as_array())
        .unwrap_or_else(|| panic!("no mergeDone step"))
        .iter()
        .map(|g| {
            g.as_array()
                .unwrap_or_else(|| panic!("a group must be a list"))
                .iter()
                .filter_map(Value::as_u64)
                .collect()
        })
        .collect()
}

/// Every state's core - its items with the lookaheads stripped - taken from the
/// `stateClosed` steps, which is what the diagram draws.
fn cores(v: &Value) -> Vec<Vec<(u64, u64)>> {
    steps_of_kind(v, "stateClosed")
        .iter()
        .map(|s| {
            let mut core: Vec<(u64, u64)> = s["items"]
                .as_array()
                .unwrap_or_else(|| panic!("a closed state must list its items"))
                .iter()
                .map(|i| (i["production"].as_u64().unwrap_or(0), i["dot"].as_u64().unwrap_or(0)))
                .collect();
            core.sort_unstable();
            core.dedup();
            core
        })
        .collect()
}

fn distinct_cores(v: &Value) -> usize {
    let mut all = cores(v);
    all.sort();
    all.dedup();
    all.len()
}

/// Preset 3, dragon book 4.42: ten CLR(1) states, seven LALR(1), verified by
/// construction (three pairs really are folded).
#[test]
fn two_halves_has_ten_clr1_states_and_seven_lalr1_ones() {
    let clr1 = ok_with("clr1", TWO_HALVES, "c d d");
    let lalr1 = ok_with("lalr1", TWO_HALVES, "c d d");

    assert_eq!(state_count(&clr1), 10, "the canonical LR(1) collection");
    assert_eq!(
        state_count(&lalr1),
        10,
        "an LALR(1) run builds that same collection first, in full"
    );
    assert_eq!(lalr_state_count(&lalr1), 7, "and then merges it down to one state per core");
    assert_eq!(lalr1["table"]["states"], 7, "the table is built over the merged states");
    assert_eq!(state_count(&ok(TWO_HALVES, "")), 7, "which is the size of the LR(0) collection");

    // The three pairs, named. Same items, same dots, different lookaheads.
    let merged: Vec<Vec<u64>> = steps_of_kind(&lalr1, "statesMerged")
        .iter()
        .map(|s| s["mergedIds"].as_array().unwrap_or(&vec![]).iter().filter_map(Value::as_u64).collect())
        .collect();
    assert_eq!(merged, vec![vec![3, 6], vec![4, 7], vec![8, 9]]);

    // Renumbering is by the smallest original id in each group, so I0 stays I0
    // and the ids stay contiguous.
    assert_eq!(
        merge_groups(&lalr1),
        vec![vec![0], vec![1], vec![2], vec![3, 6], vec![4, 7], vec![5], vec![8, 9]]
    );

    // The user-facing sentence, which is the actual product here.
    let first = steps_of_kind(&lalr1, "statesMerged")[1]["explain"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(first.contains("States I4 and I7 have the same core"), "got: {first}");
    assert!(first.contains("differ only in lookahead"), "got: {first}");
    assert!(first.contains("C → d •"), "the core is named: {first}");
    assert!(first.contains("{c, d, $}"), "and so is the union: {first}");
}
/// Preset 9: CLR(1) is clean, LALR(1) conflicts; the explanation must say that
/// the merge caused it.
/// conflict exists, which is what LALR(1) has to teach.
#[test]
fn merge_casualty_is_clr1_but_not_lalr1_and_says_which_merge_did_it() {
    let clr1 = ok_with("clr1", MERGE_CASUALTY, "a c d");
    let lalr1 = ok_with("lalr1", MERGE_CASUALTY, "a c d");

    assert_eq!(conflicts(&clr1), vec![], "CLR(1) keeps the two states apart");
    let lalr_conflicts = conflicts(&lalr1);
    assert!(!lalr_conflicts.is_empty(), "LALR(1) must not get away with it");
    assert!(
        lalr_conflicts.iter().all(|(_, _, kind)| kind == "reduce-reduce"),
        "merging can only ever collide reductions: {lalr_conflicts:?}"
    );
    assert_eq!(
        lalr_conflicts,
        vec![
            (6, "d".to_string(), "reduce-reduce".to_string()),
            (6, "e".to_string(), "reduce-reduce".to_string())
        ]
    );
    assert_eq!(lalr_state_count(&lalr1), 13, "one group merged, out of fourteen states");

    // Every one of them is attributed to the merge, in the table JSON...
    let flagged = lalr1["table"]["conflicts"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        flagged.iter().all(|c| c["mergeInduced"] == Value::Bool(true)),
        "each conflict is marked as the merge's doing: {flagged:?}"
    );

    // ...and, far more importantly, in words.
    let explained = steps_of_kind(&lalr1, "tableConflict")[0]["explain"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        explained.contains(
            "This reduce-reduce conflict did not exist in CLR(1). It appeared because states I6 and I9 were merged: separately each had a single lookahead, but the union means both reductions are now possible on `d`. This is the price LALR(1) pays for its smaller table, and it is why some grammars are CLR(1) but not LALR(1)."
        ),
        "the merge-induced explanation is missing or has drifted: {explained}"
    );

    // And the verdict points at the method that does work - which, because
    // IMPLEMENTED is ordered weakest first, is CLR(1) and not SLR(1).
    let verdict = lalr1["verdict"]["message"].as_str().unwrap_or("");
    assert!(verdict.contains("This grammar is CLR(1), though - try that."), "got: {verdict}");
    assert!(
        verdict.contains("exist only because LALR(1) merged states"),
        "the verdict names the cause too: {verdict}"
    );
}

/// Preset 8. The famous grammar that is LALR(1) but not SLR(1) - which is the
/// reason every real parser generator builds LALR(1) rather than SLR(1).
#[test]
fn pointers_grammar_is_lalr1_as_well_as_clr1() {
    let lalr1 = ok_with("lalr1", POINTERS, "id = id");
    assert_eq!(conflicts(&lalr1), vec![], "no conflicts under LALR(1)");
    assert_eq!(lalr_state_count(&lalr1), 10, "in the LR(0) collection's ten states");
    assert_eq!(comparison_row(&lalr1, "slr1")["conflicts"], 1, "where SLR(1) has one");
    assert_eq!(comparison_row(&lalr1, "clr1")["states"], 14, "and CLR(1) needs fourteen states");
    assert_eq!(lalr1["verdict"]["class"], "LALR(1)");
}

/// The expression grammar under all four. Twelve states for LR(0), SLR(1) and
/// LALR(1) - and twenty-two for CLR(1), which is the measured number for this
/// grammar and the clearest case of what merging is worth.
#[test]
fn expression_grammar_is_twelve_states_under_everything_but_clr1() {
    for variant in ["lr0", "slr1", "lalr1"] {
        let v = ok_with(variant, EXPRESSION, "id + id * id");
        assert_eq!(v["table"]["states"], 12, "{variant}: twelve states");
    }
    assert_eq!(lalr_state_count(&ok_with("lalr1", EXPRESSION, "")), 12);
    assert_eq!(ok_with("clr1", EXPRESSION, "")["table"]["states"], 22);

    for variant in ["slr1", "lalr1", "clr1"] {
        let v = ok_with(variant, EXPRESSION, "");
        assert_eq!(conflicts(&v), vec![], "{variant}: no conflicts");
    }
    // LR(0) is the one that fails, on `*` in I2 and I9.
    assert_eq!(
        conflicts(&ok(EXPRESSION, "")),
        vec![
            (2, "*".to_string(), "shift-reduce".to_string()),
            (9, "*".to_string(), "shift-reduce".to_string())
        ]
    );
}

/// The invariant that defines the method: one merged state per core. Its
/// consequence - LR(0) count <= LALR(1) count <= CLR(1) count, with the left
/// inequality always an equality - is checked at the same time.
#[test]
fn the_lalr1_collection_has_exactly_one_state_per_clr1_core() {
    for (name, grammar, test) in PRESETS {
        let lr0 = ok(grammar, test);
        let clr1 = ok_with("clr1", grammar, test);
        let lalr1 = ok_with("lalr1", grammar, test);

        let merged = lalr_state_count(&lalr1);
        let canonical = state_count(&clr1);
        assert_eq!(
            merged as usize,
            distinct_cores(&clr1),
            "{name}: the merged collection is one state per distinct CLR(1) core"
        );
        assert!(merged <= canonical, "{name}: merging never grows the collection");
        assert!(
            merged >= state_count(&lr0),
            "{name}: and never shrinks it below the LR(0) collection"
        );
        assert_eq!(
            merged,
            state_count(&lr0),
            "{name}: in fact the two coincide - LR(0) states and cores are the same thing"
        );

        // The groups partition the CLR(1) states exactly: every state accounted
        // for once, and ordered by the smallest id in each group.
        let groups = merge_groups(&lalr1);
        assert_eq!(groups.len(), merged as usize, "{name}: one group per merged state");
        let mut seen: Vec<u64> = groups.iter().flatten().copied().collect();
        seen.sort_unstable();
        assert_eq!(seen, (0..canonical).collect::<Vec<u64>>(), "{name}: a partition");
        let firsts: Vec<u64> = groups.iter().map(|g| g[0]).collect();
        let mut sorted = firsts.clone();
        sorted.sort_unstable();
        assert_eq!(firsts, sorted, "{name}: renumbered by the smallest original id");
        assert_eq!(groups[0], vec![0], "{name}: I0 is never merged and stays I0");
    }
}

/// Merging can create a reduce-reduce conflict. It can never create a
/// shift-reduce one: the shifts follow the core, and the core is exactly what
/// merging preserves. If this ever fails, the merge is wrong.
#[test]
fn merging_never_introduces_a_shift_reduce_conflict() {
    for (name, grammar, test) in PRESETS {
        let clr1 = ok_with("clr1", grammar, test);
        let lalr1 = ok_with("lalr1", grammar, test);

        let clr_sr = conflicts(&clr1).into_iter().filter(|(_, _, k)| k == "shift-reduce").count();
        let lalr_sr = conflicts(&lalr1).into_iter().filter(|(_, _, k)| k == "shift-reduce").count();
        assert!(
            lalr_sr <= clr_sr,
            "{name}: LALR(1) has {lalr_sr} shift-reduce conflicts against CLR(1)'s {clr_sr}"
        );

        // Said the other way round, from the engine's own attribution: nothing
        // marked merge-induced is ever a shift-reduce conflict.
        for c in lalr1["table"]["conflicts"].as_array().cloned().unwrap_or_default() {
            if c["mergeInduced"] == Value::Bool(true) {
                assert_eq!(c["kind"], "reduce-reduce", "{name}: {c:?}");
            }
        }
        assert_eq!(
            comparison_row(&lalr1, "lalr1")["mergeInduced"]
                .as_u64()
                .unwrap_or(0),
            lalr1["table"]["conflicts"]
                .as_array()
                .map(|cs| cs.iter().filter(|c| c["mergeInduced"] == Value::Bool(true)).count())
                .unwrap_or(0) as u64,
            "{name}: the comparison row counts the same conflicts the table marks"
        );
    }
}

/// Only the merge casualty separates LALR(1) from CLR(1). Compared by column and
/// kind, not state number, since merging renumbers (dangling else: I8 vs I4).
#[test]
fn lalr1_and_clr1_agree_on_every_preset_but_the_merge_casualty() {
    fn cells(v: &Value) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> =
            conflicts(v).into_iter().map(|(_, symbol, kind)| (symbol, kind)).collect();
        out.sort();
        out
    }
    for (name, grammar, test) in PRESETS {
        let lalr1 = ok_with("lalr1", grammar, test);
        let clr1 = ok_with("clr1", grammar, test);
        if name == "Merge casualty" {
            assert_ne!(cells(&lalr1), cells(&clr1), "{name}: this is the one that breaks");
            assert_eq!(cells(&clr1), vec![], "{name}: and CLR(1) is the one that copes");
            continue;
        }
        assert_eq!(
            cells(&lalr1),
            cells(&clr1),
            "{name}: merging cost this grammar nothing, so the two tables must agree"
        );
        assert_eq!(
            lalr1["verdict"]["isValid"], clr1["verdict"]["isValid"],
            "{name}: and so must the verdicts"
        );
    }
}

/// Every preset, under every variant, end to end: no panic, no empty
/// explanation, and the merging stage where - and only where - it belongs.
#[test]
fn every_preset_analyses_cleanly_under_every_variant() {
    for (name, grammar, test) in PRESETS {
        for variant in ["lr0", "slr1", "lalr1", "clr1"] {
            let v = ok_with(variant, grammar, test);
            for s in steps(&v) {
                assert!(
                    !s["explain"].as_str().unwrap_or("").trim().is_empty(),
                    "{name} under {variant}: step {} ({}) has no explanation",
                    s["index"],
                    s["kind"]
                );
            }
            let merges = steps_of_kind(&v, "statesMerged").len();
            let done = steps_of_kind(&v, "mergeDone").len();
            if variant == "lalr1" {
                assert_eq!(done, 1, "{name}: an LALR(1) run always closes the merging stage");
                // Every merging step belongs to the automaton phase, after the
                // canonical collection is complete and before the table starts.
                for s in steps_of_kind(&v, "statesMerged") {
                    assert_eq!(s["phase"], "automaton", "{name}");
                }
                assert_eq!(steps_of_kind(&v, "mergeDone")[0]["phase"], "automaton", "{name}");
                let last_automaton = steps(&v)
                    .iter()
                    .rfind(|s| s["phase"] == "automaton")
                    .map(|s| s["kind"].as_str().unwrap_or("").to_string());
                assert_eq!(
                    last_automaton.as_deref(),
                    Some("mergeDone"),
                    "{name}: merging is the last thing the automaton phase does"
                );
            } else {
                assert_eq!(merges, 0, "{name}: {variant} merges nothing");
                assert_eq!(done, 0, "{name}: {variant} has no merging stage");
            }
        }
    }
}

/// The opening step names the method and its rule. LALR(1) shares the
/// `item-lookahead` rule with CLR(1) - it really does place reductions the same
/// way - so the sentence has to carry the difference instead.
#[test]
fn the_lalr1_opening_step_says_it_merges() {
    let v = ok_with("lalr1", TWO_HALVES, "");
    let chosen = steps_of_kind(&v, "variantChosen");
    assert_eq!(chosen.len(), 1);
    assert_eq!(chosen[0]["variant"], "lalr1");
    assert_eq!(chosen[0]["reduceRule"], "item-lookahead");
    let explain = chosen[0]["explain"].as_str().unwrap_or("");
    assert!(explain.contains("merges them"), "got: {explain}");
    assert!(explain.contains("CLR(1) but not LALR(1)"), "the cost is named up front: {explain}");
}

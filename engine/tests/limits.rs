//! One test per constant in `limits.rs`: just inside a cap is analysed, over it is a
//! clean `{ok: false, error}` naming the limit, quickly. The one-second claims hold for
//! release; check with `CARGO_PROFILE_RELEASE_PANIC=unwind cargo test --release --test limits`.
//! The override is needed because the release profile sets `panic = "abort"` and the
//! harness must unwind (cargo issue 6313: a `cdylib` is built both ways).

use lr_engine::items::ItemMode;
use lr_engine::limits::{
    MAX_COMPARISON_WORK, MAX_FIXPOINT_ROUNDS, MAX_PARSE_STACK, MAX_PARSE_STEPS, MAX_PRODUCTIONS,
    MAX_RHS_LEN, MAX_SOURCE_CHARS, MAX_STATES, MAX_STEPS, MAX_SYMBOLS, MAX_SYMBOL_LEN,
    MAX_TEST_SYMBOLS, MAX_WARNINGS, MAX_WORK,
};
use lr_engine::trace::Recorder;
use lr_engine::{automaton, grammar, sets};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const VARIANTS: [&str; 4] = ["lr0", "slr1", "lalr1", "clr1"];

/// True for the profile the one-second claims are made about.
const RELEASE: bool = !cfg!(debug_assertions);

/// How long a single `analyze()` call may take in this file.
fn budget() -> Duration {
    if RELEASE {
        Duration::from_secs(1)
    } else {
        Duration::from_secs(20)
    }
}

/// Timed calls do not overlap in a release run: the test harness runs tests on
/// several threads, and two analyses sharing a core would each look slow.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// One call, its wall-clock time, and the two things that hold of every input:
/// the answer is valid JSON, and `ok` is a boolean.
fn timed(variant: &str, grammar: &str, test: &str) -> (Value, Duration) {
    let _alone = if RELEASE {
        Some(ONE_AT_A_TIME.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))
    } else {
        None
    };
    let started = Instant::now();
    let json = lr_engine::analyze(grammar, variant, test);
    let took = started.elapsed();
    let v: Value = serde_json::from_str(&json)
        .unwrap_or_else(|e| panic!("{variant}: analyze must return valid JSON: {e}"));
    assert!(v["ok"].is_boolean(), "{variant}: `ok` must be a boolean, got {}", v["ok"]);
    (v, took)
}

/// One call that must come back within [`budget`].
fn run(variant: &str, grammar: &str, test: &str) -> Value {
    let (v, took) = timed(variant, grammar, test);
    assert!(
        took < budget(),
        "{variant}: analyze took {took:?}, over {:?} (grammar starts {:?})",
        budget(),
        grammar.chars().take(40).collect::<String>()
    );
    v
}

/// The input must be analysed. Returns the whole result.
fn analysed(variant: &str, grammar: &str, test: &str) -> Value {
    let v = run(variant, grammar, test);
    assert_eq!(v["ok"], Value::Bool(true), "{variant}: expected an analysis, got {}", v["error"]);
    v
}

/// The input must be refused. Returns the `error` object.
fn refused(variant: &str, grammar: &str, test: &str) -> Value {
    let v = run(variant, grammar, test);
    assert_eq!(v["ok"], Value::Bool(false), "{variant}: expected a refusal, got an analysis");
    assert!(v.get("steps").is_none(), "{variant}: a refused input carries no steps");
    assert!(!message(&v["error"]).trim().is_empty(), "{variant}: an error must say what is wrong");
    v["error"].clone()
}

/// The input must be refused identically by all four methods.
fn refused_by_all(grammar: &str, test: &str) -> Value {
    let first = refused(VARIANTS[0], grammar, test);
    for variant in &VARIANTS[1..] {
        assert_eq!(refused(variant, grammar, test), first, "{variant}: the refusal depends on the method");
    }
    first
}

fn message(e: &Value) -> &str {
    e["message"].as_str().unwrap_or("")
}

fn hint(e: &Value) -> &str {
    e["hint"].as_str().unwrap_or("")
}

fn accepted(v: &Value) -> bool {
    v["steps"]
        .as_array()
        .is_some_and(|s| s.iter().any(|s| s["kind"] == "parseAccept"))
}

fn parse_steps(v: &Value) -> usize {
    v["steps"].as_array().map_or(0, |s| s.iter().filter(|s| s["phase"] == "parse").count())
}

/// The variant ids of the comparison rows, in order.
fn rows(v: &Value) -> Vec<String> {
    v["verdict"]["comparison"]
        .as_array()
        .map(|a| a.iter().filter_map(|r| r["variant"].as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

fn warning_messages(v: &Value) -> Vec<String> {
    v["warnings"]
        .as_array()
        .map(|a| a.iter().filter_map(|w| w["message"].as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

/// `count` distinct productions `S -> w x y z` over `a`-`d`, one per line.
fn distinct_productions(count: usize) -> String {
    (0..count)
        .map(|i| {
            let rhs: Vec<&str> = (0..4)
                .map(|k| ["a", "b", "c", "d"].get((i >> (2 * k)) & 3).copied().unwrap_or("a"))
                .collect();
            format!("S -> {}\n", rhs.join(" "))
        })
        .collect()
}

/// `S -> t1 ... t40 | t41 ... t80 | ...` over `count` distinct terminals: with
/// `S` and the augmented start that is `count + 2` symbols in all.
fn distinct_terminals(count: usize) -> String {
    let symbols: Vec<String> = (1..=count).map(|i| format!("t{i}")).collect();
    let alternatives: Vec<String> = symbols.chunks(40).map(|c| c.join(" ")).collect();
    format!("S -> {}", alternatives.join(" | "))
}

/// `chains` productions `S -> c_i a a ... a`, each as long as a right-hand side
/// may be. No two share a prefix, so each costs a state per symbol:
/// `chains * MAX_RHS_LEN + 2` states under every method.
fn chains(count: usize) -> String {
    (0..count).map(|i| format!("S -> c{i} {}\n", "a ".repeat(MAX_RHS_LEN - 1))).collect()
}

/// The ambiguous expression grammar with `operators` binary operators:
/// `E -> E o1 E | E o2 E | ... | id`. One non-terminal, and under CLR(1) every
/// item of every state carries every operator as a lookahead - few states, few
/// productions, and an amount of set-merging that grows with the fourth power.
fn ambiguous(operators: usize) -> String {
    let mut alternatives: Vec<String> = (1..=operators).map(|i| format!("E o{i} E")).collect();
    alternatives.push("id".to_string());
    format!("E -> {}\n", alternatives.join(" | "))
}

/// The layered-precedence family of `tests/perf/gen_grammars.py`, at any size:
/// `(productions - 2) / 2` levels of left-recursive binary operators.
fn layered(productions: usize) -> String {
    let levels = (productions - 2) / 2;
    let mut text = String::new();
    for i in 1..=levels {
        let tighter = if i < levels { format!("E{:02}", i + 1) } else { "atom".to_string() };
        text.push_str(&format!("E{i:02} -> E{i:02} op{i:02} {tighter} | {tighter}\n"));
    }
    text.push_str("atom -> ( E01 ) | id\n");
    text
}

fn project_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or_else(|| panic!("engine/ must have a parent directory"))
        .to_path_buf()
}

// ---------------------------------------------------------------------------
// What may be read
// ---------------------------------------------------------------------------

#[test]
fn max_source_chars() {
    let head = "S -> a\n// ";
    let at_limit = format!("{head}{}", "x".repeat(MAX_SOURCE_CHARS - head.chars().count()));
    assert_eq!(at_limit.chars().count(), MAX_SOURCE_CHARS);
    analysed("lr0", &at_limit, "a");

    let e = refused_by_all(&format!("{at_limit}x"), "");
    assert_eq!(
        message(&e),
        format!(
            "The grammar text is {} characters, over the {MAX_SOURCE_CHARS} character limit.",
            MAX_SOURCE_CHARS + 1
        )
    );
    assert!(!hint(&e).is_empty());
}

#[test]
fn max_productions() {
    analysed("lr0", &distinct_productions(MAX_PRODUCTIONS), "abcd");

    let e = refused_by_all(&distinct_productions(MAX_PRODUCTIONS + 1), "");
    assert_eq!(
        message(&e),
        format!("This grammar has more than {MAX_PRODUCTIONS} productions once `|` alternatives are expanded.")
    );
    assert_eq!(e["line"], MAX_PRODUCTIONS + 1, "the line that crossed it");
    assert!(!hint(&e).is_empty());
}

#[test]
fn max_rhs_len() {
    // Exactly at the limit is an ordinary, if long, production.
    let at_limit = format!("S -> {}", "a ".repeat(MAX_RHS_LEN));
    for variant in VARIANTS {
        let v = analysed(variant, &at_limit, "");
        assert_eq!(v["table"]["states"], MAX_RHS_LEN + 2, "{variant}: one state per symbol read");
    }

    let e = refused_by_all(&format!("S -> b\nS -> {}", "a ".repeat(MAX_RHS_LEN + 1)), "");
    assert_eq!(
        message(&e),
        format!(
            "A right-hand side of `S` has {} symbols, over the {MAX_RHS_LEN} symbol limit for one production.",
            MAX_RHS_LEN + 1
        )
    );
    assert_eq!(e["line"], 2);
    assert!(hint(&e).contains("non-terminal"), "the hint says how to split it: {}", hint(&e));

    // The limit is per production, not per line: the same symbols spread over
    // alternatives are fine.
    let spread = format!("S -> {} | {}", "a ".repeat(MAX_RHS_LEN), "b ".repeat(MAX_RHS_LEN));
    analysed("lr0", &spread, "");

    // A 20,000-character production is refused at once, before any quadratic pass.
    let huge = format!("S -> {}", "a ".repeat(9_900));
    assert!(huge.chars().count() <= MAX_SOURCE_CHARS);
    let e = refused_by_all(&huge, "");
    assert!(message(&e).contains("has 9900 symbols"), "got: {}", message(&e));
}

#[test]
fn max_symbols() {
    analysed("lr0", &distinct_terminals(MAX_SYMBOLS - 2), "");

    let e = refused_by_all(&distinct_terminals(MAX_SYMBOLS - 1), "");
    assert_eq!(
        message(&e),
        format!(
            "This grammar uses {} distinct symbols, over the {MAX_SYMBOLS} symbol limit.",
            MAX_SYMBOLS + 1
        )
    );
    assert!(hint(&e).contains("tokenization mode"));
}

#[test]
fn max_symbol_len() {
    let fits = "x".repeat(MAX_SYMBOL_LEN);
    assert!(accepted(&analysed("lr0", &format!("S -> {fits} y"), &format!("{fits} y"))));

    let long = "x".repeat(MAX_SYMBOL_LEN + 1);
    let e = refused_by_all(&format!("S -> a\nS -> {long} y"), "");
    assert_eq!(
        message(&e),
        format!(
            "The symbol `{long}` is {} characters long, over the {MAX_SYMBOL_LEN} character limit.",
            MAX_SYMBOL_LEN + 1
        )
    );
    assert_eq!(e["line"], 2);
    assert!(hint(&e).contains("spaces between them"), "got: {}", hint(&e));
}

#[test]
fn max_warnings() {
    // Every repeat of a production is worth one warning. Exactly as many as
    // the list holds: all of them, and no closing line.
    let v = analysed("lr0", &"S -> a\n".repeat(MAX_WARNINGS + 1), "a");
    let w = warning_messages(&v);
    assert_eq!(w.len(), MAX_WARNINGS);
    assert!(w.iter().all(|m| m.starts_with("Duplicate production")), "{w:?}");

    // One more, and the list closes with a line that counts what it left out.
    let v = analysed("lr0", &"S -> a\n".repeat(MAX_WARNINGS + 2), "a");
    let w = warning_messages(&v);
    assert_eq!(w.len(), MAX_WARNINGS + 1);
    assert_eq!(
        w.last().map(String::as_str),
        Some(
            format!("+1 more warning not shown. Only the first {MAX_WARNINGS} are listed; deal with those and analyse again to see the rest.")
                .as_str()
        )
    );
    assert_eq!(v["warnings"][MAX_WARNINGS]["severity"], "warning");
    assert!(v["warnings"][MAX_WARNINGS]["line"].is_null());

    // The cap is on what is listed, never on the analysis: the grammar is the
    // same one production either way, and the same under every method.
    for variant in VARIANTS {
        let v = analysed(variant, &"S -> a\n".repeat(500), "a");
        assert_eq!(warning_messages(&v).len(), MAX_WARNINGS + 1, "{variant}");
        assert_eq!(v["warnings"][MAX_WARNINGS]["message"].as_str().map(|m| m.starts_with("+449 more warnings")), Some(true));
        assert_eq!(v["verdict"]["isValid"], Value::Bool(true), "{variant}");
    }
}

// ---------------------------------------------------------------------------
// What may be built and shown
// ---------------------------------------------------------------------------

#[test]
fn max_states() {
    // Nine chains: 452 states, under the cap, under every method.
    for variant in VARIANTS {
        let v = analysed(variant, &chains(9), "");
        assert_eq!(v["table"]["states"], 9 * MAX_RHS_LEN + 2, "{variant}");
    }

    // Eleven chains would be 552.
    const _: () = assert!(11 * MAX_RHS_LEN + 2 > MAX_STATES);
    let e = refused_by_all(&chains(11), "");
    assert_eq!(
        message(&e),
        format!("This grammar produces more than {MAX_STATES} states. That is too large to display usefully - try a smaller grammar.")
    );
    assert!(!hint(&e).is_empty());
}

#[test]
fn max_steps() {
    // 150 alternatives: LR(0) fills every column, 22,650 reduce cells, one step each.
    let alternatives: Vec<String> = (1..=150).map(|i| format!("a{i}")).collect();
    let text = format!("S -> {}", alternatives.join(" | "));

    let e = refused("lr0", &text, "");
    assert_eq!(message(&e), format!("This analysis needs more than {MAX_STEPS} steps to show."));
    assert!(!hint(&e).is_empty());

    // SLR(1) reduces only under `$`, so the same grammar fits; the comparison omits
    // the one method whose table could not be shown.
    for variant in ["slr1", "lalr1", "clr1"] {
        let v = analysed(variant, &text, "a7");
        assert!(accepted(&v), "{variant}");
        assert_eq!(rows(&v), vec!["slr1", "lalr1", "clr1"], "{variant}");
        assert!(v["steps"].as_array().map_or(0, Vec::len) < MAX_STEPS);
    }
}

#[test]
fn max_test_symbols() {
    let at_limit = "a ".repeat(MAX_TEST_SYMBOLS);
    let v = analysed("lr0", "S -> S a | a", &at_limit);
    assert!(accepted(&v));

    let e = refused_by_all("S -> S a | a", &"a ".repeat(MAX_TEST_SYMBOLS + 1));
    assert_eq!(
        message(&e),
        format!(
            "The test string is {} symbols long, over the {MAX_TEST_SYMBOLS} symbol limit.",
            MAX_TEST_SYMBOLS + 1
        )
    );
    assert!(hint(&e).contains("shorter string"), "got: {}", hint(&e));

    // Counted in symbols, the way the grammar is read: one long word is one
    // symbol, and it is refused for being unknown, not for being long.
    let e = refused("lr0", "S -> id S | id", &"x".repeat(5_000));
    assert!(message(&e).ends_with("is not a terminal in this grammar. Terminals are: id."), "got: {}", message(&e));
}

#[test]
fn max_parse_steps() {
    // Four unit reductions under every `a`: six steps a symbol. The grammar is LR(0);
    // the parse is just long.
    let chain = "S -> S T | T\nT -> U\nU -> V\nV -> W\nW -> a";
    let fits = (MAX_PARSE_STEPS - 2) / 6;
    for variant in VARIANTS {
        let v = analysed(variant, chain, &"a".repeat(fits));
        assert_eq!(v["verdict"]["isValid"], Value::Bool(true), "{variant}");
        assert!(accepted(&v), "{variant}");
        assert_eq!(parse_steps(&v), 6 * fits + 2, "{variant}");
        assert!(parse_steps(&v) <= MAX_PARSE_STEPS);
    }

    let e = refused_by_all(chain, &"a".repeat(fits + 1));
    assert_eq!(message(&e), format!("The string parse did not finish within {MAX_PARSE_STEPS} steps."));
    // No conflicts, and the hint says so instead of sending the reader to look for one.
    assert!(hint(&e).contains("no conflicts"), "got: {}", hint(&e));
    assert!(hint(&e).contains("unit productions"), "got: {}", hint(&e));
    assert!(!hint(&e).contains("Parsing Table tab"), "got: {}", hint(&e));

    // With a conflict in the table the engine cannot know it played no part, so the
    // hint offers both explanations.
    let conflicted = "S -> S T | T\nT -> U\nU -> V\nV -> W\nW -> a | X | Y\nX -> b\nY -> b";
    for variant in VARIANTS {
        let v = analysed(variant, conflicted, &"a".repeat(fits));
        assert_eq!(v["verdict"]["isValid"], Value::Bool(false), "{variant}");
        assert!(accepted(&v), "{variant}");
    }
    let e = refused_by_all(conflicted, &"a".repeat(fits + 1));
    assert_eq!(message(&e), format!("The string parse did not finish within {MAX_PARSE_STEPS} steps."));
    assert!(hint(&e).contains("has conflicts"), "got: {}", hint(&e));
    assert!(hint(&e).contains("Parsing Table tab"), "got: {}", hint(&e));
    assert!(hint(&e).contains("try a shorter one"), "got: {}", hint(&e));
}

#[test]
fn max_parse_stack() {
    // Under LR(0) `A -> ε` is reduced in every column and loops forever, pushing an
    // `A` each time; the stack-depth cap stops it long before the step cap.
    let looping = "S -> A S b | c\nA -> #";
    let e = refused("lr0", looping, "b");
    assert_eq!(message(&e), format!("The parse stack grew past {MAX_PARSE_STACK} entries."));
    assert!(hint(&e).contains("has conflicts"), "got: {}", hint(&e));
    assert!(hint(&e).contains("Parsing Table tab"), "got: {}", hint(&e));

    // SLR(1) does not reduce `A -> ε` under `b`: an ordinary rejection, so the loop was
    // the conflict's.
    let v = analysed("slr1", looping, "b");
    assert!(!accepted(&v));
    assert!(parse_steps(&v) < 10);

    // A legitimately deep stack: two empty symbols before every `b` in a
    // right-recursive list leave three entries per `b`. The grammar is SLR(1).
    let piling = "S -> X X b S | a\nX -> #";
    let string = |count: usize| format!("{}a", "b".repeat(count));
    let fits = (MAX_PARSE_STACK - 2) / 3;
    for variant in ["slr1", "lalr1", "clr1"] {
        // Inside the cap it is an ordinary parse.
        let v = analysed(variant, piling, &string(fits - 30));
        assert_eq!(v["verdict"]["isValid"], Value::Bool(true), "{variant}");
        assert!(accepted(&v), "{variant}");
        assert!(parse_steps(&v) > 1_000 && parse_steps(&v) <= MAX_PARSE_STEPS, "{variant}: {}", parse_steps(&v));

        let e = refused(variant, piling, &string(fits + 8));
        assert_eq!(message(&e), format!("The parse stack grew past {MAX_PARSE_STACK} entries."), "{variant}");
        assert!(hint(&e).contains("no conflicts"), "{variant}: {}", hint(&e));
        assert!(hint(&e).contains("ε-productions"), "{variant}: {}", hint(&e));
        assert!(!hint(&e).contains("Parsing Table tab"), "{variant}: {}", hint(&e));
    }
}

#[test]
fn max_fixpoint_rounds() {
    // A valve no grammar can reach: FIRST, FOLLOW and closure settle in at most one
    // round per symbol or item.
    const _: () = assert!(MAX_FIXPOINT_ROUNDS > 10 * (MAX_SYMBOLS + MAX_PRODUCTIONS));

    // What can be tested is that it never cuts a legal fixpoint short: a unit chain
    // as long as the symbol limit allows.
    let length = MAX_SYMBOLS - 3; // N1..Nn, the terminal `t`, and the augmented start
    let link = |i: usize| -> String {
        if i < length {
            format!("N{i} -> N{}\n", i + 1)
        } else {
            format!("N{i} -> t\n")
        }
    };
    // Top down, FIRST learns of `t` one link per round, from the bottom up.
    let top_down: String = (1..=length).map(link).collect();
    // With the start first and the rest bottom up, FOLLOW is the slow one.
    let bottom_up: String =
        std::iter::once(1).chain((2..=length).rev()).map(link).collect();

    for text in [&top_down, &bottom_up] {
        for variant in VARIANTS {
            let v = analysed(variant, text, "t");
            assert!(accepted(&v), "{variant}");
            let steps = v["steps"].as_array().cloned().unwrap_or_default();
            let table = |kind: &str| -> Vec<Value> {
                steps
                    .iter()
                    .find(|s| s["kind"] == kind)
                    .and_then(|s| s["table"].as_array().cloned())
                    .unwrap_or_default()
            };
            // Every non-terminal, the augmented start included, begins with
            // `t` and is followed by `$`: the fact reached the far end.
            let first = table("firstDone");
            let follow = table("followDone");
            assert_eq!(first.len(), length + 1, "{variant}");
            assert_eq!(follow.len(), length + 1, "{variant}");
            for row in &first {
                assert_eq!(row[1], serde_json::json!(["t"]), "{variant}: FIRST({})", row[0]);
            }
            for row in &follow {
                assert_eq!(row[1], serde_json::json!(["$"]), "{variant}: FOLLOW({})", row[0]);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// What may be spent
// ---------------------------------------------------------------------------

#[test]
fn max_work() {
    // The collection: a hundred productions for one non-terminal, each reachable with
    // every operator as a lookahead. The work budget stops it before the step cap.
    let e = refused("clr1", &ambiguous(99), "");
    assert_eq!(
        message(&e),
        format!("Building the item sets for this grammar takes more than {MAX_WORK} units of work, over the work limit.")
    );
    assert!(hint(&e).contains("LR(0) or SLR(1)"), "the hint names a way out: {}", hint(&e));
    assert!(e["line"].is_null());
    // LALR(1) builds the same collection first, so it is refused the same way.
    assert_eq!(refused("lalr1", &ambiguous(99), ""), e);

    // FIRST and FOLLOW, before any item set exists: one nullable non-terminal with 99
    // terminals in FIRST and 94 long productions. Refused the same under every method.
    let terminals: Vec<String> = (1..100).map(|i| format!("t{i}")).collect();
    let mut bomb = String::from("S -> ");
    let productions: Vec<String> =
        (1..95).map(|k| format!("{}u{k}", "B ".repeat(MAX_RHS_LEN - 1))).collect();
    bomb.push_str(&productions.join(" | "));
    bomb.push_str(&format!("\nB -> {} | eps\n", terminals.join(" | ")));
    assert!(bomb.chars().count() <= MAX_SOURCE_CHARS);
    let sets = refused("lr0", &bomb, "");
    assert_eq!(
        message(&sets),
        format!("Computing FIRST and FOLLOW for this grammar takes more than {MAX_WORK} units of work, over the work limit.")
    );
    // This time another method would not help, and the hint does not offer one.
    assert!(hint(&sets).contains("Every method computes these sets"), "got: {}", hint(&sets));
    assert!(hint(&sets).contains("shorter productions"), "got: {}", hint(&sets));
    if RELEASE {
        for variant in &VARIANTS[1..] {
            assert_eq!(refused(variant, &bomb, ""), sets, "{variant}");
        }
    }
}

#[test]
fn max_comparison_work() {
    // No error names this one: a comparison row that cannot be afforded is left out
    // and the requested analysis goes ahead. The CLR(1) collection costs more than a
    // comparison may spend and less than an analysis may.
    let text = ambiguous(67);

    // Asked for LR(0) or SLR(1): answered, without the two rows that would
    // have needed that collection.
    for variant in ["lr0", "slr1"] {
        let v = analysed(variant, &text, "");
        assert_eq!(rows(&v), vec!["lr0", "slr1"], "{variant}");
        // Nothing may be said about a method that was not run.
        let comparison = v["steps"]
            .as_array()
            .and_then(|s| s.iter().find(|s| s["kind"] == "variantComparison").cloned())
            .unwrap_or(Value::Null);
        let explain = comparison["explain"].as_str().unwrap_or("");
        assert!(!explain.contains("CLR(1)") && !explain.contains("LALR(1)"), "got: {explain}");
        assert!(!v["verdict"]["message"].as_str().unwrap_or("").contains("try that"));
        if !RELEASE {
            break; // one is enough to show it; the second costs five seconds unoptimized
        }
    }

    // Asked for CLR(1) itself, the same collection has the whole budget and all four
    // rows are there (release only: it is the dearest grammar still answered).
    if RELEASE {
        let (v, _) = timed("clr1", &text, "");
        assert_eq!(v["ok"], Value::Bool(true), "{}", v["error"]);
        assert_eq!(rows(&v), VARIANTS.to_vec());
    }

    // The collection really is between the two budgets.
    let parsed = grammar::parse(&text).unwrap_or_else(|e| panic!("{}", e.message));
    let mut rec = Recorder::new();
    let s = sets::compute(&parsed.grammar, &mut rec).unwrap_or_else(|e| panic!("{}", e.message));
    let mut rec = Recorder::new();
    automaton::build(&parsed.grammar, ItemMode::Lr1(&s), &mut rec)
        .unwrap_or_else(|e| panic!("{}", e.message));
    let spent = rec.work().spent();
    assert!(spent > MAX_COMPARISON_WORK && spent < MAX_WORK, "CLR(1) collection cost {spent}");
}

// ---------------------------------------------------------------------------
// Time
// ---------------------------------------------------------------------------

/// A 60-production grammar under a kilobyte: every method builds the CLR(1) collection
/// for the comparison, so it must end, with a result or a clean error, in under a second.
#[test]
fn the_sixty_production_grammar_that_froze_the_tab_is_answered_in_under_a_second() {
    let text = ambiguous(59);
    assert_eq!(text.len(), 589);

    // A debug build tries one variant; a release build tries all four, best of three,
    // so one unlucky schedule is not mistaken for a slow engine.
    let variants: &[&str] = if RELEASE { &VARIANTS } else { &["lr0"] };
    for variant in variants {
        let mut best: Option<(Value, Duration)> = None;
        for _ in 0..if RELEASE { 3 } else { 1 } {
            let (v, took) = timed(variant, &text, "");
            eprintln!("60 productions, {variant}: {took:?}");
            let faster = match &best {
                Some((_, so_far)) => took < *so_far,
                None => true,
            };
            if faster {
                best = Some((v, took));
            }
        }
        let (v, took) = best.unwrap_or_else(|| panic!("no run"));
        assert!(took < budget(), "{variant}: {took:?}, over {:?}", budget());
        // It is in fact answered, in full: 121 states under every method, and
        // every method conflicts, which for this grammar is the right answer.
        assert_eq!(v["ok"], Value::Bool(true), "{variant}: {}", v["error"]);
        assert_eq!(v["table"]["states"], 121, "{variant}");
        assert_eq!(rows(&v), VARIANTS.to_vec(), "{variant}");
        assert_eq!(v["verdict"]["isValid"], Value::Bool(false), "{variant}");
    }
}

/// The layered-precedence family runs out of steps at 172 productions. The refusal
/// is the step cap, not the work budget, and it must arrive quickly.
#[test]
fn the_172_production_grammar_is_refused_by_the_step_cap_and_quickly() {
    let text = layered(172);
    let variants: &[&str] = if RELEASE { &VARIANTS } else { &["lr0", "clr1"] };
    for variant in variants {
        let (v, took) = timed(variant, &text, "");
        eprintln!("172 productions, {variant}: {took:?}");
        assert!(took < budget(), "{variant}: {took:?}, over {:?}", budget());
        assert_eq!(v["ok"], Value::Bool(false), "{variant}");
        assert_eq!(
            message(&v["error"]),
            format!("This analysis needs more than {MAX_STEPS} steps to show."),
            "{variant}"
        );
    }
}

/// The work budget is ten times what the largest corpus grammar spends under its
/// dearest method; this fails if a change eats that margin.
#[test]
fn the_work_budget_is_ten_times_what_the_corpus_needs() {
    let root = project_root();
    let mut files: Vec<PathBuf> = Vec::new();
    for dir in ["examples", "tests/perf/grammars"] {
        let entries = std::fs::read_dir(root.join(dir))
            .unwrap_or_else(|e| panic!("{dir} must exist and be readable: {e}"));
        files.extend(
            entries
                .filter_map(|entry| entry.ok().map(|e| e.path()))
                .filter(|p| p.extension().map(|e| e == "txt").unwrap_or(false)),
        );
    }
    files.sort();
    assert!(files.len() >= 18, "thirteen handouts and five corpus grammars, found {}", files.len());

    let mut dearest: (u64, String) = (0, String::new());
    for file in &files {
        let name = file.file_name().unwrap_or_default().to_string_lossy().to_string();
        let text = std::fs::read_to_string(file).unwrap_or_else(|e| panic!("{name}: {e}"));
        let parsed = grammar::parse(&text).unwrap_or_else(|e| panic!("{name}: {}", e.message));
        let g = &parsed.grammar;

        // The dearest analysis: LALR(1) computes the sets, builds the CLR(1)
        // collection and merges it, all on one budget.
        let mut rec = Recorder::new();
        let s = sets::compute(g, &mut rec).unwrap_or_else(|e| panic!("{name}: {}", e.message));
        automaton::build_lalr(g, &s, &mut rec).unwrap_or_else(|e| panic!("{name}: {}", e.message));
        let whole = rec.work().spent();
        assert!(
            whole.saturating_mul(10) <= MAX_WORK,
            "{name}: an LALR(1) analysis spends {whole} units, more than a tenth of MAX_WORK"
        );
        if whole > dearest.0 {
            dearest = (whole, name.clone());
        }

        // The dearest comparison build: the CLR(1) collection on its own.
        let mut rec = Recorder::for_comparison();
        let canonical = automaton::build(g, ItemMode::Lr1(&s), &mut rec)
            .unwrap_or_else(|e| panic!("{name}: {}", e.message));
        let collection = rec.work().spent();
        assert!(
            collection.saturating_mul(6) <= MAX_COMPARISON_WORK,
            "{name}: the CLR(1) collection spends {collection} units, more than a sixth of MAX_COMPARISON_WORK"
        );
        // ...and the merge of it, which is a comparison build of its own.
        let mut rec = Recorder::for_comparison();
        automaton::merge_by_core(g, &canonical, &mut rec)
            .unwrap_or_else(|e| panic!("{name}: {}", e.message));
        assert!(rec.work().spent().saturating_mul(6) <= MAX_COMPARISON_WORK, "{name}: the merge");
    }

    // The figures `limits.rs` quotes are these ones.
    assert_eq!(dearest.1, "gen-080.txt", "the largest grammar of the corpus is the 80-production one");
    assert!(
        (11_000_000..12_000_000).contains(&dearest.0),
        "gen-080 under LALR(1) spends {} units; limits.rs says 11.8 million",
        dearest.0
    );
}

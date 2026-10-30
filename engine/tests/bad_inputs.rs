//! Hostile and malformed grammar input, under every variant.
//!
//! `analyze()` must return valid JSON, never panic or hang, and name the problem and
//! its line when it refuses. Grammar errors are found before method-specific work,
//! so each is asserted identical under all four variants. Caps: see `limits.rs`.

use lr_engine::limits::{
    MAX_PARSE_STEPS, MAX_PRODUCTIONS, MAX_RHS_LEN, MAX_SOURCE_CHARS, MAX_SYMBOLS, MAX_SYMBOL_LEN,
    MAX_TEST_SYMBOLS, MAX_WARNINGS,
};
use serde_json::Value;
use std::time::{Duration, Instant};

const VARIANTS: [&str; 4] = ["lr0", "slr1", "lalr1", "clr1"];

/// The wall-clock budget for one `analyze()` call on a hostile input. Generous
/// for a debug build; a release WASM build is several times faster.
const BUDGET: Duration = Duration::from_secs(2);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// One call, checked for the two things that must hold of *every* input: the
/// result is valid JSON with a boolean `ok`, and it arrived within budget.
fn run(variant: &str, grammar: &str, test: &str) -> Value {
    let started = Instant::now();
    let json = lr_engine::analyze(grammar, variant, test);
    let took = started.elapsed();
    assert!(
        took < BUDGET,
        "{variant}: analyze took {took:?}, over the {BUDGET:?} budget (grammar starts {:?})",
        grammar.chars().take(40).collect::<String>()
    );
    let v: Value = serde_json::from_str(&json)
        .unwrap_or_else(|e| panic!("{variant}: analyze must return valid JSON: {e}"));
    assert!(v["ok"].is_boolean(), "{variant}: `ok` must be a boolean, got {}", v["ok"]);
    if v["ok"] == Value::Bool(true) {
        for s in v["steps"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
            assert!(
                !s["explain"].as_str().unwrap_or("").trim().is_empty(),
                "{variant}: step {} ({}) has no explanation",
                s["index"],
                s["kind"]
            );
        }
    } else {
        let message = v["error"]["message"].as_str().unwrap_or("");
        assert!(!message.trim().is_empty(), "{variant}: an error must say what went wrong");
    }
    v
}

/// The input must be refused, identically, under every variant. Returns the
/// `error` object.
fn err_all(grammar: &str, test: &str) -> Value {
    let mut first: Option<Value> = None;
    for variant in VARIANTS {
        let v = run(variant, grammar, test);
        assert_eq!(
            v["ok"],
            Value::Bool(false),
            "{variant}: expected an error, got success (grammar {:?})",
            grammar.chars().take(60).collect::<String>()
        );
        assert!(v.get("steps").is_none(), "{variant}: a refused input carries no steps");
        match &first {
            None => first = Some(v["error"].clone()),
            Some(e) => assert_eq!(&v["error"], e, "{variant}: the error depends on the method"),
        }
    }
    first.unwrap_or(Value::Null)
}

/// The input must be accepted under every variant. Returns the lr0 result,
/// whose `grammar` and `warnings` the others are asserted to share.
fn ok_all(grammar: &str, test: &str) -> Value {
    let mut first: Option<Value> = None;
    for variant in VARIANTS {
        let v = run(variant, grammar, test);
        assert_eq!(
            v["ok"],
            Value::Bool(true),
            "{variant}: expected success, got error: {}",
            v["error"]
        );
        match &first {
            None => first = Some(v),
            Some(f) => {
                assert_eq!(v["grammar"], f["grammar"], "{variant}: grammar payload differs");
                assert_eq!(v["warnings"], f["warnings"], "{variant}: warnings differ");
            }
        }
    }
    first.unwrap_or(Value::Null)
}

fn message(e: &Value) -> &str {
    e["message"].as_str().unwrap_or("")
}

fn hint(e: &Value) -> &str {
    e["hint"].as_str().unwrap_or("")
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

/// `(severity, line, message)` for every warning.
fn warnings(v: &Value) -> Vec<(String, Option<u64>, String)> {
    v["warnings"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|w| {
                    (
                        w["severity"].as_str().unwrap_or("").to_string(),
                        w["line"].as_u64(),
                        w["message"].as_str().unwrap_or("").to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn accepted(v: &Value) -> bool {
    v["steps"]
        .as_array()
        .is_some_and(|s| s.iter().any(|s| s["kind"] == "parseAccept"))
}

// ---------------------------------------------------------------------------
// Empty and shapeless input
// ---------------------------------------------------------------------------

#[test]
fn an_empty_box_is_refused_with_a_hint() {
    // Blank lines, tabs, CRLF and `//` comments all count as nothing.
    for text in ["", "\n\n\n", "   \t  ", "\r\n\r\n", "// only a comment", "  // one\n\n// two\n"] {
        let e = err_all(text, "");
        assert_eq!(message(&e), "The grammar is empty.", "for {text:?}");
        assert!(hint(&e).contains("->"), "the hint shows a production: {}", hint(&e));
        assert!(e["line"].is_null(), "an empty grammar has no line to point at");
    }
}

#[test]
fn a_line_with_no_arrow_is_named_by_number_and_text() {
    // Blank and comment lines still count, so the number is the editor's.
    let e = err_all("S -> a S\n\n// the next line is the typo\nS b", "");
    assert_eq!(e["line"], 4);
    assert!(message(&e).contains("No arrow"), "got: {}", message(&e));
    assert!(message(&e).contains("`S b`"), "the offending text is quoted: {}", message(&e));
    assert!(hint(&e).contains("->") && hint(&e).contains("::="), "got: {}", hint(&e));

    // CRLF line endings: same line number, and no stray `\r` in the quote.
    let e = err_all("S -> a\r\nS b\r\n", "");
    assert_eq!(e["line"], 2);
    assert!(message(&e).contains("`S b`") && !message(&e).contains('\r'));
}

#[test]
fn nothing_left_of_the_arrow_is_named_by_line() {
    let e = err_all("S -> a\n-> b", "");
    assert_eq!(e["line"], 2);
    assert!(message(&e).contains("Nothing on the left of the arrow"), "got: {}", message(&e));
    assert!(!hint(&e).is_empty());
}

#[test]
fn an_epsilon_spelling_on_the_left_reads_as_an_empty_left_side() {
    // `eps`, `#` and `ε` are epsilon wherever they stand, so a production that
    // tries to define one is told its left-hand side is empty. Pinned as is.
    for text in ["eps -> a", "# -> a", "ε -> a"] {
        let e = err_all(text, "");
        assert_eq!(e["line"], 1, "for {text:?}");
        assert_eq!(message(&e), "The left-hand side is empty.", "for {text:?}");
        assert!(!hint(&e).is_empty());
    }
}

#[test]
fn an_unknown_variant_is_refused_by_name() {
    let v = run("lr2", "S -> a", "");
    assert_eq!(v["ok"], Value::Bool(false));
    assert!(message(&v["error"]).contains("`lr2`"), "got: {}", message(&v["error"]));
    assert!(hint(&v["error"]).contains("lalr1"));
}

// ---------------------------------------------------------------------------
// Grammars that parse, but not as the author may have meant
// ---------------------------------------------------------------------------

#[test]
fn an_undefined_uppercase_symbol_is_a_terminal_with_an_advisory_warning() {
    // A symbol is a non-terminal iff some production defines it, so `B` is a
    // terminal (with a warning), not an error.
    let v = ok_all("S -> A B\nA -> a", "a B");
    assert!(strings(&v["grammar"]["terminals"]).contains(&"B".to_string()));
    assert!(!strings(&v["grammar"]["nonterminals"]).contains(&"B".to_string()));
    let w = warnings(&v);
    assert_eq!(w.len(), 1, "exactly one remark: {w:?}");
    let (severity, line, text) = &w[0];
    assert_eq!(severity, "warning");
    assert_eq!(*line, None);
    assert!(text.starts_with("`B` is used on a right-hand side but never defined"), "got: {text}");
    assert!(text.contains("treated as a terminal"), "got: {text}");
    // It really is a terminal: the test string may contain it.
    assert!(accepted(&v), "`a B` is a sentence of the grammar as read");
}

#[test]
fn an_undefined_lowercase_word_is_a_terminal_and_draws_no_warning() {
    // The lookalike check is deliberately narrow (single uppercase letters, in
    // a grammar that has single-uppercase non-terminals). `expr` here is a
    // terminal, silently - pinned so a widening of the heuristic is noticed.
    let v = ok_all("stmt -> expr ;", "expr ;");
    assert_eq!(v["grammar"]["mode"], "space-separated");
    assert_eq!(strings(&v["grammar"]["terminals"]), vec!["expr", ";"]);
    assert!(warnings(&v).is_empty(), "got: {:?}", warnings(&v));
    assert!(accepted(&v));
}

#[test]
fn a_duplicate_production_is_merged_with_a_warning_on_its_line() {
    for (text, line) in [("S -> a\nS -> a", 2), ("S -> a | a", 1)] {
        let v = ok_all(text, "a");
        // S' -> S and one copy of S -> a: the duplicate is gone.
        assert_eq!(v["grammar"]["productions"].as_array().map(Vec::len), Some(2), "{text:?}");
        let w = warnings(&v);
        assert_eq!(w.len(), 1, "{text:?}: {w:?}");
        assert_eq!(w[0].0, "warning");
        assert_eq!(w[0].1, Some(line), "{text:?}: the warning names the repeat's line");
        assert!(w[0].2.contains("Duplicate production `S → a`"), "got: {}", w[0].2);
        // And because it was merged, the typo is not a reduce-reduce conflict.
        assert_eq!(v["verdict"]["isValid"], Value::Bool(true), "{text:?}");
    }
}

#[test]
fn an_unreachable_nonterminal_is_a_warning_and_is_kept() {
    let v = ok_all("S -> a\nA -> b", "a");
    let w = warnings(&v);
    assert_eq!(w.len(), 1, "{w:?}");
    assert_eq!(w[0].0, "warning");
    assert_eq!(w[0].1, None);
    assert_eq!(
        w[0].2,
        "`A` cannot be reached from the start symbol `S`, so it plays no part in the analysis."
    );
    // Not dropped: it is still a non-terminal with its production.
    assert!(strings(&v["grammar"]["nonterminals"]).contains(&"A".to_string()));
    assert_eq!(v["grammar"]["productions"].as_array().map(Vec::len), Some(3));
}

#[test]
fn the_end_marker_and_dead_grammars_are_errors_with_lines() {
    let e = err_all("S -> a\nS -> $", "");
    assert_eq!(e["line"], 2);
    assert!(message(&e).contains("`$` is reserved"), "got: {}", message(&e));
    assert!(!hint(&e).is_empty());

    // A cycle is reported as a cycle, before productivity.
    let e = err_all("S -> A\nA -> S", "");
    assert!(message(&e).contains("cycle"), "got: {}", message(&e));
    assert!(message(&e).contains("S => A => S") || message(&e).contains("A => S => A"));

    // A non-terminal with no way out names itself and its line.
    let e = err_all("S -> a\nS -> B\nB -> B b", "");
    assert_eq!(e["line"], 3);
    assert!(message(&e).starts_with("`B` can never derive a string of terminals"));
}

#[test]
fn an_undefined_primed_symbol_is_a_terminal_like_any_other() {
    // `S'` is defined by no production, so it is a terminal. The augmented start
    // takes the first primed name the grammar does not use, so it cannot clash.
    let v = ok_all("S -> a S'", "a S'");
    assert!(strings(&v["grammar"]["terminals"]).contains(&"S'".to_string()));
    assert_eq!(v["grammar"]["start"], "S''", "one more prime, and it is free");
    assert_eq!(v["grammar"]["productions"][0]["lhs"], "S''");
    assert!(accepted(&v));
}

#[test]
fn the_augmented_start_keeps_adding_primes_until_the_name_is_free() {
    // Nothing clashes: exactly the name it has always had.
    let v = ok_all("S -> a", "a");
    assert_eq!(v["grammar"]["start"], "S'");

    // A non-terminal already called `S'`, and a terminal called `S''`: both
    // are taken, whichever kind of symbol took them.
    let v = ok_all("S -> S' | S''\nS' -> a", "a");
    assert_eq!(v["grammar"]["start"], "S'''");
    assert!(strings(&v["grammar"]["terminals"]).contains(&"S''".to_string()));
    assert!(strings(&v["grammar"]["nonterminals"]).contains(&"S'".to_string()));
    // The new symbol is on no right-hand side, which is the point of it.
    for p in v["grammar"]["productions"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
        assert!(!strings(&p["rhs"]).contains(&"S'''".to_string()), "{p}");
    }
    assert!(accepted(&v));
}

#[test]
fn a_long_nullable_production_is_refused_within_budget() {
    // One production of 9,990 nullable symbols is under the character limit but over
    // `MAX_RHS_LEN`: refused as read, before any quadratic pass. `err_all` holds the time.
    let text = format!("S -> {}| #", "S ".repeat(9_990));
    assert!(text.chars().count() <= MAX_SOURCE_CHARS);
    let e = err_all(&text, "");
    assert_eq!(
        message(&e),
        format!("A right-hand side of `S` has 9990 symbols, over the {MAX_RHS_LEN} symbol limit for one production.")
    );
    assert_eq!(e["line"], 1);
    assert!(hint(&e).contains("non-terminal"), "got: {}", hint(&e));

    // The cycle itself is still reported as a cycle when the production is
    // short enough to be read.
    let short = format!("S -> {}| #", "S ".repeat(MAX_RHS_LEN));
    let e = err_all(&short, "");
    assert!(message(&e).contains("cycle: S => S"), "got: {}", message(&e));
}

// ---------------------------------------------------------------------------
// Unicode, invisible and bidi characters in symbol names
// ---------------------------------------------------------------------------
// - Greek, CJK and emoji are ordinary symbol characters; a multi-codepoint emoji
//   flips the grammar to space-separated mode. Text round-trips byte for byte.
// - U+202E, U+200B and U+FEFF are not whitespace, so inside a token they are part
//   of the symbol, with no warning. The frontend uses `textContent`, so nothing runs.
// - One leading U+FEFF is a byte-order mark and is ignored; anywhere else it is
//   symbol text.

#[test]
fn greek_cjk_and_emoji_are_ordinary_symbols_that_round_trip() {
    // Single-character mode: each Greek letter is its own terminal.
    let v = ok_all("S -> α S β | γ", "α γ β");
    assert_eq!(v["grammar"]["mode"], "single-char");
    assert_eq!(strings(&v["grammar"]["terminals"]), vec!["α", "β", "γ"]);
    assert!(accepted(&v));

    // Space-separated CJK, including on the left-hand side.
    let v = ok_all("句子 -> 名词 动词\n名词 -> 猫 | 狗", "狗 动词");
    assert_eq!(v["grammar"]["mode"], "space-separated");
    assert_eq!(v["grammar"]["originalStart"], "句子");
    assert_eq!(v["grammar"]["start"], "句子'");
    assert_eq!(strings(&v["grammar"]["terminals"]), vec!["动词", "猫", "狗"]);
    assert!(accepted(&v));

    // Single-codepoint emoji stay in single-character mode.
    let v = ok_all("S -> 😀 S 🎉 | 🙂", "😀🙂🎉");
    assert_eq!(v["grammar"]["mode"], "single-char");
    assert_eq!(strings(&v["grammar"]["terminals"]), vec!["😀", "🎉", "🙂"]);
    assert!(accepted(&v));

    // A ZWJ family and a flag are several codepoints each: one symbol apiece,
    // space-separated mode, bytes intact.
    let family = "👨\u{200D}👩\u{200D}👧";
    let flag = "🇳🇿";
    let v = ok_all(&format!("S -> {family} {flag}"), &format!("{family} {flag}"));
    assert_eq!(v["grammar"]["mode"], "space-separated");
    assert_eq!(strings(&v["grammar"]["terminals"]), vec![family, flag]);
    assert!(accepted(&v));
}

#[test]
fn bidi_and_zero_width_characters_inside_a_symbol_are_kept_byte_exact() {
    for (name, ch) in [
        ("RIGHT-TO-LEFT OVERRIDE", '\u{202E}'),
        ("ZERO WIDTH SPACE", '\u{200B}'),
        ("ZERO WIDTH NO-BREAK SPACE", '\u{FEFF}'),
        ("NUL", '\u{0}'),
    ] {
        let sym = format!("a{ch}b");
        let v = ok_all(&format!("S -> {sym} c"), &format!("{sym} c"));
        assert_eq!(v["grammar"]["mode"], "space-separated", "{name}");
        assert_eq!(
            strings(&v["grammar"]["terminals"]),
            vec![sym.clone(), "c".to_string()],
            "{name}: the symbol is the three chars as written"
        );
        assert!(warnings(&v).is_empty(), "{name}: no warning today: {:?}", warnings(&v));
        assert!(accepted(&v), "{name}: the same bytes in the test string match");

        // Without the invisible character the test string is a different symbol,
        // and the error says so rather than silently rejecting.
        let e = err_all(&format!("S -> {sym} c"), "ab c");
        assert!(message(&e).starts_with("`ab` is not a terminal"), "{name}: {}", message(&e));
    }
}

#[test]
fn only_one_leading_byte_order_mark_is_ignored() {
    // The first U+FEFF is a byte-order mark and is dropped; a second is symbol text
    // that joins the start symbol's name, making the right-hand `S` a terminal.
    let v = ok_all("\u{FEFF}\u{FEFF}S -> a S b | c", "a S b");
    assert_eq!(v["grammar"]["mode"], "space-separated");
    assert_eq!(v["grammar"]["originalStart"], "\u{FEFF}S");
    assert_eq!(strings(&v["grammar"]["terminals"]), vec!["a", "S", "b", "c"]);
    assert!(accepted(&v), "`a S b` - with a terminal `S` - is what it accepts");

    // A mark that is not at the very start is not a mark either.
    let v = ok_all("// comment\n\u{FEFF}S -> a", "a");
    assert_eq!(v["grammar"]["originalStart"], "\u{FEFF}S");

    // And a text that is nothing but a mark is an empty grammar.
    let e = err_all("\u{FEFF}", "");
    assert_eq!(message(&e), "The grammar is empty.");
}

#[test]
fn a_leading_byte_order_mark_does_not_change_the_grammar() {
    // One leading mark is ignored: the whole result is the one the plain text gets.
    for body in ["S -> a S b | c", "E -> E + T | T\nT -> id"] {
        let with_bom = format!("\u{FEFF}{body}");
        for variant in VARIANTS {
            let plain = run(variant, body, "");
            let marked = run(variant, &with_bom, "");
            assert_eq!(marked["ok"], Value::Bool(true), "{variant}: {}", marked["error"]);
            assert_eq!(marked, plain, "{variant}: {body:?}");
        }
    }

    // The mark sits before line 1, so line numbers do not move.
    let body = "S -> a S\n\n// the next line is the typo\nS b";
    let plain = err_all(body, "");
    let marked = err_all(&format!("\u{FEFF}{body}"), "");
    assert_eq!(plain["line"], 4);
    assert_eq!(marked, plain);

    // And it does not count towards the character limit.
    let head = "S -> a\n// ";
    let at_limit = format!("\u{FEFF}{head}{}", "x".repeat(MAX_SOURCE_CHARS - head.chars().count()));
    assert_eq!(at_limit.chars().count(), MAX_SOURCE_CHARS + 1);
    ok_all(&at_limit, "a");
}

// ---------------------------------------------------------------------------
// Markup as symbol text
// ---------------------------------------------------------------------------

#[test]
fn markup_in_a_symbol_is_kept_as_literal_text() {
    // The engine does not escape and must not: it hands symbol text to a
    // frontend that only ever uses `textContent`. What it owes is that the text
    // arrives literally, so the browser shows `<script>` rather than running it.
    let cases: [(&str, &[&str]); 3] = [
        ("S -> <script>alert(1)</script> | x", &["<script>alert(1)</script>", "x"]),
        ("S -> <img src=x onerror=alert(1)> | x", &["<img", "src=x", "onerror=alert(1)>", "x"]),
        ("S -> \"><svg onload=alert(1)> | x", &["\"><svg", "onload=alert(1)>", "x"]),
    ];
    for (grammar, expected) in cases {
        for variant in VARIANTS {
            let json = lr_engine::analyze(grammar, variant, "x");
            // `<`, `>` and `=` are not escaped by serde_json: the raw payload
            // carries the markup verbatim, which is the point being pinned.
            assert!(json.contains(expected[0].trim_start_matches('"')), "{variant}: {grammar}");
            let v = run(variant, grammar, "x");
            assert_eq!(v["ok"], Value::Bool(true), "{variant}: {}", v["error"]);
            assert_eq!(v["grammar"]["mode"], "space-separated");
            assert_eq!(strings(&v["grammar"]["terminals"]), *expected, "{variant}: {grammar}");
            assert!(accepted(&v), "{variant}: `x` is still a sentence");
            // The glued-token check notices the punctuation and asks.
            assert!(
                warnings(&v).iter().any(|(_, line, text)| *line == Some(1)
                    && text.starts_with("In space-separated mode,")
                    && text.contains(expected[0])),
                "{variant}: {:?}",
                warnings(&v)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Limits
// ---------------------------------------------------------------------------

#[test]
fn a_five_thousand_line_grammar_hits_the_character_limit_fast() {
    let repeated = "S -> a\n".repeat(5_000);
    let numbered: String = (1..=5_000).map(|i| format!("A{i} -> a\n")).collect();
    for text in [&repeated, &numbered] {
        assert!(text.chars().count() > MAX_SOURCE_CHARS);
        let started = Instant::now();
        let e = err_all(text, "");
        assert!(started.elapsed() < BUDGET, "all four refusals took {:?}", started.elapsed());
        assert_eq!(
            message(&e),
            format!(
                "The grammar text is {} characters, over the {MAX_SOURCE_CHARS} character limit.",
                text.chars().count()
            )
        );
        assert!(!hint(&e).is_empty());
        assert!(e["line"].is_null());
    }
}

#[test]
fn the_character_limit_counts_characters_not_bytes() {
    // Pad with a comment of two-byte characters: exactly at the limit is fine
    // although it is nearly twice that many bytes; one more is refused.
    let head = "S -> a\n// ";
    let pad = MAX_SOURCE_CHARS - head.chars().count();
    let at_limit = format!("{head}{}", "α".repeat(pad));
    assert_eq!(at_limit.chars().count(), MAX_SOURCE_CHARS);
    assert!(at_limit.len() > MAX_SOURCE_CHARS);
    ok_all(&at_limit, "a");

    let over = format!("{at_limit}α");
    let e = err_all(&over, "");
    assert!(message(&e).contains(&format!("{} characters", MAX_SOURCE_CHARS + 1)));
}

#[test]
fn thousands_of_lines_under_the_character_limit_stay_fast_and_count_lines() {
    // 5,000 comment lines then a typo: the line number is still exact.
    let text = format!("{}S -> a\nS a", "//\n".repeat(5_000));
    assert!(text.chars().count() <= MAX_SOURCE_CHARS);
    let e = err_all(&text, "");
    assert_eq!(e["line"], 5_002);

    // 2,800 copies of one production: merged into one. Each repeat is worth a
    // warning, but 2,799 boxes on the page help nobody: the first MAX_WARNINGS
    // are listed and one closing line counts the rest.
    let text = "S -> a\n".repeat(2_800);
    assert!(text.chars().count() <= MAX_SOURCE_CHARS);
    let v = ok_all(&text, "a");
    assert_eq!(v["grammar"]["productions"].as_array().map(Vec::len), Some(2));
    let w = warnings(&v);
    assert_eq!(w.len(), MAX_WARNINGS + 1);
    // The ones that are listed are the first ones, on their own lines.
    assert_eq!(w[0].1, Some(2));
    assert_eq!(w[MAX_WARNINGS - 1].1, Some(u64::try_from(MAX_WARNINGS).unwrap_or(0) + 1));
    assert_eq!(
        w[MAX_WARNINGS].2,
        format!(
            "+{} more warnings not shown. Only the first {MAX_WARNINGS} are listed; deal with those and analyse again to see the rest.",
            2_799 - MAX_WARNINGS
        )
    );
}

/// `count` distinct single-character productions `S -> w x y z` over `a`-`d`,
/// one per line - short enough that hundreds fit far under the character limit.
fn many_productions(count: usize) -> String {
    (0..count)
        .map(|i| {
            let rhs: Vec<&str> = (0..4)
                .map(|k| ["a", "b", "c", "d"].get((i >> (2 * k)) & 3).copied().unwrap_or("a"))
                .collect();
            format!("S -> {}\n", rhs.join(" "))
        })
        .collect()
}

#[test]
fn more_productions_than_the_limit_is_refused_on_the_line_that_crosses_it() {
    let v = ok_all(&many_productions(MAX_PRODUCTIONS), "abcd");
    assert_eq!(v["grammar"]["mode"], "single-char");

    let text = many_productions(MAX_PRODUCTIONS + 1);
    assert!(text.chars().count() < MAX_SOURCE_CHARS);
    let e = err_all(&text, "");
    assert_eq!(e["line"], MAX_PRODUCTIONS + 1);
    assert_eq!(
        message(&e),
        format!("This grammar has more than {MAX_PRODUCTIONS} productions once `|` alternatives are expanded.")
    );
    assert!(!hint(&e).is_empty());

    // The same count reached through `|` alternatives on one line.
    let alts: Vec<String> = many_productions(MAX_PRODUCTIONS + 1)
        .lines()
        .filter_map(|l| l.strip_prefix("S -> ").map(str::to_string))
        .collect();
    let e = err_all(&format!("// one long rule\nS -> {}", alts.join(" | ")), "");
    assert_eq!(e["line"], 2);
    assert!(message(&e).contains(&format!("more than {MAX_PRODUCTIONS} productions")));
}

#[test]
fn more_distinct_symbols_than_the_limit_is_refused() {
    // S' and S plus n terminals, forty to an alternative: a right-hand side
    // has a length limit of its own, so they cannot all go in one production.
    let with = |n: usize| -> String {
        let symbols: Vec<String> = (1..=n).map(|i| format!("t{i}")).collect();
        let alternatives: Vec<String> = symbols.chunks(40).map(|c| c.join(" ")).collect();
        format!("S -> {}", alternatives.join(" | "))
    };
    ok_all(&with(MAX_SYMBOLS - 2), "");

    let e = err_all(&with(MAX_SYMBOLS - 1), "");
    assert_eq!(
        message(&e),
        format!(
            "This grammar uses {} distinct symbols, over the {MAX_SYMBOLS} symbol limit.",
            MAX_SYMBOLS + 1
        )
    );
    assert!(hint(&e).contains("tokenization mode"));
    // Not attributed to a line: the inventory is a whole-grammar property.
    assert!(e["line"].is_null());
}

#[test]
fn a_symbol_longer_than_the_limit_is_refused_on_its_line() {
    let ok_name = "x".repeat(MAX_SYMBOL_LEN);
    ok_all(&format!("S -> {ok_name}"), &ok_name);

    let long = "x".repeat(MAX_SYMBOL_LEN + 1);
    for (text, line) in [
        (format!("S -> a T\nT -> b\nT -> {long} b"), 3),
        (format!("S -> a\n{long} -> b"), 2),
    ] {
        let e = err_all(&text, "");
        assert_eq!(e["line"], line, "{text}");
        assert_eq!(
            message(&e),
            format!(
                "The symbol `{long}` is {} characters long, over the {MAX_SYMBOL_LEN} character limit.",
                MAX_SYMBOL_LEN + 1
            )
        );
        // The hint says what usually causes it: a missing space.
        assert!(hint(&e).contains("spaces between them"), "got: {}", hint(&e));
    }

    // Counted in chars: 32 CJK characters (96 bytes) is still within it.
    let wide = "字".repeat(MAX_SYMBOL_LEN);
    ok_all(&format!("S -> {wide} x"), "");
}

// ---------------------------------------------------------------------------
// Test-string errors
// ---------------------------------------------------------------------------

#[test]
fn an_unknown_test_string_symbol_fails_the_analysis_and_names_it() {
    // Known and accepted: a typo in the test string fails the whole analysis,
    // not just the parse phase.
    let e = err_all("S -> a S b | c", "a c d b");
    assert_eq!(message(&e), "`d` is not a terminal in this grammar. Terminals are: a b c.");
    assert!(hint(&e).contains("single-char mode"), "got: {}", hint(&e));
    assert!(e["line"].is_null());

    // A non-terminal in the test string gets a hint saying exactly that.
    let e = err_all("S -> A b\nA -> a", "A b");
    assert!(message(&e).starts_with("`A` is not a terminal"));
    assert!(hint(&e).contains("`A` is a non-terminal"), "got: {}", hint(&e));

    // Space-separated grammar, single-char habit: `idid` is one unknown word.
    let e = err_all("E -> id + id", "idid + id");
    assert!(message(&e).starts_with("`idid` is not a terminal"));
    assert!(hint(&e).contains("space-separated mode"));
}

#[test]
fn a_test_string_over_the_symbol_limit_is_refused() {
    let e = err_all("S -> a S | a", &"a ".repeat(MAX_TEST_SYMBOLS + 1));
    assert_eq!(
        message(&e),
        format!(
            "The test string is {} symbols long, over the {MAX_TEST_SYMBOLS} symbol limit.",
            MAX_TEST_SYMBOLS + 1
        )
    );
    assert!(hint(&e).contains("shorter string"), "got: {}", hint(&e));
}

#[test]
fn a_test_string_at_the_symbol_limit_can_be_parsed() {
    // `S -> S a | a` is LR(0); n symbols take one shift and one reduce each,
    // `S -> S a | a` takes 2n + 2 parse steps; the cap must leave room for the longest
    // test string the tool accepts.
    const _: () = assert!(2 * MAX_TEST_SYMBOLS + 2 <= MAX_PARSE_STEPS);
    for variant in VARIANTS {
        let v = run(variant, "S -> S a | a", &"a".repeat(MAX_TEST_SYMBOLS));
        assert_eq!(v["ok"], Value::Bool(true), "{variant}: {}", v["error"]);
        assert!(accepted(&v), "{variant}");
        let parse = v["steps"]
            .as_array()
            .map_or(0, |s| s.iter().filter(|s| s["phase"] == "parse").count());
        assert_eq!(parse, 2 * MAX_TEST_SYMBOLS + 2, "{variant}");
    }

    // The textbook expression grammar at full length, at its dearest: every
    // operand is reduced through F, T and E. Still inside the cap under the
    // three methods that can parse it.
    let ids = vec!["id"; MAX_TEST_SYMBOLS / 2].join(" + ");
    for variant in ["slr1", "lalr1", "clr1"] {
        let v = run(variant, "E -> E + T | T\nT -> T * F | F\nF -> ( E ) | id", &ids);
        assert_eq!(v["ok"], Value::Bool(true), "{variant}: {}", v["error"]);
        assert!(accepted(&v), "{variant}");
    }
}

#[test]
fn a_bad_test_string_is_answered_before_the_grammar_is_analysed() {
    // Eleven chains of fifty symbols: over 500 states, so the analysis itself
    // is refused. A typo in the test string is the commoner mistake and the
    // cheaper one to check, so it is reported first, and by name.
    let grammar: String =
        (0..11).map(|i| format!("S -> c{i} {}\n", "a ".repeat(MAX_RHS_LEN - 1))).collect();
    let e = err_all(&grammar, "");
    assert!(message(&e).contains("more than 500 states"), "got: {}", message(&e));

    let e = err_all(&grammar, "c0 a oops");
    assert!(message(&e).starts_with("`oops` is not a terminal in this grammar."), "got: {}", message(&e));

    let e = err_all(&grammar, &"a ".repeat(MAX_TEST_SYMBOLS + 1));
    assert!(message(&e).starts_with("The test string is 501 symbols long"), "got: {}", message(&e));
}

// ---------------------------------------------------------------------------
// Garbage
// ---------------------------------------------------------------------------

/// A tiny deterministic generator, so "random" garbage is the same every run.
fn garbage(seed: u64, len: usize) -> String {
    const ALPHABET: &[char] = &[
        'S', 'A', 'a', 'b', ' ', ' ', '\n', '\t', '-', '>', '|', ':', '=', '→', 'ε', '#', '$',
        '/', '\'', '\0', '\u{202E}', '\u{200B}', '\u{FEFF}', '😀', '中', '<', '"', '\r',
    ];
    let mut x = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    (0..len)
        .map(|_| {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let i = usize::try_from(x >> 33).unwrap_or(0) % ALPHABET.len();
            ALPHABET.get(i).copied().unwrap_or('a')
        })
        .collect()
}

#[test]
fn garbage_never_panics_and_always_answers_in_json() {
    let mut inputs: Vec<String> = [
        "\0",
        "\0\0\0\n\0",
        "->",
        "→",
        "::=",
        "| | |",
        "->->->",
        "S ->->->",
        "S -> | | |",
        "S -> a ->",
        "S :: = a",
        "'''' -> a",
        "S -> S'",
        "\u{FEFF}",
        "\u{202E}S -> a",
        "\r\r\r",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    // Very long single lines, under and at the character limit.
    inputs.push("x".repeat(MAX_SOURCE_CHARS));
    inputs.push(format!("S -> {}", "a".repeat(MAX_SOURCE_CHARS - 5)));
    inputs.push(format!("S -> {}", "a ".repeat((MAX_SOURCE_CHARS - 5) / 2)));
    inputs.push(format!("S -> {}", "|".repeat(MAX_SOURCE_CHARS - 5)));
    inputs.push(format!("S -> {}", "->".repeat((MAX_SOURCE_CHARS - 5) / 2)));
    for seed in 0..40 {
        inputs.push(garbage(seed, 50 + 20 * usize::try_from(seed).unwrap_or(0)));
    }
    for text in &inputs {
        for variant in VARIANTS {
            // `run` asserts valid JSON, a boolean `ok`, the time budget, and
            // either a non-empty error message or an explain on every step.
            run(variant, text, "");
            run(variant, text, "a b");
        }
    }
}

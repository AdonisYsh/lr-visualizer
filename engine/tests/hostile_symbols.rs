//! Symbols named after JavaScript's inherited object members, which the engine
//! must treat as ordinary symbols. With `LRV_HOSTILE_JSON_DIR` set, each variant's
//! analyze() output is also written there as `<variant>.json`.

use serde_json::Value;

/// Hostile non-terminals and terminals: LR(0) conflicts on it, the other three do not.
const HOSTILE: &str = "constructor -> toString __proto__ | valueOf\n\
                       toString -> toString hasOwnProperty | hasOwnProperty\n\
                       __proto__ -> valueOf | ε";

const TEST_STRING: &str = "hasOwnProperty hasOwnProperty valueOf";

const VARIANTS: [&str; 4] = ["lr0", "slr1", "lalr1", "clr1"];

fn names(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

#[test]
fn prototype_member_names_are_ordinary_symbols_under_every_variant() {
    for variant in VARIANTS {
        let json = lr_engine::analyze(HOSTILE, variant, TEST_STRING);

        if let Some(dir) = std::env::var_os("LRV_HOSTILE_JSON_DIR") {
            let path = std::path::Path::new(&dir).join(format!("{variant}.json"));
            std::fs::write(&path, &json)
                .unwrap_or_else(|e| panic!("could not write {}: {e}", path.display()));
        }

        let v: Value = serde_json::from_str(&json)
            .unwrap_or_else(|e| panic!("{variant}: analyze must return valid JSON: {e}"));
        assert_eq!(
            v["ok"],
            Value::Bool(true),
            "{variant}: expected success, got error: {}",
            v["error"]
        );

        let steps = v["steps"]
            .as_array()
            .unwrap_or_else(|| panic!("{variant}: no step array"));
        assert!(!steps.is_empty(), "{variant}: no steps emitted");
        for s in steps {
            let explain = s["explain"].as_str().unwrap_or("");
            assert!(
                !explain.trim().is_empty(),
                "{variant}: step {} ({}) has no explanation",
                s["index"],
                s["kind"]
            );
        }

        let parsed = steps
            .iter()
            .find(|s| s["kind"] == "grammarParsed")
            .unwrap_or_else(|| panic!("{variant}: no grammarParsed step"));
        assert_eq!(parsed["mode"], "space-separated", "{variant}: wrong tokenization mode");
        // The augmented start comes first, and is named after the hostile one.
        assert_eq!(
            names(&parsed["nonterminals"]),
            vec!["constructor'", "constructor", "toString", "__proto__"],
            "{variant}: non-terminals"
        );
        assert_eq!(parsed["start"], "constructor'", "{variant}: augmented start");
        assert_eq!(
            names(&parsed["terminals"]),
            vec!["valueOf", "hasOwnProperty"],
            "{variant}: terminals"
        );

        // The hostile names reach the table as real columns, not as nothing.
        let cells_on = |symbol: &str| {
            steps
                .iter()
                .filter(|s| s["kind"] == "tableCellSet" && s["symbol"] == symbol)
                .count()
        };
        for symbol in ["valueOf", "hasOwnProperty", "toString", "__proto__", "constructor"] {
            assert!(cells_on(symbol) > 0, "{variant}: no table cell in column `{symbol}`");
        }

        assert!(
            steps.iter().any(|s| s["kind"] == "parseAccept"),
            "{variant}: the test string should be accepted"
        );
    }
}

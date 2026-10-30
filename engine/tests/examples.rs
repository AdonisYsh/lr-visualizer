//! The `examples/` handouts keep their measured numbers honest: a grammar that
//! stopped parsing, or a header claiming a count the engine no longer produces,
//! fails the build.

use serde_json::Value;
use std::path::{Path, PathBuf};

fn examples_dir() -> PathBuf {
    // The handouts sit beside engine/.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or_else(|| panic!("engine/ must have a parent directory"))
        .join("examples")
}

fn example_files() -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(examples_dir())
        .unwrap_or_else(|e| panic!("examples/ must exist and be readable: {e}"))
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|e| e == "txt").unwrap_or(false))
        .collect();
    files.sort();
    files
}

fn analyze(text: &str, variant: &str) -> Value {
    serde_json::from_str(&lr_engine::analyze(text, variant, ""))
        .unwrap_or_else(|e| panic!("analyze must return valid JSON: {e}"))
}

/// `// conflicts/states:  LR(0) 2/12  SLR(1) 0/12 ...` -> [("lr0", 2, 12), ...].
fn claimed(text: &str) -> Vec<(String, u64, u64)> {
    let line = text
        .lines()
        .find(|l| l.contains("conflicts/states:"))
        .unwrap_or_else(|| panic!("every example must carry a conflicts/states line"));
    let ids = [("LR(0)", "lr0"), ("SLR(1)", "slr1"), ("LALR(1)", "lalr1"), ("CLR(1)", "clr1")];
    ids.iter()
        .map(|(label, id)| {
            let at = line
                .find(label)
                .unwrap_or_else(|| panic!("the counts line must name {label}: {line}"));
            let rest = &line[at + label.len()..];
            let pair: &str = rest.split_whitespace().next().unwrap_or("");
            let (c, s) = pair
                .split_once('/')
                .unwrap_or_else(|| panic!("expected conflicts/states after {label}, got {pair:?}"));
            (
                (*id).to_string(),
                c.parse().unwrap_or_else(|_| panic!("bad conflict count {c:?}")),
                s.parse().unwrap_or_else(|_| panic!("bad state count {s:?}")),
            )
        })
        .collect()
}

#[test]
fn there_is_one_example_file_per_preset() {
    let files = example_files();
    assert_eq!(files.len(), 13, "one handout per preset grammar");
    for file in &files {
        let name = file.file_name().unwrap_or_default().to_string_lossy().to_string();
        assert!(
            name.chars().take(2).all(|c| c.is_ascii_digit()),
            "{name}: handouts are numbered so they sort into teaching order"
        );
    }
}

#[test]
fn every_example_analyses_under_every_variant() {
    for file in example_files() {
        let text = std::fs::read_to_string(&file)
            .unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        let name = file.file_name().unwrap_or_default().to_string_lossy().to_string();
        for variant in ["lr0", "slr1", "lalr1", "clr1"] {
            let v = analyze(&text, variant);
            assert_eq!(
                v["ok"],
                Value::Bool(true),
                "{name} under {variant}: {}",
                v["error"]
            );
            for step in v["steps"].as_array().cloned().unwrap_or_default() {
                assert!(
                    !step["explain"].as_str().unwrap_or("").trim().is_empty(),
                    "{name} under {variant}: a step has no explanation"
                );
            }
        }
    }
}

/// The header's numbers must be the engine's numbers: a wrong handout is worse than none.
#[test]
fn every_example_states_the_numbers_it_actually_produces() {
    for file in example_files() {
        let text = std::fs::read_to_string(&file)
            .unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        let name = file.file_name().unwrap_or_default().to_string_lossy().to_string();
        let run = analyze(&text, "lr0");
        let rows = run["verdict"]["comparison"].as_array().cloned().unwrap_or_default();

        for (id, conflicts, states) in claimed(&text) {
            let row = rows
                .iter()
                .find(|r| r["variant"] == id.as_str())
                .unwrap_or_else(|| panic!("{name}: no comparison row for {id}"));
            assert_eq!(
                row["conflicts"].as_u64(),
                Some(conflicts),
                "{name}: the header claims {conflicts} conflicts for {id}"
            );
            assert_eq!(
                row["states"].as_u64(),
                Some(states),
                "{name}: the header claims {states} states for {id}"
            );
        }
    }
}

/// The handouts and the in-app preset list must never drift apart.
#[test]
fn the_examples_match_the_presets_in_the_app() {
    let js = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap_or(Path::new(".")).join("web/grammars.js"),
    )
    .unwrap_or_else(|e| panic!("web/grammars.js must be readable: {e}"));

    for file in example_files() {
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        let name = file.file_name().unwrap_or_default().to_string_lossy().to_string();
        // The grammar itself is everything after the comment header.
        let grammar: String = text
            .lines()
            .filter(|l| !l.trim_start().starts_with("//") && !l.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\\n");
        assert!(
            js.contains(&grammar),
            "{name}: this grammar is not in web/grammars.js as `{grammar}`"
        );
    }
}

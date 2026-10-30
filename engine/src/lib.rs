//! LR parsing analysis engine.
//!
//! One exported function, [`analyze`], which returns a JSON string and never
//! panics. Everything the browser draws comes from the `steps` array in that JSON.
// No panics may cross the WASM boundary: every fallible path returns `Result`.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

pub mod automaton;
pub mod error;
pub mod grammar;
pub mod items;
pub mod limits;
pub mod parser;
pub mod sets;
pub mod table;
pub mod trace;

use automaton::Automaton;
use error::{EngineError, Result};
use grammar::{Grammar, Mode};
use items::ItemMode;
use serde_json::{json, Value};
use sets::Sets;
use table::{MergeProvenance, Table, Variant};
use trace::{Recorder, Step, VariantConflicts};

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::wasm_bindgen;

/// The one WASM export. Returns a JSON string; `test_string` may be empty, which skips the parse phase.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn analyze(grammar_text: &str, variant: &str, test_string: &str) -> String {
    let value = match run(grammar_text, variant, test_string) {
        Ok(v) => v,
        Err(e) => json!({
            "ok": false,
            "error": { "line": e.line, "message": e.message, "hint": e.hint },
        }),
    };
    serde_json::to_string(&value).unwrap_or_else(|_| {
        // Unreachable for a `Value`; kept so nothing can panic here.
        r#"{"ok":false,"error":{"line":null,"message":"The analysis could not be serialized.","hint":null}}"#.to_string()
    })
}

/// Map a variant id to a `Variant`; an unknown id is an error, not a fallback.
fn check_variant(variant: &str) -> Result<Variant> {
    match variant {
        "lr0" => Ok(Variant::Lr0),
        "slr1" => Ok(Variant::Slr1),
        "lalr1" => Ok(Variant::Lalr1),
        "clr1" => Ok(Variant::Clr1),
        other => Err(EngineError::new(format!(
            "Unknown parser variant `{other}`."
        ))
        .hint("Valid variants are lr0, slr1, lalr1 and clr1.")),
    }
}

/// The three canonical collections (LR(0), LR(1), merged LALR(1)), built at most
/// once each and seeded with whatever the run already built. The cache key is the
/// item mode plus whether the variant merges. Collections built here are for the
/// comparison alone, so they use the smaller `MAX_COMPARISON_WORK` budget.
struct Collections<'a> {
    g: &'a Grammar,
    sets: &'a Sets,
    lr0: Option<Automaton>,
    lr1: Option<Automaton>,
    lalr: Option<(Automaton, Vec<Vec<usize>>)>,
}

impl<'a> Collections<'a> {
    fn new(g: &'a Grammar, sets: &'a Sets) -> Self {
        Collections { g, sets, lr0: None, lr1: None, lalr: None }
    }

    /// The LR(0) collection, shared by LR(0) and SLR(1).
    fn lr0(&mut self) -> Result<Automaton> {
        if self.lr0.is_none() {
            let mut scratch = Recorder::for_comparison();
            self.lr0 = Some(automaton::build(self.g, ItemMode::Lr0, &mut scratch)?);
        }
        self.lr0.clone().ok_or_else(cache_failed)
    }

    /// The canonical LR(1) collection, shared by CLR(1) and LALR(1).
    fn lr1(&mut self) -> Result<Automaton> {
        if self.lr1.is_none() {
            let mut scratch = Recorder::for_comparison();
            self.lr1 =
                Some(automaton::build(self.g, ItemMode::Lr1(self.sets), &mut scratch)?);
        }
        self.lr1.clone().ok_or_else(cache_failed)
    }

    /// The merged collection, and what each merged state was merged from.
    fn lalr(&mut self) -> Result<(Automaton, Vec<Vec<usize>>)> {
        if self.lalr.is_none() {
            let canonical = self.lr1()?;
            let mut scratch = Recorder::for_comparison();
            self.lalr = Some(automaton::merge_by_core(self.g, &canonical, &mut scratch)?);
        }
        self.lalr.clone().ok_or_else(cache_failed)
    }
}

fn cache_failed() -> EngineError {
    EngineError::new("internal error: the automaton cache did not fill")
}

/// One row of the comparison, measured by building that method's table; steps are discarded.
fn measure(cache: &mut Collections, variant: Variant) -> Result<VariantConflicts> {
    let (g, sets) = (cache.g, cache.sets);
    let mut scratch = Recorder::for_comparison();
    let strategy = variant.strategy(sets);

    let (table, states) = if variant.merges() {
        let canonical = cache.lr1()?;
        let (merged, groups) = cache.lalr()?;
        let provenance = provenance_for(g, sets, &canonical, groups, &mut scratch)?;
        let states = merged.states.len();
        (table::build(g, &merged, &strategy, Some(&provenance), &mut scratch)?, states)
    } else {
        let automaton = if variant.uses_lr1_items() { cache.lr1()? } else { cache.lr0()? };
        let states = automaton.states.len();
        (table::build(g, &automaton, &strategy, None, &mut scratch)?, states)
    };
    Ok(row_for(variant, sets, &table, states))
}

/// The CLR(1) table plus the merge mapping, so a merge-induced conflict is
/// established by comparison with a table that was built, not inferred.
fn provenance_for(
    g: &Grammar,
    sets: &Sets,
    canonical: &Automaton,
    groups: Vec<Vec<usize>>,
    scratch: &mut Recorder,
) -> Result<MergeProvenance> {
    let clr = table::build(g, canonical, &Variant::Clr1.strategy(sets), None, scratch)?;
    let clr_items = canonical.states.iter().map(|s| s.items.clone()).collect();
    Ok(MergeProvenance { groups, clr, clr_items })
}

/// What the comparison shows about one method, taken off a built table.
fn row_for(variant: Variant, sets: &Sets, table: &Table, states: usize) -> VariantConflicts {
    let strategy = variant.strategy(sets);
    VariantConflicts {
        variant: variant.id().to_string(),
        label: variant.label().to_string(),
        conflicts: table.conflicts.len(),
        shift_reduce: table.shift_reduce_count(),
        reduce_reduce: table.reduce_reduce_count(),
        merge_induced: table.merge_induced,
        is_valid: table.is_valid(),
        states,
        class: table::class_of(&strategy, table.is_valid()),
        message: table::verdict_message(&strategy, table),
    }
}

/// Conflict and state counts under every implemented variant, by running each.
/// A variant whose collection cannot be built is left out rather than failing
/// the analysis, so only variants actually run can ever be suggested.
fn compare_variants(
    cache: &mut Collections,
    current: Variant,
    current_row: VariantConflicts,
) -> Vec<VariantConflicts> {
    let mut out: Vec<VariantConflicts> = Vec::new();
    for variant in Variant::IMPLEMENTED {
        if *variant == current {
            out.push(current_row.clone());
            continue;
        }
        if let Ok(row) = measure(cache, *variant) {
            out.push(row);
        }
    }
    out
}

/// The first implemented variant stronger than `current` that has no conflicts.
/// `IMPLEMENTED` is ordered weakest first, so "stronger" is "later in the list".
fn stronger_clean_variant(
    results: &[VariantConflicts],
    current: Variant,
) -> Option<&VariantConflicts> {
    let at = results.iter().position(|r| r.variant == current.id())?;
    results.get(at + 1..)?.iter().find(|r| r.is_valid)
}

fn comparison_explain(results: &[VariantConflicts], current: Variant) -> String {
    let counts: Vec<String> = results
        .iter()
        .map(|r| {
            let n = match r.conflicts {
                0 => "no conflicts".to_string(),
                1 => "1 conflict".to_string(),
                n => format!("{n} conflicts"),
            };
            format!("{} - {}", r.label, n)
        })
        .collect();

    let states: Vec<String> = results
        .iter()
        .map(|r| format!("{} in {}", r.states, r.label))
        .collect();
    let grew = results.iter().map(|r| r.states).max().unwrap_or(0)
        != results.iter().map(|r| r.states).min().unwrap_or(0);
    let sizes = if grew {
        format!(
            " The tables are not the same size: {}. A method that tells more contexts apart needs more states to tell them apart in.",
            states.join(", ")
        )
    } else {
        String::new()
    };

    let tail = match stronger_clean_variant(results, current) {
        Some(better) if better.states == current_states(results, current) => format!(
            " The item sets are the same either way; all that changes is where the reductions are allowed to go, and under {} that is enough to clear every cell. Switch the variant selector to see it.",
            better.label
        ),
        Some(better) => format!(
            " Under {} every cell clears. Switch the variant selector to see it.",
            better.label
        ),
        None => String::new(),
    };
    format!(
        "The same grammar, under every method this tool implements: {}.{sizes}{}{tail}",
        counts.join("; "),
        merge_verdict(results)
    )
}

/// The sentence about what LALR(1)'s merge bought or cost; only when both rows were measured.
fn merge_verdict(results: &[VariantConflicts]) -> String {
    let find = |id: &str| results.iter().find(|r| r.variant == id);
    let (Some(lalr), Some(clr)) = (find("lalr1"), find("clr1")) else { return String::new() };

    if lalr.conflicts > clr.conflicts {
        let extra = lalr.conflicts.saturating_sub(clr.conflicts);
        return format!(
            " LALR(1) conflicts where CLR(1) does not: {extra} cell{} {} only because merging put two states together that CLR(1) keeps apart. That is the one thing LALR(1) can get wrong, and this grammar is an example of it.",
            if extra == 1 { "" } else { "s" },
            if extra == 1 { "conflicts" } else { "conflict" }
        );
    }
    if lalr.states < clr.states {
        return format!(
            " LALR(1) reached the same answer as CLR(1) in {} states rather than {}: merging cost this grammar nothing at all, which is why LALR(1) is what real parser generators build.",
            lalr.states, clr.states
        );
    }
    " No two CLR(1) states share a core here, so LALR(1) merged nothing and the two methods are the same table.".to_string()
}

fn current_states(results: &[VariantConflicts], current: Variant) -> usize {
    results
        .iter()
        .find(|r| r.variant == current.id())
        .map_or(0, |r| r.states)
}

// ---------------------------------------------------------------------------

fn run(grammar_text: &str, variant: &str, test_string: &str) -> Result<Value> {
    let variant = check_variant(variant)?;

    let parsed = grammar::parse(grammar_text)?;
    let g = &parsed.grammar;

    // Check the test string before spending any analysis on it.
    let tokens = if test_string.trim().is_empty() {
        None
    } else {
        Some(parser::tokenize_test_string(g, test_string)?)
    };

    let mut rec = Recorder::new();

    // --- phase: grammar ------------------------------------------------------
    rec.push(Step::GrammarParsed {
        productions: g.productions.clone(),
        terminals: g.terminals.clone(),
        nonterminals: g.nonterminals.clone(),
        start: g.start.clone(),
        mode: g.mode.label().to_string(),
        explain: grammar_explain(g),
    })?;
    rec.push(Step::Augmented {
        production: g.production(0)?.clone(),
        explain: format!(
            "Production 0, {}, was added to augment the grammar. The new start symbol {} appears on no right-hand side, so the parser has exactly one way to finish: reducing by production 0 means the entire input has been recognised. Without augmentation, a grammar whose start symbol can appear inside a sentential form would have no unambiguous state in which to accept.",
            g.production(0)?.display(),
            g.start
        ),
    })?;

    rec.push(Step::VariantChosen {
        variant: variant.id().to_string(),
        label: variant.label().to_string(),
        reduce_rule: variant.reduce_rule().to_string(),
        explain: variant.explain(),
    })?;

    // --- phase: sets ---------------------------------------------------------
    // Computed once for every variant (LR(0) ignores them, SLR(1) uses FOLLOW,
    // CLR(1)/LALR(1) use FIRST only).
    let sets = sets::compute(g, &mut rec)?;

    // --- phase: automaton ----------------------------------------------------
    // LR(0) and SLR(1) share a collection; CLR(1) has its own; LALR(1) builds
    // CLR(1)'s and merges it.
    let mut cache = Collections::new(g, &sets);
    let (automaton, provenance) = if variant.merges() {
        let built = automaton::build_lalr(g, &sets, &mut rec)?;
        let mut scratch = Recorder::for_comparison();
        let provenance =
            provenance_for(g, &sets, &built.canonical, built.groups.clone(), &mut scratch)?;
        // Seed the cache so the comparison builds no automaton.
        cache.lr1 = Some(built.canonical);
        cache.lalr = Some((built.merged.clone(), built.groups));
        (built.merged, Some(provenance))
    } else {
        let built = automaton::build(g, variant.item_mode(&sets), &mut rec)?;
        if variant.uses_lr1_items() {
            cache.lr1 = Some(built.clone());
        } else {
            cache.lr0 = Some(built.clone());
        }
        (built, None)
    };

    // --- phase: table --------------------------------------------------------
    let strategy = variant.strategy(&sets);
    let tbl = table::build(g, &automaton, &strategy, provenance.as_ref(), &mut rec)?;
    let valid = tbl.is_valid();

    let current_row = row_for(variant, &sets, &tbl, automaton.states.len());
    let results = compare_variants(&mut cache, variant, current_row);
    rec.push(Step::VariantComparison {
        current: variant.label().to_string(),
        results: results.clone(),
        explain: comparison_explain(&results, variant),
    })?;

    // --- phase: parse --------------------------------------------------------
    let mut verdict_extra = String::new();
    if let Some(tokens) = &tokens {
        let accepted = parser::run(g, &tbl, tokens, &mut rec)?;
        verdict_extra = if accepted {
            " The test string was accepted.".to_string()
        } else {
            " The test string was rejected.".to_string()
        };
    }

    // The suggestion names only a variant that was actually run, never one
    // inferred from the shape of a conflict.
    let suggestion = match stronger_clean_variant(&results, variant) {
        Some(better) if !valid => {
            format!(" This grammar is {}, though - try that.", better.label)
        }
        _ => String::new(),
    };

    Ok(json!({
        "ok": true,
        "grammar": grammar_json(g),
        "warnings": parsed.warnings,
        "table": table_json(g, &tbl),
        "verdict": {
            "isValid": valid,
            "class": table::class_of(&strategy, valid),
            "message": format!("{}{}{}", table::verdict_message(&strategy, &tbl), suggestion, verdict_extra),
            "note": if valid { Value::Null } else { Value::String(table::VERDICT_NOTE.to_string()) },
            "comparison": results,
        },
        "steps": rec.into_steps(),
    }))
}

// ---------------------------------------------------------------------------
// JSON views of the final result
// ---------------------------------------------------------------------------

fn grammar_json(g: &Grammar) -> Value {
    json!({
        "productions": g.productions,
        "terminals": g.terminals,
        "nonterminals": g.nonterminals,
        "start": g.start,
        "originalStart": g.original_start,
        "mode": g.mode.label(),
        "symbolOrder": g.symbol_order,
    })
}

fn table_json(g: &Grammar, t: &table::Table) -> Value {
    let action: Value = t
        .action
        .iter()
        .map(|(state, row)| (state.to_string(), json!(row)))
        .collect::<serde_json::Map<String, Value>>()
        .into();
    let goto: Value = t
        .goto
        .iter()
        .map(|(state, row)| (state.to_string(), json!(row)))
        .collect::<serde_json::Map<String, Value>>()
        .into();
    json!({
        "states": t.states,
        "actionColumns": g.action_columns(),
        "gotoColumns": g.nonterminals.iter().filter(|n| **n != g.start).collect::<Vec<_>>(),
        "action": action,
        "goto": goto,
        "conflicts": t.conflicts,
    })
}

fn grammar_explain(g: &Grammar) -> String {
    let mode_sentence = match g.mode {
        Mode::SingleChar => "The grammar was read in single-character mode: nothing in it is longer than one character, so every character of a right-hand side is its own symbol and spaces are only for readability.".to_string(),
        Mode::SpaceSeparated => format!(
            "The grammar was read in space-separated mode, because at least one token is longer than one character (for example `{}`). Symbols are split on whitespace, so `( E )` is three symbols while `(E)` would be a single symbol named \"(E)\".",
            g.symbol_order
                .iter()
                .find(|s| s.chars().count() > 1 && **s != g.start)
                .cloned()
                .unwrap_or_else(|| g.start.clone())
        ),
    };
    format!(
        "{mode_sentence} It has {} production{} (before augmentation), {} non-terminal{} ({}) and {} terminal{} ({}). A symbol is a non-terminal exactly when some production defines it; everything else is a terminal. The start symbol is {}, taken from the left-hand side of the first production.",
        g.productions.len().saturating_sub(1),
        if g.productions.len() == 2 { "" } else { "s" },
        g.nonterminals.len().saturating_sub(1),
        if g.nonterminals.len() == 2 { "" } else { "s" },
        g.nonterminals.iter().filter(|n| **n != g.start).cloned().collect::<Vec<_>>().join(", "),
        g.terminals.len(),
        if g.terminals.len() == 1 { "" } else { "s" },
        g.terminals.join(", "),
        g.original_start
    )
}

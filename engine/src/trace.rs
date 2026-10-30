//! The [`Step`] log: the engine's actual output.
//!
//! The engine computes everything up front and records every intermediate move
//! as a `Step`. The browser is a dumb player walking that list; stepping
//! backwards is an array index, never a recomputation. All four LR variants
//! emit this same shape, so the player is written once.
//!
//! `phase` and `index` are attached by [`Recorder`], not by the construction
//! sites, so no caller can forget them or get them out of sync.

use crate::error::{EngineError, Result};
use crate::limits::{self, Work};
use serde::Serialize;
use serde_json::{Map, Value};

// ---------------------------------------------------------------------------
// Shared payload types
// ---------------------------------------------------------------------------

/// An LR item `A -> α • β`.
///
/// `lookaheads` is `None` for LR(0) and SLR(1) items and `Some(..)` for CLR(1)
/// and LALR(1), where it holds the terminals that may follow the left-hand side
/// in the context this state was reached in - kept as a *set* on one item, so
/// `[A -> α • β, {a, b, c}]` is one line rather than three. Keeping one type
/// across all four variants is what lets the frontend renderers stay
/// variant-agnostic.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct Item {
    pub production: usize,
    pub dot: usize,
    pub lookaheads: Option<Vec<String>>,
}

impl Item {
    pub fn new(production: usize, dot: usize) -> Self {
        Item { production, dot, lookaheads: None }
    }

    /// An item that carries lookaheads. The caller is responsible for the set
    /// already being in canonical order; `items::closure` guarantees that for
    /// everything it produces.
    pub fn with_lookaheads(production: usize, dot: usize, lookaheads: Vec<String>) -> Self {
        Item { production, dot, lookaheads: Some(lookaheads) }
    }

    /// The same item with the dot moved one symbol to the right.
    pub fn advanced(&self) -> Self {
        Item { production: self.production, dot: self.dot + 1, lookaheads: self.lookaheads.clone() }
    }
}

/// A parsing-table entry.
///
/// Serialized as a tagged object (`{"type":"shift","state":5}`) rather than a
/// bare number so the frontend never has to infer what a cell means.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Action {
    Shift { state: usize },
    Reduce { production: usize },
    Accept,
    Goto { state: usize },
}

impl Action {
    /// The conventional short form: `s5`, `r3`, `acc`, `7`.
    pub fn short(&self) -> String {
        match self {
            Action::Shift { state } => format!("s{state}"),
            Action::Reduce { production } => format!("r{production}"),
            Action::Accept => "acc".to_string(),
            Action::Goto { state } => format!("{state}"),
        }
    }
}

/// One entry of the parse stack. Bundles the state with the symbol pushed
/// alongside it, rather than the textbook's alternating symbol/state sequence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StackEntry {
    pub state: usize,
    pub symbol: Option<String>,
}

/// One row of the variant comparison: how many cells conflict, of what kinds,
/// how many states it took, and what the verdict would say - when the same
/// grammar is run through one particular method.
///
/// Every field is measured by running that method, never inferred; `states` is
/// the price of the stronger methods.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VariantConflicts {
    /// The id the UI uses: `lr0`, `slr1`, ...
    pub variant: String,
    /// The name a human reads: `LR(0)`, `SLR(1)`, ...
    pub label: String,
    pub conflicts: usize,
    pub shift_reduce: usize,
    pub reduce_reduce: usize,
    /// LALR(1) only: conflicts the CLR(1) table does not have, and that
    /// therefore exist only because merging brought two states together.
    pub merge_induced: usize,
    pub is_valid: bool,
    /// The size of the canonical collection this method builds. For LALR(1)
    /// this is the count *after* merging.
    pub states: usize,
    /// `LR(0)` or `not LR(0)`: the verdict in two or three words.
    pub class: String,
    /// The verdict sentence this method would headline with.
    pub message: String,
}

// ---------------------------------------------------------------------------
// The Step enum
// ---------------------------------------------------------------------------

/// One recorded move. Every variant carries an `explain` written for reading
/// aloud in a classroom - that text is the product, not a debug aid.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Step {
    // --- phase: grammar ----------------------------------------------------
    /// Which method is being built, and the one rule that separates it from the
    /// others. Closes the grammar phase, so any renderer can ask what it is
    /// drawing without having to infer it from the table.
    VariantChosen {
        /// `lr0`, `slr1`, ...
        variant: String,
        /// `LR(0)`, `SLR(1)`, ...
        label: String,
        /// How reduce columns are chosen:
        /// `every-column` | `follow` | `item-lookahead`.
        reduce_rule: String,
        explain: String,
    },
    GrammarParsed {
        productions: Vec<crate::grammar::Production>,
        terminals: Vec<String>,
        nonterminals: Vec<String>,
        start: String,
        mode: String,
        explain: String,
    },
    Augmented {
        production: crate::grammar::Production,
        explain: String,
    },

    // --- phase: sets -------------------------------------------------------
    FirstAdd {
        nonterminal: String,
        symbol: String,
        from_production: usize,
        explain: String,
    },
    FirstDone {
        table: Vec<(String, Vec<String>)>,
        explain: String,
    },
    FollowAdd {
        nonterminal: String,
        symbol: String,
        from_production: usize,
        /// "start" | "beta-first" | "beta-nullable" | "tail"
        rule_used: String,
        explain: String,
    },
    FollowDone {
        table: Vec<(String, Vec<String>)>,
        explain: String,
    },

    // --- phase: automaton --------------------------------------------------
    StateCreated {
        id: usize,
        kernel: Vec<Item>,
        from_state: Option<usize>,
        on_symbol: Option<String>,
        explain: String,
    },
    ClosureItemAdded {
        state: usize,
        item: Item,
        because_of: Item,
        explain: String,
    },
    StateClosed {
        id: usize,
        items: Vec<Item>,
        explain: String,
    },
    TransitionAdded {
        from: usize,
        to: usize,
        symbol: String,
        target_is_new: bool,
        explain: String,
    },
    /// LALR(1) only: one group of same-core CLR(1) states folded into one.
    ///
    /// `merged_ids` are ids in the CLR(1) automaton; `new_id` is the id the
    /// merged state has once the collection is renumbered. `items` carries the
    /// merged state's items with the lookahead sets already unioned, so no
    /// renderer ever has to union a lookahead set for itself.
    StatesMerged {
        merged_ids: Vec<usize>,
        new_id: usize,
        /// The shared core: the same items with lookaheads stripped.
        core: Vec<Item>,
        /// The merged state's items, lookaheads unioned across the group.
        items: Vec<Item>,
        explain: String,
    },
    /// LALR(1) only: the merging stage is over. Closes the automaton phase with
    /// the complete renumbering, so a renderer can redirect every transition
    /// and relabel every state without deriving the mapping itself.
    MergeDone {
        /// Indexed by the new state id: the CLR(1) states it was merged from.
        /// Groups of one are in here too - they were renumbered as well.
        groups: Vec<Vec<usize>>,
        /// The size of the CLR(1) collection this started from.
        before: usize,
        /// The size of the LALR(1) collection it ended as.
        after: usize,
        /// The two method names for the running side-by-side counter, so the
        /// renderer never has to know which method merges into which.
        before_label: String,
        after_label: String,
        explain: String,
    },
    AutomatonDone {
        state_count: usize,
        explain: String,
    },

    // --- phase: table ------------------------------------------------------
    TableCellSet {
        state: usize,
        symbol: String,
        action: Action,
        source_item: Option<Item>,
        explain: String,
    },
    TableConflict {
        state: usize,
        symbol: String,
        existing: Action,
        incoming: Action,
        /// "shift-reduce" | "reduce-reduce"
        conflict_kind: String,
        /// LALR(1) only: true when the CLR(1) table has no conflict in the
        /// states this one was merged from, so merging is what caused it.
        /// Always false for the three methods that merge nothing.
        merge_induced: bool,
        explain: String,
    },
    TableDone {
        conflicts: usize,
        is_valid: bool,
        class: String,
        explain: String,
    },
    /// The same grammar's conflict count under every method this build
    /// implements, so the cost of the weaker rule is visible at a glance. The
    /// counts come from re-running table construction, never from a guess.
    VariantComparison {
        /// The label of the method actually being shown.
        current: String,
        results: Vec<VariantConflicts>,
        explain: String,
    },

    // --- phase: parse ------------------------------------------------------
    ParseInit {
        stack: Vec<StackEntry>,
        input: Vec<String>,
        explain: String,
    },
    ParseShift {
        symbol: String,
        to_state: usize,
        stack: Vec<StackEntry>,
        input_pos: usize,
        explain: String,
    },
    ParseReduce {
        production: usize,
        popped: usize,
        exposed_state: usize,
        goto_state: usize,
        stack: Vec<StackEntry>,
        input_pos: usize,
        explain: String,
    },
    ParseAccept {
        explain: String,
    },
    ParseReject {
        input_pos: usize,
        state: usize,
        lookahead: String,
        expected: Vec<String>,
        explain: String,
    },
}

/// The five phases, matching the five tabs in the UI.
pub const PHASES: [&str; 5] = ["grammar", "sets", "automaton", "table", "parse"];

impl Step {
    pub fn phase(&self) -> &'static str {
        match self {
            Step::VariantChosen { .. }
            | Step::GrammarParsed { .. }
            | Step::Augmented { .. } => "grammar",
            Step::FirstAdd { .. }
            | Step::FirstDone { .. }
            | Step::FollowAdd { .. }
            | Step::FollowDone { .. } => "sets",
            Step::StateCreated { .. }
            | Step::ClosureItemAdded { .. }
            | Step::StateClosed { .. }
            | Step::TransitionAdded { .. }
            | Step::StatesMerged { .. }
            | Step::MergeDone { .. }
            | Step::AutomatonDone { .. } => "automaton",
            Step::TableCellSet { .. }
            | Step::TableConflict { .. }
            | Step::TableDone { .. }
            | Step::VariantComparison { .. } => "table",
            Step::ParseInit { .. }
            | Step::ParseShift { .. }
            | Step::ParseReduce { .. }
            | Step::ParseAccept { .. }
            | Step::ParseReject { .. } => "parse",
        }
    }

    fn explain(&self) -> &str {
        match self {
            Step::VariantChosen { explain, .. }
            | Step::GrammarParsed { explain, .. }
            | Step::Augmented { explain, .. }
            | Step::FirstAdd { explain, .. }
            | Step::FirstDone { explain, .. }
            | Step::FollowAdd { explain, .. }
            | Step::FollowDone { explain, .. }
            | Step::StateCreated { explain, .. }
            | Step::ClosureItemAdded { explain, .. }
            | Step::StateClosed { explain, .. }
            | Step::TransitionAdded { explain, .. }
            | Step::StatesMerged { explain, .. }
            | Step::MergeDone { explain, .. }
            | Step::AutomatonDone { explain, .. }
            | Step::TableCellSet { explain, .. }
            | Step::TableConflict { explain, .. }
            | Step::TableDone { explain, .. }
            | Step::VariantComparison { explain, .. }
            | Step::ParseInit { explain, .. }
            | Step::ParseShift { explain, .. }
            | Step::ParseReduce { explain, .. }
            | Step::ParseAccept { explain, .. }
            | Step::ParseReject { explain, .. } => explain,
        }
    }
}

// ---------------------------------------------------------------------------
// Recorder
// ---------------------------------------------------------------------------

/// Collects steps, stamping each with its phase and sequential index and
/// enforcing the step cap. It also carries the run's work meter.
///
/// The parse phase has its own cap, [`limits::MAX_PARSE_STEPS`], enforced by
/// the parse driver.
pub struct Recorder {
    steps: Vec<Value>,
    /// How many steps have been recorded. The index of the next one, and what
    /// the step cap is measured against.
    count: usize,
    /// False for a comparison build, whose steps are counted and checked like
    /// any others but never read, and so are not stored.
    keep: bool,
    work: Work,
}

impl Recorder {
    /// The recorder for the analysis the user asked for: the full work budget.
    pub fn new() -> Self {
        Recorder { steps: Vec::new(), count: 0, keep: true, work: Work::new(limits::MAX_WORK) }
    }

    /// A recorder for a comparison row: smaller work budget, steps discarded
    /// but still counted against the step cap and checked for an `explain`.
    pub fn for_comparison() -> Self {
        Recorder {
            steps: Vec::new(),
            count: 0,
            keep: false,
            work: Work::new(limits::MAX_COMPARISON_WORK),
        }
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The work meter of this run. Every loop a grammar can make long charges
    /// it; see [`limits::MAX_WORK`].
    pub fn work(&mut self) -> &mut Work {
        &mut self.work
    }

    /// Record one step. Fails if a cap is exceeded or the step has no `explain`.
    pub fn push(&mut self, step: Step) -> Result<()> {
        if self.count >= limits::MAX_STEPS {
            return Err(EngineError::new(format!(
                "This analysis needs more than {} steps to show.",
                limits::MAX_STEPS
            ))
            .hint("The grammar is too large to walk through one step at a time. Try a smaller one."));
        }
        let phase = step.phase();
        if step.explain().trim().is_empty() {
            return Err(EngineError::new("internal error: a step was recorded with no explanation"));
        }

        let index = self.count;
        self.count = self.count.saturating_add(1);
        if !self.keep {
            return Ok(());
        }
        let mut value = serde_json::to_value(&step)
            .map_err(|e| EngineError::new(format!("internal error: cannot serialize step: {e}")))?;
        match value.as_object_mut() {
            Some(obj) => {
                obj.insert("phase".to_string(), Value::String(phase.to_string()));
                obj.insert("index".to_string(), Value::from(index));
            }
            None => {
                // Unreachable for an internally-tagged enum, but this crate
                // never panics across the WASM boundary.
                let mut obj = Map::new();
                obj.insert("phase".to_string(), Value::String(phase.to_string()));
                obj.insert("index".to_string(), Value::from(index));
                obj.insert("payload".to_string(), value);
                value = Value::Object(obj);
            }
        }
        self.steps.push(value);
        Ok(())
    }

    /// Every step recorded, in order. Empty for a comparison build, which
    /// keeps none.
    pub fn into_steps(self) -> Vec<Value> {
        self.steps
    }
}

impl Default for Recorder {
    fn default() -> Self {
        Self::new()
    }
}

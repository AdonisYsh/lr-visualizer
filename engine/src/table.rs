//! ACTION/GOTO construction, with conflicts kept rather than resolved.
//!
//! The variants differ in one question: which terminal columns get
//! `reduce A -> α` for a completed item? [`ReduceStrategy`] answers it (LR(0):
//! all; SLR(1): FOLLOW(A); CLR(1)/LALR(1): the item's own lookaheads, FOLLOW is
//! not consulted). Everything else here is shared.

use crate::error::Result;
use crate::grammar::{Grammar, END};
use crate::items::{is_complete, item_display, production_display, ItemMode};
use crate::sets::Sets;
use crate::trace::{Action, Item, Recorder, Step};
use crate::automaton::Automaton;
use serde::Serialize;
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Variants and the reduce seam
// ---------------------------------------------------------------------------

/// The parser variants this build implements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Lr0,
    Slr1,
    Lalr1,
    Clr1,
}

impl Variant {
    /// Everything implemented, weakest method first, so "is there a stronger
    /// method that works?" is a forward scan.
    pub const IMPLEMENTED: &'static [Variant] =
        &[Variant::Lr0, Variant::Slr1, Variant::Lalr1, Variant::Clr1];

    /// The id the UI and [`crate::analyze`] use.
    pub fn id(self) -> &'static str {
        match self {
            Variant::Lr0 => "lr0",
            Variant::Slr1 => "slr1",
            Variant::Lalr1 => "lalr1",
            Variant::Clr1 => "clr1",
        }
    }

    /// The name a human reads.
    pub fn label(self) -> &'static str {
        match self {
            Variant::Lr0 => "LR(0)",
            Variant::Slr1 => "SLR(1)",
            Variant::Lalr1 => "LALR(1)",
            Variant::Clr1 => "CLR(1)",
        }
    }

    /// How this variant chooses reduce columns, as a tag the frontend can
    /// switch on: `every-column` | `follow` | `item-lookahead`.
    pub fn reduce_rule(self) -> &'static str {
        match self {
            Variant::Lr0 => "every-column",
            Variant::Slr1 => "follow",
            Variant::Lalr1 | Variant::Clr1 => "item-lookahead",
        }
    }

    /// Whether this variant's items carry lookaheads, and therefore whether it
    /// needs its own canonical collection. LR(0) and SLR(1) share one.
    pub fn item_mode(self, sets: &Sets) -> ItemMode<'_> {
        match self {
            Variant::Lr0 | Variant::Slr1 => ItemMode::Lr0,
            Variant::Lalr1 | Variant::Clr1 => ItemMode::Lr1(sets),
        }
    }

    /// True when this variant builds the LR(1) collection rather than the LR(0)
    /// one. Used, with [`Variant::merges`], to decide which automaton a
    /// comparison run can borrow.
    pub fn uses_lr1_items(self) -> bool {
        matches!(self, Variant::Clr1 | Variant::Lalr1)
    }

    /// True when this variant merges same-core states after building the LR(1)
    /// collection. Only LALR(1) does, and it is the one thing that separates it
    /// from CLR(1) - so it is also the second half of the automaton cache key.
    pub fn merges(self) -> bool {
        matches!(self, Variant::Lalr1)
    }

    /// The rule this variant applies to a completed item. FOLLOW is computed
    /// once in the sets phase and borrowed here rather than recomputed.
    pub fn strategy(self, sets: &Sets) -> ReduceStrategy<'_> {
        match self {
            Variant::Lr0 => ReduceStrategy::EveryColumn,
            Variant::Slr1 => ReduceStrategy::Follow(sets),
            Variant::Lalr1 | Variant::Clr1 => ReduceStrategy::ItemLookaheads { variant: self },
        }
    }

    /// The sentence that opens the analysis, naming the rule that will be used.
    pub fn explain(self) -> String {
        match self {
            Variant::Lr0 => "This analysis builds an LR(0) parser. LR(0) consults no lookahead at all: a state alone decides what to do, so a completed item A → α • is reduced in every terminal column and in $. That blanket rule is what makes LR(0) the weakest of the four methods, and it is worth seeing fail before reaching for a stronger one.".to_string(),
            Variant::Slr1 => "This analysis builds an SLR(1) parser. SLR(1) differs from LR(0) in exactly one place: the item sets, the closure, the GOTO function and the state numbering are all identical, but a completed item A → α • is reduced only in the columns of FOLLOW(A) - the symbols that can legally come after A - instead of in every column. One symbol of lookahead, used in one place, and several textbook grammars stop conflicting.".to_string(),
            Variant::Lalr1 => "This analysis builds an LALR(1) parser - the method almost every real parser generator uses, yacc and bison among them. LALR(1) starts by building the CLR(1) automaton in full, and then does one further thing: wherever two states hold the same items with the dots in the same places, differing only in their lookaheads, it merges them into one state and unions the lookahead sets. The result has exactly one state per core, which is exactly as many states as LR(0) builds - so LALR(1) buys an LR(0)-sized table while still placing each reduction on per-item lookaheads rather than on FOLLOW. It is not free. Merging can put two reductions that CLR(1) kept apart into the same cell, and a grammar that is CLR(1) but not LALR(1) is precisely one where that happens. The merging stage is shown at the end of the Item Sets tab: do not skip it, it is the whole method.".to_string(),
            Variant::Clr1 => "This analysis builds a CLR(1) parser, also called canonical LR(1). This is the first method whose item sets themselves change: an item is now [A → α • β, a], carrying the terminals that may follow A here, and closure propagates them with FIRST(β a). Two states are the same only if their lookaheads match too, so the automaton is larger than the LR(0) one - that growth is the method, not a side effect. A completed item is then reduced only on the lookaheads it carries. FOLLOW is not consulted anywhere in this analysis; it is shown in the FIRST/FOLLOW tab for reference only. That is the distinction students most often blur: SLR(1) asks what can follow A anywhere in the grammar, CLR(1) asks what can follow A here.".to_string(),
        }
    }
}

/// Where a completed item's reduction goes: the seam between the variants.
pub enum ReduceStrategy<'a> {
    /// LR(0): every terminal column and `$`, whatever the item is.
    EveryColumn,
    /// SLR(1): the columns of `FOLLOW(A)`, where `A` is the item's left-hand
    /// side. Borrows the sets computed in the sets phase.
    Follow(&'a Sets),
    /// CLR(1) and LALR(1): the columns the item itself names; FOLLOW is not
    /// consulted. The arm carries the variant only so the wording can differ.
    ItemLookaheads { variant: Variant },
}

/// The answer to "which columns take this reduction, and why".
pub struct ReduceColumns {
    /// Columns that get the entry, in ACTION-column order.
    pub chosen: Vec<String>,
    /// Columns LR(0) would have filled that this strategy leaves alone. Always
    /// empty for LR(0) itself; it is the whole lesson for everything else.
    pub skipped: Vec<String>,
    /// The set that was consulted, named for the explanation. `None` when the
    /// strategy consults nothing at all.
    pub consulted: Option<ConsultedSet>,
}

/// A named lookahead set, as it should be read aloud: `FOLLOW(E) = { +, ), $ }`.
pub struct ConsultedSet {
    pub name: String,
    pub members: Vec<String>,
}

impl ConsultedSet {
    fn rendered(&self) -> String {
        if self.members.is_empty() {
            format!("{} = {{ }}", self.name)
        } else {
            format!("{} = {{ {} }}", self.name, self.members.join(", "))
        }
    }
}

impl ReduceStrategy<'_> {
    /// The variant this strategy implements.
    pub fn variant(&self) -> Variant {
        match self {
            ReduceStrategy::EveryColumn => Variant::Lr0,
            ReduceStrategy::Follow(_) => Variant::Slr1,
            ReduceStrategy::ItemLookaheads { variant } => *variant,
        }
    }

    pub fn label(&self) -> &'static str {
        self.variant().label()
    }

    /// Which columns take `item`'s reduction, which ones LR(0) would have
    /// filled but this method skips, and the set the decision came from.
    pub fn columns_for(&self, grammar: &Grammar, item: &Item) -> Result<ReduceColumns> {
        let all = grammar.action_columns();
        match self {
            ReduceStrategy::EveryColumn => Ok(ReduceColumns {
                chosen: all,
                skipped: Vec::new(),
                consulted: None,
            }),
            ReduceStrategy::Follow(sets) => {
                let lhs = grammar.production(item.production)?.lhs.clone();
                let follow = sets.follow_of(&lhs);
                // Walk the ACTION columns rather than FOLLOW itself, so cells
                // are written left to right exactly as the table displays them.
                let (chosen, skipped): (Vec<String>, Vec<String>) =
                    all.into_iter().partition(|c| follow.iter().any(|f| f == c));
                Ok(ReduceColumns {
                    chosen,
                    skipped,
                    consulted: Some(ConsultedSet {
                        name: format!("FOLLOW({lhs})"),
                        members: follow.to_vec(),
                    }),
                })
            }
            ReduceStrategy::ItemLookaheads { .. } => {
                let lhs = grammar.production(item.production)?.lhs.clone();
                let carried: Vec<String> = item.lookaheads.clone().unwrap_or_default();
                let (chosen, skipped): (Vec<String>, Vec<String>) =
                    all.into_iter().partition(|c| carried.iter().any(|f| f == c));
                Ok(ReduceColumns {
                    chosen,
                    skipped,
                    consulted: Some(ConsultedSet {
                        // Named for the item, not the non-terminal: the point
                        // is that a different state's A → α • may carry a
                        // different set entirely.
                        name: format!("the lookaheads this state's {lhs} item carries"),
                        members: carried,
                    }),
                })
            }
        }
    }

    /// The explanation for one reduce cell. Names the production, the item and
    /// the rule that fired - and, where this method skips a column LR(0) would
    /// have filled, says so, because that contrast is the whole lesson.
    fn reduce_explain(
        &self,
        state: usize,
        production: usize,
        prod: &str,
        item: &str,
        column: &str,
        cols: &ReduceColumns,
    ) -> String {
        let col = column_name(column);
        let head = format!("ACTION[{state}, {col}] = reduce {production} ({prod}).");

        match self {
            ReduceStrategy::EveryColumn => format!(
                "{head} I{state} contains the completed item {item}: the whole right-hand side is on the stack, so it can be replaced by {prod}. LR(0) consults no lookahead at all, so this reduction is written into every terminal column and into $ - including {col}, whether or not {col} could ever legally follow here. That blanket rule is precisely why LR(0) is the weakest of the four methods."
            ),
            ReduceStrategy::Follow(_) => {
                let set = cols
                    .consulted
                    .as_ref()
                    .map(ConsultedSet::rendered)
                    .unwrap_or_default();
                let lhs = item.split(' ').next().unwrap_or("the left-hand side");
                let contrast = if cols.skipped.is_empty() {
                    format!(
                        " Every ACTION column is in {}, so here SLR(1) places exactly what LR(0) would have.",
                        cols.consulted.as_ref().map_or("FOLLOW", |c| c.name.as_str())
                    )
                } else {
                    format!(
                        " - and, unlike LR(0), not under {}.",
                        list_or(&cols.skipped)
                    )
                };
                format!(
                    "{head} State {state} holds the completed item {item}. In SLR(1) we reduce only on symbols that can legally follow {lhs}. {set}, so r{production} goes in {}{contrast}",
                    columns_phrase(cols.chosen.len())
                )
            }
            ReduceStrategy::ItemLookaheads { variant } => {
                let label = variant.label();
                let lhs = item.split(' ').next().unwrap_or("the left-hand side");
                let members = cols
                    .consulted
                    .as_ref()
                    .map(|c| c.members.join(", "))
                    .unwrap_or_default();
                let contrast = if cols.skipped.is_empty() {
                    format!(" Every ACTION column is in that set, so here {label} places exactly what LR(0) would have.")
                } else {
                    format!(" - and, unlike LR(0), not under {}.", list_or(&cols.skipped))
                };
                // Where the set came from is the one thing that differs between
                // the two methods that share this arm, and it is worth a
                // sentence: a LALR(1) set may be a union of several CLR(1) ones.
                let source = match variant {
                    Variant::Lalr1 => "Those lookaheads were worked out per state exactly as CLR(1) works them out, and then unioned over the states merged into this one - narrower than FOLLOW, but wider than any one CLR(1) state's.",
                    _ => "Those lookaheads were computed for this state alone, from the context it is reached in.",
                };
                format!(
                    "{head} State {state} holds the completed item {item}. {label} reduces only on the lookaheads written into the item itself, {{{members}}}, so r{production} goes in {}{contrast} {source} FOLLOW({lhs}) is not consulted here at all - that is the difference from SLR(1), which would use FOLLOW({lhs}) and so allow the reduction anywhere {lhs} can be followed in the whole grammar.",
                    columns_phrase(cols.chosen.len())
                )
            }
        }
    }
}


// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Conflict {
    pub state: usize,
    pub symbol: String,
    /// "shift-reduce" | "reduce-reduce"
    pub kind: String,
    /// Every action competing for the cell, in the order they were written.
    pub actions: Vec<Action>,
    /// LALR(1) only: true when the CLR(1) table has no conflict in the states
    /// this one was merged from, so merging is what caused it. Always false for
    /// the other three methods, which merge nothing.
    pub merge_induced: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Table {
    /// state -> terminal (or `$`) -> actions. More than one action is a conflict.
    pub action: BTreeMap<usize, BTreeMap<String, Vec<Action>>>,
    /// state -> non-terminal -> state.
    pub goto: BTreeMap<usize, BTreeMap<String, usize>>,
    pub conflicts: Vec<Conflict>,
    /// The item responsible for each ACTION cell, for the UI's hover panel.
    pub sources: BTreeMap<usize, BTreeMap<String, Item>>,
    pub states: usize,
    /// How many of `conflicts` exist only because LALR(1) merged two states.
    pub merge_induced: usize,
}

impl Table {
    pub fn actions_at(&self, state: usize, symbol: &str) -> &[Action] {
        self.action
            .get(&state)
            .and_then(|row| row.get(symbol))
            .map_or(&[], Vec::as_slice)
    }

    pub fn goto_at(&self, state: usize, nonterminal: &str) -> Option<usize> {
        self.goto.get(&state).and_then(|row| row.get(nonterminal)).copied()
    }

    /// Terminals with an entry in this state's ACTION row, for "expected: ..." .
    pub fn expected_at(&self, grammar: &Grammar, state: usize) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for col in grammar.action_columns() {
            if !self.actions_at(state, &col).is_empty() {
                out.push(col);
            }
        }
        out
    }

    pub fn shift_reduce_count(&self) -> usize {
        self.conflicts.iter().filter(|c| c.kind == "shift-reduce").count()
    }

    pub fn reduce_reduce_count(&self) -> usize {
        self.conflicts.iter().filter(|c| c.kind == "reduce-reduce").count()
    }

    pub fn is_valid(&self) -> bool {
        self.conflicts.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Telling a conflict the merge caused from one CLR(1) already had
// ---------------------------------------------------------------------------

/// What a LALR(1) state was merged from, alongside the CLR(1) table over the
/// same grammar. A conflict is blamed on the merge only by comparison: none of
/// the CLR(1) states it came from conflicts in that cell.
pub struct MergeProvenance {
    /// Indexed by LALR(1) state id: the CLR(1) states it was merged from.
    pub groups: Vec<Vec<usize>>,
    /// The CLR(1) table over the same grammar, built with a scratch recorder.
    pub clr: Table,
    /// The CLR(1) states' items, so the sentence can speak about the lookahead
    /// sets that were separate until the merge.
    pub clr_items: Vec<Vec<Item>>,
}

/// Where one LALR(1) conflict came from.
enum MergeVerdict {
    /// Not a merged automaton at all - LR(0), SLR(1) or CLR(1).
    NotApplicable,
    /// A merged automaton, but CLR(1) has this conflict too.
    Inherited,
    /// Merging caused it, and here is the sentence that says so.
    Caused(String),
}

impl MergeProvenance {
    fn verdict(&self, state: usize, symbol: &str, kind: &str, actions: &[Action]) -> MergeVerdict {
        let Some(group) = self.groups.get(state) else { return MergeVerdict::Inherited };
        // A state merged from one state is a state that was not merged.
        if group.len() < 2 {
            return MergeVerdict::Inherited;
        }
        // If any state this one came from already held two actions here, the
        // collision predates the merge and the merge gets no blame for it.
        if group.iter().any(|src| self.clr.actions_at(*src, symbol).len() > 1) {
            return MergeVerdict::Inherited;
        }
        MergeVerdict::Caused(self.caused_explain(group, symbol, kind, actions))
    }

    /// The lookaheads one CLR(1) state gave the completed item for `production`.
    fn lookaheads_at(&self, state: usize, production: usize) -> Option<&[String]> {
        self.clr_items.get(state)?.iter().find_map(|item| {
            if item.production != production {
                return None;
            }
            item.lookaheads.as_deref()
        })
    }

    fn caused_explain(
        &self,
        group: &[usize],
        symbol: &str,
        kind: &str,
        actions: &[Action],
    ) -> String {
        let col = column_name(symbol);
        let listed = and_list(group);

        // "separately each had a single lookahead" is the flagship case and the
        // clearest way to say it - but only when it is true of every state and
        // every reduction involved. Otherwise say the weaker thing, which is
        // what the comparison actually established.
        let each_single = actions.iter().all(|a| match a {
            Action::Reduce { production } => group
                .iter()
                .all(|src| self.lookaheads_at(*src, *production).is_some_and(|l| l.len() == 1)),
            _ => false,
        });
        let separately = if each_single {
            "separately each had a single lookahead".to_string()
        } else {
            "separately not one of them allowed both of these actions here".to_string()
        };
        let both = if actions.len() == 2 {
            "both reductions are".to_string()
        } else {
            format!("all {} reductions are", actions.len())
        };

        format!(
            " This {kind} conflict did not exist in CLR(1). It appeared because states {listed} were merged: {separately}, but the union means {both} now possible on {col}. This is the price LALR(1) pays for its smaller table, and it is why some grammars are CLR(1) but not LALR(1)."
        )
    }
}

/// `I6 and I9`, `I6, I7 and I9`.
fn and_list(ids: &[usize]) -> String {
    let names: Vec<String> = ids.iter().map(|i| format!("I{i}")).collect();
    match names.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
}

// ---------------------------------------------------------------------------

/// Build the ACTION/GOTO table under `strategy`, recording one step per cell.
///
/// The automaton is taken as given; for LALR(1) `merged` records what each
/// state was merged from. Conflicts are counted per colliding action.
pub fn build(
    grammar: &Grammar,
    automaton: &Automaton,
    strategy: &ReduceStrategy,
    merged: Option<&MergeProvenance>,
    rec: &mut Recorder,
) -> Result<Table> {
    let mut table = Table { states: automaton.states.len(), ..Table::default() };

    for state in &automaton.states {
        // --- shifts and gotos, in grammar-appearance order -------------------
        for symbol in &grammar.symbol_order {
            let Some(target) = automaton.target(state.id, symbol) else { continue };
            let source = source_item_for(grammar, state, symbol)?;

            if grammar.is_nonterminal(symbol) {
                table
                    .goto
                    .entry(state.id)
                    .or_default()
                    .insert(symbol.clone(), target);
                rec.push(Step::TableCellSet {
                    state: state.id,
                    symbol: symbol.clone(),
                    action: Action::Goto { state: target },
                    source_item: source.clone(),
                    explain: format!(
                        "GOTO[{}, {}] = {}. After the parser reduces something to {}, it has just pushed {} onto the stack while in state I{}, and the automaton says that leaves it in I{}. GOTO holds no actions - it only records where non-terminals lead.",
                        state.id, symbol, target, symbol, symbol, state.id, target
                    ),
                })?;
            } else {
                write_action(&mut table, rec, strategy, merged, Entry {
                    state: state.id,
                    symbol: symbol.clone(),
                    action: Action::Shift { state: target },
                    source,
                    explain: shift_explain(state.id, symbol, target),
                })?;
            }
        }

        // --- reductions ------------------------------------------------------
        for item in &state.items {
            if !is_complete(grammar, item)? {
                continue;
            }
            if item.production == 0 {
                // S' -> S • means the input is fully recognised. Accept goes in
                // the $ column only: anything else still to read is an error.
                let explain = format!(
                    "ACTION[{}, $] = accept. I{} contains {}, the augmented production with the dot at the end, so the start symbol has been fully recognised. Accept is written in the $ column alone - if any input remained, this would not be a complete parse.",
                    state.id,
                    state.id,
                    item_display(grammar, item)?
                );
                write_action(&mut table, rec, strategy, merged, Entry {
                    state: state.id,
                    symbol: END.to_string(),
                    action: Action::Accept,
                    source: Some(item.clone()),
                    explain,
                })?;
                continue;
            }

            // The seam: everything above is shared, and this one call is the
            // only thing that differs between the four methods.
            let cols = strategy.columns_for(grammar, item)?;
            let prod = production_display(grammar, item.production)?;
            let shown = item_display(grammar, item)?;
            for col in &cols.chosen {
                write_action(&mut table, rec, strategy, merged, Entry {
                    state: state.id,
                    symbol: col.clone(),
                    action: Action::Reduce { production: item.production },
                    source: Some(item.clone()),
                    explain: strategy.reduce_explain(
                        state.id,
                        item.production,
                        &prod,
                        &shown,
                        col,
                        &cols,
                    ),
                })?;
            }
        }
    }

    let conflicts = table.conflicts.len();
    let valid = table.is_valid();
    rec.push(Step::TableDone {
        conflicts,
        is_valid: valid,
        class: class_of(strategy, valid),
        explain: table_done_explain(strategy, &table),
    })?;

    Ok(table)
}

/// One prospective ACTION entry, before it is known whether the cell is free.
struct Entry {
    state: usize,
    symbol: String,
    action: Action,
    /// The item the entry came from, for the UI's hover panel.
    source: Option<Item>,
    explain: String,
}

/// Write one ACTION cell, recording a conflict instead of overwriting.
fn write_action(
    table: &mut Table,
    rec: &mut Recorder,
    strategy: &ReduceStrategy,
    merged: Option<&MergeProvenance>,
    entry: Entry,
) -> Result<()> {
    let Entry { state, symbol, action, source, explain } = entry;
    let row = table.action.entry(state).or_default();
    let cell = row.entry(symbol.clone()).or_default();

    if cell.contains(&action) {
        // The same action arriving twice is not a conflict, just a repeat.
        return Ok(());
    }

    if let Some(existing) = cell.first().cloned() {
        let kind = classify(&existing, &action);
        cell.push(action.clone());
        let actions = cell.clone();
        let verdict = match merged {
            Some(provenance) => provenance.verdict(state, &symbol, kind, &actions),
            None => MergeVerdict::NotApplicable,
        };
        // Merging can never introduce a shift-reduce conflict: shifts follow
        // the core, which merging leaves alone.
        if matches!(verdict, MergeVerdict::Caused(_)) && kind == "shift-reduce" {
            return Err(crate::error::EngineError::new(
                "internal error: merging states by core produced a shift-reduce conflict that CLR(1) does not have. Merging cannot do that, so this is a bug in the engine, not in your grammar.",
            ));
        }
        let from_merge = matches!(verdict, MergeVerdict::Caused(_));
        if from_merge {
            table.merge_induced = table.merge_induced.saturating_add(1);
        }
        table.conflicts.push(Conflict {
            state,
            symbol: symbol.clone(),
            kind: kind.to_string(),
            actions,
            merge_induced: from_merge,
        });
        rec.push(Step::TableConflict {
            state,
            symbol: symbol.clone(),
            existing: existing.clone(),
            incoming: action,
            conflict_kind: kind.to_string(),
            merge_induced: from_merge,
            explain: conflict_explain(strategy, state, &symbol, &existing, kind, &verdict),
        })?;
        return Ok(());
    }

    cell.push(action.clone());
    if let Some(item) = source {
        table.sources.entry(state).or_default().insert(symbol.clone(), item.clone());
        rec.push(Step::TableCellSet { state, symbol, action, source_item: Some(item), explain })?;
    } else {
        rec.push(Step::TableCellSet { state, symbol, action, source_item: None, explain })?;
    }
    Ok(())
}

fn classify(existing: &Action, incoming: &Action) -> &'static str {
    match (existing, incoming) {
        (Action::Shift { .. }, _) | (_, Action::Shift { .. }) => "shift-reduce",
        // Accept is the reduction by the augmented production, so a collision
        // with a reduce is a reduce-reduce conflict.
        _ => "reduce-reduce",
    }
}

/// The first item in the state whose dot sits before `symbol`.
fn source_item_for(
    grammar: &Grammar,
    state: &crate::automaton::State,
    symbol: &str,
) -> Result<Option<Item>> {
    for item in &state.items {
        if crate::items::symbol_after_dot(grammar, item)?.as_deref() == Some(symbol) {
            return Ok(Some(item.clone()));
        }
    }
    Ok(None)
}

// --- explanation text ------------------------------------------------------

/// `$` bare, everything else in backticks: `$` is not a symbol of the grammar.
fn column_name(symbol: &str) -> String {
    if symbol == END {
        "$".to_string()
    } else {
        format!("`{symbol}`")
    }
}

/// `` `*` ``, `` `*` or `(` ``, `` `*`, `(` or `id` ``.
fn list_or(columns: &[String]) -> String {
    let names: Vec<String> = columns.iter().map(|c| column_name(c)).collect();
    match names.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
    }
}

/// "that one column" / "those three columns", for reading aloud.
fn columns_phrase(n: usize) -> String {
    const WORDS: [&str; 11] =
        ["no", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten"];
    match n {
        0 => "no columns at all".to_string(),
        1 => "that one column".to_string(),
        n => format!(
            "those {} columns",
            WORDS.get(n).copied().map_or_else(|| n.to_string(), str::to_string)
        ),
    }
}

fn shift_explain(state: usize, symbol: &str, target: usize) -> String {
    format!(
        "ACTION[{state}, {symbol}] = shift {target}. The automaton has an edge from I{state} to I{target} labelled {symbol}, which means some item in I{state} has its dot immediately before {symbol}. So if the next input symbol is {symbol}, the parser pushes it and moves to I{target}."
    )
}

fn conflict_explain(
    strategy: &ReduceStrategy,
    state: usize,
    symbol: &str,
    existing: &Action,
    kind: &str,
    merge: &MergeVerdict,
) -> String {
    let col = column_name(symbol);
    let blind = match strategy {
        ReduceStrategy::EveryColumn => "LR(0) has no information with which to choose, because it never looks at the lookahead.".to_string(),
        ReduceStrategy::Follow(_) => format!("SLR(1) does look at {col} - but {col} is in the FOLLOW set of the completed item's left-hand side, so the reduction is legal here too, and one symbol of lookahead is not enough to separate the two."),
        // Merging cannot cause a shift-reduce conflict - the shifts follow the
        // core, which merging leaves alone - so under LALR(1) this cell says
        // exactly what the CLR(1) states behind it said.
        ReduceStrategy::ItemLookaheads { variant: Variant::Lalr1 } => format!("LALR(1) has already narrowed this reduction to the lookaheads the item carries in this state, and {col} is among them, so the reduction really is legal here, and so is the shift. Merging is not what did this: a merge can only bring reductions together, because every state merged into this one reads {col} to the same place. CLR(1) has the same conflict in the state this one came from, so no LR method with one symbol of lookahead will clear it."),
        ReduceStrategy::ItemLookaheads { .. } => format!("CLR(1) has already narrowed this reduction as far as one symbol of lookahead can go: {col} is among the lookaheads this very state computed for the item, so the reduction really is legal here, and so is the shift. No amount of extra context would separate them, because the context is already exact. A conflict that survives CLR(1) survives every LR method."),
    };
    let both_reduce = match strategy {
        ReduceStrategy::EveryColumn => format!(
            "Two different completed items in I{state} both want to reduce on {col}. LR(0) writes every completed item into every column, so two reductions in one state always collide."
        ),
        ReduceStrategy::Follow(_) => format!(
            "Two different completed items in I{state} both want to reduce on {col}, because {col} is in the FOLLOW set of both left-hand sides. SLR(1) narrows each reduction to its own FOLLOW set, but where two of those sets overlap it has nothing left to separate them with."
        ),
        ReduceStrategy::ItemLookaheads { variant: Variant::Lalr1 } => format!(
            "Two different completed items in I{state} both want to reduce on {col}, and both carry {col} as a lookahead in this state."
        ),
        ReduceStrategy::ItemLookaheads { .. } => format!(
            "Two different completed items in I{state} both want to reduce on {col}, and both really do carry {col} as a lookahead in this state. CLR(1) works those sets out from the exact context the state is reached in, so this is not a set that could be sharpened - one symbol ahead, the two reductions are genuinely indistinguishable."
        ),
    };
    let detail = match kind {
        "shift-reduce" => match existing {
            Action::Shift { state: to } => format!(
                "The cell already said shift {to}: on seeing {col} the parser could read one more symbol. The new entry says reduce instead. Both are possible. {blind}"
            ),
            _ => format!(
                "The cell already held a reduction, and a shift now wants the same cell. On seeing {col} the parser could either read one more symbol or reduce what it already has. {blind}"
            ),
        },
        _ => both_reduce,
    };
    // Say whether the merge is to blame either way, so "every LALR conflict is
    // a merge" is not the takeaway.
    let provenance = match merge {
        MergeVerdict::Caused(sentence) => sentence.clone(),
        MergeVerdict::Inherited if kind == "reduce-reduce" =>
            " Merging is not to blame for this one: CLR(1) conflicts in the same cell, so the two reductions were already indistinguishable one symbol ahead before any states were folded together.".to_string(),
        _ => String::new(),
    };
    format!("Conflict at ACTION[{state}, {col}] - a {kind} conflict. {detail}{provenance} The cell keeps both entries so you can see the collision; a real parser would have to pick one.")
}

fn table_done_explain(strategy: &ReduceStrategy, table: &Table) -> String {
    let conflicts = table.conflicts.len();
    if table.is_valid() {
        return match strategy {
            ReduceStrategy::EveryColumn => "The table is complete and every cell holds at most one action, so this grammar is LR(0). A parser can decide what to do knowing only which state it is in - it never needs to look at the next input symbol.".to_string(),
            ReduceStrategy::Follow(_) => "The table is complete and every cell holds at most one action, so this grammar is SLR(1). Confining each reduction to the columns of FOLLOW was enough to keep it out of every cell another action wanted; one symbol of lookahead, consulted in one place, is all this grammar needs.".to_string(),
            ReduceStrategy::ItemLookaheads { variant: Variant::Lalr1 } => "The table is complete and every cell holds at most one action, so this grammar is LALR(1). Merging cost it nothing here: every state that was folded together could be folded safely, and the reductions still land only where the items say they may. This is the table a real parser generator would produce for this grammar - CLR(1)'s reduce placement, in an LR(0)-sized table.".to_string(),
            ReduceStrategy::ItemLookaheads { .. } => "The table is complete and every cell holds at most one action, so this grammar is CLR(1). Each reduction went only where the item that produced it said it could, using lookaheads computed for that state alone - so where SLR(1) would have had to allow a reduction everywhere FOLLOW permits, this table allows it only where it is actually correct. The cost is the larger automaton; the benefit is this table.".to_string(),
        };
    }
    format!(
        "The table is complete but {} cell{} hold{} more than one action. {} {}",
        conflicts,
        if conflicts == 1 { "" } else { "s" },
        if conflicts == 1 { "s" } else { "" },
        verdict_message(strategy, table),
        VERDICT_NOTE
    )
}

pub const VERDICT_NOTE: &str = "A conflict shows only that this method cannot parse this grammar. It is not a proof of ambiguity: the grammar may be perfectly unambiguous and simply need a more powerful parser.";

pub fn class_of(strategy: &ReduceStrategy, valid: bool) -> String {
    let label = strategy.label();
    if valid {
        label.to_string()
    } else {
        format!("not {label}")
    }
}

/// The headline sentence. Deliberately free of the word "ambiguous": the
/// caveat lives in [`VERDICT_NOTE`] so this line only ever states what a
/// conflict actually proves. A test pins that.
pub fn verdict_message(strategy: &ReduceStrategy, table: &Table) -> String {
    let label = strategy.label();
    if table.is_valid() {
        return match strategy {
            ReduceStrategy::EveryColumn => "This grammar is LR(0). Every cell of the table holds exactly one action, so a parser can run without ever looking ahead.".to_string(),
            ReduceStrategy::Follow(_) => "This grammar is SLR(1). Every cell of the table holds exactly one action, so a parser guided by FOLLOW never has to guess.".to_string(),
            ReduceStrategy::ItemLookaheads { variant: Variant::Lalr1 } => "This grammar is LALR(1). Every cell of the table holds exactly one action, and it does so after merging - so this grammar gets CLR(1)'s reduce placement in an LR(0)-sized table.".to_string(),
            ReduceStrategy::ItemLookaheads { .. } => "This grammar is CLR(1). Every cell of the table holds exactly one action, so a parser guided by each item's own lookaheads never has to guess.".to_string(),
        };
    }
    let total = table.conflicts.len();
    let sr = table.shift_reduce_count();
    let rr = table.reduce_reduce_count();
    let mut kinds: Vec<String> = Vec::new();
    if sr > 0 {
        kinds.push(format!("{sr} shift-reduce"));
    }
    if rr > 0 {
        kinds.push(format!("{rr} reduce-reduce"));
    }
    let first = table
        .conflicts
        .first()
        .map(|c| {
            let sym = if c.symbol == END { "$".to_string() } else { c.symbol.clone() };
            format!(", first at state {} on {}", c.state, sym)
        })
        .unwrap_or_default();
    // LALR(1) only, and the most useful thing this tool can say about it: how
    // many of these the merge is responsible for. Measured against the CLR(1)
    // table, never guessed at.
    let from_merge = match table.merge_induced {
        0 => String::new(),
        1 => " One of them exists only because LALR(1) merged two states that CLR(1) keeps apart.".to_string(),
        2 if total == 2 => " Both of them exist only because LALR(1) merged states that CLR(1) keeps apart.".to_string(),
        n if n == total => format!(
            " All {n} exist only because LALR(1) merged states that CLR(1) keeps apart."
        ),
        n => format!(
            " {n} of them exist only because LALR(1) merged states that CLR(1) keeps apart."
        ),
    };
    format!(
        "This grammar is not {} - {} conflict{} ({}){}.{from_merge}",
        label,
        total,
        if total == 1 { "" } else { "s" },
        kinds.join(", "),
        first
    )
}

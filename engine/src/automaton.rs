//! The canonical collection of item sets, LR(0) or LR(1) depending on `ItemMode`.
//!
//! GOTO iterates symbols in order of first appearance in the grammar, so state
//! numbers match the textbooks. LALR(1) is deliberately built by constructing
//! the full CLR(1) collection and then merging same-core states: the merge is
//! what is taught.

use crate::error::{EngineError, Result};
use crate::grammar::Grammar;
use crate::items::{
    closure, core_signature, goto_kernel, item_display, next_symbol, signature, weight, widen,
    ClosureAddition, ItemMode,
};
use crate::limits::{self, Stage, Work};
use crate::sets::Sets;
use crate::trace::{Item, Recorder, Step};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct State {
    pub id: usize,
    /// The items the state was created from, before closure.
    pub kernel: Vec<Item>,
    /// Kernel plus everything closure added, in the order they were added.
    pub items: Vec<Item>,
    pub from_state: Option<usize>,
    pub on_symbol: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Transition {
    pub from: usize,
    pub to: usize,
    pub symbol: String,
}

#[derive(Debug, Clone)]
pub struct Automaton {
    pub states: Vec<State>,
    pub transitions: Vec<Transition>,
    /// `transitions` again, indexed by source state and then by symbol, so that
    /// [`Automaton::target`] is a lookup. Built once by [`Automaton::new`].
    edges: Vec<HashMap<String, usize>>,
}

impl Automaton {
    /// An automaton over these states and transitions. Nothing is added to
    /// either afterwards, which is what lets the edge index be built once.
    pub fn new(states: Vec<State>, transitions: Vec<Transition>) -> Self {
        let mut edges: Vec<HashMap<String, usize>> = Vec::new();
        for t in &transitions {
            if edges.len() <= t.from {
                edges.resize_with(t.from.saturating_add(1), HashMap::new);
            }
            if let Some(row) = edges.get_mut(t.from) {
                // The first edge listed for a (state, symbol) pair wins.
                row.entry(t.symbol.clone()).or_insert(t.to);
            }
        }
        Automaton { states, transitions, edges }
    }

    pub fn state(&self, id: usize) -> Result<&State> {
        self.states
            .get(id)
            .ok_or_else(|| EngineError::new(format!("internal error: no state I{id}")))
    }

    /// The target of `goto(I_from, symbol)`, if there is one.
    pub fn target(&self, from: usize, symbol: &str) -> Option<usize> {
        self.edges.get(from)?.get(symbol).copied()
    }
}

/// Build the canonical collection, recording a step for every item and edge.
pub fn build(grammar: &Grammar, mode: ItemMode, rec: &mut Recorder) -> Result<Automaton> {
    build_collection(grammar, mode, false, rec)
}

/// An LALR(1) build: the merged collection, the CLR(1) one it came from, and the mapping.
pub struct Lalr {
    /// The CLR(1) collection, before merging.
    pub canonical: Automaton,
    /// The LALR(1) collection: same automaton, same-core states folded.
    pub merged: Automaton,
    /// Indexed by merged state id: the CLR(1) states it was merged from.
    pub groups: Vec<Vec<usize>>,
}

/// Build the LALR(1) collection: the full CLR(1) one, then merged by core.
pub fn build_lalr(grammar: &Grammar, sets: &Sets, rec: &mut Recorder) -> Result<Lalr> {
    let canonical = build_collection(grammar, ItemMode::Lr1(sets), true, rec)?;
    let (merged, groups) = merge_by_core(grammar, &canonical, rec)?;
    Ok(Lalr { canonical, merged, groups })
}

/// `will_merge` only changes the wording: this collection is the input to a merge.
fn build_collection(
    grammar: &Grammar,
    mode: ItemMode,
    will_merge: bool,
    rec: &mut Recorder,
) -> Result<Automaton> {
    rec.work().enter(Stage::ItemSets);
    let mut states: Vec<State> = Vec::new();
    let mut transitions: Vec<Transition> = Vec::new();
    // Closed item sets seen so far; under CLR(1) the key includes lookaheads,
    // which is what splits states.
    let mut seen: HashMap<Vec<Item>, usize> = HashMap::new();
    // Cores seen so far, so a CLR(1) split can name the state it split from.
    let mut cores: HashMap<Vec<Item>, usize> = HashMap::new();
    // Speed only, changes no answer: a memo of which state a kernel led to.
    let mut traced: HashMap<Vec<Item>, usize> = HashMap::new();

    // I0 is the closure of the augmented production with the dot at the start.
    let start = vec![mode.start_item()];
    let closed = closure(grammar, mode, &start, rec.work())?;
    create_state(
        grammar,
        mode,
        &mut states,
        &mut seen,
        &mut cores,
        rec,
        Fresh { kernel: start, closed, from_state: None, on_symbol: None },
        will_merge,
    )?;

    let mut cursor = 0;
    while cursor < states.len() {
        let (id, items) = match states.get(cursor) {
            Some(s) => (s.id, s.items.clone()),
            None => break,
        };
        cursor += 1;
        rec.work().spend(weight(&items))?;

        // Only symbols some item has its dot before can have a non-empty GOTO.
        let mut readable: HashSet<&str> = HashSet::new();
        for item in &items {
            if let Some(symbol) = next_symbol(grammar, item)? {
                readable.insert(symbol);
            }
        }

        for symbol in &grammar.symbol_order {
            if !readable.contains(symbol.as_str()) {
                continue;
            }
            rec.work().spend(items.len())?;
            let kernel = goto_kernel(grammar, &items, symbol)?;
            if kernel.is_empty() {
                continue;
            }
            rec.work().show(weight(&kernel))?;

            let (target, is_new) = match traced.get(&kernel).copied() {
                Some(existing) => (existing, false),
                None => {
                    // Compare on the full closed set, lookaheads included.
                    let closed = closure(grammar, mode, &kernel, rec.work())?;
                    rec.work().spend(weight(&closed.0))?;
                    let found = seen.get(&signature(&closed.0)).copied();
                    let resolved = match found {
                        Some(existing) => (existing, false),
                        None => (
                            create_state(
                                grammar,
                                mode,
                                &mut states,
                                &mut seen,
                                &mut cores,
                                rec,
                                Fresh {
                                    kernel: kernel.clone(),
                                    closed,
                                    from_state: Some(id),
                                    on_symbol: Some(symbol.clone()),
                                },
                                will_merge,
                            )?,
                            true,
                        ),
                    };
                    traced.insert(kernel.clone(), resolved.0);
                    resolved
                }
            };

            transitions.push(Transition { from: id, to: target, symbol: symbol.clone() });
            rec.push(Step::TransitionAdded {
                from: id,
                to: target,
                symbol: symbol.clone(),
                target_is_new: is_new,
                explain: transition_explain(grammar, id, target, symbol, is_new, &kernel)?,
            })?;
        }
    }

    rec.push(Step::AutomatonDone {
        state_count: states.len(),
        explain: automaton_done_explain(mode, states.len(), transitions.len(), &states, will_merge),
    })?;

    Ok(Automaton::new(states, transitions))
}

/// A state about to be created: its kernel, the closure of that kernel, and
/// where it was reached from.
struct Fresh {
    kernel: Vec<Item>,
    /// `closure(kernel)`, already computed by the caller.
    closed: (Vec<Item>, Vec<ClosureAddition>),
    from_state: Option<usize>,
    on_symbol: Option<String>,
}

/// Create a state from a kernel and its closure: emit its creation, one step
/// per item the closure added, then the closed set. Returns the new state's id.
#[allow(clippy::too_many_arguments)]
fn create_state(
    grammar: &Grammar,
    mode: ItemMode,
    states: &mut Vec<State>,
    seen: &mut HashMap<Vec<Item>, usize>,
    cores: &mut HashMap<Vec<Item>, usize>,
    rec: &mut Recorder,
    fresh: Fresh,
    will_merge: bool,
) -> Result<usize> {
    if states.len() >= limits::MAX_STATES {
        return Err(EngineError::new(format!(
            "This grammar produces more than {} states. That is too large to display usefully - try a smaller grammar.",
            limits::MAX_STATES
        ))
        .hint("Large state counts usually come from a tokenization surprise. Check the symbol list in the Grammar tab against what you meant to write."));
    }

    let Fresh { kernel, closed: (items, additions), from_state, on_symbol } = fresh;
    let id = states.len();
    rec.work().show(weight(&kernel).saturating_add(weight(&items)))?;
    rec.work().spend(weight(&items).saturating_mul(2))?;
    // Under CLR(1) a new state may share its core with an existing one; the
    // explanation names that twin.
    let core = core_signature(&items);
    let twin = if mode.carries_lookaheads() { cores.get(&core).copied() } else { None };

    rec.push(Step::StateCreated {
        id,
        kernel: kernel.clone(),
        from_state,
        on_symbol: on_symbol.clone(),
        explain: state_created_explain(
            grammar,
            id,
            &kernel,
            from_state,
            on_symbol.as_deref(),
            twin,
            will_merge,
        )?,
    })?;

    for add in &additions {
        rec.push(Step::ClosureItemAdded {
            state: id,
            item: add.item.clone(),
            because_of: add.because_of.clone(),
            explain: closure_explain(grammar, id, add)?,
        })?;
    }

    rec.push(Step::StateClosed {
        id,
        items: items.clone(),
        explain: format!(
            "I{} is closed: {} kernel item{} plus {} added by closure, {} in total. Nothing further can be added, so the state is final.",
            id,
            kernel.len(),
            if kernel.len() == 1 { "" } else { "s" },
            additions.len(),
            items.len()
        ),
    })?;

    seen.insert(signature(&items), id);
    cores.entry(core).or_insert(id);
    states.push(State { id, kernel, items, from_state, on_symbol });
    Ok(id)
}

/// The explanation for one closure addition; items with lookaheads also say where they came from.
fn closure_explain(
    grammar: &Grammar,
    id: usize,
    add: &crate::items::ClosureAddition,
) -> Result<String> {
    let nt = &add.nonterminal;
    let base = format!(
        "Added {} to the closure of I{} because the dot in {} sits before the non-terminal {}. To recognise {} the parser must next start recognising one of {}'s productions, so every one of them joins the state with its dot at the beginning.",
        item_display(grammar, &add.item)?,
        id,
        item_display(grammar, &add.because_of)?,
        nt,
        nt,
        nt
    );
    // LR(0) stops here.
    let Some(beta) = &add.beta else { return Ok(base) };

    let set = add.item.lookaheads.as_ref().map(|l| l.join(", ")).unwrap_or_default();
    let parent = add
        .because_of
        .lookaheads
        .as_ref()
        .map(|l| l.join(", "))
        .unwrap_or_default();

    let source = if beta.is_empty() {
        format!(
            "Nothing follows {nt} in that item, so the new item simply inherits the lookaheads of the item that spawned it: {{{parent}}}."
        )
    } else {
        // β is written out on its own: spliced into FIRST(...), a β ending in
        // a bracket would make the sentence unreadable.
        format!(
            "The lookaheads {{{set}}} come from FIRST(β a), where β is whatever follows {nt} in that item - here `{}` - and a runs over the lookaheads the spawning item carries, {{{parent}}}. That is everything which can begin β, plus - only where β can vanish entirely - a itself.",
            beta.join(" ")
        )
    };

    let merged = if add.other_causes == 0 {
        String::new()
    } else {
        format!(
            " {} other item{} in I{id} put the dot before {nt} as well; their lookaheads are merged into the one set shown rather than kept as a second copy of the same item.",
            add.other_causes,
            if add.other_causes == 1 { "" } else { "s" }
        )
    };

    Ok(format!(
        "{base} {source} Either way the set is worked out for this state alone, from the context this state is reached in - never from FOLLOW({nt}), which is the union over every context at once. Working the lookaheads out per state rather than once for the whole grammar is exactly what separates CLR(1) and LALR(1) from SLR(1).{merged}"
    ))
}

fn automaton_done_explain(
    mode: ItemMode,
    state_count: usize,
    transitions: usize,
    states: &[State],
    will_merge: bool,
) -> String {
    let base = format!(
        "The canonical collection is complete: {} states, I0 to I{}, joined by {} transitions. Every state is a set of items describing exactly what the parser could be in the middle of recognising, given everything it has read so far.",
        state_count,
        state_count.saturating_sub(1),
        transitions
    );
    if !mode.carries_lookaheads() {
        return base;
    }
    // Distinct LR(0) cores: the size the SLR(1) automaton would have had.
    let mut cores: Vec<Vec<Item>> = states.iter().map(|s| core_signature(&s.items)).collect();
    cores.sort();
    cores.dedup();
    let split = state_count.saturating_sub(cores.len());
    if will_merge {
        // LALR(1) run: this collection is the input to the merge, so hand over to it.
        let handover = if split == 0 {
            format!(
                " No two of these {state_count} states share a core, though, so there is nothing here for LALR(1) to merge. For this grammar the two methods build the very same automaton, and they must therefore agree on every cell of the table."
            )
        } else {
            format!(
                " These {state_count} states cover only {} distinct cores, so {} state{} {} another that holds the same items with the dots in the same places, differing only in lookahead. LALR(1) now folds each of those groups into a single state and unions their lookaheads. Watch what merges, and what it costs.",
                cores.len(),
                split,
                if split == 1 { "" } else { "s" },
                if split == 1 { "duplicates the core of" } else { "duplicate the cores of" }
            )
        };
        return format!(
            "{base} That is the canonical LR(1) collection, exactly as CLR(1) builds it - and it is where LALR(1) starts rather than where it stops.{handover}"
        );
    }
    let tail = if split == 0 {
        " No two states here share a core, so CLR(1) built exactly the collection LR(0) would have - this grammar simply needed the lookaheads written down, not the states pulled apart.".to_string()
    } else {
        format!(
            " These {state_count} states cover only {} distinct cores: {split} of them {} a state LR(0) would have written once, split here because the same items can be reached with different lookaheads. That larger table is the price of CLR(1), and the extra strength is what it buys.",
            cores.len(),
            if split == 1 { "duplicates" } else { "duplicate" }
        )
    };
    format!("{base}{tail}")
}

fn state_created_explain(
    grammar: &Grammar,
    id: usize,
    kernel: &[Item],
    from_state: Option<usize>,
    on_symbol: Option<&str>,
    same_core_as: Option<usize>,
    will_merge: bool,
) -> Result<String> {
    let listed = kernel
        .iter()
        .map(|i| item_display(grammar, i))
        .collect::<Result<Vec<_>>>()?
        .join(", ");

    match (from_state, on_symbol) {
        (Some(from), Some(sym)) => {
            // Under LALR(1) a same-core twin is about to be merged; foreshadow it.
            let novelty = match (same_core_as, will_merge) {
                (Some(twin), false) => format!(
                    "I{twin} already holds the same items with the dots in the same places - but not the same lookaheads, and under CLR(1) that makes this a different state. Splitting the two apart is exactly what lets CLR(1) reduce in one context and not the other."
                ),
                (Some(twin), true) => format!(
                    "I{twin} already holds the same items with the dots in the same places, differing only in lookahead - and the canonical LR(1) collection keeps the two apart, so this is a new state. Remember the pair: this is exactly what LALR(1) will fold back together in the merging stage, once the collection is complete."
                ),
                (None, _) => "No existing state has this item set, so this is a genuinely new state.".to_string(),
            };
            Ok(format!(
                "New state I{id} = GOTO(I{from}, {sym}). Every item in I{from} whose dot sat before {sym} has had the dot moved past it, giving the kernel {listed}. {novelty}",
            ))
        }
        _ => Ok(format!(
            "State I{id} is the starting state. Its kernel is {listed}: the parser has read nothing, and the whole job ahead of it is to recognise the start symbol {}.",
            grammar.original_start
        )),
    }
}

fn transition_explain(
    grammar: &Grammar,
    from: usize,
    to: usize,
    symbol: &str,
    is_new: bool,
    kernel: &[Item],
) -> Result<String> {
    let sources = kernel
        .iter()
        .map(|i| -> Result<String> {
            let before = Item { production: i.production, dot: i.dot.saturating_sub(1), lookaheads: i.lookaheads.clone() };
            item_display(grammar, &before)
        })
        .collect::<Result<Vec<_>>>()?
        .join(", ");

    let kind = if grammar.is_nonterminal(symbol) {
        format!("This is a non-terminal, so it becomes a GOTO entry: GOTO[{from}, {symbol}] = {to}.")
    } else {
        format!("This is a terminal, so it becomes a shift: ACTION[{from}, {symbol}] = shift {to}.")
    };

    let novelty = if is_new {
        format!("The target I{to} was created just now.")
    } else {
        format!("The target item set already exists as I{to}, so no new state is created - the edge simply points back at it.")
    };

    Ok(format!(
        "Reading {symbol} in I{from} moves the dot past it in {sources}, taking the parser to I{to}. {novelty} {kind}"
    ))
}


// ---------------------------------------------------------------------------
// LALR(1): merging by core
// ---------------------------------------------------------------------------

/// Fold every group of same-core states of `clr` into one, union their
/// lookaheads, renumber, and redirect every transition.
///
/// Renumbering happens once, in order of each group's smallest original id, so
/// I0 stays I0. One `StatesMerged` per multi-state group, then `MergeDone`.
pub fn merge_by_core(
    grammar: &Grammar,
    clr: &Automaton,
    rec: &mut Recorder,
) -> Result<(Automaton, Vec<Vec<usize>>)> {
    rec.work().enter(Stage::ItemSets);
    // --- group the CLR(1) states by core -----------------------------------
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut group_of: Vec<usize> = vec![0; clr.states.len()];
    let mut index_of: HashMap<Vec<Item>, usize> = HashMap::new();
    for state in &clr.states {
        rec.work().spend(weight(&state.items))?;
        let core = core_signature(&state.items);
        let at = match index_of.get(&core) {
            Some(at) => *at,
            None => {
                let at = groups.len();
                index_of.insert(core, at);
                groups.push(Vec::new());
                at
            }
        };
        groups.get_mut(at).ok_or_else(lost)?.push(state.id);
        *group_of.get_mut(state.id).ok_or_else(lost)? = at;
    }

    // --- one merged state per group ----------------------------------------
    let mut states: Vec<State> = Vec::new();
    for (new_id, group) in groups.iter().enumerate() {
        let first = *group.first().ok_or_else(lost)?;
        let rep = clr.state(first)?;
        let mut items = rep.items.clone();
        let mut kernel = rep.kernel.clone();
        for other in group.iter().skip(1) {
            let state = clr.state(*other)?;
            union_lookaheads(grammar, &mut items, &state.items, rec.work())?;
            union_lookaheads(grammar, &mut kernel, &state.kernel, rec.work())?;
        }
        let from_state = match rep.from_state {
            Some(from) => Some(*group_of.get(from).ok_or_else(lost)?),
            None => None,
        };
        states.push(State { id: new_id, kernel, items, from_state, on_symbol: rep.on_symbol.clone() });
    }

    // --- redirect the transitions ------------------------------------------
    // Grammar-appearance order, as in a direct build. Same-core states always
    // agree on targets, so a disagreement is an engine bug.
    let mut transitions: Vec<Transition> = Vec::new();
    for (new_id, group) in groups.iter().enumerate() {
        for symbol in &grammar.symbol_order {
            let mut target: Option<usize> = None;
            for source in group {
                let Some(to) = clr.target(*source, symbol) else { continue };
                let mapped = *group_of.get(to).ok_or_else(lost)?;
                match target {
                    None => target = Some(mapped),
                    Some(seen) if seen != mapped => {
                        return Err(EngineError::new(
                            "internal error: merging states by core produced two different targets for the same symbol. Same-core states must lead to same-core states, so this is a bug in the engine, not in your grammar.",
                        ))
                    }
                    Some(_) => {}
                }
            }
            if let Some(to) = target {
                transitions.push(Transition { from: new_id, to, symbol: symbol.clone() });
            }
        }
    }

    // --- report -------------------------------------------------------------
    for (new_id, group) in groups.iter().enumerate() {
        if group.len() < 2 {
            continue;
        }
        let state = states.get(new_id).ok_or_else(lost)?;
        rec.work().show(weight(&state.items).saturating_mul(2))?;
        let core = display_core(&state.items);
        rec.push(Step::StatesMerged {
            merged_ids: group.clone(),
            new_id,
            core: core.clone(),
            items: state.items.clone(),
            explain: merged_explain(grammar, group, new_id, &core, &state.items)?,
        })?;
    }

    let folded = groups.iter().filter(|g| g.len() > 1).count();
    rec.push(Step::MergeDone {
        groups: groups.clone(),
        before: clr.states.len(),
        after: states.len(),
        before_label: "CLR(1)".to_string(),
        after_label: "LALR(1)".to_string(),
        explain: merge_done_explain(clr.states.len(), states.len(), folded),
    })?;

    Ok((Automaton::new(states, transitions), groups))
}

fn lost() -> EngineError {
    EngineError::new("internal error: a state went missing while merging by core")
}

/// The same items with the lookaheads stripped, in the state's own order.
fn display_core(items: &[Item]) -> Vec<Item> {
    items.iter().map(|i| Item::new(i.production, i.dot)).collect()
}

/// Merge `extra`'s lookaheads into the matching items of `target` (same cores, so each finds a partner).
fn union_lookaheads(
    grammar: &Grammar,
    target: &mut [Item],
    extra: &[Item],
    work: &mut Work,
) -> Result<()> {
    let mut position: HashMap<(usize, usize), usize> = HashMap::new();
    for (i, item) in target.iter().enumerate() {
        position.entry((item.production, item.dot)).or_insert(i);
    }
    work.spend(target.len().saturating_add(extra.len()))?;

    for item in extra {
        let slot = position
            .get(&(item.production, item.dot))
            .and_then(|at| target.get_mut(*at));
        let Some(slot) = slot else {
            return Err(EngineError::new(
                "internal error: two states grouped by core turned out to hold different items",
            ));
        };
        if let Some(set) = slot.lookaheads.as_mut() {
            widen(grammar, set, item.lookaheads.as_deref().unwrap_or(&[]), work)?;
        }
    }
    Ok(())
}

/// `I4 and I11`, `I4, I7 and I11`.
fn and_list(ids: &[usize]) -> String {
    let names: Vec<String> = ids.iter().map(|i| format!("I{i}")).collect();
    match names.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
}

fn merged_explain(
    grammar: &Grammar,
    group: &[usize],
    new_id: usize,
    core: &[Item],
    items: &[Item],
) -> Result<String> {
    let listed = and_list(group);
    let core_text = core
        .iter()
        .map(|i| item_display(grammar, i))
        .collect::<Result<Vec<_>>>()?
        .join(", ");
    let merged_text = items
        .iter()
        .map(|i| item_display(grammar, i))
        .collect::<Result<Vec<_>>>()?
        .join("; ");
    let numbering = if group.contains(&new_id) {
        format!("which keeps the number I{new_id} once the collection is renumbered")
    } else {
        format!("numbered I{new_id} once the collection is renumbered")
    };
    Ok(format!(
        "States {listed} have the same core {{ {core_text} }} and differ only in lookahead. LALR(1) merges them into one state, {numbering}, carrying the union of their lookahead sets: {merged_text}. Every edge that pointed at any of {listed} now points here, and the edges leaving them are the same edges, because two states with the same core always read the same symbols to the same places. That is why this merge cannot disturb a single shift - only a reduction can be affected, and only by the lookahead sets having grown."
    ))
}

fn merge_done_explain(before: usize, after: usize, folded: usize) -> String {
    if folded == 0 {
        return format!(
            "Merging is complete, and nothing merged: no two of the {before} states share a core, so the LALR(1) automaton is the CLR(1) automaton, state for state. For this grammar the two methods must agree on every cell of the table - LALR(1) can only ever differ from CLR(1) where it actually merges something."
        );
    }
    format!(
        "Merging is complete: {before} CLR(1) states became {after} LALR(1) states, with {folded} group{} folded together. That is the whole of LALR(1) - build the canonical LR(1) collection, then merge the states that differ only in lookahead. The result has exactly one state per core, which is exactly as many states as LR(0) builds, so LALR(1) gets an LR(0)-sized table while still placing its reductions by per-item lookahead rather than by FOLLOW. The bill for that arrives, if it arrives at all, as a reduce-reduce conflict: two reductions CLR(1) kept in separate states, each with its own lookahead, now share a state and the union of both. Merging can never cause a shift-reduce conflict, because the shifts follow the core and the core is exactly what merging leaves alone. The table is built next - that is where you find out whether this grammar had to pay.",
        if folded == 1 { "" } else { "s" }
    )
}

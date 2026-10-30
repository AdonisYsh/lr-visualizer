//! LR items, closure and goto, for LR(0) items and LR(1) items.
//!
//! Items with the same core are merged into one carrying a lookahead set,
//! `[A -> α • β, {a, b, c}]`, never kept as duplicates.

use crate::error::{EngineError, Result};
use crate::grammar::{Grammar, END, EPSILON};
use crate::limits::{self, Work};
use crate::sets::Sets;
use crate::trace::Item;
use std::collections::{HashMap, HashSet};

/// Whether items carry lookaheads, and what is needed to propagate them.
#[derive(Clone, Copy)]
pub enum ItemMode<'a> {
    /// LR(0) and SLR(1): `Item.lookaheads` stays `None` throughout.
    Lr0,
    /// CLR(1) and LALR(1): every item carries a lookahead set, propagated with FIRST(β a).
    Lr1(&'a Sets),
}

impl ItemMode<'_> {
    pub fn carries_lookaheads(&self) -> bool {
        matches!(self, ItemMode::Lr1(_))
    }

    /// The item I0 is built from: `S' -> • S`, with lookahead `$` when lookaheads are tracked.
    pub fn start_item(&self) -> Item {
        match self {
            ItemMode::Lr0 => Item::new(0, 0),
            ItemMode::Lr1(_) => Item::with_lookaheads(0, 0, vec![END.to_string()]),
        }
    }
}

/// `E -> E • + T`, or `E -> E • + T, {+, ), $}`, as shown in the diagram and explanations.
pub fn item_display(grammar: &Grammar, item: &Item) -> Result<String> {
    let p = grammar.production(item.production)?;
    let mut parts: Vec<String> = Vec::new();
    for (i, sym) in p.rhs.iter().enumerate() {
        if i == item.dot {
            parts.push("•".to_string());
        }
        parts.push(sym.clone());
    }
    if item.dot >= p.rhs.len() {
        parts.push("•".to_string());
    }
    let body = if p.rhs.is_empty() { "•".to_string() } else { parts.join(" ") };
    let base = format!("{} → {}", p.lhs, body);
    Ok(match &item.lookaheads {
        Some(las) if !las.is_empty() => format!("{base}, {{{}}}", las.join(", ")),
        _ => base,
    })
}

/// The symbol immediately after the dot, or `None` if the item is complete.
pub fn symbol_after_dot(grammar: &Grammar, item: &Item) -> Result<Option<String>> {
    Ok(next_symbol(grammar, item)?.map(str::to_string))
}

/// [`symbol_after_dot`] without the copy.
pub fn next_symbol<'g>(grammar: &'g Grammar, item: &Item) -> Result<Option<&'g str>> {
    let p = grammar.production(item.production)?;
    Ok(p.rhs.get(item.dot).map(String::as_str))
}

/// What an item set costs on the work meter: one unit per item and per lookahead.
pub fn weight(items: &[Item]) -> usize {
    items.iter().fold(0, |sum: usize, item| {
        sum.saturating_add(1).saturating_add(item.lookaheads.as_ref().map_or(0, Vec::len))
    })
}

/// True when the dot is at the far right and the production is ready to reduce.
pub fn is_complete(grammar: &Grammar, item: &Item) -> Result<bool> {
    Ok(symbol_after_dot(grammar, item)?.is_none())
}

/// One addition made by [`closure`]: the item added, and why.
pub struct ClosureAddition {
    /// The item in its final form, with every lookahead merged in.
    pub item: Item,
    /// The item whose dot sat before the non-terminal, in its final form.
    pub because_of: Item,
    /// The non-terminal sitting after the dot in `because_of`.
    pub nonterminal: String,
    /// LR(1) only: the β after the non-terminal in `because_of`, as in FIRST(β a).
    /// Empty when the non-terminal is last (lookaheads inherited); `None` for LR(0).
    pub beta: Option<Vec<String>>,
    /// LR(1) only: how many other items in the state also widened this item's lookaheads.
    pub other_causes: usize,
}

/// Close an item set: wherever the dot sits before a non-terminal `B`, add
/// `B -> • γ` for every production of `B`, to fixpoint. Returns the items in
/// the order added (kernel first) and the additions, each ready to be explained.
pub fn closure(
    grammar: &Grammar,
    mode: ItemMode,
    kernel: &[Item],
    work: &mut Work,
) -> Result<(Vec<Item>, Vec<ClosureAddition>)> {
    match mode {
        ItemMode::Lr0 => closure_lr0(grammar, kernel, work),
        ItemMode::Lr1(sets) => closure_lr1(grammar, sets, kernel, work),
    }
}

// --- LR(0) -----------------------------------------------------------------

fn closure_lr0(
    grammar: &Grammar,
    kernel: &[Item],
    work: &mut Work,
) -> Result<(Vec<Item>, Vec<ClosureAddition>)> {
    let mut items: Vec<Item> = kernel.to_vec();
    // Set copy of `items` for fast membership tests.
    let mut present: HashSet<Item> = items.iter().cloned().collect();
    let mut additions: Vec<ClosureAddition> = Vec::new();
    work.spend(items.len())?;

    // The index walk reaches items added as it goes, which is the fixpoint.
    let mut cursor = 0;
    while cursor < items.len() {
        let Some(item) = items.get(cursor).cloned() else { break };
        cursor += 1;
        let Some(next) = symbol_after_dot(grammar, &item)? else { continue };
        if !grammar.is_nonterminal(&next) {
            continue;
        }
        let productions = grammar.productions_for(&next);
        work.spend(productions.len().saturating_add(1))?;
        for prod_index in productions {
            let fresh = Item::new(*prod_index, 0);
            if present.contains(&fresh) {
                continue;
            }
            present.insert(fresh.clone());
            items.push(fresh.clone());
            additions.push(ClosureAddition {
                item: fresh,
                because_of: item.clone(),
                nonterminal: next.clone(),
                beta: None,
                other_causes: 0,
            });
        }
    }
    Ok((items, additions))
}

// --- CLR(1) ----------------------------------------------------------------

/// What happened when an item was merged into a set.
enum Merge {
    /// A new core, appended at the end of the set.
    Added,
    /// An existing core at this position gained at least one lookahead.
    Widened(usize),
    /// An existing core already had every lookahead offered.
    Unchanged,
}

/// An item set under construction, with an index from core to position.
struct Building {
    items: Vec<Item>,
    /// `(production, dot)` -> position in `items`; one item per core.
    position: HashMap<(usize, usize), usize>,
}

impl Building {
    fn new() -> Self {
        Building { items: Vec::new(), position: HashMap::new() }
    }

    /// Add `[production, dot, offered]`, merging into the item with the same
    /// core if there is one, so a state shows one line per core carrying a set.
    /// `offered` must already be in canonical display order.
    fn merge(
        &mut self,
        grammar: &Grammar,
        production: usize,
        dot: usize,
        offered: &[String],
        work: &mut Work,
    ) -> Result<Merge> {
        let Some(pos) = self.position.get(&(production, dot)).copied() else {
            work.spend(offered.len().saturating_add(1))?;
            self.position.insert((production, dot), self.items.len());
            self.items.push(Item { production, dot, lookaheads: Some(offered.to_vec()) });
            return Ok(Merge::Added);
        };
        let existing = self.items.get_mut(pos).ok_or_else(out_of_step)?;
        let Some(set) = existing.lookaheads.as_mut() else { return Ok(Merge::Unchanged) };
        Ok(if widen(grammar, set, offered, work)? { Merge::Widened(pos) } else { Merge::Unchanged })
    }
}

/// Add every member of `offered` to `set`, keeping canonical display order.
/// Returns true if anything was new.
///
/// Same result as calling [`Grammar::insert_sorted`] per member; the forward
/// walk is only for speed and changes no answer.
pub(crate) fn widen(
    grammar: &Grammar,
    set: &mut Vec<String>,
    offered: &[String],
    work: &mut Work,
) -> Result<bool> {
    work.spend(set.len().saturating_add(offered.len()).saturating_add(1))?;
    let mut from: usize = 0;
    let mut widened = false;
    for la in offered {
        let found = set.get(from..).and_then(|rest| rest.iter().position(|s| s == la));
        match found {
            Some(offset) => from = from.saturating_add(offset).saturating_add(1),
            None => {
                work.spend(set.len().saturating_add(1).saturating_mul(2))?;
                if grammar.insert_sorted(set, la) {
                    widened = true;
                }
            }
        }
    }
    Ok(widened)
}

/// An item's lookahead set in canonical display order, so equal sets compare equal.
fn canonical(grammar: &Grammar, item: &Item, work: &mut Work) -> Result<Vec<String>> {
    let given: &[String] = item.lookaheads.as_deref().unwrap_or(&[]);
    // Usually already in order; checking is cheaper than rebuilding.
    work.spend(given.len().saturating_add(1))?;
    if grammar.is_canonical(given) {
        return Ok(given.to_vec());
    }
    let mut set: Vec<String> = Vec::new();
    for la in given {
        work.spend(set.len().saturating_add(1))?;
        grammar.insert_sorted(&mut set, la);
    }
    Ok(set)
}

/// What one item hands down: FIRST(β a) for every lookahead `a` it carries.
///
/// `a` is a terminal or `$`, so this is FIRST(β), plus all of the item's
/// lookaheads when β can vanish; computed once per item from FIRST(β).
/// `None` when the item carries no lookaheads.
fn handed_down(
    grammar: &Grammar,
    sets: &Sets,
    item: Option<&Item>,
    beta: &[String],
    work: &mut Work,
) -> Result<Option<Vec<String>>> {
    let Some(own) = item.and_then(|i| i.lookaheads.as_ref()) else { return Ok(None) };
    if own.is_empty() {
        return Ok(None);
    }
    let (first_beta, vanishes, cost) = sets.first_of_sequence_costed(grammar, beta);
    work.spend(cost)?;
    if !vanishes {
        return Ok(Some(first_beta));
    }
    work.spend(own.len())?;
    if first_beta.is_empty() {
        // Nothing can start β: the lookaheads are inherited as they stand.
        return Ok(Some(own.clone()));
    }
    let mut offered = first_beta;
    widen(grammar, &mut offered, own, work)?;
    Ok(Some(offered))
}

/// The LR(1) closure: for `[A -> α • B β, a]` and each `B -> γ`, add
/// `[B -> • γ, b]` for every `b` in FIRST(β a).
///
/// The fixpoint runs to completion before any addition is reported, so each
/// added item is explained once, in its final merged form.
fn closure_lr1(
    grammar: &Grammar,
    sets: &Sets,
    kernel: &[Item],
    work: &mut Work,
) -> Result<(Vec<Item>, Vec<ClosureAddition>)> {
    let mut building = Building::new();
    for k in kernel {
        // The kernel comes from outside, so canonicalise rather than trust it.
        let own = canonical(grammar, k, work)?;
        building.merge(grammar, k.production, k.dot, &own, work)?;
    }
    let kernel_len = building.items.len();

    // Aligned with `items`: (causing position, non-terminal after its dot, β)
    // for the first cause, and every distinct cause in `contributors`.
    let mut cause: Vec<Option<(usize, String, Vec<String>)>> = vec![None; kernel_len];
    let mut contributors: Vec<Vec<usize>> = vec![Vec::new(); kernel_len];
    // Speed only, changes no answer: sets only grow, so an item whose lookahead
    // count is unchanged since its last turn has nothing new to hand down.
    let mut handed: Vec<Option<usize>> = Vec::new();

    let mut settled = false;
    for _ in 0..limits::MAX_FIXPOINT_ROUNDS {
        let mut changed = false;
        let mut cursor = 0;
        // The walk reaches new items; the outer loop catches sets widened behind it.
        while cursor < building.items.len() {
            let Some((production, dot)) =
                building.items.get(cursor).map(|i| (i.production, i.dot))
            else {
                break;
            };
            let at = cursor;
            cursor += 1;
            work.spend(1)?;

            let carried = building
                .items
                .get(at)
                .and_then(|i| i.lookaheads.as_ref())
                .map_or(0, Vec::len);
            if handed.len() <= at {
                handed.resize(at.saturating_add(1), None);
            }
            if let Some(last) = handed.get_mut(at) {
                if *last == Some(carried) {
                    continue;
                }
                *last = Some(carried);
            }

            let p = grammar.production(production)?;
            let Some(next) = p.rhs.get(dot) else { continue };
            if !grammar.is_nonterminal(next) {
                continue;
            }
            let beta: &[String] = p.rhs.get(dot.saturating_add(1)..).unwrap_or(&[]);

            // An item may hand lookaheads to itself (`B -> • B c`); that is seen next round.
            let Some(offered) = handed_down(grammar, sets, building.items.get(at), beta, work)?
            else {
                continue;
            };

            for prod_index in grammar.productions_for(next) {
                match building.merge(grammar, *prod_index, 0, &offered, work)? {
                    Merge::Added => {
                        changed = true;
                        cause.push(Some((at, next.clone(), beta.to_vec())));
                        contributors.push(vec![at]);
                        check_aligned(&building.items, &cause, &contributors)?;
                    }
                    Merge::Widened(pos) => {
                        changed = true;
                        if let Some(slot) = cause.get_mut(pos) {
                            if slot.is_none() {
                                *slot = Some((at, next.clone(), beta.to_vec()));
                            }
                        }
                        if let Some(list) = contributors.get_mut(pos) {
                            if !list.contains(&at) {
                                list.push(at);
                            }
                        }
                    }
                    Merge::Unchanged => {}
                }
            }
        }
        if !changed {
            settled = true;
            break;
        }
    }
    if !settled {
        return Err(EngineError::new(
            "The item-set closure did not settle. This is a bug in the engine, not in your grammar.",
        ));
    }
    let items = building.items;

    // Each non-kernel item, explained once in final form; causes are read back
    // out of the settled set.
    let mut additions: Vec<ClosureAddition> = Vec::new();
    for pos in kernel_len..items.len() {
        let (Some(item), Some(Some((from, nonterminal, beta)))) =
            (items.get(pos), cause.get(pos))
        else {
            continue;
        };
        let Some(because_of) = items.get(*from) else { continue };
        work.show(weight(std::slice::from_ref(item)))?;
        work.show(weight(std::slice::from_ref(because_of)))?;
        let others = contributors.get(pos).map_or(0, |c| c.len().saturating_sub(1));
        additions.push(ClosureAddition {
            item: item.clone(),
            because_of: because_of.clone(),
            nonterminal: nonterminal.clone(),
            beta: Some(beta.clone()),
            other_causes: others,
        });
    }
    Ok((items, additions))
}

/// The three vectors must stay aligned; a mismatch is an error, not a panic.
fn check_aligned(
    items: &[Item],
    cause: &[Option<(usize, String, Vec<String>)>],
    contributors: &[Vec<usize>],
) -> Result<()> {
    if items.len() == cause.len() && items.len() == contributors.len() {
        return Ok(());
    }
    Err(out_of_step())
}

fn out_of_step() -> EngineError {
    EngineError::new("internal error: closure bookkeeping fell out of step")
}

// ---------------------------------------------------------------------------

/// The kernel of `goto(I, X)`: every item of `I` with the dot before `X`, dot
/// advanced, in source order. Lookaheads ride along untouched.
pub fn goto_kernel(grammar: &Grammar, items: &[Item], symbol: &str) -> Result<Vec<Item>> {
    let mut out: Vec<Item> = Vec::new();
    for item in items {
        if next_symbol(grammar, item)? == Some(symbol) {
            let advanced = item.advanced();
            if !out.contains(&advanced) {
                out.push(advanced);
            }
        }
    }
    Ok(out)
}

/// A canonical, order-independent key for comparing two item sets. It compares
/// lookaheads as well as cores, which is why the LR(1) collection is larger.
pub fn signature(items: &[Item]) -> Vec<Item> {
    let mut sorted = items.to_vec();
    sorted.sort();
    sorted.dedup();
    sorted
}

/// The same key with lookaheads stripped: states share a core when their LR(0)
/// items match. Used to explain CLR(1) splits and to decide LALR(1) merges.
pub fn core_signature(items: &[Item]) -> Vec<Item> {
    let mut sorted: Vec<Item> = items
        .iter()
        .map(|i| Item { production: i.production, dot: i.dot, lookaheads: None })
        .collect();
    sorted.sort();
    sorted.dedup();
    sorted
}

/// `A -> α` for use inside a sentence, with ε shown for an empty right-hand side.
pub fn production_display(grammar: &Grammar, index: usize) -> Result<String> {
    let p = grammar.production(index)?;
    Ok(if p.rhs.is_empty() {
        format!("{} → {}", p.lhs, EPSILON)
    } else {
        format!("{} → {}", p.lhs, p.rhs.join(" "))
    })
}

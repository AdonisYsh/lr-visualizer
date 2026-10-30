//! FIRST and FOLLOW, computed by fixpoint and recorded one addition at a time.
//!
//! Only SLR(1) consults FOLLOW to place reductions; LR(1) methods use FIRST.

use crate::error::Result;
use crate::grammar::{Grammar, Production, END, EPSILON};
use crate::limits::{self, Stage};
use crate::trace::{Recorder, Step};
use std::collections::BTreeMap;

/// FIRST and FOLLOW for every non-terminal, plus the derived nullable set.
pub struct Sets {
    pub first: BTreeMap<String, Vec<String>>,
    pub follow: BTreeMap<String, Vec<String>>,
}

impl Sets {
    pub fn first_of(&self, nt: &str) -> &[String] {
        self.first.get(nt).map_or(&[], Vec::as_slice)
    }

    pub fn follow_of(&self, nt: &str) -> &[String] {
        self.follow.get(nt).map_or(&[], Vec::as_slice)
    }

    pub fn is_nullable(&self, nt: &str) -> bool {
        self.first_of(nt).iter().any(|s| s == EPSILON)
    }

    /// FIRST of a symbol sequence, and whether the whole sequence is nullable.
    /// ε is never in the returned set; the flag carries it.
    pub fn first_of_sequence(&self, grammar: &Grammar, seq: &[String]) -> (Vec<String>, bool) {
        let (out, nullable, _) = self.first_of_sequence_costed(grammar, seq);
        (out, nullable)
    }

    /// [`Sets::first_of_sequence`], plus its cost in work units for the meter.
    pub fn first_of_sequence_costed(
        &self,
        grammar: &Grammar,
        seq: &[String],
    ) -> (Vec<String>, bool, usize) {
        let mut out: Vec<String> = Vec::new();
        let mut cost: usize = 0;
        for sym in seq {
            cost = cost.saturating_add(1);
            if grammar.is_terminal(sym) || sym == END {
                cost = cost.saturating_add(out.len());
                grammar.insert_sorted(&mut out, sym);
                return (out, false, cost);
            }
            for s in self.first_of(sym) {
                if s != EPSILON {
                    cost = cost.saturating_add(out.len().saturating_add(1));
                    grammar.insert_sorted(&mut out, s);
                }
            }
            if !self.is_nullable(sym) {
                return (out, false, cost);
            }
        }
        (out, true, cost)
    }
}

fn table_of(grammar: &Grammar, sets: &BTreeMap<String, Vec<String>>) -> Vec<(String, Vec<String>)> {
    grammar
        .nonterminals
        .iter()
        .map(|nt| (nt.clone(), sets.get(nt).cloned().unwrap_or_default()))
        .collect()
}

/// Read `E -> E + T` for use inside an explanation.
fn cite(p: &Production) -> String {
    format!("production {} ({})", p.index, p.display())
}

// ---------------------------------------------------------------------------

/// Compute FIRST then FOLLOW, emitting a step for every symbol added.
pub fn compute(grammar: &Grammar, rec: &mut Recorder) -> Result<Sets> {
    rec.work().enter(Stage::Sets);
    let first = compute_first(grammar, rec)?;
    let mut sets = Sets { first, follow: BTreeMap::new() };
    sets.follow = compute_follow(grammar, &sets, rec)?;
    Ok(sets)
}

// --- FIRST -----------------------------------------------------------------

fn compute_first(grammar: &Grammar, rec: &mut Recorder) -> Result<BTreeMap<String, Vec<String>>> {
    let mut first: BTreeMap<String, Vec<String>> =
        grammar.nonterminals.iter().map(|n| (n.clone(), Vec::new())).collect();

    for _ in 0..limits::MAX_FIXPOINT_ROUNDS {
        let mut changed = false;
        for p in &grammar.productions {
            rec.work().spend(1)?;
            // An empty right-hand side puts ε in FIRST directly.
            if p.rhs.is_empty() {
                if add_first(grammar, &mut first, &p.lhs, EPSILON) {
                    changed = true;
                    rec.push(Step::FirstAdd {
                        nonterminal: p.lhs.clone(),
                        symbol: EPSILON.to_string(),
                        from_production: p.index,
                        explain: format!(
                            "FIRST({}) gains ε because {} has an empty right-hand side, so {} can derive the empty string and vanish.",
                            p.lhs, cite(p), p.lhs
                        ),
                    })?;
                }
                continue;
            }

            // Walk the right-hand side until a symbol that cannot vanish.
            let mut all_nullable = true;
            for (pos, sym) in p.rhs.iter().enumerate() {
                // Each candidate is searched for in FIRST(lhs) before adding.
                let held = first.get(&p.lhs).map_or(0, Vec::len).saturating_add(1);
                if grammar.is_terminal(sym) {
                    rec.work().spend(held)?;
                    if add_first(grammar, &mut first, &p.lhs, sym) {
                        changed = true;
                        rec.push(Step::FirstAdd {
                            nonterminal: p.lhs.clone(),
                            symbol: sym.clone(),
                            from_production: p.index,
                            explain: first_terminal_explain(p, pos, sym),
                        })?;
                    }
                    all_nullable = false;
                    break;
                }

                let source: Vec<String> = first.get(sym).cloned().unwrap_or_default();
                rec.work().spend(source.len().saturating_mul(held).saturating_add(1))?;
                for s in source.iter().filter(|s| *s != EPSILON) {
                    if add_first(grammar, &mut first, &p.lhs, s) {
                        changed = true;
                        rec.push(Step::FirstAdd {
                            nonterminal: p.lhs.clone(),
                            symbol: s.clone(),
                            from_production: p.index,
                            explain: first_via_nonterminal_explain(p, pos, sym, s),
                        })?;
                    }
                }
                if !source.iter().any(|s| s == EPSILON) {
                    all_nullable = false;
                    break;
                }
            }

            if all_nullable && add_first(grammar, &mut first, &p.lhs, EPSILON) {
                changed = true;
                rec.push(Step::FirstAdd {
                    nonterminal: p.lhs.clone(),
                    symbol: EPSILON.to_string(),
                    from_production: p.index,
                    explain: format!(
                        "FIRST({}) gains ε because every symbol on the right of {} can itself vanish, so the whole right-hand side can derive the empty string.",
                        p.lhs, cite(p)
                    ),
                })?;
            }
        }
        if !changed {
            break;
        }
    }

    rec.push(Step::FirstDone {
        table: table_of(grammar, &first),
        explain: format!(
            "FIRST is complete. FIRST(X) is the set of terminals that can begin a string derived from X; ε in the set means X can also derive nothing at all. {} non-terminals were processed.",
            grammar.nonterminals.len()
        ),
    })?;
    Ok(first)
}

fn add_first(
    grammar: &Grammar,
    first: &mut BTreeMap<String, Vec<String>>,
    nt: &str,
    symbol: &str,
) -> bool {
    match first.get_mut(nt) {
        Some(set) => grammar.insert_sorted(set, symbol),
        None => false,
    }
}

fn first_terminal_explain(p: &Production, pos: usize, sym: &str) -> String {
    if pos == 0 {
        format!(
            "FIRST({}) gains `{}` because {} starts with the terminal `{}`, so any string derived from {} using this production begins with it.",
            p.lhs, sym, cite(p), sym, p.lhs
        )
    } else {
        let prefix = p.rhs.get(..pos).unwrap_or(&[]).join(" ");
        format!(
            "FIRST({}) gains `{}` because in {} everything before the terminal `{}` ({}) can vanish, leaving `{}` first.",
            p.lhs, sym, cite(p), sym, prefix, sym
        )
    }
}

fn first_via_nonterminal_explain(p: &Production, pos: usize, via: &str, sym: &str) -> String {
    if pos == 0 {
        format!(
            "FIRST({}) gains `{}` because {} begins with {}, and `{}` is in FIRST({}). Whatever can start {} can start {}.",
            p.lhs, sym, cite(p), via, sym, via, via, p.lhs
        )
    } else {
        let prefix = p.rhs.get(..pos).unwrap_or(&[]).join(" ");
        format!(
            "FIRST({}) gains `{}` because in {} everything before {} ({}) can vanish, and `{}` is in FIRST({}).",
            p.lhs, sym, cite(p), via, prefix, sym, via
        )
    }
}

// --- FOLLOW ----------------------------------------------------------------

fn compute_follow(
    grammar: &Grammar,
    sets: &Sets,
    rec: &mut Recorder,
) -> Result<BTreeMap<String, Vec<String>>> {
    let mut follow: BTreeMap<String, Vec<String>> =
        grammar.nonterminals.iter().map(|n| (n.clone(), Vec::new())).collect();

    // Rule 1: the end of input follows the augmented start symbol.
    if let Some(set) = follow.get_mut(&grammar.start) {
        grammar.insert_sorted(set, END);
    }
    rec.push(Step::FollowAdd {
        nonterminal: grammar.start.clone(),
        symbol: END.to_string(),
        from_production: 0,
        rule_used: "start".to_string(),
        explain: format!(
            "FOLLOW({}) gains $ because {} is the start symbol of the augmented grammar: when it has been recognised, the only thing that can come next is the end of the input.",
            grammar.start, grammar.start
        ),
    })?;

    for _ in 0..limits::MAX_FIXPOINT_ROUNDS {
        let mut changed = false;
        for p in &grammar.productions {
            rec.work().spend(p.rhs.len().saturating_add(1))?;
            for (i, b) in p.rhs.iter().enumerate() {
                if !grammar.is_nonterminal(b) {
                    continue;
                }
                let beta: Vec<String> = p.rhs.get(i + 1..).unwrap_or(&[]).to_vec();
                let (first_beta, beta_nullable, cost) =
                    sets.first_of_sequence_costed(grammar, &beta);
                // Charge for recomputing FIRST(β) and searching FOLLOW(B).
                let held = follow.get(b).map_or(0, Vec::len).saturating_add(1);
                rec.work().spend(
                    beta.len()
                        .saturating_add(cost)
                        .saturating_add(first_beta.len().saturating_mul(held))
                        .saturating_add(1),
                )?;

                // Rule 2: anything that can start what comes after B can follow B.
                for s in &first_beta {
                    if add_follow(grammar, &mut follow, b, s) {
                        changed = true;
                        rec.push(Step::FollowAdd {
                            nonterminal: b.clone(),
                            symbol: s.clone(),
                            from_production: p.index,
                            rule_used: "beta-first".to_string(),
                            explain: follow_first_explain(grammar, p, b, s, &beta),
                        })?;
                    }
                }

                // Rule 3: if what comes after B can vanish (or there is nothing
                // after it), anything that can follow A can follow B.
                if beta_nullable {
                    let source: Vec<String> = follow.get(&p.lhs).cloned().unwrap_or_default();
                    rec.work().spend(source.len().saturating_mul(held.saturating_add(1)))?;
                    let rule = if beta.is_empty() { "tail" } else { "beta-nullable" };
                    for s in &source {
                        if add_follow(grammar, &mut follow, b, s) {
                            changed = true;
                            rec.push(Step::FollowAdd {
                                nonterminal: b.clone(),
                                symbol: s.clone(),
                                from_production: p.index,
                                rule_used: rule.to_string(),
                                explain: follow_tail_explain(p, b, s, &beta),
                            })?;
                        }
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }

    rec.push(Step::FollowDone {
        table: table_of(grammar, &follow),
        explain: "FOLLOW is complete. FOLLOW(A) is the set of terminals that can appear immediately after A in some derivation from the start symbol - anywhere in the grammar, in any context. SLR(1) is the one method built on these sets: a reduction to A is allowed exactly in the columns of FOLLOW(A). LR(0) ignores them entirely, which is why it reduces in every column instead. CLR(1) and LALR(1) ignore them too, but for the opposite reason: they work out a lookahead set for each item in each state separately, and FOLLOW(A) is the union of all of those at once - too coarse to be worth consulting.".to_string(),
    })?;
    Ok(follow)
}

fn add_follow(
    grammar: &Grammar,
    follow: &mut BTreeMap<String, Vec<String>>,
    nt: &str,
    symbol: &str,
) -> bool {
    match follow.get_mut(nt) {
        Some(set) => grammar.insert_sorted(set, symbol),
        None => false,
    }
}

fn follow_tail_explain(p: &Production, b: &str, s: &str, beta: &[String]) -> String {
    let sym = if s == END { "$".to_string() } else { format!("`{s}`") };
    if beta.is_empty() {
        format!(
            "FOLLOW({}) gains {} because in {}, {} sits at the very end. Once {} is complete so is {}, so anything that can follow {} can follow {}.",
            b, sym, cite(p), b, b, p.lhs, p.lhs, b
        )
    } else {
        format!(
            "FOLLOW({}) gains {} because in {}, everything after {} ({}) can vanish, so {} can end the production and anything that can follow {} can follow {}.",
            b,
            sym,
            cite(p),
            b,
            beta.join(" "),
            b,
            p.lhs,
            b
        )
    }
}

/// Rule 2's explanation; the common case of `B` right before a terminal is said plainly.
fn follow_first_explain(
    grammar: &Grammar,
    p: &Production,
    b: &str,
    s: &str,
    beta: &[String],
) -> String {
    let immediate = beta.first().is_some_and(|f| f == s && grammar.is_terminal(f));
    if immediate {
        return format!(
            "FOLLOW({b}) gains `{s}` because in {}, {b} is immediately followed by the terminal `{s}`, so `{s}` can come straight after {b}.",
            cite(p)
        );
    }
    format!(
        "FOLLOW({b}) gains `{s}` because in {}, {b} is followed by `{}`, and `{s}` is one of the terminals that can begin `{}`.",
        cite(p),
        beta.join(" "),
        beta.join(" ")
    )
}

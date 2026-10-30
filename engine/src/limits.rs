//! Every cap in the engine lives here.
//! Hitting one returns a clean error that names the limit.

use crate::error::{EngineError, Result};

/// Maximum length of the raw grammar text, in characters.
pub const MAX_SOURCE_CHARS: usize = 20_000;

/// Maximum number of productions after expanding `|` alternatives.
pub const MAX_PRODUCTIONS: usize = 200;

/// Maximum number of symbols on one right-hand side. Several passes compare
/// every pair of positions, so a single huge production would be quadratic.
pub const MAX_RHS_LEN: usize = 50;

/// Maximum number of distinct grammar symbols (terminals + non-terminals).
pub const MAX_SYMBOLS: usize = 200;

/// Maximum length of a single symbol name, in characters.
pub const MAX_SYMBOL_LEN: usize = 32;

/// Maximum number of warnings returned; the rest collapse into one
/// "+N more" line.
pub const MAX_WARNINGS: usize = 50;

/// Maximum number of states in the canonical collection.
pub const MAX_STATES: usize = 500;

/// Maximum number of steps in the whole trace.
pub const MAX_STEPS: usize = 20_000;

/// Maximum number of symbols in the test string.
pub const MAX_TEST_SYMBOLS: usize = 500;

/// Maximum steps in the parse phase: four per test-string symbol plus 100
/// spare. Separate from [`MAX_STEPS`] so a looping conflicted parse is
/// diagnosed clearly.
pub const MAX_PARSE_STEPS: usize = 4 * MAX_TEST_SYMBOLS + 100;

/// Maximum depth of the parse stack: twice the longest test string. Stops a
/// conflicted table that loops on empty reductions long before
/// [`MAX_PARSE_STEPS`].
pub const MAX_PARSE_STACK: usize = 2 * MAX_TEST_SYMBOLS;

/// Safety valve on fixpoint loops (FIRST/FOLLOW, closure, productivity).
/// They converge in far fewer rounds; hitting this means a bug.
pub const MAX_FIXPOINT_ROUNDS: usize = 10_000;

/// Maximum work one analysis may spend on FIRST/FOLLOW and its own item sets,
/// counted by [`Work`]. A unit is one item or lookahead handled; it counts
/// operations, not time, so the same grammar is refused on every machine.
/// About 10x what the largest benchmark grammar needs.
pub const MAX_WORK: u64 = 120_000_000;

/// Work cap for collections built only to fill a comparison row: smaller, so
/// an LR(0) run is not kept waiting by them. An over-budget row is omitted.
pub const MAX_COMPARISON_WORK: u64 = MAX_WORK / 2;

/// Work units charged for an item or lookahead that is shown in a step
/// rather than merely handled (copying and serializing it costs far more).
pub const SHOWN: usize = 20;

// ---------------------------------------------------------------------------
// The work meter
// ---------------------------------------------------------------------------

/// The meter behind [`MAX_WORK`]: loops a grammar controls charge it for each
/// item and lookahead they handle.
#[derive(Debug, Clone)]
pub struct Work {
    spent: u64,
    limit: u64,
    stage: Stage,
}

/// What the meter is charging for, so an overrun can say what ran out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// FIRST and FOLLOW, shared by all four methods.
    Sets,
    /// Closure, GOTO and merging by core: the collection of item sets.
    ItemSets,
}

impl Work {
    /// A meter that allows `limit` units.
    pub fn new(limit: u64) -> Self {
        Work { spent: 0, limit, stage: Stage::ItemSets }
    }

    /// Switch stage; the total carries over (one budget per run).
    pub fn enter(&mut self, stage: Stage) {
        self.stage = stage;
    }

    /// Units charged so far.
    pub fn spent(&self) -> u64 {
        self.spent
    }

    /// Charge for `weight` items and lookaheads put into a step, at [`SHOWN`] units each.
    pub fn show(&mut self, weight: usize) -> Result<()> {
        self.spend(weight.saturating_mul(SHOWN))
    }

    /// Charge `units`. Fails, naming the limit, once the total passes it.
    pub fn spend(&mut self, units: usize) -> Result<()> {
        let units = u64::try_from(units).unwrap_or(u64::MAX);
        self.spent = self.spent.saturating_add(units);
        if self.spent > self.limit {
            return Err(match self.stage {
                Stage::Sets => EngineError::new(format!(
                    "Computing FIRST and FOLLOW for this grammar takes more than {} units of work, over the work limit.",
                    self.limit
                ))
                .hint("Nothing is wrong with the way the grammar is written - it is large in a way that multiplies. A unit of work is one symbol or one member of a set handled, and the count grows fastest when long right-hand sides are made of non-terminals that can all vanish and that have large FIRST sets: every position then needs the FIRST of everything after it. Try shorter productions, or fewer of them. Every method computes these sets, so choosing another one will not help."),
                Stage::ItemSets => EngineError::new(format!(
                    "Building the item sets for this grammar takes more than {} units of work, over the work limit.",
                    self.limit
                ))
                .hint("Nothing is wrong with the way the grammar is written - it is large in a way that multiplies. A unit of work is one item or one lookahead handled, and the count grows fastest when one non-terminal has many productions and every item carries many lookaheads, which is what CLR(1) and LALR(1) build. Try fewer alternatives, or LR(0) or SLR(1), whose items carry no lookaheads."),
            });
        }
        Ok(())
    }
}

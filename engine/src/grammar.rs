//! Grammar text -> validated, augmented [`Grammar`].
//!
//! Tokenization mode is detected once for the whole source ([`detect_mode`]).
//! A symbol is a non-terminal iff some production defines it, never by case.

use crate::error::{EngineError, Result, Warning};
use crate::limits;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// The end-of-input marker. Reserved: a grammar may not contain it.
pub const END: &str = "$";

/// How the epsilon (empty string) right-hand side may be written.
const EPSILON_ALIASES: [&str; 5] = ["ε", "#", "eps", "epsilon", "ϵ"];

/// The symbol shown for epsilon in FIRST sets and elsewhere.
pub const EPSILON: &str = "ε";

/// U+FEFF, which some editors write at the very start of a UTF-8 file.
const BYTE_ORDER_MARK: char = '\u{FEFF}';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// Every non-space character of a right-hand side is its own symbol.
    SingleChar,
    /// Right-hand sides are split on whitespace.
    SpaceSeparated,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::SingleChar => "single-char",
            Mode::SpaceSeparated => "space-separated",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Production {
    pub index: usize,
    pub lhs: String,
    pub rhs: Vec<String>,
    /// 1-based source line, or `None` for the augmented production.
    pub line: Option<usize>,
}

impl Production {
    /// `E -> E + T`, or `A -> ε` for an empty right-hand side.
    pub fn display(&self) -> String {
        if self.rhs.is_empty() {
            format!("{} → {}", self.lhs, EPSILON)
        } else {
            format!("{} → {}", self.lhs, self.rhs.join(" "))
        }
    }
}

#[derive(Debug, Clone)]
pub struct Grammar {
    /// Production 0 is always the augmented `S' -> S`.
    pub productions: Vec<Production>,
    pub terminals: Vec<String>,
    pub nonterminals: Vec<String>,
    /// The augmented start symbol, `S'`.
    pub start: String,
    /// The user's start symbol, `S`.
    pub original_start: String,
    pub mode: Mode,
    /// Every symbol in order of first appearance; GOTO iterates in this order.
    pub symbol_order: Vec<String>,
    rank: HashMap<String, usize>,
    productions_of: HashMap<String, Vec<usize>>,
}

impl Grammar {
    pub fn production(&self, index: usize) -> Result<&Production> {
        self.productions
            .get(index)
            .ok_or_else(|| EngineError::new(format!("internal error: no production {index}")))
    }

    /// Indices of every production with `nt` on the left, in source order.
    pub fn productions_for(&self, nt: &str) -> &[usize] {
        self.productions_of.get(nt).map_or(&[], Vec::as_slice)
    }

    pub fn is_nonterminal(&self, symbol: &str) -> bool {
        self.productions_of.contains_key(symbol)
    }

    /// True for any symbol this grammar does not define, which is the right
    /// question when classifying a symbol *taken from the grammar*.
    pub fn is_terminal(&self, symbol: &str) -> bool {
        !self.is_nonterminal(symbol) && symbol != END
    }

    /// True only for terminals this grammar contains; use for outside input.
    pub fn is_known_terminal(&self, symbol: &str) -> bool {
        self.terminals.iter().any(|t| t == symbol)
    }

    /// Sort key giving symbols their first-appearance order, with `$` and `ε`
    /// pushed to the end so they always render last in a set.
    pub fn rank(&self, symbol: &str) -> usize {
        match symbol {
            END => usize::MAX - 1,
            EPSILON => usize::MAX,
            _ => self.rank.get(symbol).copied().unwrap_or(usize::MAX - 2),
        }
    }

    /// ACTION columns: every terminal in appearance order, then `$`.
    pub fn action_columns(&self) -> Vec<String> {
        let mut cols = self.terminals.clone();
        cols.push(END.to_string());
        cols
    }

    /// Insert into a set kept in canonical order. Returns false if present.
    pub fn insert_sorted(&self, set: &mut Vec<String>, symbol: &str) -> bool {
        if set.iter().any(|s| s == symbol) {
            return false;
        }
        let rank = self.rank(symbol);
        let pos = set
            .iter()
            .position(|s| self.rank(s) > rank)
            .unwrap_or(set.len());
        set.insert(pos, symbol.to_string());
        true
    }

    /// True when `set` is already canonical, as [`Grammar::insert_sorted`] builds it.
    pub fn is_canonical(&self, set: &[String]) -> bool {
        let mut last: Option<usize> = None;
        for symbol in set {
            let rank = self.rank(symbol);
            if last.is_some_and(|previous| previous >= rank) {
                return false;
            }
            last = Some(rank);
        }
        true
    }
}

/// Everything [`parse`] produces: the grammar plus the remarks about it.
pub struct ParsedGrammar {
    pub grammar: Grammar,
    pub warnings: Vec<Warning>,
    /// Non-terminals not reachable from the start symbol.
    pub unreachable: Vec<String>,
}

// ---------------------------------------------------------------------------
// Line splitting
// ---------------------------------------------------------------------------

struct RawRule {
    line: usize,
    lhs: String,
    /// Right-hand side alternatives, still as raw text.
    alternatives: Vec<String>,
}

fn find_arrow(line: &str) -> Option<(usize, usize)> {
    ["::=", "→", "->"]
        .iter()
        .filter_map(|a| line.find(a).map(|i| (i, a.len())))
        .min_by_key(|(i, _)| *i)
}

fn split_lines(text: &str) -> Result<Vec<RawRule>> {
    let mut rules = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        let trimmed = raw.trim();
        // Only `//` introduces a comment: `#` cannot, because it is an
        // accepted spelling of epsilon.
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }
        let Some((at, len)) = find_arrow(trimmed) else {
            return Err(EngineError::at(
                line,
                format!("No arrow on this line: `{trimmed}`"),
            )
            .hint("Every production needs an arrow. Write `A -> B c`, `A → B c` or `A ::= B c`."));
        };
        let lhs = trimmed.get(..at).unwrap_or("").trim().to_string();
        let rhs = trimmed.get(at + len..).unwrap_or("").trim();
        if lhs.is_empty() {
            return Err(EngineError::at(line, "Nothing on the left of the arrow.")
                .hint("A production looks like `A -> B c`. The left side names the non-terminal being defined."));
        }
        let alternatives: Vec<String> = rhs.split('|').map(|a| a.trim().to_string()).collect();
        rules.push(RawRule { line, lhs, alternatives });
    }
    if rules.is_empty() {
        return Err(EngineError::new("The grammar is empty.")
            .hint("Type at least one production, for example `S -> a S b | c`."));
    }
    Ok(rules)
}

// ---------------------------------------------------------------------------
// Mode detection
// ---------------------------------------------------------------------------

fn is_epsilon(token: &str) -> bool {
    EPSILON_ALIASES.iter().any(|e| e.eq_ignore_ascii_case(token))
}

/// Pick one tokenization mode for the whole grammar: any token over one character
/// (left-hand sides included, epsilon spellings excluded) means space-separated.
fn detect_mode(rules: &[RawRule]) -> Mode {
    for rule in rules {
        if rule.lhs.chars().count() > 1 && !is_epsilon(&rule.lhs) {
            return Mode::SpaceSeparated;
        }
        for alt in &rule.alternatives {
            for token in alt.split_whitespace() {
                if token.chars().count() > 1 && !is_epsilon(token) {
                    return Mode::SpaceSeparated;
                }
            }
        }
    }
    Mode::SingleChar
}

fn tokenize_alt(alt: &str, mode: Mode) -> Vec<String> {
    if alt.trim().is_empty() || is_epsilon(alt.trim()) {
        return Vec::new();
    }
    match mode {
        Mode::SpaceSeparated => alt
            .split_whitespace()
            .filter(|t| !is_epsilon(t))
            .map(str::to_string)
            .collect(),
        Mode::SingleChar => alt
            .chars()
            .filter(|c| !c.is_whitespace())
            .map(|c| c.to_string())
            .collect(),
    }
}

/// Tokenize a test string exactly the way the grammar was tokenized.
pub fn tokenize_input(text: &str, mode: Mode) -> Vec<String> {
    match mode {
        Mode::SpaceSeparated => text.split_whitespace().map(str::to_string).collect(),
        Mode::SingleChar => text
            .chars()
            .filter(|c| !c.is_whitespace())
            .map(|c| c.to_string())
            .collect(),
    }
}

/// How many symbols [`tokenize_input`] would return, without building them, so
/// an over-long test string is refused before it is copied symbol by symbol.
pub fn count_input(text: &str, mode: Mode) -> usize {
    match mode {
        Mode::SpaceSeparated => text.split_whitespace().count(),
        Mode::SingleChar => text.chars().filter(|c| !c.is_whitespace()).count(),
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Parse, validate and augment a grammar. `Err` blocks analysis; warnings do not.
pub fn parse(text: &str) -> Result<ParsedGrammar> {
    // One leading U+FEFF is ignored; otherwise it would rename the start symbol.
    let text = text.strip_prefix(BYTE_ORDER_MARK).unwrap_or(text);

    if text.chars().count() > limits::MAX_SOURCE_CHARS {
        return Err(EngineError::new(format!(
            "The grammar text is {} characters, over the {} character limit.",
            text.chars().count(),
            limits::MAX_SOURCE_CHARS
        ))
        .hint("This tool is for teaching-sized grammars. Try a smaller excerpt."));
    }

    let rules = split_lines(text)?;
    let mode = detect_mode(&rules);
    let mut warnings = Vec::new();

    // --- left-hand sides -----------------------------------------------------
    // Validated before right-hand sides so "which symbols are non-terminals" is
    // settled before anything else is interpreted.
    let mut nonterminals: Vec<String> = Vec::new();
    for rule in &rules {
        let lhs_tokens = tokenize_alt(&rule.lhs, mode);
        let name = match lhs_tokens.as_slice() {
            [one] => one.clone(),
            [] => {
                return Err(EngineError::at(rule.line, "The left-hand side is empty.")
                    .hint("Name the non-terminal being defined, e.g. `A -> B c`."))
            }
            many => {
                return Err(EngineError::at(
                    rule.line,
                    format!(
                        "The left-hand side `{}` reads as {} symbols ({}), but a production may only define one non-terminal.",
                        rule.lhs.trim(),
                        many.len(),
                        many.join(" ")
                    ),
                )
                .hint(if mode == Mode::SingleChar {
                    "This grammar is being read in single-character mode, where every character is a symbol."
                } else {
                    "In space-separated mode the left-hand side must be a single token with no spaces."
                }))
            }
        };
        check_symbol(&name, rule.line)?;
        if !nonterminals.contains(&name) {
            nonterminals.push(name);
        }
    }

    // --- productions ---------------------------------------------------------
    let mut productions: Vec<Production> = Vec::new();
    let mut seen: HashSet<(String, Vec<String>)> = HashSet::new();
    let mut multi_char_tokens: Vec<(usize, String)> = Vec::new();

    for rule in &rules {
        let lhs = tokenize_alt(&rule.lhs, mode).first().cloned().unwrap_or_default();
        for alt in &rule.alternatives {
            if mode == Mode::SpaceSeparated {
                for token in alt.split_whitespace() {
                    if token.chars().count() > 1 {
                        multi_char_tokens.push((rule.line, token.to_string()));
                    }
                }
            }
            let rhs = tokenize_alt(alt, mode);
            for sym in &rhs {
                check_symbol(sym, rule.line)?;
            }
            if rhs.len() > limits::MAX_RHS_LEN {
                return Err(EngineError::at(
                    rule.line,
                    format!(
                        "A right-hand side of `{lhs}` has {} symbols, over the {} symbol limit for one production.",
                        rhs.len(),
                        limits::MAX_RHS_LEN
                    ),
                )
                .hint("Split it up: give part of the right-hand side a name of its own, as a new non-terminal, and use that name here. If the line was meant to be several alternatives, separate them with `|`."));
            }
            if !seen.insert((lhs.clone(), rhs.clone())) {
                warnings.push(Warning::warn(
                    Some(rule.line),
                    format!(
                        "Duplicate production `{}` was written more than once. The copies have been merged; two identical productions would otherwise show up as a reduce-reduce conflict caused by the typo rather than by the grammar.",
                        Production { index: 0, lhs: lhs.clone(), rhs: rhs.clone(), line: None }.display()
                    ),
                ));
                continue;
            }
            if productions.len() + 1 > limits::MAX_PRODUCTIONS {
                return Err(EngineError::at(
                    rule.line,
                    format!(
                        "This grammar has more than {} productions once `|` alternatives are expanded.",
                        limits::MAX_PRODUCTIONS
                    ),
                )
                .hint("Grammars this large cannot be displayed usefully. Try a smaller one."));
            }
            productions.push(Production {
                index: productions.len() + 1, // +1: production 0 is the augmented rule
                lhs: lhs.clone(),
                rhs,
                line: Some(rule.line),
            });
        }
    }

    let original_start = nonterminals
        .first()
        .cloned()
        .ok_or_else(|| EngineError::new("The grammar defines no non-terminals."))?;

    // --- augmentation --------------------------------------------------------
    // The augmented start is the first primed name no symbol (terminal or not) uses.
    let start = {
        let used_on_a_right: HashSet<&str> = productions
            .iter()
            .flat_map(|p| p.rhs.iter().map(String::as_str))
            .collect();
        let mut start = format!("{original_start}'");
        while nonterminals.contains(&start) || used_on_a_right.contains(start.as_str()) {
            start.push('\'');
        }
        start
    };
    check_symbol(&start, 0)?;
    productions.insert(
        0,
        Production {
            index: 0,
            lhs: start.clone(),
            rhs: vec![original_start.clone()],
            line: None,
        },
    );
    nonterminals.insert(0, start.clone());

    // --- symbol inventory ----------------------------------------------------
    // First-appearance order, walking productions in source order and each
    // production left to right. This is what fixes the GOTO iteration order.
    let mut symbol_order: Vec<String> = Vec::new();
    let push_sym = |order: &mut Vec<String>, s: &str| {
        if !order.iter().any(|x| x == s) {
            order.push(s.to_string());
        }
    };
    for p in &productions {
        push_sym(&mut symbol_order, &p.lhs);
        for s in &p.rhs {
            push_sym(&mut symbol_order, s);
        }
    }

    let nt_set: HashSet<&String> = nonterminals.iter().collect();
    let terminals: Vec<String> = symbol_order
        .iter()
        .filter(|s| !nt_set.contains(*s))
        .cloned()
        .collect();

    if symbol_order.len() > limits::MAX_SYMBOLS {
        return Err(EngineError::new(format!(
            "This grammar uses {} distinct symbols, over the {} symbol limit.",
            symbol_order.len(),
            limits::MAX_SYMBOLS
        ))
        .hint("Check the tokenization mode - a grammar read one character at a time can produce far more symbols than intended."));
    }

    let rank: HashMap<String, usize> = symbol_order
        .iter()
        .enumerate()
        .map(|(i, s)| (s.clone(), i))
        .collect();
    let mut productions_of: HashMap<String, Vec<usize>> = HashMap::new();
    for nt in &nonterminals {
        productions_of.insert(nt.clone(), Vec::new());
    }
    for p in &productions {
        productions_of.entry(p.lhs.clone()).or_default().push(p.index);
    }

    let grammar = Grammar {
        productions,
        terminals,
        nonterminals,
        start,
        original_start,
        mode,
        symbol_order,
        rank,
        productions_of,
    };

    // --- sanity checks -------------------------------------------------------
    glued_token_warnings(&grammar, &multi_char_tokens, &mut warnings);
    undefined_lookalike_warnings(&grammar, &mut warnings);
    // Order matters: a pure cycle such as `A -> B` / `B -> A` is also
    // non-productive, and "cycle" is the more specific diagnosis of the two.
    check_cycles(&grammar)?;
    check_productive(&grammar)?;
    let unreachable = unreachable_nonterminals(&grammar);
    for nt in &unreachable {
        warnings.push(Warning::warn(
            None,
            format!("`{nt}` cannot be reached from the start symbol `{}`, so it plays no part in the analysis.", grammar.original_start),
        ));
    }
    if let Some(cycle) = left_recursion(&grammar) {
        warnings.push(Warning::note(format!(
            "This grammar is left-recursive: {cycle} - a non-terminal can derive a string beginning with itself. That is fine for LR parsers. Unlike LL parsers, which have to have left recursion removed before they can work at all, LR parsers handle it naturally, so no rewriting is needed."
        )));
    }
    cap_warnings(&mut warnings);

    Ok(ParsedGrammar { grammar, warnings, unreachable })
}

fn check_symbol(name: &str, line: usize) -> Result<()> {
    if name == END {
        return Err(EngineError::at(
            line,
            "`$` is reserved as the end-of-input marker and may not appear in a grammar.",
        )
        .hint("Rename the symbol. The parser appends `$` to the input on its own."));
    }
    if name.chars().count() > limits::MAX_SYMBOL_LEN {
        return Err(EngineError::at(
            line,
            format!(
                "The symbol `{name}` is {} characters long, over the {} character limit.",
                name.chars().count(),
                limits::MAX_SYMBOL_LEN
            ),
        )
        .hint("Shorten the name. If it was meant to be several symbols, put spaces between them: a grammar with any symbol longer than one character is split on whitespace, so everything between two spaces is read as a single name."));
    }
    Ok(())
}

/// Keep the first [`limits::MAX_WARNINGS`] remarks, then one line counting the rest.
fn cap_warnings(warnings: &mut Vec<Warning>) {
    let extra = warnings.len().saturating_sub(limits::MAX_WARNINGS);
    if extra == 0 {
        return;
    }
    warnings.truncate(limits::MAX_WARNINGS);
    warnings.push(Warning::warn(
        None,
        format!(
            "+{extra} more warning{} not shown. Only the first {} are listed; deal with those and analyse again to see the rest.",
            if extra == 1 { "" } else { "s" },
            limits::MAX_WARNINGS
        ),
    ));
}

// ---------------------------------------------------------------------------
// Sanity checks
// ---------------------------------------------------------------------------

/// Warn when space-separated mode reads `(E)` as one terminal, a grammar the user never wrote.
fn glued_token_warnings(
    grammar: &Grammar,
    multi_char: &[(usize, String)],
    warnings: &mut Vec<Warning>,
) {
    let standalone: HashSet<char> = grammar
        .symbol_order
        .iter()
        .filter(|s| s.chars().count() == 1)
        .filter_map(|s| s.chars().next())
        .collect();
    let mut reported: HashSet<String> = HashSet::new();

    for (line, token) in multi_char {
        if reported.contains(token) {
            continue;
        }
        let chars: Vec<char> = token.chars().collect();
        let contains_standalone = chars.iter().any(|c| standalone.contains(c));
        let has_letter = chars.iter().any(|c| c.is_alphanumeric());
        let has_punct = chars
            .iter()
            .any(|c| "()[]{}+-*/=<>,;.:!?&%^~".contains(*c));
        if !contains_standalone && !(has_letter && has_punct) {
            continue;
        }
        reported.insert(token.clone());
        let spaced: Vec<String> = chars.iter().map(|c| c.to_string()).collect();
        warnings.push(Warning::warn(
            Some(*line),
            format!(
                "In space-separated mode, `{token}` was read as one symbol. Did you mean `{}`?",
                spaced.join(" ")
            ),
        ));
    }
}

/// Advisory only: warn when a never-defined (hence terminal) symbol looks like a non-terminal.
fn undefined_lookalike_warnings(grammar: &Grammar, warnings: &mut Vec<Warning>) {
    // Deliberately narrow. A case-insensitive "did you mean" would fire on
    // `S -> C C` / `C -> c C | d`, where `c` and `C` are both intended.
    let single_upper_nt = grammar
        .nonterminals
        .iter()
        .any(|n| n.chars().count() == 1 && n.chars().all(char::is_uppercase));

    for t in &grammar.terminals {
        if single_upper_nt && t.chars().count() == 1 && t.chars().all(char::is_uppercase) {
            warnings.push(Warning::warn(
                None,
                format!(
                    "`{t}` is used on a right-hand side but never defined by a production, so it is being treated as a terminal. If it was meant to be a non-terminal, add a production for it."
                ),
            ));
        }
    }
}

/// Nullable non-terminals, by fixpoint. Used by the cycle and left-recursion
/// checks and re-derived (with steps) in `sets.rs`.
fn nullable_set(grammar: &Grammar) -> HashSet<String> {
    let mut nullable: HashSet<String> = HashSet::new();
    for _ in 0..limits::MAX_FIXPOINT_ROUNDS {
        let mut changed = false;
        for p in &grammar.productions {
            if nullable.contains(&p.lhs) {
                continue;
            }
            if p.rhs.iter().all(|s| nullable.contains(s)) {
                nullable.insert(p.lhs.clone());
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    nullable
}

/// A non-terminal that derives no terminal string makes the automaton
/// meaningless, so this is an error rather than a warning.
fn check_productive(grammar: &Grammar) -> Result<()> {
    let mut productive: HashSet<String> = HashSet::new();
    for _ in 0..limits::MAX_FIXPOINT_ROUNDS {
        let mut changed = false;
        for p in &grammar.productions {
            if productive.contains(&p.lhs) {
                continue;
            }
            let ok = p
                .rhs
                .iter()
                .all(|s| grammar.is_terminal(s) || productive.contains(s));
            if ok {
                productive.insert(p.lhs.clone());
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    // The augmented start is skipped: it can only ever be unproductive as a
    // consequence of the user's own start symbol being so, and naming it first
    // would point the user at a production they did not write.
    let dead: Vec<&String> = grammar
        .nonterminals
        .iter()
        .filter(|n| **n != grammar.start && !productive.contains(*n))
        .collect();
    if let Some(first) = dead.first() {
        let line = grammar
            .productions_for(first)
            .first()
            .and_then(|i| grammar.productions.get(*i))
            .and_then(|p| p.line);
        let names: Vec<&str> = dead.iter().map(|s| s.as_str()).collect();
        return Err(EngineError {
            line,
            message: format!(
                "`{}` can never derive a string of terminals, so it matches nothing.{}",
                first,
                if names.len() > 1 {
                    format!(" (Also: {}.)", names.get(1..).unwrap_or(&[]).join(", "))
                } else {
                    String::new()
                }
            ),
            hint: Some(
                "Every non-terminal needs at least one production whose right-hand side can eventually become terminals. A rule like `A -> A a` with no other alternative loops forever.".into(),
            ),
        });
    }
    Ok(())
}

/// Build the graph `A -> B` for every production `A -> α B β` where α and β can
/// both vanish, i.e. every case where `A` can derive exactly `B`.
fn unit_graph(grammar: &Grammar) -> BTreeMap<String, BTreeSet<String>> {
    let nullable = nullable_set(grammar);
    let mut graph: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for p in &grammar.productions {
        for (i, sym) in p.rhs.iter().enumerate() {
            if !grammar.is_nonterminal(sym) {
                continue;
            }
            let rest_nullable = p
                .rhs
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .all(|(_, s)| nullable.contains(s));
            if rest_nullable {
                graph.entry(p.lhs.clone()).or_default().insert(sym.clone());
            }
        }
    }
    graph
}

fn find_cycle(graph: &BTreeMap<String, BTreeSet<String>>, roots: &[String]) -> Option<Vec<String>> {
    // Three-colour DFS: 1 = on the current path, 2 = finished. Iterative with
    // an explicit stack so a deep grammar cannot blow the native stack.
    let mut colour: HashMap<String, u8> = HashMap::new();
    for root in roots {
        if colour.get(root).copied().unwrap_or(0) != 0 {
            continue;
        }
        let mut path: Vec<String> = Vec::new();
        let mut stack: Vec<(String, bool)> = vec![(root.clone(), false)];
        while let Some((node, exiting)) = stack.pop() {
            if exiting {
                colour.insert(node, 2);
                path.pop();
                continue;
            }
            match colour.get(&node).copied().unwrap_or(0) {
                // A back edge into the current path is exactly a cycle.
                1 => {
                    if let Some(pos) = path.iter().position(|n| *n == node) {
                        let mut cycle: Vec<String> = path.get(pos..).unwrap_or(&[]).to_vec();
                        cycle.push(node);
                        return Some(cycle);
                    }
                    continue;
                }
                2 => continue,
                _ => {}
            }
            colour.insert(node.clone(), 1);
            path.push(node.clone());
            stack.push((node.clone(), true));
            if let Some(succ) = graph.get(&node) {
                for s in succ.iter().rev() {
                    stack.push((s.clone(), false));
                }
            }
        }
    }
    None
}

/// `A =>+ A`: the automaton builds fine but a reduce can loop forever, so catch
/// it up front where the message can be clear.
fn check_cycles(grammar: &Grammar) -> Result<()> {
    let graph = unit_graph(grammar);
    let roots: Vec<String> = grammar.nonterminals.clone();
    if let Some(cycle) = find_cycle(&graph, &roots) {
        return Err(EngineError::new(format!(
            "This grammar has a cycle: {}. A non-terminal can derive itself with nothing else, which lets the parser reduce forever without consuming input.",
            cycle.join(" => ")
        ))
        .hint("Remove one of the unit productions in the cycle, or give it something else to derive."));
    }
    Ok(())
}

/// Same graph as [`unit_graph`] but only requiring the *prefix* to vanish, so
/// it catches `E -> E + T` as well as `A -> B`, `B -> A`.
fn left_recursion(grammar: &Grammar) -> Option<String> {
    let nullable = nullable_set(grammar);
    let mut graph: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for p in &grammar.productions {
        for sym in &p.rhs {
            if grammar.is_nonterminal(sym) {
                graph.entry(p.lhs.clone()).or_default().insert(sym.clone());
            }
            if !nullable.contains(sym) {
                break;
            }
        }
    }
    find_cycle(&graph, &grammar.nonterminals).map(|c| c.join(" => "))
}

fn unreachable_nonterminals(grammar: &Grammar) -> Vec<String> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut queue: Vec<&str> = vec![grammar.start.as_str()];
    seen.insert(grammar.start.as_str());
    while let Some(nt) = queue.pop() {
        for idx in grammar.productions_for(nt) {
            let Some(p) = grammar.productions.get(*idx) else { continue };
            for sym in &p.rhs {
                if grammar.is_nonterminal(sym) && seen.insert(sym.as_str()) {
                    queue.push(sym.as_str());
                }
            }
        }
    }
    grammar
        .nonterminals
        .iter()
        .filter(|n| !seen.contains(n.as_str()))
        .cloned()
        .collect()
}

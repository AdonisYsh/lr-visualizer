//! Random-grammar property tests: small grammars from `proptest`, run under all four methods.
//! Properties: clean answers; state counts; LALR(1) = CLR(1) merged by core; conflict
//! monotonicity; merging adds no shift-reduce conflict; derived sentences are accepted;
//! acceptance agrees with an Earley recogniser. Cases per property: 256, or set `PROPTEST_CASES`.
//! Failures are shrunk and saved to `tests/random_grammars.proptest-regressions`.

use lr_engine::analyze;
use proptest::prelude::*;
use proptest::test_runner::{
    Config, FileFailurePersistence, TestCaseError, TestCaseResult, TestError, TestRunner,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Mutex;
use std::time::Instant;

const VARIANTS: [&str; 4] = ["lr0", "slr1", "lalr1", "clr1"];
const PHASES: [&str; 5] = ["grammar", "sets", "automaton", "table", "parse"];

type R<T> = Result<T, TestCaseError>;

fn bad<T>(message: impl Into<String>) -> R<T> {
    Err(TestCaseError::fail(message.into()))
}

// ---------------------------------------------------------------------------
// The grammar model
// ---------------------------------------------------------------------------

/// One symbol of a generated grammar: terminal `t<i>` or non-terminal `N<i>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Sym {
    T(usize),
    N(usize),
}

/// `rules[i]` is the list of alternatives of `N<i>`; `N0` is the start symbol.
/// An empty alternative is epsilon.
#[derive(Clone)]
struct Gram {
    rules: Vec<Vec<Vec<Sym>>>,
}

impl fmt::Debug for Gram {
    // The grammar as engine text, so a shrunk counterexample reads as a
    // grammar and can be pasted straight into the tool.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Gram {{\n{}\n}}", self.text())
    }
}

fn sym_name(s: Sym) -> String {
    match s {
        Sym::T(i) => format!("t{i}"),
        Sym::N(i) => format!("N{i}"),
    }
}

fn terminal_names(tokens: &[usize]) -> Vec<String> {
    tokens.iter().map(|t| format!("t{t}")).collect()
}

impl Gram {
    /// The grammar as the engine reads it. `N0` is longer than one character,
    /// so the engine always reads it in space-separated mode.
    fn text(&self) -> String {
        self.rules
            .iter()
            .enumerate()
            .map(|(i, alts)| {
                let alts: Vec<String> = alts
                    .iter()
                    .map(|alt| {
                        if alt.is_empty() {
                            "ε".to_string()
                        } else {
                            alt.iter().map(|s| sym_name(*s)).collect::<Vec<_>>().join(" ")
                        }
                    })
                    .collect();
                format!("N{i} -> {}", alts.join(" | "))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The terminals that occur on some right-hand side: exactly the ones the
    /// engine will accept in a test string.
    fn used_terminals(&self) -> Vec<usize> {
        let mut seen = BTreeSet::new();
        for alt in self.rules.iter().flatten() {
            for s in alt {
                if let Sym::T(t) = s {
                    seen.insert(*t);
                }
            }
        }
        seen.into_iter().collect()
    }

    /// Which non-terminals can derive the empty string.
    fn nullable(&self) -> Vec<bool> {
        let mut nullable = vec![false; self.rules.len()];
        loop {
            let mut changed = false;
            for (i, alts) in self.rules.iter().enumerate() {
                let yes = alts.iter().any(|alt| {
                    alt.iter().all(|s| matches!(s, Sym::N(j) if nullable.get(*j) == Some(&true)))
                });
                if yes && !nullable[i] {
                    nullable[i] = true;
                    changed = true;
                }
            }
            if !changed {
                return nullable;
            }
        }
    }

    /// Which non-terminals derive some string of terminals.
    fn productive(&self) -> Vec<bool> {
        let mut productive = vec![false; self.rules.len()];
        loop {
            let mut changed = false;
            for (i, alts) in self.rules.iter().enumerate() {
                let yes = alts.iter().any(|alt| {
                    alt.iter().all(|s| match s {
                        Sym::T(_) => true,
                        Sym::N(j) => productive.get(*j) == Some(&true),
                    })
                });
                if yes && !productive[i] {
                    productive[i] = true;
                    changed = true;
                }
            }
            if !changed {
                return productive;
            }
        }
    }

/// Whether some non-terminal derives itself alone, `A =>+ A`, counting steps whose other
/// symbols can vanish.
    fn has_unit_cycle(&self) -> bool {
        let n = self.rules.len();
        let nullable = self.nullable();
        let mut reach = vec![vec![false; n]; n];
        for (a, alts) in self.rules.iter().enumerate() {
            for alt in alts {
                for (k, s) in alt.iter().enumerate() {
                    let Sym::N(b) = s else { continue };
                    let rest_vanishes = alt.iter().enumerate().all(|(j, other)| {
                        j == k || matches!(other, Sym::N(m) if nullable.get(*m) == Some(&true))
                    });
                    if rest_vanishes {
                        reach[a][*b] = true;
                    }
                }
            }
        }
        for k in 0..n {
            for i in 0..n {
                for j in 0..n {
                    if reach[i][k] && reach[k][j] {
                        reach[i][j] = true;
                    }
                }
            }
        }
        (0..n).any(|i| reach[i][i])
    }

/// Whether every non-terminal is reachable from `N0`; measures how much of a grammar is live.
    fn all_reachable(&self) -> bool {
        let mut seen = vec![false; self.rules.len()];
        let mut work = vec![0usize];
        while let Some(i) = work.pop() {
            if std::mem::replace(&mut seen[i], true) {
                continue;
            }
            for s in self.rules[i].iter().flatten() {
                if let Sym::N(j) = s {
                    work.push(*j);
                }
            }
        }
        seen.iter().all(|s| *s)
    }

    /// The engine must refuse this grammar (cycle, or a non-terminal that
    /// derives nothing) - restated here without looking at the engine.
    fn must_be_refused(&self) -> bool {
        self.has_unit_cycle() || self.productive().iter().any(|p| !p)
    }

/// Per non-terminal, an alternative of minimal derived length (Bellman-Ford, strict
/// improvement only, so following the choices always ends).
    fn shortest_alts(&self) -> Vec<Option<usize>> {
        let n = self.rules.len();
        let mut best: Vec<Option<usize>> = vec![None; n];
        let mut choice: Vec<Option<usize>> = vec![None; n];
        for _ in 0..1000 {
            let mut changed = false;
            for (i, alts) in self.rules.iter().enumerate() {
                for (a, alt) in alts.iter().enumerate() {
                    let mut total = Some(0usize);
                    for s in alt {
                        total = match (total, s) {
                            (Some(t), Sym::T(_)) => Some(t + 1),
                            (Some(t), Sym::N(j)) => best[*j].map(|b| t + b),
                            (None, _) => None,
                        };
                    }
                    if let Some(total) = total {
                        let better = match best[i] {
                            None => true,
                            Some(b) => total < b,
                        };
                        if better {
                            best[i] = Some(total);
                            choice[i] = Some(a);
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
        choice
    }

/// A random sentence by leftmost derivation; `tape` supplies the choices. Past `depth_bound`,
/// `budget` or the tape's end, shortest alternatives force it to finish. `None` if too long.
    fn derive(&self, tape: &[u8], depth_bound: usize, budget: usize) -> Option<Vec<usize>> {
        let shortest = self.shortest_alts();
        let mut out = Vec::new();
        let mut stack = vec![(Sym::N(0), 0usize)];
        let (mut next, mut expansions, mut fuel) = (0usize, 0usize, 5_000usize);
        while let Some((sym, depth)) = stack.pop() {
            fuel = fuel.checked_sub(1)?;
            match sym {
                Sym::T(t) => {
                    out.push(t);
                    if out.len() > 20 {
                        return None;
                    }
                }
                Sym::N(i) => {
                    let alts = self.rules.get(i)?;
                    let forced = depth >= depth_bound || expansions >= budget || next >= tape.len();
                    let pick = if forced {
                        (*shortest.get(i)?)?
                    } else {
                        let byte = *tape.get(next)?;
                        next += 1;
                        usize::from(byte) % alts.len()
                    };
                    expansions += 1;
                    for s in alts.get(pick)?.iter().rev() {
                        stack.push((*s, depth + 1));
                    }
                }
            }
        }
        Some(out)
    }
}

// ---------------------------------------------------------------------------
// The generator
// ---------------------------------------------------------------------------

/// The alternatives of `N<i>`: one terminal-only (so it is productive), up to two arbitrary,
/// in random order; usually a forward `N<i+1>` is added so most non-terminals stay reachable.
fn arb_rule(i: usize, nts: usize, ts: usize) -> impl Strategy<Value = Vec<Vec<Sym>>> {
    use proptest::sample::Index;
    let terminal_only = prop::collection::vec((0..ts).prop_map(Sym::T), 0..=3);
    let any_symbol = prop_oneof![
        3 => (0..ts).prop_map(Sym::T),
        2 => (0..nts).prop_map(Sym::N),
    ];
    let extra = prop::collection::vec(prop::collection::vec(any_symbol, 0..=4), 0..=2);
    let link = (prop::bool::weighted(0.75), any::<Index>(), any::<Index>());
    (terminal_only, extra, link)
        .prop_map(move |(base, mut extra, (link, which_alt, which_place))| {
            extra.insert(0, base);
            if link && i + 1 < nts {
                let at = which_alt.index(extra.len());
                if let Some(alt) = extra.get_mut(at) {
                    if alt.len() < 4 {
                        alt.insert(which_place.index(alt.len() + 1), Sym::N(i + 1));
                    }
                }
            }
            extra
        })
        .prop_shuffle()
}

fn arb_gram() -> impl Strategy<Value = Gram> {
    // One to four of each, weighted towards more non-terminals (shrinking walks down anyway).
    let nts = prop_oneof![
        1 => Just(1usize),
        2 => Just(2usize),
        3 => Just(3usize),
        3 => Just(4usize),
    ];
    (nts, 1usize..=4).prop_flat_map(|(nts, ts)| {
        // A Vec of strategies is a strategy for a Vec: one rule per non-terminal.
        let rules: Vec<_> = (0..nts).map(|i| arb_rule(i, nts, ts).boxed()).collect();
        rules.prop_map(|rules| Gram { rules })
    })
}

fn arb_tape() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..=40)
}

// ---------------------------------------------------------------------------
// Talking to the engine
// ---------------------------------------------------------------------------

/// One `analyze` call, checked for what must hold of every answer: valid JSON,
/// a boolean `ok`, and a message on every error.
fn analyze_json(variant: &str, text: &str, test: &str) -> R<Value> {
    let json = analyze(text, variant, test);
    let v: Value = match serde_json::from_str(&json) {
        Ok(v) => v,
        Err(e) => return bad(format!("{variant}: analyze did not return valid JSON: {e}")),
    };
    let Some(ok) = v["ok"].as_bool() else {
        return bad(format!("{variant}: `ok` is not a boolean: {}", v["ok"]));
    };
    if !ok {
        let message = v["error"]["message"].as_str().unwrap_or("");
        prop_assert!(!message.trim().is_empty(), "{variant}: an error must say what went wrong");
        prop_assert!(v.get("steps").is_none(), "{variant}: a refusal carries no steps");
    }
    Ok(v)
}

fn at<'a>(v: &'a Value, path: &[&str]) -> R<&'a Value> {
    let mut cur = v;
    for key in path {
        cur = match cur.get(*key) {
            Some(next) => next,
            None => return bad(format!("missing JSON field {}", path.join("."))),
        };
    }
    Ok(cur)
}

fn at_usize(v: &Value, path: &[&str]) -> R<usize> {
    match at(v, path)?.as_u64().and_then(|n| usize::try_from(n).ok()) {
        Some(n) => Ok(n),
        None => bad(format!("{} is not a number", path.join("."))),
    }
}

fn at_array<'a>(v: &'a Value, path: &[&str]) -> R<&'a Vec<Value>> {
    match at(v, path)?.as_array() {
        Some(a) => Ok(a),
        None => bad(format!("{} is not an array", path.join("."))),
    }
}

fn as_usize(v: &Value, what: &str) -> R<usize> {
    match v.as_u64().and_then(|n| usize::try_from(n).ok()) {
        Some(n) => Ok(n),
        None => bad(format!("{what} is not a number: {v}")),
    }
}

fn as_str<'a>(v: &'a Value, what: &str) -> R<&'a str> {
    match v.as_str() {
        Some(s) => Ok(s),
        None => bad(format!("{what} is not a string: {v}")),
    }
}

fn steps(v: &Value) -> R<&Vec<Value>> {
    at_array(v, &["steps"])
}

fn steps_of_kind<'a>(v: &'a Value, kind: &str) -> R<Vec<&'a Value>> {
    Ok(steps(v)?.iter().filter(|s| s["kind"] == kind).collect())
}

fn states(v: &Value) -> R<usize> {
    at_usize(v, &["table", "states"])
}

fn conflicts(v: &Value) -> R<&Vec<Value>> {
    at_array(v, &["table", "conflicts"])
}

fn conflicts_of_kind(v: &Value, kind: &str) -> R<usize> {
    Ok(conflicts(v)?.iter().filter(|c| c["kind"] == kind).count())
}

fn is_conflict_free(v: &Value) -> R<bool> {
    Ok(conflicts(v)?.is_empty())
}

/// What the four methods said about one accepted grammar.
struct Runs {
    lr0: Value,
    slr1: Value,
    lalr1: Value,
    clr1: Value,
}

impl Runs {
    fn get(&self, id: &str) -> &Value {
        match id {
            "lr0" => &self.lr0,
            "slr1" => &self.slr1,
            "lalr1" => &self.lalr1,
            _ => &self.clr1,
        }
    }

    fn all(&self) -> [(&'static str, &Value); 4] {
        [("lr0", &self.lr0), ("slr1", &self.slr1), ("lalr1", &self.lalr1), ("clr1", &self.clr1)]
    }
}

enum Outcome {
    Accepted(Box<Runs>),
    /// Refused by every method, with the same error: a grammar problem.
    Refused(String),
    /// Some method hit a size or work cap the others did not (e.g. CLR(1) past `MAX_STATES`); skipped.
    LimitHit,
}

/// Run the grammar under all four methods and sort the answers into one of
/// three outcomes, failing on anything else (a method disagreeing about
/// whether the grammar is valid, say).
fn run_all(text: &str) -> R<Outcome> {
    let mut answers = Vec::new();
    for id in VARIANTS {
        answers.push(analyze_json(id, text, "")?);
    }
    let ok: Vec<bool> = answers.iter().map(|v| v["ok"] == Value::Bool(true)).collect();
    if ok.iter().all(|b| *b) {
        let mut it = answers.into_iter();
        let (Some(lr0), Some(slr1), Some(lalr1), Some(clr1)) =
            (it.next(), it.next(), it.next(), it.next())
        else {
            return bad("four variants, fewer than four answers");
        };
        return Ok(Outcome::Accepted(Box::new(Runs { lr0, slr1, lalr1, clr1 })));
    }
    let messages: Vec<String> = answers
        .iter()
        .filter(|v| v["ok"] != Value::Bool(true))
        .map(|v| v["error"]["message"].as_str().unwrap_or("").to_string())
        .collect();
    let all_limits = messages.iter().all(|m| m.contains("more than"));
    if ok.iter().all(|b| !*b) {
        if messages.windows(2).all(|w| w[0] == w[1]) {
            return Ok(Outcome::Refused(messages[0].clone()));
        }
        if all_limits {
            return Ok(Outcome::LimitHit);
        }
        return bad(format!("the methods refuse this grammar for different reasons: {messages:?}"));
    }
    if all_limits {
        return Ok(Outcome::LimitHit);
    }
    bad(format!("some methods accept this grammar and others refuse it: {messages:?}"))
}

// ---------------------------------------------------------------------------
// Counting what was generated
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Stats {
    counts: Mutex<BTreeMap<&'static str, usize>>,
}

impl Stats {
    fn bump(&self, name: &'static str) {
        self.add(name, 1);
    }

    fn add(&self, name: &'static str, n: usize) {
        let mut guard = self.counts.lock().unwrap_or_else(|e| e.into_inner());
        *guard.entry(name).or_insert(0) += n;
    }

    fn get(&self, name: &str) -> usize {
        let guard = self.counts.lock().unwrap_or_else(|e| e.into_inner());
        guard.get(name).copied().unwrap_or(0)
    }

    fn line(&self) -> String {
        let guard = self.counts.lock().unwrap_or_else(|e| e.into_inner());
        guard.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" ")
    }
}

/// Proptest's configuration: 256 cases or `PROPTEST_CASES`, failures written
/// next to this file (unless `PROPTEST_DISABLE_FAILURE_PERSISTENCE` is set).
fn config() -> Config {
    let mut config = Config::default();
    if config.failure_persistence.is_some() {
        config.failure_persistence =
            Some(Box::new(FileFailurePersistence::WithSource("proptest-regressions")));
    }
    config.source_file = Some(file!());
    // Shrinking re-runs the engine many times; allow more than the default 1024 attempts,
    // but stop on the clock.
    if std::env::var_os("PROPTEST_MAX_SHRINK_ITERS").is_none() {
        config.max_shrink_iters = 20_000;
    }
    if std::env::var_os("PROPTEST_MAX_SHRINK_TIME").is_none() {
        config.max_shrink_time = 180_000;
    }
    config
}

/// Run one property, print generated/accepted/skipped, and fail on a counterexample or a
/// vacuous generator (under a fifth accepted, or a `must_count` counter still zero).
fn check<S>(
    name: &str,
    strategy: S,
    must_count: &[&'static str],
    body: impl Fn(&Stats, S::Value) -> TestCaseResult,
) where
    S: Strategy,
    S::Value: fmt::Debug,
{
    let stats = Stats::default();
    let started = Instant::now();
    let result: Result<(), TestError<S::Value>> =
        TestRunner::new(config()).run(&strategy, |value| {
            stats.bump("generated");
            body(&stats, value)
        });
    println!("[{name}] {} in {:.1?}", stats.line(), started.elapsed());
    if let Err(e) = result {
        panic!("property `{name}` failed: {e}");
    }
    let (generated, accepted) = (stats.get("generated"), stats.get("accepted"));
    if generated >= 50 {
        assert!(
            accepted * 5 >= generated,
            "[{name}] only {accepted} of {generated} generated grammars were accepted by the engine; \
             the generator is too weak for the property to mean anything"
        );
        for counter in must_count {
            assert!(
                stats.get(counter) > 0,
                "[{name}] `{counter}` is still zero after {generated} cases: the property checked nothing"
            );
        }
    }
}

/// Run the grammar under all four methods; `Some` only for an accepted one,
/// counting the rest.
fn accepted(stats: &Stats, g: &Gram) -> R<Option<(String, Box<Runs>)>> {
    let text = g.text();
    match run_all(&text)? {
        Outcome::Accepted(runs) => {
            stats.bump("accepted");
            if g.all_reachable() {
                stats.bump("all-nts-reachable");
            }
            Ok(Some((text, runs)))
        }
        Outcome::Refused(_) => {
            stats.bump("refused");
            Ok(None)
        }
        Outcome::LimitHit => {
            stats.bump("limit-skipped");
            Ok(None)
        }
    }
}

// ---------------------------------------------------------------------------
// An independent recogniser: Earley
// ---------------------------------------------------------------------------

/// Whether `input` (terminal indices) is in the language of `g`.
///
/// An item is (non-terminal, alternative, dot, origin). Each set is closed by re-running
/// predict and complete until nothing new appears; slow but obviously correct with epsilon.
fn earley(g: &Gram, input: &[usize]) -> bool {
    type Item = (usize, usize, usize, usize);
    let rhs = |nt: usize, alt: usize| -> &[Sym] { &g.rules[nt][alt] };
    let mut chart: Vec<BTreeSet<Item>> = vec![BTreeSet::new(); input.len() + 1];
    for alt in 0..g.rules[0].len() {
        chart[0].insert((0, alt, 0, 0));
    }
    for i in 0..=input.len() {
        loop {
            let before = chart[i].len();
            let items: Vec<Item> = chart[i].iter().copied().collect();
            for (nt, alt, dot, origin) in items {
                match rhs(nt, alt).get(dot) {
                    Some(Sym::N(m)) => {
                        for a in 0..g.rules[*m].len() {
                            chart[i].insert((*m, a, 0, i));
                        }
                    }
                    Some(Sym::T(t)) => {
                        if input.get(i) == Some(t) {
                            chart[i + 1].insert((nt, alt, dot + 1, origin));
                        }
                    }
                    None => {
                        let waiting: Vec<Item> = chart[origin]
                            .iter()
                            .filter(|(n2, a2, d2, _)| rhs(*n2, *a2).get(*d2) == Some(&Sym::N(nt)))
                            .copied()
                            .collect();
                        for (n2, a2, d2, o2) in waiting {
                            chart[i].insert((n2, a2, d2 + 1, o2));
                        }
                    }
                }
            }
            if chart[i].len() == before {
                break;
            }
        }
    }
    chart[input.len()]
        .iter()
        .any(|(nt, alt, dot, origin)| *nt == 0 && *origin == 0 && *dot == rhs(0, *alt).len())
}

/// A table-driven LR parser over the engine's `table` JSON (conflict-free tables only).
/// The engine skips the parse phase for an empty string; this also cross-checks `parser.rs`.
fn drive(v: &Value, tokens: &[String]) -> R<bool> {
    let productions = at_array(v, &["grammar", "productions"])?;
    let action = at(v, &["table", "action"])?;
    let goto = at(v, &["table", "goto"])?;
    let mut input: Vec<String> = tokens.to_vec();
    input.push("$".to_string());
    let mut stack: Vec<usize> = vec![0];
    let mut pos = 0usize;
    for _ in 0..10_000 {
        let Some(state) = stack.last().copied() else { return bad("driver: empty stack") };
        let Some(lookahead) = input.get(pos) else { return bad("driver: ran past the end") };
        let cell = action.get(state.to_string()).and_then(|row| row.get(lookahead));
        let actions: &[Value] = cell.and_then(Value::as_array).map_or(&[], Vec::as_slice);
        let [only] = actions else {
            if actions.is_empty() {
                return Ok(false);
            }
            return bad(format!("driver: conflicted cell ({state}, {lookahead}) in a conflict-free table"));
        };
        match only["type"].as_str() {
            Some("shift") => {
                stack.push(at_usize(only, &["state"])?);
                pos += 1;
            }
            Some("reduce") => {
                let p = at_usize(only, &["production"])?;
                let Some(production) = productions.get(p) else {
                    return bad(format!("driver: no production {p}"));
                };
                let len = at_array(production, &["rhs"])?.len();
                let keep = stack.len().saturating_sub(len);
                if keep == 0 {
                    return bad(format!("driver: reduce by {p} pops below the bottom of the stack"));
                }
                stack.truncate(keep);
                let (Some(top), lhs) = (stack.last().copied(), as_str(&production["lhs"], "lhs")?)
                else {
                    return bad("driver: empty stack after a reduce");
                };
                let target = goto.get(top.to_string()).and_then(|row| row.get(lhs));
                let Some(target) = target.and_then(Value::as_u64) else {
                    return bad(format!("driver: GOTO[{top}, {lhs}] is empty in a conflict-free table"));
                };
                stack.push(as_usize(&Value::from(target), "goto")?);
            }
            Some("accept") => return Ok(pos + 1 == input.len()),
            other => return bad(format!("driver: unexpected action type {other:?}")),
        }
    }
    bad("driver: no verdict within 10000 moves")
}

/// Does `id`, run on `tokens`, accept? `runs` supplies the table for the empty string;
/// otherwise the engine's parse steps are read and the driver above must agree.
fn engine_accepts(runs: &Runs, id: &str, text: &str, tokens: &[usize]) -> R<bool> {
    let names = terminal_names(tokens);
    if tokens.is_empty() {
        return drive(runs.get(id), &names);
    }
    let v = analyze_json(id, text, &names.join(" "))?;
    prop_assert_eq!(&v["ok"], &Value::Bool(true), "{}: the parse was refused: {}", id, v["error"]);
    let accepts = steps_of_kind(&v, "parseAccept")?.len();
    let rejects = steps_of_kind(&v, "parseReject")?.len();
    prop_assert_eq!(accepts + rejects, 1, "{}: a parse ends in exactly one accept or reject", id);
    let engine = accepts == 1;
    let driven = drive(&v, &names)?;
    prop_assert_eq!(engine, driven, "{}: the engine's parse and the table-driver disagree", id);
    Ok(engine)
}

// ---------------------------------------------------------------------------
// Reading item sets out of the step log
// ---------------------------------------------------------------------------

type Core = BTreeSet<(usize, usize)>;
/// An item set as the engine shows it: one entry per core, with its lookaheads
/// (empty for LR(0) items).
type ItemSet = BTreeMap<(usize, usize), BTreeSet<String>>;

fn read_items(items: &Value, lookaheads: bool, what: &str) -> R<ItemSet> {
    let Some(items) = items.as_array() else { return bad(format!("{what}: items is not an array")) };
    let mut set = ItemSet::new();
    for item in items {
        let core = (at_usize(item, &["production"])?, at_usize(item, &["dot"])?);
        let las: BTreeSet<String> = match (&item["lookaheads"], lookaheads) {
            (Value::Array(a), true) => {
                let mut out = BTreeSet::new();
                for la in a {
                    out.insert(as_str(la, "lookahead")?.to_string());
                }
                out
            }
            (Value::Null, false) => BTreeSet::new(),
            (other, _) => {
                return bad(format!("{what}: lookaheads {other} where lookaheads={lookaheads} was expected"))
            }
        };
        if lookaheads {
            prop_assert!(!las.is_empty(), "{}: an item with an empty lookahead set", what);
        }
        prop_assert!(
            set.insert(core, las).is_none(),
            "{}: item {:?} listed twice in one state; items with one core must share a line",
            what,
            core
        );
    }
    Ok(set)
}

/// Every `stateClosed` step, by state id, which must run 0..n without gaps.
fn closed_states(v: &Value, lookaheads: bool) -> R<Vec<ItemSet>> {
    let mut by_id = BTreeMap::new();
    for s in steps_of_kind(v, "stateClosed")? {
        let id = at_usize(s, &["id"])?;
        let items = read_items(&s["items"], lookaheads, &format!("stateClosed I{id}"))?;
        prop_assert!(by_id.insert(id, items).is_none(), "state I{} was closed twice", id);
    }
    let ids: Vec<usize> = by_id.keys().copied().collect();
    prop_assert_eq!(ids, (0..by_id.len()).collect::<Vec<_>>(), "closed state ids run 0..n");
    Ok(by_id.into_values().collect())
}

fn core_of(items: &ItemSet) -> Core {
    items.keys().copied().collect()
}

// ---------------------------------------------------------------------------
// Property 1: clean answers
// ---------------------------------------------------------------------------

fn clean_answers(stats: &Stats, g: Gram) -> TestCaseResult {
    let text = g.text();
    let expect_refusal = g.must_be_refused();
    if expect_refusal {
        stats.bump("oracle-says-refuse");
    }
    match run_all(&text)? {
        Outcome::LimitHit => {
            stats.bump("limit-skipped");
        }
        Outcome::Refused(message) => {
            stats.bump("refused");
            prop_assert!(
                expect_refusal,
                "the engine refused a grammar that has no cycle and derives strings: {}\n{}",
                message,
                text
            );
        }
        Outcome::Accepted(runs) => {
            stats.bump("accepted");
            prop_assert!(
                !expect_refusal,
                "the engine accepted a grammar with a cycle or an unproductive non-terminal:\n{}",
                text
            );
            well_formed(&runs)?;
        }
    }
    Ok(())
}

/// What every accepted analysis must look like, whatever the grammar.
fn well_formed(runs: &Runs) -> R<()> {
    for (id, v) in runs.all() {
        let list = steps(v)?;
        prop_assert!(!list.is_empty(), "{}: no steps", id);
        let mut last_phase = 0usize;
        for (position, step) in list.iter().enumerate() {
            let what = format!("{id}: step {position} ({})", step["kind"]);
            prop_assert_eq!(as_usize(&step["index"], &what)?, position, "{}: index", what);
            let phase = as_str(&step["phase"], &what)?;
            let Some(rank) = PHASES.iter().position(|p| *p == phase) else {
                return bad(format!("{what}: unknown phase {phase}"));
            };
            prop_assert!(rank >= last_phase, "{}: phase {} appears after a later phase", what, phase);
            last_phase = rank;
            let explain = as_str(&step["explain"], &what)?;
            prop_assert!(!explain.trim().is_empty(), "{}: empty explain", what);
            prop_assert!(
                !explain.to_lowercase().contains("grammar is ambiguous"),
                "{}: a step claims the grammar is ambiguous: {}",
                what,
                explain
            );
        }
        prop_assert!(last_phase < 4, "{}: parse steps appeared although no test string was given", id);

        let chosen = steps_of_kind(v, "variantChosen")?;
        prop_assert_eq!(chosen.len(), 1, "{}: one variantChosen step", id);
        prop_assert_eq!(chosen[0]["variant"].as_str(), Some(id), "variantChosen names the method run");

        let valid = at(v, &["verdict", "isValid"])?.as_bool();
        prop_assert_eq!(valid, Some(is_conflict_free(v)?), "{}: verdict.isValid vs table.conflicts", id);
        let message = as_str(at(v, &["verdict", "message"])?, "verdict.message")?;
        prop_assert!(!message.trim().is_empty(), "{}: empty verdict message", id);
        prop_assert!(
            !message.to_lowercase().contains("ambig"),
            "{}: the verdict message uses a form of `ambiguous`: {}",
            id,
            message
        );
        if let Some(note) = at(v, &["verdict", "note"])?.as_str() {
            prop_assert!(
                !note.to_lowercase().contains("grammar is ambiguous"),
                "{}: the verdict note says the grammar is ambiguous: {}",
                id,
                note
            );
        }
    }
    // The grammar itself does not depend on the method.
    for (id, v) in runs.all() {
        prop_assert_eq!(&v["grammar"], &runs.lr0["grammar"], "{}: grammar payload differs from lr0", id);
        prop_assert_eq!(&v["warnings"], &runs.lr0["warnings"], "{}: warnings differ from lr0", id);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Property 2: state counts
// ---------------------------------------------------------------------------

fn state_counts(stats: &Stats, g: Gram) -> TestCaseResult {
    let Some((_, runs)) = accepted(stats, &g)? else { return Ok(()) };
    let (lr0, slr1) = (states(&runs.lr0)?, states(&runs.slr1)?);
    let (lalr1, clr1) = (states(&runs.lalr1)?, states(&runs.clr1)?);
    prop_assert_eq!(slr1, lr0, "SLR(1) builds the LR(0) automaton");
    prop_assert_eq!(lalr1, lr0, "LALR(1) has one state per LR(0) state");
    prop_assert!(lalr1 <= clr1, "LALR(1) has {} states, more than CLR(1)'s {}", lalr1, clr1);
    if clr1 > lalr1 {
        stats.bump("clr1-bigger");
    }

    // SLR(1) differs from LR(0) in where it reduces, never in the item sets.
    prop_assert_eq!(
        closed_states(&runs.lr0, false)?,
        closed_states(&runs.slr1, false)?,
        "LR(0) and SLR(1) must close identical item sets"
    );

    // AutomatonDone reports the collection the method built: the canonical
    // one for LALR(1), which merges it only afterwards.
    for (id, v) in runs.all() {
        let done = steps_of_kind(v, "automatonDone")?;
        prop_assert_eq!(done.len(), 1, "{}: one automatonDone step", id);
        let built = at_usize(done[0], &["stateCount"])?;
        let expected = if id == "lalr1" { clr1 } else { states(v)? };
        prop_assert_eq!(built, expected, "{}: automatonDone.stateCount", id);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Property 3: LALR(1) is CLR(1) merged by core
// ---------------------------------------------------------------------------

fn lalr_is_merged_clr(stats: &Stats, g: Gram) -> TestCaseResult {
    let Some((_, runs)) = accepted(stats, &g)? else { return Ok(()) };

    // The canonical collection, as CLR(1) itself builds it - and the LALR(1)
    // run must have started from exactly the same one.
    let clr = closed_states(&runs.clr1, true)?;
    prop_assert_eq!(clr.len(), states(&runs.clr1)?, "CLR(1): one stateClosed per state");
    prop_assert_eq!(
        &closed_states(&runs.lalr1, true)?,
        &clr,
        "the LALR(1) run must build the CLR(1) collection before merging"
    );

    // Group by core, in order of first appearance - what the groups must be.
    let mut ids_by_core: BTreeMap<Core, Vec<usize>> = BTreeMap::new();
    for (id, state) in clr.iter().enumerate() {
        ids_by_core.entry(core_of(state)).or_default().push(id);
    }
    let mut expected: Vec<Vec<usize>> = ids_by_core.values().cloned().collect();
    expected.sort();

    let done = steps_of_kind(&runs.lalr1, "mergeDone")?;
    prop_assert_eq!(done.len(), 1, "one mergeDone step");
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for group in at_array(done[0], &["groups"])? {
        let Some(members) = group.as_array() else { return bad("a mergeDone group is not an array") };
        let mut ids = Vec::new();
        for m in members {
            ids.push(as_usize(m, "group member")?);
        }
        groups.push(ids);
    }
    prop_assert_eq!(&groups, &expected, "mergeDone.groups: CLR(1) states grouped by core, numbered by smallest member");
    prop_assert_eq!(at_usize(done[0], &["before"])?, clr.len(), "mergeDone.before");
    prop_assert_eq!(at_usize(done[0], &["after"])?, groups.len(), "mergeDone.after");
    prop_assert_eq!(states(&runs.lalr1)?, groups.len(), "LALR(1) table.states is the number of cores");

    // The LALR(1) cores are the distinct CLR(1) cores, each exactly once...
    let lalr_cores: Vec<Core> = groups.iter().map(|grp| core_of(&clr[grp[0]])).collect();
    let distinct: BTreeSet<&Core> = lalr_cores.iter().collect();
    prop_assert_eq!(distinct.len(), lalr_cores.len(), "two LALR(1) states share a core");
    prop_assert_eq!(distinct.len(), ids_by_core.len(), "LALR(1) cores vs distinct CLR(1) cores");
    // ...and, independently of the CLR(1) code path, they are the LR(0) states.
    let lr0_cores: BTreeSet<Core> = closed_states(&runs.lr0, false)?.iter().map(core_of).collect();
    prop_assert_eq!(
        lalr_cores.iter().cloned().collect::<BTreeSet<Core>>(),
        lr0_cores,
        "the LALR(1) cores are the LR(0) item sets"
    );

    // Each merged state is the union of its members, item by item.
    let merged_steps = steps_of_kind(&runs.lalr1, "statesMerged")?;
    let folded = groups.iter().filter(|g| g.len() > 1).count();
    prop_assert_eq!(merged_steps.len(), folded, "one statesMerged step per group of more than one");
    if folded > 0 {
        stats.bump("grammars-with-a-merge");
    }
    for (new_id, group) in groups.iter().enumerate() {
        if group.len() < 2 {
            continue;
        }
        let Some(step) = merged_steps.iter().find(|s| s["newId"].as_u64() == Some(new_id as u64)) else {
            return bad(format!("no statesMerged step for the group that became I{new_id}"));
        };
        let mut ids = Vec::new();
        for m in at_array(step, &["mergedIds"])? {
            ids.push(as_usize(m, "mergedIds member")?);
        }
        prop_assert_eq!(&ids, group, "statesMerged.mergedIds for I{}", new_id);

        let mut union = ItemSet::new();
        for member in group {
            for (core, las) in &clr[*member] {
                union.entry(*core).or_default().extend(las.iter().cloned());
            }
        }
        let shown = read_items(&step["items"], true, &format!("statesMerged I{new_id}"))?;
        prop_assert_eq!(&shown, &union, "I{}: lookaheads must be the union over {:?}", new_id, group);
        let shown_core = read_items(&step["core"], false, &format!("statesMerged core I{new_id}"))?;
        prop_assert_eq!(core_of(&shown), core_of(&shown_core), "statesMerged.core is the items without lookaheads");
        stats.add("states-folded-away", group.len() - 1);
    }

    // Every CLR(1) transition survives, redirected, in the LALR(1) table.
    let new_id_of: BTreeMap<usize, usize> = groups
        .iter()
        .enumerate()
        .flat_map(|(new_id, group)| group.iter().map(move |old| (*old, new_id)))
        .collect();
    let nonterminals: BTreeSet<&str> = at_array(&runs.lalr1, &["grammar", "nonterminals"])?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let table = at(&runs.lalr1, &["table"])?;
    for t in steps_of_kind(&runs.lalr1, "transitionAdded")? {
        let (from, to) = (at_usize(t, &["from"])?, at_usize(t, &["to"])?);
        let symbol = as_str(&t["symbol"], "transition symbol")?;
        let (Some(from), Some(to)) = (new_id_of.get(&from), new_id_of.get(&to)) else {
            return bad(format!("transition {from} -> {to} names a state outside the collection"));
        };
        if nonterminals.contains(symbol) {
            prop_assert_eq!(
                table["goto"][from.to_string()][symbol].as_u64(),
                Some(*to as u64),
                "GOTO[{}, {}] should be {}",
                from,
                symbol,
                to
            );
        } else {
            let cell = table["action"][from.to_string()][symbol].as_array();
            let shift = serde_json::json!({ "type": "shift", "state": to });
            prop_assert!(
                cell.is_some_and(|c| c.contains(&shift)),
                "ACTION[{}, {}] should contain shift {}",
                from,
                symbol,
                to
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Property 4: conflict ordering, and the comparison rows
// ---------------------------------------------------------------------------

fn conflict_ordering(stats: &Stats, g: Gram) -> TestCaseResult {
    let Some((_, runs)) = accepted(stats, &g)? else { return Ok(()) };
    let n = |id: &str| conflicts(runs.get(id)).map(Vec::len);
    let (c_lr0, c_slr1, c_lalr1, c_clr1) = (n("lr0")?, n("slr1")?, n("lalr1")?, n("clr1")?);

    // A stronger method may only gain conflict-freeness, never lose it.
    if c_lr0 == 0 {
        prop_assert!(c_slr1 == 0 && c_lalr1 == 0 && c_clr1 == 0, "LR(0) is clean but a stronger method is not");
    }
    if c_slr1 == 0 {
        prop_assert!(c_lalr1 == 0 && c_clr1 == 0, "SLR(1) is clean but LALR(1) or CLR(1) is not");
    }
    if c_lalr1 == 0 {
        prop_assert!(c_clr1 == 0, "LALR(1) is clean but CLR(1) is not");
    }
    // SLR(1) uses LR(0)'s automaton and only withdraws reduce entries, so each
    // cell can only lose actions, and with it conflict entries.
    prop_assert!(c_slr1 <= c_lr0, "SLR(1) has {} conflicts, more than LR(0)'s {}", c_slr1, c_lr0);
    stats.bump(if c_lr0 == 0 {
        "weakest-clean=LR(0)"
    } else if c_slr1 == 0 {
        "weakest-clean=SLR(1)"
    } else if c_lalr1 == 0 {
        "weakest-clean=LALR(1)"
    } else if c_clr1 == 0 {
        "weakest-clean=CLR(1)"
    } else {
        "weakest-clean=none"
    });

    // The comparison: one row per method, the same in every run, and equal to
    // what the method's own run reports.
    let reference = at_array(&runs.lr0, &["verdict", "comparison"])?;
    prop_assert_eq!(reference.len(), 4, "a row for each of the four methods");
    for (id, v) in runs.all() {
        prop_assert_eq!(at_array(v, &["verdict", "comparison"])?, reference, "{}: comparison differs from lr0's", id);
        let step = steps_of_kind(v, "variantComparison")?;
        prop_assert_eq!(step.len(), 1, "{}: one variantComparison step", id);
        prop_assert_eq!(&step[0]["results"], &Value::Array(reference.clone()), "{}: step vs verdict.comparison", id);
    }
    for (row, (id, own)) in reference.iter().zip(runs.all()) {
        let what = format!("comparison row for {id}");
        prop_assert_eq!(row["variant"].as_str(), Some(id), "{}: variant", what);
        prop_assert_eq!(at_usize(row, &["conflicts"])?, conflicts(own)?.len(), "{}: conflicts", what);
        prop_assert_eq!(at_usize(row, &["states"])?, states(own)?, "{}: states", what);
        prop_assert_eq!(&row["isValid"], &own["verdict"]["isValid"], "{}: isValid", what);
        prop_assert_eq!(at_usize(row, &["shiftReduce"])?, conflicts_of_kind(own, "shift-reduce")?, "{}: shiftReduce", what);
        prop_assert_eq!(at_usize(row, &["reduceReduce"])?, conflicts_of_kind(own, "reduce-reduce")?, "{}: reduceReduce", what);
        let induced = conflicts(own)?.iter().filter(|c| c["mergeInduced"] == Value::Bool(true)).count();
        prop_assert_eq!(at_usize(row, &["mergeInduced"])?, induced, "{}: mergeInduced", what);
        // The run's own message may carry a "try that" suggestion after the
        // row's text, never instead of it.
        let row_message = as_str(&row["message"], &what)?;
        let own_message = as_str(&own["verdict"]["message"], &what)?;
        prop_assert!(own_message.starts_with(row_message), "{}: {:?} does not start with {:?}", what, own_message, row_message);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Property 5: merging never adds a shift-reduce conflict
// ---------------------------------------------------------------------------

fn merging_adds_no_shift_reduce(stats: &Stats, g: Gram) -> TestCaseResult {
    let Some((_, runs)) = accepted(stats, &g)? else { return Ok(()) };
    let sr_clr1 = conflicts_of_kind(&runs.clr1, "shift-reduce")?;
    let sr_lalr1 = conflicts_of_kind(&runs.lalr1, "shift-reduce")?;
    if sr_clr1 > 0 {
        stats.bump("clr1-has-shift-reduce");
    }
    if sr_clr1 == 0 {
        prop_assert_eq!(sr_lalr1, 0, "CLR(1) has no shift-reduce conflict but LALR(1) has {}", sr_lalr1);
    }

    // Only LALR(1) merges, so only LALR(1) may flag a conflict as caused by
    // it, and what it flags must be a reduce-reduce conflict.
    for (id, v) in runs.all() {
        for c in conflicts(v)? {
            let induced = c["mergeInduced"] == Value::Bool(true);
            if id != "lalr1" {
                prop_assert!(!induced, "{}: a conflict flagged mergeInduced by a method that merges nothing", id);
            } else if induced {
                prop_assert_eq!(c["kind"].as_str(), Some("reduce-reduce"), "a merge-induced conflict must be reduce-reduce");
            }
        }
    }
    // If CLR(1) is clean, every LALR(1) conflict is the merge's doing.
    if conflicts(&runs.clr1)?.is_empty() {
        for c in conflicts(&runs.lalr1)? {
            stats.bump("merge-induced-conflicts");
            prop_assert_eq!(
                &c["mergeInduced"],
                &Value::Bool(true),
                "CLR(1) is clean, so this LALR(1) conflict must be flagged as caused by merging: {}",
                c
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Property 6: derived sentences are accepted
// ---------------------------------------------------------------------------

fn derived_sentences_are_accepted(stats: &Stats, (g, tapes): (Gram, Vec<Vec<u8>>)) -> TestCaseResult {
    let Some((text, runs)) = accepted(stats, &g)? else { return Ok(()) };
    let mut sentences: BTreeSet<Vec<usize>> = BTreeSet::new();
    for (i, tape) in tapes.iter().enumerate() {
        // Different depth bounds, so some sentences are shallow and some deep.
        let Some(sentence) = g.derive(tape, 2 + 2 * i, 25) else { continue };
        prop_assert!(earley(&g, &sentence), "derived {:?} is not in the language (the derivation or the recogniser is wrong)", sentence);
        sentences.insert(sentence);
    }
    stats.add("sentences", sentences.len());
    for (id, v) in runs.all() {
        if !is_conflict_free(v)? {
            continue;
        }
        stats.bump("conflict-free-methods");
        for sentence in &sentences {
            if sentence.is_empty() {
                stats.bump("empty-sentences");
            }
            let accepts = engine_accepts(&runs, id, &text, sentence)?;
            prop_assert!(accepts, "{} is conflict-free but rejects the sentence `{}` the grammar derives", id, terminal_names(sentence).join(" "));
            stats.bump("sentences-checked");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Property 7: acceptance agrees with Earley
// ---------------------------------------------------------------------------

/// One test string: the tape's first byte picks how it is made.
/// One test string; the tape's first byte picks: 0 random terminals, 1 a derived sentence,
/// 2 a derived sentence with one edit. Anything over six symbols falls back to random.
fn make_string(g: &Gram, alphabet: &[usize], mode: u8, tape: &[u8]) -> Vec<usize> {
    let byte = |i: usize| -> usize {
        if tape.is_empty() { i } else { usize::from(tape[i % tape.len()]) }
    };
    let pick = |i: usize| -> Option<usize> {
        if alphabet.is_empty() { None } else { alphabet.get(byte(i) % alphabet.len()).copied() }
    };
    let random = || -> Vec<usize> {
        let len = byte(0) % 7;
        (0..len).filter_map(|i| pick(i + 1)).collect()
    };
    let derived = || g.derive(tape.get(1..).unwrap_or(&[]), 3, 25).filter(|s| s.len() <= 6);
    match mode % 3 {
        0 => random(),
        1 => derived().unwrap_or_else(random),
        _ => {
            let Some(mut s) = derived() else { return random() };
            let at = if s.is_empty() { 0 } else { byte(1) % s.len() };
            match byte(2) % 3 {
                0 if !s.is_empty() => {
                    s.remove(at);
                }
                1 if !s.is_empty() => {
                    if let Some(t) = pick(3) {
                        s[at] = t;
                    }
                }
                _ => {
                    if let Some(t) = pick(3) {
                        s.insert(at, t);
                    }
                }
            }
            s.truncate(6);
            s
        }
    }
}

fn acceptance_agrees_with_earley(
    stats: &Stats,
    (g, specs): (Gram, Vec<(u8, Vec<u8>)>),
) -> TestCaseResult {
    let Some((text, runs)) = accepted(stats, &g)? else { return Ok(()) };
    let alphabet = g.used_terminals();
    let mut strings: BTreeSet<Vec<usize>> = BTreeSet::new();
    for (mode, tape) in &specs {
        strings.insert(make_string(&g, &alphabet, *mode, tape));
    }
    let recognised: Vec<(Vec<usize>, bool)> = strings.into_iter().map(|s| {
        let yes = earley(&g, &s);
        (s, yes)
    }).collect();
    for (id, v) in runs.all() {
        if !is_conflict_free(v)? {
            continue;
        }
        stats.bump("conflict-free-methods");
        for (string, in_language) in &recognised {
            let engine = engine_accepts(&runs, id, &text, string)?;
            prop_assert_eq!(
                engine,
                *in_language,
                "{}: the engine {} `{}`, Earley says it is {} the language",
                id,
                if engine { "accepts" } else { "rejects" },
                terminal_names(string).join(" "),
                if *in_language { "in" } else { "not in" }
            );
            stats.bump(if *in_language { "in-language-checked" } else { "not-in-language-checked" });
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The properties, run
// ---------------------------------------------------------------------------

#[test]
fn refusal_matches_an_independent_check() {
    // Property 1: clean answers (valid JSON, a message on every refusal, explained steps),
    // and refusal exactly when there is a cycle.
    check("1 clean answers", arb_gram(), &[], clean_answers);
}

#[test]
fn state_counts_follow_the_theory() {
    // Property 2.
    check("2 state counts", arb_gram(), &["clr1-bigger"], state_counts);
}

#[test]
fn lalr1_is_clr1_merged_by_core() {
    // Property 3.
    check("3 merge by core", arb_gram(), &["grammars-with-a-merge"], lalr_is_merged_clr);
}

#[test]
fn conflict_freeness_is_monotone_and_the_comparison_is_honest() {
    // Property 4.
    check(
        "4 conflict ordering",
        arb_gram(),
        &["weakest-clean=LR(0)", "weakest-clean=SLR(1)", "weakest-clean=none"],
        conflict_ordering,
    );
}

#[test]
fn merging_never_adds_a_shift_reduce_conflict() {
    // Property 5.
    check("5 merging and shift-reduce", arb_gram(), &["clr1-has-shift-reduce"], merging_adds_no_shift_reduce);
}

#[test]
fn derived_sentences_are_accepted_by_every_conflict_free_method() {
    // Property 6.
    check(
        "6 derived sentences",
        (arb_gram(), prop::collection::vec(arb_tape(), 3..=3)),
        &["sentences-checked", "empty-sentences"],
        derived_sentences_are_accepted,
    );
}

#[test]
fn acceptance_agrees_with_an_earley_recogniser() {
    // Property 7.
    let spec = (any::<u8>(), arb_tape());
    check(
        "7 accept vs Earley",
        (arb_gram(), prop::collection::vec(spec, 6..=6)),
        &["in-language-checked", "not-in-language-checked"],
        acceptance_agrees_with_earley,
    );
}

// ---------------------------------------------------------------------------
// The helpers, checked on grammars whose answers are known
// ---------------------------------------------------------------------------

use Sym::{N, T};

/// `N0 -> t0 N0 t1 | ε`: a^n b^n.
fn pairs() -> Gram {
    Gram { rules: vec![vec![vec![T(0), N(0), T(1)], vec![]]] }
}

/// `N0 -> N0 t0 N0 | t1`: ambiguous, and left- and right-recursive.
fn ambiguous() -> Gram {
    Gram { rules: vec![vec![vec![N(0), T(0), N(0)], vec![T(1)]]] }
}

/// `N0 -> N1 N2 t0`, `N1 -> ε | t1`, `N2 -> N1 N1`: nullable symbols in a row.
fn nullable_chain() -> Gram {
    Gram {
        rules: vec![
            vec![vec![N(1), N(2), T(0)]],
            vec![vec![], vec![T(1)]],
            vec![vec![N(1), N(1)]],
        ],
    }
}

#[test]
fn the_generated_text_is_what_the_engine_reads() {
    assert_eq!(pairs().text(), "N0 -> t0 N0 t1 | ε");
    assert_eq!(nullable_chain().text(), "N0 -> N1 N2 t0\nN1 -> ε | t1\nN2 -> N1 N1");
    assert_eq!(nullable_chain().used_terminals(), vec![0, 1]);
}

#[test]
fn earley_agrees_with_the_known_languages() {
    let g = pairs();
    for n in 0..4 {
        let mut s = vec![0; n];
        s.extend(vec![1; n]);
        assert!(earley(&g, &s), "a^{n} b^{n}");
    }
    for s in [vec![0], vec![1], vec![1, 0], vec![0, 0, 1], vec![0, 1, 1], vec![0, 1, 0, 1]] {
        assert!(!earley(&g, &s), "{s:?} is not a^n b^n");
    }

    let g = ambiguous();
    assert!(earley(&g, &[1]));
    assert!(earley(&g, &[1, 0, 1]));
    assert!(earley(&g, &[1, 0, 1, 0, 1]));
    for s in [vec![], vec![0], vec![1, 0], vec![0, 1], vec![1, 1], vec![1, 0, 0, 1]] {
        assert!(!earley(&g, &s), "{s:?} is not 1 (0 1)*");
    }

    // N0 = (ε|t1) (N1 N1) t0, so t0 preceded by zero to three t1.
    let g = nullable_chain();
    for ones in 0..=3 {
        let mut s = vec![1; ones];
        s.push(0);
        assert!(earley(&g, &s), "{ones} t1 then t0");
    }
    for s in [vec![], vec![1, 1, 1, 1, 0], vec![0, 0], vec![0, 1], vec![1]] {
        assert!(!earley(&g, &s), "{s:?}");
    }
}

#[test]
fn the_independent_refusal_check_is_right_about_known_grammars() {
    let cycle = Gram { rules: vec![vec![vec![N(0)], vec![T(0)]]] };
    let hidden_cycle = Gram { rules: vec![vec![vec![N(1), N(0)], vec![T(0)]], vec![vec![], vec![T(1)]]] };
    let unproductive = Gram { rules: vec![vec![vec![N(0), T(0)]]] };
    let left_recursive = Gram { rules: vec![vec![vec![N(0), T(0)], vec![T(0)]]] };
    let cases = [
        (cycle, true),
        (hidden_cycle, true),
        (unproductive, true),
        (left_recursive, false),
        (pairs(), false),
        (ambiguous(), false),
        (nullable_chain(), false),
    ];
    for (g, refused) in cases {
        assert_eq!(g.must_be_refused(), refused, "{g:?}");
        // The oracle only means something if the engine agrees with it.
        for id in VARIANTS {
            let v: Value = serde_json::from_str(&analyze(&g.text(), id, "")).expect("JSON");
            assert_eq!(v["ok"] == Value::Bool(false), refused, "{id}: {g:?} -> {}", v["error"]);
        }
    }
}

#[test]
fn derivation_ends_even_when_a_shortest_choice_could_loop() {
    // N0 -> N1 | t0, N1 -> N0 | t1 is a cycle (refused by the engine), but shortest_alts must
    // still pick the terminal alternatives.
    let g = Gram { rules: vec![vec![vec![N(1)], vec![T(0)]], vec![vec![N(0)], vec![T(1)]]] };
    let sentence = g.derive(&[], 5, 25).expect("a finite derivation exists");
    assert_eq!(sentence.len(), 1);
    assert!(earley(&g, &sentence));
    // A tape that chases the recursion still ends, via the depth bound.
    let g = ambiguous();
    for tape in [vec![0u8; 40], vec![1u8; 40], (0..40).collect::<Vec<u8>>()] {
        let sentence = g.derive(&tape, 3, 25).expect("bounded derivation");
        assert!(earley(&g, &sentence), "{sentence:?}");
    }
    // Longer than twenty symbols is refused rather than returned.
    assert!(pairs().derive(&[0u8; 40], 100, 100).is_none());
}

#[test]
fn the_table_driver_parses_the_textbook_expression_grammar() {
    let text = "E -> E + T | T\nT -> T * F | F\nF -> ( E ) | id";
    let v: Value = serde_json::from_str(&analyze(text, "slr1", "")).expect("JSON");
    let words = |s: &str| -> Vec<String> { s.split_whitespace().map(str::to_string).collect() };
    for s in ["id", "id + id * id", "( id + id ) * id", "( ( id ) )"] {
        assert!(drive(&v, &words(s)).expect("driver"), "{s}");
    }
    for s in ["", "id +", "+ id", "id id", "( id", "id )", "id + * id"] {
        assert!(!drive(&v, &words(s)).expect("driver"), "{s}");
    }
}

// ---------------------------------------------------------------------------
// The same property bodies, on grammars whose behaviour is known
// ---------------------------------------------------------------------------

/// The textbook presets in the generator's vocabulary. Random sampling rarely reaches some
/// corners (CLR(1) but not LALR(1) is one in thousands), so run the bodies on them directly.
fn textbook_grammars() -> Vec<(&'static str, Gram)> {
    vec![
        // E -> E + T | T ; T -> T * F | F ; F -> ( E ) | id
        (
            "expression",
            Gram {
                rules: vec![
                    vec![vec![N(0), T(0), N(1)], vec![N(1)]],
                    vec![vec![N(1), T(1), N(2)], vec![N(2)]],
                    vec![vec![T(2), N(0), T(3)], vec![T(4)]],
                ],
            },
        ),
        // S -> C C ; C -> c C | d
        (
            "two halves",
            Gram { rules: vec![vec![vec![N(1), N(1)]], vec![vec![T(0), N(1)], vec![T(1)]]] },
        ),
        // S -> L = R | R ; L -> * R | id ; R -> L
        (
            "pointers",
            Gram {
                rules: vec![
                    vec![vec![N(1), T(0), N(2)], vec![N(2)]],
                    vec![vec![T(1), N(2)], vec![T(2)]],
                    vec![vec![N(1)]],
                ],
            },
        ),
        // S -> i S e S | i S | a
        (
            "dangling else",
            Gram { rules: vec![vec![vec![T(0), N(0), T(1), N(0)], vec![T(0), N(0)], vec![T(2)]]] },
        ),
        // S -> a A d | b B d | a B e | b A e ; A -> c ; B -> c
        ("merge casualty", merge_casualty()),
        // S -> A | B ; A -> a ; B -> a
        ("reduce-reduce", Gram { rules: vec![vec![vec![N(1)], vec![N(2)]], vec![vec![T(0)]], vec![vec![T(0)]]] }),
        // S -> A a A b | B b B a ; A -> ε ; B -> ε
        (
            "two empty non-terminals",
            Gram {
                rules: vec![
                    vec![vec![N(1), T(0), N(1), T(1)], vec![N(2), T(1), N(2), T(0)]],
                    vec![vec![]],
                    vec![vec![]],
                ],
            },
        ),
        ("a^n b^n", pairs()),
        ("ambiguous", ambiguous()),
        ("nullable chain", nullable_chain()),
    ]
}

fn merge_casualty() -> Gram {
    Gram {
        rules: vec![
            vec![
                vec![T(0), N(1), T(1)],
                vec![T(2), N(2), T(1)],
                vec![T(0), N(2), T(3)],
                vec![T(2), N(1), T(3)],
            ],
            vec![vec![T(4)]],
            vec![vec![T(4)]],
        ],
    }
}

#[test]
fn every_property_holds_on_the_textbook_grammars() {
    let tapes = vec![vec![0u8; 30], vec![1, 0, 1, 1, 0, 0, 1, 0, 1, 1, 0, 1], vec![2, 2, 1, 0, 0, 3, 1, 1, 2, 0]];
    let specs: Vec<(u8, Vec<u8>)> = (0u8..6).map(|m| (m, tapes[usize::from(m) % 3].clone())).collect();
    for (name, g) in textbook_grammars() {
        let stats = Stats::default();
        let ran = clean_answers(&stats, g.clone())
            .and_then(|()| state_counts(&stats, g.clone()))
            .and_then(|()| lalr_is_merged_clr(&stats, g.clone()))
            .and_then(|()| conflict_ordering(&stats, g.clone()))
            .and_then(|()| merging_adds_no_shift_reduce(&stats, g.clone()))
            .and_then(|()| derived_sentences_are_accepted(&stats, (g.clone(), tapes.clone())))
            .and_then(|()| acceptance_agrees_with_earley(&stats, (g.clone(), specs.clone())));
        if let Err(e) = ran {
            panic!("{name}: {e}\n{g:?}");
        }
        assert_eq!(stats.get("accepted"), 7, "{name}: all seven property bodies must accept the grammar: {}", stats.line());
    }
}

#[test]
fn the_merge_casualty_reaches_the_merge_induced_branches() {
    let g = merge_casualty();
    let stats = Stats::default();
    merging_adds_no_shift_reduce(&stats, g.clone()).expect("property 5");
    assert!(stats.get("merge-induced-conflicts") > 0, "CLR(1) is clean and LALR(1) is not: {}", stats.line());
    let stats = Stats::default();
    conflict_ordering(&stats, g.clone()).expect("property 4");
    assert_eq!(stats.get("weakest-clean=CLR(1)"), 1, "only CLR(1) handles it: {}", stats.line());
    let stats = Stats::default();
    state_counts(&stats, g.clone()).expect("property 2");
    assert_eq!(stats.get("clr1-bigger"), 1);
    let stats = Stats::default();
    lalr_is_merged_clr(&stats, g).expect("property 3");
    assert_eq!(stats.get("grammars-with-a-merge"), 1);
}

//! Native timings of the engine over the performance corpus, as CSV.
//! Always run with `--release`; each measurement runs in a child killed on timeout.
//!     cargo run --release --bin bench -- [--out <file>] [--only <substring>]
//!                                        [--runs <n>] [--timeout-secs <s>] [--root <dir>]

use lr_engine::error::EngineError;
use lr_engine::table::Variant;
use lr_engine::trace::Recorder;
use lr_engine::{automaton, grammar, sets};
use serde_json::{json, Value};
use std::hint::black_box;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

/// The column layout; browser timings append to the same file, so do not reorder.
const HEADER: &str = "grammar,family,productions,terminals,nonterminals,variant,environment,metric,runs,median_ms,min_ms,max_ms,outcome,states,steps,conflicts,json_bytes,detail";

/// Weakest method first, the order the engine itself lists them in.
const VARIANTS: [&str; 4] = ["lr0", "slr1", "lalr1", "clr1"];

const METRICS: [&str; 2] = ["analyze", "automaton"];

const ENVIRONMENT: &str = if cfg!(debug_assertions) { "native-debug" } else { "native-release" };

const DEFAULT_RUNS: usize = 7;
const DEFAULT_TIMEOUT_SECS: u64 = 60;

/// A grammar whose warm-up takes longer than this gets `SLOW_RUNS` timed runs
/// instead of the full count.
const SLOW_RUN_MS: f64 = 5_000.0;
const SLOW_RUNS: usize = 3;

/// The hidden first argument that puts the binary in single-measurement mode.
const CHILD_FLAG: &str = "--measure-one";

const USAGE: &str = "usage: bench [--out <file>] [--only <substring>] [--runs <n>] [--timeout-secs <s>] [--root <dir>]";

/// Dispatch: the hidden child mode, `--help`, or a whole run.
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some(CHILD_FLAG) {
        return child_main(args.get(1..).unwrap_or(&[]));
    }
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    match parent_main(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("bench: {message}");
            ExitCode::from(2)
        }
    }
}

// ---------------------------------------------------------------------------
// Options and corpus
// ---------------------------------------------------------------------------

/// The command line, with the defaults filled in.
struct Options {
    root: PathBuf,
    out: Option<PathBuf>,
    only: Option<String>,
    runs: usize,
    timeout: Duration,
}

/// Read the command line. Every option takes a value, as `--flag value` or
/// `--flag=value`.
fn parse_options(args: &[String]) -> Result<Options, String> {
    let mut opts = Options {
        root: default_root(),
        out: None,
        only: None,
        runs: DEFAULT_RUNS,
        timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
    };
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) => (flag, Some(value.to_string())),
            None => (arg.as_str(), None),
        };
        if !["--out", "--only", "--runs", "--timeout-secs", "--root"].contains(&flag) {
            return Err(format!("unknown option `{arg}`\n{USAGE}"));
        }
        let value = match inline {
            Some(value) => value,
            None => rest.next().cloned().ok_or_else(|| format!("{flag} needs a value\n{USAGE}"))?,
        };
        match flag {
            "--out" => opts.out = Some(PathBuf::from(value)),
            "--only" => opts.only = Some(value),
            "--root" => opts.root = PathBuf::from(value),
            "--runs" => opts.runs = positive(flag, &value)?,
            _ => opts.timeout = Duration::from_secs(positive(flag, &value)? as u64),
        }
    }
    Ok(opts)
}

/// A count or a number of seconds: a whole number, and not zero.
fn positive(flag: &str, value: &str) -> Result<usize, String> {
    match value.parse::<usize>() {
        Ok(n) if n > 0 => Ok(n),
        _ => Err(format!("{flag} needs a whole number above zero, not `{value}`")),
    }
}

/// The project root, taken from where the crate was built so `cargo run`
/// finds the corpus from any working directory.
fn default_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map_or_else(|| PathBuf::from(".."), Path::to_path_buf)
}

/// One grammar file of the corpus.
struct Entry {
    /// The file stem, which is the grammar's name in the CSV.
    stem: String,
    family: &'static str,
    path: PathBuf,
}

/// The whole corpus in its fixed order: textbook, generated, pascal, each
/// sorted by file name. A missing family is an error, not a shorter run.
fn corpus(root: &Path) -> Result<Vec<Entry>, String> {
    let perf = root.join("tests").join("perf").join("grammars");
    let mut entries = Vec::new();
    for (family, dir, prefix) in [
        ("textbook", root.join("examples"), ""),
        ("generated", perf.clone(), "gen-"),
        ("pascal", perf, "pascal-subset"),
    ] {
        let files = grammar_files(&dir, prefix)?;
        if files.is_empty() {
            return Err(format!(
                "no `{prefix}*.txt` in {} - is --root the project root?",
                dir.display()
            ));
        }
        for path in files {
            let stem = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
            entries.push(Entry { stem, family, path });
        }
    }
    Ok(entries)
}

/// The `.txt` files in `dir` whose name starts with `prefix`, sorted by name.
fn grammar_files(dir: &Path, prefix: &str) -> Result<Vec<PathBuf>, String> {
    let listing =
        std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    let mut files: Vec<PathBuf> = listing
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "txt"))
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(prefix)))
        .collect();
    files.sort();
    Ok(files)
}

// ---------------------------------------------------------------------------
// The parent: one child per measurement, one CSV row per child
// ---------------------------------------------------------------------------

/// Measure the whole corpus, or the part `--only` selects, and write the CSV.
fn parent_main(args: &[String]) -> Result<(), String> {
    let opts = parse_options(args)?;
    let exe = std::env::current_exe().map_err(|e| format!("cannot find this binary: {e}"))?;
    let entries: Vec<Entry> = corpus(&opts.root)?
        .into_iter()
        .filter(|e| opts.only.as_ref().map_or(true, |only| e.stem.contains(only.as_str())))
        .collect();
    if entries.is_empty() {
        return Err("--only matched no grammar in the corpus".to_string());
    }

    let mut out: Box<dyn Write> = match &opts.out {
        None => Box::new(std::io::stdout()),
        Some(path) => {
            // tests/perf/out/ is gitignored, so it may not exist yet.
            if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir)
                    .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
            }
            let file = std::fs::File::create(path)
                .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
            Box::new(file)
        }
    };
    let cannot_write = |e: std::io::Error| format!("cannot write the CSV: {e}");
    writeln!(out, "{HEADER}").map_err(cannot_write)?;

    for entry in &entries {
        for variant in VARIANTS {
            for metric in METRICS {
                let (measured, end) = measure(&exe, entry, variant, metric, &opts);
                let row = Row::new(entry, variant, metric, &measured, &end, opts.timeout);
                eprintln!("{}", row.progress_line());
                // Flushed row by row, so an interrupted run still leaves a
                // valid CSV of everything measured so far.
                writeln!(out, "{}", row.csv_line()).and_then(|()| out.flush()).map_err(cannot_write)?;
            }
        }
    }
    Ok(())
}

/// Everything a child reported before it stopped.
#[derive(Default)]
struct Measured {
    /// The grammar's size as the engine's reader sees it, sent before any timing.
    counts: Option<Value>,
    /// The warm-up run: its outcome and what the result contained.
    warmup: Option<Value>,
    /// The timed runs, in milliseconds, in the order they were taken.
    samples: Vec<f64>,
}

impl Measured {
    /// Take in one line of the child's output; anything unrecognised is ignored.
    fn absorb(&mut self, line: &str) {
        let Ok(value) = serde_json::from_str::<Value>(line) else { return };
        match value["line"].as_str() {
            Some("grammar") => self.counts = Some(value),
            Some("warmup") => self.warmup = Some(value),
            Some("timed") => self.samples.extend(value["ms"].as_f64()),
            _ => {}
        }
    }
}

/// How a child's run of samples came to an end.
enum End {
    Finished(ExitStatus),
    TimedOut,
    /// The child could not be started or waited for.
    Failed(String),
}

/// Run one grammar x variant x metric in a child process. The child prints a
/// line after every call, so the deadline applies to each call on its own.
fn measure(exe: &Path, entry: &Entry, variant: &str, metric: &str, opts: &Options) -> (Measured, End) {
    let mut measured = Measured::default();
    let spawned = Command::new(exe)
        .arg(CHILD_FLAG)
        .arg(metric)
        .arg(variant)
        .arg(opts.runs.to_string())
        .arg(&entry.path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => return (measured, End::Failed(format!("could not start the measuring process: {e}"))),
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return (measured, End::Failed("the measuring process has no output pipe".to_string()));
    };

    // A blocking read cannot be given a deadline, so a thread does the reading
    // and the deadline goes on the channel instead.
    let (tx, rx) = mpsc::channel::<String>();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let end = loop {
        match rx.recv_timeout(opts.timeout) {
            Ok(line) => measured.absorb(&line),
            Err(RecvTimeoutError::Timeout) => {
                let _ = child.kill();
                let _ = child.wait();
                break End::TimedOut;
            }
            // The pipe closed: the child is done, one way or another.
            Err(RecvTimeoutError::Disconnected) => {
                break match child.wait() {
                    Ok(status) => End::Finished(status),
                    Err(e) => End::Failed(format!("could not wait for the measuring process: {e}")),
                };
            }
        }
    };
    let _ = reader.join();
    (measured, end)
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/// One CSV row, every field already text. Empty means "does not apply".
struct Row {
    fields: Vec<String>,
}

impl Row {
    /// Turn what a child reported, and how it ended, into the eighteen columns.
    fn new(
        entry: &Entry,
        variant: &str,
        metric: &str,
        measured: &Measured,
        end: &End,
        timeout: Duration,
    ) -> Row {
        let warmup = measured.warmup.as_ref();
        let (outcome, detail) = match end {
            End::TimedOut => {
                let during = match warmup {
                    None => "the warm-up run".to_string(),
                    Some(_) => format!("timed run {}", measured.samples.len() + 1),
                };
                let secs = timeout.as_secs();
                ("timeout".to_string(), format!("no result within {secs} s during {during}; the measuring process was killed"))
            }
            End::Failed(why) => ("error".to_string(), why.clone()),
            End::Finished(status) => match warmup {
                // No warm-up line and no timeout: the child itself died.
                None => ("error".to_string(), format!("the measuring process ended without a result ({status})")),
                Some(_) if !status.success() => {
                    ("error".to_string(), format!("the measuring process died part-way ({status})"))
                }
                Some(w) => (
                    w["outcome"].as_str().unwrap_or("error").to_string(),
                    w["detail"].as_str().unwrap_or("").to_string(),
                ),
            },
        };

        // Timings only from a measurement that ran to the end.
        let complete = matches!(end, End::Finished(status) if status.success());
        let mut sorted = measured.samples.clone();
        sorted.sort_by(f64::total_cmp);
        let timing = |ms: Option<f64>| match ms {
            Some(ms) if complete => format!("{ms:.3}"),
            _ => String::new(),
        };

        // Size from the result's `grammar` object, else from the reader, so error
        // and timeout rows still report it.
        let counts = warmup
            .map(|w| &w["grammar"])
            .filter(|g| g.is_object())
            .or(measured.counts.as_ref().map(|c| &c["grammar"]));
        let count = |key: &str| number(counts.map(|c| &c[key]));
        // What the result contained is only meaningful when there was a result.
        let succeeded = outcome == "ok";
        let fact = |key: &str| match warmup {
            Some(w) if succeeded => number(Some(&w[key])),
            _ => String::new(),
        };

        Row {
            fields: vec![
                entry.stem.clone(),
                entry.family.to_string(),
                count("productions"),
                count("terminals"),
                count("nonterminals"),
                variant.to_string(),
                ENVIRONMENT.to_string(),
                metric.to_string(),
                sorted.len().to_string(),
                timing(median(&sorted)),
                timing(sorted.first().copied()),
                timing(sorted.last().copied()),
                outcome,
                fact("states"),
                fact("steps"),
                fact("conflicts"),
                fact("jsonBytes"),
                detail,
            ],
        }
    }

    /// One column by position, in `HEADER` order; empty if there is no such column.
    fn field(&self, index: usize) -> &str {
        self.fields.get(index).map_or("", String::as_str)
    }

    /// The row as one line of CSV, quoted where it has to be.
    fn csv_line(&self) -> String {
        self.fields.iter().map(|f| csv_field(f)).collect::<Vec<_>>().join(",")
    }

    /// The same row for a human watching stderr.
    fn progress_line(&self) -> String {
        let head = format!("{:<26} {:<5} {:<9} {:<7}", self.field(0), self.field(5), self.field(7), self.field(12));
        let timing = if self.field(9).is_empty() {
            String::new()
        } else {
            format!(
                " median {:>10} ms  (min {}, max {}, {} runs)",
                self.field(9),
                self.field(10),
                self.field(11),
                self.field(8)
            )
        };
        let size = if self.field(13).is_empty() {
            String::new()
        } else {
            format!("  {} states, {} steps", self.field(13), self.field(14))
        };
        let detail = if self.field(17).is_empty() { String::new() } else { format!("  {}", self.field(17)) };
        format!("{head}{timing}{size}{detail}")
    }
}

/// A JSON number as CSV text; anything else, including absent, is empty.
fn number(value: Option<&Value>) -> String {
    value.and_then(Value::as_u64).map_or_else(String::new, |n| n.to_string())
}

/// The median of an already-sorted list: the middle value, or the mean of the
/// middle two when the count is even.
fn median(sorted: &[f64]) -> Option<f64> {
    let mid = sorted.len() / 2;
    let upper = *sorted.get(mid)?;
    if sorted.len() % 2 == 1 {
        return Some(upper);
    }
    let lower = *sorted.get(mid.checked_sub(1)?)?;
    Some((lower + upper) / 2.0)
}

/// RFC 4180 quoting: a field containing a comma, a quote or a line break is
/// wrapped in quotes, with every quote inside it doubled. Engine messages are
/// full sentences, so `detail` needs this more often than not.
fn csv_field(field: &str) -> String {
    if field.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

// ---------------------------------------------------------------------------
// The child: one grammar, one variant, one metric
// ---------------------------------------------------------------------------

/// `--measure-one <metric> <variant> <runs> <file>`: a warm-up run and then the
/// timed runs, one JSON line on stdout after each. The parent holds the clock
/// that matters; this process may be killed at any moment.
fn child_main(args: &[String]) -> ExitCode {
    let [metric, variant, runs, path] = args else {
        eprintln!("bench: {CHILD_FLAG} is internal; it takes <metric> <variant> <runs> <file>");
        return ExitCode::from(2);
    };
    let Ok(runs) = runs.parse::<usize>() else {
        eprintln!("bench: bad run count `{runs}`");
        return ExitCode::from(2);
    };
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("bench: cannot read {path}: {e}");
            return ExitCode::from(2);
        }
    };

    // Sent first and outside any timing, so an errored or timed-out row can
    // still say how large the grammar was.
    if let Ok(parsed) = grammar::parse(&text) {
        let g = &parsed.grammar;
        emit(&json!({
            "line": "grammar",
            "grammar": {
                // Minus one each: the augmented production and its start symbol
                // are the engine's additions, not part of what was written.
                "productions": g.productions.len().saturating_sub(1),
                "terminals": g.terminals.len(),
                "nonterminals": g.nonterminals.len().saturating_sub(1),
            },
        }));
    }

    match metric.as_str() {
        "analyze" => sample(runs, || time_analyze(&text, variant), |json| analyze_facts(json)),
        "automaton" => {
            let Some(variant) = Variant::IMPLEMENTED.iter().copied().find(|v| v.id() == variant) else {
                eprintln!("bench: unknown variant `{variant}`");
                return ExitCode::from(2);
            };
            sample(runs, || time_automaton(&text, variant), automaton_facts);
        }
        other => {
            eprintln!("bench: unknown metric `{other}`");
            return ExitCode::from(2);
        }
    }
    ExitCode::SUCCESS
}

/// One warm-up call, described in full, then the timed calls as times only
/// (the engine is deterministic, so they return what the warm-up did).
fn sample<T>(runs: usize, once: impl Fn() -> (f64, T), facts: impl Fn(&T) -> Value) {
    let (warmup_ms, result) = once();
    let mut line = facts(&result);
    if let Some(object) = line.as_object_mut() {
        object.insert("line".to_string(), json!("warmup"));
        object.insert("ms".to_string(), json!(warmup_ms));
    }
    emit(&line);
    drop(result);

    let runs = if warmup_ms > SLOW_RUN_MS { runs.min(SLOW_RUNS) } else { runs };
    for _ in 0..runs {
        let (ms, _) = once();
        emit(&json!({ "line": "timed", "ms": ms }));
    }
}

/// Print one protocol line and push it through the pipe at once: the parent's
/// deadline is measured from the last line it saw.
fn emit(line: &Value) {
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{line}");
    let _ = stdout.flush();
}

/// Time since `start` in milliseconds, keeping the fraction.
fn millis(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1_000.0
}

/// One call of the entry point the browser uses, with no test string and so no
/// parse phase. The clock stops before the result is looked at or freed.
fn time_analyze(text: &str, variant: &str) -> (f64, String) {
    let start = Instant::now();
    let json = black_box(lr_engine::analyze(black_box(text), black_box(variant), ""));
    (millis(start), json)
}

/// What an `analyze` result says about itself: sizes on success, the engine's
/// own message on failure.
fn analyze_facts(json: &str) -> Value {
    let Ok(result) = serde_json::from_str::<Value>(json) else {
        return json!({ "outcome": "error", "detail": "analyze() did not return valid JSON" });
    };
    if result["ok"].as_bool() != Some(true) {
        let message = result["error"]["message"].as_str().unwrap_or("analyze() failed without a message");
        let line = result["error"]["line"].as_u64().and_then(|line| usize::try_from(line).ok());
        return json!({ "outcome": "error", "detail": error_detail(line, message) });
    }
    let len = |value: &Value| value.as_array().map_or(0, Vec::len);
    json!({
        "outcome": "ok",
        "detail": "",
        "states": result["table"]["states"],
        "steps": len(&result["steps"]),
        // Counted per conflicted cell, as the table view and the verdict do.
        "conflicts": len(&result["table"]["conflicts"]),
        "jsonBytes": json.len(),
        "grammar": {
            "productions": len(&result["grammar"]["productions"]).saturating_sub(1),
            "terminals": len(&result["grammar"]["terminals"]),
            "nonterminals": len(&result["grammar"]["nonterminals"]).saturating_sub(1),
        },
    })
}

/// An engine error as the `detail` column shows it: the engine's own message,
/// led by the grammar line it points at when it is a reader error.
fn error_detail(line: Option<usize>, message: &str) -> String {
    match line {
        Some(line) => format!("line {line}: {message}"),
        None => message.to_string(),
    }
}

/// How many states a collection has and how many steps building it recorded.
struct Collection {
    states: usize,
    steps: usize,
}

/// One build of a single variant's own collection, timed from the grammar
/// text: read it, compute FIRST and FOLLOW, build the automaton.
fn time_automaton(text: &str, variant: Variant) -> (f64, Result<Collection, EngineError>) {
    let start = Instant::now();
    let built = black_box(build_collection(black_box(text), variant));
    (millis(start), built)
}

/// The grammar, sets and automaton phases of `analyze`, wired as `lib.rs` wires
/// them, with steps recorded and discarded: no table, comparison or JSON.
fn build_collection(text: &str, variant: Variant) -> Result<Collection, EngineError> {
    let parsed = grammar::parse(text)?;
    let g = &parsed.grammar;
    let mut scratch = Recorder::new();
    let sets = sets::compute(g, &mut scratch)?;
    let states = if variant.merges() {
        automaton::build_lalr(g, &sets, &mut scratch)?.merged.states.len()
    } else {
        automaton::build(g, variant.item_mode(&sets), &mut scratch)?.states.len()
    };
    Ok(Collection { states, steps: scratch.len() })
}

/// The `automaton` metric's counterpart of [`analyze_facts`]. `steps` here is
/// the sets and automaton phases only; there is no table and no JSON to size.
fn automaton_facts(built: &Result<Collection, EngineError>) -> Value {
    match built {
        Ok(c) => json!({ "outcome": "ok", "detail": "", "states": c.states, "steps": c.steps }),
        Err(e) => json!({ "outcome": "error", "detail": error_detail(e.line, &e.message) }),
    }
}

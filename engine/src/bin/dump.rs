//! Text dump of the full step log, for checking the engine without the UI.
//!
//!     cargo run --bin dump                        # expression grammar, LR(0)
//!     cargo run --bin dump -- --variant=slr1      # ...the same, under SLR(1)
//!     cargo run --bin dump -- [--variant=<id>] <file> [test string]

use std::io::{self, BufWriter, ErrorKind, Read, Write};
use std::process::ExitCode;

const EXPRESSION_GRAMMAR: &str = "E -> E + T | T\nT -> T * F | F\nF -> ( E ) | id";

fn main() -> ExitCode {
    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    // `println!` panics when stdout is closed (`dump | head`). A closed pipe
    // only means the reader has seen enough, so it is not a failure.
    match run(&mut out).and_then(|()| out.flush()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.kind() == ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("dump: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(out: &mut impl Write) -> io::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();

    // An optional leading --variant=<id>, so the documented invocations above
    // keep working unchanged.
    let mut variant = "lr0".to_string();
    if let Some(rest) = args.first().and_then(|a| a.strip_prefix("--variant=")) {
        variant = rest.to_string();
        args.remove(0);
    }

    let (text, test) = match args.split_first() {
        None => (EXPRESSION_GRAMMAR.to_string(), "id + id * id".to_string()),
        Some((path, rest)) => {
            let mut buf = String::new();
            if path == "-" {
                let _ = std::io::stdin().read_to_string(&mut buf);
            } else {
                buf = std::fs::read_to_string(path).unwrap_or_default();
            }
            (buf, rest.join(" "))
        }
    };

    let json = lr_engine::analyze(&text, &variant, &test);
    let value: serde_json::Value = match serde_json::from_str(&json) {
        Ok(v) => v,
        Err(e) => {
            writeln!(out, "output was not valid JSON: {e}")?;
            return Ok(());
        }
    };

    if value.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        writeln!(out, "ERROR: {}", serde_json::to_string_pretty(&value["error"]).unwrap_or_default())?;
        return Ok(());
    }

    writeln!(out, "=== VARIANT ===\n  {variant}\n")?;
    writeln!(out, "=== GRAMMAR ===")?;
    writeln!(out, "{}", serde_json::to_string_pretty(&value["grammar"]).unwrap_or_default())?;
    if let Some(ws) = value["warnings"].as_array() {
        if !ws.is_empty() {
            writeln!(out, "\n=== WARNINGS ===")?;
            for w in ws {
                writeln!(
                    out,
                    "  [{}] {}",
                    w["severity"].as_str().unwrap_or("?"),
                    w["message"].as_str().unwrap_or("")
                )?;
            }
        }
    }

    writeln!(out, "\n=== VERDICT ===")?;
    writeln!(out, "  {}", value["verdict"]["message"].as_str().unwrap_or(""))?;
    if let Some(note) = value["verdict"]["note"].as_str() {
        writeln!(out, "  {note}")?;
    }

    let empty = vec![];
    let steps = value["steps"].as_array().unwrap_or(&empty);
    writeln!(out, "\n=== STEPS ({}) ===", steps.len())?;
    let mut phase = String::new();
    for s in steps {
        let p = s["phase"].as_str().unwrap_or("").to_string();
        if p != phase {
            writeln!(out, "\n--- phase: {p} ---")?;
            phase = p;
        }
        writeln!(
            out,
            "[{:>4}] {:<18} {}",
            s["index"].as_u64().unwrap_or(0),
            s["kind"].as_str().unwrap_or("?"),
            s["explain"].as_str().unwrap_or("")
        )?;
    }

    writeln!(out, "\n=== TABLE ===")?;
    writeln!(out, "{}", serde_json::to_string_pretty(&value["table"]).unwrap_or_default())?;
    Ok(())
}

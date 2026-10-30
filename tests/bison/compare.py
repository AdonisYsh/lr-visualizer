#!/usr/bin/env python3
"""Cross-check the engine's LALR(1) and CLR(1) results against GNU Bison.

For each examples/*.txt preset: read the grammar and tables from the engine's
`dump` binary, check them against the handout, write a plain Bison grammar to
grammars/, run `bison -v` under LALR and canonical LR, and compare the two.
Anything the rules in results.md do not explain is a DISAGREEMENT. The engine
is never changed; the script only reports.

Standard library only. Run from the project root with
`python3 tests/bison/compare.py`. Exit status: 0 all agree,
1 a disagreement, 2 bison or cargo missing.
"""

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
EXAMPLES = ROOT / "examples"
ENGINE_MANIFEST = ROOT / "engine" / "Cargo.toml"
GRAMMAR_DIR = HERE / "grammars"
RESULTS_MD = HERE / "results.md"

# Bison shifts `$end` into one extra state (`$accept: S $end .`) where we
# accept in place, so it should always have exactly one state more. Checked, not assumed.
EXPECTED_STATE_OFFSET = 1

# (our variant id, our label, bison lr.type, bison label)
MODES = [
    ("lalr1", "LALR(1)", "lalr", "LALR"),
    ("clr1", "CLR(1)", "canonical-lr", "canonical-LR"),
]

LOUD = "!" * 72

# Not a preset: after `a` the cell on `a` holds a shift and two reduces, the one
# case where the engine's count and Bison's per-token count can differ (results.md, rule 2).
PROBE_NAME = "probe-three-actions.txt"
PROBE_TITLE = "(probe) shift + 2 reduces in one cell"
PROBE_GRAMMAR = "S -> A a | B a | a a\nA -> a\nB -> a\n"


# --------------------------------------------------------------------------
# The engine side
# --------------------------------------------------------------------------

def build_dump():
    """Build the engine's `dump` binary once and return its path."""
    proc = subprocess.run(
        ["cargo", "build", "--locked", "--quiet", "--manifest-path", str(ENGINE_MANIFEST),
         "--bin", "dump", "--message-format=json"],
        capture_output=True, text=True, check=False,
    )
    if proc.returncode != 0:
        sys.stderr.write(proc.stderr)
        raise SystemExit("cargo build --bin dump failed")
    exe = None
    for line in proc.stdout.splitlines():
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        if msg.get("reason") == "compiler-artifact" and msg.get("executable") \
                and msg.get("target", {}).get("name") == "dump":
            exe = msg["executable"]
    if not exe:
        raise SystemExit("could not find the dump executable in cargo's output")
    return exe


def run_engine(dump, path, variant):
    """Run the engine on one preset under one variant; return its (grammar, table) JSON.

    The grammar comes from the engine itself, so Bison sees exactly what it read.
    """
    proc = subprocess.run([dump, f"--variant={variant}", str(path)],
                          capture_output=True, text=True, check=False)
    out = proc.stdout
    if proc.returncode != 0 or "=== GRAMMAR ===" not in out or "=== TABLE ===" not in out:
        raise SystemExit(f"engine failed on {path.name} ({variant}):\n{out}{proc.stderr}")
    grammar_text = out.split("=== GRAMMAR ===", 1)[1].split("\n=== ", 1)[0]
    table_text = out.split("=== TABLE ===", 1)[1]
    return json.loads(grammar_text), json.loads(table_text)


def our_numbers(table):
    """Read the engine's ACTION table cell by cell, translating it into Bison's counting."""
    cells = 0
    bison_like = []  # one (sr, rr) per state that has any conflict
    for _state, row in table["action"].items():
        st_sr = st_rr = 0
        for _symbol, entries in row.items():
            if len(entries) < 2:
                continue
            cells += 1
            # Accept is Bison's shift of $end, so it counts with the shifts.
            shifts = sum(1 for e in entries if e["type"] in ("shift", "accept"))
            reduces = sum(1 for e in entries if e["type"] == "reduce")
            # Bison's rule (results.md, rule 2): one S/R per token where a shift meets a reduction.
            if shifts and reduces:
                st_sr += 1
            if reduces >= 2:
                st_rr += reduces - 1
        if st_sr or st_rr:
            bison_like.append((st_sr, st_rr))
    # The engine's own count and kinds, exactly as its verdict reports them.
    conflicts = table.get("conflicts", [])
    note = None
    if cells != len(conflicts):
        # One conflict per colliding action: a cell with 3+ actions (none in the
        # presets) makes the engine's conflict count differ from its cell count.
        note = (f"engine lists {len(conflicts)} conflicts over {cells} conflicted "
                f"cells (a cell holds 3+ actions)")
    return {
        "states": table["states"],
        "cells": len(conflicts),
        "sr_cells": sum(1 for c in conflicts if c.get("kind") == "shift-reduce"),
        "rr_cells": sum(1 for c in conflicts if c.get("kind") == "reduce-reduce"),
        "note": note,
        "bison_sr": sum(s for s, _ in bison_like),
        "bison_rr": sum(r for _, r in bison_like),
        "per_state": sorted(bison_like),
    }


HANDOUT_RE = re.compile(r"conflicts/states:(.*)")
HANDOUT_ENTRY_RE = re.compile(r"(LR\(0\)|SLR\(1\)|LALR\(1\)|CLR\(1\))\s+(\d+)/(\d+)")


def handout_numbers(path):
    """The `conflicts/states:` line of a handout, as {label: (conflicts, states)}."""
    for line in path.read_text(encoding="utf-8").splitlines():
        m = HANDOUT_RE.search(line)
        if m:
            return {lab: (int(c), int(s)) for lab, c, s in HANDOUT_ENTRY_RE.findall(m.group(1))}
    return {}


def preset_title(path):
    """The handout's first comment line, e.g. `Expression grammar (classic)`."""
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if line.startswith("//"):
            return line[2:].strip()
    return path.stem


# --------------------------------------------------------------------------
# Writing the .y file
# --------------------------------------------------------------------------

IDENT_RE = re.compile(r"^[A-Za-z0-9_]+$")


def safe_name(prefix, symbol):
    """A Bison identifier for a user symbol; the prefix keeps terminals and non-terminals apart."""
    if IDENT_RE.match(symbol):
        return f"{prefix}{symbol}"
    return prefix + "x" + "".join(f"{ord(ch):x}" for ch in symbol)


def terminal_name(symbol):
    """Single ASCII punctuation becomes a char literal ('+'); anything else a %token."""
    if len(symbol) == 1 and symbol.isascii() and symbol.isprintable() \
            and not symbol.isalnum() and symbol not in "'\\":
        return f"'{symbol}'"
    return safe_name("tk_", symbol)


def write_bison_grammar(grammar, source_name, title, dest):
    """Translate the engine's parsed grammar into a plain Bison grammar file."""
    nonterms = set(grammar["nonterminals"])
    augmented_start = grammar["start"]
    tokens = [t for t in grammar["terminals"] if not terminal_name(t).startswith("'")]

    def sym(s):
        return safe_name("nt_", s) if s in nonterms else terminal_name(s)

    lines = [
        f"/* Generated by tests/bison/compare.py from examples/{source_name}",
        f"   ({title}). Do not edit: it is rewritten on every run.",
        "   No %left/%right/%prec and no %expect, so every conflict is reported raw.",
        "   Symbol names: nt_X is our non-terminal X, tk_x is our terminal x,",
        "   single punctuation stays a char literal. Bison adds $accept: S $end. */",
        "",
    ]
    if tokens:
        lines.append("%token " + " ".join(terminal_name(t) for t in tokens))
    lines.append(f"%start {sym(grammar['originalStart'])}")
    lines += ["", "%%", ""]

    # Group by left-hand side, in the engine's production order, skipping the
    # augmented S' -> S: Bison makes its own.
    order, by_lhs = [], {}
    for p in grammar["productions"]:
        if p["lhs"] == augmented_start:
            continue
        if p["lhs"] not in by_lhs:
            order.append(p["lhs"])
            by_lhs[p["lhs"]] = []
        by_lhs[p["lhs"]].append(p["rhs"])
    for lhs in order:
        alts = [" ".join(sym(s) for s in rhs) if rhs else "%empty" for rhs in by_lhs[lhs]]
        lines.append(f"{sym(lhs)}")
        lines.append(f"  : {alts[0]}")
        for alt in alts[1:]:
            lines.append(f"  | {alt}")
        lines.append("  ;")
        lines.append("")
    dest.write_text("\n".join(lines), encoding="utf-8")


# --------------------------------------------------------------------------
# The Bison side
# --------------------------------------------------------------------------

STATE_HEADER_RE = re.compile(r"^State (\d+)$", re.M)
STATE_CONFLICT_RE = re.compile(r"^State (\d+) conflicts?: (.*)$", re.M)
SR_RE = re.compile(r"(\d+) shift/reduce")
RR_RE = re.compile(r"(\d+) reduce/reduce")


def run_bison(y_path, lr_type, workdir):
    """Run bison -v under one lr.type; return its numbers and the raw .output."""
    tag = lr_type.replace("-", "_")
    output = workdir / f"{y_path.stem}.{tag}.output"
    proc = subprocess.run(
        ["bison", "-v", f"-Dlr.type={lr_type}", f"--report-file={output}",
         "-o", str(workdir / f"{y_path.stem}.{tag}.tab.c"), str(y_path)],
        capture_output=True, text=True, check=False,
    )
    if proc.returncode != 0 or not output.exists():
        raise SystemExit(f"bison failed on {y_path.name} ({lr_type}):\n{proc.stderr}")
    report = output.read_text(encoding="utf-8")
    per_state = []
    for _state, desc in STATE_CONFLICT_RE.findall(report):
        sr = SR_RE.search(desc)
        rr = RR_RE.search(desc)
        per_state.append((int(sr.group(1)) if sr else 0, int(rr.group(1)) if rr else 0))
    # Bison's stderr summary is a second, independent reading of the totals.
    err_sr = SR_RE.search(proc.stderr)
    err_rr = RR_RE.search(proc.stderr)
    return {
        "states": len(STATE_HEADER_RE.findall(report)),
        "sr": sum(s for s, _ in per_state),
        "rr": sum(r for _, r in per_state),
        "per_state": sorted(per_state),
        "stderr_sr": int(err_sr.group(1)) if err_sr else 0,
        "stderr_rr": int(err_rr.group(1)) if err_rr else 0,
        "report": report,
        "report_path": output,
    }


def conflict_excerpt(report, limit=60):
    """The summary lines plus every conflicted state's block, for the loud failure."""
    keep = [l for l in report.splitlines()[:40] if l.startswith("State ") and "conflict" in l]
    conflicted = {int(n) for n, _ in STATE_CONFLICT_RE.findall(report)}
    blocks = re.split(r"(?m)^(?=State \d+$)", report)
    for b in blocks:
        m = re.match(r"State (\d+)$", b.split("\n", 1)[0])
        if m and int(m.group(1)) in conflicted:
            keep.append("")
            keep.extend(b.rstrip().splitlines())
    if not keep:
        keep.append(f"(no conflicted states; {len(STATE_HEADER_RE.findall(report))} states in all)")
    return "\n".join(keep[:limit])


# --------------------------------------------------------------------------
# Comparison
# --------------------------------------------------------------------------

def fmt_ours(n):
    if not n["cells"]:
        return f"{n['states']} st, 0"
    parts = []
    if n["sr_cells"]:
        parts.append(f"{n['sr_cells']} S/R")
    if n["rr_cells"]:
        parts.append(f"{n['rr_cells']} R/R")
    return f"{n['states']} st, {n['cells']} ({', '.join(parts)})"


def fmt_bison(b):
    total = b["sr"] + b["rr"]
    if not total:
        return f"{b['states']} st, 0"
    parts = []
    if b["sr"]:
        parts.append(f"{b['sr']} S/R")
    if b["rr"]:
        parts.append(f"{b['rr']} R/R")
    return f"{b['states']} st, {total} ({', '.join(parts)})"


def compare(ours, bison):
    """Problems with one (ours, bison) pair, after the documented adjustments."""
    problems = []
    offset = bison["states"] - ours["states"]
    if offset != EXPECTED_STATE_OFFSET:
        problems.append(f"states: ours {ours['states']}, bison {bison['states']} "
                        f"(offset {offset:+d}, expected +{EXPECTED_STATE_OFFSET})")
    if (bison["sr"], bison["rr"]) != (ours["bison_sr"], ours["bison_rr"]):
        problems.append(f"conflicts: ours translated to Bison's counting = "
                        f"{ours['bison_sr']} S/R + {ours['bison_rr']} R/R, "
                        f"bison = {bison['sr']} S/R + {bison['rr']} R/R")
    elif bison["per_state"] != ours["per_state"]:
        problems.append(f"same totals but a different per-state distribution: "
                        f"ours {ours['per_state']}, bison {bison['per_state']}")
    if (bison["stderr_sr"], bison["stderr_rr"]) != (bison["sr"], bison["rr"]):
        problems.append(f"bison's stderr summary ({bison['stderr_sr']} S/R, "
                        f"{bison['stderr_rr']} R/R) differs from its own .output")
    return problems, offset


def main():
    if not shutil.which("bison"):
        print('bison not found on PATH - see README.md, "Running from source".', file=sys.stderr)
        return 2
    if not shutil.which("cargo"):
        print('cargo not found on PATH - see README.md, "Running from source".', file=sys.stderr)
        return 2

    bison_version = subprocess.run(["bison", "--version"], capture_output=True,
                                   text=True, check=False).stdout.splitlines()[0]
    dump = build_dump()
    GRAMMAR_DIR.mkdir(exist_ok=True)
    workdir = Path(tempfile.mkdtemp(prefix="lrv-bison-"))

    # The presets, then the probe, which checks the three-action cell no preset reaches.
    probe = workdir / PROBE_NAME
    probe.write_text(PROBE_GRAMMAR, encoding="utf-8")
    sources = [(path, preset_title(path), handout_numbers(path))
               for path in sorted(EXAMPLES.glob("*.txt"))]
    sources.append((probe, PROBE_TITLE, None))

    rows, failures, handout_drift, notes, offsets = [], [], [], [], set()
    for path, title, handout in sources:
        y_path = GRAMMAR_DIR / f"{path.stem}.y"
        row = {"preset": title, "file": path.name, "problems": []}
        grammar_written = False
        for variant, label, lr_type, blabel in MODES:
            grammar, table = run_engine(dump, path, variant)
            if not grammar_written:
                write_bison_grammar(grammar, path.name, title, y_path)
                grammar_written = True
            ours = our_numbers(table)
            if ours["note"]:
                notes.append(f"{path.name} {label}: {ours['note']}")
            if handout is not None and handout.get(label) != (ours["cells"], ours["states"]):
                handout_drift.append(f"{path.name} {label}: handout says {handout.get(label)}, "
                                     f"engine says ({ours['cells']}, {ours['states']})")
            bison = run_bison(y_path, lr_type, workdir)
            problems, offset = compare(ours, bison)
            offsets.add(offset)
            row[variant] = ours
            row[lr_type] = bison
            for p in problems:
                row["problems"].append(f"{label} vs Bison {blabel}: {p}")
            if problems:
                failures.append((path.name, label, blabel, problems, bison))
        rows.append(row)

    # ---- the table, on the terminal and as markdown -----------------------
    header = ["preset", "ours LALR(1)", "bison LALR", "ours CLR(1)", "bison canonical-LR", "verdict"]
    body = []
    for r in rows:
        verdict = "agree" if not r["problems"] else "DISAGREE"
        body.append([r["preset"], fmt_ours(r["lalr1"]), fmt_bison(r["lalr"]),
                     fmt_ours(r["clr1"]), fmt_bison(r["canonical-lr"]), verdict])
    widths = [max(len(str(x)) for x in col) for col in zip(header, *body)]
    line = lambda cells: " | ".join(str(c).ljust(w) for c, w in zip(cells, widths))
    print(f"Engine vs {bison_version}  (st = states; conflicts as each tool counts them)\n")
    print(line(header))
    print("-+-".join("-" * w for w in widths))
    for b in body:
        print(line(b))
    print()
    print(f"State offset (bison - ours) observed across all runs: "
          f"{sorted(offsets)}; expected exactly +{EXPECTED_STATE_OFFSET}.")

    if notes:
        print("\nEngine observations (not Bison disagreements):")
        for n in notes:
            print("  - " + n)

    if handout_drift:
        print("\n" + LOUD)
        print("HANDOUT DRIFT - examples/*.txt disagrees with the engine:")
        for d in handout_drift:
            print("  " + d)
        print(LOUD)

    for name, label, blabel, problems, bison in failures:
        print("\n" + LOUD)
        print(f"DISAGREEMENT: {name}, {label} vs Bison {blabel}")
        for p in problems:
            print("  - " + p)
        print(f"  raw report: {bison['report_path']}")
        print("  excerpt:")
        for l in conflict_excerpt(bison["report"]).splitlines():
            print("    " + l)
        print(LOUD)

    ok = not failures and not handout_drift
    write_markdown(rows, bison_version, sorted(offsets), handout_drift, notes)
    if ok:
        shutil.rmtree(workdir, ignore_errors=True)
        kept = ""
    else:
        kept = f" Bison reports kept in {workdir}."
    print(f"\nWrote {RESULTS_MD.relative_to(ROOT)} and {len(rows)} grammars "
          f"in {GRAMMAR_DIR.relative_to(ROOT)}/.{kept}")
    print("RESULT: " + ("all presets agree after the documented adjustments."
                        if ok else "UNEXPLAINED DISAGREEMENT - see above."))
    return 0 if ok else 1


def write_markdown(rows, bison_version, offsets, handout_drift, notes):
    md = [
        "# Engine vs GNU Bison",
        "",
        "Generated by `tests/bison/compare.py` - do not edit by hand. "
        f"Checked against {bison_version}.",
        "",
        "`st` is the number of states. Conflicts are shown as each tool reports "
        "them: ours as the engine's verdict counts them, Bison per token per "
        "extra action (rule 2 below). The last row is not a preset; see rule 2.",
        "",
        "| preset | ours LALR(1) | bison LALR | ours CLR(1) | bison canonical-LR | verdict |",
        "| --- | --- | --- | --- | --- | --- |",
    ]
    for r in rows:
        verdict = "agree" if not r["problems"] else "**DISAGREE**"
        md.append(f"| {r['preset']} | {fmt_ours(r['lalr1'])} | {fmt_bison(r['lalr'])} | "
                  f"{fmt_ours(r['clr1'])} | {fmt_bison(r['canonical-lr'])} | {verdict} |")
    md.append("")
    problems = [(r["file"], p) for r in rows for p in r["problems"]]
    if problems or handout_drift:
        md += ["## Unexplained disagreements", ""]
        md += [f"- `{f}`: {p}" for f, p in problems]
        md += [f"- handout drift: {d}" for d in handout_drift]
        md.append("")
    else:
        md += ["Every preset agrees with Bison, under both methods, once the "
               "three rules below are applied, and so does the probe. No rule is "
               "specific to a preset. Every engine number above also matches the "
               "`conflicts/states:` line of its handout.", ""]
    if notes:
        md += ["## Engine observations", "",
               "Not disagreements with Bison - the translated numbers agree - but "
               "worth knowing:", ""]
        md += [f"- {n}" for n in notes]
        md += ["",
               "On a cell with three actions the engine lists one conflict per "
               "colliding action, not one per cell, and labels both "
               "`shift-reduce`. This tool counts conflicts per colliding action, "
               "as the engine's verdict does. For all thirteen presets that is "
               "also the number of conflicted cells, because none of them, under "
               "any variant, has a cell with more than two actions.",
               ""]
    md += [
        "## How Bison counts differently",
        "",
        "Each rule is applied identically to every row; a difference that "
        "needs anything else is reported as a disagreement.",
        "",
        "1. **One extra state, always.** Bison augments with "
        "`$accept: S $end` and actually *shifts* `$end` into a final state "
        "`$accept: S $end .`, where it accepts. We augment with `S' -> S` and "
        "write `accept` into the `$` column of the state holding `S' -> S .`, "
        "so that last state does not exist for us. Bison's count is therefore "
        f"ours + 1. Offsets observed over all {2 * len(rows)} runs: "
        f"{', '.join(f'{o:+d}' for o in offsets)}.",
        "2. **Conflicts are counted per token per extra action.** Bison, per "
        "state, counts one shift/reduce for each token on which a shift meets "
        "at least one reduction, and `n - 1` reduce/reduce for each token on "
        "which `n >= 2` reductions meet. A cell holding one shift and one "
        "reduction, or two reductions, is one conflict either way, and that is "
        "every conflicted cell in every preset. The two ways of counting can "
        "only part on a cell with three or more actions, which no preset has, "
        "so the probe row builds one: shift + reduce + reduce on `a` is 1 S/R "
        "+ 1 R/R for Bison. The script does not compare the engine's own count "
        "with Bison's; it translates the engine's ACTION table cell by cell "
        "with exactly this rule and compares totals *and* the per-state "
        "distribution. Our `accept` is treated as Bison's shift of `$end`.",
        "3. **State numbering differs.** Bison numbers its states in its own "
        "order, not ours (ours follow the order in which symbols first appear in the grammar), so conflicts are compared as a "
        "multiset of per-state `(S/R, R/R)` pairs rather than by state number.",
        "",
        "Two things that look like they might matter, and do not:",
        "",
        "- **Default resolution.** Bison resolves an unresolved shift/reduce in "
        "favour of the shift and a reduce/reduce in favour of the rule written "
        "first. That decides what the generated parser does; it does not change "
        "the number of conflicts reported, which is all that is compared here. "
        "Our table keeps every colliding action in the cell instead.",
        "- **Default reductions.** `lr.default-reduction` is `most` for LALR "
        "and `accepting` for canonical LR (Bison manual, *Default Reductions*). "
        "Default reductions are chosen after conflicts are counted and only "
        "compress the table, so neither the state count nor the conflict count "
        "moves. The manual describes `canonical-lr` as the canonical LR tables "
        "of Knuth 1965, and the +1 offset holding on every CLR(1) row is "
        "consistent with that.",
        "",
        "The grammars Bison was given are in `grammars/`, one per preset plus "
        "the probe, generated from the engine's own parse of `examples/*.txt`.",
        "",
    ]
    RESULTS_MD.write_text("\n".join(md), encoding="utf-8")


if __name__ == "__main__":
    sys.exit(main())

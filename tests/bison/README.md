# Bison cross-check

An independent check of the engine's LALR(1) and CLR(1) numbers against GNU
Bison 3.8, for all thirteen presets in `examples/`.

```
python3 tests/bison/compare.py
```

Standard library only. Exit status: `0` every preset agrees after the
documented adjustments, `1` any unexplained disagreement (printed loudly, with
the raw `.output` excerpt), `2` `bison` or `cargo` is not on `PATH`.

## What it does

1. Builds the engine's `dump` binary and runs it on each `examples/*.txt` under
   `lalr1` and `clr1`. The grammar comes from the engine's own parse, so Bison
   sees exactly what the engine read; the state count and every ACTION cell
   come from the engine's table.
2. Re-checks those numbers against the handout's `conflicts/states:` line.
3. Writes a plain Bison grammar per preset to `grammars/` (no precedence, no
   `%expect`, so conflicts are raw). `nt_X` is our non-terminal `X`, `tk_x` our
   terminal `x`, single punctuation stays a char literal, ε is `%empty`.
4. Runs `bison -v` with `lr.type=lalr` and `lr.type=canonical-lr`, and reads
   the state count and the `State N conflicts:` lines out of the report.
5. Compares, and writes `results.md`.

It also runs one grammar that is not a preset (the probe row), built to put
three actions in one cell, because that is the only place the engine's and
Bison's ways of counting conflicts can part, and no preset reaches it.

## The quirks, in short

`results.md` explains each in full. They are applied to every row alike;
nothing is special-cased per preset.

- **Bison has exactly one more state.** It shifts `$end` into
  `$accept: S $end .`; we accept in place in the state holding `S' -> S .`.
- **Bison counts conflicts per token per extra action.** One S/R per token
  where a shift meets any reduction, `n - 1` R/R per token where `n`
  reductions meet. The engine counts one conflict per colliding action, which is
  one per cell whenever a cell holds exactly two actions - every preset - so the
  two counts agree. The script translates our table cell by cell with Bison's
  rule rather than comparing headline numbers.
- **State numbers differ,** so per-state conflicts are compared as a multiset.
- Bison's default resolution (shift wins, earlier rule wins) and default
  reductions change neither count.

The script never changes the engine. If Bison disagrees, it reports it and
exits 1; deciding who is right is a human's job.

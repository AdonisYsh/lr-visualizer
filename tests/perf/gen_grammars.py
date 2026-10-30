#!/usr/bin/env python3
"""Write the generated half of the performance corpus (gen-*.txt).

    python3 tests/perf/gen_grammars.py           write grammars/gen-*.txt
    python3 tests/perf/gen_grammars.py --check   verify they are current

Layered-precedence expression grammar: k levels of left-recursive binary
operators over a parenthesised atom. Deterministic, standard library only.
"""

import pathlib
import sys

# Production counts of the files to write. Each must be even and at least 4.
SIZES = [10, 20, 40, 80]

OUT_DIR = pathlib.Path(__file__).resolve().parent / "grammars"


def grammar_text(productions: int) -> str:
    """The whole file, header included, for a grammar with this many productions."""
    if productions < 4 or productions % 2 != 0:
        raise ValueError(f"{productions}: the family only has even sizes from 4 up")
    levels = (productions - 2) // 2

    # Two digits keep the names aligned and sorted.
    def level(i: int) -> str:
        return f"E{i:02d}"

    rules = []
    for i in range(1, levels + 1):
        tighter = level(i + 1) if i < levels else "atom"
        rules.append(f"{level(i)} -> {level(i)} op{i:02d} {tighter} | {tighter}")
    rules.append(f"atom -> ( {level(1)} ) | id")

    header = [
        "// GENERATED FILE - do not edit by hand.",
        "// Written by tests/perf/gen_grammars.py, which is deterministic: run it",
        "// again and this file comes out byte for byte the same.",
        "//",
        f"// Layered-precedence expression grammar: {levels} levels of left-recursive",
        "// binary operators over a parenthesised atom. Level 1 binds loosest.",
        "//",
        f"// productions: {productions}  (after `|` expansion, not counting the augmented",
        f"//                  {level(1)}' -> {level(1)})",
        f"// non-terminals: {levels + 1}   terminals: {levels + 3}",
        "",
    ]
    return "\n".join(header + rules) + "\n"


def main() -> int:
    check = "--check" in sys.argv[1:]
    unknown = [a for a in sys.argv[1:] if a != "--check"]
    if unknown:
        print(f"unknown option: {unknown[0]}", file=sys.stderr)
        return 2

    stale = 0
    for productions in SIZES:
        path = OUT_DIR / f"gen-{productions:03d}.txt"
        # Bytes, not text, so newline translation cannot change the output.
        wanted = grammar_text(productions).encode("utf-8")
        if check:
            current = path.read_bytes() if path.exists() else None
            ok = current == wanted
            stale += 0 if ok else 1
            print(f"{'ok   ' if ok else 'STALE'}  {path.name}")
        else:
            OUT_DIR.mkdir(parents=True, exist_ok=True)
            path.write_bytes(wanted)
            print(f"wrote  {path.name}  ({productions} productions)")
    return 1 if stale else 0


if __name__ == "__main__":
    sys.exit(main())

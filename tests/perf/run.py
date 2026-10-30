#!/usr/bin/env python3
"""The performance numbers in one command: native bench + browser bench -> one CSV.

    python3 tests/perf/run.py                  everything  -> tests/perf/out/performance.csv
    python3 tests/perf/run.py --out FILE       everything  -> FILE
    python3 tests/perf/run.py --only gen-020   one grammar -> tests/perf/out/performance-partial.csv
    python3 tests/perf/run.py --browsers chromium | --runs 3 | --skip-native | --skip-browser | --no-build

Builds, runs both benches, joins and checks their CSVs, then writes the result
in one rename; only a full run replaces the full result. Every child has a
deadline and is killed with its process group.
"""

import argparse
import collections
import csv
import os
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

sys.dont_write_bytecode = True  # leave no __pycache__ behind in the tree
ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(Path(__file__).resolve().parent))

import render_bench  # noqa: E402 - the path tweak above must come first
from render_bench import HEADER, COLUMNS  # noqa: E402

# ------------------------------------------------------------------ constants

OUT_DIR = ROOT / 'tests' / 'perf' / 'out'
DELIVERABLE = OUT_DIR / 'performance.csv'
NATIVE_CSV = OUT_DIR / 'native.csv'
BROWSER_CSV = OUT_DIR / 'browser.csv'
PARTIAL_CSV = OUT_DIR / 'performance-partial.csv'

NATIVE_ENVIRONMENT = 'native-release'
NATIVE_METRICS = ('analyze', 'automaton')
# The native bench is pinned to CPU 0, which exists everywhere.
PINNED_CPU = '0'
# Linux lists a hybrid CPU's performance cores here; absent otherwise.
PERFORMANCE_CORES = Path('/sys/devices/cpu_core/cpus')

# Deadlines for whole child processes, in seconds: they only stop a hang.
BUILD_DEADLINE = 30 * 60
NATIVE_DEADLINE = 45 * 60
BROWSER_DEADLINE = 90 * 60

# Columns where a browser row must match the native `analyze` row.
SHARED_COUNTS = ('productions', 'terminals', 'nonterminals', 'states', 'steps', 'conflicts')


def say(text):
    print('\n==> ' + text, file=sys.stderr, flush=True)


def run_child(command, cwd, deadline):
    """Run a child; returns its exit code, or None if killed at the deadline.

    The whole process group is killed so cargo/Playwright children do not linger."""
    print('    $ ' + ' '.join(str(c) for c in command), file=sys.stderr, flush=True)
    child = subprocess.Popen(command, cwd=str(cwd), start_new_session=True)
    try:
        return child.wait(timeout=deadline)
    except subprocess.TimeoutExpired:
        print('    killed: still running after %d s' % deadline, file=sys.stderr, flush=True)
        kill_group(child)
        return None
    except KeyboardInterrupt:
        kill_group(child)
        raise


def kill_group(child):
    try:
        os.killpg(child.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    child.wait()


# ----------------------------------------------------------------- the steps


def build():
    """`./build.sh`; True if it succeeded."""
    say('Building dist/index.html')
    return run_child(['./build.sh'], ROOT, BUILD_DEADLINE) == 0


def native(opts):
    """The native bench, pinned to one core when `taskset` exists; True on success."""
    say('Native bench')
    engine = ROOT / 'engine'
    # Compile unpinned so the pinned run only measures.
    if run_child(['cargo', 'build', '--locked', '--release', '--bin', 'bench'], engine, BUILD_DEADLINE) != 0:
        return False
    command = ['cargo', 'run', '--locked', '--release', '--quiet', '--bin', 'bench', '--',
               '--out', str(NATIVE_CSV)]
    if opts.only:
        command += ['--only', opts.only]
    if opts.runs:
        command += ['--runs', str(opts.runs)]
    if shutil.which('taskset'):
        command = ['taskset', '-c', PINNED_CPU] + command
        print('    pinned to CPU %s with taskset' % PINNED_CPU, file=sys.stderr)
    else:
        print('    taskset not found: running unpinned, expect about 10 %% run-to-run spread',
              file=sys.stderr)
    return run_child(command, engine, NATIVE_DEADLINE) == 0


def performance_cores():
    """The CPU list of the performance cores, e.g. '0-11', or None (non-hybrid CPU)."""
    try:
        cpus = PERFORMANCE_CORES.read_text(encoding='ascii').strip()
    except OSError:
        return None
    return cpus or None


def browser(opts):
    """The browser bench; returns its exit code (None if it was killed)."""
    say('Browser bench')
    command = [sys.executable, str(Path(__file__).resolve().parent / 'render_bench.py'),
               '--out', str(BROWSER_CSV), '--browsers', ','.join(opts.browsers)]
    if opts.only:
        command += ['--only', opts.only]
    if opts.runs:
        command += ['--runs', str(opts.runs)]
    cores = performance_cores()
    if cores and shutil.which('taskset'):
        # Affinity is inherited by every browser process.
        command = ['taskset', '-c', cores] + command
        print('    confined to the performance cores (CPUs %s) with taskset' % cores,
              file=sys.stderr)
    else:
        print('    not confined to any cores: no performance/efficiency split reported, '
              'or no taskset', file=sys.stderr)
    return run_child(command, ROOT, BROWSER_DEADLINE)


# ---------------------------------------------------------------- the checks


def read_rows(path, problems):
    """(raw text after the header, parsed rows) of one bench's CSV; the raw text is copied verbatim."""
    text = path.read_text(encoding='utf-8')
    head, _, body = text.partition('\n')
    if head != HEADER:
        problems.append('%s does not start with the shared header' % path.name)
        return '', []
    rows = []
    for number, cells in enumerate(csv.reader(body.splitlines(keepends=True)), start=2):
        if len(cells) != len(COLUMNS):
            problems.append('%s line %d has %d fields, not %d'
                            % (path.name, number, len(cells), len(COLUMNS)))
            continue
        rows.append(dict(zip(COLUMNS, cells)))
    if body and not body.endswith('\n'):
        problems.append('%s does not end with a newline' % path.name)
    return body, rows


def key_of(row):
    return (row['grammar'], row['variant'], row['environment'], row['metric'])


def expected_keys(opts, entries):
    """Every row a run with these options must contain."""
    keys = []
    for entry in entries:
        for variant in render_bench.VARIANTS:
            if not opts.skip_native:
                keys += [(entry.stem, variant, NATIVE_ENVIRONMENT, m) for m in NATIVE_METRICS]
            if not opts.skip_browser:
                for name in opts.browsers:
                    keys += [(entry.stem, variant, name + '-wasm', m) for m in render_bench.METRICS]
    return keys


def check(rows, opts, entries):
    """Everything wrong with the joined rows, as a list of sentences."""
    problems = []
    seen = collections.Counter(key_of(r) for r in rows)
    for key, n in seen.items():
        if n > 1:
            problems.append('%d rows for %s' % (n, ' / '.join(key)))
    wanted = expected_keys(opts, entries)
    for key in wanted:
        if key not in seen:
            problems.append('no row for %s' % ' / '.join(key))
    for key in seen:
        if key not in set(wanted):
            problems.append('unexpected row %s' % ' / '.join(key))

    # The browser must agree with the native engine on the same grammar.
    reference = {(r['grammar'], r['variant']): r for r in rows
                 if r['environment'] == NATIVE_ENVIRONMENT and r['metric'] == 'analyze'
                 and r['outcome'] == 'ok'}
    for row in rows:
        ref = reference.get((row['grammar'], row['variant']))
        if ref is None or row['environment'] == NATIVE_ENVIRONMENT:
            continue
        for column in SHARED_COUNTS:
            if row[column] != '' and row[column] != ref[column]:
                problems.append('%s: %s is %s in the browser and %s natively'
                                % (' / '.join(key_of(row)), column, row[column], ref[column]))
        size = detail_value(row['detail'], 'result_bytes')
        if size is not None and size != ref['json_bytes']:
            problems.append('%s: the JSON is %s bytes in the browser and %s natively'
                            % (' / '.join(key_of(row)), size, ref['json_bytes']))
    return problems


def detail_value(detail, name):
    """The value of `name=` in a `;`-separated detail field, or None."""
    for part in detail.split(';'):
        if part.startswith(name + '='):
            return part[len(name) + 1:]
    return None


def summarise(rows):
    """Row counts per environment x metric x outcome, and every row that is not ok."""
    counts = collections.Counter((r['environment'], r['metric'], r['outcome']) for r in rows)
    print('\nrows  environment      metric               outcome', file=sys.stderr)
    for (environment, metric, outcome), n in counts.items():
        print('%4d  %-15s  %-19s  %s' % (n, environment, metric, outcome), file=sys.stderr)
    print('%4d  in all' % len(rows), file=sys.stderr)
    for row in rows:
        if row['outcome'] != 'ok':
            print('  %s: %s - %s' % (' / '.join(key_of(row)), row['outcome'], row['detail']),
                  file=sys.stderr)


# ---------------------------------------------------------------------- main


def main(argv=None):
    """Run the benches, join and check their output; returns the exit code."""
    parser = argparse.ArgumentParser(
        description='Native bench + browser bench -> one CSV')
    parser.add_argument('--out', type=Path, default=DELIVERABLE,
                        help='where a full run writes its CSV (default: tests/perf/out/performance.csv)')
    parser.add_argument('--only', help='only grammars whose name contains this (a partial run)')
    parser.add_argument('--browsers', type=render_bench.parse_browsers,
                        default=list(render_bench.harness.BROWSERS),
                        help='comma-separated: chromium,firefox (default: both)')
    parser.add_argument('--runs', type=render_bench.positive,
                        help='timed runs per measurement, for both benches '
                             '(default: native 7, browser %d)' % render_bench.DEFAULT_RUNS)
    parser.add_argument('--skip-native', action='store_true', help='leave the native rows out')
    parser.add_argument('--skip-browser', action='store_true', help='leave the browser rows out')
    parser.add_argument('--no-build', action='store_true',
                        help='do not run ./build.sh first; measure the existing dist/index.html')
    opts = parser.parse_args(argv)
    if opts.skip_native and opts.skip_browser:
        parser.error('--skip-native and --skip-browser together leave nothing to do')
    if shutil.which('cargo') is None:
        print('cargo is not on PATH. Run this inside the dev shell: '
              'python3 tests/perf/run.py', file=sys.stderr)
        return 2
    entries = render_bench.corpus(opts.only)
    if not entries:
        print('--only matched no grammar in the corpus', file=sys.stderr)
        return 2

    full = not (opts.only or opts.skip_native or opts.skip_browser
                or opts.browsers != list(render_bench.harness.BROWSERS))
    target = opts.out.resolve() if full else PARTIAL_CSV
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    problems = []

    if not opts.skip_browser and not opts.no_build and not build():
        print('the build failed; nothing was measured', file=sys.stderr)
        return 1

    bodies, rows = [], []
    if not opts.skip_native:
        if not native(opts):
            print('the native bench failed; nothing was written', file=sys.stderr)
            return 1
        body, parsed = read_rows(NATIVE_CSV, problems)
        bodies.append(body)
        rows += parsed
    if not opts.skip_browser:
        code = browser(opts)
        if code is None or code not in (0, 1) or not BROWSER_CSV.is_file():
            print('the browser bench did not finish; nothing was written', file=sys.stderr)
            return 1
        if code == 1:
            problems.append('the browser bench reported a page doing something it must never '
                            'do (see its output above)')
        body, parsed = read_rows(BROWSER_CSV, problems)
        bodies.append(body)
        rows += parsed

    say('Checking')
    problems += check(rows, opts, entries)
    summarise(rows)
    print('\n%d rows in %.0f s' % (len(rows), time.monotonic() - started), file=sys.stderr)
    if problems:
        print('\nFAILED - %s was not written:' % os.path.relpath(target), file=sys.stderr)
        for line in problems:
            print('    ' + line, file=sys.stderr)
        return 1

    # One rename, so the file is never half written.
    target.parent.mkdir(parents=True, exist_ok=True)
    scratch = target.with_name(target.name + '.tmp')
    scratch.write_text(HEADER + '\n' + ''.join(bodies), encoding='utf-8')
    os.replace(scratch, target)
    print('wrote %s%s' % (os.path.relpath(target),
                          '' if full else '  (a partial run: the full result was not touched)'),
          file=sys.stderr)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())

"""Run every browser suite in this directory: each `test_*.py`, both browsers.

    python3 tests/browser/run.py [--browser chromium] [-k name] [--list]
"""

import importlib
import sys
from pathlib import Path

sys.dont_write_bytecode = True  # leave no __pycache__ behind in the tree
HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import harness  # noqa: E402 - the path tweak above must come first

if __name__ == '__main__':
    for suite in sorted(HERE.glob('test_*.py')):
        importlib.import_module(suite.stem)
    raise SystemExit(harness.main())

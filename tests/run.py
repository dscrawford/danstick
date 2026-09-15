"""Every test padmap has, in one command.

    nix develop --command python3 tests/run.py            # everything
    nix develop --command python3 tests/run.py --python   # just the suites here
    nix develop --command python3 tests/run.py --rust     # just cargo test
    nix develop --command python3 tests/run.py --coverage # and measure it

padmap's tests were hard to find, which is most of why this exists. They are
in three places and only one of them is a choice:

* `tests/check_*.py` -- the Python suites. Each is a standalone program that
  prints a line per assertion and exits non-zero on failure. No framework:
  they have to run against real device nodes, real sockets and a real daemon,
  and a runner that owned the process lifetime got in the way more than it
  helped.
* `tests/e2e_*.py` -- the same, but they stand up a daemon and drive it.
  Slower, and skipped unless asked for.
* `rust/crates/*/src/**.rs` and `rust/crates/*/tests/` -- cargo requires unit
  tests to live beside the code they test and integration tests in the
  crate's own `tests/`. Those cannot move here, so this runs them instead.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent


def suites(kind: str) -> list[Path]:
    return sorted(HERE.glob(f"{kind}_*.py"))


def run_python(paths: list[Path], verbose: bool) -> tuple[int, int, float]:
    passed = failed = 0
    started = time.monotonic()
    for path in paths:
        # A fresh runtime dir each time: several of these bind a socket or
        # write a daemon marker, and sharing one lets an earlier suite's
        # leftovers decide a later suite's result.
        env = dict(os.environ, XDG_RUNTIME_DIR=tempfile.mkdtemp())
        result = subprocess.run(
            [sys.executable, str(path)], env=env, cwd=REPO,
            capture_output=True, text=True, timeout=600)
        if result.returncode == 0:
            passed += 1
            print(f"  ok   {path.name}")
        else:
            failed += 1
            print(f"  FAIL {path.name}")
            tail = (result.stdout + result.stderr).strip().splitlines()
            for line in tail[-12:]:
                print(f"       {line}")
        if verbose and result.returncode == 0:
            for line in result.stdout.splitlines():
                if line.startswith("  ok"):
                    print(f"    {line.strip()}")
    return passed, failed, time.monotonic() - started


def run_rust(coverage: bool) -> bool:
    """`cargo test`, or `cargo llvm-cov` when a number is wanted.

    Coverage needs the llvm-tools that ship *with rustc*, in a separate
    nixpkgs output that is not on PATH. Resolved here rather than documented,
    because the failure without it -- "failed to find llvm-tools-preview" --
    sends the reader looking for a rustup component that a Nix build does not
    have and would not use.
    """
    cargo = ["cargo", "test", "--workspace"]
    env = dict(os.environ, CARGO_HOME=os.environ.get(
        "CARGO_HOME", str(REPO / ".cargo-home")))
    if coverage:
        tools = subprocess.run(
            ["nix", "build", "--no-link", "--print-out-paths",
             "nixpkgs#llvmPackages.bintools-unwrapped"],
            capture_output=True, text=True)
        if tools.returncode == 0:
            prefix = tools.stdout.strip().splitlines()[-1]
            env["LLVM_COV"] = f"{prefix}/bin/llvm-cov"
            env["LLVM_PROFDATA"] = f"{prefix}/bin/llvm-profdata"
        cargo = ["cargo", "llvm-cov", "--workspace", "--summary-only"]
    result = subprocess.run(cargo, cwd=REPO / "rust", env=env)
    return result.returncode == 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--python", action="store_true",
                        help="only the suites in tests/")
    parser.add_argument("--rust", action="store_true", help="only cargo test")
    parser.add_argument("--e2e", action="store_true",
                        help="also run tests/e2e_*.py, which start a daemon")
    parser.add_argument("--coverage", action="store_true",
                        help="measure Rust coverage instead of just running")
    parser.add_argument("-v", "--verbose", action="store_true",
                        help="print every assertion, not just every suite")
    args = parser.parse_args()

    both = not (args.python or args.rust)
    failures = 0

    if args.python or both:
        paths = suites("check")
        if args.e2e:
            paths += suites("e2e")
        print(f"== {len(paths)} Python suite(s)")
        passed, failed, seconds = run_python(paths, args.verbose)
        print(f"   {passed} passed, {failed} failed, {seconds:.1f}s")
        failures += failed

    if args.rust or both:
        print("== Rust")
        if not run_rust(args.coverage):
            failures += 1

    print("\nEverything passed." if not failures
          else f"\n{failures} failure(s).")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())

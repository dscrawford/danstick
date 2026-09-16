"""Every test padmap has, in one command.

    nix develop --command python3 tests/run.py            # everything
    nix develop --command python3 tests/run.py --coverage # and measure it

padmap is one Rust workspace, so this is a thin wrapper over `cargo test` --
kept because "how do I run the tests" should have one answer that does not
depend on knowing where cargo wants to be invoked from, and because the
coverage run needs flags nobody remembers.

The tests themselves live beside the code they test (cargo requires unit tests
there) and in each crate's `tests/` (cargo requires integration tests there),
so there is nothing for this file to collect.
"""

from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
RUST = REPO / "rust"


def run(args: list[str]) -> int:
    print(f"$ {' '.join(args)}\n", flush=True)
    return subprocess.run(args, cwd=RUST).returncode


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--coverage", action="store_true",
                        help="measure line and region coverage")
    parser.add_argument("--lint", action="store_true",
                        help="also run rustfmt and clippy")
    args = parser.parse_args()

    if args.coverage:
        # The e2e tests create uinput devices and drive a real daemon; they
        # are part of the number, so they are not excluded here.
        return run(["cargo", "llvm-cov", "--workspace", "--summary-only"])

    failed = run(["cargo", "test", "--workspace"])
    if args.lint and not failed:
        failed = run(["cargo", "fmt", "--all", "--check"]) or run(
            ["cargo", "clippy", "--all-targets", "--", "-D", "warnings"])
    print("\nEverything passed." if not failed else "\nSomething failed.")
    return failed


if __name__ == "__main__":
    sys.exit(main())

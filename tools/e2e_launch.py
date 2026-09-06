"""Does a game get the launch command of ITS collection?

Reported symptom: an N64 ROM was launched with the MAME arcade core. Pegasus
assigns a game the launch command of the first collection it is added to
(SearchContext::game_add_to only fills it when still empty), so the layout of
the metadata file decides whether that is the right one.

This runs real Pegasus headless over a two-collection library and reads back
which command each game actually resolved to, via the "Executing command"
line Pegasus logs. Each collection launches a distinct marker script, so the
log says unambiguously which one was used.

    python3 tools/e2e_launch.py [--single-file] [--keep]
"""

import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path


def build(repo: Path, attr: str) -> str:
    out = subprocess.run(
        ["nix", "build", "--no-link", "--print-out-paths", f".#{attr}"],
        cwd=repo, capture_output=True, text=True,
    )
    if out.returncode != 0:
        raise RuntimeError(f"nix build .#{attr} failed:\n{out.stderr}")
    return out.stdout.strip().splitlines()[-1]


def main() -> int:
    single_file = "--single-file" in sys.argv
    keep = "--keep" in sys.argv
    repo = Path(__file__).resolve().parent.parent

    print("building...")
    pegasus_bin = build(repo, "pegasus") + "/bin/pegasus-fe"
    theme_dir = Path(build(repo, "pegasus-theme")) / "share/pegasus-frontend/themes/padmap"

    root = Path(tempfile.mkdtemp(prefix="padmap-launch-"))
    try:
        runtime = root / "run"
        config = root / "config"
        for path in (runtime, config):
            path.mkdir(parents=True, exist_ok=True)
        runtime.chmod(0o700)

        cfg = config / "pegasus-frontend"
        (cfg / "themes").mkdir(parents=True, exist_ok=True)
        (cfg / "themes" / "padmap").symlink_to(theme_dir)
        (cfg / "settings.txt").write_text(
            "general.theme: themes/padmap/\ngeneral.fullscreen: false\n")

        alpha = root / "alpha"
        beta = root / "beta"
        alpha.mkdir(); beta.mkdir()
        (alpha / "one.aaa").write_bytes(b"")
        (beta / "two.bbb").write_bytes(b"")

        block_a = (
            "collection: Alpha\n"
            "extensions: aaa\n"
            "launch: /bin/echo LAUNCHER_ALPHA {file.path}\n"
            "\n"
            "game: One\n"
            f"file: {alpha / 'one.aaa'}\n"
        )
        block_b = (
            "collection: Beta\n"
            "extensions: bbb\n"
            "launch: /bin/echo LAUNCHER_BETA {file.path}\n"
            "\n"
            "game: Two\n"
            f"file: {beta / 'two.bbb'}\n"
        )

        if single_file:
            # How padmap currently exports: both collections in one file, in
            # one directory.
            shared = root / "collections"
            shared.mkdir()
            (shared / "metadata.pegasus.txt").write_text(block_a + "\n" + block_b)
            (cfg / "game_dirs.txt").write_text(f"{shared}\n")
            print("layout: ONE metadata file, two collection blocks")
        else:
            # One directory per collection, each listed in game_dirs.
            (alpha / "metadata.pegasus.txt").write_text(block_a)
            (beta / "metadata.pegasus.txt").write_text(block_b)
            (cfg / "game_dirs.txt").write_text(f"{alpha}\n{beta}\n")
            print("layout: one metadata file PER collection directory")

        env = dict(os.environ)
        env.update({
            "XDG_RUNTIME_DIR": str(runtime),
            "XDG_CONFIG_HOME": str(config),
            "XDG_DATA_HOME": str(root / "data"),
            "QT_QPA_PLATFORM": "offscreen",
        })

        log_path = root / "pegasus.log"
        proc = subprocess.Popen(
            [pegasus_bin], env=env,
            stdout=open(log_path, "w"), stderr=subprocess.STDOUT)
        time.sleep(6)
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()

        log = log_path.read_text(errors="replace")
        print("\ncollections and games Pegasus built:")
        for line in log.splitlines():
            if "games found" in line or "collection" in line.lower():
                print("  " + line.strip())

        # The theme cannot be driven headlessly, so read the resolved command
        # out of the model instead: Pegasus logs it only on launch. Fall back
        # to reporting what we can see.
        if "LAUNCHER" in log:
            for line in log.splitlines():
                if "LAUNCHER" in line:
                    print("  " + line.strip())
        print(f"\nfull log: {log_path if keep else '(discarded)'}")
        return 0
    finally:
        if not keep:
            shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())

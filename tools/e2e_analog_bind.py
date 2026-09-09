"""Does a BUTTON bound to an analog half-axis reach a core as deflection?

padmap puts the N64 C-buttons on the right analog stick, because that is where
mupen64plus-next reads them from. So mapping a face button to C-up produces

    input_r_y_minus_btn = "3"

a button bound to one half of an analog axis. Reported: doing exactly that
"didn't map to the c button". Everything on padmap's side was verified correct
first -- the capture is stored, the autoconfig carries the line, the button
index matches RetroArch's own numbering for the pad, and RetroArch's shipped
database uses this form in 40 profiles -- so the only remaining question is
what the running RetroArch hands the core.

This asks it. A synthetic pad, a generated autoconfig binding one button to
r_y_minus, real RetroArch on a virtual display, the press injected through
uinput, and a probe core reporting what it was given on both paths:
RETRO_DEVICE_ANALOG (which is the only way mupen sees a C-button) and
RETRO_DEVICE_JOYPAD (so a run that saw nothing at all is distinguishable from
one where only the analog path was empty).

Reading RetroArch's source would answer a different question -- what some
version does -- and the build installed here is the one that matters.

Nothing here touches the live daemon or the real controllers: its pad is its
own uinput node, and every XDG directory is redirected.

ANSWERED, but not by this test -- see tools/check_half_axis_binds.py. RetroArch
does convert such a button into full deflection, and the reported failure was
narrower than "it doesn't work": in `input_joypad_analog_axis` the button is
only consulted when the axis reads *exactly* zero,

    if (res == 0) { ... consult bind_minus->joykey ... }

and the pad's C-stick rests at 131 on a 0..255 axis, which normalises to 900.
So `res` was never zero and the button was never read. That is now modelled
directly from the RetroArch source, which is both cheaper and sharper than this
harness: it names the rest value that breaks the bind, where a run here could
only have said yes or no.

STILL INCOMPLETE as a program. It does not get its pad bound -- RetroArch logs

    [Autoconf] padmap analog probe pad (4617/20816) not configured.

so the profile is never matched and the control check correctly refuses to
report a verdict rather than inventing one. That control check is the part
worth keeping: it distinguishes "the analog path is empty" from "nothing
reached the core at all", which is exactly the mistake this test exists to
avoid making.

Kept for the scaffolding -- probe core, synthetic pad, injected hold, real
RetroArch on Xvfb -- which is most of the work for the next question that only
the running binary can answer.

    python3 tools/e2e_analog_bind.py [--keep]
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

PAD_NAME = "padmap analog probe pad"
PAD_VID = 0x1209
PAD_PID = 0x5150
# Contiguous from BTN_JOYSTICK, so SDL and RetroArch number them identically
# and the index under test is unambiguous.
FIRST_BTN = 0x120
BUTTONS = 12
# The button the autoconfig binds to r_y_minus. 0x123 is index 3.
BOUND_INDEX = 3


def run(cmd, **kw):
    return subprocess.run(cmd, capture_output=True, text=True, **kw)


def build_headers() -> Path:
    out = run(["nix", "build", "nixpkgs#retroarch-bare.src",
               "--no-link", "--print-out-paths"], cwd=REPO)
    if out.returncode != 0:
        raise SystemExit(f"FAIL: could not fetch libretro.h:\n{out.stderr}")
    return Path(out.stdout.strip().splitlines()[-1])


def build_probe(dest: Path, headers: Path) -> Path:
    so = dest / "probe_analog.so"
    out = run(["nix", "shell", "nixpkgs#gcc", "--command",
               "gcc", "-shared", "-fPIC", "-O1", "-o", str(so),
               str(REPO / "tools" / "probe_analog.c"),
               f"-I{headers}/libretro-common/include"], cwd=REPO)
    if out.returncode != 0:
        raise SystemExit(f"FAIL: probe core did not build:\n{out.stderr}")
    return so


def make_pad():
    """A synthetic joypad we own, so no real controller is involved."""
    import evdev
    from evdev import AbsInfo, ecodes

    caps = {
        ecodes.EV_KEY: [FIRST_BTN + n for n in range(BUTTONS)],
        ecodes.EV_ABS: [
            (ecodes.ABS_X, AbsInfo(128, 0, 255, 0, 0, 0)),
            (ecodes.ABS_Y, AbsInfo(128, 0, 255, 0, 0, 0)),
            (ecodes.ABS_Z, AbsInfo(128, 0, 255, 0, 0, 0)),
            (ecodes.ABS_RZ, AbsInfo(128, 0, 255, 0, 0, 0)),
        ],
    }
    return evdev.UInput(events=caps, name=PAD_NAME, vendor=PAD_VID,
                        product=PAD_PID, version=0x0110, phys="analogprobe/0")


def autoconfig_for(directory: Path) -> Path:
    """One profile, binding a BUTTON to the negative half of the right Y axis.

    Exactly the shape padmap emits for "this face button is C-up".
    """
    driver = directory / "udev"
    driver.mkdir(parents=True, exist_ok=True)
    profile = driver / f"{PAD_NAME}.cfg"
    profile.write_text(
        f'input_driver = "udev"\n'
        f'input_device = "{PAD_NAME}"\n'
        f'input_vendor_id = "{PAD_VID}"\n'
        f'input_product_id = "{PAD_PID}"\n'
        # The line under test.
        f'input_r_y_minus_btn = "{BOUND_INDEX}"\n'
        # A plain button bind on the same pad, as a control: if this reaches
        # the core and the analog one does not, the difference is the analog
        # path and nothing else.
        f'input_a_btn = "{BOUND_INDEX}"\n'
    )
    return profile


def main() -> int:
    keep = "--keep" in sys.argv
    root = Path(tempfile.mkdtemp(prefix="padmap-analogbind-"))
    for name in ("XDG_RUNTIME_DIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME"):
        (root / name.lower()).mkdir(parents=True, exist_ok=True)
        os.environ[name] = str(root / name.lower())

    print("building the probe core...")
    probe = build_probe(root, build_headers())

    import evdev  # noqa: F401  -- imported after the env is redirected
    from padmap import retroarch

    pad = make_pad()
    time.sleep(0.6)   # let udev settle so RetroArch enumerates it

    try:
        node = pad.device.path
        order = retroarch.visible_order()
        index = next((i for i, path in order.items() if path == node), None)
        if index is None:
            raise SystemExit(
                "FAIL: the synthetic pad is not in RetroArch's enumeration, so "
                "nothing could be bound to it. Is it hidden by udev rules?")
        print(f"  pad {node} is RetroArch joypad index {index}")

        autoconfig = root / "autoconfig"
        autoconfig_for(autoconfig)

        config = root / "retroarch"
        config.mkdir(parents=True, exist_ok=True)
        # nul every player-1 bind, or a leftover in the config outranks the
        # autoconfig profile -- RetroArch only falls back to the profile when
        # the explicit bind is nul.
        binds = "\n".join(
            f'input_player1_{b}_btn = "nul"\ninput_player1_{b}_axis = "nul"'
            for b in retroarch.PLAYER_BINDS)
        (config / "retroarch.cfg").write_text(
            f'input_joypad_driver = "udev"\n'
            f'joypad_autoconfig_dir = "{autoconfig}"\n'
            f'input_player1_joypad_index = "{index}"\n'
            f'input_autodetect_enable = "true"\n'
            f'input_analog_deadzone = "0.000000"\n'
            f'input_analog_sensitivity = "1.000000"\n'
            f'video_driver = "gl"\n'
            f'audio_driver = "null"\n'
            f'menu_driver = "null"\n'
            f'config_save_on_exit = "false"\n'
            f'{binds}\n')

        rom = root / "probe.z64"
        rom.write_bytes(b"\x80\x37\x12\x40" + b"\0" * 1024)

        display = ":91"
        xvfb = subprocess.Popen(
            ["nix", "shell", "nixpkgs#xorg.xvfb", "--command",
             "Xvfb", display, "-screen", "0", "640x480x24"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(1.5)

        # Held down BEFORE RetroArch starts. With a null audio driver and no
        # vsync the core runs unthrottled, so its whole 240-frame run finished
        # inside the first second -- the earlier attempt began holding after
        # that and the probe correctly reported seeing nothing. evdev is level
        # triggered, so a button already down is simply down when the first
        # poll happens.
        from evdev import ecodes
        pad.write(ecodes.EV_KEY, FIRST_BTN + BOUND_INDEX, 1)
        pad.syn()
        time.sleep(0.3)

        env = dict(os.environ, DISPLAY=display)
        log = root / "retroarch.log"
        print("running RetroArch with the probe core...")
        with log.open("w") as handle:
            proc = subprocess.Popen(
                ["retroarch", "--config", str(config / "retroarch.cfg"),
                 "-L", str(probe), str(rom), "--verbose"],
                env=env, stdout=handle, stderr=subprocess.STDOUT)

            # Press and HOLD, without releasing, for the whole run. evdev is
            # level triggered and RetroArch reads the current state on each
            # poll, so a held button cannot fall between two samples -- the
            # first attempt pulsed the button and finished before the core's
            # first frame, and the probe honestly reported seeing nothing.
            # Wait for the probe to say it is finished rather than for the
            # process to exit: with no menu driver RetroArch does not act on
            # RETRO_ENVIRONMENT_SHUTDOWN, so waiting on the process times out
            # even though the answer is already in the log.
            deadline = time.monotonic() + 60
            while time.monotonic() < deadline:
                if "ANALOGPROBE: DONE" in log.read_text(errors="replace"):
                    break
                if proc.poll() is not None:
                    break
                time.sleep(0.25)
            pad.write(ecodes.EV_KEY, FIRST_BTN + BOUND_INDEX, 0)
            pad.syn()
            if proc.poll() is None:
                proc.kill()
                proc.wait(timeout=10)
        xvfb.terminate()

        text = log.read_text(errors="replace")
        lines = [ln for ln in text.splitlines() if "ANALOGPROBE:" in ln]
        if not any("DONE" in ln for ln in lines):
            raise SystemExit(
                f"FAIL: the probe core never finished. Log: {log}\n"
                + "\n".join(text.splitlines()[-15:]))

        analog = next((ln for ln in lines if "ANALOG rx=" in ln), "")
        joypad = next((ln for ln in lines if "JOYPAD" in ln), "")
        print()
        print(f"  {analog.strip()}")
        print(f"  {joypad.strip()}")
        print()

        saw_button = "JOYPAD 0x00000000" not in joypad
        ry = analog.split("ry=")[-1].split()[0] if "ry=" in analog else "?"
        saw_analog = ry not in ("0..0", "?")

        if not saw_button:
            raise SystemExit(
                "FAIL: the injected press did not reach the core even as an "
                "ordinary button, so this run proves nothing about the analog "
                "path -- the pad, the autoconfig or the joypad index is wrong")
        print("  ok  the injected press reached the core as a JOYPAD button")

        if saw_analog:
            print(f"  ok  and as analog deflection: ry={ry}")
            print("\nVERDICT: RetroArch DOES convert a button bound to an "
                  "analog half-axis into deflection, so padmap's "
                  "input_r_y_minus_btn line is a correct way to say "
                  "'this button is C-up'.")
        else:
            print(f"  gap: the analog axis never moved (ry={ry})")
            print("\nVERDICT: RetroArch does NOT feed a button bound to an "
                  "analog half-axis to a core reading RETRO_DEVICE_ANALOG. "
                  "mupen64plus-next reads the N64 C-buttons that way, so a "
                  "button mapped to C-up can never work through this bind and "
                  "padmap must express it some other way.")
        print("\nall checks passed")
        return 0
    finally:
        try:
            pad.close()
        except Exception:                                # noqa: BLE001
            pass
        if keep:
            print(f"\nkept: {root}")
        else:
            shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())

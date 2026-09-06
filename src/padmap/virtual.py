"""Republish physical pads as virtual ones in a chosen order.

Each physical pad is grabbed (EVIOCGRAB, so nothing else sees its events) and
re-emitted through uinput as a pad with a name and phys we control:

    padmap Player 1   phys=padmap/p1
    padmap Player 2   phys=padmap/p2

That buys two things the kernel will not give us:

  * Unique identity. Four Mayflash ports are byte-identical upstream; their
    virtual counterparts are not.
  * A name RetroArch can pin. Its existing reservation matcher compares
    device names exactly (task_autodetect.c, reallocate_port_if_needed), so
    `input_playerN_reserved_device = "padmap Player N"` binds a player to a
    pad regardless of enumeration order. No RetroArch patch required.

What each pad claims to *be* is a separate decision from what it is called,
and a switchable one -- see identity_for. By default it mirrors the source
controller, so SDL's database and libretro's autoconfig recognise it; the
alternative is padmap's own 1209:0001, which is what lets PADMAP_ONLY_VIRTUAL
hide the physical pads.

Force feedback is proxied rather than dropped: effect uploads arriving on the
virtual device are re-uploaded to the physical one and playback commands are
translated through an effect-id map. Rumble matters for N64 and GameCube.
"""

from __future__ import annotations

import errno
import logging
import os
import select
from dataclasses import dataclass, field

import evdev
from evdev import ecodes

from .devices import Pad, VIRTUAL_PHYS_PREFIX, open_device
from .profiles import AxisCalibration, load as load_profile

log = logging.getLogger("padmap.virtual")

# pid.codes, the VID set aside for open-source hardware projects. Using a real
# vendor's ID here would make our pads impersonate their hardware to any
# autoconfig heuristic that matches on vid/pid.
PADMAP_VID = 0x1209
PADMAP_PID = 0x0001
# The version a uinput device reports unless told otherwise.
PADMAP_VERSION = 0x0001

# Which identity the virtual pads advertise. See identity_for.
ENV_IDENTITY = "PADMAP_PAD_IDENTITY"
ENV_ONLY_VIRTUAL = "PADMAP_ONLY_VIRTUAL"
IDENTITY_MIRROR = "mirror"
IDENTITY_PADMAP = "padmap"


@dataclass(frozen=True)
class Identity:
    """What a virtual pad tells the world it is.

    All four fields, not just vid/pid, because all four go into the SDL
    joystick GUID -- and measured against SDL itself, the *bus* is the field
    that decides whether its database matches:

        bus 3, ids 0079:1830, any name, any version  -> Arcade Fightstick F300
        bus 6, ids 0079:1830, any name, any version  -> no match

    SDL zeroes the name checksum before comparing and ignores the version, so
    mirroring vid/pid alone would have changed nothing: uinput devices report
    BUS_VIRTUAL, and every database entry for a USB pad is under BUS_USB.
    """

    vendor: int
    product: int
    bustype: int
    version: int


def identity_mode() -> str:
    """`mirror` (default) or `padmap`.

    The two are a genuine trade, not a preference:

    * **mirror** -- the virtual pad presents the source controller's bus and
      ids, so SDL's built-in database and libretro's autoconfig match it
      exactly as they would the real thing. A controller nobody has mapped yet
      therefore behaves as it did before padmap existed, which matters because
      the wizard that maps it lives *inside* the front-end: a front-end you
      cannot navigate is a wizard you cannot reach.
    * **padmap** -- 1209:0001 on BUS_VIRTUAL. This is what makes
      PADMAP_ONLY_VIRTUAL work: it hides the physical pads by telling SDL to
      ignore everything except 1209:0001, which only leaves the virtual pads
      behind if they are the only things carrying those ids.

    So hiding the physical pads is selected for here too. With mirroring on,
    the ignore-list matches nothing and the user is left with no controller at
    all -- the exact failure that switch had before, in reverse.

    Worth being plain about what mirroring does *not* do: it restores whatever
    SDL already knew, which for a pad absent from its database is a guessed
    layout with Accept on raw button 0. Usable enough to reach the wizard,
    which is all it is claimed to be.
    """
    chosen = os.environ.get(ENV_IDENTITY, "").strip().lower()
    if chosen in (IDENTITY_MIRROR, IDENTITY_PADMAP):
        return chosen
    if os.environ.get(ENV_ONLY_VIRTUAL) == "1":
        return IDENTITY_PADMAP
    return IDENTITY_MIRROR


def identity_for(
    pad: Pad, source: evdev.InputDevice | None = None
) -> Identity:
    """The identity a pad's virtual counterpart will advertise.

    The single place this is decided. Everything that computes an SDL GUID or
    writes a vid/pid has to agree with what the device actually reports, and
    disagreement is silent in both directions -- a mapping under a GUID SDL
    never looks up simply does nothing, and says nothing.
    """
    if identity_mode() == IDENTITY_PADMAP:
        return Identity(PADMAP_VID, PADMAP_PID, ecodes.BUS_VIRTUAL,
                        PADMAP_VERSION)

    info = None
    device = source
    try:
        if device is None:
            device = open_device(pad)
        info = device.info
    except OSError as error:
        log.warning("could not read %s's ids (%s); using padmap's own",
                    pad.event, error)
    finally:
        if device is not None and source is None:
            device.close()

    if info is None:
        # Deliberately the same answer padmap mode gives, so a pad that cannot
        # be read still has *one* identity rather than two half-applied ones.
        return Identity(PADMAP_VID, PADMAP_PID, ecodes.BUS_VIRTUAL,
                        PADMAP_VERSION)
    return Identity(info.vendor, info.product, info.bustype, info.version)

# Event types that flow controller -> host. EV_FF and EV_FF_STATUS travel the
# other way and are handled separately.
FORWARD_TYPES = frozenset({
    ecodes.EV_KEY, ecodes.EV_ABS, ecodes.EV_REL, ecodes.EV_MSC, ecodes.EV_SYN,
})


# Every virtual pad name starts with this. Exposed separately so code that has
# to recognise one of our names without knowing its player number -- cleaning
# padmap values back out of retroarch.cfg, say -- cannot drift from the name
# actually written here.
VIRTUAL_PREFIX = "padmap Player "


def virtual_name(player: int) -> str:
    return f"{VIRTUAL_PREFIX}{player}"


def virtual_phys(player: int) -> str:
    return f"{VIRTUAL_PHYS_PREFIX}p{player}"


@dataclass
class VirtualPad:
    player: int
    pad: Pad
    source: evdev.InputDevice
    ui: evdev.UInput
    # effect id on our virtual device -> effect id on the physical device
    effects: dict[int, int] = field(default_factory=dict)
    # ABS code -> calibration, applied as events pass through. Correcting
    # here rather than in a front-end means every consumer benefits, and a
    # worn stick stops reading as permanently deflected everywhere at once.
    axes: dict[int, AxisCalibration] = field(default_factory=dict)

    @property
    def name(self) -> str:
        return virtual_name(self.player)

    def close(self) -> None:
        try:
            self.source.ungrab()
        except OSError:
            pass
        self.source.close()
        self.ui.close()


def _capabilities_for(source: evdev.InputDevice) -> dict:
    """Source capabilities, minus the bookkeeping uinput supplies itself."""
    caps = dict(source.capabilities())
    # EV_SYN is implicit; passing it through confuses UInput's setup.
    caps.pop(ecodes.EV_SYN, None)
    # We proxy FF ourselves, so the virtual device must advertise it even
    # though from_device would otherwise be free to drop it.
    if ecodes.EV_FF in caps:
        caps[ecodes.EV_FF] = list(caps[ecodes.EV_FF])
    return caps


def create(pad: Pad, player: int, grab: bool = True) -> VirtualPad:
    source = open_device(pad)
    if grab:
        source.grab()

    # UInput writes ff_effects_max into the uinput setup unconditionally,
    # defaulting to 96 even when EV_FF is absent from `events`. RetroArch
    # reads that number back and reports "supports 96 force feedback effects"
    # for a pad that cannot rumble at all. Mirror the source instead: we can
    # only proxy effects the physical device is able to play.
    max_effects = source.ff_effects_count if ecodes.EV_FF in source.capabilities() else 0

    # What this pad claims to be -- mirrored from the source by default. See
    # identity_for, which is the only place that decides, because a GUID
    # written from one answer and a device created from another is a mapping
    # that is simply never matched, with nothing said by anyone.
    identity = identity_for(pad, source)
    ui = evdev.UInput(
        events=_capabilities_for(source),
        name=virtual_name(player),
        phys=virtual_phys(player),
        vendor=identity.vendor,
        product=identity.product,
        version=identity.version,
        # The bus is mirrored too, and it is the field that matters most:
        # SDL's database is keyed on a GUID whose first field is the bus, so a
        # pad on BUS_VIRTUAL matches no entry written for a USB controller no
        # matter what ids it carries. Measured directly against SDL.
        bustype=identity.bustype,
        max_effects=max_effects,
    )
    # A stored profile carries the measured resting position of each stick.
    # Without one the pad is forwarded verbatim, which is correct for a
    # controller that actually centres itself.
    profile = load_profile(pad)
    axes = profile.axes if profile is not None else {}
    if axes:
        log.info("player %d: applying calibration for %d axis/axes",
                 player, len(axes))

    if axes:
        # Seed every calibrated axis at its centre.
        #
        # Not at the source's current value: an adapter's absinfo can hold a
        # stale power-on default until the stick is physically moved. This
        # N64 adapter reports 174/185 on a 0-255 axis that actually centres
        # at 128, and only corrects itself once touched. Copying that through
        # makes the pad look permanently deflected from the moment it appears,
        # which a frontend acts on immediately -- runaway menu navigation.
        #
        # Assuming centre is safe in the other direction too: if the stick
        # really is deflected at startup, the first event corrects it within
        # milliseconds, and a pad that briefly reads centred is harmless where
        # one that briefly reads slammed to a corner is not.
        mid_by_code = {
            code: (cal.minimum + cal.maximum) // 2 for code, cal in axes.items()
        }
        for code, value in mid_by_code.items():
            ui.write(ecodes.EV_ABS, code, value)
        ui.syn()

    log.info("player %d: %s -> %s (%s, %04x:%04x bus %d, %s identity)",
             player, pad.event, ui.device.path, virtual_name(player),
             identity.vendor, identity.product, identity.bustype,
             identity_mode())
    return VirtualPad(player=player, pad=pad, source=source, ui=ui, axes=axes)


class Republisher:
    """Pumps events between physical and virtual pads until stopped."""

    def __init__(self, pads: list[VirtualPad]) -> None:
        self.pads = pads
        self._by_source_fd = {p.source.fd: p for p in pads}
        self._by_ui_fd = {p.ui.fd: p for p in pads}
        self._stop = False

    def stop(self) -> None:
        self._stop = True

    @property
    def fds(self) -> list[int]:
        """Descriptors to watch. Physical sources plus our own uinput nodes.

        Both directions matter: sources carry button presses inbound, and the
        uinput nodes carry force-feedback requests back out.
        """
        return list(self._by_source_fd) + list(self._by_ui_fd)

    def handle_readable(self, fd: int) -> None:
        """Service one descriptor reported readable, in either direction."""
        if fd in self._by_source_fd:
            self._forward(self._by_source_fd[fd])
        elif fd in self._by_ui_fd:
            self._handle_feedback(self._by_ui_fd[fd])

    def pump(self, timeout: float | None = 0.5) -> None:
        readable, _, _ = select.select(self.fds, [], [], timeout)
        for fd in readable:
            self.handle_readable(fd)

    def run(self) -> None:
        while not self._stop:
            self.pump()

    def close(self) -> None:
        for pad in self.pads:
            pad.close()

    def _forward(self, vpad: VirtualPad) -> None:
        try:
            events = list(vpad.source.read())
        except (BlockingIOError, OSError) as exc:
            if isinstance(exc, OSError) and exc.errno == errno.ENODEV:
                log.warning("player %d: source disappeared", vpad.player)
                self._stop = True
            return
        for event in events:
            if event.type not in FORWARD_TYPES:
                continue
            if event.type == ecodes.EV_ABS:
                calibration = vpad.axes.get(event.code)
                if calibration is not None:
                    event.value = calibration.apply(event.value)
            vpad.ui.write_event(event)

    def _handle_feedback(self, vpad: VirtualPad) -> None:
        """Proxy rumble from the virtual device back to the physical one."""
        try:
            events = list(vpad.ui.read())
        except (BlockingIOError, OSError):
            return

        for event in events:
            if event.type == ecodes.EV_UINPUT:
                if event.code == ecodes.UI_FF_UPLOAD:
                    self._proxy_upload(vpad, event.value)
                elif event.code == ecodes.UI_FF_ERASE:
                    self._proxy_erase(vpad, event.value)
            elif event.type == ecodes.EV_FF:
                # Playback: translate the virtual effect id to the real one.
                real_id = vpad.effects.get(event.code)
                if real_id is not None:
                    try:
                        vpad.source.write(ecodes.EV_FF, real_id, event.value)
                    except OSError:
                        log.debug("player %d: rumble write failed", vpad.player)

    def _proxy_upload(self, vpad: VirtualPad, request_id: int) -> None:
        upload = vpad.ui.begin_upload(request_id)
        try:
            effect = upload.effect
            virtual_id = effect.id
            # Effect ids are per-device; ask the physical device to allocate
            # its own rather than reusing the virtual one.
            effect.id = -1
            real_id = vpad.source.upload_effect(effect)
            vpad.effects[virtual_id] = real_id
            upload.retval = 0
        except OSError as exc:
            log.debug("player %d: effect upload failed: %s", vpad.player, exc)
            upload.retval = -1
        finally:
            vpad.ui.end_upload(upload)

    def _proxy_erase(self, vpad: VirtualPad, request_id: int) -> None:
        erase = vpad.ui.begin_erase(request_id)
        try:
            real_id = vpad.effects.pop(erase.effect_id, None)
            if real_id is not None:
                vpad.source.erase_effect(real_id)
            erase.retval = 0
        except OSError as exc:
            log.debug("player %d: effect erase failed: %s", vpad.player, exc)
            erase.retval = -1
        finally:
            vpad.ui.end_erase(erase)

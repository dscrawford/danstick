//! Republish a physical pad as a virtual one under a name we control.
//!
//! Each physical pad is grabbed (EVIOCGRAB, so nothing else sees its events)
//! and re-emitted through uinput as a pad called `padmap Player N` on phys
//! `padmap/pN`. That buys two things the kernel will not give us: unique
//! identity, since four Mayflash ports are byte-identical upstream and their
//! clones are not; and a name RetroArch can pin, since its reservation matcher
//! compares device names exactly, so `input_playerN_reserved_device = "padmap
//! Player N"` binds a player to a pad regardless of enumeration order.

use std::collections::BTreeMap;
use std::ffi::CString;

use evdev::{
    uinput::VirtualDevice, AbsInfo, AbsoluteAxisCode, AttributeSet, BusType, Device, EventType,
    FFEffect, InputEvent, InputId, KeyCode, UInputEvent, UinputAbsSetup,
};
use log::{info, warn};
use padmap_core::calibration::AxisCalibration;
use padmap_core::dsupad;
use padmap_core::emit::version_for;

use crate::motion;
use crate::pad::{Pad, VIRTUAL_PHYS_PREFIX};
use crate::triton;

/// pid.codes, the vendor id set aside for open-source hardware projects. Using
/// a real vendor's id here would make our pads impersonate their hardware to
/// any autoconfig heuristic that matches on vid/pid.
pub const PADMAP_VID: u16 = 0x1209;
pub const PADMAP_PID: u16 = 0x0001;
pub use padmap_core::emit::PADMAP_VERSION;
const BUS_VIRTUAL: u16 = 0x06;

pub const ENV_IDENTITY: &str = "PADMAP_PAD_IDENTITY";
pub const ENV_ONLY_VIRTUAL: &str = "PADMAP_ONLY_VIRTUAL";

/// Every virtual pad name starts with this.
pub const VIRTUAL_PREFIX: &str = "padmap Player ";

pub fn virtual_name(player: u32) -> String {
    format!("{VIRTUAL_PREFIX}{player}")
}

pub fn virtual_phys(player: u32) -> String {
    format!("{VIRTUAL_PHYS_PREFIX}p{player}")
}

/// Which identity the virtual pads advertise.
///
/// The two are a genuine trade, not a preference.
///
/// * **Mirror** -- the clone presents the source controller's bus and ids, so
///   SDL's built-in database and libretro's autoconfig match it exactly as they
///   would the real thing. A controller nobody has mapped yet therefore behaves
///   as it did before padmap existed, which matters because the wizard that
///   maps it lives *inside* the front-end: a front-end you cannot navigate is a
///   wizard you cannot reach.
/// * **Padmap** -- 1209:0001 on BUS_VIRTUAL. This is what makes
///   `PADMAP_ONLY_VIRTUAL` work: it hides the physical pads by telling SDL to
///   ignore everything except 1209:0001, which only leaves the clones behind if
///   they are the only things carrying those ids. With mirroring on, the
///   ignore-list matches nothing and the user is left with no controller at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityMode {
    Mirror,
    Padmap,
}

impl IdentityMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            IdentityMode::Mirror => "mirror",
            IdentityMode::Padmap => "padmap",
        }
    }

    /// Read once, at startup.
    ///
    /// The Python re-read the environment on every `identity_for` call, so a
    /// variable changed under a running daemon could give two clones different
    /// identities in one session -- and a GUID written from one answer against
    /// a device created from the other is a mapping that is simply never
    /// matched, with nothing said by anyone.
    pub fn from_env() -> Self {
        match std::env::var(ENV_IDENTITY)
            .unwrap_or_default()
            .trim()
            .to_lowercase()
            .as_str()
        {
            "mirror" => IdentityMode::Mirror,
            "padmap" => IdentityMode::Padmap,
            _ if std::env::var(ENV_ONLY_VIRTUAL).as_deref() == Ok("1") => IdentityMode::Padmap,
            _ => IdentityMode::Mirror,
        }
    }
}

/// What a virtual pad tells the world it is.
///
/// All four fields, not just vid/pid, because all four go into the SDL joystick
/// GUID -- and measured against SDL itself, the *bus* is the field that decides
/// whether its database matches:
///
/// ```text
/// bus 3, ids 0079:1830, any name, any version  -> Arcade Fightstick F300
/// bus 6, ids 0079:1830, any name, any version  -> no match
/// ```
///
/// SDL zeroes the name checksum before comparing and ignores the version, so
/// mirroring vid/pid alone would have changed nothing: uinput devices report
/// BUS_VIRTUAL, and every database entry for a USB pad is under BUS_USB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    pub vendor: u16,
    pub product: u16,
    pub bustype: u16,
    pub version: u16,
}

impl Identity {
    pub const PADMAP: Identity = Identity {
        vendor: PADMAP_VID,
        product: PADMAP_PID,
        bustype: BUS_VIRTUAL,
        version: PADMAP_VERSION,
    };

    /// The identity a pad's clone will advertise.
    ///
    /// The single place this is decided. Everything that computes an SDL GUID
    /// or writes a vid/pid has to agree with what the device actually reports,
    /// and disagreement is silent in both directions.
    pub fn for_source(mode: IdentityMode, source: &Device, player: u32) -> Identity {
        let version = version_for(player);
        match mode {
            IdentityMode::Padmap => Identity {
                version,
                ..Identity::PADMAP
            },
            IdentityMode::Mirror => {
                let id = source.input_id();
                Identity {
                    vendor: id.vendor(),
                    product: id.product(),
                    bustype: id.bus_type().0,
                    version,
                }
            }
        }
    }
}

/// Event types that flow controller -> host.
///
/// EV_FF and EV_FF_STATUS travel the other way and are handled separately.
#[inline]
pub fn forwarded(kind: EventType) -> bool {
    matches!(
        kind,
        EventType::KEY
            | EventType::ABSOLUTE
            | EventType::RELATIVE
            | EventType::MISC
            | EventType::SYNCHRONIZATION
    )
}

/// Where a clone's events come from.
///
/// Two kinds, because padmap has two. Most controllers the kernel drives and
/// evdev reads; a 2026 Steam Controller has no evdev node at all on a kernel
/// before 7.3, so padmap speaks its protocol and hands the same events out
/// the other side. Everything downstream -- the clone, the calibration, the
/// forwarding loop -- cannot tell them apart, which is the point: padmap is a
/// virtual gamepad, and a controller that needs a workaround should still
/// arrive as an ordinary pad.
#[derive(Debug)]
pub enum Source {
    // Boxed for the same reason the other is: `Device` is 280 bytes and a
    // `triton::Source` a few dozen, and every VirtualPad carries one of these
    // whichever it turns out to be.
    Evdev(Box<Device>),
    Triton(Box<triton::Source>),
}

impl Source {
    /// Append events since the last call to `out`, or `WouldBlock` if none.
    ///
    /// Into a caller's buffer rather than a fresh one, because the caller has
    /// a buffer it reuses and `republish.rs` says so: "reused between calls so
    /// the hot path allocates nothing". That was true of the `Republisher`'s
    /// own vectors and not of this, which built and returned a new `Vec` on
    /// every call that produced an event -- up to 250 times a second per pad,
    /// for as long as a stick is moving. Small, but it is the one path this
    /// project exists to keep quiet, and a comment claiming an allocation-free
    /// hot path should not have one underneath it.
    pub fn fetch_events(&mut self, out: &mut Vec<InputEvent>) -> std::io::Result<()> {
        match self {
            // `fetch_events` hands back an iterator over evdev's own reused
            // buffer; extending from it keeps that reuse, where collecting
            // into a Vec threw it away.
            Source::Evdev(device) => {
                out.extend(device.fetch_events()?);
                Ok(())
            }
            Source::Triton(source) => source.fetch_events(out),
        }
    }

    /// Motion this source decoded itself, if it decodes any.
    ///
    /// Only the Steam Controller does: its gyro arrives in the same reports as
    /// its buttons. An evdev pad's motion is a separate node, held in
    /// [`VirtualPad::sensor`].
    pub fn motion(&self) -> Option<padmap_core::motion::Motion> {
        match self {
            Source::Evdev(_) => None,
            Source::Triton(source) => source.motion(),
        }
    }

    /// Let go, if we were ever holding it.
    pub fn ungrab(&mut self) {
        match self {
            Source::Evdev(device) => {
                let _ = device.ungrab();
            }
            // Nothing to release: a hidraw node is not grabbed. padmap holds
            // it open, and the kernel is not publishing an evdev node for
            // anything else to read in the first place.
            Source::Triton(_) => {}
        }
    }

    /// Keys currently down, for lifting them when a wizard pauses.
    pub fn held_keys(&self) -> Vec<u16> {
        match self {
            Source::Evdev(device) => device
                .get_key_state()
                .map(|keys| keys.iter().map(|key| key.code()).collect())
                .unwrap_or_default(),
            Source::Triton(source) => source.held_keys(),
        }
    }

    /// Hand an effect to the physical device, or refuse.
    ///
    /// A Steam Controller's rumble is an output report padmap does not send
    /// yet, and a clone that accepted an upload it cannot play would report
    /// itself as capable and then be silent. `assemble` already mirrors the
    /// source's effect count for the same reason.
    pub fn upload_ff_effect(&mut self, effect: evdev::FFEffectData) -> std::io::Result<FFEffect> {
        match self {
            Source::Evdev(device) => device.upload_ff_effect(effect),
            Source::Triton(_) => Err(std::io::Error::from(std::io::ErrorKind::Unsupported)),
        }
    }

    /// The controls this source reports, for the mapping wizard.
    pub fn capabilities(&self) -> (Vec<u16>, Vec<u16>) {
        match self {
            Source::Evdev(device) => capabilities(device),
            Source::Triton(source) => {
                let (keys, axes) = source.capabilities();
                (keys, axes.into_iter().map(|(code, _)| code).collect())
            }
        }
    }

    /// Each axis's travel and where it rests, for deciding when one has been
    /// pushed rather than nudged.
    pub fn axis_spans(&self) -> BTreeMap<u16, padmap_core::sdl::AxisSpan> {
        match self {
            Source::Evdev(device) => axis_spans(device),
            Source::Triton(source) => source
                .capabilities()
                .1
                .into_iter()
                .map(|(code, info)| {
                    (
                        code,
                        padmap_core::sdl::AxisSpan::new(
                            info.minimum(),
                            info.maximum(),
                            info.value(),
                        ),
                    )
                })
                .collect(),
        }
    }

    /// Read without waiting.
    ///
    /// The daemon leaves an evdev source blocking, because epoll has already
    /// said the descriptor is readable before `fetch_events` is ever called.
    /// Anything driving the pump directly -- a test, a probe -- has nothing
    /// gating it, and a blocking source turns the first call into a wait for
    /// an event that only arrives after the call returns.
    ///
    /// A Triton source is opened `O_NONBLOCK` and is always in this state, so
    /// this is a no-op for one.
    pub fn set_nonblocking(&mut self, nonblocking: bool) -> std::io::Result<()> {
        match self {
            Source::Evdev(device) => device.set_nonblocking(nonblocking),
            Source::Triton(_) => Ok(()),
        }
    }

    pub fn path(&self) -> String {
        match self {
            Source::Evdev(device) => device.physical_path().unwrap_or_default().to_owned(),
            Source::Triton(source) => source.path().display().to_string(),
        }
    }
}

impl std::os::fd::AsFd for Source {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        match self {
            Source::Evdev(device) => device.as_fd(),
            Source::Triton(source) => source.as_fd(),
        }
    }
}

impl std::os::fd::AsRawFd for Source {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        match self {
            Source::Evdev(device) => device.as_raw_fd(),
            Source::Triton(source) => source.as_raw_fd(),
        }
    }
}

/// Open a pad for reading, as the republisher would, without cloning it.
///
/// For an assignment session and the wizards: they read the *physical* pad
/// directly, grabbed so the front-end never sees the presses. The same source
/// the republisher picks, not always the evdev node -- a pad that has to be
/// read over hidraw has to be read that way here too, or the screen that maps
/// a controller cannot see the controller it is mapping.
pub fn open_source(pad: &Pad, grab: bool) -> Result<Source, CloneError> {
    if triton::owns(pad) {
        let source = triton::Source::open(&pad.path)
            .map_err(|error| CloneError::Open(pad.path.display().to_string(), error))?;
        return Ok(Source::Triton(Box::new(source)));
    }
    let mut source = Device::open(&pad.path)
        .map_err(|error| CloneError::Open(pad.path.display().to_string(), error))?;
    // See `Source::drain`: a blocking source stops the loop dead.
    let _ = source.set_nonblocking(true);
    if grab {
        if let Err(error) = source.grab() {
            warn!(
                "could not grab {} ({error}): presses will leak through to other applications",
                pad.event()
            );
        }
    }
    Ok(Source::Evdev(Box::new(source)))
}

impl Source {
    /// Whether the kernel let us hold this exclusively. A hidraw source is
    /// never grabbed and never needs to be.
    pub fn grab(&mut self) -> std::io::Result<()> {
        match self {
            Source::Evdev(device) => device.grab(),
            Source::Triton(_) => Ok(()),
        }
    }

    /// Every axis as the driver declares it, for calibration.
    pub fn declared_axes(&self) -> BTreeMap<u16, padmap_core::calibration::Declared> {
        use padmap_core::calibration::Declared;
        match self {
            Source::Evdev(device) => device
                .get_absinfo()
                .map(|axes| {
                    axes.map(|(code, info)| {
                        (
                            code.0,
                            Declared {
                                minimum: info.minimum(),
                                maximum: info.maximum(),
                                value: info.value(),
                                flat: info.flat(),
                            },
                        )
                    })
                    .collect()
                })
                .unwrap_or_default(),
            Source::Triton(source) => source
                .capabilities()
                .1
                .into_iter()
                .map(|(code, info)| {
                    (
                        code,
                        Declared {
                            minimum: info.minimum(),
                            maximum: info.maximum(),
                            value: info.value(),
                            flat: info.flat(),
                        },
                    )
                })
                .collect(),
        }
    }

    /// The GUID SDL computes for the *physical* controller, or `None` for a
    /// source SDL never sees.
    ///
    /// Deliberately the device's raw name, not the cleaned one: SDL checksums
    /// what the kernel reports, and the N64 adapter measured here prefixes its
    /// name with a 0x18 byte. Stripping it changes the checksum and the lookup
    /// silently matches nothing.
    pub fn physical_guid(&self) -> Option<String> {
        match self {
            Source::Evdev(device) => {
                let id = device.input_id();
                Some(padmap_core::sdl::guid(
                    id.bus_type().0,
                    id.vendor(),
                    id.product(),
                    id.version(),
                    device.name().unwrap_or(""),
                ))
            }
            // The kernel publishes no joypad for it, so SDL has no GUID for it
            // and no database entry that could be carried over.
            Source::Triton(_) => None,
        }
    }

    /// Discard whatever is queued, so a press from before a session opened
    /// cannot claim a slot.
    pub fn drain(&mut self) {
        let mut sink = Vec::new();
        let _ = self.set_nonblocking(true);
        while self.fetch_events(&mut sink).is_ok() && !sink.is_empty() {
            sink.clear();
        }
        // Left non-blocking, deliberately. Every reader of a source is
        // epoll-driven, and epoll only promises that *one* read will not
        // block -- a second, to finish a frame or drain the rest, blocks the
        // whole daemon on a pad that has gone quiet. That is not a stall, it
        // is the end: one loop serves every pad and the socket too.
    }
}

/// A physical pad, its clone, and what passes between them.
#[derive(Debug)]
pub struct VirtualPad {
    pub player: u32,
    pub pad: Pad,
    /// What this clone advertises. Kept, because the SDL GUID is computed from
    /// it and a mapping written under a different one is never matched.
    pub identity: Identity,
    pub source: Source,
    pub clone: VirtualDevice,
    /// ABS code -> calibration, applied as events pass through. Correcting here
    /// rather than in a front-end means every consumer benefits, and a worn
    /// stick stops reading as permanently deflected everywhere at once.
    pub axes: BTreeMap<u16, AxisCalibration>,
    /// A DSU-shaped picture of the same events, for the motion server.
    ///
    /// Fed from the *corrected* stream rather than the raw one, so a consumer
    /// reading padmap over UDP sees the same calibrated sticks as one reading
    /// the clone. Two pictures of one pad that disagree is worse than one.
    pub tracker: dsupad::Tracker,
    /// The controller's motion sensor, open, when the kernel publishes one.
    ///
    /// `None` covers three cases that behave identically: a pad with no gyro,
    /// a Steam Controller (whose motion arrives in the same reports as its
    /// buttons, so there is no second node), and a node that would not open.
    pub sensor: Option<motion::Sensor>,
    /// Events the clone refused. Counted rather than logged one for one: this
    /// is the hot path, a pad emits at about 8ms, and a fault that repeats
    /// would otherwise write a log line per event for as long as the game runs.
    /// The Python learned this by filling a 3.1GB tmpfs with 114 million lines.
    pub dropped: u64,
    /// The physical source is gone and this clone is finished.
    pub gone: bool,
    /// Effect id on our clone -> the effect the physical device allocated for
    /// it. Effect ids are per-device, so the two are not interchangeable.
    ///
    /// `FFEffect` erases itself from the physical pad when dropped, so removing
    /// an entry here is what frees the slot -- there is no separate teardown to
    /// forget.
    effects: BTreeMap<i16, FFEffect>,
    forwarded_any: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum CloneError {
    #[error("opening {0}: {1}")]
    Open(String, #[source] std::io::Error),
    #[error("building a clone for player {0}: {1}")]
    Build(u32, #[source] std::io::Error),
}

/// Grab a pad and publish its clone.
pub fn create(
    pad: &Pad,
    player: u32,
    mode: IdentityMode,
    profile_axes: &BTreeMap<u16, AxisCalibration>,
    grab: bool,
) -> Result<VirtualPad, CloneError> {
    // Two ways to obtain the same three things. A Steam Controller slot is
    // opened as hidraw and decoded by padmap, because the kernel publishes no
    // joypad for it to read; everything after this block is identical for
    // both, which is the point -- a controller needing a workaround should
    // still arrive downstream as an ordinary pad.
    let (source, identity, clone) = if triton::owns(pad) {
        let source = triton::Source::open(&pad.path)
            .map_err(|error| CloneError::Open(pad.path.display().to_string(), error))?;
        // for_source reads the ids off an evdev::Device and there is none
        // here. Mirroring means the ids the controller itself reports, which
        // discovery already read out of sysfs -- on BUS_USB, because that is
        // how it is attached and the bus is the field SDL's database is keyed
        // on.
        let identity = match mode {
            IdentityMode::Padmap => Identity {
                version: version_for(player),
                ..Identity::PADMAP
            },
            IdentityMode::Mirror => Identity {
                vendor: pad.vid,
                product: pad.pid,
                bustype: BusType::BUS_USB.0,
                version: version_for(player),
            },
        };
        let (keys, axes) = source.capabilities();
        let clone = build_clone_from(&keys, &axes, player, identity)
            .map_err(|error| CloneError::Build(player, error))?;
        (Source::Triton(Box::new(source)), identity, clone)
    } else {
        let mut source = Device::open(&pad.path)
            .map_err(|error| CloneError::Open(pad.path.display().to_string(), error))?;
        // See `Source::drain`: epoll promises one read will not block, and
        // the forwarder does more than one.
        let _ = source.set_nonblocking(true);
        if grab {
            if let Err(error) = source.grab() {
                // Worth saying out loud rather than swallowing: a failed grab
                // means every press ALSO reaches whatever else is listening.
                warn!(
                    "could not grab {} ({error}): presses will leak through to other applications",
                    pad.event()
                );
            }
        }
        let identity = Identity::for_source(mode, &source, player);
        let clone = build_clone(&source, player, identity)
            .map_err(|error| CloneError::Build(player, error))?;
        (Source::Evdev(Box::new(source)), identity, clone)
    };

    // A stored profile carries the measured resting position of each stick.
    // Without one the pad is forwarded verbatim, which is correct for a
    // controller that actually centres itself.
    let mut axes = BTreeMap::new();
    for (code, calibration) in profile_axes {
        // Repeats a check the profile store has usually already run, because
        // the store is not the only source: a live measurement is handed over
        // without a round trip through JSON. Getting it wrong does not cost a
        // bad axis, it costs the process.
        if calibration.fits() {
            axes.insert(*code, *calibration);
        } else {
            warn!(
                "player {player}: axis {code}'s calibration cannot be written to an evdev value \
                 (centre {}, range {}..{}); forwarding that axis uncorrected instead",
                calibration.center, calibration.minimum, calibration.maximum
            );
        }
    }

    let tracker = dsupad::Tracker::new(
        source
            .declared_axes()
            .iter()
            .map(|(code, declared)| {
                (
                    *code,
                    dsupad::Range::declared(declared.minimum, declared.maximum, declared.value),
                )
            })
            .collect(),
    );
    // Opened here rather than on demand: a gyro node that appears and
    // disappears with a subscription would be a second thing to get wrong
    // about hotplug, and an unread evdev node costs nothing until it is read.
    let sensor = pad
        .motion
        .as_deref()
        .and_then(|path| match motion::Sensor::open(path) {
            Ok(sensor) => Some(sensor),
            Err(error) => {
                warn!(
                    "player {player}: motion sensor {} would not open ({error}); \
                 the pad works, its gyro does not",
                    path.display()
                );
                None
            }
        });

    let mut vpad = VirtualPad {
        player,
        pad: pad.clone(),
        identity,
        source,
        clone,
        axes,
        tracker,
        sensor,
        dropped: 0,
        gone: false,
        effects: BTreeMap::new(),
        forwarded_any: false,
    };
    vpad.seed_calibrated_axes();

    info!(
        "player {player}: {} -> {} ({}, {:04x}:{:04x} bus {}, {} identity)",
        pad.event(),
        virtual_name(player),
        virtual_name(player),
        identity.vendor,
        identity.product,
        identity.bustype,
        mode.as_str()
    );
    Ok(vpad)
}

/// Build the clone, and if the phys tag is refused, build it again without one.
///
/// evdev 0.13.2 declares `UI_SET_PHYS` with a `libc::c_char` payload where the
/// kernel's header says `_IOW(UINPUT_IOCTL_BASE, 108, char*)`. The payload size
/// is part of the ioctl *number*, so the request the crate sends is not the one
/// the kernel implements, and the answer is EINVAL -- measured here against a
/// synthetic pad, where every other builder step succeeds and this one refuses.
///
/// The retry is a second open of `/dev/uinput`, once, at startup. It is not
/// conditional on the error, because `with_phys` consumes the builder and there
/// is nothing left to inspect: a clone that cannot be created at all fails the
/// same way twice and reports the second error, which is the same error.
///
/// A clone with no phys still works. What it loses is the tag discovery uses to
/// recognise padmap's own output, which is why both sides also match on the
/// name -- see [`crate::pad::is_padmap_clone`].
fn build_clone(source: &Device, player: u32, identity: Identity) -> std::io::Result<VirtualDevice> {
    with_phys_retry(player, |set_phys| {
        assemble(source, player, identity, set_phys)
    })
}

/// A clone built from a bare capability list rather than from a device.
///
/// For a source padmap decodes itself: there is no `evdev::Device` to copy
/// keys and absinfo off, because the kernel never published one.
fn build_clone_from(
    keys: &[u16],
    axes: &[(u16, AbsInfo)],
    player: u32,
    identity: Identity,
) -> std::io::Result<VirtualDevice> {
    // The name/ids/phys head is spelled out here as well as in `assemble`.
    // Factoring it out is not possible: the builder borrows the name, so a
    // helper returning one cannot outlive the local it borrowed.
    with_phys_retry(player, |set_phys| {
        let name = virtual_name(player);
        let mut builder = VirtualDevice::builder()?.name(&name).input_id(InputId::new(
            BusType(identity.bustype),
            identity.vendor,
            identity.product,
            identity.version,
        ));
        if set_phys {
            let phys = CString::new(virtual_phys(player)).unwrap_or_default();
            builder = builder.with_phys(&phys)?;
        }
        let mut key_set = AttributeSet::<KeyCode>::new();
        for &code in keys {
            key_set.insert(KeyCode(code));
        }
        builder = builder.with_keys(&key_set)?;
        for &(code, info) in axes {
            builder =
                builder.with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisCode(code), info))?;
        }
        builder.build()
    })
}

/// Build, retrying without a phys tag if the first attempt is refused.
///
/// evdev 0.13 encodes `UI_SET_PHYS`'s payload size into the ioctl number and
/// gets it wrong -- a `c_char` where the kernel wants a `char *` -- so the
/// call can return EINVAL on an otherwise-working device. A clone with no
/// phys still works: it is matched by name instead, which is why
/// [`crate::pad::is_padmap_clone`] tests both.
fn with_phys_retry(
    player: u32,
    build: impl Fn(bool) -> std::io::Result<VirtualDevice>,
) -> std::io::Result<VirtualDevice> {
    match build(true) {
        Ok(device) => Ok(device),
        Err(first) => {
            warn!(
                "player {player}: could not publish a clone with a phys tag ({first}); \
                 retrying without one, so it is recognised by name instead"
            );
            build(false)
        }
    }
}

fn assemble(
    source: &Device,
    player: u32,
    identity: Identity,
    set_phys: bool,
) -> std::io::Result<VirtualDevice> {
    let name = virtual_name(player);
    let mut builder = VirtualDevice::builder()?.name(&name).input_id(InputId::new(
        BusType(identity.bustype),
        identity.vendor,
        identity.product,
        identity.version,
    ));
    if set_phys {
        let phys = CString::new(virtual_phys(player)).unwrap_or_default();
        builder = builder.with_phys(&phys)?;
    }

    if let Some(keys) = source.supported_keys() {
        builder = builder.with_keys(keys)?;
    }
    if let Some(relative) = source.supported_relative_axes() {
        builder = builder.with_relative_axes(relative)?;
    }
    if let Some(misc) = source.misc_properties() {
        builder = builder.with_msc(misc)?;
    }
    builder = builder.with_properties(source.properties())?;

    // Full absinfo per axis, never bare codes.
    //
    // A clone created from codes alone comes out with min = max = 0 on every
    // axis. Nothing complains: the pad appears, is configured, and forwards
    // input. RetroArch then divides by that range in udev_compute_axis --
    //
    //     int range = info->maximum - info->minimum;
    //     int axis  = (value - info->minimum) * 0xffff / range - 0x7fff;
    //
    // -- which is a divide by zero the moment anything polls the pad. The game
    // dies with SIGFPE at its first input poll, on the start screen, with a
    // backtrace naming nothing about padmap.
    if let Ok(absinfo) = source.get_absinfo() {
        for (code, info) in absinfo {
            builder = builder.with_absolute_axis(&UinputAbsSetup::new(code, info))?;
        }
    }

    // Mirror the source's effect count rather than taking uinput's default of
    // 96. RetroArch reads that number back and reports "supports 96 force
    // feedback effects" for a pad that cannot rumble at all; we can only proxy
    // effects the physical device is able to play.
    if let Some(ff) = source.supported_ff() {
        builder = builder.with_ff(ff)?;
        builder = builder.with_ff_effects_max(source.max_ff_effects() as u32);
    }

    builder.build()
}

impl VirtualPad {
    /// `/dev/input/eventN` of the clone, as a consumer would open it.
    ///
    /// Asked of the kernel rather than remembered: the node appears a moment
    /// after the device is created, and the `controller` event that names it
    /// must not be sent before it exists.
    pub fn node(&mut self) -> Option<String> {
        // The eventN child of the sysfs input directory appears a moment
        // after the device is created, so a first look right after `create`
        // finds nothing. A short bounded wait rather than a fixed sleep:
        // this is off the hot path (a few republishes a session), and the
        // node is usually there on the first or second try.
        for attempt in 0..50 {
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let found = self
                .clone
                .enumerate_dev_nodes_blocking()
                .ok()
                .and_then(|mut nodes| {
                    nodes.find_map(|path| {
                        path.ok().filter(|path| {
                            path.file_name()
                                .and_then(|n| n.to_str())
                                .is_some_and(|n| n.starts_with("event"))
                        })
                    })
                });
            if let Some(path) = found {
                return Some(path.display().to_string());
            }
        }
        None
    }

    pub fn name(&self) -> String {
        virtual_name(self.player)
    }

    /// Seed every calibrated axis at its centre.
    ///
    /// Not at the source's current value: an adapter's absinfo can hold a stale
    /// power-on default until the stick is physically moved. An N64 adapter
    /// here reports 174/185 on a 0-255 axis that actually centres at 128, and
    /// only corrects itself once touched. Copying that through makes the pad
    /// look permanently deflected from the moment it appears, which a front-end
    /// acts on immediately -- runaway menu navigation.
    ///
    /// Safe in the other direction too: if the stick really is deflected at
    /// startup the first event corrects it within milliseconds, and a pad that
    /// briefly reads centred is harmless where one that briefly reads slammed
    /// into a corner is not.
    fn seed_calibrated_axes(&mut self) {
        if self.axes.is_empty() {
            return;
        }
        let mut frame: Vec<InputEvent> = self
            .axes
            .iter()
            .map(|(code, calibration)| {
                InputEvent::new(EventType::ABSOLUTE.0, *code, calibration.midpoint() as i32)
            })
            .collect();
        frame.push(InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0));
        info!(
            "player {}: applying calibration for {} axis/axes",
            self.player,
            self.axes.len()
        );
        if let Err(error) = self.clone.emit(&frame) {
            warn!(
                "player {}: could not seed calibrated axes: {error}",
                self.player
            );
        }
    }

    /// Rewrite one event on its way through, or leave it alone.
    #[inline]
    pub fn correct(&self, event: InputEvent) -> InputEvent {
        if event.event_type() != EventType::ABSOLUTE {
            return event;
        }
        match self.axes.get(&event.code()) {
            Some(calibration) => InputEvent::new(
                event.event_type().0,
                event.code(),
                calibration.apply(event.value()),
            ),
            None => event,
        }
    }

    /// Say once, and only once, that input is actually reaching the clone.
    ///
    /// "No input reaches the game" has two very different causes -- withheld on
    /// purpose, or never read at all -- and they are indistinguishable from
    /// outside without this line.
    pub fn note_forwarding(&mut self) {
        if !self.forwarded_any {
            self.forwarded_any = true;
            info!("player {}: forwarding input to the clone", self.player);
        }
    }

    pub fn note_dropped(&mut self, error: &std::io::Error) {
        self.dropped += 1;
        if self.dropped == 1 {
            // Nothing about one event is worth the daemon. A dropped event
            // costs a press, or one frame of stick movement; a failure
            // propagating from here costs every player's controller at once.
            warn!(
                "player {}: the clone refused a frame ({error}); dropping it and continuing -- \
                 a controller missing an input recovers, a daemon exiting does not",
                self.player
            );
        }
    }

    pub fn release(&mut self) {
        self.source.ungrab();
    }

    /// A game uploaded an effect to the clone; re-upload it to the real pad.
    ///
    /// Rumble matters for N64 and GameCube, and a clone that advertises force
    /// feedback and then swallows it is worse than one that never claimed to
    /// have any -- the game believes the pad is rumbling.
    pub fn proxy_upload(&mut self, event: UInputEvent) {
        let mut upload = match self.clone.process_ff_upload(event) {
            Ok(upload) => upload,
            Err(error) => {
                warn!(
                    "player {}: effect upload could not begin: {error}",
                    self.player
                );
                return;
            }
        };
        let virtual_id = upload.effect_id();
        // Ask the physical device to allocate its own id rather than reusing
        // the clone's: they are per-device and the two will not agree.
        match self.source.upload_ff_effect(upload.effect()) {
            Ok(effect) => {
                self.effects.insert(virtual_id, effect);
                upload.set_retval(0);
            }
            Err(error) => {
                warn!("player {}: effect upload failed: {error}", self.player);
                upload.set_retval(-1);
            }
        }
    }

    /// A game erased an effect. Dropping our handle erases it upstream too.
    pub fn proxy_erase(&mut self, event: UInputEvent) {
        let mut erase = match self.clone.process_ff_erase(event) {
            Ok(erase) => erase,
            Err(error) => {
                warn!(
                    "player {}: effect erase could not begin: {error}",
                    self.player
                );
                return;
            }
        };
        self.effects.remove(&(erase.effect_id() as i16));
        erase.set_retval(0);
    }

    /// Playback: translate the clone's effect id to the real one.
    pub fn play(&mut self, virtual_id: u16, count: i32) {
        let Some(effect) = self.effects.get_mut(&(virtual_id as i16)) else {
            return;
        };
        if let Err(error) = effect.play(count) {
            // Debug, not warning: a game can ask a pad that has been unplugged
            // to rumble, and that is not worth a line per attempt on a path
            // some titles drive continuously.
            log::debug!("player {}: rumble write failed: {error}", self.player);
        }
    }
}

/// The key codes and real axis codes a pad reports, for guessing a mapping.
///
/// Axis codes include the hat, because the guess needs to know whether there
/// is one -- a pad whose d-pad is a hat and whose mapping says otherwise has
/// no d-pad at all.
pub fn capabilities(source: &Device) -> (Vec<u16>, Vec<u16>) {
    let keys: Vec<u16> = source
        .supported_keys()
        .map(|set| set.iter().map(|key| key.0).collect())
        .unwrap_or_default();
    let axes: Vec<u16> = source
        .supported_absolute_axes()
        .map(|set| set.iter().map(|axis| axis.0).collect())
        .unwrap_or_default();
    (keys, axes)
}

/// Every axis's declared travel and where it currently rests.
///
/// The rest value is what separates a stick from a trigger, and the evdev code
/// cannot: the Mayflash GameCube adapter reports its analogue triggers as
/// ABS_RX and ABS_RY.
pub fn axis_spans(source: &Device) -> BTreeMap<u16, padmap_core::sdl::AxisSpan> {
    let Ok(absinfo) = source.get_absinfo() else {
        return BTreeMap::new();
    };
    absinfo
        .map(|(code, info)| {
            (
                code.0,
                padmap_core::sdl::AxisSpan::new(info.minimum(), info.maximum(), info.value()),
            )
        })
        .collect()
}

/// Every key currently held on a source, for letting go of them on the clone.
pub fn held_keys(source: &Device) -> AttributeSet<evdev::KeyCode> {
    source.get_key_state().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clone_is_named_and_physed_predictably() {
        // RetroArch's reservation matcher compares device names exactly, so
        // these two strings are an interface, not a label.
        assert_eq!(virtual_name(1), "padmap Player 1");
        assert_eq!(virtual_name(4), "padmap Player 4");
        assert_eq!(virtual_phys(1), "padmap/p1");
        assert!(virtual_name(2).starts_with(VIRTUAL_PREFIX));
    }

    #[test]
    fn identity_mode_defaults_to_mirroring() {
        // A controller nobody has mapped yet has to behave as it did before
        // padmap existed, because the wizard that maps it lives inside the
        // front-end.
        assert_eq!(IdentityMode::Mirror.as_str(), "mirror");
        assert_eq!(IdentityMode::Padmap.as_str(), "padmap");
    }

    #[test]
    fn padmaps_own_identity_is_on_the_virtual_bus() {
        // PADMAP_ONLY_VIRTUAL hides the physical pads by telling SDL to ignore
        // everything except these ids; if the clone did not carry them the user
        // would be left with no controller at all.
        assert_eq!(Identity::PADMAP.vendor, 0x1209);
        assert_eq!(Identity::PADMAP.product, 0x0001);
        assert_eq!(Identity::PADMAP.bustype, 0x06);
    }

    #[test]
    fn only_the_inbound_event_types_are_forwarded() {
        for kind in [
            EventType::KEY,
            EventType::ABSOLUTE,
            EventType::RELATIVE,
            EventType::MISC,
            EventType::SYNCHRONIZATION,
        ] {
            assert!(forwarded(kind), "{kind:?} must reach the clone");
        }
        // Force feedback travels the other way; echoing it back would be a loop.
        for kind in [
            EventType::FORCEFEEDBACK,
            EventType::FORCEFEEDBACKSTATUS,
            EventType::LED,
        ] {
            assert!(!forwarded(kind), "{kind:?} must not be forwarded");
        }
    }
}

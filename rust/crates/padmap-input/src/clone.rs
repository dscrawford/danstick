//! Republish a physical pad as `padmap Player N` on phys `padmap/pN`: a unique
//! identity per port, and a name RetroArch's reservation matcher can pin exactly.

use std::collections::BTreeMap;
use std::ffi::CString;

use evdev::uinput::{VirtualDevice, VirtualDeviceBuilder};
use evdev::{
    AbsInfo, AbsoluteAxisCode, AttributeSet, BusType, Device, EventType, FFEffect, InputEvent,
    InputId, KeyCode, UInputEvent, UinputAbsSetup,
};
use log::{info, warn};
use padmap_core::calibration::{AxisCalibration, Declared};
use padmap_core::dsupad;
use padmap_core::emit::version_for;
use padmap_core::sdl::AxisSpan;
use padmap_core::tuning::{Debouncer, Tuning};

use crate::motion;
use crate::pad::{Pad, VIRTUAL_PHYS_PREFIX};
use crate::triton;

/// pid.codes, the open-source vendor id: a real vendor's would impersonate their hardware.
pub const PADMAP_VID: u16 = 0x1209;
pub const PADMAP_PID: u16 = 0x0001;
pub use padmap_core::emit::PADMAP_VERSION;
const BUS_VIRTUAL: u16 = 0x06;

pub const ENV_IDENTITY: &str = "PADMAP_PAD_IDENTITY";
pub const ENV_ONLY_VIRTUAL: &str = "PADMAP_ONLY_VIRTUAL";

pub const VIRTUAL_PREFIX: &str = "padmap Player ";

pub fn virtual_name(player: u32) -> String {
    format!("{VIRTUAL_PREFIX}{player}")
}

pub fn virtual_phys(player: u32) -> String {
    format!("{VIRTUAL_PHYS_PREFIX}p{player}")
}

/// Mirror keeps the source's bus and ids so SDL's database still matches an unmapped pad;
/// Padmap is 1209:0001 on BUS_VIRTUAL, the only thing `PADMAP_ONLY_VIRTUAL` lets SDL see.
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

    /// Read once at startup, so two clones in one session cannot get different answers.
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

/// All four fields go into the SDL GUID, and measured against SDL the *bus* decides a match:
/// every database entry for a USB pad is under BUS_USB, and uinput defaults to BUS_VIRTUAL.
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

    /// The single place a clone's identity is decided; the GUID writers must agree with it.
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

/// Event types that flow controller -> host; EV_FF travels the other way.
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

/// Where a clone's events come from: the kernel's evdev node, or a protocol padmap speaks itself.
#[derive(Debug)]
pub enum Source {
    Evdev(Box<Device>),
    Triton(Box<triton::Source>),
}

fn declared(info: &AbsInfo) -> Declared {
    Declared {
        minimum: info.minimum(),
        maximum: info.maximum(),
        value: info.value(),
        flat: info.flat(),
    }
}

fn span(info: &AbsInfo) -> AxisSpan {
    AxisSpan::new(info.minimum(), info.maximum(), info.value())
}

fn key_ups(codes: impl IntoIterator<Item = u16>) -> Vec<InputEvent> {
    codes
        .into_iter()
        .map(|code| InputEvent::new(EventType::KEY.0, code, 0))
        .collect()
}

impl Source {
    /// Append events since the last call, or `WouldBlock`. Into the caller's buffer: hot path.
    pub fn fetch_events(&mut self, out: &mut Vec<InputEvent>) -> std::io::Result<()> {
        match self {
            Source::Evdev(device) => {
                out.extend(device.fetch_events()?);
                Ok(())
            }
            Source::Triton(source) => source.fetch_events(out),
        }
    }

    /// Motion decoded from the same reports as the buttons; an evdev pad's is a separate node.
    pub fn motion(&self) -> Option<padmap_core::motion::Motion> {
        match self {
            Source::Evdev(_) => None,
            Source::Triton(source) => source.motion(),
        }
    }

    pub fn ungrab(&mut self) {
        if let Source::Evdev(device) = self {
            let _ = device.ungrab();
        }
    }

    /// A hidraw source is never grabbed: the kernel publishes no evdev node for anyone else.
    pub fn grab(&mut self) -> std::io::Result<()> {
        match self {
            Source::Evdev(device) => device.grab(),
            Source::Triton(_) => Ok(()),
        }
    }

    pub fn held_keys(&self) -> Vec<u16> {
        match self {
            Source::Evdev(device) => device
                .get_key_state()
                .map(|keys| keys.iter().map(|key| key.code()).collect())
                .unwrap_or_default(),
            Source::Triton(source) => source.held_keys(),
        }
    }

    /// Refused for a hidraw source: accepting an effect it cannot play would advertise rumble.
    pub fn upload_ff_effect(&mut self, effect: evdev::FFEffectData) -> std::io::Result<FFEffect> {
        match self {
            Source::Evdev(device) => device.upload_ff_effect(effect),
            Source::Triton(_) => Err(std::io::Error::from(std::io::ErrorKind::Unsupported)),
        }
    }

    pub fn capabilities(&self) -> (Vec<u16>, Vec<u16>) {
        match self {
            Source::Evdev(device) => capabilities(device),
            Source::Triton(source) => {
                let (keys, axes) = source.capabilities();
                (keys, axes.into_iter().map(|(code, _)| code).collect())
            }
        }
    }

    pub fn axis_spans(&self) -> BTreeMap<u16, AxisSpan> {
        match self {
            Source::Evdev(device) => axis_spans(device),
            Source::Triton(source) => source
                .capabilities()
                .1
                .into_iter()
                .map(|(code, info)| (code, span(&info)))
                .collect(),
        }
    }

    pub fn declared_axes(&self) -> BTreeMap<u16, Declared> {
        match self {
            Source::Evdev(device) => device
                .get_absinfo()
                .map(|axes| {
                    axes.map(|(code, info)| (code.0, declared(&info)))
                        .collect()
                })
                .unwrap_or_default(),
            Source::Triton(source) => source
                .capabilities()
                .1
                .into_iter()
                .map(|(code, info)| (code, declared(&info)))
                .collect(),
        }
    }

    /// A Triton source is opened `O_NONBLOCK` and stays that way; this is a no-op for one.
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

    /// SDL's GUID for the physical pad, from the raw name: SDL checksums what the kernel
    /// reports, and stripping e.g. the N64 adapter's leading 0x18 byte would change it.
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
            Source::Triton(_) => None,
        }
    }

    /// Discard whatever is queued. Left non-blocking: epoll only promises one read will not block.
    pub fn drain(&mut self) {
        let mut sink = Vec::new();
        let _ = self.set_nonblocking(true);
        while self.fetch_events(&mut sink).is_ok() && !sink.is_empty() {
            sink.clear();
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
pub fn open_source(pad: &Pad, grab: bool) -> Result<Source, CloneError> {
    let open_error = |error| CloneError::Open(pad.path.display().to_string(), error);
    if triton::owns(pad) {
        let source = triton::Source::open(&pad.path).map_err(open_error)?;
        return Ok(Source::Triton(Box::new(source)));
    }
    let mut source = Device::open(&pad.path).map_err(open_error)?;
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

/// A physical pad, its clone, and what passes between them.
#[derive(Debug)]
pub struct VirtualPad {
    pub player: u32,
    pub pad: Pad,
    /// Kept because the SDL GUID is computed from it.
    pub identity: Identity,
    pub source: Source,
    pub clone: VirtualDevice,
    /// ABS code -> calibration, applied as events pass through.
    pub axes: BTreeMap<u16, AxisCalibration>,
    /// Applied after calibration.
    pub tuning: Tuning,
    pub declared: BTreeMap<u16, Declared>,
    pub debouncer: Debouncer,
    /// The DSU picture, fed from the *corrected* stream so UDP and the clone agree.
    pub tracker: dsupad::Tracker,
    /// `None` for no gyro, a Steam Controller (motion rides in its reports), or a node that would not open.
    pub sensor: Option<motion::Sensor>,
    /// Counted, not logged per event: a repeating fault once filled a 3.1GB tmpfs with log lines.
    pub dropped: u64,
    pub gone: bool,
    /// Clone effect id -> the physical device's effect. Dropping an entry erases it upstream.
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
    tuning: Tuning,
    grab: bool,
) -> Result<VirtualPad, CloneError> {
    let source = open_source(pad, grab)?;
    let (identity, clone) = match &source {
        // No evdev device to read ids off: mirror what discovery read from sysfs, on BUS_USB,
        // because that is how it is attached and the bus is what SDL's database keys on.
        Source::Triton(triton) => {
            let version = version_for(player);
            let identity = match mode {
                IdentityMode::Padmap => Identity {
                    version,
                    ..Identity::PADMAP
                },
                IdentityMode::Mirror => Identity {
                    vendor: pad.vid,
                    product: pad.pid,
                    bustype: BusType::BUS_USB.0,
                    version,
                },
            };
            let (keys, axes) = triton.capabilities();
            (identity, build_clone_from(&keys, &axes, player, identity))
        }
        Source::Evdev(device) => {
            let identity = Identity::for_source(mode, device, player);
            (identity, build_clone(device, player, identity))
        }
    };
    let clone = clone.map_err(|error| CloneError::Build(player, error))?;

    // Re-checked here because a live measurement arrives without a round trip through the store.
    let mut axes = BTreeMap::new();
    for (code, calibration) in profile_axes {
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

    let declared = source.declared_axes();
    let tracker = dsupad::Tracker::new(
        declared
            .iter()
            .map(|(code, declared)| {
                (
                    *code,
                    dsupad::Range::declared(declared.minimum, declared.maximum, declared.value),
                )
            })
            .collect(),
    );
    let sensor = pad.motion.as_deref().and_then(|path| {
        motion::Sensor::open(path)
            .inspect_err(|error| {
                warn!(
                    "player {player}: motion sensor {} would not open ({error}); \
                 the pad works, its gyro does not",
                    path.display()
                );
            })
            .ok()
    });

    let mut vpad = VirtualPad {
        player,
        pad: pad.clone(),
        identity,
        source,
        clone,
        axes,
        debouncer: Debouncer::new(tuning.debounce_ms),
        tuning,
        declared,
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

fn build_clone(source: &Device, player: u32, identity: Identity) -> std::io::Result<VirtualDevice> {
    with_phys_retry(player, |set_phys| {
        assemble(source, player, identity, set_phys)
    })
}

/// A clone from a bare capability list, for a source the kernel never published a device for.
fn build_clone_from(
    keys: &[u16],
    axes: &[(u16, AbsInfo)],
    player: u32,
    identity: Identity,
) -> std::io::Result<VirtualDevice> {
    with_phys_retry(player, |set_phys| {
        let name = virtual_name(player);
        let key_set: AttributeSet<KeyCode> = keys.iter().map(|&code| KeyCode(code)).collect();
        let mut builder = head(&name, player, identity, set_phys)?.with_keys(&key_set)?;
        for &(code, info) in axes {
            builder =
                builder.with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisCode(code), info))?;
        }
        builder.build()
    })
}

/// evdev 0.13 encodes `UI_SET_PHYS` with a `c_char` payload where the kernel wants `char *`,
/// so the ioctl can be EINVAL; a clone without a phys is matched by name (`pad::is_padmap_clone`).
fn with_phys_retry(
    player: u32,
    build: impl Fn(bool) -> std::io::Result<VirtualDevice>,
) -> std::io::Result<VirtualDevice> {
    build(true).or_else(|first| {
        warn!(
            "player {player}: could not publish a clone with a phys tag ({first}); \
             retrying without one, so it is recognised by name instead"
        );
        build(false)
    })
}

/// Name, ids and (optionally) phys. The builder borrows the name, so the caller owns it.
fn head<'a>(
    name: &'a str,
    player: u32,
    identity: Identity,
    set_phys: bool,
) -> std::io::Result<VirtualDeviceBuilder<'a>> {
    let mut builder = VirtualDevice::builder()?.name(name).input_id(InputId::new(
        BusType(identity.bustype),
        identity.vendor,
        identity.product,
        identity.version,
    ));
    if set_phys {
        let phys = CString::new(virtual_phys(player)).unwrap_or_default();
        builder = builder.with_phys(&phys)?;
    }
    Ok(builder)
}

fn assemble(
    source: &Device,
    player: u32,
    identity: Identity,
    set_phys: bool,
) -> std::io::Result<VirtualDevice> {
    let name = virtual_name(player);
    let mut builder = head(&name, player, identity, set_phys)?;
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
    // Full absinfo, never bare codes: min = max = 0 is a divide by zero in RetroArch's
    // udev_compute_axis, and the game dies with SIGFPE at its first input poll.
    if let Ok(absinfo) = source.get_absinfo() {
        for (code, info) in absinfo {
            builder = builder.with_absolute_axis(&UinputAbsSetup::new(code, info))?;
        }
    }
    // Mirror the source's effect count: uinput's default of 96 advertises rumble it cannot play.
    if let Some(ff) = source.supported_ff() {
        builder = builder.with_ff(ff)?;
        builder = builder.with_ff_effects_max(source.max_ff_effects() as u32);
    }
    builder.build()
}

impl VirtualPad {
    /// `/dev/input/eventN` of the clone. The node appears a moment after creation: bounded wait.
    pub fn node(&mut self) -> Option<String> {
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

    /// Seed each calibrated axis at its centre, not the source's value: an adapter's absinfo
    /// can hold a stale power-on default (174 on a 0-255 axis centred at 128) until touched.
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

    /// Calibration, then tuning. A `None` may be a release the debouncer is holding for later.
    pub fn shape(&mut self, event: InputEvent, now_ms: u64) -> Option<InputEvent> {
        let event = self.correct(event);
        let code = event.code();
        let value = match event.event_type() {
            EventType::ABSOLUTE => {
                self.tuning
                    .shape_axis(code, event.value(), self.declared.get(&code))?
            }
            EventType::KEY => {
                if self.tuning.ignores_button(code) {
                    return None;
                }
                self.debouncer.key(code, event.value(), now_ms)?
            }
            _ => return Some(event),
        };
        Some(InputEvent::new(event.event_type().0, code, value))
    }

    /// Releases the debouncer has finished holding, as key-up events.
    pub fn due_releases(&mut self, now_ms: u64) -> Vec<InputEvent> {
        key_ups(self.debouncer.due(now_ms))
    }

    /// Every release still held, as key-up events.
    pub fn held_releases(&mut self) -> Vec<InputEvent> {
        key_ups(self.debouncer.drain())
    }

    /// Logged once: "no input reaches the game" has two causes this line tells apart.
    pub fn note_forwarding(&mut self) {
        if !self.forwarded_any {
            self.forwarded_any = true;
            info!("player {}: forwarding input to the clone", self.player);
        }
    }

    pub fn note_dropped(&mut self, error: &std::io::Error) {
        self.dropped += 1;
        if self.dropped == 1 {
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

    /// Re-upload a game's effect to the real pad, which allocates its own id.
    pub fn proxy_upload(&mut self, event: UInputEvent) {
        let player = self.player;
        let Ok(mut upload) = self
            .clone
            .process_ff_upload(event)
            .inspect_err(|error| warn!("player {player}: effect upload could not begin: {error}"))
        else {
            return;
        };
        let virtual_id = upload.effect_id();
        match self.source.upload_ff_effect(upload.effect()) {
            Ok(effect) => {
                self.effects.insert(virtual_id, effect);
                upload.set_retval(0);
            }
            Err(error) => {
                warn!("player {player}: effect upload failed: {error}");
                upload.set_retval(-1);
            }
        }
    }

    /// Dropping our handle erases the effect upstream too.
    pub fn proxy_erase(&mut self, event: UInputEvent) {
        let player = self.player;
        let Ok(mut erase) = self
            .clone
            .process_ff_erase(event)
            .inspect_err(|error| warn!("player {player}: effect erase could not begin: {error}"))
        else {
            return;
        };
        self.effects.remove(&(erase.effect_id() as i16));
        erase.set_retval(0);
    }

    pub fn play(&mut self, virtual_id: u16, count: i32) {
        let Some(effect) = self.effects.get_mut(&(virtual_id as i16)) else {
            return;
        };
        if let Err(error) = effect.play(count) {
            // Debug: a game may drive rumble continuously at a pad that has been unplugged.
            log::debug!("player {}: rumble write failed: {error}", self.player);
        }
    }
}

/// Key codes and absolute axis codes (hat included) a pad reports.
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

/// Travel and rest value per axis; rest is what separates a stick from a trigger on ABS_RX.
pub fn axis_spans(source: &Device) -> BTreeMap<u16, AxisSpan> {
    let Ok(absinfo) = source.get_absinfo() else {
        return BTreeMap::new();
    };
    absinfo
        .map(|(code, info)| (code.0, span(&info)))
        .collect()
}

pub fn held_keys(source: &Device) -> AttributeSet<evdev::KeyCode> {
    source.get_key_state().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clone_is_named_and_physed_predictably() {
        assert_eq!(virtual_name(1), "padmap Player 1");
        assert_eq!(virtual_name(4), "padmap Player 4");
        assert_eq!(virtual_phys(1), "padmap/p1");
        assert!(virtual_name(2).starts_with(VIRTUAL_PREFIX));
    }

    #[test]
    fn identity_mode_defaults_to_mirroring() {
        assert_eq!(IdentityMode::Mirror.as_str(), "mirror");
        assert_eq!(IdentityMode::Padmap.as_str(), "padmap");
    }

    #[test]
    fn padmaps_own_identity_is_on_the_virtual_bus() {
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
        for kind in [
            EventType::FORCEFEEDBACK,
            EventType::FORCEFEEDBACKSTATUS,
            EventType::LED,
        ] {
            assert!(!forwarded(kind), "{kind:?} must not be forwarded");
        }
    }
}

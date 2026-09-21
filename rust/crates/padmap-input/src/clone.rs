//! Clone a physical pad as `padmap Player N` with unique identity per port.

use std::collections::BTreeMap;
use std::ffi::CString;

use evdev::uinput::{VirtualDevice, VirtualDeviceBuilder};
use evdev::{
    AbsInfo, AbsoluteAxisCode, AttributeSet, BusType, Device, EventType, FFEffect, InputEvent,
    InputId, KeyCode, UInputEvent, UinputAbsSetup,
};
use log::{info, warn};
use padmap_core::binding::Binding;
use padmap_core::calibration::{AxisCalibration, Declared};
use padmap_core::control::Control;
use padmap_core::dsupad;
use padmap_core::emit::version_for;
use padmap_core::sdl::AxisSpan;
use padmap_core::tuning::{Debouncer, Tuning};
use padmap_core::xbox;

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

/// Mirror: keep source's bus/ids; Padmap: use 1209:0001 on BUS_VIRTUAL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityMode {
    Mirror,
    Padmap,
    /// A wired Xbox 360 pad, layout and all: what every SDL maps out of the box.
    Xbox360,
}

impl IdentityMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            IdentityMode::Mirror => "mirror",
            IdentityMode::Padmap => "padmap",
            IdentityMode::Xbox360 => "xbox360",
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
            "xbox360" => IdentityMode::Xbox360,
            _ if std::env::var(ENV_ONLY_VIRTUAL).as_deref() == Ok("1") => IdentityMode::Padmap,
            _ => IdentityMode::Mirror,
        }
    }
}

/// Four fields for SDL GUID; bus field decides match in SDL database.
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

    /// The wired 360 pad's, version and all: SDL's GUID carries the version,
    /// and the database entry is for 0x0110. The player lives in phys instead.
    pub const XBOX360: Identity = Identity {
        vendor: xbox::VENDOR,
        product: xbox::PRODUCT,
        bustype: xbox::BUS_USB,
        version: xbox::VERSION,
    };

    pub fn for_source(mode: IdentityMode, source: &Device, player: u32) -> Identity {
        let version = version_for(player);
        match mode {
            IdentityMode::Xbox360 => Identity::XBOX360,
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

/// Event source: evdev node or Triton (Steam Controller).
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
    pub fn fetch_events(&mut self, out: &mut Vec<InputEvent>) -> std::io::Result<()> {
        match self {
            Source::Evdev(device) => {
                out.extend(device.fetch_events()?);
                Ok(())
            }
            Source::Triton(source) => source.fetch_events(out),
        }
    }

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
                .map(|axes| axes.map(|(code, info)| (code.0, declared(&info))).collect())
                .unwrap_or_default(),
            Source::Triton(source) => source
                .capabilities()
                .1
                .into_iter()
                .map(|(code, info)| (code, declared(&info)))
                .collect(),
        }
    }

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

/// Physical pad, clone, and event pipeline.
#[derive(Debug)]
pub struct VirtualPad {
    pub player: u32,
    pub pad: Pad,
    pub identity: Identity,
    pub source: Source,
    pub clone: VirtualDevice,
    pub axes: BTreeMap<u16, AxisCalibration>,
    pub tuning: Tuning,
    pub declared: BTreeMap<u16, Declared>,
    pub debouncer: Debouncer,
    pub tracker: dsupad::Tracker,
    pub sensor: Option<motion::Sensor>,
    pub dropped: u64,
    pub gone: bool,
    effects: BTreeMap<i16, FFEffect>,
    forwarded_any: bool,
    /// Under `IdentityMode::Xbox360`: the source's events onto the 360 layout.
    pub translator: Option<xbox::Translator>,
}

#[derive(Debug, thiserror::Error)]
pub enum CloneError {
    #[error("opening {0}: {1}")]
    Open(String, #[source] std::io::Error),
    #[error("building a clone for player {0}: {1}")]
    Build(u32, #[source] std::io::Error),
}

/// Grab a pad and publish its clone.
/// `bindings` is the pad's stored capture, used only to translate it onto the
/// 360 layout under `IdentityMode::Xbox360`; empty for an unmapped pad.
pub fn create(
    pad: &Pad,
    player: u32,
    mode: IdentityMode,
    profile_axes: &BTreeMap<u16, AxisCalibration>,
    tuning: Tuning,
    grab: bool,
    bindings: &BTreeMap<Control, Binding>,
) -> Result<VirtualPad, CloneError> {
    let source = open_source(pad, grab)?;
    let mut translator = None;
    let (identity, clone) = if mode == IdentityMode::Xbox360 {
        let (keys, _) = source.capabilities();
        translator = Some(xbox::Translator::new(&keys, &source.axis_spans(), bindings));
        let axes: Vec<(u16, AbsInfo)> = xbox::AXES
            .iter()
            .map(|&(code, minimum, maximum, fuzz, flat)| {
                let rest = if minimum < 0 { 0 } else { minimum };
                (code, AbsInfo::new(rest, minimum, maximum, fuzz, flat, 0))
            })
            .collect();
        (
            Identity::XBOX360,
            build_clone_from(&xbox::KEYS, &axes, player, Identity::XBOX360),
        )
    } else {
        match &source {
            Source::Triton(triton) => {
                let version = version_for(player);
                let identity = match mode {
                    IdentityMode::Padmap => Identity {
                        version,
                        ..Identity::PADMAP
                    },
                    IdentityMode::Mirror | IdentityMode::Xbox360 => Identity {
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
        }
    };
    let clone = clone.map_err(|error| CloneError::Build(player, error))?;

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
        translator,
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

/// evdev 0.13 may fail UI_SET_PHYS; retry without phys (matched by name instead).
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
    if let Ok(absinfo) = source.get_absinfo() {
        for (code, info) in absinfo {
            builder = builder.with_absolute_axis(&UinputAbsSetup::new(code, info))?;
        }
    }
    // Mirror source's effect count: default 96 would falsely advertise rumble.
    if let Some(ff) = source.supported_ff() {
        builder = builder.with_ff(ff)?;
        builder = builder.with_ff_effects_max(source.max_ff_effects() as u32);
    }
    builder.build()
}

impl VirtualPad {
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

    /// Seed calibrated axes at centre, not source default (may be stale).
    fn seed_calibrated_axes(&mut self) {
        if self.axes.is_empty() || self.translator.is_some() {
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

    /// Calibration then tuning; None may be debounced release.
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

    /// What the clone is given for one shaped source event: itself, or its
    /// translation onto the 360 layout.
    pub fn outgoing(&mut self, event: InputEvent) -> Vec<InputEvent> {
        match self.translator.as_mut() {
            None => vec![event],
            Some(translator) => translator
                .translate(event.event_type().0, event.code(), event.value())
                .into_iter()
                .map(|out| InputEvent::new(out.kind, out.code, out.value))
                .collect(),
        }
    }

    /// Every event the clone needs to read "nothing held", in its own codes.
    pub fn outgoing_release_all(&mut self) -> Vec<InputEvent> {
        match self.translator.as_mut() {
            None => Vec::new(),
            Some(translator) => translator
                .release_all()
                .into_iter()
                .map(|out| InputEvent::new(out.kind, out.code, out.value))
                .collect(),
        }
    }

    pub fn due_releases(&mut self, now_ms: u64) -> Vec<InputEvent> {
        key_ups(self.debouncer.due(now_ms))
    }

    pub fn held_releases(&mut self) -> Vec<InputEvent> {
        key_ups(self.debouncer.drain())
    }

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
            log::debug!("player {}: rumble write failed: {error}", self.player);
        }
    }
}

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

pub fn axis_spans(source: &Device) -> BTreeMap<u16, AxisSpan> {
    let Ok(absinfo) = source.get_absinfo() else {
        return BTreeMap::new();
    };
    absinfo.map(|(code, info)| (code.0, span(&info))).collect()
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

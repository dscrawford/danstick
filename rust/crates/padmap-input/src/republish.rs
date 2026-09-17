//! Pump events between physical pads and their clones.
//!
//! Two directions. Presses travel source -> clone; force feedback travels the
//! other way, because the uinput node is what a game uploads effects to and
//! only the physical device can play them.
//!
//! Forwarding is done a *frame* at a time. The kernel separates packets of
//! simultaneous changes with `SYN_REPORT`, and half a frame is not a smaller
//! truth -- publishing `ABS_X, SYN_REPORT` when the source said `ABS_X, ABS_Y,
//! SYN_REPORT` turns a diagonal into an axis-aligned move. So a frame is
//! written whole, in one `write(2)`, and a trailing partial frame is held until
//! its terminator arrives. That also happens to be the cheap way round: the
//! Python issued two syscalls *per event* (python-evdev does an `fcntl` before
//! every write), where this issues one per frame.

use std::io::ErrorKind;

use evdev::{EventType, InputEvent};
use log::warn;

use crate::clone::{forwarded, VirtualPad};

/// Frames larger than this do not come off a gamepad; the buffer grows if one
/// ever does, and this only decides where it starts.
const FRAME_HINT: usize = 64;

/// What a pump found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pumped {
    pub frames: usize,
    pub events: usize,
    /// The source returned ENODEV: the pad is gone and this clone is finished.
    pub gone: bool,
}

/// Pumps events between physical and virtual pads until stopped.
#[derive(Debug)]
pub struct Republisher {
    pub pads: Vec<VirtualPad>,
    paused: bool,
    /// Reused between calls so the hot path allocates nothing.
    frame: Vec<InputEvent>,
    pending: Vec<InputEvent>,
}

impl Republisher {
    pub fn new(pads: Vec<VirtualPad>) -> Self {
        Republisher {
            pads,
            paused: false,
            frame: Vec::with_capacity(FRAME_HINT),
            pending: Vec::with_capacity(FRAME_HINT),
        }
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    /// Stop or resume forwarding presses to the clones.
    ///
    /// A mapping or calibration wizard reads the *physical* pad directly, and
    /// the daemon holds EVIOCGRAB so the front-end cannot see it. That is only
    /// half the story: the same pad is also being republished, and the clone is
    /// exactly what the front-end *does* watch. So every press the wizard asked
    /// for was also delivered to the UI, and a wizard that says "press B" had
    /// its answer read as "go back" -- the step cancelled itself with the button
    /// it requested.
    ///
    /// Pausing rather than stopping, because stopping destroys the uinput nodes:
    /// the front-end would see every controller disconnect when the wizard
    /// opened and reappear when it closed, which reshuffles SDL's joystick
    /// indices mid-configuration.
    ///
    /// Sources are still drained while paused. An unread evdev node does not go
    /// quiet, it fills, and the backlog would arrive in a burst the moment the
    /// wizard closed.
    pub fn set_paused(&mut self, paused: bool) {
        if paused == self.paused {
            return;
        }
        self.paused = paused;
        if paused {
            self.release_all();
        }
    }

    /// Let go of anything held, on the clone, before we stop forwarding.
    ///
    /// Otherwise a button held as the wizard opens stays held forever: the press
    /// was forwarded, the release lands during the pause and is dropped, and the
    /// clone is left with a key that is down with nothing to lift it. That is
    /// the shape of the stuck-input bug that made exiting a game immediately
    /// launch another one.
    fn release_all(&mut self) {
        for vpad in &mut self.pads {
            let held = vpad.source.held_keys();
            let mut frame: Vec<InputEvent> = held
                .iter()
                .map(|&code| InputEvent::new(EventType::KEY.0, code, 0))
                .collect();
            if frame.is_empty() {
                continue;
            }
            frame.push(InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0));
            if let Err(error) = vpad.clone.emit(&frame) {
                warn!(
                    "player {}: could not release held keys: {error}",
                    vpad.player
                );
            }
        }
    }

    /// Service one pad reported readable, forwarding whole frames.
    pub fn forward(&mut self, index: usize) -> Pumped {
        let mut out = Pumped::default();
        let Some(vpad) = self.pads.get_mut(index) else {
            return out;
        };
        if vpad.gone {
            return out;
        }

        self.pending.clear();
        match vpad.source.fetch_events(&mut self.pending) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::WouldBlock => return out,
            Err(error) => {
                // ENODEV, once, and then never again for this pad.
                //
                // The Python logged on every call and a dead node stays
                // readable forever, so the selector woke on it continuously and
                // wrote a line each time: 114 million lines, 3.1GB, the whole
                // tmpfs, and then everything that needed to write there failed.
                // The visible symptom was a game exiting instantly with code 1,
                // which points nowhere near a disconnected controller.
                warn!(
                    "player {}: source disappeared ({error}), dropping the clone",
                    vpad.player
                );
                vpad.gone = true;
                out.gone = true;
                return out;
            }
        }
        out.events = self.pending.len();

        // Drained above whether or not we are paused, then discarded here --
        // that is the whole point of the pause.
        if self.paused {
            return out;
        }

        self.frame.clear();
        let mut emitted_any = false;
        for event in self.pending.drain(..) {
            if !forwarded(event.event_type()) {
                continue;
            }
            let corrected = vpad.correct(event);
            // The DSU picture is fed the corrected event, not the raw one, so
            // a consumer reading padmap over UDP sees the same calibrated
            // stick as one reading the clone.
            vpad.tracker.apply(
                corrected.event_type().0,
                corrected.code(),
                corrected.value(),
            );
            self.frame.push(corrected);
            if event.event_type() == EventType::SYNCHRONIZATION {
                // A whole packet, and never less than one. Writing a partial
                // frame publishes a torn reading -- a diagonal as an
                // axis-aligned move -- which no consumer can detect.
                match vpad.clone.emit(&self.frame) {
                    Ok(()) => {
                        out.frames += 1;
                        emitted_any = true;
                    }
                    Err(error) => vpad.note_dropped(&error),
                }
                self.frame.clear();
            }
        }
        // Whatever arrived after the last terminator waits for the next read.
        // The kernel never splits a frame across a read boundary, so this is
        // only non-empty when the device genuinely has more to say.
        self.pending.append(&mut self.frame);
        if emitted_any {
            vpad.note_forwarding();
        }
        // A Steam Controller's gyro rides in the reports just drained, so it
        // is folded in here rather than on a descriptor of its own.
        if let Some(sample) = vpad.source.motion() {
            vpad.tracker.set_motion(sample);
        }
        out
    }

    /// Service a motion sensor reported readable.
    ///
    /// Separate from `forward` because it is a separate descriptor: a gyro
    /// node emits at its own rate, faster than the buttons and independently
    /// of them, and a pad held perfectly still still reports gravity.
    pub fn read_motion(&mut self, index: usize) -> Pumped {
        let mut out = Pumped::default();
        let Some(vpad) = self.pads.get_mut(index) else {
            return out;
        };
        let Some(sensor) = vpad.sensor.as_mut() else {
            return out;
        };
        match sensor.read() {
            Ok(true) => {}
            Ok(false) => {
                // The sensor is gone. Not fatal to the pad: the buttons live
                // on a different node and may well still be there, and a
                // controller that works without its gyro beats one that
                // disappears.
                warn!(
                    "player {}: motion sensor disappeared, dropping it",
                    vpad.player
                );
                vpad.sensor = None;
                return out;
            }
            Err(error) => {
                warn!("player {}: motion sensor: {error}", vpad.player);
                return out;
            }
        }
        if let Some(sample) = vpad.sensor.as_ref().and_then(crate::motion::Sensor::motion) {
            vpad.tracker.set_motion(sample);
            out.frames = 1;
        }
        out
    }

    /// Proxy rumble from a clone back to its physical pad.
    ///
    /// Only an evdev source can play an effect. A pad read over hidraw has no
    /// EVIOCSFF, and saying so here is the point of returning a result rather
    /// than swallowing it: the Python called `source.write(...)` unconditionally
    /// inside an `except OSError`, and on a hidraw source that is an
    /// `AttributeError` -- not an `OSError` -- which would have escaped the
    /// selector loop and ended the daemon. It was unreachable only because the
    /// clone was built with no EV_FF, so the kernel never delivered one.
    pub fn feedback(&mut self, index: usize) {
        let Some(vpad) = self.pads.get_mut(index) else {
            return;
        };
        let events: Vec<InputEvent> = match vpad.clone.fetch_events() {
            Ok(events) => events.collect(),
            Err(_) => return,
        };
        for event in events {
            match event.destructure() {
                evdev::EventSummary::UInput(uinput, code, _) => match code {
                    evdev::UInputCode::UI_FF_UPLOAD => vpad.proxy_upload(uinput),
                    evdev::UInputCode::UI_FF_ERASE => vpad.proxy_erase(uinput),
                    _ => {}
                },
                evdev::EventSummary::ForceFeedback(_, code, value) => {
                    vpad.play(code.0, value);
                }
                _ => {}
            }
        }
    }

    /// Source descriptors whose pad has gone, for the caller to unregister.
    ///
    /// The republisher cannot do it itself -- the epoll set belongs to the loop.
    /// Left registered, a dead node reports readable forever and the loop spins
    /// on it for as long as the daemon runs.
    pub fn dead(&self) -> Vec<usize> {
        self.pads
            .iter()
            .enumerate()
            .filter(|(_, vpad)| vpad.gone)
            .map(|(index, _)| index)
            .collect()
    }

    pub fn close(&mut self) {
        for vpad in &mut self.pads {
            vpad.release();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_is_everything_up_to_and_including_its_terminator() {
        // Not a behaviour test of the pump -- that needs a device -- but of the
        // rule it implements, stated where a reader will look for it.
        let frame = [
            InputEvent::new(EventType::ABSOLUTE.0, 0, 10),
            InputEvent::new(EventType::ABSOLUTE.0, 1, 20),
            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
        ];
        assert_eq!(
            frame.last().map(|e| e.event_type()),
            Some(EventType::SYNCHRONIZATION)
        );
        assert!(frame.iter().all(|event| forwarded(event.event_type())));
    }

    #[test]
    fn a_pumped_report_starts_empty() {
        let empty = Pumped::default();
        assert_eq!(empty.frames, 0);
        assert_eq!(empty.events, 0);
        assert!(!empty.gone);
    }
}

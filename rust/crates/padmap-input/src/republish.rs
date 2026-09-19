//! Pump events between physical pads and their clones (presses source->clone, force feedback reverse).

use std::io::ErrorKind;
use std::time::Instant;

use evdev::{EventType, InputEvent};
use log::warn;
use padmap_core::tuning::{Debouncer, Tuning};

use crate::clone::{forwarded, VirtualPad};

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
    /// Per pad: its presses are being read by a modal flow, not forwarded.
    held_back: Vec<bool>,
    frame: Vec<InputEvent>,
    pending: Vec<InputEvent>,
    started: Instant,
}

impl Republisher {
    pub fn new(pads: Vec<VirtualPad>) -> Self {
        Republisher {
            held_back: vec![false; pads.len()],
            pads,
            paused: false,
            frame: Vec::with_capacity(FRAME_HINT),
            pending: Vec::with_capacity(FRAME_HINT),
            started: Instant::now(),
        }
    }

    fn now_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    /// Stop or resume forwarding presses.
    pub fn set_paused(&mut self, paused: bool) {
        if paused == self.paused {
            return;
        }
        self.paused = paused;
        if paused {
            self.release_all();
        }
    }

    pub fn held_back(&self, index: usize) -> bool {
        self.held_back.get(index).copied().unwrap_or(false)
    }

    /// Keep one pad's presses from its clone while a modal flow reads them; everyone else plays on.
    pub fn hold_back(&mut self, index: usize, held: bool) {
        let Some(slot) = self.held_back.get_mut(index) else {
            return;
        };
        if *slot == held {
            return;
        }
        *slot = held;
        if held {
            self.release_pad(index);
        }
    }

    /// Release all held keys to avoid stuck input when pause begins.
    fn release_all(&mut self) {
        for index in 0..self.pads.len() {
            self.release_pad(index);
        }
    }

    fn release_pad(&mut self, index: usize) {
        let Some(vpad) = self.pads.get_mut(index) else {
            return;
        };
        vpad.tracker.release_all();
        let held = vpad.source.held_keys();
        let mut frame: Vec<InputEvent> = held
            .iter()
            .map(|&code| InputEvent::new(EventType::KEY.0, code, 0))
            .collect();
        frame.extend(vpad.held_releases());
        if frame.is_empty() {
            return;
        }
        frame.push(InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0));
        if let Err(error) = vpad.clone.emit(&frame) {
            warn!(
                "player {}: could not release held keys: {error}",
                vpad.player
            );
        }
    }

    /// Service one pad reported readable, forwarding whole frames.
    pub fn forward(&mut self, index: usize) -> Pumped {
        let mut out = Pumped::default();
        let held_back = self.held_back(index);
        let Some(vpad) = self.pads.get_mut(index) else {
            return out;
        };
        if vpad.gone {
            return out;
        }

        let before = self.pending.len();
        match vpad.source.fetch_events(&mut self.pending) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::WouldBlock => return out,
            Err(error) => {
                warn!(
                    "player {}: source disappeared ({error}), dropping the clone",
                    vpad.player
                );
                vpad.gone = true;
                out.gone = true;
                return out;
            }
        }
        out.events = self.pending.len() - before;

        if self.paused || held_back {
            self.pending.clear();
            return out;
        }

        self.frame.clear();
        let mut emitted_any = false;
        let now_ms = self.started.elapsed().as_millis() as u64;
        let complete = self
            .pending
            .iter()
            .rposition(|event| event.event_type() == EventType::SYNCHRONIZATION)
            .map(|at| at + 1)
            .unwrap_or(0);
        for event in self.pending.drain(..complete) {
            if !forwarded(event.event_type()) {
                continue;
            }
            if event.event_type() == EventType::SYNCHRONIZATION {
                if self.frame.is_empty() {
                    continue;
                }
                self.frame.push(event);
                match vpad.clone.emit(&self.frame) {
                    Ok(()) => {
                        out.frames += 1;
                        emitted_any = true;
                    }
                    Err(error) => vpad.note_dropped(&error),
                }
                self.frame.clear();
                continue;
            }
            let Some(shaped) = vpad.shape(event, now_ms) else {
                continue;
            };
            vpad.tracker
                .apply(shaped.event_type().0, shaped.code(), shaped.value());
            self.frame.push(shaped);
        }
        debug_assert!(self.frame.is_empty());
        if emitted_any {
            vpad.note_forwarding();
        }
        if let Some(sample) = vpad.source.motion() {
            vpad.tracker.set_motion(sample);
        }
        out
    }

    /// Service a motion sensor (separate descriptor emitting at its own rate).
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
                out.gone = true;
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

    /// Change one pad's tuning without rebuilding its clone (keeps uinput node intact).
    pub fn retune(&mut self, index: usize, tuning: Tuning) {
        let Some(vpad) = self.pads.get_mut(index) else {
            return;
        };
        let mut frame = vpad.held_releases();
        if !frame.is_empty() && !self.paused && !self.held_back.get(index).copied().unwrap_or(false)
        {
            for event in &frame {
                vpad.tracker.apply(event.event_type().0, event.code(), 0);
            }
            frame.push(InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0));
            if let Err(error) = vpad.clone.emit(&frame) {
                vpad.note_dropped(&error);
            }
        }
        vpad.debouncer = Debouncer::new(tuning.debounce_ms);
        vpad.tuning = tuning;
    }

    /// Deliver every release the debouncers have finished holding.
    pub fn flush_debounce(&mut self) {
        if self.paused {
            return;
        }
        let now_ms = self.now_ms();
        for (index, vpad) in self.pads.iter_mut().enumerate() {
            if vpad.gone || self.held_back.get(index).copied().unwrap_or(false) {
                continue;
            }
            let mut frame = vpad.due_releases(now_ms);
            if frame.is_empty() {
                continue;
            }
            for event in &frame {
                vpad.tracker.apply(event.event_type().0, event.code(), 0);
            }
            frame.push(InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0));
            if let Err(error) = vpad.clone.emit(&frame) {
                vpad.note_dropped(&error);
            }
        }
    }

    /// Let go of a sensor whose device has gone (buttons live on different node).
    pub fn drop_sensor(&mut self, index: usize) {
        if let Some(vpad) = self.pads.get_mut(index) {
            if vpad.sensor.take().is_some() {
                warn!(
                    "player {}: motion sensor disappeared, dropping it",
                    vpad.player
                );
            }
        }
    }

    /// Proxy rumble from a clone back to its physical pad (only evdev sources support EVIOCSFF).
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

    /// Source descriptors whose pad has gone (for caller to unregister from epoll).
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

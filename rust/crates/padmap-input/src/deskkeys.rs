//! The desk's keyboards, read for a held space bar and never grabbed.
//!
//! Raw, not `evdev::Device`: the synchronizing one inserts fake events after a
//! `SYN_DROPPED`, which here would be a space bar somebody was merely still
//! holding arriving as a fresh press and taking a seat nobody asked for.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use evdev::raw_stream::RawDevice;
use log::{debug, info, warn};

/// More keyboards than a desk has: past this, something is manufacturing them,
/// and an fd each would surface as the daemon failing somewhere unrelated.
const MAX_KEYBOARDS: usize = 32;

/// Every keyboard with a space bar except the ones `excluded` already owns.
///
/// A pad in lizard mode publishes keyboards of its own, already grabbed beside
/// the pad; reading them here too would let a trackpad click bound to space
/// seat "the keyboard".
fn wanted(nodes: &[PathBuf], excluded: &BTreeSet<PathBuf>) -> BTreeSet<PathBuf> {
    let all: BTreeSet<PathBuf> = nodes
        .iter()
        .filter(|path| !excluded.contains(*path))
        .cloned()
        .collect();
    if all.len() > MAX_KEYBOARDS {
        warn!(
            "{} keyboards on the machine; reading the first {MAX_KEYBOARDS}",
            all.len()
        );
        return all.into_iter().take(MAX_KEYBOARDS).collect();
    }
    all
}

/// A keyboard's identity for a hold, stable while its node is.
fn device_id(path: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::hash::DefaultHasher::new();
    path.hash(&mut hasher);
    hasher.finish()
}

/// Keyboards held open for reading. Nothing here is grabbed: the space bar
/// reaches the game and the desktop as it always did.
#[derive(Debug, Default)]
pub struct Keyboards {
    open: BTreeMap<PathBuf, RawDevice>,
    buffer: Vec<evdev::InputEvent>,
    /// Whether each node seen so far has a space bar, so a hotplug opens the
    /// one new node rather than all thirteen key-ish ones again.
    probed: BTreeMap<PathBuf, bool>,
}

impl Keyboards {
    pub fn paths(&self) -> Vec<&Path> {
        self.open.keys().map(PathBuf::as_path).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.open.is_empty()
    }

    /// Find every keyboard with a space bar and read all of them but the ones
    /// `excluded` already owns.
    ///
    /// udev's own `ID_INPUT_KEY` narrows 28 nodes to 13 on this desk before
    /// anything is opened, which matters because opening an evdev node and
    /// closing it again costs about 10ms. `ID_INPUT_KEYBOARD` would be
    /// narrower still and is wrong: a real Bluetooth keyboard here carries
    /// `ID_INPUT_KEY` alone, so the space bar is confirmed by asking the
    /// device -- once per node, remembered after that.
    pub fn refresh(&mut self, excluded: &BTreeSet<PathBuf>) {
        let Ok(mut enumerator) = udev::Enumerator::new() else {
            return;
        };
        if enumerator.match_subsystem("input").is_err() {
            return;
        }
        let Ok(devices) = enumerator.scan_devices() else {
            return;
        };
        let only = std::env::var(crate::pad::ENV_ONLY).unwrap_or_default();
        let mut present: BTreeSet<PathBuf> = BTreeSet::new();
        let mut found: Vec<PathBuf> = Vec::new();
        for device in devices {
            let Some(path) = device.devnode().map(Path::to_path_buf) else {
                continue;
            };
            if !path.to_string_lossy().starts_with("/dev/input/event") {
                continue;
            }
            let keyish = device
                .property_value("ID_INPUT_KEY")
                .is_some_and(|value| value.to_string_lossy() == "1");
            if !keyish {
                continue;
            }
            if !only.is_empty() {
                let parent = device.parent();
                let owner = parent.as_ref().unwrap_or(&device);
                let name = crate::pad::attribute(owner, "name").unwrap_or_default();
                if !name.contains(&only) {
                    continue;
                }
            }
            present.insert(path.clone());
            let has_space = match self.probed.get(&path) {
                Some(known) => *known,
                None => {
                    let answer = evdev::Device::open(&path).is_ok_and(|device| {
                        device.supported_keys().is_some_and(|keys| {
                            keys.contains(evdev::KeyCode::new(padmap_core::keyboard::SEAT_KEY))
                        })
                    });
                    self.probed.insert(path.clone(), answer);
                    answer
                }
            };
            if has_space {
                found.push(path);
            }
        }
        self.probed.retain(|path, _| present.contains(path));
        self.sync(&wanted(&found, excluded));
    }

    /// Read exactly `wanted`: open what is new, drop what is not wanted.
    pub fn sync(&mut self, wanted: &BTreeSet<PathBuf>) {
        let stale: Vec<PathBuf> = self
            .open
            .keys()
            .filter(|path| !wanted.contains(*path))
            .cloned()
            .collect();
        for path in stale {
            self.open.remove(&path);
            debug!("stopped reading {}", path.display());
        }
        for path in wanted {
            if self.open.contains_key(path) {
                continue;
            }
            // Non-blocking at open, or the whole tick stalls here on a keystroke.
            let opened = rustix::fs::open(
                path,
                rustix::fs::OFlags::RDONLY
                    | rustix::fs::OFlags::NONBLOCK
                    | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::empty(),
            );
            let fd = match opened {
                Ok(fd) => fd,
                Err(error) => {
                    debug!("could not open {} to read it: {error}", path.display());
                    continue;
                }
            };
            match RawDevice::from_fd(fd) {
                Ok(device) => {
                    info!(
                        "reading {} ({}) for a held space bar; not grabbed",
                        path.display(),
                        device.name().unwrap_or("?")
                    );
                    self.open.insert(path.clone(), device);
                }
                Err(error) => debug!("could not read {}: {error}", path.display()),
            }
        }
    }

    /// Offer every pending event to `feed`, told which keyboard it came from
    /// and how long ago; a keyboard that has gone is dropped.
    pub fn drain(&mut self, mut feed: impl FnMut(u64, u16, u16, i32, f64)) {
        let mut gone: Vec<PathBuf> = Vec::new();
        for (path, device) in self.open.iter_mut() {
            match device.fetch_events() {
                Ok(events) => {
                    self.buffer.clear();
                    self.buffer.extend(events);
                    for event in &self.buffer {
                        feed(
                            device_id(path),
                            event.event_type().0,
                            event.code(),
                            event.value(),
                            crate::clone::event_age(event),
                        );
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => {
                    debug!("{} stopped reading: {error}", path.display());
                    gone.push(path.clone());
                }
            }
        }
        for path in gone {
            self.open.remove(&path);
        }
    }

    pub fn close_all(&mut self) {
        self.sync(&BTreeSet::new());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(path: &str) -> PathBuf {
        PathBuf::from(path)
    }

    #[test]
    fn a_pads_own_keyboards_are_left_to_the_pad() {
        let nodes = [node("/dev/input/event1"), node("/dev/input/event3")];
        let held: BTreeSet<PathBuf> = [node("/dev/input/event3")].into_iter().collect();
        assert_eq!(
            wanted(&nodes, &held),
            [node("/dev/input/event1")].into_iter().collect(),
            "a node the pad already holds is not ours to read"
        );
    }

    #[test]
    fn nothing_is_wanted_when_there_are_no_keyboards() {
        assert!(wanted(&[], &BTreeSet::new()).is_empty());
    }
}

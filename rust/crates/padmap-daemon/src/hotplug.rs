//! Noticing controllers arriving and leaving, cheaply.
//!
//! The daemon's tick runs fifty times a second on the thread that forwards
//! controller events, and a full device scan reads several files per pad.
//! Everything here exists to avoid doing that scan unless something changed:
//! a directory listing costs a fraction of a millisecond and answers "has
//! anything appeared or gone away", which is the only question the tick has.

use std::collections::{BTreeMap, BTreeSet};

/// Names of the evdev nodes that exist right now.
pub fn event_nodes() -> BTreeSet<String> {
    let Ok(entries) = std::fs::read_dir("/dev/input") else {
        return BTreeSet::new();
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .filter(|name| name.starts_with("event"))
        .collect()
}

/// How many attempts a controller gets at being opened.
///
/// A device node exists before it is readable: udev applies the uaccess ACL
/// *after* the node appears, so the first open of a freshly plugged
/// controller can fail with EACCES and succeed a fraction of a second later.
/// Without a retry the controller is lost until it is replugged; without a
/// *bounded* one, a node that genuinely cannot be opened rescans at tick rate.
pub const ATTACH_ATTEMPTS: u32 = 20;
/// How often an arrival scan may run while a controller is being retried.
pub const ATTACH_SCAN_SECONDS: f64 = 0.25;

/// Which controllers are live, and which could not be.
#[derive(Debug, Default, Clone)]
pub struct Attached {
    /// signature -> player, for every controller announced as live. Not
    /// derived from the assignments: a pad stays assigned while it is
    /// unplugged, so a reconnecting controller gets its slot back, and this
    /// is the narrower question of what is attached *now*.
    pub live: BTreeMap<String, u32>,
    /// signature -> failed opens so far.
    pub attempts: BTreeMap<String, u32>,
    /// Controllers that ran out of attempts. Kept so the scan a third device
    /// triggers does not start the whole cycle again.
    pub unbindable: BTreeSet<String>,
}

/// What one scan found, given what is present now.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Changes {
    /// Signatures that were live and are gone, in order.
    pub departed: Vec<String>,
    /// Signatures present now that were not live, in order, excluding ones
    /// given up on.
    pub arrived: Vec<String>,
}

impl Attached {
    /// Compare what is present against what is live.
    ///
    /// A controller that has gone gets its attempts back, so replugging a pad
    /// that could not be opened is always worth doing.
    pub fn diff(&mut self, present: &BTreeSet<String>) -> Changes {
        let departed: Vec<String> = self
            .live
            .keys()
            .filter(|signature| !present.contains(*signature))
            .cloned()
            .collect();
        self.attempts
            .retain(|signature, _| present.contains(signature));
        self.unbindable
            .retain(|signature| present.contains(signature));
        let arrived: Vec<String> = present
            .iter()
            .filter(|signature| !self.live.contains_key(*signature))
            .filter(|signature| !self.unbindable.contains(*signature))
            .cloned()
            .collect();
        Changes { departed, arrived }
    }

    /// One more failed open. True if the controller should be given up on.
    pub fn failed(&mut self, signature: &str) -> bool {
        let attempts = self.attempts.entry(signature.to_owned()).or_insert(0);
        *attempts += 1;
        if *attempts >= ATTACH_ATTEMPTS {
            self.unbindable.insert(signature.to_owned());
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn arrivals_and_departures_are_the_difference_from_what_is_live() {
        let mut attached = Attached::default();
        attached.live.insert("a".into(), 1);
        attached.live.insert("b".into(), 2);
        let changes = attached.diff(&set(&["b", "c"]));
        assert_eq!(changes.departed, vec!["a"]);
        assert_eq!(changes.arrived, vec!["c"]);
    }

    #[test]
    fn a_controller_given_up_on_is_not_offered_again_until_replugged() {
        let mut attached = Attached::default();
        for _ in 0..ATTACH_ATTEMPTS - 1 {
            assert!(!attached.failed("x"));
        }
        assert!(attached.failed("x"), "the last attempt gives up");
        assert_eq!(attached.diff(&set(&["x"])).arrived, Vec::<String>::new());
        // Unplugging clears both the count and the verdict.
        attached.diff(&set(&[]));
        assert!(attached.attempts.is_empty());
        assert!(attached.unbindable.is_empty());
        assert_eq!(attached.diff(&set(&["x"])).arrived, vec!["x"]);
    }
}

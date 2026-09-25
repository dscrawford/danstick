//! Noticing controllers arriving and leaving, cheaply.

use std::collections::{BTreeMap, BTreeSet};

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

/// How long after the input nodes change a rescan keeps following up, because a
/// node can appear before udev has finished describing it.
pub const SETTLE_SECONDS: f64 = 1.0;

/// When a full device discovery is worth running: at once when what it would
/// find may have changed, a few more times while that settles, and never
/// otherwise. Discovery opens devices, and running it every tick left the
/// event loop inside a tick nearly all the time.
#[derive(Debug, Default, Clone)]
pub struct ScanGate<K> {
    seen: Option<K>,
    settle_until: f64,
    last: f64,
}

impl<K: PartialEq> ScanGate<K> {
    /// Whether to discover now, given what discovery depends on and the clock.
    pub fn due(&mut self, key: K, clock: f64) -> bool {
        if self.seen.as_ref() != Some(&key) {
            self.seen = Some(key);
            self.settle_until = clock + SETTLE_SECONDS;
            self.last = clock;
            return true;
        }
        if clock < self.settle_until && clock - self.last >= ATTACH_SCAN_SECONDS {
            self.last = clock;
            return true;
        }
        false
    }

    /// Forget what was seen, so the next call discovers.
    pub fn reset(&mut self) {
        self.seen = None;
    }
}

pub const ATTACH_ATTEMPTS: u32 = 20;
pub const ATTACH_SCAN_SECONDS: f64 = 0.25;

#[derive(Debug, Default, Clone)]
pub struct Attached {
    pub live: BTreeMap<String, u32>,
    pub attempts: BTreeMap<String, u32>,
    pub unbindable: BTreeSet<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Changes {
    pub departed: Vec<String>,
    pub arrived: Vec<String>,
}

impl Attached {
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
    #[test]
    fn nothing_changing_is_never_rediscovered() {
        let mut gate = ScanGate::default();
        assert!(gate.due(1, 0.0), "the first look discovers");
        let rescans = (1..=500)
            .filter(|tick| gate.due(1, SETTLE_SECONDS + *tick as f64 * 0.02))
            .count();
        assert_eq!(rescans, 0, "ten quiet seconds rediscovered {rescans} times");
    }

    #[test]
    fn a_change_rediscovers_at_once_and_follows_up_while_it_settles() {
        let mut gate = ScanGate::default();
        gate.due(1, 0.0);
        let at = 100.0;
        assert!(
            gate.due(2, at),
            "a new node is looked at on the tick it appears"
        );
        let follow_ups: Vec<f64> = (1..200)
            .map(|tick| at + tick as f64 * 0.02)
            .filter(|clock| gate.due(2, *clock))
            .collect();
        assert!(
            !follow_ups.is_empty(),
            "a node udev has not finished with is never looked at again"
        );
        assert!(
            follow_ups.iter().all(|clock| *clock <= at + SETTLE_SECONDS),
            "kept rediscovering after it settled: {follow_ups:?}"
        );
        assert!(
            follow_ups
                .windows(2)
                .all(|w| w[1] - w[0] >= ATTACH_SCAN_SECONDS - 1e-9),
            "followed up faster than the attach scan does: {follow_ups:?}"
        );
    }

    #[test]
    fn a_reset_discovers_on_the_next_look() {
        let mut gate = ScanGate::default();
        gate.due(1, 0.0);
        assert!(!gate.due(1, 5.0));
        gate.reset();
        assert!(gate.due(1, 5.02), "seating reopened and looked at nothing");
    }

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
        attached.diff(&set(&[]));
        assert!(attached.attempts.is_empty());
        assert!(attached.unbindable.is_empty());
        assert_eq!(attached.diff(&set(&["x"])).arrived, vec!["x"]);
    }
}

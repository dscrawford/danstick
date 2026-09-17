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

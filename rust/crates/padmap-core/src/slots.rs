//! Slots: whether clones exist before anybody sits in them, and what a leave does.

use serde_json::{json, Value};

/// The fewest slots a fixed daemon publishes.
pub const MIN_COUNT: u32 = 1;
/// The most: RetroArch's player limit.
pub const MAX_COUNT: u32 = crate::retroarch::MAX_PLAYERS;
/// How many a fixed daemon publishes when nobody says.
pub const DEFAULT_COUNT: u32 = 4;

pub const ENV_MODE: &str = "PADMAP_SLOTS";
pub const ENV_COUNT: &str = "PADMAP_SLOT_COUNT";
pub const ENV_ON_LEAVE: &str = "PADMAP_ON_LEAVE";
pub const ENV_LAYOUT: &str = "PADMAP_LAYOUT";

/// When a seat's clone is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// A clone per claim, made when the seat is taken.
    OnDemand,
    /// Every slot's clone made when the daemon starts and kept for its life.
    Fixed,
}

/// What a fixed slot does when its player leaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnLeave {
    /// The clone stays at its node and goes quiet until the next hold takes it.
    Stay,
    /// The clone is destroyed and the slot made again at a new node.
    Destroy,
}

/// How a pad's buttons land on a 360 clone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// The bottom face button is the 360's bottom one, whatever it is labelled.
    Position,
    /// The button labelled A is the 360's A, wherever it sits.
    Label,
}

impl Layout {
    pub const fn as_str(self) -> &'static str {
        match self {
            Layout::Position => "position",
            Layout::Label => "label",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_lowercase().as_str() {
            "position" => Some(Layout::Position),
            "label" => Some(Layout::Label),
            _ => None,
        }
    }
}

impl Mode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Mode::OnDemand => "on-demand",
            Mode::Fixed => "fixed",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_lowercase().as_str() {
            "on-demand" => Some(Mode::OnDemand),
            "fixed" => Some(Mode::Fixed),
            _ => None,
        }
    }
}

impl OnLeave {
    pub const fn as_str(self) -> &'static str {
        match self {
            OnLeave::Stay => "stay",
            OnLeave::Destroy => "destroy",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_lowercase().as_str() {
            "stay" => Some(OnLeave::Stay),
            "destroy" => Some(OnLeave::Destroy),
            _ => None,
        }
    }
}

/// How a daemon publishes its slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    pub mode: Mode,
    pub count: u32,
    pub on_leave: OnLeave,
    pub layout: Layout,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            mode: Mode::OnDemand,
            count: DEFAULT_COUNT,
            on_leave: OnLeave::Stay,
            layout: Layout::Position,
        }
    }
}

/// A change asked for; a field left out keeps what is in force.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Change {
    pub mode: Option<String>,
    pub count: Option<i64>,
    pub on_leave: Option<String>,
    pub layout: Option<String>,
}

impl Policy {
    /// This policy with `change` applied, or why it cannot be: nothing is
    /// half-applied.
    pub fn with(self, change: &Change) -> Result<Policy, String> {
        let mut next = self;
        if let Some(mode) = change.mode.as_deref() {
            next.mode = Mode::parse(mode)
                .ok_or_else(|| format!("unknown slots mode {mode:?}; one of fixed, on-demand"))?;
        }
        if let Some(count) = change.count {
            if !(i64::from(MIN_COUNT)..=i64::from(MAX_COUNT)).contains(&count) {
                return Err(format!("{count} slots is not {MIN_COUNT} to {MAX_COUNT}"));
            }
            next.count = count as u32;
        }
        if let Some(on_leave) = change.on_leave.as_deref() {
            next.on_leave = OnLeave::parse(on_leave)
                .ok_or_else(|| format!("unknown on_leave {on_leave:?}; one of stay, destroy"))?;
        }
        if let Some(layout) = change.layout.as_deref() {
            next.layout = Layout::parse(layout)
                .ok_or_else(|| format!("unknown layout {layout:?}; one of position, label"))?;
        }
        Ok(next)
    }

    /// Read from the environment through `get`; a value that cannot be read is
    /// named in the second half and left at its default, since a daemon that
    /// will not start over a typo seats nobody.
    pub fn from_env(get: impl Fn(&str) -> Option<String>) -> (Policy, Vec<String>) {
        let mut policy = Policy::default();
        let mut complaints = Vec::new();
        let present = |name: &str| get(name).filter(|value| !value.trim().is_empty());
        let fields = [
            (
                ENV_MODE,
                Change {
                    mode: present(ENV_MODE),
                    ..Change::default()
                },
            ),
            (
                ENV_COUNT,
                Change {
                    count: match present(ENV_COUNT) {
                        None => None,
                        Some(text) => match text.trim().parse::<i64>() {
                            Ok(count) => Some(count),
                            Err(_) => {
                                complaints.push(format!("{ENV_COUNT}={text:?} is not a number"));
                                None
                            }
                        },
                    },
                    ..Change::default()
                },
            ),
            (
                ENV_ON_LEAVE,
                Change {
                    on_leave: present(ENV_ON_LEAVE),
                    ..Change::default()
                },
            ),
            (
                ENV_LAYOUT,
                Change {
                    layout: present(ENV_LAYOUT),
                    ..Change::default()
                },
            ),
        ];
        for (name, change) in fields {
            match policy.with(&change) {
                Ok(next) => policy = next,
                Err(why) => complaints.push(format!("{name}: {why}")),
            }
        }
        (policy, complaints)
    }

    /// The seats that exist before anybody sits in them: 1 to `count` when fixed.
    pub fn standing(&self) -> u32 {
        match self.mode {
            Mode::Fixed => self.count,
            Mode::OnDemand => 0,
        }
    }

    /// How many seats a `reserve` for `players` leaves standing: never fewer
    /// than the fixed slots, which outlive every launch.
    pub fn reserving(&self, players: u32) -> u32 {
        players.max(self.standing())
    }

    /// Whether a player leaving `player`'s seat keeps its clone.
    pub fn keeps_on_leave(&self, player: u32) -> bool {
        self.mode == Mode::Fixed && self.on_leave == OnLeave::Stay && player <= self.count
    }

    /// The policy as `state` reports it.
    pub fn to_json(&self) -> Value {
        json!({
            "mode": self.mode.as_str(),
            "count": self.count,
            "on_leave": self.on_leave.as_str(),
            "layout": self.layout.as_str(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    #[test]
    fn nothing_said_is_todays_behaviour() {
        let (policy, complaints) = Policy::from_env(env(&[]));
        assert_eq!(policy, Policy::default());
        assert_eq!(policy.mode, Mode::OnDemand);
        assert_eq!(policy.standing(), 0);
        assert!(complaints.is_empty());
    }

    #[test]
    fn the_environment_sets_every_part() {
        let (policy, complaints) = Policy::from_env(env(&[
            (ENV_MODE, "fixed"),
            (ENV_COUNT, "6"),
            (ENV_ON_LEAVE, "destroy"),
            (ENV_LAYOUT, "label"),
        ]));
        assert!(complaints.is_empty(), "{complaints:?}");
        assert_eq!(
            policy,
            Policy {
                mode: Mode::Fixed,
                count: 6,
                on_leave: OnLeave::Destroy,
                layout: Layout::Label,
            }
        );
        assert_eq!(policy.standing(), 6);
    }

    #[test]
    fn a_value_nobody_can_read_is_named_and_left_at_its_default() {
        let (policy, complaints) = Policy::from_env(env(&[
            (ENV_MODE, "fixed"),
            (ENV_COUNT, "lots"),
            (ENV_ON_LEAVE, "wander"),
        ]));
        assert_eq!(
            policy.mode,
            Mode::Fixed,
            "the part that could be read is kept"
        );
        assert_eq!(policy.count, DEFAULT_COUNT);
        assert_eq!(policy.on_leave, OnLeave::Stay);
        assert_eq!(complaints.len(), 2, "{complaints:?}");
    }

    #[test]
    fn a_change_is_all_or_nothing() {
        let before = Policy::default();
        let refused = before.with(&Change {
            mode: Some("fixed".into()),
            count: Some(17),
            on_leave: None,
            layout: Some("sideways".into()),
        });
        assert!(refused.is_err());
        assert!(before
            .with(&Change {
                count: Some(0),
                ..Change::default()
            })
            .is_err());
        let fixed = before
            .with(&Change {
                mode: Some(" Fixed ".into()),
                count: Some(16),
                ..Change::default()
            })
            .expect("fixed, sixteen");
        assert_eq!(
            (fixed.mode, fixed.count, fixed.on_leave),
            (Mode::Fixed, 16, OnLeave::Stay)
        );
        assert_eq!(
            fixed.with(&Change::default()),
            Ok(fixed),
            "saying nothing changes nothing"
        );
    }

    #[test]
    fn a_reservation_never_takes_away_a_fixed_slot() {
        let fixed = Policy {
            mode: Mode::Fixed,
            ..Policy::default()
        };
        assert_eq!(
            fixed.reserving(0),
            4,
            "giving back a launch's seats keeps the slots"
        );
        assert_eq!(fixed.reserving(6), 6);
        assert_eq!(Policy::default().reserving(0), 0);
    }

    #[test]
    fn only_a_fixed_slot_that_stays_keeps_its_clone() {
        let fixed = Policy {
            mode: Mode::Fixed,
            ..Policy::default()
        };
        assert!(fixed.keeps_on_leave(1));
        assert!(fixed.keeps_on_leave(4));
        assert!(
            !fixed.keeps_on_leave(5),
            "a seat past the slots was made on demand"
        );
        let destroy = Policy {
            on_leave: OnLeave::Destroy,
            ..fixed
        };
        assert!(!destroy.keeps_on_leave(1));
        assert!(!Policy::default().keeps_on_leave(1));
    }

    #[test]
    fn every_name_reads_back_as_it_is_printed() {
        for mode in [Mode::OnDemand, Mode::Fixed] {
            assert_eq!(Mode::parse(mode.as_str()), Some(mode));
        }
        for on_leave in [OnLeave::Stay, OnLeave::Destroy] {
            assert_eq!(OnLeave::parse(on_leave.as_str()), Some(on_leave));
        }
        for layout in [Layout::Position, Layout::Label] {
            assert_eq!(Layout::parse(layout.as_str()), Some(layout));
        }
        assert_eq!(Mode::parse("ondemand"), None);
        let reported = Policy::default().to_json();
        assert_eq!(reported["layout"], "position");
        assert_eq!(reported["mode"], "on-demand");
        assert_eq!(reported["count"], 4);
        assert_eq!(reported["on_leave"], "stay");
    }
}

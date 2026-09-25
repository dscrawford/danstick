//! Which Steam pad repeats whose press. Steam's pads name no source, but each
//! presses a moment after the one it repeats, so a Steam press that follows
//! another inside [`ECHO_SECONDS`] is that one's echo -- paired one to one,
//! closest first, since Steam makes one pad per source.

/// How long after its source Steam's pad presses, at the most.
pub const ECHO_SECONDS: f64 = 0.05;

/// How long a hold is judged afresh before its verdict stands: long enough
/// for a press the loop read late to have been noted.
const SETTLE_SECONDS: f64 = 1.0;

/// How long a press is kept to pair with: until every hold it could start has settled.
const KEEP_SECONDS: f64 = SETTLE_SECONDS + 0.5;

/// Presses kept at the most, so a device pressing without end costs a bounded pairing.
const MAX_PRESSES: usize = 512;

/// Where a button went down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// A pad seating is watching, by its index there.
    Watched { pad: usize, steam: bool },
    /// A seated player's own pad.
    Seated { player: u32, steam: bool },
    /// danstick's clone for a seat, as danstick wrote it.
    Clone { player: u32 },
}

impl Origin {
    fn steam(self) -> bool {
        match self {
            Origin::Watched { steam, .. } | Origin::Seated { steam, .. } => steam,
            Origin::Clone { .. } => false,
        }
    }
}

/// One button going down.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Press {
    pub origin: Origin,
    pub at: f64,
}

/// What a watched pad's hold is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Nothing else pressed with it: its own controller.
    Own,
    /// A pad Steam may yet repeat, too soon after its press to say.
    Unsettled,
    /// A Steam pad repeating danstick's own clone.
    CloneEcho { player: u32 },
    /// A Steam pad repeating a seated player's pad.
    SeatedEcho { player: u32 },
    /// A Steam pad repeating a watched pad: the same controller, and Steam's
    /// pad is the one that stands for it.
    SteamOf { pad: usize },
    /// A pad Steam repeats: its Steam pad stands for it, seated or not.
    DrivenBySteam,
}

impl Verdict {
    /// Whether this hold may go on to claim a seat.
    pub fn may_claim(self) -> bool {
        matches!(self, Verdict::Own | Verdict::SteamOf { .. })
    }
}

/// Recent presses on every node that could be a Steam pad's source or a Steam
/// pad, and the verdicts that have settled.
#[derive(Debug, Clone, Default)]
pub struct Echoes {
    presses: Vec<Press>,
    settled: Vec<(usize, f64, Verdict)>,
}

impl Echoes {
    pub fn press(&mut self, origin: Origin, at: f64) {
        if self.presses.len() >= MAX_PRESSES {
            self.presses.remove(0);
        }
        self.presses.push(Press { origin, at });
    }

    /// Carry each watched pad's presses to its new index, as [`crate::assign::Assigner::remap`] does.
    pub fn remap(&mut self, moved: impl Fn(usize) -> Option<usize>) {
        self.presses.retain_mut(|press| match &mut press.origin {
            Origin::Watched { pad, .. } => match moved(*pad) {
                Some(to) => {
                    *pad = to;
                    true
                }
                None => false,
            },
            _ => true,
        });
        self.settled.retain_mut(|(pad, _, _)| match moved(*pad) {
            Some(to) => {
                *pad = to;
                true
            }
            None => false,
        });
    }

    pub fn clear(&mut self) {
        self.presses.clear();
        self.settled.clear();
    }

    /// What each hold in flight -- a pad, and when its hold started -- is.
    /// `steam_near` says a Steam pad is watched or seated, so a pad's own
    /// press may yet be repeated.
    pub fn judge(&mut self, holds: &[(usize, f64)], now: f64, steam_near: bool) -> Vec<Verdict> {
        self.presses.retain(|press| now - press.at <= KEEP_SECONDS);
        self.settled
            .retain(|(pad, since, _)| holds.contains(&(*pad, *since)));
        if holds.is_empty() {
            return Vec::new();
        }
        let pairs = pairs(&self.presses);
        let mut out = Vec::with_capacity(holds.len());
        for &(pad, since) in holds {
            let settled = self
                .settled
                .iter()
                .find(|(at, started, _)| (*at, *started) == (pad, since))
                .map(|(_, _, verdict)| *verdict);
            let verdict = settled.unwrap_or_else(|| {
                let verdict = self.verdict(&pairs, pad, since, now, steam_near);
                if now - since >= SETTLE_SECONDS {
                    self.settled.push((pad, since, verdict));
                }
                verdict
            });
            out.push(verdict);
        }
        out
    }

    fn verdict(
        &self,
        pairs: &[(usize, usize)],
        pad: usize,
        since: f64,
        now: f64,
        steam_near: bool,
    ) -> Verdict {
        let Some(mine) = self.presses.iter().position(|press| {
            matches!(press.origin, Origin::Watched { pad: at, .. } if at == pad)
                && (press.at - since).abs() < 1e-6
        }) else {
            return Verdict::Own;
        };
        let partner = pairs.iter().find_map(|&(source, echo)| match mine {
            _ if mine == echo => Some(self.presses[source].origin),
            _ if mine == source => Some(self.presses[echo].origin),
            _ => None,
        });
        match (self.presses[mine].origin.steam(), partner) {
            (true, Some(Origin::Clone { player })) => Verdict::CloneEcho { player },
            (true, Some(Origin::Seated { player, .. })) => Verdict::SeatedEcho { player },
            (true, Some(Origin::Watched { pad, .. })) => Verdict::SteamOf { pad },
            (false, Some(_)) => Verdict::DrivenBySteam,
            (false, None) if steam_near && now - since < ECHO_SECONDS => Verdict::Unsettled,
            (_, None) => Verdict::Own,
        }
    }
}

/// Each Steam press paired with the press it repeats, as indices into
/// `presses`: closest in time first, each press used once.
fn pairs(presses: &[Press]) -> Vec<(usize, usize)> {
    let mut candidates: Vec<(f64, usize, usize)> = Vec::new();
    for (echo, repeat) in presses.iter().enumerate() {
        if !repeat.origin.steam() {
            continue;
        }
        for (source, first) in presses.iter().enumerate() {
            let lag = repeat.at - first.at;
            if !first.origin.steam() && (0.0..=ECHO_SECONDS).contains(&lag) {
                candidates.push((lag, source, echo));
            }
        }
    }
    candidates.sort_by(|left, right| left.0.total_cmp(&right.0).then(left.1.cmp(&right.1)));
    let mut used = vec![false; presses.len()];
    let mut out = Vec::new();
    for (_, source, echo) in candidates {
        if used[source] || used[echo] {
            continue;
        }
        used[source] = true;
        used[echo] = true;
        out.push((source, echo));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAW: Origin = Origin::Watched {
        pad: 0,
        steam: false,
    };
    const RAWS_STEAM: Origin = Origin::Watched {
        pad: 1,
        steam: true,
    };
    const DECK: Origin = Origin::Watched {
        pad: 2,
        steam: true,
    };
    const CLONES_STEAM: Origin = Origin::Watched {
        pad: 3,
        steam: true,
    };

    /// One hold's verdict, as a tick with only that hold in flight would give it.
    fn verdict(echoes: &Echoes, pad: usize, since: f64, now: f64, near: bool) -> Verdict {
        echoes.clone().judge(&[(pad, since)], now, near)[0]
    }

    fn echoes(presses: &[(Origin, f64)]) -> Echoes {
        let mut echoes = Echoes::default();
        for (origin, at) in presses {
            echoes.press(*origin, *at);
        }
        echoes
    }

    #[test]
    fn a_press_nothing_else_made_is_its_own_controller() {
        let echoes = echoes(&[(DECK, 1.0)]);
        assert_eq!(verdict(&echoes, 2, 1.0, 1.3, true), Verdict::Own);
        let echoes = self::echoes(&[(RAW, 1.0)]);
        assert_eq!(verdict(&echoes, 0, 1.0, 1.3, false), Verdict::Own);
        assert_eq!(verdict(&echoes, 0, 1.0, 1.3, true), Verdict::Own);
    }

    #[test]
    fn a_steam_pad_repeating_a_clone_is_the_clones() {
        let echoes = echoes(&[(Origin::Clone { player: 1 }, 5.0), (CLONES_STEAM, 5.004)]);
        assert_eq!(
            verdict(&echoes, 3, 5.004, 5.3, true),
            Verdict::CloneEcho { player: 1 }
        );
        assert!(!verdict(&echoes, 3, 5.004, 5.3, true).may_claim());
    }

    #[test]
    fn a_steam_pad_repeating_a_seated_pad_is_that_seat() {
        let seated = Origin::Seated {
            player: 2,
            steam: false,
        };
        let echoes = echoes(&[(seated, 5.0), (RAWS_STEAM, 5.003)]);
        assert_eq!(
            verdict(&echoes, 1, 5.003, 5.3, true),
            Verdict::SeatedEcho { player: 2 }
        );
    }

    #[test]
    fn a_pad_and_its_steam_pad_are_one_controller_and_steams_stands_for_it() {
        let echoes = echoes(&[(RAW, 1.0), (RAWS_STEAM, 1.003)]);
        assert_eq!(verdict(&echoes, 0, 1.0, 1.3, true), Verdict::DrivenBySteam);
        assert_eq!(
            verdict(&echoes, 1, 1.003, 1.3, true),
            Verdict::SteamOf { pad: 0 }
        );
        assert!(verdict(&echoes, 1, 1.003, 1.3, true).may_claim());
        assert!(!verdict(&echoes, 0, 1.0, 1.3, true).may_claim());
    }

    #[test]
    fn a_pad_whose_steam_pad_is_seated_is_that_seat() {
        let seated = Origin::Seated {
            player: 1,
            steam: true,
        };
        let echoes = echoes(&[(RAW, 1.0), (seated, 1.003)]);
        assert_eq!(verdict(&echoes, 0, 1.0, 1.3, true), Verdict::DrivenBySteam);
    }

    #[test]
    fn a_pads_own_press_waits_out_the_moment_steam_takes_to_repeat_it() {
        let echoes = echoes(&[(RAW, 1.0)]);
        assert_eq!(verdict(&echoes, 0, 1.0, 1.01, true), Verdict::Unsettled);
        assert!(!Verdict::Unsettled.may_claim());
        assert_eq!(
            verdict(&echoes, 0, 1.0, 1.01, false),
            Verdict::Own,
            "with no Steam pad about nothing can repeat it"
        );
        assert_eq!(
            verdict(&echoes, 0, 1.0, 1.0 + ECHO_SECONDS, true),
            Verdict::Own
        );
    }

    #[test]
    fn a_steam_press_before_its_source_or_long_after_is_not_its_echo() {
        let clone = Origin::Clone { player: 1 };
        let early = echoes(&[(CLONES_STEAM, 4.99), (clone, 5.0)]);
        assert_eq!(verdict(&early, 3, 4.99, 5.3, true), Verdict::Own);
        let late = echoes(&[(clone, 5.0), (CLONES_STEAM, 5.0 + ECHO_SECONDS + 0.01)]);
        assert_eq!(verdict(&late, 3, 5.06, 5.3, true), Verdict::Own);
    }

    #[test]
    fn somebody_on_the_deck_pressing_as_a_seated_player_does_keeps_their_own_press() {
        // Player 1 readies up: the clone presses and its Steam pad repeats it.
        // The Deck's owner presses 20ms later and is nobody's echo.
        let clone = Origin::Clone { player: 1 };
        let seated = Origin::Seated {
            player: 1,
            steam: true,
        };
        let echoes = echoes(&[
            (seated, 3.0),
            (clone, 3.001),
            (CLONES_STEAM, 3.004),
            (DECK, 3.021),
        ]);
        assert_eq!(
            verdict(&echoes, 3, 3.004, 3.3, true),
            Verdict::CloneEcho { player: 1 }
        );
        assert_eq!(verdict(&echoes, 2, 3.021, 3.3, true), Verdict::Own);
    }

    #[test]
    fn two_people_pressing_together_are_two_steam_pads_and_no_raw_one() {
        let other = Origin::Watched {
            pad: 4,
            steam: false,
        };
        let others_steam = Origin::Watched {
            pad: 5,
            steam: true,
        };
        let echoes = echoes(&[
            (RAW, 1.0),
            (other, 1.002),
            (RAWS_STEAM, 1.004),
            (others_steam, 1.005),
        ]);
        // Two milliseconds apart, which raw pad is whose is not knowable, and
        // does not matter: each Steam pad stands for one of them.
        for (pad, since) in [(1, 1.004), (5, 1.005)] {
            assert!(verdict(&echoes, pad, since, 1.3, true).may_claim());
        }
        for (pad, since) in [(0, 1.0), (4, 1.002)] {
            assert_eq!(
                verdict(&echoes, pad, since, 1.3, true),
                Verdict::DrivenBySteam
            );
        }
    }

    #[test]
    fn a_hold_is_judged_by_the_press_that_started_it() {
        // The Steam pad's hold began on its own; a later press repeating the
        // clone does not make the earlier one an echo.
        let echoes = echoes(&[
            (DECK, 1.0),
            (Origin::Clone { player: 1 }, 2.0),
            (DECK, 2.003),
        ]);
        assert_eq!(verdict(&echoes, 2, 1.0, 2.1, true), Verdict::Own);
        assert_eq!(
            verdict(&echoes, 2, 2.003, 2.1, true),
            Verdict::CloneEcho { player: 1 }
        );
    }

    #[test]
    fn a_hold_with_no_press_noted_is_its_own() {
        assert_eq!(
            verdict(&Echoes::default(), 0, 1.0, 1.3, true),
            Verdict::Own,
            "nothing known is nothing to refuse"
        );
    }

    #[test]
    fn presses_follow_their_pad_to_its_new_index_and_old_ones_go() {
        let mut echoes = echoes(&[
            (RAW, 1.0),
            (RAWS_STEAM, 1.003),
            (Origin::Clone { player: 1 }, 1.0),
        ]);
        // Pad 0 went; pad 1 is now pad 0.
        echoes.remap(|pad| (pad == 1).then_some(0));
        assert_eq!(
            verdict(&echoes, 0, 1.003, 1.3, true),
            Verdict::CloneEcho { player: 1 },
            "with its raw pad gone, the nearest source left is the clone"
        );
        let later = 1.0 + KEEP_SECONDS + 0.1;
        assert_eq!(verdict(&echoes, 0, 1.003, later, true), Verdict::Own);
    }

    #[test]
    fn a_steam_press_in_the_same_instant_as_its_source_is_its_echo() {
        let echoes = echoes(&[(Origin::Clone { player: 7 }, 9.0), (CLONES_STEAM, 9.0)]);
        assert_eq!(
            verdict(&echoes, 3, 9.0, 9.3, true),
            Verdict::CloneEcho { player: 7 }
        );
    }

    #[test]
    fn each_steam_press_pairs_with_the_closest_source_even_out_of_order() {
        let other = Origin::Watched {
            pad: 4,
            steam: false,
        };
        let others_steam = Origin::Watched {
            pad: 5,
            steam: true,
        };
        // Pad 5 is 1ms after pad 4 and 11ms after pad 0; pad 1 is 21ms after pad 0.
        let echoes = echoes(&[
            (RAW, 1.000),
            (other, 1.010),
            (others_steam, 1.011),
            (RAWS_STEAM, 1.021),
        ]);
        assert_eq!(
            verdict(&echoes, 5, 1.011, 1.3, true),
            Verdict::SteamOf { pad: 4 }
        );
        assert_eq!(
            verdict(&echoes, 1, 1.021, 1.3, true),
            Verdict::SteamOf { pad: 0 }
        );
    }

    #[test]
    fn a_pad_whose_steam_pad_went_is_its_own_again() {
        let mut echoes = echoes(&[(RAW, 1.0), (RAWS_STEAM, 1.003)]);
        assert_eq!(verdict(&echoes, 0, 1.0, 1.3, true), Verdict::DrivenBySteam);
        echoes.remap(|pad| (pad == 0).then_some(0));
        assert_eq!(verdict(&echoes, 0, 1.0, 1.3, true), Verdict::Own);
    }

    #[test]
    fn a_pairing_already_made_stands_whether_or_not_a_steam_pad_is_near() {
        let echoes = echoes(&[(Origin::Clone { player: 4 }, 2.0), (CLONES_STEAM, 2.003)]);
        assert_eq!(
            verdict(&echoes, 3, 2.003, 2.3, false),
            Verdict::CloneEcho { player: 4 }
        );
    }

    #[test]
    fn a_long_holds_verdict_stands_after_the_presses_behind_it_are_forgotten() {
        // A 10s hold starting on a clone's echo must not become its own at the end.
        let mut echoes = echoes(&[(Origin::Clone { player: 1 }, 5.0), (CLONES_STEAM, 5.004)]);
        let hold = [(3, 5.004)];
        let echo = Verdict::CloneEcho { player: 1 };
        assert_eq!(echoes.judge(&hold, 5.1, true), [echo]);
        assert_eq!(echoes.judge(&hold, 5.004 + SETTLE_SECONDS, true), [echo]);
        assert_eq!(
            echoes.judge(&hold, 15.0, true),
            [echo],
            "the verdict was lost"
        );
        // Once the hold ends its verdict goes with it.
        assert!(echoes.judge(&[], 15.1, true).is_empty());
        assert_eq!(echoes.judge(&hold, 15.2, true), [Verdict::Own]);
    }

    #[test]
    fn a_device_pressing_without_end_is_kept_to_a_bounded_log() {
        let mut echoes = Echoes::default();
        for at in 0..(MAX_PRESSES * 4) {
            echoes.press(DECK, at as f64 * 1e-4);
        }
        assert_eq!(echoes.presses.len(), MAX_PRESSES);
        // The newest are the ones kept: a hold starting now can still be judged.
        echoes.press(Origin::Clone { player: 1 }, 1.0);
        echoes.press(CLONES_STEAM, 1.004);
        assert_eq!(
            echoes.judge(&[(3, 1.004)], 1.1, true),
            [Verdict::CloneEcho { player: 1 }]
        );
    }
}

//! Which Steam pad repeats whose press, paired one to one, closest first.

/// How long after its source Steam's pad presses, at the most.
pub const ECHO_SECONDS: f64 = 0.05;

/// How long a hold is judged afresh before its verdict stands.
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
    /// A pad Steam repeats on the watched Steam pad `steam`, held with the seat if `sure`.
    Mirrored { steam: usize, sure: bool },
    /// A Steam pad repeating danstick's own clone.
    CloneEcho { player: u32 },
    /// A Steam pad repeating a seated player's pad.
    SeatedEcho { player: u32 },
    /// A Steam pad repeating the watched pad `pad`, which sits in its place.
    EchoOf { pad: usize },
    /// A pad a seated Steam pad repeats: that controller already has a seat.
    SeatedThroughSteam { player: u32 },
    /// A Steam pad nobody's echo, pressing just after a pad already paired with another.
    Ambiguous,
}

impl Verdict {
    /// Whether this hold may go on to claim a seat.
    pub fn may_claim(self) -> bool {
        matches!(self, Verdict::Own | Verdict::Mirrored { .. })
    }
}

/// Recent presses on every node that could be a Steam pad's source or a Steam pad.
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

    /// Carry each watched pad's presses to its new index.
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
        // A verdict naming a pad that went is judged again.
        self.settled.retain_mut(|(pad, _, verdict)| {
            let Some(to) = moved(*pad) else {
                return false;
            };
            *pad = to;
            match verdict {
                Verdict::Mirrored { steam: other, .. } | Verdict::EchoOf { pad: other } => {
                    match moved(*other) {
                        Some(to) => {
                            *other = to;
                            true
                        }
                        None => false,
                    }
                }
                _ => true,
            }
        });
    }

    pub fn clear(&mut self) {
        self.presses.clear();
        self.settled.clear();
    }

    /// What each hold in flight -- a pad, and when its hold started -- is.
    pub fn judge(&mut self, holds: &[(usize, f64)], now: f64) -> Vec<Verdict> {
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
                let verdict = self.verdict(&pairs, pad, since);
                if now - since >= SETTLE_SECONDS {
                    self.settled.push((pad, since, verdict));
                }
                verdict
            });
            out.push(verdict);
        }
        out
    }

    fn verdict(&self, pairs: &[(usize, usize)], pad: usize, since: f64) -> Verdict {
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
        let at = self.presses[mine].at;
        let within = |press: &Press, steam: bool| {
            press.origin.steam() == steam && (0.0..=ECHO_SECONDS).contains(&(at - press.at))
        };
        match (self.presses[mine].origin.steam(), partner) {
            (true, Some(Origin::Clone { player })) => Verdict::CloneEcho { player },
            (true, Some(Origin::Seated { player, .. })) => Verdict::SeatedEcho { player },
            (true, Some(Origin::Watched { pad, .. })) => Verdict::EchoOf { pad },
            (true, None)
                if self.presses.iter().any(|press| {
                    within(press, false) && matches!(press.origin, Origin::Watched { .. })
                }) =>
            {
                Verdict::Ambiguous
            }
            (false, Some(Origin::Watched { pad, .. })) => Verdict::Mirrored {
                steam: pad,
                sure: self
                    .presses
                    .iter()
                    .filter(|press| {
                        press.origin.steam() && (0.0..=ECHO_SECONDS).contains(&(press.at - at))
                    })
                    .count()
                    == 1,
            },
            (false, Some(Origin::Seated { player, .. })) => Verdict::SeatedThroughSteam { player },
            (false, Some(Origin::Clone { .. })) | (_, None) => Verdict::Own,
        }
    }
}

/// Each Steam press paired with the press it repeats, closest first, each used once.
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
    fn verdict(echoes: &Echoes, pad: usize, since: f64, now: f64) -> Verdict {
        echoes.clone().judge(&[(pad, since)], now)[0]
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
        assert_eq!(verdict(&echoes(&[(DECK, 1.0)]), 2, 1.0, 1.3), Verdict::Own);
        assert_eq!(verdict(&echoes(&[(RAW, 1.0)]), 0, 1.0, 1.3), Verdict::Own);
        assert_eq!(
            verdict(&Echoes::default(), 0, 1.0, 1.3),
            Verdict::Own,
            "nothing known is nothing to refuse"
        );
    }

    #[test]
    fn a_steam_pad_repeating_a_clone_is_the_clones() {
        let echoes = echoes(&[(Origin::Clone { player: 1 }, 5.0), (CLONES_STEAM, 5.004)]);
        let found = verdict(&echoes, 3, 5.004, 5.3);
        assert_eq!(found, Verdict::CloneEcho { player: 1 });
        assert!(!found.may_claim());
    }

    #[test]
    fn a_steam_press_in_the_same_instant_as_its_source_is_its_echo() {
        let echoes = echoes(&[(Origin::Clone { player: 7 }, 9.0), (CLONES_STEAM, 9.0)]);
        assert_eq!(
            verdict(&echoes, 3, 9.0, 9.3),
            Verdict::CloneEcho { player: 7 }
        );
    }

    #[test]
    fn a_steam_pad_repeating_a_seated_pad_is_that_seat() {
        let seated = Origin::Seated {
            player: 2,
            steam: false,
        };
        let echoes = echoes(&[(seated, 5.0), (RAWS_STEAM, 5.003)]);
        assert_eq!(
            verdict(&echoes, 1, 5.003, 5.3),
            Verdict::SeatedEcho { player: 2 }
        );
    }

    #[test]
    fn a_pad_and_its_steam_pad_are_one_controller_and_the_pad_sits() {
        let echoes = echoes(&[(RAW, 1.0), (RAWS_STEAM, 1.003)]);
        let raw = verdict(&echoes, 0, 1.0, 1.3);
        assert_eq!(
            raw,
            Verdict::Mirrored {
                steam: 1,
                sure: true
            }
        );
        assert!(raw.may_claim());
        let steam = verdict(&echoes, 1, 1.003, 1.3);
        assert_eq!(steam, Verdict::EchoOf { pad: 0 });
        assert!(!steam.may_claim());
    }

    #[test]
    fn a_pad_whose_steam_pad_is_seated_already_has_its_seat() {
        let seated = Origin::Seated {
            player: 1,
            steam: true,
        };
        let echoes = echoes(&[(RAW, 1.0), (seated, 1.003)]);
        let found = verdict(&echoes, 0, 1.0, 1.3);
        assert_eq!(found, Verdict::SeatedThroughSteam { player: 1 });
        assert!(!found.may_claim());
    }

    #[test]
    fn somebody_on_the_deck_pressing_with_a_pad_is_neither_held_nor_a_second_seat() {
        let echoes = echoes(&[(RAW, 0.0), (DECK, 0.010), (RAWS_STEAM, 0.012)]);
        assert_eq!(
            verdict(&echoes, 0, 0.0, 0.3),
            Verdict::Mirrored {
                steam: 2,
                sure: false
            },
            "not sure, so the Deck is not held with the Xbox pad's seat"
        );
        assert!(
            !verdict(&echoes, 2, 0.010, 0.3).may_claim(),
            "the Deck retries"
        );
        assert_eq!(
            verdict(&echoes, 1, 0.012, 0.3),
            Verdict::Ambiguous,
            "the Xbox pad's copy must not take a second seat"
        );
        assert!(!Verdict::Ambiguous.may_claim());
    }

    #[test]
    fn a_steam_press_before_its_source_or_long_after_is_not_its_echo() {
        let clone = Origin::Clone { player: 1 };
        let early = echoes(&[(CLONES_STEAM, 4.99), (clone, 5.0)]);
        assert_eq!(verdict(&early, 3, 4.99, 5.3), Verdict::Own);
        let late = echoes(&[(clone, 5.0), (CLONES_STEAM, 5.0 + ECHO_SECONDS + 0.01)]);
        assert_eq!(verdict(&late, 3, 5.06, 5.3), Verdict::Own);
    }

    #[test]
    fn somebody_on_the_deck_pressing_as_a_seated_player_does_keeps_their_own_press() {
        let clone = Origin::Clone { player: 1 };
        let seated = Origin::Seated {
            player: 1,
            steam: false,
        };
        let echoes = echoes(&[
            (seated, 3.0),
            (clone, 3.001),
            (RAWS_STEAM, 3.003),
            (CLONES_STEAM, 3.004),
            (DECK, 3.021),
        ]);
        assert!(!verdict(&echoes, 3, 3.004, 3.3).may_claim());
        assert!(verdict(&echoes, 2, 3.021, 3.3).may_claim());
    }

    #[test]
    fn two_people_pressing_together_are_two_pads_and_no_steam_one() {
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
        for (pad, since) in [(0, 1.0), (4, 1.002)] {
            assert!(verdict(&echoes, pad, since, 1.3).may_claim());
        }
        for (pad, since) in [(1, 1.004), (5, 1.005)] {
            assert!(!verdict(&echoes, pad, since, 1.3).may_claim());
        }
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
        let echoes = echoes(&[
            (RAW, 1.000),
            (other, 1.010),
            (others_steam, 1.011),
            (RAWS_STEAM, 1.021),
        ]);
        assert_eq!(
            verdict(&echoes, 4, 1.010, 1.3),
            Verdict::Mirrored {
                steam: 5,
                sure: false
            }
        );
        assert_eq!(
            verdict(&echoes, 0, 1.000, 1.3),
            Verdict::Mirrored {
                steam: 1,
                sure: false
            },
            "two Steam presses followed it, so neither is surely its copy"
        );
    }

    #[test]
    fn a_hold_is_judged_by_the_press_that_started_it() {
        let echoes = echoes(&[
            (DECK, 1.0),
            (Origin::Clone { player: 1 }, 2.0),
            (DECK, 2.003),
        ]);
        assert_eq!(verdict(&echoes, 2, 1.0, 2.1), Verdict::Own);
        assert_eq!(
            verdict(&echoes, 2, 2.003, 2.1),
            Verdict::CloneEcho { player: 1 }
        );
    }

    #[test]
    fn a_pad_whose_steam_pad_went_is_simply_its_own() {
        let mut echoes = echoes(&[(RAW, 1.0), (RAWS_STEAM, 1.003)]);
        echoes.remap(|pad| (pad == 0).then_some(0));
        assert_eq!(verdict(&echoes, 0, 1.0, 1.3), Verdict::Own);
    }

    #[test]
    fn presses_follow_their_pad_to_its_new_index_and_old_ones_go() {
        let echoes_before = echoes(&[
            (RAW, 1.0),
            (RAWS_STEAM, 1.003),
            (Origin::Clone { player: 1 }, 1.0),
        ]);
        let mut echoes = echoes_before.clone();
        echoes.remap(|pad| (pad == 1).then_some(0));
        assert_eq!(
            verdict(&echoes, 0, 1.003, 1.3),
            Verdict::CloneEcho { player: 1 },
            "with its raw pad gone, the nearest source left is the clone"
        );
        let later = 1.0 + KEEP_SECONDS + 0.1;
        assert_eq!(verdict(&echoes, 0, 1.003, later), Verdict::Own);
    }

    #[test]
    fn a_settled_verdict_follows_both_pads_it_names_when_they_move() {
        let mut echoes = echoes(&[(RAW, 1.0), (RAWS_STEAM, 1.003)]);
        let hold = [(0, 1.0)];
        assert_eq!(
            echoes.judge(&hold, 1.0 + SETTLE_SECONDS,),
            [Verdict::Mirrored {
                steam: 1,
                sure: true
            }]
        );
        echoes.remap(|pad| Some(pad + 1));
        assert_eq!(
            echoes.judge(&[(1, 1.0)], 1.0 + SETTLE_SECONDS + 0.1),
            [Verdict::Mirrored {
                steam: 2,
                sure: true
            }]
        );
    }

    #[test]
    fn a_settled_echo_follows_both_pads_it_names_when_they_move() {
        let mut echoes = echoes(&[(RAW, 1.0), (RAWS_STEAM, 1.003)]);
        let settle = 1.003 + SETTLE_SECONDS;
        assert_eq!(
            echoes.judge(&[(1, 1.003)], settle),
            [Verdict::EchoOf { pad: 0 }]
        );
        echoes.remap(|pad| Some(pad + 1));
        assert_eq!(
            echoes.judge(&[(2, 1.003)], settle + 0.1),
            [Verdict::EchoOf { pad: 1 }]
        );
    }

    #[test]
    fn a_settled_verdict_whose_other_pad_went_is_judged_again() {
        let settle = 1.003 + SETTLE_SECONDS;
        let mut echoes = echoes(&[(RAW, 1.0), (RAWS_STEAM, 1.003)]);
        assert!(matches!(
            echoes.judge(&[(0, 1.0)], settle)[0],
            Verdict::Mirrored { steam: 1, .. }
        ));
        echoes.remap(|pad| (pad == 0).then_some(0));
        assert_eq!(echoes.judge(&[(0, 1.0)], settle + 0.4), [Verdict::Own]);
        let mut echoes = self::echoes(&[(RAW, 1.0), (RAWS_STEAM, 1.003)]);
        assert_eq!(
            echoes.judge(&[(1, 1.003)], settle),
            [Verdict::EchoOf { pad: 0 }]
        );
        echoes.remap(|pad| (pad == 1).then_some(1));
        assert_eq!(echoes.judge(&[(1, 1.003)], settle + 0.4), [Verdict::Own]);
    }

    #[test]
    fn a_settled_verdict_naming_no_other_pad_moves_with_its_own() {
        let mut echoes = echoes(&[(Origin::Clone { player: 1 }, 5.0), (CLONES_STEAM, 5.004)]);
        let settle = 5.004 + SETTLE_SECONDS;
        let echo = Verdict::CloneEcho { player: 1 };
        assert_eq!(echoes.judge(&[(3, 5.004)], settle), [echo]);
        echoes.remap(|pad| (pad == 3).then_some(0));
        assert_eq!(echoes.judge(&[(0, 5.004)], settle + 5.0), [echo]);
    }

    #[test]
    fn a_long_holds_verdict_stands_after_the_presses_behind_it_are_forgotten() {
        let mut echoes = echoes(&[(Origin::Clone { player: 1 }, 5.0), (CLONES_STEAM, 5.004)]);
        let hold = [(3, 5.004)];
        let echo = Verdict::CloneEcho { player: 1 };
        assert_eq!(echoes.judge(&hold, 5.1), [echo]);
        assert_eq!(echoes.judge(&hold, 5.004 + SETTLE_SECONDS), [echo]);
        assert_eq!(echoes.judge(&hold, 15.0), [echo], "the verdict was lost");
        assert!(echoes.judge(&[], 15.1).is_empty());
        assert_eq!(echoes.judge(&hold, 15.2), [Verdict::Own]);
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
            echoes.judge(&[(3, 1.004)], 1.1),
            [Verdict::CloneEcho { player: 1 }]
        );
    }
}

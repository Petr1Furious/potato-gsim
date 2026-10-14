//! Special bodies, in the bases mode: ordinary bodies near the centre that give something to
//! whoever holds an orbit around them for a few seconds (the same orbit as for a target). Each
//! gives once; another of the same kind turns up on another body a little later.

use super::*;

/// Seconds before another special of the same kind turns up.
const COMEBACK_SECONDS: f64 = 20.0;

impl Authority {
    /// How many of each kind there should be: one for every two players, at least one.
    fn specials_wanted(&self) -> usize {
        (self.players.values().filter(|p| !p.spectating).count() / 2).max(1)
    }

    /// A body for a new special: a planet among the few closest to the centre, that is no
    /// base, star or special already. `None` if there is none.
    fn choose_special(&mut self) -> Option<u32> {
        let m = &self.massive;
        let mut cands: Vec<(u32, f64)> = (0..m.len())
            .filter(|&j| m.alive[j] && !m.fixed[j] && m.mass[j] > 0.0 && m.mass[j] < 1.0e29)
            .filter(|&j| !self.specials.iter().any(|s| s.slot as usize == j))
            .map(|j| (j as u32, m.x[j] * m.x[j] + m.y[j] * m.y[j]))
            .collect();
        // The heavier half: their orbits are roomy enough to fly.
        cands.sort_by(|a, b| m.mass[b.0 as usize].total_cmp(&m.mass[a.0 as usize]).then(a.0.cmp(&b.0)));
        cands.truncate((cands.len() / 2).max(8.min(cands.len())));
        cands.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        cands.truncate(5);
        (!cands.is_empty()).then(|| cands[(self.rng.u64() % cands.len() as u64) as usize].0)
    }

    /// Once a tick in a round of bases: specials follow their bodies through merges, give way
    /// when lost, give what they give to whoever held an orbit long enough, come back after a
    /// while, and are kept as many as wanted. Returns the holds to report.
    pub(super) fn tend_specials(&mut self, tick: Tick, merges: &[MergeEvent], playing: bool) -> Vec<(PlayerId, u32, u32)> {
        let rules = self.rules.clone();
        let before = self.specials.clone();
        let comeback = tick + (COMEBACK_SECONDS * rules.tick_hz as f64) as Tick;
        for k in (0..self.specials.len()).rev() {
            let old = self.specials[k].slot;
            let survivor = merges.iter().find(|e| e.absorbed.contains(&old)).map(|e| e.survivor);
            let slot = match survivor {
                Some(Some(s)) => s,
                Some(None) => u32::MAX,
                None => old,
            };
            // Gone, swallowed by a base, or merged with another special.
            let lost = slot == u32::MAX || !self.massive.alive[slot as usize] || self.massive.fixed[slot as usize] || self.specials[..k].iter().any(|s| s.slot == slot);
            if lost {
                let kind = self.specials.remove(k).kind;
                self.special_due.push((kind, comeback));
                continue;
            }
            self.specials[k].slot = slot;
            for p in self.players.values_mut() {
                p.hold = p.hold.map(|(s, held)| (if s == old { slot } else { s }, held));
            }
        }

        let mut gifts: Vec<(PlayerId, Special)> = Vec::new();
        if playing {
            let goal = (SPECIAL_HOLD_SECONDS * rules.tick_hz as f64) as u32;
            let bodies: Vec<(Special, Particle, f64, f64)> = {
                let m = &self.massive;
                self.specials.iter().map(|s| (*s, s.slot as usize)).map(|(s, j)| (s, Particle { x: m.x[j], y: m.y[j], vx: m.vx[j], vy: m.vy[j] }, m.pull[j], m.radius[j])).collect()
            };
            for p in self.players.values_mut() {
                let around = |slot: u32| {
                    let body = bodies.iter().find(|b| b.0.slot == slot);
                    p.ship.zip(body).is_some_and(|(ship, b)| orbit_status(&ship.p, &b.1, b.2, b.3, &rules).ok)
                };
                let on = p.hold.map(|h| h.0).filter(|slot| around(*slot)).or_else(|| bodies.iter().map(|b| b.0.slot).find(|slot| around(*slot)));
                p.hold = on.map(|slot| (slot, p.hold.filter(|h| h.0 == slot).map_or(1, |h| h.1 + 1)));
                if let Some((slot, _)) = p.hold.filter(|h| h.1 >= goal) {
                    if let Some(b) = bodies.iter().find(|b| b.0.slot == slot).filter(|_| !gifts.iter().any(|g| g.1.slot == slot)) {
                        gifts.push((p.id, b.0));
                    }
                }
            }
        }
        for (player, special) in gifts {
            self.specials.retain(|s| s.slot != special.slot);
            self.special_due.push((special.kind, comeback));
            self.give(tick, player, special);
        }
        let slots: Vec<u32> = self.specials.iter().map(|s| s.slot).collect();
        for p in self.players.values_mut() {
            p.hold = p.hold.filter(|h| slots.contains(&h.0));
        }

        // As many of each kind as wanted: the ones coming back first, then new ones at once.
        if playing {
            let wanted = self.specials_wanted();
            for kind in SpecialKind::ALL {
                let have = |a: &Authority| a.specials.iter().filter(|s| s.kind == kind).count();
                while have(self) < wanted {
                    let waiting = self.special_due.iter().filter(|d| d.0 == kind).count();
                    let due = self.special_due.iter().position(|d| d.0 == kind && d.1 <= tick);
                    if waiting + have(self) >= wanted && due.is_none() {
                        break;
                    }
                    let Some(slot) = self.choose_special() else { break };
                    if let Some(i) = due {
                        self.special_due.remove(i);
                    }
                    self.specials.push(Special { kind, slot });
                }
            }
            // Whatever is due beyond what is wanted is not coming back.
            self.special_due.retain(|d| d.1 > tick);
        }
        if self.specials != before {
            let specials = self.specials.clone();
            self.event(Event::Specials { tick, specials });
        }
        self.players.values().filter_map(|p| p.hold.map(|(slot, held)| (p.id, slot, held))).collect()
    }

    fn give(&mut self, tick: Tick, player: PlayerId, special: Special) {
        let rules = self.rules.clone();
        let Some(p) = self.players.get_mut(&player) else { return };
        match special.kind {
            SpecialKind::Armoury => p.magazine = Magazine::armoury(tick, &rules),
            SpecialKind::Armour => p.armour = true,
            SpecialKind::Shipyard => {
                if let Some(b) = self.bases.iter_mut().find(|b| b.owner == player && b.hp > 0) {
                    b.hp = BASE_HP;
                    self.bases_changed = true;
                }
            }
        }
        self.event(Event::Gift { tick, player, kind: special.kind, slot: special.slot });
    }

    /// The special bodies right now.
    pub fn specials(&self) -> Vec<Special> {
        self.specials.clone()
    }

    pub fn armoured(&self, player: PlayerId) -> Option<bool> {
        self.players.get(&player).map(|p| p.armour)
    }

    pub fn magazine(&self, player: PlayerId) -> Option<Magazine> {
        self.players.get(&player).map(|p| p.magazine)
    }
}

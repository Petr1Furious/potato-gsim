//! Crystal mines, in the bases mode: ordinary bodies near the center that give a crystal to
//! whoever holds an orbit around them for a while (the same orbit as for a target), and another
//! for every while longer, until the mine runs out and moves on to another body.

use super::*;

/// Seconds before a mine that ran out (or was lost) opens again elsewhere.
const COMEBACK_SECONDS: f64 = 20.0;

impl Authority {
    /// How many mines there should be: one, another for every two players, and maybe one more
    /// (as the round rolled).
    fn mines_wanted(&self) -> usize {
        1 + self.players.values().filter(|p| !p.spectating).count() / 2 + self.roll.mines as usize
    }

    /// A body for a new mine: a planet among the few closest to the center that is fit for it
    /// (see `fit_for_interest`), no mine already and not one of `avoid`. `None` if there is none.
    pub(super) fn choose_mine(&mut self, avoid: &[u32]) -> Option<u32> {
        let frame = SystemFrame::of(&self.massive.kinematics());
        let m = &self.massive;
        let mut cands: Vec<(u32, f64)> = (0..m.len())
            .filter(|&j| !avoid.contains(&(j as u32)) && self.fit_for_interest(&frame, j, true))
            .filter(|&j| !self.mines.iter().any(|s| s.slot as usize == j))
            .map(|j| (j as u32, m.x[j] * m.x[j] + m.y[j] * m.y[j]))
            .collect();
        // The heavier half: their orbits are roomy enough to fly.
        cands.sort_by(|a, b| m.mass[b.0 as usize].total_cmp(&m.mass[a.0 as usize]).then(a.0.cmp(&b.0)));
        cands.truncate((cands.len() / 2).max(8.min(cands.len())));
        cands.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        cands.truncate(5);
        (!cands.is_empty()).then(|| cands[(self.rng.u64() % cands.len() as u64) as usize].0)
    }

    /// Once a tick in a round of bases: mines follow their bodies through merges, give way
    /// when lost, give crystals to whoever holds an orbit long enough, open again elsewhere
    /// after a while, and are kept as many as wanted. Returns the holds to report.
    pub(super) fn tend_mines(&mut self, tick: Tick, merges: &[MergeEvent], playing: bool) -> Vec<(PlayerId, u32, u32)> {
        let rules = self.rules.clone();
        let before = self.mines.clone();
        let comeback = tick + (COMEBACK_SECONDS * rules.tick_hz as f64) as Tick;
        for k in (0..self.mines.len()).rev() {
            let old = self.mines[k].slot;
            let survivor = merges.iter().find(|e| e.absorbed.contains(&old)).map(|e| e.survivor);
            let slot = match survivor {
                Some(Some(s)) => s,
                Some(None) => u32::MAX,
                None => old,
            };
            // Gone, swallowed by a base, or merged with another mine.
            let lost = slot == u32::MAX || !self.massive.alive[slot as usize] || self.massive.fixed[slot as usize] || self.mines[..k].iter().any(|s| s.slot == slot);
            if lost {
                self.mines.remove(k);
                self.mine_due.push(comeback);
                continue;
            }
            self.mines[k].slot = slot;
            for p in self.players.values_mut() {
                p.hold = p.hold.map(|(s, held)| (if s == old { slot } else { s }, held));
            }
        }
        // Once a second: a mine swung around by a heavier body moves to it (or, when that one is
        // a mine already, to the body most like it in place and motion); one far out or racing
        // away (that nobody is on the way to) somewhere near the center again.
        if tick.is_multiple_of(rules.tick_hz.max(1) as Tick) {
            let frame = SystemFrame::of(&self.massive.kinematics());
            for k in 0..self.mines.len() {
                let slot = self.mines[k].slot;
                let new = match self.trouble(&frame, slot, true) {
                    None => continue,
                    Some(Trouble::Swung(heavy)) if !self.mines.iter().any(|s| s.slot == heavy) => Some(heavy),
                    Some(Trouble::Swung(_)) => {
                        let taken: Vec<u32> = self.mines.iter().map(|s| s.slot).collect();
                        self.most_alike(slot as usize, &taken, true)
                    }
                    Some(Trouble::Far) => self.choose_mine(&[slot]),
                };
                if let Some(new) = new {
                    self.mines[k].slot = new;
                    self.forget_holds(slot);
                }
            }
        }

        // Every so long on the orbit, a crystal (while the mine has any).
        let mut mined: Vec<(PlayerId, u32)> = Vec::new();
        if playing {
            let every = (MINE_SECONDS * rules.tick_hz as f64) as u32;
            let bodies: Vec<(u32, Particle, f64, f64)> = {
                let m = &self.massive;
                self.mines.iter().map(|s| s.slot as usize).map(|j| (j as u32, Particle { x: m.x[j], y: m.y[j], vx: m.vx[j], vy: m.vy[j] }, m.pull[j], m.radius[j])).collect()
            };
            for p in self.players.values_mut() {
                let around = |slot: u32| {
                    let body = bodies.iter().find(|b| b.0 == slot);
                    p.ship.zip(body).is_some_and(|(ship, b)| orbit_status(&ship.p, &b.1, b.2, b.3, &rules).ok)
                };
                let on = p.hold.map(|h| h.0).filter(|slot| around(*slot)).or_else(|| bodies.iter().map(|b| b.0).find(|slot| around(*slot)));
                p.hold = on.map(|slot| (slot, p.hold.filter(|h| h.0 == slot).map_or(1, |h| h.1 + 1)));
                if let Some((slot, _)) = p.hold.filter(|h| h.1.is_multiple_of(every.max(1))) {
                    mined.push((p.id, slot));
                }
            }
        }
        for (player, slot) in mined {
            let Some(k) = self.mines.iter().position(|m| m.slot == slot && m.left > 0) else { continue };
            self.mines[k].left -= 1;
            if self.mines[k].left == 0 {
                self.mines.remove(k);
                self.mine_due.push(comeback);
            }
            if let Some(p) = self.players.get_mut(&player) {
                p.cargo.add(Purse { scrap: 0, crystals: 1 });
            }
            self.event(Event::Mined { tick, player, slot });
            self.loadout(tick, player);
        }
        let slots: Vec<u32> = self.mines.iter().map(|s| s.slot).collect();
        for p in self.players.values_mut() {
            p.hold = p.hold.filter(|h| slots.contains(&h.0));
        }

        // As many as wanted: the ones coming back first, then new ones at once.
        if playing {
            let wanted = self.mines_wanted();
            while self.mines.len() < wanted {
                let waiting = self.mine_due.len();
                let due = self.mine_due.iter().position(|d| *d <= tick);
                if waiting + self.mines.len() >= wanted && due.is_none() {
                    break;
                }
                let Some(slot) = self.choose_mine(&[]) else { break };
                if let Some(i) = due {
                    self.mine_due.remove(i);
                }
                self.mines.push(Mine { slot, left: MINE_STOCK });
            }
            // Whatever is due beyond what is wanted is not coming back.
            self.mine_due.retain(|d| *d > tick);
        }
        if self.mines != before {
            let mines = self.mines.clone();
            self.event(Event::Mines { tick, mines });
        }
        self.players.values().filter_map(|p| p.hold.map(|(slot, held)| (p.id, slot, held))).collect()
    }

    /// The mines right now.
    pub fn mines(&self) -> Vec<Mine> {
        self.mines.clone()
    }

    pub fn shielded(&self, player: PlayerId) -> Option<bool> {
        self.players.get(&player).map(|p| p.shield)
    }

    pub fn magazine(&self, player: PlayerId) -> Option<Magazine> {
        self.players.get(&player).map(|p| p.magazine)
    }
}

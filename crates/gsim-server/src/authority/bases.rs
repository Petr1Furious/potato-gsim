//! The bases mode. Every player owns a base: a body that never moves, pushes other bodies
//! away and pulls ships and shells in like a small gas giant (within its reach). Ships respawn around their own base
//! while it stands; enemy shells, rams and siege orbits wear it down. A player whose base is
//! gone is out once their ship is lost, and the last player in play wins the round.

use super::*;

/// What ships and shells feel: an Earth, near the base (see [`REACH_RADII`]).
const BASE_PULL: f64 = 6.0e24;
/// What other bodies feel: a push that keeps the neighborhood of a base clear, so that ships
/// circling it are not thrown about by passing bodies. (Three times as much hardly clears it
/// better and halves the bodies left near the center within three minutes.)
const BASE_MASS: f64 = -3.0e25;
/// Ships feel a base's pull this many base radii out; beyond, it turns into the push bodies
/// feel (wholly at 1.41 times as far): a ship circling some planet far away is not dragged off
/// its orbit by bases the planet feels differently. The home zone ends there too;
/// the siege band and the spawn orbit are well inside.
const REACH_RADII: f64 = HOME_RADII;
/// Ships appear on a circular orbit this many base radii out (480 Mm, 0.9 km/s), inside the
/// home zone and high enough that full thrust is ten times what the base pulls with there: from
/// anywhere on the orbit, whichever way the ship is moving, a straight burn in any direction at
/// least 45° away from the base takes it clear.
const SPAWN_RADII: f64 = 75.0;
const SHELL_DAMAGE: u32 = 120;
/// A ship crashing into an enemy base (and dying for it).
const RAM_DAMAGE: u32 = 600;
/// What a ship on a qualifying orbit around an enemy base takes off per second.
const SIEGE_PER_SECOND: u32 = 60;

/// A team's base, and what the team shares: its bank and the upgrades bought for the base.
pub(super) struct BaseState {
    pub team: TeamId,
    /// The names of its players: whoever comes back under one gets their place back.
    pub names: Vec<String>,
    pub slot: u32,
    pub hp: u32,
    pub bank: Purse,
    /// Only the tiers for the base (see [`Upgrade::BASE`]).
    pub upgrades: Upgrades,
}

impl Authority {
    pub(super) fn base_list(&self) -> Vec<Base> {
        self.bases.iter().map(|b| Base { team: b.team, slot: b.slot, hp: b.hp }).collect()
    }

    /// One base for every team (made of everyone who plays, not those an operator set to
    /// watch, as the round rolled), evenly on a ring around the center. Called with the new
    /// world's bodies, before it is built.
    pub(super) fn lay_bases(&mut self, sc: &mut Scenario) {
        self.bases.clear();
        for p in self.players.values_mut() {
            p.team = 0;
        }
        let bases = self.mode == GameMode::Bases;
        self.rules.base_reach = if bases { REACH_RADII * BASE_RADIUS } else { 0.0 };
        if !bases {
            return;
        }
        let teams = self.form_teams();
        // Out where ships of the targets mode would spawn, as far apart as the world allows.
        let (ring, n) = (sc.spawn_r.1, teams.len().max(1) as f64);
        let phase = self.rng.angle();
        for (k, members) in teams.into_iter().enumerate() {
            let team = k as TeamId + 1;
            let a = phase + std::f64::consts::TAU * k as f64 / n;
            let slot = sc.bodies.len() as u32;
            sc.bodies.push(gsim_core::Body { x: ring * a.cos(), y: ring * a.sin(), vx: 0.0, vy: 0.0, mass: BASE_MASS, radius: BASE_RADIUS });
            let names: Vec<String> = members.iter().filter_map(|id| self.players.get(id)).map(|p| p.name.clone()).collect();
            let label = if self.roll.teams > 1 { format!("{} base", capitalized(team_name(team))) } else { format!("{}'s base", names[0]) };
            sc.names.push((slot, label));
            for id in &members {
                if let Some(p) = self.players.get_mut(id) {
                    p.team = team;
                }
            }
            self.bases.push(BaseState { team, names, slot, hp: BASE_HP, bank: Purse::default(), upgrades: Upgrades::default() });
        }
    }

    /// Who plays on which team this round: everyone on their own, or split at random into
    /// teams of the size the round rolled (as even as they come; an operator's picks first).
    fn form_teams(&mut self) -> Vec<Vec<PlayerId>> {
        let mut playing: Vec<PlayerId> = self.players.values().filter(|p| !p.spectating).map(|p| p.id).collect();
        let size = self.roll.teams.max(1) as usize;
        if size == 1 {
            return playing.into_iter().map(|id| vec![id]).collect();
        }
        // Shuffled, then dealt out in turn.
        for k in (1..playing.len()).rev() {
            let j = (self.scrap_rng.u64() % (k as u64 + 1)) as usize;
            playing.swap(k, j);
        }
        let count = playing.len().div_ceil(size).max(2);
        let mut teams: Vec<Vec<PlayerId>> = vec![Vec::new(); count];
        let pinned = |a: &Authority, id: &PlayerId| a.players.get(id).and_then(|p| a.team_picks.get(&p.name)).map(|t| (*t as usize - 1) % count);
        for id in playing.iter().filter(|id| pinned(self, id).is_some()) {
            teams[pinned(self, id).unwrap()].push(*id);
        }
        for id in playing.iter().filter(|id| pinned(self, id).is_none()) {
            let k = (0..count).min_by_key(|k| teams[*k].len()).unwrap();
            teams[k].push(*id);
        }
        teams.retain(|t| !t.is_empty());
        teams
    }

    /// Fix the bases of the world just built in place.
    pub(super) fn anchor_bases(&mut self) {
        for b in &self.bases {
            self.massive.anchor(b.slot as usize, BASE_PULL);
        }
    }

    /// The player's team's base.
    pub(super) fn base_of(&self, player: PlayerId) -> Option<&BaseState> {
        let team = self.players.get(&player).map_or(0, |p| p.team);
        self.bases.iter().find(|b| b.team == team && team != 0)
    }

    pub(super) fn base_of_mut(&mut self, player: PlayerId) -> Option<&mut BaseState> {
        let team = self.players.get(&player).map_or(0, |p| p.team);
        self.bases.iter_mut().find(|b| b.team == team && team != 0)
    }

    /// In the bases mode a ship respawns while its base stands (and, on an operator's word,
    /// even when it does not).
    pub(super) fn base_alive(&self, player: PlayerId) -> bool {
        self.base_of(player).is_some_and(|b| b.hp > 0)
    }

    /// The player's ship is within their own standing base's home zone (see
    /// [`shop::zone_radii`]), where shells cost nothing.
    pub(super) fn at_home(&self, player: PlayerId) -> bool {
        self.base_alive(player) && self.in_zone(player)
    }

    /// The player's ship is within their own base's home zone, standing or not: where cargo is
    /// banked and the shop is open.
    pub(super) fn in_zone(&self, player: PlayerId) -> bool {
        let Some(b) = self.base_of(player) else { return false };
        let Some(s) = self.players.get(&player).and_then(|p| p.ship) else { return false };
        let (m, j) = (&self.massive, b.slot as usize);
        let zone = shop::zone_radii(&b.upgrades) * m.radius[j];
        (s.p.x - m.x[j]).powi(2) + (s.p.y - m.y[j]).powi(2) <= zone * zone
    }

    /// A circular orbit around the player's base, if they have one.
    pub(super) fn base_spawn(&mut self, player: PlayerId) -> Option<Particle> {
        let slot = self.base_of(player)?.slot as usize;
        let (m, a) = (&self.massive, self.rng.angle());
        let r = SPAWN_RADII * m.radius[slot];
        let v = (self.rules.g * m.pull[slot] / r).sqrt();
        let (c, s) = (a.cos(), a.sin());
        let p = Particle { x: m.x[slot] + r * c, y: m.y[slot] + r * s, vx: -v * s, vy: v * c };
        Some(p)
    }

    /// Out of the round: watching until the next one.
    pub(super) fn eliminate(&mut self, player: PlayerId) {
        let tick = self.tick();
        let Some(p) = self.players.get_mut(&player).filter(|p| !p.spectating) else { return };
        (p.spectating, p.benched) = (true, true);
        (p.ship, p.respawn_tick, p.hold, p.shield) = (None, None, None, false);
        let line = format!("{} is out", p.name);
        self.log(line);
        self.event(Event::Gamemode { tick, player, spectating: true });
    }

    /// `by` hurts the base on `slot` (their own too: shells and rams do not tell friend from
    /// foe), if it still stands, as much as its armor lets them.
    pub(super) fn damage_base(&mut self, tick: Tick, slot: u32, by: PlayerId, amount: u32) {
        let amount = self.bases.iter().find(|b| b.slot == slot).map_or(amount, |b| shop::armored(amount, &b.upgrades));
        self.hurt(tick, slot, Some(by), amount);
    }

    /// `amount` off the base on `slot`, if it still stands (`by` nobody: sudden death).
    fn hurt(&mut self, tick: Tick, slot: u32, by: Option<PlayerId>, amount: u32) {
        let Some(b) = self.bases.iter_mut().find(|b| b.slot == slot && b.hp > 0) else { return };
        b.hp = b.hp.saturating_sub(amount);
        let (team, left) = (b.team, b.hp);
        self.bases_changed = true;
        if let Some(by) = by.filter(|by| !self.base_hits.contains(&(team, *by))) {
            self.base_hits.push((team, by));
        }
        if left > 0 {
            return;
        }
        if let Some(p) = by.and_then(|by| self.players.get_mut(&by)).filter(|p| p.team != team) {
            p.captures += 1;
        }
        self.event(Event::BaseDestroyed { tick, team, by });
        let base = if self.roll.teams > 1 { format!("the {} base", team_name(team)) } else { format!("{}'s base", self.team_label(team)) };
        let line = match by.and_then(|by| self.players.get(&by)) {
            Some(p) if p.team == team => format!("{} destroyed their own base", p.name),
            Some(p) => format!("{} destroyed {base}", p.name),
            None => format!("{}{} fell", base[..1].to_uppercase(), &base[1..]),
        };
        self.log(line);
        // Whoever is waiting for a ship gets none.
        let waiting: Vec<PlayerId> = self.players.values().filter(|p| p.team == team && p.ship.is_none()).map(|p| p.id).collect();
        for id in waiting {
            self.eliminate(id);
        }
    }

    /// Once a tick in the bases mode: sieges wear bases down, the last player in play wins,
    /// and everyone hears how the bases stand.
    pub(super) fn tend_bases(&mut self, tick: Tick, merges: &[MergeEvent], playing: bool) {
        let rules = self.rules.clone();
        let holds = self.tend_mines(tick, merges, playing);
        self.tend_scrap(tick, playing);
        let mut sieges = Vec::new();
        if playing {
            for p in self.players.values() {
                let Some(ship) = p.ship else { continue };
                for b in self.bases.iter().filter(|b| b.hp > 0 && b.team != p.team) {
                    let (m, j) = (&self.massive, b.slot as usize);
                    let body = Particle { x: m.x[j], y: m.y[j], vx: m.vx[j], vy: m.vy[j] };
                    if orbit_status(&ship.p, &body, m.pull[j], m.radius[j], &rules).ok {
                        sieges.push((p.id, b.team));
                    }
                }
            }
        }
        // Spread evenly over the ticks of a second, in whole points (as much a second as the
        // armor lets through).
        let hz = rules.tick_hz.max(1) as Tick;
        for (by, team) in sieges.clone() {
            let Some(b) = self.bases.iter().find(|b| b.team == team) else { continue };
            let rate = shop::armored(SIEGE_PER_SECOND, &b.upgrades) as Tick;
            let per_tick = (rate * (tick % hz + 1) / hz - rate * (tick % hz) / hz) as u32;
            let slot = b.slot;
            self.hurt(tick, slot, Some(by), per_tick);
        }
        // Sudden death: every standing base wears down.
        if playing && self.sudden_death.is_some_and(|t| tick >= t) {
            let rate = (BASE_HP as f64 * self.decay_percent / 100.0).round() as Tick;
            let per_tick = (rate * (tick % hz + 1) / hz - rate * (tick % hz) / hz) as u32;
            let standing: Vec<u32> = self.bases.iter().filter(|b| b.hp > 0).map(|b| b.slot).collect();
            for slot in standing {
                self.hurt(tick, slot, None, per_tick);
            }
        }
        if sieges != self.sieges {
            self.sieges = sieges;
            self.bases_changed = true;
        }
        // Which teams are still in play: those with somebody here playing. When one is left it
        // has won (and when a lone player is out, that round is over too); the next round
        // starts after the intermission.
        if playing && !self.bases.is_empty() {
            let mut left: Vec<TeamId> = self.players.values().filter(|p| !p.spectating && p.team != 0).map(|p| p.team).collect();
            left.sort_unstable();
            left.dedup();
            let contest = self.bases.len() >= 2;
            if left.is_empty() || (contest && left.len() == 1) {
                self.end_round(tick, left.first().copied().filter(|_| contest));
            }
        }
        if ((self.bases_changed || !holds.is_empty()) && tick.is_multiple_of(10)) || tick.is_multiple_of(hz) {
            self.bases_changed = false;
            let (bases, sieges, hits) = (self.base_list(), self.sieges.clone(), std::mem::take(&mut self.base_hits));
            self.send(Target::All, ServerMsg::Bases { bases, sieges, holds, hits });
        }
    }

    /// A player who comes back under the name of one of a team's players gets their place on
    /// it back: (the team, whether its base stands).
    pub(super) fn reclaim_base(&self, name: &str) -> Option<(TeamId, bool)> {
        let b = self.bases.iter().find(|b| b.names.iter().any(|n| n == name))?;
        Some((b.team, b.hp > 0))
    }

    /// Switch modes: a new round starts in the new one (once somebody is here).
    pub fn set_mode(&mut self, mode: GameMode) {
        self.mode = mode;
        if !self.players.is_empty() {
            self.start_round();
        }
    }

    pub fn mode(&self) -> GameMode {
        self.mode
    }

    /// (team, slot, hp) of every base.
    pub fn bases(&self) -> Vec<Base> {
        self.base_list()
    }

    pub fn spectating(&self, player: PlayerId) -> Option<bool> {
        self.players.get(&player).map(|p| p.spectating)
    }

    /// The player's team (0: none).
    pub fn team(&self, player: PlayerId) -> Option<TeamId> {
        self.players.get(&player).map(|p| p.team)
    }

    /// The player's team's base.
    pub fn base_of_player(&self, player: PlayerId) -> Option<Base> {
        self.base_of(player).map(|b| Base { team: b.team, slot: b.slot, hp: b.hp })
    }
}

/// Damage numbers, for tests.
pub const DAMAGE: (u32, u32, u32) = (SHELL_DAMAGE, RAM_DAMAGE, SIEGE_PER_SECOND);

impl Authority {
    /// Test hook: `by` takes `amount` off the base of `owner`'s team.
    pub fn hurt_base(&mut self, owner: PlayerId, by: PlayerId, amount: u32) {
        let tick = self.tick();
        if let Some(slot) = self.base_of(owner).map(|b| b.slot) {
            self.damage_base(tick, slot, by, amount);
        }
    }
}

impl Authority {
    /// A new round of bases rolls how much scrap there is, whether there is a mine more, and a
    /// twist; the rules start from the plain ones, bent by it.
    pub(super) fn roll_round(&mut self) {
        self.rules = self.plain_rules.clone();
        self.roll = Roll::default();
        if self.mode != GameMode::Bases {
            return;
        }
        let rng = &mut self.scrap_rng;
        let mut pick = |n: usize| (rng.u64() % n as u64) as usize;
        // Teams only with four or more playing: of two, or of three with six or more.
        let playing = self.players.values().filter(|p| !p.spectating).count();
        let sizes: Vec<u32> = [1, 2, 3].into_iter().filter(|k| *k == 1 || playing >= 2 * *k as usize).collect();
        let teams = sizes[pick(sizes.len())];
        let rolled = Roll { scrap: [60, 100, 100, 160][pick(4)], mines: pick(2) as u32, twist: Twist::ALL[pick(Twist::ALL.len())], teams };
        // What an operator set wins over the roll (and over a test's fixed one).
        let base = self.fixed_roll.unwrap_or(rolled);
        self.roll = Roll { twist: self.forced_twist.unwrap_or(base.twist), teams: self.forced_teams.unwrap_or(base.teams), ..base };
        let r = &mut self.rules;
        match self.roll.twist {
            Twist::DeepTanks => (r.fuel_max_mmps, r.regen_per_tick_mmps) = (2 * r.fuel_max_mmps, 2 * r.regen_per_tick_mmps),
            Twist::QuickGuns => (r.shell_cooldown_ticks, r.shell_regen_ticks) = (r.shell_cooldown_ticks / 2, r.shell_regen_ticks / 2),
            Twist::LongShots => (r.shell_speed_max, r.shell_lifetime_ticks) = (1.5 * r.shell_speed_max, 2 * r.shell_lifetime_ticks),
            _ => {}
        }
    }

    /// Test hook: every round from the next one on is rolled like this.
    pub fn fix_roll(&mut self, roll: Roll) {
        self.fixed_roll = Some(roll);
    }

    /// How this round differs from the others.
    pub fn roll(&self) -> Roll {
        self.roll
    }
}

/// "Red" for "red".
fn capitalized(word: &str) -> String {
    let mut c = word.chars();
    c.next().map_or(String::new(), |f| f.to_uppercase().chain(c).collect())
}

impl Authority {
    /// `/mode` settings: `twist=`, `teams=`, `sudden=` (seconds, 0 for none) and `decay=` (percent
    /// a second). All of them or none take effect.
    pub(super) fn mode_settings(&mut self, words: &[String]) -> Result<(), String> {
        let (mut twist, mut teams, mut sudden, mut decay) = (self.forced_twist, self.forced_teams, self.sudden_seconds, self.decay_percent);
        for word in words {
            let (key, value) = word.split_once('=').ok_or(format!("expected key=value, not {word:?}"))?;
            let number = || value.parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0).ok_or(format!("{key} takes a number"));
            match key {
                "twist" if value == "random" => twist = None,
                "twist" => twist = Some(Twist::ALL.into_iter().find(|t| t.key() == value).ok_or(format!("no twist called {value:?}"))?),
                "teams" => {
                    teams = match value {
                        "roll" => None,
                        "off" => Some(1),
                        "2" | "3" => Some(value.parse().unwrap()),
                        _ => return Err("teams are roll, off, 2 or 3".to_string()),
                    }
                }
                "sudden" => sudden = number()?,
                "decay" => decay = number()?.max(0.01),
                _ => return Err(format!("no setting called {key:?} (twist, teams, sudden, decay)")),
            }
        }
        (self.forced_twist, self.forced_teams, self.sudden_seconds, self.decay_percent) = (twist, teams, sudden, decay);
        Ok(())
    }

    /// "Mode bases: twist=random teams=roll sudden=720 decay=1".
    pub(super) fn mode_summary(&self) -> String {
        let twist = self.forced_twist.map_or("random", |t| t.key());
        let teams = match self.forced_teams {
            None => "roll".to_string(),
            Some(1) => "off".to_string(),
            Some(k) => k.to_string(),
        };
        format!("Mode {}: twist={twist} teams={teams} sudden={} decay={}", self.mode.name(), self.sudden_seconds, self.decay_percent)
    }
}

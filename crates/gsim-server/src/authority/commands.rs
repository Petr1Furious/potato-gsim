//! Chat, map markers and the `/commands` typed into chat.

use super::*;
use gsim_proto::command::{self, parse_metres};

/// At most this many chat lines per window, per player.
const CHAT_BURST: u32 = 10;
const CHAT_WINDOW_SECONDS: u64 = 4;

type Reply = Result<Option<String>, String>;

fn done(text: impl Into<String>) -> Reply {
    Ok(Some(text.into()))
}

impl Authority {
    pub(super) fn is_op(&self, id: PlayerId) -> bool {
        self.op_all || self.players.get(&id).is_some_and(|p| self.state.is_op(&p.name))
    }

    /// Tell every connected player whether they are an operator (after the list changed).
    pub(super) fn refresh_ops(&mut self) {
        let online: Vec<(ConnId, PlayerId)> = self.by_conn.iter().map(|(c, p)| (*c, *p)).collect();
        for (conn, id) in online {
            let op = self.is_op(id);
            self.send(Target::One(conn), ServerMsg::Operator(op));
        }
    }

    fn tell(&mut self, conn: ConnId, kind: ChatKind, text: impl Into<String>) {
        self.send(Target::One(conn), ServerMsg::Chat(ChatLine { kind, from: None, text: text.into() }));
    }

    fn announce(&mut self, text: impl Into<String>) {
        self.send(Target::All, ServerMsg::Chat(ChatLine { kind: ChatKind::System, from: None, text: text.into() }));
    }

    fn conn_of(&self, id: PlayerId) -> Option<ConnId> {
        self.by_conn.iter().find(|(_, p)| **p == id).map(|(c, _)| *c)
    }

    fn name_of(&self, id: PlayerId) -> String {
        self.players.get(&id).map_or_else(|| "?".to_string(), |p| p.name.clone())
    }

    pub(super) fn chat(&mut self, conn: ConnId, text: String) {
        let Some(&id) = self.by_conn.get(&conn) else { return };
        let text = text.chars().filter(|c| !c.is_control()).take(MAX_CHAT_CHARS).collect::<String>().trim().to_string();
        if text.is_empty() {
            return;
        }
        let now = self.tick();
        let window = CHAT_WINDOW_SECONDS * self.rules.tick_hz as Tick;
        let Some(p) = self.players.get_mut(&id) else { return };
        if now.saturating_sub(p.chat_window.0) > window {
            p.chat_window = (now, 0);
        }
        p.chat_window.1 += 1;
        if p.chat_window.1 > CHAT_BURST {
            return self.tell(conn, ChatKind::Error, "slow down");
        }
        match text.strip_prefix('/') {
            Some(line) => self.command(conn, id, line),
            None => {
                let from = Some(self.name_of(id));
                self.send(Target::All, ServerMsg::Chat(ChatLine { kind: ChatKind::Say, from, text }));
            }
        }
    }

    pub(super) fn mark(&mut self, conn: ConnId, x: f64, y: f64) {
        let Some(&id) = self.by_conn.get(&conn) else { return };
        let now = self.tick();
        let gap = self.rules.tick_hz as Tick / 2;
        let Some(p) = self.players.get_mut(&id) else { return };
        if !(x.is_finite() && y.is_finite()) || (p.last_mark != 0 && now < p.last_mark + gap) {
            return;
        }
        p.last_mark = now.max(1);
        self.send(Target::All, ServerMsg::Mark { player: id, x, y });
    }

    fn command(&mut self, conn: ConnId, id: PlayerId, line: &str) {
        let words = command::split(line);
        let Some(name) = words.first() else { return };
        let Some(spec) = command::find(name) else {
            return self.tell(conn, ChatKind::Error, format!("unknown command /{name} (try /help)"));
        };
        if spec.op && !self.is_op(id) {
            return self.tell(conn, ChatKind::Error, format!("/{} is for operators", spec.name));
        }
        match self.run(conn, id, spec.name, &words[1..]) {
            Ok(Some(reply)) => self.tell(conn, ChatKind::System, reply),
            Ok(None) => {}
            Err(problem) if problem.is_empty() => self.tell(conn, ChatKind::Error, format!("usage: {}", spec.usage())),
            Err(problem) => self.tell(conn, ChatKind::Error, problem),
        }
    }

    // --- argument helpers --------------------------------------------------------------------

    /// A connected player by name: exact (ignoring case), else a unique prefix.
    fn find_player(&self, word: &str) -> Result<PlayerId, String> {
        let w = word.to_lowercase();
        if let Some(p) = self.players.values().find(|p| p.name.to_lowercase() == w) {
            return Ok(p.id);
        }
        let mut hits = self.players.values().filter(|p| p.name.to_lowercase().starts_with(&w));
        match (hits.next(), hits.next()) {
            (Some(p), None) if !w.is_empty() => Ok(p.id),
            (Some(_), Some(_)) => Err(format!("more than one player matches {word:?}")),
            _ => Err(format!("no player called {word:?} is online")),
        }
    }

    /// The named player, or the caller when the argument is missing.
    fn player_or(&self, word: Option<&String>, me: PlayerId) -> Result<PlayerId, String> {
        word.map_or(Ok(me), |w| self.find_player(w))
    }

    /// A live body: a scenario name, `B<slot>`, a bare slot number, or `@target`.
    fn find_body(&self, word: &str) -> Result<u32, String> {
        let w = word.to_lowercase();
        let slot = if w == "@target" {
            self.target
        } else if let Some((slot, _)) = self.names.iter().find(|(_, n)| n.to_lowercase() == w) {
            Some(*slot)
        } else {
            w.strip_prefix('b').unwrap_or(&w).parse::<u32>().ok()
        };
        slot.filter(|s| self.massive.alive.get(*s as usize).copied().unwrap_or(false)).ok_or_else(|| format!("no body called {word:?}"))
    }

    fn ship_of(&self, id: PlayerId) -> Result<ShipState, String> {
        self.ship(id).ok_or_else(|| format!("{} has no ship right now", self.name_of(id)))
    }

    /// Destroy a ship by decree; it respawns after the usual delay.
    fn destroy(&mut self, id: PlayerId) -> Result<(), String> {
        self.ship_of(id)?;
        let tick = self.tick();
        let respawn_tick = tick + self.rules.respawn_ticks as Tick;
        let playing = self.next_round_tick.is_none();
        if let Some(p) = self.players.get_mut(&id) {
            p.ship = None;
            p.hold = 0;
            p.deaths += playing as u32;
            p.respawn_tick = Some(respawn_tick);
        }
        self.event(Event::ShipDied { tick, player: id, killer: None, body: None, respawn_tick });
        Ok(())
    }

    fn kick(&mut self, id: PlayerId, reason: String) {
        if let Some(conn) = self.conn_of(id) {
            self.send(Target::One(conn), ServerMsg::Reject { reason });
            self.disconnect(conn);
            self.kicks.push(conn);
        }
    }

    /// Where a `/tp` should put `who`: next to a player, near a body, or at coordinates.
    fn destination(&self, who: PlayerId, words: &[String]) -> Result<Particle, String> {
        let ship = self.ship_of(who)?.p;
        match words {
            [x, y] if is_coordinate(x) && is_coordinate(y) => {
                let axis = |word: &str, current: f64| match word.strip_prefix('~') {
                    Some("") => Some(current),
                    Some(offset) => parse_metres(offset).map(|d| current + d),
                    None => parse_metres(word),
                };
                match (axis(x, ship.x), axis(y, ship.y)) {
                    (Some(x), Some(y)) => Ok(Particle { x, y, ..ship }),
                    _ => Err(String::new()),
                }
            }
            [place] => {
                if let Ok(other) = self.find_player(place) {
                    // Just outside each other's blast radius, flying in formation.
                    let o = self.ship_of(other)?.p;
                    return Ok(Particle { x: o.x + 3.0 * self.rules.shell_blast_radius, ..o });
                }
                let slot = self.find_body(place).map_err(|_| format!("{place:?} is neither a player nor a body"))? as usize;
                let m = &self.massive;
                // Outside the scoring band, moving with the body.
                let d = 2.0 * self.rules.orbit_max_apo_radii * m.radius[slot];
                Ok(Particle { x: m.x[slot] + d, y: m.y[slot], vx: m.vx[slot], vy: m.vy[slot] })
            }
            _ => Err(String::new()),
        }
    }

    // --- the commands ------------------------------------------------------------------------

    fn run(&mut self, conn: ConnId, me: PlayerId, name: &str, args: &[String]) -> Reply {
        let hz = self.rules.tick_hz as f64;
        let rest = |from: usize| args.get(from..).unwrap_or(&[]).join(" ");
        match name {
            "help" => {
                let op = self.is_op(me);
                for c in command::COMMANDS.iter().filter(|c| op || !c.op) {
                    self.tell(conn, ChatKind::System, format!("{}  -  {}", c.usage(), c.help));
                }
                Ok(None)
            }
            "list" => {
                let names: Vec<String> =
                    self.players.values().map(|p| if self.state.is_op(&p.name) { format!("{} (op)", p.name) } else { p.name.clone() }).collect();
                done(format!("online: {}", names.join(", ")))
            }
            "msg" | "r" => {
                let (to, text) = if name == "r" {
                    let to = self.players.get(&me).and_then(|p| p.reply_to).filter(|t| self.players.contains_key(t));
                    (to.ok_or("nobody to reply to")?, rest(0))
                } else {
                    (self.find_player(args.first().ok_or("")?)?, rest(1))
                };
                if text.is_empty() {
                    return Err(String::new());
                }
                let (from_name, to_name) = (self.name_of(me), self.name_of(to));
                if let Some(p) = self.players.get_mut(&to) {
                    p.reply_to = Some(me);
                }
                if let Some(p) = self.players.get_mut(&me) {
                    p.reply_to = Some(to);
                }
                if let Some(c) = self.conn_of(to) {
                    let line = ChatLine { kind: ChatKind::Private { outgoing: false }, from: Some(from_name), text: text.clone() };
                    self.send(Target::One(c), ServerMsg::Chat(line));
                }
                let line = ChatLine { kind: ChatKind::Private { outgoing: true }, from: Some(to_name), text };
                self.send(Target::One(conn), ServerMsg::Chat(line));
                Ok(None)
            }
            "respawn" | "kill" => {
                let who = self.player_or(args.first(), me)?;
                if who != me && !self.is_op(me) {
                    return Err("only operators can respawn someone else".into());
                }
                if name == "kill" && args.is_empty() {
                    return Err(String::new());
                }
                self.destroy(who)?;
                Ok(None)
            }
            "round" => {
                let seconds = || args.get(1).and_then(|s| s.parse::<f64>().ok()).filter(|s| s.is_finite() && *s >= 0.0).ok_or(String::new());
                match args.first().map(String::as_str) {
                    Some("new") => {
                        self.start_round();
                        self.announce(format!("{} started a new round", self.name_of(me)));
                    }
                    Some("time") => {
                        let end = self.tick() + (seconds()? * hz) as Tick;
                        self.round_end_tick = Some(end);
                        self.next_round_tick = None;
                        self.event(Event::RoundClock { round_end_tick: self.round_end_tick });
                    }
                    Some("length") => {
                        self.round_ticks = (seconds()? * hz) as Tick;
                        self.round_end_tick = (self.round_ticks > 0).then(|| self.tick() + self.round_ticks);
                        self.event(Event::RoundClock { round_end_tick: self.round_end_tick });
                    }
                    _ => return Err(String::new()),
                }
                Ok(None)
            }
            "tp" => {
                // A leading player name says who moves, when something follows it.
                let named = args.len() >= 2 && !is_coordinate(&args[0]);
                let who = if named { self.find_player(&args[0])? } else { me };
                let place = self.destination(who, &args[named as usize..])?;
                let old = self.ship_of(who)?;
                self.place_ship(who, ShipState { p: place, ..old });
                Ok(None)
            }
            "orbit" => {
                let (who, body) = match args {
                    [body] => (me, body),
                    [who, body] => (self.find_player(who)?, body),
                    _ => return Err(String::new()),
                };
                let slot = self.find_body(body)? as usize;
                let m = &self.massive;
                if m.mass[slot] <= 0.0 {
                    return Err("nothing can orbit a body with negative mass".into());
                }
                let r = 10.0 * m.radius[slot];
                let v = (self.rules.g * m.mass[slot] / r).sqrt();
                let p = Particle { x: m.x[slot] + r, y: m.y[slot], vx: m.vx[slot], vy: m.vy[slot] + v };
                let old = self.ship_of(who)?;
                self.place_ship(who, ShipState { p, ..old });
                Ok(None)
            }
            "preset" => {
                let preset = args.first().ok_or("")?;
                let opts = self.generator.as_ref().map(|g| g.1.clone()).unwrap_or_default();
                scenario::build(preset, 1, &opts)?;
                self.next_seed = args.get(1).map(|s| s.parse::<u64>().map_err(|_| String::new())).transpose()?;
                self.generator = Some((preset.clone(), opts));
                self.start_round();
                self.announce(format!("{} switched the world to {preset}", self.name_of(me)));
                Ok(None)
            }
            "timescale" => {
                let scale = args.first().and_then(|s| s.parse::<f64>().ok()).filter(|x| (1.0..=1.0e7).contains(x)).ok_or("")?;
                self.rules = GameRules { escape_radius: self.rules.escape_radius, ..GameRules::new(scale, self.rules.tick_hz) };
                self.start_round();
                self.announce(format!("{} set the time scale to x{scale}", self.name_of(me)));
                Ok(None)
            }
            "target" => {
                let slot = self.find_body(args.first().ok_or("")?)?;
                self.set_target(slot);
                Ok(None)
            }
            "fuel" => {
                let who = self.player_or(args.first(), me)?;
                let ship = self.ship_of(who)?;
                self.place_ship(who, ShipState { fuel: self.rules.fuel_max_mmps, ..ship });
                Ok(None)
            }
            "god" => {
                let who = self.player_or(args.first(), me)?;
                let p = self.players.get_mut(&who).ok_or("")?;
                p.god = !p.god;
                done(format!("{} is {} to shells", p.name, if p.god { "now immune" } else { "no longer immune" }))
            }
            "score" => {
                let who = self.find_player(args.first().ok_or("")?)?;
                let number = |i: usize| args.get(i).and_then(|s| s.parse::<u32>().ok()).ok_or(String::new());
                let (kills, captures) = (number(1)?, number(2)?);
                let p = self.players.get_mut(&who).ok_or("")?;
                (p.kills, p.captures) = (kills, captures);
                let deaths = p.deaths;
                self.event(Event::Score { player: who, kills, deaths, captures });
                Ok(None)
            }
            "kick" => {
                let who = self.find_player(args.first().ok_or("")?)?;
                let (name, reason) = (self.name_of(who), rest(1));
                self.kick(who, if reason.is_empty() { "kicked by an operator".into() } else { format!("kicked: {reason}") });
                self.announce(format!("{name} was kicked"));
                Ok(None)
            }
            "ban" => {
                // Online players by (possibly abbreviated) name, anyone else by exact name.
                let word = args.first().ok_or("")?;
                let name = self.find_player(word).map(|id| self.name_of(id)).unwrap_or_else(|_| word.clone());
                self.state.ban(&name, &rest(1));
                self.enforce_state();
                self.announce(format!("{name} was banned"));
                Ok(None)
            }
            "unban" => {
                let name = rest(0);
                if self.state.unban(&name) { done(format!("unbanned {name}")) } else { Err(format!("{name} is not banned")) }
            }
            "ban-ip" => {
                let word = args.first().ok_or("")?;
                let ip = word
                    .parse::<IpAddr>()
                    .ok()
                    .or_else(|| self.find_player(word).ok().and_then(|id| self.conn_of(id)).and_then(|c| self.addrs.get(&c).copied()))
                    .or_else(|| self.state.record(word).and_then(|r| r.last_ip))
                    .ok_or_else(|| format!("{word:?} is neither an address nor a player with a known address"))?;
                self.state.ban_ip(ip, &rest(1));
                self.enforce_state();
                done(format!("banned address {ip}"))
            }
            "unban-ip" => {
                let ip = args.first().and_then(|s| s.parse::<IpAddr>().ok()).ok_or("")?;
                if self.state.unban_ip(ip) { done(format!("unbanned {ip}")) } else { Err(format!("{ip} is not banned")) }
            }
            "op" | "deop" => {
                let word = args.first().ok_or("")?;
                let target = self.find_player(word).map(|id| self.name_of(id)).unwrap_or_else(|_| word.clone());
                if name == "op" {
                    self.state.op(&target);
                } else if !self.state.deop(&target) {
                    return Err(format!("{target} is not an operator"));
                }
                self.refresh_ops();
                done(format!("{target} is {} an operator", if name == "op" { "now" } else { "no longer" }))
            }
            "whitelist" => match (args.first().map(String::as_str), args.get(1)) {
                (Some("on"), _) | (Some("off"), _) => {
                    self.state.whitelist_enabled = args[0] == "on";
                    self.enforce_state();
                    done(format!("whitelist {}", args[0]))
                }
                (Some("add"), Some(n)) => {
                    self.state.whitelist_add(n);
                    done(format!("{n} added to the whitelist"))
                }
                (Some("remove"), Some(n)) => {
                    let removed = self.state.whitelist_remove(n);
                    self.enforce_state();
                    if removed { done(format!("{n} removed from the whitelist")) } else { Err(format!("{n} is not on the whitelist")) }
                }
                (Some("list"), _) => {
                    let names: Vec<String> = self.state.whitelist().cloned().collect();
                    let state = if self.state.whitelist_enabled { "on" } else { "off" };
                    done(format!("whitelist ({state}): {}", if names.is_empty() { "empty".into() } else { names.join(", ") }))
                }
                _ => Err(String::new()),
            },
            _ => Err(String::new()),
        }
    }
}

/// `~`, `~1e9`, `-5Gm`, `3e11`: something `/tp` reads as a coordinate rather than a name.
fn is_coordinate(word: &str) -> bool {
    let w = word.strip_prefix('~').unwrap_or(word);
    word.starts_with('~') && w.is_empty() || parse_metres(w).is_some()
}

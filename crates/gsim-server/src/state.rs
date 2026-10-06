//! Persistent server state: who owns which name, the whitelist and the ban lists.
//!
//! Everything lives in plain text files in one directory so it can be read, edited and
//! backed up by hand, much like a Minecraft server:
//!
//! | file                 | one line per entry                                   |
//! |----------------------|------------------------------------------------------|
//! | `players.txt`        | name, public key, first seen (unix), last address    |
//! | `whitelist.txt`      | name                                                 |
//! | `banned-players.txt` | name, reason                                         |
//! | `banned-ips.txt`     | address, reason                                      |
//! | `ops.txt`            | name of a player who may use operator commands       |
//!
//! Fields are separated by tabs; lines starting with `#` are comments. A running server
//! notices edits within a couple of seconds, so `gsim-server admin ...` (or a text editor)
//! is all the administration there is.

use gsim_proto::identity::{fingerprint, from_hex, to_hex};
use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const PLAYERS: &str = "players.txt";
const WHITELIST: &str = "whitelist.txt";
const BANNED_PLAYERS: &str = "banned-players.txt";
const BANNED_IPS: &str = "banned-ips.txt";
const OPS: &str = "ops.txt";
const FILES: [&str; 5] = [PLAYERS, WHITELIST, BANNED_PLAYERS, BANNED_IPS, OPS];

#[derive(Clone, Debug, PartialEq)]
pub struct PlayerRecord {
    /// As first registered (lookups ignore case).
    pub name: String,
    pub key: [u8; 32],
    pub first_seen: u64,
    pub last_ip: Option<IpAddr>,
}

#[derive(Default)]
pub struct ServerState {
    /// `None`: nothing is written to disk (tests, solo play).
    dir: Option<PathBuf>,
    pub whitelist_enabled: bool,
    players: BTreeMap<String, PlayerRecord>,
    whitelist: BTreeSet<String>,
    banned_players: BTreeMap<String, String>,
    banned_ips: BTreeMap<IpAddr, String>,
    ops: BTreeSet<String>,
    stamps: [Option<SystemTime>; 5],
}

/// Names are compared without regard to case or surrounding space.
pub fn name_key(name: &str) -> String {
    name.trim().to_lowercase()
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Data lines of a file, split on tabs.
fn read_rows(path: &Path) -> Vec<Vec<String>> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    text.lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .map(|l| l.split('\t').map(|f| f.trim().to_string()).collect())
        .collect()
}

/// Replace a file in one step, so a crash never leaves half a list behind.
fn write_rows(path: &Path, header: &str, rows: impl Iterator<Item = Vec<String>>) -> std::io::Result<()> {
    let mut text = format!("# {header}\n");
    for row in rows {
        text.push_str(&row.join("\t"));
        text.push('\n');
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

impl ServerState {
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Load (creating the directory and empty files as needed).
    pub fn open(dir: &Path, whitelist_enabled: bool) -> Result<Self, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create state directory {}: {e}", dir.display()))?;
        let mut state = Self { dir: Some(dir.to_path_buf()), whitelist_enabled, ..Default::default() };
        state.load();
        // Make sure every file exists, so there is something to edit.
        state.save_all().map_err(|e| format!("cannot write to {}: {e}", dir.display()))?;
        state.remember_stamps();
        Ok(state)
    }

    fn path(&self, file: &str) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(file))
    }

    fn load(&mut self) {
        let Some(dir) = self.dir.clone() else { return };
        self.players = read_rows(&dir.join(PLAYERS))
            .into_iter()
            .filter_map(|r| {
                let record = PlayerRecord {
                    name: r.first()?.clone(),
                    key: from_hex(r.get(1)?)?,
                    first_seen: r.get(2)?.parse().ok()?,
                    // "-" when the address is unknown.
                    last_ip: r.get(3)?.parse().ok(),
                };
                Some((name_key(&record.name), record))
            })
            .collect();
        self.ops = read_rows(&dir.join(OPS)).into_iter().filter_map(|r| r.first().map(|n| name_key(n))).collect();
        self.whitelist = read_rows(&dir.join(WHITELIST)).into_iter().filter_map(|r| r.first().map(|n| name_key(n))).collect();
        let reason = |r: &[String]| r.get(1).cloned().unwrap_or_default();
        self.banned_players =
            read_rows(&dir.join(BANNED_PLAYERS)).into_iter().filter_map(|r| Some((name_key(r.first()?), reason(&r)))).collect();
        self.banned_ips = read_rows(&dir.join(BANNED_IPS)).into_iter().filter_map(|r| Some((r.first()?.parse().ok()?, reason(&r)))).collect();
    }

    fn save(&self, file: &str) -> std::io::Result<()> {
        let Some(path) = self.path(file) else { return Ok(()) };
        match file {
            PLAYERS => write_rows(
                &path,
                "name <tab> public key <tab> first seen (unix time) <tab> last address. Names are reserved for their key forever; delete a line to free a name.",
                self.players.values().map(|p| {
                    vec![p.name.clone(), to_hex(&p.key), p.first_seen.to_string(), p.last_ip.map_or("-".into(), |ip| ip.to_string())]
                }),
            ),
            WHITELIST => write_rows(
                &path,
                "one name per line. Only used when the server runs with the whitelist enabled.",
                self.whitelist.iter().map(|n| vec![n.clone()]),
            ),
            OPS => write_rows(&path, "one name per line: players who may use operator commands in chat.", self.ops.iter().map(|n| vec![n.clone()])),
            BANNED_PLAYERS => {
                write_rows(&path, "name <tab> reason", self.banned_players.iter().map(|(n, r)| vec![n.clone(), r.clone()]))
            }
            _ => write_rows(&path, "address <tab> reason", self.banned_ips.iter().map(|(ip, r)| vec![ip.to_string(), r.clone()])),
        }
    }

    fn save_all(&self) -> std::io::Result<()> {
        FILES.iter().try_for_each(|f| self.save(f))
    }

    fn stamp(&self, file: &str) -> Option<SystemTime> {
        std::fs::metadata(self.path(file)?).and_then(|m| m.modified()).ok()
    }

    fn remember_stamps(&mut self) {
        for (i, f) in FILES.iter().enumerate() {
            self.stamps[i] = self.stamp(f);
        }
    }

    fn persist(&mut self, file: &str) {
        if let Err(e) = self.save(file) {
            eprintln!("[state] could not write {file}: {e}");
        }
        self.remember_stamps();
    }

    /// Re-read the files if any of them changed on disk. Returns true if something did.
    pub fn reload_if_changed(&mut self) -> bool {
        if self.dir.is_none() {
            return false;
        }
        let changed = FILES.iter().enumerate().any(|(i, f)| self.stamp(f) != self.stamps[i]);
        if changed {
            self.load();
            self.remember_stamps();
        }
        changed
    }

    // --- checks ------------------------------------------------------------------------------

    pub fn ip_ban(&self, ip: IpAddr) -> Option<&str> {
        self.banned_ips.get(&ip).map(String::as_str)
    }

    pub fn name_ban(&self, name: &str) -> Option<&str> {
        self.banned_players.get(&name_key(name)).map(String::as_str)
    }

    pub fn whitelisted(&self, name: &str) -> bool {
        !self.whitelist_enabled || self.whitelist.contains(&name_key(name))
    }

    pub fn record(&self, name: &str) -> Option<&PlayerRecord> {
        self.players.get(&name_key(name))
    }

    /// Why this player may not be here, if they may not (bans and whitelist only).
    pub fn refusal(&self, name: &str, ip: Option<IpAddr>) -> Option<String> {
        let because = |what: &str, reason: &str| if reason.is_empty() { what.to_string() } else { format!("{what}: {reason}") };
        if let Some(reason) = ip.and_then(|ip| self.ip_ban(ip)) {
            return Some(because("your address is banned from this server", reason));
        }
        if let Some(reason) = self.name_ban(name) {
            return Some(because("you are banned from this server", reason));
        }
        if !self.whitelisted(name) {
            return Some("you are not on this server's whitelist".to_string());
        }
        None
    }

    /// Claim `name` for `key`, or confirm an existing claim. Fails if another key owns it.
    pub fn claim(&mut self, name: &str, key: &[u8; 32], ip: Option<IpAddr>) -> Result<(), String> {
        let k = name_key(name);
        match self.players.get_mut(&k) {
            Some(record) if record.key != *key => Err(format!(
                "the name {:?} belongs to another player on this server (key {})",
                record.name,
                fingerprint(&record.key)
            )),
            Some(record) => {
                if ip.is_some() && record.last_ip != ip {
                    record.last_ip = ip;
                    self.persist(PLAYERS);
                }
                Ok(())
            }
            None => {
                self.players.insert(k, PlayerRecord { name: name.trim().to_string(), key: *key, first_seen: now_unix(), last_ip: ip });
                self.persist(PLAYERS);
                Ok(())
            }
        }
    }

    // --- administration ----------------------------------------------------------------------

    pub fn players(&self) -> impl Iterator<Item = &PlayerRecord> {
        self.players.values()
    }

    pub fn whitelist(&self) -> impl Iterator<Item = &String> {
        self.whitelist.iter()
    }

    pub fn banned_players(&self) -> impl Iterator<Item = (&String, &String)> {
        self.banned_players.iter()
    }

    pub fn banned_ips(&self) -> impl Iterator<Item = (&IpAddr, &String)> {
        self.banned_ips.iter()
    }

    pub fn ban(&mut self, name: &str, reason: &str) {
        self.banned_players.insert(name_key(name), reason.replace(['\t', '\n'], " "));
        self.persist(BANNED_PLAYERS);
    }

    pub fn unban(&mut self, name: &str) -> bool {
        let removed = self.banned_players.remove(&name_key(name)).is_some();
        self.persist(BANNED_PLAYERS);
        removed
    }

    pub fn ban_ip(&mut self, ip: IpAddr, reason: &str) {
        self.banned_ips.insert(ip, reason.replace(['\t', '\n'], " "));
        self.persist(BANNED_IPS);
    }

    pub fn unban_ip(&mut self, ip: IpAddr) -> bool {
        let removed = self.banned_ips.remove(&ip).is_some();
        self.persist(BANNED_IPS);
        removed
    }

    pub fn whitelist_add(&mut self, name: &str) {
        self.whitelist.insert(name_key(name));
        self.persist(WHITELIST);
    }

    pub fn whitelist_remove(&mut self, name: &str) -> bool {
        let removed = self.whitelist.remove(&name_key(name));
        self.persist(WHITELIST);
        removed
    }

    /// Free a name so that anyone can claim it again.
    pub fn forget(&mut self, name: &str) -> bool {
        let removed = self.players.remove(&name_key(name)).is_some();
        self.persist(PLAYERS);
        removed
    }
}

/// Operators.
impl ServerState {
    pub fn is_op(&self, name: &str) -> bool {
        self.ops.contains(&name_key(name))
    }

    pub fn ops(&self) -> impl Iterator<Item = &String> {
        self.ops.iter()
    }

    pub fn op(&mut self, name: &str) {
        self.ops.insert(name_key(name));
        self.persist(OPS);
    }

    pub fn deop(&mut self, name: &str) -> bool {
        let removed = self.ops.remove(&name_key(name));
        self.persist(OPS);
        removed
    }
}

//! What the server tells its operator, Minecraft style: who came and went, what was said and
//! done, and how the rounds went. The transport prints it (see `net`).

use super::*;

impl Authority {
    /// A line for the server log.
    pub(super) fn log(&mut self, text: impl Into<String>) {
        self.log.push(("INFO", text.into()));
    }

    /// The lines logged since the last call: (level, text).
    pub fn drain_log(&mut self) -> Vec<(&'static str, String)> {
        std::mem::take(&mut self.log)
    }

    /// How a lost ship is told of, as in the chat.
    pub(super) fn death_line(&self, victim: PlayerId, killer: Option<PlayerId>, body: Option<u32>) -> String {
        let name = |id: PlayerId| self.players.get(&id).map_or("?".to_string(), |p| p.name.clone());
        match (killer, body) {
            (Some(k), _) if k == victim => format!("{} blew themselves up", name(victim)),
            (Some(k), _) => format!("{} destroyed {}", name(k), name(victim)),
            (None, Some(b)) => format!("{} crashed into {}", name(victim), self.body_name(b)),
            _ => format!("{} was destroyed", name(victim)),
        }
    }

    /// "Red" with teams, otherwise the name of the team's player.
    pub(super) fn team_label(&self, team: TeamId) -> String {
        if self.roll.teams > 1 {
            let n = team_name(team);
            n[..1].to_uppercase() + &n[1..]
        } else {
            self.bases.iter().find(|b| b.team == team).and_then(|b| b.names.first().cloned()).unwrap_or_else(|| "?".to_string())
        }
    }
}

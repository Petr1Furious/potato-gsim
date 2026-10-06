//! The chat command table, shared so the server parses exactly what the client completes.

/// What kind of thing an argument is (drives tab completion and the usage line).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Arg {
    Player,
    Body,
    /// A player, a body, or `~dx ~dy` / `x y` coordinates in metres.
    Place,
    Preset,
    Number,
    /// One of a fixed set of words.
    Word(&'static [&'static str]),
    /// The rest of the line.
    Text,
}

pub struct Command {
    pub name: &'static str,
    pub usage: &'static str,
    pub help: &'static str,
    /// Operators only.
    pub op: bool,
    pub args: &'static [Arg],
}

pub const COMMANDS: &[Command] = &[
    Command { name: "help", usage: "/help", help: "list the commands you can use", op: false, args: &[] },
    Command { name: "list", usage: "/list", help: "who is online", op: false, args: &[] },
    Command { name: "msg", usage: "/msg PLAYER TEXT", help: "private message", op: false, args: &[Arg::Player, Arg::Text] },
    Command { name: "r", usage: "/r TEXT", help: "reply to the last private message", op: false, args: &[Arg::Text] },
    Command { name: "respawn", usage: "/respawn [PLAYER]", help: "destroy your ship and start over", op: false, args: &[Arg::Player] },
    Command { name: "round", usage: "/round new | time SECONDS | length SECONDS", help: "restart the round, or set the time left / the round length", op: true, args: &[Arg::Word(&["new", "time", "length"]), Arg::Number] },
    Command { name: "tp", usage: "/tp [PLAYER] PLAYER|BODY|~DX ~DY|X Y", help: "teleport (metres; ~ is relative to the ship)", op: true, args: &[Arg::Place, Arg::Place, Arg::Place] },
    Command { name: "orbit", usage: "/orbit [PLAYER] BODY", help: "put a ship on a circular orbit around a body", op: true, args: &[Arg::Place, Arg::Body] },
    Command { name: "preset", usage: "/preset NAME [SEED]", help: "start a new round in another world", op: true, args: &[Arg::Preset, Arg::Number] },
    Command { name: "timescale", usage: "/timescale X", help: "simulated seconds per second; starts a new round", op: true, args: &[Arg::Number] },
    Command { name: "target", usage: "/target BODY", help: "move the objective", op: true, args: &[Arg::Body] },
    Command { name: "fuel", usage: "/fuel [PLAYER]", help: "refill delta-v", op: true, args: &[Arg::Player] },
    Command { name: "god", usage: "/god [PLAYER]", help: "toggle immunity to shells", op: true, args: &[Arg::Player] },
    Command { name: "kill", usage: "/kill PLAYER", help: "destroy a ship", op: true, args: &[Arg::Player] },
    Command { name: "score", usage: "/score PLAYER KILLS ORBITS", help: "set a player's score", op: true, args: &[Arg::Player, Arg::Number, Arg::Number] },
    Command { name: "kick", usage: "/kick PLAYER [REASON]", help: "disconnect a player", op: true, args: &[Arg::Player, Arg::Text] },
    Command { name: "ban", usage: "/ban PLAYER [REASON]", help: "ban a player by name", op: true, args: &[Arg::Player, Arg::Text] },
    Command { name: "unban", usage: "/unban NAME", help: "lift a ban", op: true, args: &[Arg::Text] },
    Command { name: "ban-ip", usage: "/ban-ip PLAYER|ADDRESS [REASON]", help: "ban an address", op: true, args: &[Arg::Player, Arg::Text] },
    Command { name: "unban-ip", usage: "/unban-ip ADDRESS", help: "lift an address ban", op: true, args: &[Arg::Text] },
    Command { name: "op", usage: "/op PLAYER", help: "make a player an operator", op: true, args: &[Arg::Player] },
    Command { name: "deop", usage: "/deop PLAYER", help: "remove operator rights", op: true, args: &[Arg::Player] },
    Command { name: "whitelist", usage: "/whitelist on | off | add NAME | remove NAME | list", help: "manage the whitelist", op: true, args: &[Arg::Word(&["on", "off", "add", "remove", "list"]), Arg::Player] },
];

pub fn find(name: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|c| c.name.eq_ignore_ascii_case(name))
}

/// Split a command line into words; `"double quotes"` keep spaces together.
pub fn split(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut quoted, mut any) = (false, false);
    for c in line.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}

/// Quote a name for use as one argument if it needs it.
pub fn quote(word: &str) -> String {
    if word.contains(char::is_whitespace) {
        format!("\"{word}\"")
    } else {
        word.to_string()
    }
}

/// A length in metres: a plain number (`2.5e9`) or one with a unit (`300km`, `5Mm`, `2Gm`, `1Tm`).
pub fn parse_metres(word: &str) -> Option<f64> {
    let units = [("km", 1e3), ("Mm", 1e6), ("Gm", 1e9), ("Tm", 1e12), ("m", 1.0)];
    let (number, scale) = units.iter().find_map(|(u, k)| word.strip_suffix(u).map(|n| (n, *k))).unwrap_or((word, 1.0));
    number.parse::<f64>().ok().filter(|v| v.is_finite()).map(|v| v * scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitting_and_units() {
        assert_eq!(split("  tp \"Ann Droid\"   ~1e9 ~-5Mm "), ["tp", "Ann Droid", "~1e9", "~-5Mm"]);
        assert_eq!(split("msg bob \"\" x"), ["msg", "bob", "", "x"]);
        assert_eq!(quote("Ann Droid"), "\"Ann Droid\"");
        assert_eq!(parse_metres("2Gm"), Some(2.0e9));
        assert_eq!(parse_metres("-300km"), Some(-3.0e5));
        assert_eq!(parse_metres("1e9"), Some(1.0e9));
        assert_eq!(parse_metres("12m"), Some(12.0));
        assert_eq!(parse_metres("abc"), None);
        assert!(find("TP").is_some() && find("nope").is_none());
    }
}

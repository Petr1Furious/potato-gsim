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
    /// A length of time such as `6h` or `2d` (see [`parse_span`]).
    Span,
    /// One of a fixed set of words.
    Word(&'static [&'static str]),
    /// The rest of the line.
    Text,
    /// The rest of the line: `key=value` words tuning the preset named before them.
    Settings,
}

/// One argument of a command.
pub struct Param {
    /// Shown in usage, e.g. `player` becomes `<player>` or `[<player>]`.
    pub name: &'static str,
    pub arg: Arg,
    pub optional: bool,
}

const fn req(name: &'static str, arg: Arg) -> Param {
    Param { name, arg, optional: false }
}

const fn opt(name: &'static str, arg: Arg) -> Param {
    Param { name, arg, optional: true }
}

pub struct Command {
    pub name: &'static str,
    pub help: &'static str,
    /// Operators only.
    pub op: bool,
    pub params: &'static [Param],
}

impl Param {
    /// `<name>`, or `[<name>]` when optional, as Minecraft writes it.
    pub fn usage(&self) -> String {
        if self.optional {
            format!("[<{}>]", self.name)
        } else {
            format!("<{}>", self.name)
        }
    }
}

impl Command {
    /// Usage of the parameters from `from` on, e.g. `<kills> <orbits>`.
    pub fn usage_from(&self, from: usize) -> String {
        self.params.iter().skip(from).map(Param::usage).collect::<Vec<_>>().join(" ")
    }

    pub fn usage(&self) -> String {
        format!("/{} {}", self.name, self.usage_from(0)).trim_end().to_string()
    }
}

pub const COMMANDS: &[Command] = &[
    Command { name: "help", help: "list the commands you can use", op: false, params: &[] },
    Command { name: "list", help: "who is online", op: false, params: &[] },
    Command { name: "msg", help: "private message", op: false, params: &[req("player", Arg::Player), req("message", Arg::Text)] },
    Command { name: "r", help: "reply to the last private message", op: false, params: &[req("message", Arg::Text)] },
    Command { name: "respawn", help: "destroy your ship and start over", op: false, params: &[opt("player", Arg::Player)] },
    Command {
        name: "round",
        help: "restart the round, or set the time left / the round length",
        op: true,
        params: &[req("new|time|length", Arg::Word(&["new", "time", "length"])), opt("seconds", Arg::Number)],
    },
    Command {
        name: "tp",
        help: "teleport to a player, a body, or coordinates in metres (~ is relative to the ship); name a player first to move them instead",
        op: true,
        params: &[req("player|body|x", Arg::Place), opt("player|body|x|y", Arg::Place), opt("y", Arg::Place)],
    },
    Command {
        name: "orbit",
        help: "put a ship on a circular orbit around a body",
        op: true,
        params: &[req("player|body", Arg::Place), opt("body", Arg::Body)],
    },
    Command { name: "preset", help: "start a new round in another world; settings not given (seed=, and the preset's own) return to their defaults", op: true, params: &[req("preset", Arg::Preset), opt("key=value ...", Arg::Settings)] },
    Command { name: "timescale", help: "simulated seconds per second; starts a new round", op: true, params: &[req("factor", Arg::Number)] },
    Command { name: "target", help: "move the objective", op: true, params: &[req("body", Arg::Body)] },
    Command { name: "fuel", help: "refill delta-v", op: true, params: &[opt("player", Arg::Player)] },
    Command { name: "god", help: "toggle immunity to shells", op: true, params: &[opt("player", Arg::Player)] },
    Command { name: "kill", help: "destroy a ship", op: true, params: &[req("player", Arg::Player)] },
    Command {
        name: "score",
        help: "set a player's score",
        op: true,
        params: &[req("player", Arg::Player), req("kills", Arg::Number), req("orbits", Arg::Number)],
    },
    Command { name: "kick", help: "disconnect a player", op: true, params: &[req("player", Arg::Player), opt("reason", Arg::Text)] },
    Command { name: "ban", help: "ban a player by name", op: true, params: &[req("player", Arg::Player), opt("reason", Arg::Text)] },
    Command { name: "unban", help: "lift a ban", op: true, params: &[req("name", Arg::Text)] },
    Command { name: "ban-ip", help: "ban an address", op: true, params: &[req("player|address", Arg::Player), opt("reason", Arg::Text)] },
    Command { name: "unban-ip", help: "lift an address ban", op: true, params: &[req("address", Arg::Text)] },
    Command { name: "op", help: "make a player an operator", op: true, params: &[req("player", Arg::Player)] },
    Command { name: "deop", help: "remove operator rights", op: true, params: &[req("player", Arg::Player)] },
    Command { name: "speed", help: "how much time passes per second, like 6h or 2d; 0 pauses", op: true, params: &[req("time", Arg::Span)] },
    Command { name: "accuracy", help: "opening angle of the gravity approximation: smaller is more exact and slower", op: true, params: &[req("angle", Arg::Number)] },
    Command {
        name: "whitelist",
        help: "manage the whitelist",
        op: true,
        params: &[req("on|off|add|remove|list", Arg::Word(&["on", "off", "add", "remove", "list"])), opt("player", Arg::Player)],
    },
];

/// Commands that work in a large-scale single-player world, which has no server behind it.
const SANDBOX: &[&str] = &["help", "respawn", "tp", "orbit", "fuel", "god", "kill", "speed", "accuracy"];
/// Commands that only exist there.
const SANDBOX_ONLY: &[&str] = &["speed", "accuracy"];

impl Command {
    /// Whether the command exists in this kind of world.
    pub fn available(&self, sandbox: bool) -> bool {
        if sandbox {
            SANDBOX.contains(&self.name)
        } else {
            !SANDBOX_ONLY.contains(&self.name)
        }
    }
}

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

/// A length of time in seconds: `90m`, `6h`, `2d`, `1y`, or plain seconds.
pub fn parse_span(word: &str) -> Option<f64> {
    let units = [("min", 60.0), ("s", 1.0), ("m", 60.0), ("h", 3600.0), ("d", 86_400.0), ("y", 31_557_600.0)];
    let (number, scale) = units.iter().find_map(|(u, k)| word.strip_suffix(u).map(|n| (n, *k))).unwrap_or((word, 1.0));
    number.parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0).map(|v| v * scale)
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
        assert_eq!(parse_span("6h"), Some(21_600.0));
        assert_eq!(parse_span("90min"), Some(5400.0));
        assert_eq!(parse_span("2d"), Some(172_800.0));
        assert_eq!(parse_span("0"), Some(0.0));
        assert_eq!((parse_span("-1h"), parse_span("soon")), (None, None));
    }
}

//! Tab completion for the chat box: command names and their arguments, and names of
//! players and bodies anywhere else.

use gsim_proto::command::{self, quote, Arg};

pub struct Context<'a> {
    pub players: &'a [String],
    pub bodies: &'a [String],
    pub presets: &'a [&'a str],
    /// Offer operator commands.
    pub op: bool,
}

/// Byte offset where the word being typed starts (a `"` opens a word that may contain spaces).
fn word_start(input: &str) -> usize {
    let (mut start, mut quoted) = (0, false);
    for (i, c) in input.char_indices() {
        if c == '"' {
            quoted = !quoted;
            if quoted {
                start = i;
            }
        } else if c.is_whitespace() && !quoted {
            start = i + c.len_utf8();
        }
    }
    start
}

/// Everything the word under the cursor (the end of `input`) could be completed to.
pub fn candidates(input: &str, ctx: &Context) -> Vec<String> {
    let start = word_start(input);
    let partial = input[start..].trim_start_matches('"').to_lowercase();
    let names = |lists: &[&[String]]| -> Vec<String> { lists.iter().flat_map(|l| l.iter().cloned()).collect() };
    let pool: Vec<String> = match input.strip_prefix('/') {
        // The command itself.
        Some(_) if start == 0 => {
            let mut names: Vec<String> = command::COMMANDS
                .iter()
                .filter(|c| (ctx.op || !c.op) && c.name.starts_with(partial.trim_start_matches('/')))
                .map(|c| format!("/{}", c.name))
                .collect();
            names.sort();
            return names;
        }
        Some(line) => {
            let before = command::split(&input[1..start]);
            let Some(spec) = before.first().and_then(|n| command::find(n)) else { return Vec::new() };
            let _ = line;
            let with_target = |mut v: Vec<String>| {
                v.push("@target".to_string());
                v
            };
            match spec.args.get(before.len() - 1).or(spec.args.last().filter(|a| **a == Arg::Text)) {
                Some(Arg::Player) => names(&[ctx.players]),
                Some(Arg::Body) => with_target(names(&[ctx.bodies])),
                Some(Arg::Place) => with_target(names(&[ctx.players, ctx.bodies])),
                Some(Arg::Preset) => ctx.presets.iter().map(|p| p.to_string()).collect(),
                Some(Arg::Word(words)) => words.iter().map(|w| w.to_string()).collect(),
                Some(Arg::Text) => names(&[ctx.players, ctx.bodies]),
                Some(Arg::Number) | None => Vec::new(),
            }
        }
        // Plain chat: mention a player or a body.
        None if partial.is_empty() => Vec::new(),
        None => names(&[ctx.players, ctx.bodies]),
    };
    let in_command = input.starts_with('/');
    let mut out: Vec<String> = pool
        .into_iter()
        .filter(|c| c.to_lowercase().starts_with(&partial))
        .map(|c| if in_command { quote(&c) } else { c })
        .collect();
    out.sort_by_key(|c| c.to_lowercase());
    out.dedup();
    out
}

/// `input` with the word under the cursor replaced by `candidate`, ready for the next word.
pub fn apply(input: &str, candidate: &str) -> String {
    format!("{}{candidate} ", &input[..word_start(input)])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(players: &'a [String], bodies: &'a [String], op: bool) -> Context<'a> {
        Context { players, bodies, presets: &["random", "solar", "disc"], op }
    }

    #[test]
    fn completes_commands_arguments_and_mentions() {
        let players = vec!["Ann Droid".to_string(), "bob".to_string()];
        let bodies = vec!["Sun".to_string(), "Saturn".to_string(), "B459".to_string()];
        let op = ctx(&players, &bodies, true);
        let guest = ctx(&players, &bodies, false);

        assert_eq!(candidates("/t", &op), ["/target", "/timescale", "/tp"]);
        assert!(candidates("/t", &guest).is_empty(), "operator commands are hidden from others");
        assert_eq!(candidates("/m", &guest), ["/msg"]);
        assert_eq!(apply("/t", "/tp"), "/tp ");

        // Players and bodies for a place; names with spaces come quoted.
        assert_eq!(candidates("/tp ", &op), ["\"Ann Droid\"", "@target", "B459", "bob", "Saturn", "Sun"]);
        assert_eq!(candidates("/tp s", &op), ["Saturn", "Sun"]);
        assert_eq!(candidates("/tp \"ann", &op), ["\"Ann Droid\""]);
        assert_eq!(apply("/tp \"ann", "\"Ann Droid\""), "/tp \"Ann Droid\" ");
        assert_eq!(candidates("/tp \"Ann Droid\" sa", &op), ["Saturn"]);

        assert_eq!(candidates("/preset s", &op), ["solar"]);
        assert_eq!(candidates("/round ", &op), ["length", "new", "time"]);
        assert_eq!(candidates("/kick b", &op), ["bob"]);
        assert_eq!(candidates("/target @", &op), ["@target"]);
        assert!(candidates("/score bob ", &op).is_empty(), "numbers are not completed");
        // Free text keeps completing names after the fixed arguments.
        assert_eq!(candidates("/msg bob look at sat", &guest), ["Saturn"]);

        // Plain chat mentions, unquoted.
        assert_eq!(candidates("meet at sa", &guest), ["Saturn"]);
        assert_eq!(candidates("hi an", &guest), ["Ann Droid"]);
        assert_eq!(apply("hi an", "Ann Droid"), "hi Ann Droid ");
        assert!(candidates("hello ", &guest).is_empty());
    }
}

//! What the chat input should show while typing, modelled on Minecraft's command
//! suggestions: a list of completions for the word under the cursor, or (when there is
//! nothing to suggest) a grey hint of the arguments still expected, or a red error when
//! the input cannot be right. Also says how to colour the typed command.

use gsim_proto::command::{self, parse_metres, quote, Arg, Command};

pub struct Context<'a> {
    pub players: &'a [String],
    pub bodies: &'a [String],
    /// Each preset with the keys of its parameters.
    pub presets: &'a [(&'a str, Vec<&'a str>)],
    /// Offer operator commands.
    pub op: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SpanKind {
    /// The command name.
    Literal,
    /// The n-th argument (colours cycle).
    Arg(usize),
    Error,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Analysis {
    /// Completions for the word under the cursor, sorted.
    pub suggestions: Vec<String>,
    /// Byte offset in the input where that word starts (a suggestion replaces from here).
    pub start: usize,
    /// Arguments still expected from the cursor on, e.g. `<kills> <orbits>`.
    pub usage: Option<String>,
    /// Why the input cannot be a valid command.
    pub error: Option<String>,
    /// How to colour the input: (start, end, kind) byte ranges.
    pub spans: Vec<(usize, usize, SpanKind)>,
}

/// A word of the input with its position (quotes keep spaces together).
struct Word {
    start: usize,
    end: usize,
    text: String,
}

fn words(input: &str) -> Vec<Word> {
    let mut out = Vec::new();
    let (mut start, mut quoted, mut cur) = (None, false, String::new());
    for (i, c) in input.char_indices() {
        if c == '"' {
            quoted = !quoted;
            start.get_or_insert(i);
        } else if c.is_whitespace() && !quoted {
            if let Some(s) = start.take() {
                out.push(Word { start: s, end: i, text: std::mem::take(&mut cur) });
            }
        } else {
            start.get_or_insert(i);
            cur.push(c);
        }
    }
    if let Some(s) = start {
        out.push(Word { start: s, end: input.len(), text: cur });
    }
    out
}

/// Minecraft's error format: the message, where, and the last few characters before it.
fn error_at(message: &str, input: &str, pos: usize) -> String {
    let upto = &input[..pos.min(input.len())];
    let tail_start = upto.char_indices().rev().nth(9).map_or(0, |(i, _)| i);
    let dots = if tail_start > 0 { "..." } else { "" };
    format!("{message} at position {}: {dots}{}<--[HERE]", upto.chars().count(), &upto[tail_start..])
}

fn is_coordinate(word: &str) -> bool {
    let w = word.strip_prefix('~').unwrap_or(word);
    (word.starts_with('~') && w.is_empty()) || parse_metres(w).is_some()
}

fn pool(arg: Arg, ctx: &Context) -> Vec<String> {
    let names = |lists: &[&[String]]| -> Vec<String> { lists.iter().flat_map(|l| l.iter().cloned()).collect() };
    // Selectors, as in Minecraft: yourself, everyone, someone at random; and the objective.
    let with = |mut v: Vec<String>, extra: &[&str]| {
        v.extend(extra.iter().map(|s| s.to_string()));
        v
    };
    match arg {
        Arg::Player => with(names(&[ctx.players]), &["@a", "@r", "@s"]),
        Arg::Body => with(names(&[ctx.bodies]), &["@t"]),
        Arg::Place => with(names(&[ctx.players]), &["@a", "@r", "@s"]),
        Arg::PlayerOrBody => with(names(&[ctx.players, ctx.bodies]), &["@a", "@r", "@s", "@t"]),
        Arg::Preset => ctx.presets.iter().map(|p| p.0.to_string()).collect(),
        Arg::Word(words) => words.iter().map(|w| w.to_string()).collect(),
        Arg::Text => names(&[ctx.players, ctx.bodies]),
        // Settings depend on the preset typed before them: see `setting_keys`.
        Arg::Number | Arg::Settings => Vec::new(),
    }
}

/// The keys `/preset` accepts after the preset named in `args`: its parameters and the seed.
fn setting_keys<'a>(spec: &Command, args: &[Word], ctx: &Context<'a>) -> Vec<&'a str> {
    let named = spec.params.iter().position(|p| p.arg == Arg::Preset).and_then(|i| args.get(i));
    let mut keys: Vec<&str> = named.and_then(|w| ctx.presets.iter().find(|p| p.0 == w.text)).map(|p| p.1.clone()).unwrap_or_default();
    keys.push("seed");
    keys
}

/// Is `word` a finished `key=number` with one of `keys`?
fn is_setting(word: &str, keys: &[&str]) -> bool {
    word.split_once('=').is_some_and(|(key, value)| keys.contains(&key) && if key == "seed" { value.parse::<u64>().is_ok() } else { parse_metres(value).is_some() })
}

fn matching(pool: Vec<String>, partial: &str, quoted: bool) -> Vec<String> {
    let p = partial.trim_start_matches('"').to_lowercase();
    let mut out: Vec<String> = pool.into_iter().filter(|c| c.to_lowercase().starts_with(&p)).map(|c| if quoted { quote(&c) } else { c }).collect();
    out.sort_by_key(|c| c.to_lowercase());
    out.dedup();
    out
}

/// Analyse a command line (`input` starts with `/`) with the cursor at its end.
fn command_line(input: &str, ctx: &Context) -> Analysis {
    let mut a = Analysis::default();
    let ws = words(input);
    let trailing_space = input.ends_with(char::is_whitespace);
    let Some(first) = ws.first() else { return a };

    // Still typing the command name.
    if ws.len() == 1 && !trailing_space {
        let typed = first.text.trim_start_matches('/').to_lowercase();
        let mut names: Vec<String> =
            command::COMMANDS.iter().filter(|c| (ctx.op || !c.op) && c.name.starts_with(&typed)).map(|c| format!("/{}", c.name)).collect();
        names.sort();
        if names.is_empty() {
            a.error = Some(error_at("Unknown or incomplete command", input, input.len()));
            a.spans.push((0, input.len(), SpanKind::Error));
        } else {
            a.spans.push((0, input.len(), SpanKind::Literal));
        }
        a.suggestions = names;
        return a;
    }

    let name = first.text.trim_start_matches('/');
    let Some(spec) = command::find(name).filter(|c| ctx.op || !c.op) else {
        a.error = Some(error_at("Unknown or incomplete command", input, first.end));
        a.spans.push((0, input.len(), SpanKind::Error));
        return a;
    };
    a.spans.push((first.start, first.end, SpanKind::Literal));

    // Which argument the cursor is on, and what was typed of it so far.
    let args = &ws[1..];
    let index = if trailing_space { args.len() } else { args.len() - 1 };
    let (partial, start) = if trailing_space { ("", input.len()) } else { (args[index].text.as_str(), args[index].start) };
    a.start = start;
    let raw_partial = &input[start..];

    // Free text and settings swallow the rest of the line.
    let text_from = spec.params.iter().position(|p| matches!(p.arg, Arg::Text | Arg::Settings));
    let param_at = |i: usize| match text_from {
        Some(t) if i >= t => spec.params.get(t).map(|p| (t, p)),
        _ => spec.params.get(i).map(|p| (i, p)),
    };

    // Colour the arguments typed so far; flag the first one that cannot be right.
    for (i, w) in args.iter().enumerate() {
        let complete = trailing_space || i < index;
        let bad = match param_at(i) {
            None => true,
            Some((_, p)) if complete => !valid(spec, i, &p.arg, &w.text, args, ctx),
            Some(_) => false,
        };
        if bad {
            a.spans.push((w.start, input.len(), SpanKind::Error));
            a.error = Some(error_at("Incorrect argument for command", input, w.start));
            return a;
        }
        a.spans.push((w.start, w.end, SpanKind::Arg(param_at(i).map_or(i, |p| p.0))));
    }

    let Some((slot, param)) = param_at(index) else {
        if !trailing_space || !args.is_empty() && index > spec.params.len() {
            a.error = Some(error_at("Incorrect argument for command", input, start));
        }
        return a;
    };
    // After a coordinate comes the other coordinate, not a name.
    let after_coordinate = param.arg == Arg::Place && index > 0 && is_coordinate(&args[index - 1].text);
    let arg = if after_coordinate { Arg::Number } else { param.arg };
    a.suggestions = if arg == Arg::Settings {
        // `key=` for what is not set yet; the value after it is the player's to type.
        let given = |key: &str| args.iter().enumerate().any(|(i, w)| i != index && w.text.split_once('=').is_some_and(|(k, _)| k == key));
        let keys = setting_keys(spec, args, ctx).into_iter().filter(|k| !given(k)).map(|k| format!("{k}="));
        if partial.contains('=') { Vec::new() } else { matching(keys.collect(), partial, false) }
    } else {
        matching(pool(arg, ctx), partial, true)
    };
    if raw_partial.starts_with('"') {
        // Keep offering quoted names while the quote is open.
        a.suggestions.retain(|s| s.starts_with('"'));
    }
    if a.suggestions.is_empty() {
        let hopeless = match arg {
            Arg::Word(_) | Arg::Preset => !partial.is_empty(),
            // Wrong unless it is a known key with a value still being typed.
            Arg::Settings => !partial.is_empty() && !partial.split_once('=').is_some_and(|(key, _)| setting_keys(spec, args, ctx).contains(&key)),
            Arg::Number => !partial.is_empty() && !is_coordinate(partial) && partial.parse::<f64>().is_err() && !"-+.~".contains(partial),
            _ => false,
        };
        if hopeless {
            let message = if arg == Arg::Number { "Expected a number" } else { "Incorrect argument for command" };
            a.error = Some(error_at(message, input, start));
            a.spans.retain(|s| s.0 < start);
            a.spans.push((start, input.len(), SpanKind::Error));
        } else {
            a.usage = Some(spec.usage_from(slot));
        }
    }
    a
}

/// Can `word` be the `i`-th argument once it is finished?
fn valid(spec: &Command, i: usize, arg: &Arg, word: &str, args: &[Word], ctx: &Context) -> bool {
    let after_coordinate = *arg == Arg::Place && i > 0 && is_coordinate(&args[i - 1].text);
    match arg {
        _ if after_coordinate => is_coordinate(word),
        Arg::Word(words) => words.contains(&word),
        Arg::Preset => ctx.presets.iter().any(|p| p.0 == word),
        Arg::Settings => is_setting(word, &setting_keys(spec, args, ctx)),
        Arg::Number => word.parse::<f64>().is_ok(),
        // Names are checked by the server: a player may have just joined.
        _ => true,
    }
}

/// Analyse the chat input with the cursor at its end. Plain messages only get name
/// suggestions when asked for (`forced`, i.e. Tab), as in Minecraft.
pub fn analyze(input: &str, ctx: &Context, forced: bool) -> Analysis {
    if input.starts_with('/') {
        return command_line(input, ctx);
    }
    let mut a = Analysis::default();
    let ws = words(input);
    match ws.last() {
        Some(last) if forced && !input.ends_with(char::is_whitespace) && !last.text.is_empty() => {
            a.start = last.start;
            a.suggestions = matching(pool(Arg::Text, ctx), &last.text, false);
        }
        _ => {}
    }
    a
}

/// `input` with the word under the cursor replaced by `suggestion` (no trailing space).
pub fn apply(input: &str, start: usize, suggestion: &str) -> String {
    format!("{}{suggestion}", &input[..start.min(input.len())])
}

/// What `suggestion` would add after what is already typed, for the grey inline preview.
pub fn suffix<'a>(input: &str, start: usize, suggestion: &'a str) -> Option<&'a str> {
    let typed = &input[start.min(input.len())..];
    suggestion.get(..typed.len()).filter(|head| head.eq_ignore_ascii_case(typed)).map(|_| &suggestion[typed.len()..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn presets() -> Vec<(&'static str, Vec<&'static str>)> {
        vec![("random", vec!["count", "spread", "star_mass"]), ("solar", vec!["scale", "star_mass"]), ("disc", vec!["count"])]
    }

    fn ctx<'a>(players: &'a [String], bodies: &'a [String], presets: &'a [(&'a str, Vec<&'a str>)], op: bool) -> Context<'a> {
        Context { players, bodies, presets, op }
    }

    #[test]
    fn preset_settings() {
        let presets = presets();
        let op = ctx(&[], &[], &presets, true);
        let an = |s: &str| analyze(s, &op, false);

        assert_eq!(an("/preset ").suggestions, ["disc", "random", "solar"]);
        assert_eq!(an("/preset s").suggestions, ["solar"]);
        // After the preset: its own keys and the seed, as `key=`.
        let a = an("/preset random ");
        assert_eq!(a.suggestions, ["count=", "seed=", "spread=", "star_mass="]);
        assert_eq!((a.start, a.usage, a.error), (15, None, None));
        assert_eq!(an("/preset solar ").suggestions, ["scale=", "seed=", "star_mass="]);
        assert_eq!(an("/preset random s").suggestions, ["seed=", "spread=", "star_mass="]);
        assert_eq!(apply("/preset random sp", 15, "spread="), "/preset random spread=");
        assert_eq!(suffix("/preset random sp", 15, "spread="), Some("read="));
        // What is already set is not offered again.
        let a = an("/preset random count=50 seed=7 ");
        assert_eq!((a.suggestions, a.start), (vec!["spread=".to_string(), "star_mass=".to_string()], 31));
        assert_eq!(an("/preset random count=50 s").suggestions, ["seed=", "spread=", "star_mass="]);
        assert_eq!(an("/preset disc seed=1 count=2 ").usage.as_deref(), Some("[<key=value ...>]"), "nothing left to set");

        // The value is the player's to type: no suggestions, only the hint.
        for typing in ["/preset random count=", "/preset random count=5", "/preset random spread=80G", "/preset random count=50 seed="] {
            let a = an(typing);
            assert!(a.suggestions.is_empty() && a.error.is_none(), "{typing}: {a:?}");
            assert_eq!(a.usage.as_deref(), Some("[<key=value ...>]"), "{typing}");
        }

        // Unknown keys are wrong as soon as nothing matches; bad values once the word is finished.
        assert_eq!(an("/preset random x").error.as_deref(), Some("Incorrect argument for command at position 15: ...et random <--[HERE]"));
        assert!(an("/preset random bodies=").error.is_some());
        assert!(an("/preset solar count=5").error.is_some(), "count belongs to other presets");
        assert!(an("/preset random count=abc").error.is_none() && an("/preset random count=abc ").error.is_some());
        assert!(an("/preset random count= ").error.is_some() && an("/preset random count ").error.is_some());
        assert!(an("/preset random seed=1.5 ").error.is_some() && an("/preset random seed=15 ").error.is_none());
        assert!(an("/preset random spread=80Gm count=3e2 star_mass=2e30 ").error.is_none());
        assert!(an("/preset nowhere count=5").error.is_some());

        // Every setting is coloured as the one settings argument.
        let spans = an("/preset random count=50 seed=7").spans;
        assert_eq!(spans, [(0, 7, SpanKind::Literal), (8, 14, SpanKind::Arg(0)), (15, 23, SpanKind::Arg(1)), (24, 30, SpanKind::Arg(1))]);
        assert_eq!(an("/preset random count=50 nope=1 seed=7").spans.last(), Some(&(24, 37, SpanKind::Error)));
    }

    #[test]
    fn behaves_like_minecraft() {
        let players = vec!["Ann Droid".to_string(), "bob".to_string()];
        let bodies = vec!["Sun".to_string(), "Saturn".to_string(), "B459".to_string()];
        let presets = presets();
        let op = ctx(&players, &bodies, &presets, true);
        let guest = ctx(&players, &bodies, &presets, false);
        let an = |s: &str| analyze(s, &op, false);

        // Command names: listed as soon as the slash is typed, narrowed while typing.
        assert_eq!(an("/").suggestions.len(), command::COMMANDS.len());
        assert_eq!(an("/t").suggestions, ["/target", "/targets", "/timescale", "/tp"]);
        assert_eq!(analyze("/", &guest, false).suggestions, ["/help", "/list", "/msg", "/r", "/respawn"]);
        assert_eq!(an("/t").start, 0);
        // Accepting inserts the word and nothing else.
        assert_eq!(apply("/t", 0, "/tp"), "/tp");
        assert_eq!(suffix("/t", 0, "/tp"), Some("p"));
        assert_eq!(suffix("/tp sa", 4, "Saturn"), Some("turn"));
        assert_eq!(suffix("/tp x", 4, "Saturn"), None);

        // A finished command name without a space still lists itself; after the space,
        // the argument's options.
        assert_eq!(an("/tp").suggestions, ["/tp"]);
        // Teleporting is to players (or coordinates); orbiting takes bodies as well.
        assert_eq!(an("/tp ").suggestions, ["\"Ann Droid\"", "@a", "@r", "@s", "bob"]);
        let a = an("/orbit ");
        assert_eq!(a.suggestions, ["\"Ann Droid\"", "@a", "@r", "@s", "@t", "B459", "bob", "Saturn", "Sun"]);
        assert_eq!((a.start, a.usage, a.error), (7, None, None));
        assert_eq!(an("/orbit s").suggestions, ["Saturn", "Sun"]);
        assert_eq!(an("/tp \"an").suggestions, ["\"Ann Droid\""]);
        assert_eq!(apply("/tp \"an", 4, "\"Ann Droid\""), "/tp \"Ann Droid\"");
        assert_eq!(an("/orbit \"Ann Droid\" sa").suggestions, ["Saturn"]);

        // Nothing to suggest: the grey hint lists what is still expected from here.
        let a = an("/score bob ");
        assert!(a.suggestions.is_empty());
        assert_eq!(a.usage.as_deref(), Some("<kills> <orbits>"));
        assert_eq!(an("/score bob 3 ").usage.as_deref(), Some("<orbits>"));
        assert_eq!(an("/score bob 3").usage.as_deref(), Some("<kills> <orbits>"));
        assert_eq!(an("/round time ").usage.as_deref(), Some("[<seconds>]"));
        assert_eq!(an("/tp ~1e9 ").usage.as_deref(), Some("[<player|x|y>] [<y>]"));
        assert_eq!(an("/msg bob ").usage.as_deref(), None, "free text offers names");
        assert_eq!(an("/msg bob hi the").usage.as_deref(), Some("<message>"));

        // Errors, in Minecraft's format.
        assert_eq!(an("/xyz").error.as_deref(), Some("Unknown or incomplete command at position 4: /xyz<--[HERE]"));
        assert_eq!(an("/round later").error.as_deref(), Some("Incorrect argument for command at position 7: /round <--[HERE]"));
        assert_eq!(an("/score bob abc").error.as_deref(), Some("Expected a number at position 11: ...score bob <--[HERE]"));
        assert_eq!(an("/list extra").error.as_deref(), Some("Incorrect argument for command at position 6: /list <--[HERE]"));
        assert!(analyze("/tp ", &guest, false).error.is_some(), "operator commands do not exist for others");
        assert!(an("/round new").error.is_none() && an("/timescale 3600").error.is_none());

        // Colouring: command, then one colour per argument; errors run to the end.
        assert_eq!(an("/score bob 3").spans, [(0, 6, SpanKind::Literal), (7, 10, SpanKind::Arg(0)), (11, 12, SpanKind::Arg(1))]);
        assert_eq!(an("/round later now").spans, [(0, 6, SpanKind::Literal), (7, 16, SpanKind::Error)]);

        // Plain chat: names only when Tab asks for them.
        assert!(analyze("meet at sa", &guest, false).suggestions.is_empty());
        let a = analyze("meet at sa", &guest, true);
        assert_eq!((a.suggestions, a.start), (vec!["Saturn".to_string()], 8));
        assert_eq!(apply("hi an", 3, "Ann Droid"), "hi Ann Droid");
        assert!(analyze("hello ", &guest, true).suggestions.is_empty());
    }
}

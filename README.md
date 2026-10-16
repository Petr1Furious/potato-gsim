# Potato Gravity Simulator

A multiplayer ship game in a real Newtonian N-body world: SI units, real `G`, and a clock
running 86 400x faster than real time.

## Quick start

```sh
# Server (default: 1000-body random field, 10-minute rounds, UDP 27777)
cargo run --release -p gsim-server -- --preset random
cargo run --release -p gsim-server -- --preset repulsor --set count=400 --set spread=80Gm
cargo run --release -p gsim-server -- --list-presets     # every preset with its parameters

# Client (macOS / Linux / Windows; no extra system packages needed on macOS)
cargo run --release -p gsim-client                       # menu
cargo run --release -p gsim-client -- --connect my.host  # join directly
cargo run --release -p gsim-client -- --solo random      # single player, in-process server
cargo run --release -p gsim-client -- --solo galaxy --set count=300000   # large-scale sandbox
cargo run --release -p gsim-client -- --bench            # how many bodies this machine holds
cargo run --release -p gsim-client -- --selftest         # check this machine can stay in sync
cargo run --release -p gsim-client -- --gallery          # how bodies of each mass are drawn

# Headless test player: exit code 0 = stayed bit-identical with the server
cargo run --release -p gsim-client-core --bin gsim-bot -- my.host --seconds 30
```

Prebuilt clients for Windows, macOS and Linux are attached to each
[release](https://github.com/Petr1Furious/potato-gsim/releases). The "Latest build" release
is rebuilt on every push to `master`; pushing a tag like `v0.2.0` additionally
publishes a versioned one. Client and server must be the same build.

Clients from the "Latest build" release update themselves: at start-up they compare their
commit with the release, download a newer client in the background, and install it when you
are in the menu (restarting) or when you quit. `--no-update` or `GSIM_NO_UPDATE=1` turns it
off; builds from source or from other branches never update.

On NixOS prefix commands with `./x` (e.g. `./x cargo test --workspace`) or use `nix develop`.

### Docker

```sh
mkdir -p data && docker compose up -d --build   # builds the image, runs it on 27777 (udp and tcp)
docker logs -f potato-gsim          # who joins and leaves, chat, commands, rounds (GSIM_LOG=warn: problems only)
```

Settings are environment variables in `docker-compose.yml` (`GSIM_PRESET`, `GSIM_SET`,
`GSIM_SEED`, `GSIM_ROUND_SECONDS`, `GSIM_TIME_SCALE`, ...) or flags (`gsim-server --help`).

Every preset has named parameters (body count, sizes, masses, ...): `--list-presets` prints
them with their defaults and ranges. Set them with `--set key=value` (repeatable) or
`GSIM_SET="count=400, spread=80Gm"`; lengths take the units `km`, `Mm`, `Gm` and `Tm`. A key
the preset does not have, or a value outside its range, stops the server at start-up.

### In a browser

The server also listens on TCP (the same port number, `--web-port`, 0 to switch it off): it
carries the game over WebSockets and, given the web client's files (`--web-dir`), hands out
the page. The Docker image has them built in; the compose file publishes that port on the
loopback address only, for a reverse proxy to put a name and HTTPS in front of it. The page
joins the server it came from; it has no single player and does not update itself.

```sh
rustup target add wasm32-unknown-unknown
scripts/web.sh                                   # the page, in target/web
cargo run --release -p gsim-server -- --web-dir target/web
```

Behind an HTTPS proxy the page uses `wss://` by itself; the proxy has to pass WebSocket
upgrades through.

## The game

You fly a ship with a 20 km/s delta-v budget (it refills after a few seconds of coasting)
through a system of moving, merging bodies (a heavier `star_mass` makes orbital speeds
several times that budget). Score by:

- **Holding an orbit** around a marked target body for 10 s (+5): eccentricity at most 0.5,
  lowest point at least 1.5 body radii, highest point within 60. The top-left panel shows
  which condition is failing. The target then moves to another body.
- **Destroying other ships** with shells (+2). Every death costs a point, and a new ship takes 5 s. A ship carries 10 and fires at most one a
  second; after 4 s without firing, one comes back every 2 s.

A target nobody captures moves to another body after 90 s (`GSIM_TARGET_SECONDS`, 0 = never).
The time left is shown next to the target and in the panel, and stands still while a ship is
on a qualifying orbit around it. There is one target for every two players
(`GSIM_PLAYERS_PER_TARGET`) or a fixed number of them (`GSIM_TARGETS`): set one or the other.

### Bases

With `GSIM_MODE=bases` (or `/mode bases`) there are no targets. Every player owns a base on a
ring near the edge of the world: an Earth-sized body that never moves, keeps other bodies
away and pulls ships and shells in like an Earth within 640 Mm of it; from there to 900 Mm
that turns into the push bodies feel, so that far away ships and bodies fall alike and orbits
around bodies elsewhere are left alone. Ships respawn on an orbit 480 Mm out from their own
base while it stands, where a straight burn in any direction but at the base gets them away.
Within 640 Mm of one's own base (the zone drawn around it) shells cost nothing.
Bases lose health to shell hits (2 %) and to ships crashing into them (10 %, and the ship),
one's own included, and enemy bases to a ship holding an orbit around them under the same
conditions as for a target (1 % a second). A destroyed base stays as a dark hulk; its owner
is out once their ship is lost. The last player in play wins the round. Players who join a
round under way watch it (a second player joining a lone one starts a new round); whoever
leaves and comes back while their base stands gets it back. `R` selects the nearest enemy
base, the top-left panel shows your base and the siege orbit, and the board shows each
base's health and how many bases each player has destroyed (KO).

Clouds of **scrap** float around the center. A ship takes a piece by passing within 80 Mm of
it, carries it, and banks it on coming home (into its zone). Every piece carried makes the ship
5 % heavier (a crystal 15 %), so the same burn gives less delta-v: a full hold is slow and easy
to catch. Pieces that drift far beyond the bases with no ship within 3 Gm fade away over a few
seconds. A ship that is lost spills its cargo where it was; one that crashes leaves it going slowly
round what it hit.

**Mines** are planets near the center that give a piece to a ship holding a qualifying orbit
around them while it stays: crystal mines (one, and one more for every two players) a crystal
every 16 s until their three are gone, when the mine opens on another body 20 s later; scrap
mines (one, and one more for every three players) a scrap every 3 s, without end. A mine (or a
target) that drifts far out, is swung around by a much heavier body, or ends up in a tight pair
with a body of much its mass moves to another body. A crystal
or two also turn up at the center a minute into the round and every 90 s after (announced
10 s ahead). Operators can name the first mine in commands (`/orbit mine`).

Within its home zone a ship feels only its own base: other bodies' pull fades out over the same
stretch as the base's own pull fades in, so orbits around home keep their shape.

At home (or waiting there for a ship), `B` opens the shop, even once the base has fallen. Ship
upgrades, three tiers each, kept when the ship is lost and shared by the whole team: thrust (up to x3), delta-v (up to
60 km/s), shells (up to 50), fire rate (up to x3) and shell speed (up to 24 km/s), for 4, 8 and
16 scrap (the last also takes 4 crystals). Base upgrades: armor (up to x3),
home zone (out to 1600 Mm) and respawn (down to 1 s), for 6, 12 and 24 scrap (and 4 crystals).
Supplies: a target lock (other ships can be clicked to select them, for the predicted path and
the shell preview with its closest approach, until the ship is lost), a shield (drawn around
the ship; it survives one shell), rapid fire (twice the
shells, fired and coming back twice as fast, until the ship is lost), base repair (+20 %), and
five to carry along, used with the keys 1 to 5 while the shop is closed: warp home (1 or `Q`: a
jump back home, leaving the cargo behind), a gravity well (2 or `E`: fired towards the cursor
like a shell, after a second it pulls every ship near it in for 10 s), an EMP (3: the engines
of enemy ships within 1.2 Gm go dead for 3 s), a scrap magnet (4: pieces are taken from three
times as far for 30 s) and an afterburner (5: 10 km/s of delta-v beyond a full tank). Every two
minutes a **supply crate** turns up somewhere: the first ship through it gets a carried supply.

Every round of bases rolls how much scrap floats about, whether there is a mine more, and a
twist: deep tanks (twice the delta-v), quick guns (shells fire and come back twice as fast),
long shots (faster shells that fly twice as long), scrap rush, crystal fever (crystals at the
center every 30 s) or heavy rams (2.5 times the damage to a base), or none; the chat says
which, and the twist shows by the round number. With four or more playing, a round may also be played in
**teams** of two (or of three, with six or more): split at random, each team shares one base,
its bank and every upgrade bought, and the last
team in play wins. Ships wear a ring in their team's color. A round of bases has no time limit: 12 minutes
in comes **sudden death**, when every base loses 1 % a second and cannot be repaired.

A round of targets lasts 10 minutes; then the scores are shown and a freshly generated world starts.
An empty server pauses: the next round begins when the first player joins.
Ships are drawn in their players' colors: random at first, chosen in the settings.
Bodies flung out of the system for good fade out and are removed, and are never targets.

## Players, bans and the whitelist

There are no accounts. The game makes a key pair on first start (`identity.key` next to the
settings file) and proves it to the server when joining. A server reserves each name for the
first key that uses it, forever; copy `identity.key` to play under your names on another
machine, and keep it private.

The server keeps its state as plain text files in `GSIM_STATE_DIR` (`./data` with Docker
Compose): `players.txt`, `whitelist.txt`, `banned-players.txt`, `banned-ips.txt`. Edit them by
hand or with the admin commands; a running server applies changes within two seconds and
removes players who are no longer allowed.

```sh
docker exec potato-gsim gsim-server admin players
docker exec potato-gsim gsim-server admin ban NAME being rude
docker exec potato-gsim gsim-server admin ban-ip NAME      # or an address
docker exec potato-gsim gsim-server admin unban NAME
docker exec potato-gsim gsim-server admin allow NAME       # whitelist (GSIM_WHITELIST=true)
docker exec potato-gsim gsim-server admin forget NAME      # release a reserved name
docker exec potato-gsim gsim-server admin op NAME          # may use operator commands in chat
```

A ban by name stops that identity; someone determined can make a new key and name, which
is what address bans and the whitelist are for.

## Chat and commands

`T` or `Enter` opens chat and `/` opens it for a command. The input works like Minecraft's:
suggestions for the word being typed appear in a list above it (arrows to move, `Tab` to
accept and to cycle, `Shift+Tab` back, click or wheel with the mouse, `Esc` to hide; after a
bare `/` the list waits for `Tab`); when
there is nothing to suggest, a gray hint shows the arguments still expected, or a red
message shows where the command went wrong. In plain messages `Tab` completes names of
players and bodies. `Up`/`Down` recall earlier lines, the wheel and `PgUp`/`PgDn` scroll. Joins,
kills, captures and new targets appear in chat too. Names of players and bodies in a message
are clickable: a body gets selected, a player gets followed by the camera. `G` drops a
marker under the cursor that everyone sees for a few seconds.

Everyone: `/help`, `/list` (with teams, bases and who is out), `/msg PLAYER TEXT` (or `/tell`, `/w`), `/r TEXT`, `/me ACTION`, `/respawn`. Wherever a player is
expected, `@s` is you, `@a` everyone and `@r` someone at random; `@t` is the target body.

Operators (listed in `ops.txt`; the host of a solo game always is one):

| Command | Effect |
|---|---|
| `/round new`, `/round end`, `/round time S`, `/round length S` | Start a new round; end this one (with bases, the best-standing base wins); set the time left (with bases: until sudden death); set the round length |
| `/tp [PLAYER] PLAYER\|~DX ~DY\|X Y` | Teleport onto a player or to coordinates in meters (`~` is relative, units like `5Gm` work) |
| `/orbit [PLAYER] BODY` | Put a ship on a circular orbit around a body, 20 radii up |
| `/preset NAME [KEY=VALUE ...]`, `/timescale X` | New round in another world (e.g. `/preset random count=300 seed=7`; what is not set returns to its default) or at another time scale |
| `/mode [targets\|bases] [KEY=VALUE ...]` | New round with targets or with bases, with settings for bases that stay until changed: `twist=random\|plain\|quick-guns\|...`, `teams=roll\|off\|2\|3`, `sudden=SECONDS` until sudden death (0: none), `decay=PERCENT` a second; alone, the settings in force |
| `/gamemode spectator\|play [PLAYER]` | Watch without a ship (`Space` follows the next player), or play again; with bases, at one's own base even if it has fallen |
| `/target BODY` | Make a body a target |
| `/targets count N`, `/targets per N` | A fixed number of targets, or one for every N players |
| `/team PLAYER N\|none` | Put a player on team N (1 red, 2 blue, ...) in the next rounds with teams |
| `/say TEXT` | Say something to everyone as [you] |
| `/give PLAYER ITEM [COUNT]` | Give scrap or crystals (into the bank) or supplies (`shield`, `rapid-fire`, `base-repair`, `warp-home`, `gravity-well`) |
| `/fuel`, `/god`, `/kill`, `/respawn PLAYER`, `/score PLAYER KILLS ORBITS` | Refill, immunity to shells, destroy, set scores |
| `/kick`, `/ban`, `/unban`, `/ban-ip`, `/unban-ip`, `/op`, `/deop`, `/whitelist ...` | Moderation, same lists as the admin tool |

## Controls

| Input | Action |
|---|---|
| W / Up | Thrust along heading |
| Mouse (or A / D after pressing M) | Heading |
| Shift / Ctrl, X / Z | Throttle up / down, 0 % / 100 % |
| Space | Fire a shell towards the cursor (further cursor = faster shell) |
| Wheel, drag | Zoom about the cursor, pan |
| Click | Select a body, a piece of scrap, a crystal or a crate (or, with a target lock, another ship): prediction is then drawn relative to it |
| F | Camera: with the selected body, or your ship while nothing is selected (the default), or automatic: with the selection, moving and zooming by itself to keep the ship, its predicted path up to the closest approach and the selected body on screen (it holds still under thrust, and leaves a view set by hand alone for a few seconds) |
| R | Select the nearest target (with bases: enemy base), then the next one |
| P, O, N | Ship trajectory prediction, shell trajectory preview, body names |
| L, K | Trails, trails relative to selection / world |
| T, /, G | Chat, command, point at the map |
| B, 1 - 8, 1 - 5, Q, E | Shop (bases, at home), buy upgrades; use supplies (Q warp home, E gravity well) |
| Backspace (held 1.5 s) | Destroy your own ship and respawn |
| F3, F11, Esc | Network details, fullscreen, menu |

Color shows mass: dim slate for the lightest bodies, through ice blue and pale sand to amber
and ember for the heaviest; stars glow white-gold, negative masses are violet to pink.

The green line is where the ship goes if the engine stays off; yellow is with the current
burn held. Both are exact, not estimates. Shells are armed 0.5 s after launch (small circle on
the preview) and destroy any ship within their blast radius, including the one that fired.

## How it stays in sync

Planets are massive, ships and shells are massless test particles. That one decision makes
the planets' motion independent of what players do, so:

- **Massive bodies** are a pure function of (initial state, tick). The server sends them once
  at join. Every client then computes them itself, several seconds ahead of the present
  (the *ephemeris*), bit-identically. The server only broadcasts a hash once a second; a
  mismatch triggers a fresh snapshot.
- **Inputs are tick-stamped.** The client schedules each control change a few ticks in the
  future (100 ms by default, more on slow links) and applies it locally at exactly that tick.
  The server applies it at the same tick, so the client's own ship is never mispredicted. If
  a command does arrive late, the server applies it immediately and says which tick it used.
- **Remote ships** are integrated from their broadcast inputs. An input that arrives after it
  took effect re-simulates that one ship from that tick; nothing else is touched.
- **Safety net:** the server periodically sends each ship's authoritative state; any
  difference is corrected and replayed. In normal play there are none (see F3).

Bit-identical means the core uses only `+ - * /` and `sqrt` on `f64`, pure-Rust `libm` for
everything else, fixed summation order, and integer fuel. `clippy.toml` in `gsim-core` bans
the rest. Every binary carries a golden hash of a reference scene and refuses to play if its
own result differs.

## Single player

**Single player** in the menu lists every world with sliders for its parameters (the same
ones `--set` and `/preset` take) and a seed. There are two kinds:

- **Exact** worlds are the multiplayer presets, run by a server inside the client: rounds,
  the orbit objective, shells, every command.
- **Sandbox** worlds (`empty`, `galaxy`, `collision`, `cloud`) are run by a separate engine,
  `gsim-swarm`, that trades exactness for size: hundreds of thousands of bodies. There is no
  ship: you watch through a free camera (drag or WASD, wheel or Q/E, click a body to follow
  it) and change the world with the tools on keys 1 to 6: place and throw a body, spray
  many, drop a whole galaxy, cloud or ring, erase, shatter. The selected body's mass, size
  and velocity can be edited. Space pauses; holding `<` or `>` changes how much time one
  second is worth. `X` makes light linger so orbits draw themselves, `C` switches what
  color shows. Worlds can be saved and loaded from the Esc menu, where saves can
  also be deleted.

How the large-scale engine works: there is no fixed step rate. It steps as fast as it can,
and each step covers "pace x how long steps have been taking", so the world keeps its pace
whether a step takes one millisecond or fifty.

Two bodies that hold each other too tightly for such a step (a moon and its planet, a star
and the core it circles closely, two bodies passing each other) are tied: what they do to
each other is taken out of the world's step and followed in steps of the pair's own, in
double precision. Those are the world's step halved as often as it takes for the two to turn
about each other by no more than a tenth of a radian in each, up to 65,536 of them, chosen by
where the pair is heading. Both are kicked at the same moments, so a pair keeps its momentum
exactly, and everything else acts on the two once per step of the world, as it does on
anybody. Only the pair itself is worked out at each of its steps, so a tight pair costs
next to nothing, and thousands of bodies close to one heavy core each cost only their own
steps. Time slows down, and the header says so, when the ties' steps come to take as long
as everything else in a step does, or when something needs more than 65,536 of them. Bodies that get far closer than the step they are on allowed for are
not thrown apart: no pull between them is applied for longer than it takes to turn them by
0.6 of a radian, and those that overlap merge first. Where ties are few, somewhat longer
steps of the world pay for them and the pace is held; where they are very many it falls
behind. Following them can be switched off (per world, and in the Esc menu): then every
body takes the world's step, and whatever turns faster comes out wrong. A world of 192
bodies or fewer is stepped whole, and follows tight orbits by shortening every step instead.

What this does not get right: a pull from outside that differs across a tight orbit (the
star's on a moon) is felt once per step of the world, not as the orbit goes round. Over
thousands of revolutions that moves a moon in the outer part of its planet's reach by a few
percent of its distance. And two bodies passing each other so fast that they cross many
times their closest distance in one step of the world are turned too little.

Nothing in the engine is a fixed length, mass or time: a pair of pebbles a millimeter apart
is followed like a pair of stars, also when the world around them is ten thousand million
times wider. Forces are single precision, worked out from each group's own center in units
of the group's own size; two bodies too close together for that to tell how far apart they
are (a thousandth of the group's size) are tied like those that turn too fast, and get
double precision.

Above 4000 bodies they are kept sorted along a Z-order curve (laid over where most of
them are, so that a few far away do not blur the rest) and grouped into a tree, and each group of up to 64 neighbors gathers one list of what
acts on it (nearby bodies one by one, distant cells as a point mass plus quadrupole) and
evaluates it with AVX-512, AVX2 or NEON. Up to 4000, every pair is summed through the same
kernels; up to 192, in double precision with a fourth-order integrator. Positions are double
precision throughout. Overlapping bodies merge. Nothing in it is reproducible between
machines, which is why it is single player only.

## Layout

| Crate | Purpose |
|---|---|
| `gsim-core` | Deterministic simulation: massive tier, ephemeris, particles, ships, prediction |
| `gsim-proto` | Wire messages (postcard) and channel layout |
| `gsim-server` | Authoritative world, presets, UDP and WebSocket drivers, `gsim-server` binary |
| `gsim-client-core` | Headless client: clock sync, replicas with per-ship rollback, `gsim-bot` |
| `gsim-swarm` | Fast approximate engine for very large single-player worlds |
| `gsim-client` | macroquad + egui game client (also built for browsers; `web/` is its page) |
| `gsim-testkit` | Virtual-time network lab (latency, jitter, loss) and netcode tests |

## Tests

```sh
cargo test --workspace
```

- `gsim-core`: merges and negative-mass annihilation, thread-count-independent hashes,
  snapshot continuity, low-orbit stability, anti-tunneling, prediction equals reality
  bit for bit, golden self-test.
- `gsim-server`: every preset runs, at its defaults and at the extremes of its parameters;
  the repulsor's negative bodies stay finite.
- `gsim-swarm`: forces against exact sums on every instruction set the machine has, the
  pairwise and precise modes, a moon and a planet around a pair of stars on steps of their
  own, close passes that fling nothing away, merges conserving mass and
  momentum, every scenario at the limits of its parameters, editing and saving worlds,
  holding the pace and slowing down for a tight orbit.
- `gsim-testkit`: server plus clients over simulated links (clean, lossy and jittery, 500 ms
  round trip, late join, forced divergence and resync, shell kills, arming, the shell stock,
  crashes, orbit capture, targets timing out, their number, round rollover into a new world).

## Credits

The app icon combines a hand-drawn potato with the "rocket" glyph from
[Lucide](https://lucide.dev) (ISC license). Fonts: Rajdhani and Share Tech Mono (SIL Open Font
Licence; texts in `crates/gsim-client/assets`).

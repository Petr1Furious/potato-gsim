# Potato Gravity Simulator

A multiplayer ship game in a real Newtonian N-body world: SI units, real `G`, and a clock
running 86 400x faster than real time.

## Quick start

```sh
# Server (default: 1000-body random field, 10-minute rounds, UDP 27777)
cargo run --release -p gsim-server -- --preset random
cargo run --release -p gsim-server -- --preset disc --set count=400 --set spread=80Gm
cargo run --release -p gsim-server -- --list-presets     # every preset with its parameters

# Client (macOS / Linux / Windows; no extra system packages needed on macOS)
cargo run --release -p gsim-client                       # menu
cargo run --release -p gsim-client -- --connect my.host  # join directly
cargo run --release -p gsim-client -- --solo solar       # single player, in-process server
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
mkdir -p data && docker compose up -d --build   # builds the image, runs it on 27777/udp
docker logs -f potato-gsim          # one status line every 10 s
```

Settings are environment variables in `docker-compose.yml` (`GSIM_PRESET`, `GSIM_SET`,
`GSIM_SEED`, `GSIM_ROUND_SECONDS`, `GSIM_TIME_SCALE`, ...) or flags (`gsim-server --help`).

Every preset has named parameters (body count, sizes, masses, ...): `--list-presets` prints
them with their defaults and ranges. Set them with `--set key=value` (repeatable) or
`GSIM_SET="count=400, spread=80Gm"`; lengths take the units `km`, `Mm`, `Gm` and `Tm`. A key
the preset does not have, or a value outside its range, stops the server at start-up.

## The game

You fly a ship with a 20 km/s delta-v budget (it refills after a few seconds of coasting)
through a system of moving, merging bodies (try the `disc` or `solar` presets for orbital
speeds several times that budget). Score by:

- **Holding an orbit** around the marked target body for 10 s (+3): eccentricity at most 0.5,
  lowest point at least 1.5 body radii, highest point within 60. The bottom-left panel shows
  which condition is failing. The target then moves to another body.
- **Destroying other ships** with shells (+1).

A round lasts 10 minutes; then the scores are shown and a freshly generated world starts.
An empty server pauses: the next round begins when the first player joins.
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
there is nothing to suggest, a grey hint shows the arguments still expected, or a red
message shows where the command went wrong. In plain messages `Tab` completes names of
players and bodies. `Up`/`Down` recall earlier lines, the wheel and `PgUp`/`PgDn` scroll. Joins,
kills, captures and new targets appear in chat too. Names of players and bodies in a message
are clickable: a body gets selected, a player gets followed by the camera. `G` drops a
marker under the cursor that everyone sees for a few seconds.

Everyone: `/help`, `/list`, `/msg PLAYER TEXT`, `/r TEXT`, `/respawn`. Wherever a player is
expected, `@s` is you, `@a` everyone and `@r` someone at random; `@t` is the target body.

Operators (listed in `ops.txt`; the host of a solo game always is one):

| Command | Effect |
|---|---|
| `/round new`, `/round time S`, `/round length S` | Restart the round; set the time left; set the round length |
| `/tp [PLAYER] PLAYER\|~DX ~DY\|X Y` | Teleport onto a player or to coordinates in metres (`~` is relative, units like `5Gm` work) |
| `/orbit [PLAYER] BODY` | Put a ship on a circular orbit around a body, 20 radii up |
| `/preset NAME [KEY=VALUE ...]`, `/timescale X` | New round in another world (e.g. `/preset random count=300 seed=7`; what is not set returns to its default) or at another time scale |
| `/target BODY` | Move the objective |
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
| Click | Select a body: prediction and trails are then drawn relative to it |
| F | Camera: stays with the ship, with the selected body (the default), or automatic: with the selection, moving and zooming by itself to keep the ship, its predicted path up to the closest approach and the selected body on screen (it holds still under thrust, and leaves a view set by hand alone for a few seconds) |
| R | Select the body the round is about |
| P, O, N | Ship trajectory prediction, shell trajectory preview, body names |
| L, K | Trails, trails relative to selection / world |
| T, /, G | Chat, command, point at the map |
| F3, F11, Esc | Network details, fullscreen, menu |

Colour shows mass: dim slate for the lightest bodies, through ice blue and pale sand to amber
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
- **Large scale** worlds (`empty`, `galaxy`, `collision`, `cloud`) are run by a separate engine,
  `gsim-swarm`, that trades exactness for size: hundreds of thousands of bodies. There is no
  ship: you watch through a free camera (drag or WASD, wheel or Q/E, click a body to follow
  it) and change the world with the tools on keys 1 to 6: place and throw a body, spray
  many, drop a whole galaxy, cloud or ring, erase, shatter. The selected body's mass, size
  and velocity can be edited. Space pauses; holding `<` or `>` changes how much time one
  second is worth. `X` makes light linger so orbits draw themselves, `C` switches what
  colour shows. Worlds can be saved and loaded from the Esc menu, which also goes back to
  the last automatic checkpoint.

How the large-scale engine works: there is no fixed step rate. It steps as fast as it can,
and each step covers "pace x how long steps have been taking", so the world keeps its pace
whether a step takes one millisecond or fifty. A step is never longer than a fifth of the
time the tightest bound orbit takes to turn a radian; when that limit bites, time runs slower
than asked and the header says so. Above 4000 bodies they are kept sorted along a Z-order
curve and grouped into a tree, and each group of up to 64 neighbours gathers one list of what
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
| `gsim-server` | Authoritative world, presets, UDP driver, `gsim-server` binary |
| `gsim-client-core` | Headless client: clock sync, replicas with per-ship rollback, `gsim-bot` |
| `gsim-swarm` | Fast approximate engine for very large single-player worlds |
| `gsim-client` | macroquad + egui game client |
| `gsim-testkit` | Virtual-time network lab (latency, jitter, loss) and netcode tests |

## Tests

```sh
cargo test --workspace
```

- `gsim-core`: merges and negative-mass annihilation, thread-count-independent hashes,
  snapshot continuity, low-orbit stability, anti-tunnelling, prediction equals reality
  bit for bit, golden self-test.
- `gsim-server`: every preset runs; the solar system keeps all orbits (and the Moon) for two
  simulated years; the figure-eight returns to its start; the runaway pair accelerates.
- `gsim-swarm`: forces against exact sums on every instruction set the machine has, the
  pairwise and precise modes, finding tight orbits in a crowd, merges conserving mass and
  momentum, every scenario at the limits of its parameters, editing and saving worlds,
  holding the pace and slowing down for a tight orbit.
- `gsim-testkit`: server plus clients over simulated links (clean, lossy and jittery, 500 ms
  round trip, late join, forced divergence and resync, shell kills, arming, crashes, orbit
  capture, round rollover into a new world).

## Credits

The app icon combines a hand-drawn potato with the "rocket" glyph from
[Lucide](https://lucide.dev) (ISC licence). Fonts: Rajdhani and Share Tech Mono (SIL Open Font
Licence; texts in `crates/gsim-client/assets`).

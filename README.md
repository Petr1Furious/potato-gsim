# potato-gsim

A multiplayer ship game in a real Newtonian N-body world: SI units, real `G`, and a clock
running 86 400x faster than real time.

## Quick start

```sh
# Server (default: 1000-body random field, 10-minute rounds, UDP 27777)
cargo run --release -p gsim-server -- --preset random
cargo run --release -p gsim-server -- --list-presets

# Client (macOS / Linux / Windows; no extra system packages needed on macOS)
cargo run --release -p gsim-client                       # menu
cargo run --release -p gsim-client -- --connect my.host  # join directly
cargo run --release -p gsim-client -- --solo solar       # offline, in-process server
cargo run --release -p gsim-client -- --selftest         # check this machine can stay in sync

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
docker compose up -d --build        # builds the image and runs it on 27777/udp
docker logs -f potato-gsim          # one status line every 10 s
```

Settings are environment variables in `docker-compose.yml` (`GSIM_PRESET`, `GSIM_SEED`,
`GSIM_RANDOM_COUNT`, `GSIM_ROUND_SECONDS`, `GSIM_TIME_SCALE`, ...) or flags (`gsim-server --help`).

## The game

You fly a ship with a 10 km/s delta-v budget (it refills after a few seconds of coasting)
through a system of moving, merging bodies (try the `disc` or `solar` presets for orbital
speeds several times that budget). Score by:

- **Holding an orbit** around the marked target body for 10 s (+3): eccentricity at most 0.5,
  lowest point at least 1.5 body radii, highest point within 60. The bottom-left panel shows
  which condition is failing. The target then moves to another body.
- **Destroying other ships** with shells (+1).

A round lasts 10 minutes; then the scores are shown and a freshly generated world starts.
Bodies flung out of the system for good fade out and are removed, and are never targets.

## Controls

| Input | Action |
|---|---|
| W / Up | Thrust along heading |
| Mouse (or A / D after pressing M) | Heading |
| Shift / Ctrl, X / Z | Throttle up / down, 0 % / 100 % |
| Space | Fire a shell towards the cursor (further cursor = faster shell) |
| Wheel, drag | Zoom about the cursor, pan |
| Click | Select a body: prediction and trails are then drawn relative to it |
| F | Camera follows ship / selection |
| P, O | Ship trajectory prediction, shell trajectory preview |
| L, T | Trails, trails relative to selection / world |
| R, F3, F11, Esc | Recentre, network details, fullscreen, menu |

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

## Layout

| Crate | Purpose |
|---|---|
| `gsim-core` | Deterministic simulation: massive tier, ephemeris, particles, ships, prediction |
| `gsim-proto` | Wire messages (postcard) and channel layout |
| `gsim-server` | Authoritative world, presets, UDP driver, `gsim-server` binary |
| `gsim-client-core` | Headless client: clock sync, replicas with per-ship rollback, `gsim-bot` |
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
- `gsim-testkit`: server plus clients over simulated links (clean, lossy and jittery, 500 ms
  round trip, late join, forced divergence and resync, shell kills, arming, crashes, orbit
  capture, round rollover into a new world).

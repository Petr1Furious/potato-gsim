//! The integrator: a Barnes-Hut style tree over bodies kept sorted along a Z-order curve.
//!
//! A step of the world is a leapfrog step (kick, drift, kick), with one thing set apart: two
//! bodies that would turn about each other by more than a little in such a step are *tied*.
//! What they do to each other is left out of the world's kick and followed in steps of the
//! tie's own, in double precision: the world's step halved as often as it takes (up to sixteen
//! times) for the two to turn by a tenth of a radian in each. Both are kicked at the same
//! moments, so what they do to each other cancels exactly; everything else acts on them once
//! per step of the world, as on anybody. A tie that would ask for more halvings than there
//! are makes the world's step too long: [`Engine::longest_step`] says how long it may be.
//!
//! The step is split into phases, so that a renderer can read the bodies in between:
//! 1. [`Engine::begin`] (writes bodies): half the world's kick, and for each tie its own step
//!    and half its kick;
//! 2. [`Engine::own`] (writes, a little at a time): the ties' steps, moment by moment;
//! 3. [`Engine::end`] (writes): everything is moved to the end of the step, the ties give the
//!    other half of their kicks, the bodies are re-sorted and the tree is rebuilt;
//! 4. [`Engine::forces`] (reads): refit the tree, gather each group's interaction lists,
//!    evaluate them with the vector kernels, note what is tied and what overlaps;
//! 5. [`Engine::finish`] (writes): second half of the world's kick, merge the overlapping.
//!
//! Positions and velocities are double precision. Forces are computed in single precision on
//! coordinates relative to each group's centre, in units of that group's own size, and two
//! bodies too close together for that are tied as well (see [`kernel::CLOSE`]): bodies are
//! told apart however small what they form is next to the world, and nothing in the engine
//! is a length, a mass or a time of some fixed size.

use crate::kernel::{self, Far, Level, Near, Targets, PAD, PAD_X};
use rayon::prelude::*;

const FILE_MAGIC: &[u8; 4] = b"GSW1";

/// All bodies, as parallel arrays in tree order (which changes every step; `id` does not).
/// A body with zero mass was absorbed or removed and disappears at the next sort.
#[derive(Clone, Default)]
pub struct Bodies {
    pub x: Vec<f64>,
    pub y: Vec<f64>,
    pub vx: Vec<f64>,
    pub vy: Vec<f64>,
    /// Acceleration at the current position (m/s^2).
    pub ax: Vec<f32>,
    pub ay: Vec<f32>,
    /// kg
    pub m: Vec<f32>,
    /// m
    pub r: Vec<f32>,
    pub id: Vec<u32>,
    /// Which cloud or galaxy the body started in (kept by the heavier partner in a merge).
    pub group: Vec<u8>,
    /// 0 for a body that is staying. One found to be leaving the world for good fades from
    /// just above 0 to 1 and is then dropped.
    pub fade: Vec<f32>,
    /// Part of the way through a step, the bodies that have been moved on since it began: how
    /// many seconds into it each of them is. Empty between steps.
    pub late: Vec<f32>,
    /// Simulated time these positions belong to (maintained by whoever steps the engine).
    pub time: f64,
    /// Seconds the step that led here covered: bodies moved `v * dt` to arrive.
    pub dt: f64,
    /// The id the next new body gets: ids are never reused.
    pub next_id: u32,
}

impl Bodies {
    pub fn len(&self) -> usize {
        self.x.len()
    }

    pub fn is_empty(&self) -> bool {
        self.x.is_empty()
    }

    pub fn push(&mut self, x: f64, y: f64, vx: f64, vy: f64, mass: f64, radius: f64, group: u8) {
        self.id.push(self.next_id);
        self.next_id += 1;
        self.x.push(x);
        self.y.push(y);
        self.vx.push(vx);
        self.vy.push(vy);
        self.ax.push(0.0);
        self.ay.push(0.0);
        // Zero marks a body that is gone: one with no mass has the least there can be.
        self.m.push(mass.max(f32::MIN_POSITIVE as f64) as f32);
        self.r.push(radius as f32);
        self.group.push(group);
        self.fade.push(0.0);
    }

    /// Where a body is `tau` seconds after `time`, as far as its velocity tells.
    pub fn place(&self, i: usize, tau: f64) -> (f64, f64) {
        let tau = tau - self.late.get(i).copied().unwrap_or(0.0) as f64;
        (self.x[i] + self.vx[i] * tau, self.y[i] + self.vy[i] * tau)
    }

    pub fn alive(&self, i: usize) -> bool {
        self.m[i] > 0.0
    }

    /// Add all of `other`, giving its bodies new ids.
    pub fn append(&mut self, other: &Bodies) {
        for i in 0..other.len() {
            self.push(other.x[i], other.y[i], other.vx[i], other.vy[i], other.m[i] as f64, other.r[i] as f64, other.group[i]);
        }
    }

    /// Drop the bodies for which `keep` says no (and any already dead).
    pub fn retain(&mut self, mut keep: impl FnMut(&Bodies, usize) -> bool) {
        let stay: Vec<bool> = (0..self.len()).map(|i| self.alive(i) && keep(self, i)).collect();
        fn sift<T>(v: &mut Vec<T>, stay: &[bool]) {
            let mut i = 0;
            v.retain(|_| {
                i += 1;
                stay[i - 1]
            });
        }
        sift(&mut self.x, &stay);
        sift(&mut self.y, &stay);
        sift(&mut self.vx, &stay);
        sift(&mut self.vy, &stay);
        sift(&mut self.ax, &stay);
        sift(&mut self.ay, &stay);
        sift(&mut self.m, &stay);
        sift(&mut self.r, &stay);
        sift(&mut self.id, &stay);
        sift(&mut self.group, &stay);
        sift(&mut self.fade, &stay);
    }

    /// Write the world in a simple binary form (little-endian arrays after a short header).
    pub fn write(&self, out: &mut impl std::io::Write) -> std::io::Result<()> {
        out.write_all(FILE_MAGIC)?;
        out.write_all(&(self.len() as u64).to_le_bytes())?;
        out.write_all(&self.time.to_le_bytes())?;
        out.write_all(&self.next_id.to_le_bytes())?;
        for v in [&self.x, &self.y, &self.vx, &self.vy] {
            for value in v {
                out.write_all(&value.to_le_bytes())?;
            }
        }
        for v in [&self.m, &self.r] {
            for value in v {
                out.write_all(&value.to_le_bytes())?;
            }
        }
        for id in &self.id {
            out.write_all(&id.to_le_bytes())?;
        }
        out.write_all(&self.group)
    }

    /// Read what [`Bodies::write`] wrote.
    pub fn read(input: &mut impl std::io::Read) -> std::io::Result<Bodies> {
        fn take<const N: usize>(input: &mut impl std::io::Read) -> std::io::Result<[u8; N]> {
            let mut bytes = [0u8; N];
            input.read_exact(&mut bytes)?;
            Ok(bytes)
        }
        let bad = |what: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, what.to_string());
        if &take::<4>(input)? != FILE_MAGIC {
            return Err(bad("not a saved world"));
        }
        let n = u64::from_le_bytes(take(input)?) as usize;
        if n > 50_000_000 {
            return Err(bad("implausible body count"));
        }
        let mut b = Bodies { time: f64::from_le_bytes(take(input)?), next_id: u32::from_le_bytes(take(input)?), ..Default::default() };
        for v in [&mut b.x, &mut b.y, &mut b.vx, &mut b.vy] {
            *v = (0..n).map(|_| take(input).map(f64::from_le_bytes)).collect::<Result<_, _>>()?;
        }
        for v in [&mut b.m, &mut b.r] {
            *v = (0..n).map(|_| take(input).map(f32::from_le_bytes)).collect::<Result<_, _>>()?;
        }
        b.id = (0..n).map(|_| take(input).map(u32::from_le_bytes)).collect::<Result<_, _>>()?;
        b.group = vec![0; n];
        input.read_exact(&mut b.group)?;
        (b.ax, b.ay, b.fade) = (vec![0.0; n], vec![0.0; n], vec![0.0; n]);
        if b.x.iter().chain(&b.y).chain(&b.vx).chain(&b.vy).any(|v| !v.is_finite()) || b.m.iter().chain(&b.r).any(|v| !v.is_finite()) {
            return Err(bad("the file holds numbers that are not finite"));
        }
        Ok(b)
    }

    /// Where the body with this id currently sits.
    pub fn locate(&self, id: u32) -> Option<usize> {
        self.id.par_iter().position_any(|v| *v == id).filter(|i| self.alive(*i))
    }

    pub fn total_mass(&self) -> f64 {
        self.m.par_iter().map(|m| *m as f64).sum()
    }

    /// Indices of the `k` heaviest bodies, heaviest first.
    pub fn heaviest(&self, k: usize) -> Vec<usize> {
        let mut best: Vec<usize> = (0..self.len())
            .into_par_iter()
            .fold(Vec::new, |mut top: Vec<usize>, i| {
                if k > 0 && (top.len() < k || self.m[i] > self.m[*top.last().unwrap()]) {
                    let at = top.partition_point(|j| self.m[*j] >= self.m[i]);
                    top.insert(at, i);
                    top.truncate(k);
                }
                top
            })
            .reduce(Vec::new, |mut a, b| {
                a.extend(b);
                a
            });
        best.sort_by(|a, b| self.m[*b].total_cmp(&self.m[*a]).then(a.cmp(b)));
        best.truncate(k);
        best.retain(|i| self.alive(*i));
        best
    }
}

#[derive(Clone, Copy, Default)]
struct Node {
    lo: u32,
    hi: u32,
    /// 0 = leaf.
    left: u32,
    right: u32,
    /// Centre of mass and its velocity.
    cx: f64,
    cy: f64,
    vx: f64,
    vy: f64,
    /// In units of the engine's mass unit.
    mass: f64,
    min_x: f64,
    max_x: f64,
    min_y: f64,
    max_y: f64,
    /// Radius of the largest body inside (m).
    reach: f64,
    /// The length (m) that forces on the bodies of this cell are worked out in units of: its
    /// own size, so that single precision resolves bodies as finely as they are packed.
    unit: f64,
    /// Traceless quadrupole about the centre of mass, in scaled units.
    qxx: f64,
    qxy: f64,
    qyy: f64,
}

/// What the tree walk looks at, in single precision relative to the world's corner and
/// scaled to its size: a third of the memory of a full node.
#[derive(Clone, Copy)]
struct Walk {
    cx: f32,
    cy: f32,
    x0: f32,
    x1: f32,
    y0: f32,
    y1: f32,
    size2: f32,
    /// The radius of the largest body in the cell.
    r: f32,
    /// 0 = leaf; the right child follows the left one.
    left: u32,
}

impl Node {
    fn size(&self) -> f64 {
        (self.max_x - self.min_x).max(self.max_y - self.min_y)
    }
}

/// Two bodies became one. Positions are where the survivor ended up.
#[derive(Clone, Copy, Debug)]
pub struct Merge {
    pub x: f64,
    pub y: f64,
    pub vx: f64,
    pub vy: f64,
    /// Mass of the absorbed body.
    pub mass: f32,
    pub survivor: u32,
    pub absorbed: u32,
}

/// Shares a raw pointer between rayon tasks that write disjoint ranges through it.
struct Shared<T>(*mut T);
unsafe impl<T> Sync for Shared<T> {}
unsafe impl<T> Send for Shared<T> {}

/// Smallest piece of a simple pass over the bodies worth giving to another thread: these
/// passes move memory more than they compute, and waking a thread costs tens of microseconds.
const GRAIN: usize = 32_768;

/// Interleave the low 16 bits of `v` with zeros.
fn spread(v: u32) -> u32 {
    let mut x = v & 0xFFFF;
    x = (x | (x << 8)) & 0x00FF_00FF;
    x = (x | (x << 4)) & 0x0F0F_0F0F;
    x = (x | (x << 2)) & 0x3333_3333;
    (x | (x << 1)) & 0x5555_5555
}

/// Parallel least-significant-digit radix sort on the top 32 bits (three 11-bit passes).
fn radix_sort(keys: &mut Vec<u64>, tmp: &mut Vec<u64>) {
    let n = keys.len();
    tmp.resize(n, 0);
    // More pieces than threads, so one thread that is busy elsewhere does not hold up a
    // pass; but never pieces so small that handing them out costs more than doing them.
    let chunk = n.div_ceil(rayon::current_num_threads().max(1) * 4).max(GRAIN);
    for pass in 0..3 {
        let shift = 32 + 11 * pass;
        let hists: Vec<Vec<u32>> = keys
            .par_chunks(chunk)
            .map(|c| {
                let mut h = vec![0u32; 2048];
                for k in c {
                    h[((k >> shift) & 2047) as usize] += 1;
                }
                h
            })
            .collect();
        let mut offsets = vec![vec![0u32; 2048]; hists.len()];
        let mut run = 0u32;
        for d in 0..2048 {
            for (c, h) in hists.iter().enumerate() {
                offsets[c][d] = run;
                run += h[d];
            }
        }
        let out = Shared(tmp.as_mut_ptr());
        keys.par_chunks(chunk).zip(offsets.into_par_iter()).for_each(|(c, mut off)| {
            let out = &out;
            for k in c {
                let d = ((k >> shift) & 2047) as usize;
                // SAFETY: the offsets partition `0..n` between digits and chunks.
                unsafe { *out.0.add(off[d] as usize) = *k };
                off[d] += 1;
            }
        });
        std::mem::swap(keys, tmp);
    }
}

/// Reorder `v` so that element `i` is the old element `keys[i] & 0xFFFF_FFFF`.
fn permute<T: Copy + Send + Sync + Default>(v: &mut Vec<T>, scratch: &mut Vec<T>, keys: &[u64]) {
    scratch.clear();
    scratch.resize(keys.len(), T::default());
    let src: &[T] = v;
    scratch.par_iter_mut().zip(keys.par_iter()).with_min_len(GRAIN).for_each(|(o, k)| *o = src[(*k & 0xFFFF_FFFF) as usize]);
    std::mem::swap(v, scratch);
}

/// Buffers of the force pass for one group, kept by each thread from one group to the next.
#[derive(Default)]
struct Lists {
    /// The bodies of the group whose pull is wanted.
    who: Vec<u32>,
    /// Those bodies relative to the group centre, and what comes out for them.
    /// (x, y, mass, radius)
    target: [Vec<f32>; 4],
    ax: Vec<f32>,
    ay: Vec<f32>,
    rate: Vec<f32>,
    /// The nearby bodies (as the targets) and where the distant cells are, as the kernels
    /// take them.
    near: [Vec<f32>; 4],
    /// Body index of each near entry.
    ids: Vec<u32>,
    hit: Vec<u32>,
    /// What the kernels left out of the sums, among the nearby bodies and the distant cells.
    left: [Vec<kernel::Left>; 2],
    acting: Acting,
}

thread_local! {
    static LISTS: std::cell::RefCell<Lists> = std::cell::RefCell::default();
}

/// What acts on one group, found by walking the tree.
#[derive(Default)]
struct Acting {
    /// Runs of nearby bodies, each taken by itself.
    near: Vec<(u32, u32)>,
    /// Distant cells taken as lumps: where they are relative to the group centre, in units
    /// of the group's size, their mass and their quadrupole.
    far: [Vec<f32>; 6],
    /// Which cell each of those is.
    cells: Vec<u32>,
    stack: Vec<u32>,
}

/// For a world small enough to be stepped whole: a step may be this fraction of the time the
/// fastest bound orbit takes to turn one radian (about 30 steps per revolution, with the
/// fourth-order integrator used there).
pub const ORBIT_FRACTION: f64 = 0.2;
/// Two bodies that hold each other tightly are tied: what they do to each other is left out
/// of the world's step and followed in steps of the pair's own, each of which turns them
/// about each other by no more than this many radians (about 60 steps per revolution).
/// Fewer, and bodies that merely pass each other closely come out of it bound to each other
/// more often than they should.
const OWN_TURN: f64 = 0.1;
/// They are tied from the moment they would turn by this much in a step of the world:
/// sooner than a step of their own is shorter than the world's, so that two bodies falling
/// towards each other are tied before they need to be.
const TIE_TURN: f64 = 0.05;
/// No pull between tied bodies is applied for longer than it takes to turn them about each
/// other by this many radians.
const MOST_TURN: f64 = 0.6;
/// A tie's own step is the world's step halved up to this many times.
const DEEPEST: u8 = 16;
/// The world's step in ticks, the shortest own step there can be.
const WHOLE: u32 = 1 << DEEPEST;
/// Nothing is taken to be further from a group than this many times the group's size: the
/// square of a distance has to stay within single precision.
const FURTHEST: f64 = 1.0e18;
/// Up to this many bodies every pair is summed: no approximation, and cheaper than a tree.
pub const PAIRWISE_MAX: usize = 4000;
/// Up to this many, in double precision with a fourth-order integrator as well.
pub const PRECISE_MAX: usize = 192;

/// How many times the world's step `dt` has to be halved for two bodies that circle each
/// other at `rate` (rad/s).
fn depth_for(dt: f64, rate: f64) -> u8 {
    let wanted = dt * rate / OWN_TURN;
    if wanted <= 1.0 {
        0
    } else {
        (wanted.log2().ceil() as u8).min(DEEPEST)
    }
}

/// The rate to choose a tie's next step by: the one now, carried half of that step ahead
/// the way it changed over the last one (of `last` seconds, begun at `before`). Going by the
/// rate now alone, a body falling inwards would always take steps a little too long and one
/// climbing out steps a little too short, and an eccentric orbit would lose or gain a little
/// with every turn.
fn ahead(rate: f64, before: f64, last: f64) -> f64 {
    if !(rate > 0.0 && before > 0.0 && last > 0.0) {
        return rate;
    }
    let part = (0.5 * OWN_TURN / (rate * last)).min(1.0);
    rate * (rate / before).clamp(0.25, 4.0).powf(part)
}

/// How long half of a kick lasts for a tie on steps of `own` seconds that turns at `rate`
/// where the kick is given: half its step, but never so long that the pull would turn the
/// two by more than [`MOST_TURN`]. This is not how steps are meant to be kept short enough
/// (they are chosen for that); it is for bodies that find themselves far closer than the
/// step they are on allowed for, which are then not thrown apart by it.
fn half_kick(own: f64, rate: f64) -> f64 {
    0.5 * own.min(MOST_TURN / rate)
}

/// Give a tie's bodies `secs` of what they do to each other.
fn pull(b: &mut Bodies, tie: &Tie, tug: &Tug, secs: f64) {
    let i = tie.i as usize;
    b.vx[i] += tug.ax * secs;
    b.vy[i] += tug.ay * secs;
    if let (true, With::Body(j)) = (tie.both, tie.with) {
        b.vx[j as usize] -= tug.ax * tug.back * secs;
        b.vy[j as usize] -= tug.ay * tug.back * secs;
    }
}

/// Two overlapping bodies become one: mass and momentum are kept, volumes add up, and the
/// heavier lives on. Nothing happens if either is gone already.
fn merge(b: &mut Bodies, i: usize, j: usize) -> Option<Merge> {
    if i == j || b.m[i] <= 0.0 || b.m[j] <= 0.0 {
        return None;
    }
    let (s, a) = if b.m[j] > b.m[i] { (j, i) } else { (i, j) };
    let (ms, ma) = (b.m[s] as f64, b.m[a] as f64);
    let inv = 1.0 / (ms + ma);
    b.x[s] = (b.x[s] * ms + b.x[a] * ma) * inv;
    b.y[s] = (b.y[s] * ms + b.y[a] * ma) * inv;
    b.vx[s] = (b.vx[s] * ms + b.vx[a] * ma) * inv;
    b.vy[s] = (b.vy[s] * ms + b.vy[a] * ma) * inv;
    // The pull of the two on each other cancels in the weighted mean, leaving what the rest
    // of the world does to the pair.
    b.ax[s] = ((b.ax[s] as f64 * ms + b.ax[a] as f64 * ma) * inv) as f32;
    b.ay[s] = ((b.ay[s] as f64 * ms + b.ay[a] as f64 * ma) * inv) as f32;
    b.r[s] = ((b.r[s] as f64).powi(3) + (b.r[a] as f64).powi(3)).cbrt() as f32;
    b.m[s] = (ms + ma) as f32;
    let merge = Merge { x: b.x[s], y: b.y[s], vx: b.vx[s], vy: b.vy[s], mass: b.m[a], survivor: b.id[s], absorbed: b.id[a] };
    (b.m[a], b.r[a]) = (0.0, 0.0);
    Some(merge)
}

/// What a body is tied to.
#[derive(Clone, Copy, PartialEq)]
enum With {
    Body(u32),
    /// A distant cell taken as one lump: where it was when the step began (m), how it moves
    /// (m/s) and its mass (kg).
    Lump { x: f64, y: f64, vx: f64, vy: f64, mass: f64 },
}

/// Two things that hold each other too tightly for the world's step: their pull on each
/// other is followed in steps of its own.
#[derive(Clone, Copy)]
struct Tie {
    /// The body that is kicked.
    i: u32,
    with: With,
    /// The other one is a body tied to this one in turn, and kicked along with it: one entry
    /// stands for both ways.
    both: bool,
    /// How many times the world's step is halved for this tie's own.
    depth: u8,
    /// The rate (rad/s) at the tie's latest kick and at the one before, and how long (s) the
    /// step between them was.
    rate: f64,
    before: f64,
    last: f64,
}

/// What two tied things do to each other at one moment.
struct Tug {
    /// Acceleration of the body (m/s^2), and of the other one over that (the ratio of the
    /// masses, the other way round).
    ax: f64,
    ay: f64,
    back: f64,
    /// The rate (rad/s) at which the two would circle each other.
    rate: f64,
    /// Two bodies overlap.
    touching: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Distant groups approximated through the tree.
    Tree,
    /// Every pair, single-precision vector kernels.
    Pairwise,
    /// Every pair, double precision, fourth order.
    Precise,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Tree => "tree",
            Mode::Pairwise => "every pair",
            Mode::Precise => "every pair, 4th order",
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct StepStats {
    pub sort_ms: f32,
    /// Spent on bodies taking steps of their own.
    pub own_ms: f32,
    pub force_ms: f32,
    pub finish_ms: f32,
    /// Sources evaluated per body in the last force pass.
    pub interactions: f32,
    /// And for the bodies taking steps of their own, in all of those steps together, counted
    /// the same way: per body of the world.
    pub own_interactions: f32,
    pub merges: u32,
    pub removed: u32,
    /// Bodies that took more than one step of their own, and the most any took.
    pub fine: u32,
    pub most: u32,
}

/// A body counts as clear of the crowd beyond this many times the radius that holds
/// [`CROWD`] of the mass.
pub const CLEAR: f64 = 1.5;
pub const CROWD: f64 = 0.9;
/// The grid the bodies are sorted on reaches this many times as far as most of them do.
const WIDE: f64 = 4.0;
/// Each body is looked at for leaving about once in this many steps.
const ROUND: usize = 64;
const CENSUS_EVERY: u32 = 128;
/// Pairs summed a step, at most, to settle whether bodies are leaving.
const SUM_BUDGET: usize = 4_000_000;

/// Where the mass of the world is, taken now and then.
#[derive(Clone, Copy, Default)]
struct Census {
    mass: f64,
    x: f64,
    y: f64,
    vx: f64,
    vy: f64,
    /// Radius around the centre of mass holding [`CROWD`] of the mass.
    crowd: f64,
}

impl Census {
    fn take(b: &Bodies) -> Self {
        let mut c = Census::default();
        for i in 0..b.len() {
            let m = b.m[i] as f64;
            c.mass += m;
            (c.x, c.y, c.vx, c.vy) = (c.x + m * b.x[i], c.y + m * b.y[i], c.vx + m * b.vx[i], c.vy + m * b.vy[i]);
        }
        if c.mass <= 0.0 {
            return Census::default();
        }
        (c.x, c.y, c.vx, c.vy) = (c.x / c.mass, c.y / c.mass, c.vx / c.mass, c.vy / c.mass);
        let mut far: Vec<(f32, f32)> = (0..b.len()).map(|i| (((b.x[i] - c.x).powi(2) + (b.y[i] - c.y).powi(2)).sqrt() as f32, b.m[i])).collect();
        far.par_sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        let mut within = 0.0;
        for (d, m) in far {
            within += m as f64;
            c.crowd = d as f64;
            if within >= CROWD * c.mass {
                break;
            }
        }
        c
    }
}

pub struct Engine {
    level: Level,
    /// Largest number of bodies in a group.
    pub leaf: usize,
    /// Opening angle: a cell is one lump when it looks smaller than this from the group.
    pub theta: f32,
    pub g: f64,
    /// Plummer softening (m).
    pub softening: f64,
    /// Follow bodies that hold each other tightly in steps of their own. Without it every
    /// body takes the world's step, and whatever turns faster than that comes out wrong.
    pub own_steps: bool,
    /// How tightly each body is held, as of the last force pass: the rate (rad/s) at which
    /// it and whatever holds it most tightly would circle each other.
    rate: Vec<f32>,
    /// The highest of those rates.
    tightest: f32,
    /// The ties of the step under way, those on the shortest steps first; and the ones the
    /// last force pass found, which the next step will have.
    ties: Vec<Tie>,
    found: Vec<Tie>,
    /// The length (s) of the step that `found` was made for: what turns by more than
    /// [`TIE_TURN`] in it is tied. 0 when nothing is.
    tied_for: f64,
    /// The step under way: its length (s) and how many ticks of it have passed.
    span: f64,
    now: u32,
    /// The tick at which each body was last moved.
    since: Vec<u32>,
    /// Where the last sort put each body (the dead: nowhere), for the ties to follow.
    moved: Vec<u32>,
    /// Merges made part of the way through a step, handed over by the next `finish`.
    early: Vec<Merge>,
    /// How much of its fade a leaving body goes through in one step.
    pub fade_step: f32,
    /// Whether bodies leaving for good are faded out and dropped at all.
    pub drop_leavers: bool,
    census: Census,
    /// Steps until the census is taken again.
    census_due: u32,
    fading: usize,
    dice: u64,
    mass_unit: f64,
    len_unit: f64,
    nodes: Vec<Node>,
    walk: Vec<Walk>,
    /// Mass and quadrupole of each node, scaled, for the far lists.
    moments: Vec<[f32; 4]>,
    origin: (f64, f64),
    leaves: Vec<u32>,
    keys: Vec<u64>,
    tmp: Vec<u64>,
    s64: Vec<f64>,
    s32: Vec<f32>,
    su32: Vec<u32>,
    su8: Vec<u8>,
    acc_x: Vec<f32>,
    acc_y: Vec<f32>,
    pairs: Vec<(u32, u32)>,
    /// False until the first force pass over the current arrays.
    primed: bool,
    /// For a world small enough to be stepped whole (see [`Engine::precise`]): the step
    /// length the caller would like next, and the square of the fastest rate (rad/s) at
    /// which a bound pair that needs a shorter one was found turning.
    pub dt_hint: f64,
    pub omega2: f64,
    /// How the last step was computed.
    pub mode: Mode,
    steps: u64,
    pub stats: StepStats,
}

fn ms(since: std::time::Instant) -> f32 {
    since.elapsed().as_secs_f32() * 1e3
}

impl Engine {
    pub fn new(g: f64, softening: f64) -> Self {
        Self::with_level(Level::detect(), g, softening)
    }

    pub fn with_level(level: Level, g: f64, softening: f64) -> Self {
        assert!(Level::available().contains(&level), "{} is not available on this CPU", level.name());
        Self {
            level,
            leaf: 64,
            theta: 0.7,
            g,
            softening,
            own_steps: true,
            rate: Vec::new(),
            tightest: 0.0,
            ties: Vec::new(),
            found: Vec::new(),
            tied_for: 0.0,
            span: 0.0,
            now: 0,
            since: Vec::new(),
            moved: Vec::new(),
            early: Vec::new(),
            fade_step: 1.0 / 64.0,
            drop_leavers: true,
            census: Census::default(),
            census_due: 0,
            fading: 0,
            dice: 0x9E37_79B9_7F4A_7C15,
            mass_unit: 1.0,
            len_unit: 1.0,
            nodes: Vec::new(),
            walk: Vec::new(),
            moments: Vec::new(),
            origin: (0.0, 0.0),
            leaves: Vec::new(),
            keys: Vec::new(),
            tmp: Vec::new(),
            s64: Vec::new(),
            s32: Vec::new(),
            su32: Vec::new(),
            su8: Vec::new(),
            acc_x: Vec::new(),
            acc_y: Vec::new(),
            pairs: Vec::new(),
            primed: false,
            dt_hint: 0.0,
            omega2: 0.0,
            mode: Mode::Tree,
            steps: 0,
            stats: StepStats::default(),
        }
    }

    pub fn level(&self) -> Level {
        self.level
    }

    /// Compute accelerations for a freshly filled or edited set of bodies.
    pub fn prime(&mut self, b: &mut Bodies) {
        self.primed = true;
        self.ties.clear();
        self.found.clear();
        if b.is_empty() {
            return;
        }
        let mass = b.total_mass();
        self.mass_unit = if mass > 0.0 { mass } else { 1.0 };
        self.sort(b);
        self.span = self.dt_hint;
        self.forces(b);
        self.adopt(b);
        if self.precise(b) {
            self.omega2 = crate::exact::tightest(b, self.g, self.softening, self.dt_hint);
        }
    }

    /// Take what a force pass outside any step found for the bodies as they are.
    fn adopt(&mut self, b: &mut Bodies) {
        std::mem::swap(&mut b.ax, &mut self.acc_x);
        std::mem::swap(&mut b.ay, &mut self.acc_y);
        self.ties = std::mem::take(&mut self.found);
    }

    /// The bodies were changed from outside: accelerations must be worked out afresh.
    pub fn invalidate(&mut self) {
        self.primed = false;
        (self.census, self.census_due) = (Census::default(), 0);
    }

    /// Whether a world of this size is stepped in one go by [`Engine::step`] alone (the
    /// phases are for the tree and pairwise modes).
    pub fn precise(&self, b: &Bodies) -> bool {
        b.len() <= PRECISE_MAX
    }

    /// The longest step of the world in which tight orbits are still followed, going by what
    /// the last step found: one in which the tightest tie can take steps of its own as short
    /// as it needs or, in a world stepped whole, one short enough for everybody.
    pub fn longest_step(&self) -> f64 {
        if !self.own_steps {
            f64::MAX
        } else if self.omega2 > 0.0 {
            ORBIT_FRACTION / self.omega2.sqrt()
        } else if self.tightest > 0.0 {
            WHOLE as f64 * OWN_TURN / self.tightest as f64
        } else {
            f64::MAX
        }
    }

    /// One whole step. See the module docs for running the phases under a lock instead.
    pub fn step(&mut self, b: &mut Bodies, dt: f64) -> Vec<Merge> {
        if self.precise(b) {
            let t0 = std::time::Instant::now();
            let (merges, omega2) = crate::exact::step(b, dt, self.g, self.softening, self.dt_hint);
            (self.omega2, self.tightest, self.mode, self.primed) = (omega2, 0.0, Mode::Precise, true);
            self.steps += 1;
            self.stats = StepStats { force_ms: ms(t0), interactions: b.len() as f32, merges: merges.len() as u32, ..Default::default() };
            return merges;
        }
        self.begin(b, dt);
        while self.own(b, std::time::Duration::MAX) {}
        self.end(b);
        self.forces(b);
        self.finish(b)
    }

    /// What two tied things do to each other, `secs` into the step (which is where a lump
    /// has got to; bodies are taken where they are).
    fn tug(&self, b: &Bodies, tie: &Tie, secs: f64) -> Option<Tug> {
        let i = tie.i as usize;
        let (x, y, mass, reach) = match tie.with {
            With::Body(j) => (b.x[j as usize], b.y[j as usize], b.m[j as usize] as f64, (b.r[i] + b.r[j as usize]) as f64),
            With::Lump { x, y, vx, vy, mass } => (x + vx * secs, y + vy * secs, mass, 0.0),
        };
        let own = b.m[i] as f64;
        if own <= 0.0 || mass <= 0.0 {
            return None;
        }
        let (dx, dy) = (x - b.x[i], y - b.y[i]);
        let d2 = dx * dx + dy * dy;
        if d2 <= 0.0 {
            // In the very same spot: nothing pulls either way.
            return Some(Tug { ax: 0.0, ay: 0.0, back: 0.0, rate: 0.0, touching: reach > 0.0 });
        }
        let soft = d2 + self.softening * self.softening;
        let inv3 = 1.0 / (soft * soft.sqrt());
        Some(Tug { ax: self.g * mass * dx * inv3, ay: self.g * mass * dy * inv3, back: own / mass, rate: (self.g * (own + mass) * inv3).sqrt(), touching: d2 < reach * reach })
    }

    /// Phase 1: the first half of the world's kick, from everything that is not tied; and
    /// each tie gets a step of its own for this step of the world, and half its kick.
    ///
    /// A tie's own step is the world's, halved until the two turn about each other by no
    /// more than [`OWN_TURN`] in it. A moon takes many with its planet while the planet takes
    /// one with its star.
    pub fn begin(&mut self, b: &mut Bodies, dt: f64) {
        if !self.primed {
            self.prime(b);
        }
        // Ties are made for a step of some length. For one much longer or shorter (or when
        // following them was switched on or off) they are made afresh.
        let fits = if self.own_steps { self.tied_for > 0.0 && (0.5..=2.0).contains(&(self.tied_for / dt)) } else { self.tied_for == 0.0 };
        if !fits && !b.is_empty() {
            self.span = dt;
            self.forces(b);
            self.adopt(b);
        }
        (self.span, self.now) = (dt, 0);
        (self.stats.own_ms, self.stats.own_interactions, self.stats.fine, self.stats.most) = (0.0, 0.0, 0, 1);
        if b.is_empty() {
            return;
        }
        let t0 = std::time::Instant::now();
        self.since.clear();
        self.since.resize(b.len(), 0);
        b.late.clear();
        b.late.resize(b.len(), 0.0);
        (&mut b.vx, &mut b.vy, &b.ax, &b.ay).into_par_iter().with_min_len(GRAIN).for_each(|(vx, vy, ax, ay)| {
            *vx += *ax as f64 * 0.5 * dt;
            *vy += *ay as f64 * 0.5 * dt;
        });
        self.stats.sort_ms = ms(t0);
        // What follows is for the ties, and counted as theirs.
        let t0 = std::time::Instant::now();
        let mut ties = std::mem::take(&mut self.ties);
        // Bodies that overlap become one before anything pulls on them: the closer they are,
        // the harder they would be thrown apart.
        for tie in &ties {
            if let (Some(Tug { touching: true, .. }), With::Body(j)) = (self.tug(b, tie, 0.0), tie.with) {
                self.early.extend(merge(b, tie.i as usize, j as usize));
            }
        }
        let this: &Engine = self;
        let tugs: Vec<Option<Tug>> = ties.par_iter().with_min_len(4096).map(|tie| this.tug(b, tie, 0.0)).collect();
        for (tie, tug) in ties.iter_mut().zip(&tugs) {
            let Some(tug) = tug else {
                tie.depth = 0;
                continue;
            };
            // (With following switched off there are still ties, between bodies too close
            // together for the world's sums, but they take the world's step.)
            tie.depth = if self.own_steps { depth_for(dt, ahead(tug.rate, tie.before, tie.last)) } else { 0 };
            pull(b, tie, tug, half_kick(dt / (1u32 << tie.depth) as f64, tug.rate));
            self.stats.most = self.stats.most.max(1 << tie.depth);
        }
        // The shortest steps first: at most moments only the first few have anything to do.
        ties.sort_unstable_by_key(|tie| std::cmp::Reverse(tie.depth));
        let mut tied: Vec<u32> = ties.iter().flat_map(|tie| [Some(tie.i), if let With::Body(j) = tie.with { Some(j) } else { None }]).flatten().collect();
        tied.sort_unstable();
        tied.dedup();
        self.stats.fine = tied.len() as u32;
        self.ties = ties;
        self.stats.own_ms += ms(t0);
    }

    /// Phase 2: the ties' own steps. At each moment at which some of them end a step before
    /// the world's step does, their bodies are moved to where they are by then and kicked by
    /// each other, and the tie chooses its next step; bodies that have come to overlap merge.
    ///
    /// Works for about `at_most` and says whether there is more to do, so that whoever
    /// draws the bodies can be let in between.
    pub fn own(&mut self, b: &mut Bodies, at_most: std::time::Duration) -> bool {
        let t0 = std::time::Instant::now();
        let (dt, tick) = (self.span, self.span / WHOLE as f64);
        let mut ties = std::mem::take(&mut self.ties);
        let mut kicks = 0;
        let more = loop {
            let Some(deepest) = ties.first().map(|tie| tie.depth).filter(|depth| *depth > 0) else { break false };
            // The shortest steps there are now end at multiples of their length. (Not simply
            // one such length on: the tie that was on shorter ones may just have gone.)
            let shortest = WHOLE >> deepest;
            let now = (self.now / shortest + 1) * shortest;
            if now >= WHOLE {
                break false;
            }
            self.now = now;
            // Steps end at multiples of their length, so the more often the more they are
            // halved: at this moment, those halved at least this many times.
            let ending = DEEPEST - now.trailing_zeros() as u8;
            let due = ties.partition_point(|tie| tie.depth >= ending);
            let mut gone = false;
            for tie in &mut ties[..due] {
                self.bring(b, tie.i as usize);
                if let With::Body(j) = tie.with {
                    self.bring(b, j as usize);
                }
                let tug = self.tug(b, tie, now as f64 * tick);
                if let (Some(Tug { touching: true, .. }), With::Body(j)) = (&tug, tie.with) {
                    self.early.extend(merge(b, tie.i as usize, j as usize));
                }
                let Some(tug) = tug.filter(|tug| !tug.touching) else {
                    (tie.depth, gone) = (0, true);
                    continue;
                };
                let ended = dt / (1u32 << tie.depth) as f64;
                // The step the tie now asks for, if such a step can end in time with all
                // the others: so not a longer one than those ending at this moment.
                tie.depth = depth_for(dt, ahead(tug.rate, tie.rate, ended)).max(ending);
                let next = dt / (1u32 << tie.depth) as f64;
                pull(b, tie, &tug, half_kick(ended, tug.rate) + half_kick(next, tug.rate));
                (tie.before, tie.rate, tie.last) = (tie.rate, tug.rate, ended);
                self.stats.most = self.stats.most.max(1 << tie.depth);
            }
            kicks += due;
            // Only those that were due can have changed, and none of them to less than was
            // due unless it is gone: putting them back in order leaves the whole list in order.
            let stirred = if gone { ties.len() } else { due };
            ties[..stirred].sort_unstable_by_key(|tie| std::cmp::Reverse(tie.depth));
            if t0.elapsed() >= at_most {
                break true;
            }
        };
        self.ties = ties;
        self.stats.own_interactions += kicks as f32 / b.len().max(1) as f32;
        self.stats.own_ms += ms(t0);
        more
    }

    /// Move a body, in a straight line, from where it was last put to where it is now.
    fn bring(&mut self, b: &mut Bodies, i: usize) {
        let secs = (self.now - self.since[i]) as f64 * self.span / WHOLE as f64;
        b.x[i] += b.vx[i] * secs;
        b.y[i] += b.vy[i] * secs;
        self.since[i] = self.now;
        b.late[i] = (self.now as f64 * self.span / WHOLE as f64) as f32;
    }

    /// Phase 3: every body is moved to the end of the world's step, the ties give the second
    /// half of their kicks there, and the bodies are put back in order for the force pass.
    /// Afterwards positions are those of the next tick.
    pub fn end(&mut self, b: &mut Bodies) {
        if b.is_empty() {
            return;
        }
        let t0 = std::time::Instant::now();
        let (dt, tick) = (self.span, self.span / WHOLE as f64);
        (&mut b.x, &mut b.y, &b.vx, &b.vy, &self.since).into_par_iter().with_min_len(GRAIN).for_each(|(x, y, vx, vy, since)| {
            let secs = (WHOLE - since) as f64 * tick;
            *x += vx * secs;
            *y += vy * secs;
        });
        b.late.clear();
        let tied = std::time::Instant::now();
        let mut ties = std::mem::take(&mut self.ties);
        for tie in &mut ties {
            // (Those that overlap by now are about to merge.)
            let Some(tug) = self.tug(b, tie, dt).filter(|tug| !tug.touching) else { continue };
            let own = dt / (1u32 << tie.depth) as f64;
            pull(b, tie, &tug, half_kick(own, tug.rate));
            (tie.before, tie.rate, tie.last) = (tie.rate, tug.rate, own);
        }
        self.ties = ties;
        let tied = ms(tied);
        self.stats.own_ms += tied;
        self.sort(b);
        self.stats.sort_ms += ms(t0) - tied;
    }

    /// Reorder the bodies along the Z-order curve, dropping dead ones, and rebuild the tree.
    fn sort(&mut self, b: &mut Bodies) {
        let n = b.len();
        let span = |v: &Vec<f64>| {
            v.par_iter().with_min_len(GRAIN).fold(|| (f64::MAX, f64::MIN), |a, v| (a.0.min(*v), a.1.max(*v))).reduce(|| (f64::MAX, f64::MIN), |a, b| (a.0.min(b.0), a.1.max(b.1)))
        };
        let ((mut min_x, mut max_x), (mut min_y, mut max_y)) = (span(&b.x), span(&b.y));
        // A body far out must not stretch the grid until everything else sits in one cell of
        // it: the grid is laid over [`WIDE`] times the stretch that holds the middle nine tenths
        // of the bodies along each axis (going by a sample of them), and what is further out is
        // squeezed in along its edges.
        let middle = |v: &Vec<f64>| {
            let mut some: Vec<f64> = v.iter().step_by((n / 2048).max(1)).copied().collect();
            some.sort_unstable_by(f64::total_cmp);
            let (lo, hi) = (some[some.len() / 20], some[some.len() - 1 - some.len() / 20]);
            (0.5 * (lo + hi), 0.5 * WIDE * (hi - lo))
        };
        if n > 0 {
            let ((cx, reach_x), (cy, reach_y)) = (middle(&b.x), middle(&b.y));
            // The same distance both ways, so that cells stay square.
            let reach = reach_x.max(reach_y);
            if reach > 0.0 {
                (min_x, max_x) = (min_x.max(cx - reach), max_x.min(cx + reach));
                (min_y, max_y) = (min_y.max(cy - reach), max_y.min(cy + reach));
            }
        }
        let across = (max_x - min_x).max(max_y - min_y);
        self.len_unit = if across > 0.0 { across } else { 1.0 };
        self.origin = (min_x, min_y);
        let len_unit = self.len_unit;
        // Where on the grid a coordinate falls: evenly over the middle nine tenths for what
        // is within the box, and ever closer to the ends for what is beyond it.
        let cell = |v: f64, lo: f64| {
            let u = (v - lo) / len_unit;
            let across = if u < 0.0 { 0.05 / (1.0 - u) } else if u > 1.0 { 1.0 - 0.05 / u } else { 0.05 + 0.9 * u };
            (across * 65534.0) as u32
        };
        self.keys.resize(n, 0);
        let (x, y, m) = (&b.x, &b.y, &b.m);
        let dead = m.par_iter().with_min_len(GRAIN).filter(|m| **m <= 0.0).count();
        self.keys.par_iter_mut().enumerate().with_min_len(GRAIN).for_each(|(i, k)| {
            // The dead sort past every live body and are cut off below.
            let key = if m[i] > 0.0 { spread(cell(x[i], min_x)) << 1 | spread(cell(y[i], min_y)) } else { u32::MAX };
            *k = ((key as u64) << 32) | i as u64;
        });
        radix_sort(&mut self.keys, &mut self.tmp);
        self.keys.truncate(n - dead);
        for v in [&mut b.x, &mut b.y, &mut b.vx, &mut b.vy] {
            permute(v, &mut self.s64, &self.keys);
        }
        for v in [&mut b.m, &mut b.r, &mut b.fade] {
            permute(v, &mut self.s32, &self.keys);
        }
        permute(&mut b.id, &mut self.su32, &self.keys);
        permute(&mut b.group, &mut self.su8, &self.keys);
        if !self.ties.is_empty() {
            // The ties go where their bodies went, and those of the dead go with them.
            self.moved.clear();
            self.moved.resize(n, u32::MAX);
            for (to, key) in self.keys.iter().enumerate() {
                self.moved[(key & 0xFFFF_FFFF) as usize] = to as u32;
            }
            let moved = &self.moved;
            self.ties.retain_mut(|tie| {
                tie.i = moved[tie.i as usize];
                if let With::Body(j) = &mut tie.with {
                    *j = moved[*j as usize];
                    if *j == u32::MAX {
                        return false;
                    }
                }
                tie.i != u32::MAX
            });
        }
        // Accelerations are recomputed before anyone reads them again.
        b.ax.truncate(n - dead);
        b.ay.truncate(n - dead);
        self.build();
    }

    /// Top-down: split each range of bodies where the highest remaining key bit flips, so
    /// every node is a cell of the plane, halved alternately in x and y.
    fn build(&mut self) {
        let n = self.keys.len() as u32;
        let keys = &self.keys;
        let nodes = &mut self.nodes;
        nodes.clear();
        nodes.push(Node { lo: 0, hi: n, ..Default::default() });
        let mut leaves: Vec<(u32, u32)> = Vec::new();
        // (node, next key bit to test)
        let mut stack: Vec<(u32, i32)> = vec![(0, 63)];
        while let Some((node, mut bit)) = stack.pop() {
            let (lo, hi) = (nodes[node as usize].lo, nodes[node as usize].hi);
            let mut split = lo;
            if (hi - lo) as usize > self.leaf {
                while bit >= 32 {
                    let p = keys[lo as usize..hi as usize].partition_point(|k| (k >> bit) & 1 == 0) as u32;
                    bit -= 1;
                    if p > 0 && p < hi - lo {
                        split = lo + p;
                        break;
                    }
                }
            }
            if split == lo {
                // Small enough, or all in one cell of the finest grid.
                leaves.push((lo, node));
                continue;
            }
            let a = nodes.len() as u32;
            nodes.push(Node { lo, hi: split, ..Default::default() });
            nodes.push(Node { lo: split, hi, ..Default::default() });
            nodes[node as usize].left = a;
            nodes[node as usize].right = a + 1;
            stack.push((a, bit));
            stack.push((a + 1, bit));
        }
        leaves.sort_unstable();
        self.leaves = leaves.into_iter().map(|l| l.1).collect();
    }

    /// Recompute every node's mass, centre, velocity, box and quadrupole.
    fn refit(&mut self, b: &Bodies) {
        let (inv_mu, inv_lu) = (1.0 / self.mass_unit, 1.0 / self.len_unit);
        let nodes = &self.nodes;
        let fitted: Vec<Node> = self
            .leaves
            .par_iter()
            .map(|&at| {
                let mut node = nodes[at as usize];
                let range = node.lo as usize..node.hi as usize;
                let (mut m, mut sx, mut sy, mut svx, mut svy) = (0.0, 0.0, 0.0, 0.0, 0.0);
                let (mut x0, mut x1, mut y0, mut y1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
                let mut rmax = 0.0f32;
                for i in range.clone() {
                    let w = b.m[i] as f64 * inv_mu;
                    m += w;
                    sx += w * b.x[i];
                    sy += w * b.y[i];
                    svx += w * b.vx[i];
                    svy += w * b.vy[i];
                    x0 = x0.min(b.x[i]);
                    x1 = x1.max(b.x[i]);
                    y0 = y0.min(b.y[i]);
                    y1 = y1.max(b.y[i]);
                    rmax = rmax.max(b.r[i]);
                }
                let inv = if m > 0.0 { 1.0 / m } else { 0.0 };
                // A group of only massless bodies still needs a centre inside its box.
                let (cx, cy) = if m > 0.0 { (sx * inv, sy * inv) } else { (0.5 * (x0 + x1), 0.5 * (y0 + y1)) };
                let (mut qxx, mut qxy, mut qyy) = (0.0, 0.0, 0.0);
                for i in range {
                    let w = b.m[i] as f64 * inv_mu;
                    let (rx, ry) = ((b.x[i] - cx) * inv_lu, (b.y[i] - cy) * inv_lu);
                    qxx += w * (2.0 * rx * rx - ry * ry);
                    qyy += w * (2.0 * ry * ry - rx * rx);
                    qxy += w * 3.0 * rx * ry;
                }
                node = Node { cx, cy, vx: svx * inv, vy: svy * inv, mass: m, min_x: x0, max_x: x1, min_y: y0, max_y: y1, reach: rmax as f64, qxx, qxy, qyy, ..node };
                node
            })
            .collect();
        for (k, node) in fitted.into_iter().enumerate() {
            self.nodes[self.leaves[k] as usize] = node;
        }
        // Children were always created after their parent.
        for i in (0..self.nodes.len()).rev() {
            let (a, c) = (self.nodes[i].left as usize, self.nodes[i].right as usize);
            if a == 0 {
                continue;
            }
            let (l, r) = (self.nodes[a], self.nodes[c]);
            let m = l.mass + r.mass;
            let inv = if m > 0.0 { 1.0 / m } else { 0.0 };
            let (wl, wr) = if m > 0.0 { (l.mass * inv, r.mass * inv) } else { (0.5, 0.5) };
            let (cx, cy) = (l.cx * wl + r.cx * wr, l.cy * wl + r.cy * wr);
            // Shift each child's quadrupole to the parent's centre and add.
            let (mut qxx, mut qxy, mut qyy) = (0.0, 0.0, 0.0);
            for child in [l, r] {
                let (rx, ry) = ((child.cx - cx) * inv_lu, (child.cy - cy) * inv_lu);
                qxx += child.qxx + child.mass * (2.0 * rx * rx - ry * ry);
                qyy += child.qyy + child.mass * (2.0 * ry * ry - rx * rx);
                qxy += child.qxy + child.mass * 3.0 * rx * ry;
            }
            let node = &mut self.nodes[i];
            node.mass = m;
            (node.cx, node.cy) = (cx, cy);
            (node.vx, node.vy) = (l.vx * wl + r.vx * wr, l.vy * wl + r.vy * wr);
            (node.min_x, node.max_x) = (l.min_x.min(r.min_x), l.max_x.max(r.max_x));
            (node.min_y, node.max_y) = (l.min_y.min(r.min_y), l.max_y.max(r.max_y));
            node.reach = l.reach.max(r.reach);
            (node.qxx, node.qxy, node.qyy) = (qxx, qxy, qyy);
        }
        // A cell of no size (one body, or several in one spot) measures as its parent does.
        // Parents come before their children.
        for i in 0..self.nodes.len() {
            let (size, left) = (self.nodes[i].size(), self.nodes[i].left as usize);
            if i == 0 {
                self.nodes[0].unit = if size > 0.0 { size } else { 1.0 };
            }
            let unit = self.nodes[i].unit;
            for child in [left, left + 1].into_iter().filter(|_| left != 0) {
                let size = self.nodes[child].size();
                self.nodes[child].unit = if size > 0.0 { size } else { unit };
            }
        }
        let (ox, oy) = self.origin;
        let nodes = &self.nodes;
        self.walk.clear();
        self.walk.par_extend(nodes.par_iter().map(|n| {
            let f = |v: f64, o: f64| ((v - o) * inv_lu) as f32;
            let size = (n.size() * inv_lu) as f32;
            Walk { cx: f(n.cx, ox), cy: f(n.cy, oy), x0: f(n.min_x, ox), x1: f(n.max_x, ox), y0: f(n.min_y, oy), y1: f(n.max_y, oy), size2: size * size, r: (n.reach * inv_lu) as f32, left: n.left }
        }));
        self.moments.clear();
        self.moments.par_extend(nodes.par_iter().map(|n| [n.mass as f32, n.qxx as f32, n.qxy as f32, n.qyy as f32]));
    }

    /// Phase 4: accelerations at the current positions from everything that is not tied,
    /// how tightly each body is held there, which bodies are tied, and which overlap.
    ///
    /// Two bodies are tied when they would circle each other by more than [`TIE_TURN`] in a
    /// step of the world (as long as the one under way, or last taken), or when they are
    /// too close together for single precision to tell how far apart ([`kernel::CLOSE`]).
    pub fn forces(&mut self, b: &Bodies) {
        self.omega2 = 0.0;
        self.pairs.clear();
        self.found.clear();
        self.tied_for = 0.0;
        if b.is_empty() {
            return;
        }
        let t0 = std::time::Instant::now();
        self.refit(b);
        let n = b.len();
        self.mode = if n <= PAIRWISE_MAX { Mode::Pairwise } else { Mode::Tree };
        if self.own_steps && self.span > 0.0 {
            self.tied_for = self.span;
        }
        // Taken out of `self` for the pass, so that each group can fill in its own stretch.
        let mut out = [std::mem::take(&mut self.acc_x), std::mem::take(&mut self.acc_y), std::mem::take(&mut self.rate)];
        for list in out.iter_mut() {
            list.clear();
            list.resize(n, 0.0);
        }
        let to = out.each_mut().map(|list| Shared(list.as_mut_ptr()));
        let this: &Engine = self;
        let (count, mut pairs, mut tied) = (this.leaves.par_iter())
            .map(|&at| {
                LISTS.with(|l| {
                    let (l, to) = (&mut *l.borrow_mut(), &to);
                    let node = &this.nodes[at as usize];
                    l.who.clear();
                    l.who.extend(node.lo..node.hi);
                    let (mut touching, mut tied) = (Vec::new(), Vec::new());
                    let mut acting = std::mem::take(&mut l.acting);
                    this.acting(at, &mut acting);
                    let sources = this.pull(b, at, &acting, l, &mut touching, &mut tied);
                    l.acting = acting;
                    for (to, from) in to.iter().zip([&l.ax, &l.ay, &l.rate]) {
                        // SAFETY: groups cover disjoint ranges of bodies.
                        unsafe { std::ptr::copy_nonoverlapping(from.as_ptr(), to.0.add(node.lo as usize), from.len()) };
                    }
                    (sources * l.who.len(), touching, tied)
                })
            })
            .reduce(
                || (0, Vec::new(), Vec::new()),
                |mut a, mut b| {
                    a.0 += b.0;
                    a.1.append(&mut b.1);
                    a.2.append(&mut b.2);
                    a
                },
            );
        let [acc_x, acc_y, rate] = out;
        self.tightest = rate.par_iter().with_min_len(GRAIN).copied().reduce(|| 0.0, f32::max);
        (self.acc_x, self.acc_y, self.rate) = (acc_x, acc_y, rate);
        // A pair is found from either side.
        pairs.par_sort_unstable();
        pairs.dedup();
        self.pairs = pairs;
        // So is a tie between two bodies, when each has the other among those it takes one by
        // one: then one entry stands for both ways, and what the two do to each other cancels
        // exactly. Seen from one side only (the other takes it as part of a lump), it is
        // that side's alone.
        let sorting = std::time::Instant::now();
        let mut bodies: Vec<(u32, u32)> = tied.iter().filter_map(|(i, with)| if let With::Body(j) = with { Some((*i, *j)) } else { None }).collect();
        bodies.par_sort_unstable();
        tied.retain(|(i, with)| match with {
            With::Body(j) => i < j || bodies.binary_search(&(*j, *i)).is_err(),
            With::Lump { .. } => true,
        });
        self.found.extend(tied.into_iter().map(|(i, with)| {
            let both = matches!(with, With::Body(j) if bodies.binary_search(&(j, i)).is_ok());
            Tie { i, with, both, depth: 0, rate: 0.0, before: 0.0, last: 0.0 }
        }));
        self.stats.interactions = count as f32 / n as f32;
        // (Sorting the ties out is their cost, not the force pass's.)
        self.stats.own_ms += ms(sorting);
        self.stats.force_ms = ms(t0) - ms(sorting);
    }

    /// What acts on the group `at`, by walking the tree from the top: cells that look small
    /// enough from the group count as lumps, the bodies of the rest one by one.
    fn acting(&self, at: u32, a: &mut Acting) {
        // A zero opening angle opens every cell: the sum over every pair, through the same
        // code, with each group still measuring from its own centre.
        let theta = if self.mode == Mode::Pairwise { 0.0 } else { self.theta };
        let theta2 = theta * theta;
        let group = &self.nodes[at as usize];
        let (ox, oy) = (0.5 * (group.min_x + group.max_x), 0.5 * (group.min_y + group.max_y));
        let inv_u = 1.0 / group.unit;
        // Quadrupoles are kept in units of the whole world's size.
        let wide = ((self.len_unit * inv_u) * (self.len_unit * inv_u)) as f32;
        let me = self.walk[at as usize];
        a.near.clear();
        a.cells.clear();
        for list in a.far.iter_mut() {
            list.clear();
        }
        a.stack.clear();
        a.stack.push(0);
        while let Some(i) = a.stack.pop() {
            let w = &self.walk[i as usize];
            // Distance from the cell's centre of mass to the nearest point of the group.
            let dx = (me.x0 - w.cx).max(w.cx - me.x1).max(0.0);
            let dy = (me.y0 - w.cy).max(w.cy - me.y1).max(0.0);
            let gx = (me.x0 - w.x1).max(w.x0 - me.x1).max(0.0);
            let gy = (me.y0 - w.y1).max(w.y0 - me.y1).max(0.0);
            // No body of a cell further than this from the group's box can touch a body in it
            // (with slack for the rounding of single precision).
            let clear = me.r + w.r + 2.0e-6;
            if w.size2 < theta2 * (dx * dx + dy * dy) && gx * gx + gy * gy > clear * clear {
                // Single precision is plenty for where a distant cell is, once that is taken
                // from the group's centre.
                let (n, m) = (&self.nodes[i as usize], &self.moments[i as usize]);
                let place = [(n.cx - ox) * inv_u, (n.cy - oy) * inv_u].map(|v| v.clamp(-FURTHEST, FURTHEST) as f32);
                for (list, v) in a.far.iter_mut().zip([place[0], place[1], m[0], m[1] * wide, m[2] * wide, m[3] * wide]) {
                    list.push(v);
                }
                a.cells.push(i);
            } else if w.left == 0 {
                let node = &self.nodes[i as usize];
                a.near.push((node.lo, node.hi));
            } else {
                a.stack.push(w.left);
                a.stack.push(w.left + 1);
            }
        }
        for (k, list) in a.far.iter_mut().enumerate() {
            // Padding sits out of the way, with no mass.
            list.resize(list.len().div_ceil(PAD) * PAD, if k == 0 { PAD_X } else { 0.0 });
        }
    }

    /// What `acting` does to the bodies of the group `at` listed in `l.who`, as far as it is
    /// not tied to them.
    ///
    /// Leaves the accelerations (m/s^2) in `l.ax` and `l.ay` and how tightly each body is
    /// held (rad/s, see `rate`) in `l.rate`, adds the pairs found overlapping to `touching`
    /// and what each body is tied to to `tied`, and returns the number of sources.
    fn pull(&self, b: &Bodies, at: u32, acting: &Acting, l: &mut Lists, touching: &mut Vec<(u32, u32)>, tied: &mut Vec<(u32, With)>) -> usize {
        let group = &self.nodes[at as usize];
        let (ox, oy) = (0.5 * (group.min_x + group.max_x), 0.5 * (group.min_y + group.max_y));
        // Everything is measured from the group's centre in units of its size.
        let inv_lu = 1.0 / group.unit;
        let inv_mu = (1.0 / self.mass_unit) as f32;
        let inv_lu32 = inv_lu as f32;
        // Softening in those units. The floor is there for a body's distance to itself, which
        // must not divide by zero, and keeps 1/d^3 within single precision.
        let eps = (self.softening * inv_lu).max(1.0e-12) as f32;
        let eps2 = eps * eps;
        // The kernels' m / d^3 is the square of a rate, in units of this.
        let rate_unit = self.g * self.mass_unit * inv_lu * inv_lu * inv_lu;
        // What turns faster than this is tied.
        let cut = if self.tied_for > 0.0 { ((TIE_TURN / self.tied_for).powi(2) / rate_unit).min(f32::MAX as f64) as f32 } else { f32::INFINITY };
        let place = |v: f64| v.clamp(-FURTHEST, FURTHEST) as f32;

        // The nearby bodies, relative to the group centre.
        let bodies: usize = acting.near.iter().map(|run| (run.1 - run.0) as usize).sum();
        l.ids.clear();
        for list in l.near.iter_mut().chain(l.target.iter_mut()) {
            list.clear();
        }
        for i in acting.near.iter().flat_map(|run| run.0..run.1).map(|i| (i, false)).chain(l.who.iter().map(|i| (*i, true))) {
            let (lists, i) = (if i.1 { &mut l.target } else { &mut l.near }, i.0 as usize);
            for (list, v) in lists.iter_mut().zip([place((b.x[i] - ox) * inv_lu), place((b.y[i] - oy) * inv_lu), b.m[i] * inv_mu, b.r[i] * inv_lu32]) {
                list.push(v);
            }
        }
        l.ids.extend(acting.near.iter().flat_map(|run| run.0..run.1));
        for (k, list) in l.near.iter_mut().enumerate() {
            // Only x is moved out of the way; mass and radius are zero.
            list.resize(bodies.div_ceil(PAD) * PAD, if k == 0 { PAD_X } else { 0.0 });
        }
        for out in [&mut l.ax, &mut l.ay, &mut l.rate] {
            out.clear();
            out.resize(l.who.len(), 0.0);
        }
        l.hit.clear();
        let [left_near, left_far] = &mut l.left;
        left_near.clear();
        left_far.clear();
        let [tx, ty, tm, tr] = &l.target;
        let targets = Targets { x: tx, y: ty, m: tm, r: tr };
        let [nx, ny, nm, nr] = &l.near;
        let near = Near { x: nx, y: ny, m: nm, r: nr };
        kernel::near(self.level, &targets, &mut l.ax, &mut l.ay, &mut l.rate, &near, eps2, cut, &mut l.hit, left_near);
        let [fx, fy, fm, qxx, qxy, qyy] = &acting.far;
        if !fm.is_empty() {
            let far = Far { x: fx, y: fy, m: fm, qxx, qxy, qyy };
            kernel::far(self.level, &targets, &mut l.ax, &mut l.ay, &mut l.rate, &far, eps2, cut, left_far);
        }
        // Out of the kernels' scaled units.
        let to_si = (self.g * self.mass_unit * inv_lu * inv_lu) as f32;
        for k in 0..l.who.len() {
            l.ax[k] *= to_si;
            l.ay[k] *= to_si;
            l.rate[k] = (l.rate[k] as f64 * rate_unit).sqrt() as f32;
        }
        // What was left out of those sums is what the bodies are tied to (a body itself, which
        // the kernels meet at distance zero, aside; and padding, should it ever qualify).
        let lanes = |bits: u32| (0..32).filter(move |lane| bits >> lane & 1 == 1);
        for &(target, first, bits) in left_near.iter() {
            let i = l.who[target as usize];
            tied.extend(lanes(bits).filter_map(|lane| l.ids.get((first + lane) as usize)).filter(|j| **j != i).map(|j| (i, With::Body(*j))));
        }
        for &(target, first, bits) in left_far.iter() {
            let i = l.who[target as usize];
            tied.extend(lanes(bits).filter_map(|lane| acting.cells.get((first + lane) as usize)).map(|cell| {
                let n = &self.nodes[*cell as usize];
                (i, With::Lump { x: n.cx, y: n.cy, vx: n.vx, vy: n.vy, mass: n.mass * self.mass_unit })
            }));
        }
        // Rare: find out who exactly the flagged bodies touch, in full precision.
        for &t in &l.hit {
            let a = l.who[t as usize] as usize;
            for &j in &l.ids {
                let j = j as usize;
                if a != j && b.m[a] > 0.0 && b.m[j] > 0.0 {
                    let reach = (b.r[a] + b.r[j]) as f64;
                    if (b.x[j] - b.x[a]).powi(2) + (b.y[j] - b.y[a]).powi(2) < reach * reach {
                        touching.push((a.min(j) as u32, a.max(j) as u32));
                    }
                }
            }
        }
        bodies + fm.len()
    }

    /// Phase 5: the second half of the world's kick, the ties change hands, and the
    /// overlapping pairs merge.
    pub fn finish(&mut self, b: &mut Bodies) -> Vec<Merge> {
        if b.is_empty() {
            return Vec::new();
        }
        let t0 = std::time::Instant::now();
        std::mem::swap(&mut b.ax, &mut self.acc_x);
        std::mem::swap(&mut b.ay, &mut self.acc_y);
        let half = 0.5 * self.span;
        (&mut b.vx, &mut b.vy, &b.ax, &b.ay).into_par_iter().with_min_len(GRAIN).for_each(|(vx, vy, ax, ay)| {
            *vx += *ax as f64 * half;
            *vy += *ay as f64 * half;
        });
        let handing = std::time::Instant::now();
        self.hand_over(b);
        self.stats.own_ms += ms(handing);
        let t0 = t0 + handing.elapsed();
        // Those that met part of the way through the step come first.
        let mut merges = std::mem::take(&mut self.early);
        for &(i, j) in &self.pairs {
            merges.extend(merge(b, i as usize, j as usize));
        }
        self.steps += 1;
        self.stats.merges = merges.len() as u32;
        self.stats.finish_ms = ms(t0);
        merges
    }

    /// The ties of the step that is ending give way to those the force pass has just found.
    ///
    /// That pass left the new ties out of the accelerations, with which every body has now
    /// had the second half of its kick. But this step's kick is owed for what was not tied
    /// while it lasted. So whatever is newly tied is given its half kick here (no longer than
    /// [`half_kick`] allows: this is where two bodies that fell together within the step are
    /// found out), and whatever is tied no longer has it taken back.
    fn hand_over(&mut self, b: &mut Bodies) {
        let dt = self.span;
        let (old, mut new) = (std::mem::take(&mut self.ties), std::mem::take(&mut self.found));
        // Each way of each tie between two bodies, and where it is in its list.
        let ways = |ties: &[Tie]| {
            let mut ways: Vec<(u32, u32, u32)> = Vec::new();
            for (at, tie) in ties.iter().enumerate() {
                if let With::Body(j) = tie.with {
                    ways.push((tie.i, j, at as u32));
                    if tie.both {
                        ways.push((j, tie.i, at as u32));
                    }
                }
            }
            ways.sort_unstable();
            ways
        };
        let (was, is) = (ways(&old), ways(&new));
        let one_way = |i: u32, j: u32| Tie { i, with: With::Body(j), both: false, depth: 0, rate: 0.0, before: 0.0, last: 0.0 };
        let (mut a, mut c) = (0, 0);
        while a < was.len() || c < is.len() {
            let ended = was.get(a).map(|way| (way.0, way.1));
            let begun = is.get(c).map(|way| (way.0, way.1));
            if ended.is_some() && ended == begun {
                // Goes on: so does what is known of how it has been turning.
                let (from, to) = (&old[was[a].2 as usize], &mut new[is[c].2 as usize]);
                (to.rate, to.before, to.last) = (from.rate, from.before, from.last);
                (a, c) = (a + 1, c + 1);
            } else if begun.is_none() || ended.is_some_and(|ended| Some(ended) < begun) {
                let tie = one_way(was[a].0, was[a].1);
                if let Some(tug) = self.tug(b, &tie, dt) {
                    pull(b, &tie, &tug, -0.5 * dt);
                }
                a += 1;
            } else {
                let tie = one_way(is[c].0, is[c].1);
                if let Some(tug) = self.tug(b, &tie, dt).filter(|tug| !tug.touching) {
                    pull(b, &tie, &tug, half_kick(dt, tug.rate));
                }
                c += 1;
            }
        }
        // Lumps are never the same from one step to the next.
        for tie in old.iter().filter(|tie| matches!(tie.with, With::Lump { .. })) {
            if let Some(tug) = self.tug(b, tie, dt) {
                pull(b, tie, &tug, -0.5 * dt);
            }
        }
        for tie in new.iter().filter(|tie| matches!(tie.with, With::Lump { .. })) {
            if let Some(tug) = self.tug(b, tie, 0.0) {
                pull(b, tie, &tug, 0.5 * dt);
            }
        }
        self.ties = new;
    }

    /// Find bodies that are leaving for good, and fade and drop those found earlier.
    ///
    /// A body is leaving when it is beyond [`CLEAR`] times the radius holding [`CROWD`] of the
    /// mass, moving outwards, and has more energy than the pull of every other body can take
    /// back. A random handful is looked at each step, so each body comes up every
    /// [`ROUND`] steps or so; the energy is summed exactly, and only for those few that a
    /// cheap estimate lets through.
    pub fn leave(&mut self, b: &mut Bodies) {
        self.stats.removed = 0;
        let n = b.len();
        if !self.drop_leavers {
            // Those already on their way out stay after all.
            if std::mem::take(&mut self.fading) > 0 {
                b.fade.fill(0.0);
            }
            self.census_due = 0;
            return;
        }
        if n < 2 {
            return;
        }
        if self.census_due == 0 {
            self.census = Census::take(b);
            self.census_due = CENSUS_EVERY;
            self.fading = b.fade.iter().filter(|f| **f > 0.0).count();
        }
        self.census_due -= 1;
        let c = self.census;
        if self.fading > 0 {
            let mut left = 0;
            for i in 0..n {
                if b.fade[i] > 0.0 && b.m[i] > 0.0 {
                    b.fade[i] += self.fade_step;
                    if b.fade[i] >= 1.0 {
                        (b.m[i], b.r[i]) = (0.0, 0.0);
                        self.stats.removed += 1;
                    } else {
                        left += 1;
                    }
                }
            }
            self.fading = left;
        }
        let clear2 = (CLEAR * c.crowd).powi(2);
        let mut suspects: Vec<usize> = Vec::new();
        // No more exact sums a step than cost a small part of it.
        let most = (SUM_BUDGET / n).clamp(1, 64);
        for _ in 0..n.div_ceil(ROUND).min(4096) {
            self.dice ^= self.dice << 13;
            self.dice ^= self.dice >> 7;
            self.dice ^= self.dice << 17;
            let i = (self.dice % n as u64) as usize;
            if b.m[i] <= 0.0 || b.fade[i] > 0.0 {
                continue;
            }
            let (dx, dy, vx, vy) = (b.x[i] - c.x, b.y[i] - c.y, b.vx[i] - c.vx, b.vy[i] - c.vy);
            let d2 = dx * dx + dy * dy;
            // Half the energy a point holding all the mass would ask for: lumps nearby can
            // only hold a body tighter than that, which the exact sum then finds.
            if d2 > clear2 && dx * vx + dy * vy > 0.0 && (vx * vx + vy * vy) * d2.sqrt() > self.g * c.mass && !suspects.contains(&i) {
                suspects.push(i);
                if suspects.len() == most {
                    break;
                }
            }
        }
        let eps2 = self.softening * self.softening;
        for i in suspects {
            let (x, y) = (b.x[i], b.y[i]);
            let well: f64 = (0..n)
                .into_par_iter()
                .with_min_len(GRAIN)
                .map(|j| {
                    let d2 = (b.x[j] - x).powi(2) + (b.y[j] - y).powi(2) + eps2;
                    if j != i && d2 > 0.0 { b.m[j] as f64 / d2.sqrt() } else { 0.0 }
                })
                .sum();
            let v2 = (b.vx[i] - c.vx).powi(2) + (b.vy[i] - c.vy).powi(2);
            if 0.5 * v2 > self.g * well {
                b.fade[i] = f32::MIN_POSITIVE;
                self.fading += 1;
            }
        }
    }

    /// Centre of mass of the world and its velocity, as of the last force pass.
    pub fn barycentre(&self) -> (f64, f64, f64, f64) {
        self.nodes.first().map_or((0.0, 0.0, 0.0, 0.0), |n| (n.cx, n.cy, n.vx, n.vy))
    }

    /// Half-width of the world as of the last sort.
    pub fn extent(&self) -> f64 {
        self.len_unit
    }

    /// Root-mean-square error of the stored accelerations relative to the typical one,
    /// measured on `samples` bodies against exact double-precision sums.
    pub fn force_error(&self, b: &Bodies, samples: usize) -> f64 {
        let n = b.len();
        if n < 2 {
            return 0.0;
        }
        let eps2 = self.softening * self.softening;
        // What the ties do belongs to a body's acceleration too.
        let mut tied = vec![(0.0f64, 0.0f64); if self.ties.is_empty() { 0 } else { n }];
        for tie in &self.ties {
            let Some(tug) = self.tug(b, tie, 0.0) else { continue };
            let at = &mut tied[tie.i as usize];
            *at = (at.0 + tug.ax, at.1 + tug.ay);
            if let (true, With::Body(j)) = (tie.both, tie.with) {
                let at = &mut tied[j as usize];
                *at = (at.0 - tug.ax * tug.back, at.1 - tug.ay * tug.back);
            }
        }
        let stride = (n / samples.max(1)).max(1);
        let (err, norm) = (0..n)
            .into_par_iter()
            .step_by(stride)
            .filter(|i| b.alive(*i))
            .map(|i| {
                let (mut sx, mut sy) = (0.0, 0.0);
                for j in 0..n {
                    let (dx, dy) = (b.x[j] - b.x[i], b.y[j] - b.y[i]);
                    let d2 = dx * dx + dy * dy + eps2;
                    let f = self.g * b.m[j] as f64 / (d2 * d2.sqrt());
                    sx += dx * f;
                    sy += dy * f;
                }
                let own = tied.get(i).copied().unwrap_or((0.0, 0.0));
                ((b.ax[i] as f64 + own.0 - sx).powi(2) + (b.ay[i] as f64 + own.1 - sy).powi(2), sx * sx + sy * sy)
            })
            .reduce(|| (0.0, 0.0), |a, b| (a.0 + b.0, a.1 + b.1));
        if norm > 0.0 { (err / norm).sqrt() } else { 0.0 }
    }
}

//! The integrator: a Barnes-Hut style tree over bodies kept sorted along a Z-order curve.
//!
//! One step is three phases, split so that a renderer can read the bodies while the
//! expensive middle one runs:
//! 1. [`Engine::advance`] (writes bodies): half kick, drift, re-sort, rebuild the tree shape;
//! 2. [`Engine::forces`] (reads bodies): refit the tree, gather each group's interaction
//!    lists, evaluate them with the vector kernels, note overlapping pairs;
//! 3. [`Engine::finish`] (writes bodies): second half kick, merge the overlapping pairs.
//!
//! Positions and velocities are double precision. Forces are computed in single precision
//! on coordinates relative to each group's centre and scaled to the size of the world, which
//! keeps them accurate anywhere and within range of an `f32`.

use crate::kernel::{self, Far, Level, Near, PAD, PAD_X};
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
        self.m.push(mass as f32);
        self.r.push(radius as f32);
        self.group.push(group);
        self.fade.push(0.0);
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
    /// Traceless quadrupole about the centre of mass, in scaled units.
    qxx: f64,
    qxy: f64,
    qyy: f64,
}

/// What the tree walk looks at, in single precision relative to the world's corner and
/// scaled to its size: a quarter of the memory of a full node.
#[derive(Clone, Copy)]
struct Walk {
    cx: f32,
    cy: f32,
    x0: f32,
    x1: f32,
    y0: f32,
    y1: f32,
    size2: f32,
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

/// Per-thread buffers of the force pass.
#[derive(Default)]
struct Lists {
    /// Group targets relative to the group centre.
    tx: Vec<f32>,
    ty: Vec<f32>,
    tr: Vec<f32>,
    ax: Vec<f32>,
    ay: Vec<f32>,
    near: [Vec<f32>; 4],
    far: [Vec<f32>; 6],
    /// Body index of each near entry.
    ids: Vec<u32>,
    hit: Vec<u32>,
    close: Vec<u32>,
    stack: Vec<u32>,
}

/// A step may be this fraction of the time the fastest bound orbit takes to turn one radian
/// (about 30 steps per revolution).
pub const ORBIT_FRACTION: f64 = 0.2;
/// Up to this many bodies every pair is summed: no approximation, and cheaper than a tree.
pub const PAIRWISE_MAX: usize = 4000;
/// In tree mode, the share of the world's mass a pair (or a distant cell) must hold for its
/// orbital rate to limit the step.
const SUBSTANTIAL: f32 = 1.0e-3;
/// Orbits are looked for down to those that would only limit a step this many times longer
/// than the one asked for.
pub(crate) const HINT_MARGIN: f64 = 4.0;
/// Up to this many, in double precision with a fourth-order integrator as well.
pub const PRECISE_MAX: usize = 192;

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
    pub force_ms: f32,
    pub finish_ms: f32,
    /// Sources evaluated per body in the last force pass.
    pub interactions: f32,
    pub merges: u32,
    pub removed: u32,
}

/// A body counts as clear of the crowd beyond this many times the radius that holds
/// [`CROWD`] of the mass.
pub const CLEAR: f64 = 1.5;
pub const CROWD: f64 = 0.9;
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
    /// How much of its fade a leaving body goes through in one step.
    pub fade_step: f32,
    census: Census,
    /// Steps until the census is taken again.
    census_due: u32,
    fading: usize,
    dice: u64,
    mass_unit: f64,
    len_unit: f64,
    max_radius: f64,
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
    /// The step length the caller would like next: pairs bound tightly enough to need a
    /// shorter one are looked for during the force pass.
    pub dt_hint: f64,
    /// Square of the fastest orbital rate (rad/s) found in the last force pass, among bound
    /// pairs that tight and the pull of distant cells.
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
            fade_step: 1.0 / 64.0,
            census: Census::default(),
            census_due: 0,
            fading: 0,
            dice: 0x9E37_79B9_7F4A_7C15,
            mass_unit: 1.0,
            len_unit: 1.0,
            max_radius: 0.0,
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
        if b.is_empty() {
            return;
        }
        self.mass_unit = b.total_mass().max(1.0);
        self.sort(b);
        self.forces(b);
        std::mem::swap(&mut b.ax, &mut self.acc_x);
        std::mem::swap(&mut b.ay, &mut self.acc_y);
        self.primed = true;
    }

    /// The bodies were changed from outside: accelerations must be worked out afresh.
    pub fn invalidate(&mut self) {
        self.primed = false;
        self.census_due = 0;
    }

    /// Whether a world of this size is stepped in one go by [`Engine::step`] alone (the
    /// three phases are for the tree and pairwise modes).
    pub fn precise(&self, b: &Bodies) -> bool {
        b.len() <= PRECISE_MAX
    }

    /// One whole step. See the module docs for running the phases under a lock instead.
    pub fn step(&mut self, b: &mut Bodies, dt: f64) -> Vec<Merge> {
        if self.precise(b) {
            let t0 = std::time::Instant::now();
            let (merges, omega2) = crate::exact::step(b, dt, self.g, self.softening, self.dt_hint);
            (self.omega2, self.mode, self.primed) = (omega2, Mode::Precise, true);
            self.steps += 1;
            self.stats = StepStats { force_ms: ms(t0), interactions: b.len() as f32, merges: merges.len() as u32, ..Default::default() };
            return merges;
        }
        self.advance(b, dt);
        self.forces(b);
        self.finish(b, dt)
    }

    /// Phase 1: half kick, drift, sort. Afterwards positions are those of the next tick.
    pub fn advance(&mut self, b: &mut Bodies, dt: f64) {
        if !self.primed {
            self.prime(b);
        }
        if b.is_empty() {
            return;
        }
        let t0 = std::time::Instant::now();
        let half = 0.5 * dt;
        let kick_drift = |v: &mut Vec<f64>, x: &mut Vec<f64>, a: &Vec<f32>| {
            v.par_iter_mut().zip(x.par_iter_mut()).zip(a.par_iter()).with_min_len(GRAIN).for_each(|((v, x), a)| {
                *v += *a as f64 * half;
                *x += *v * dt;
            });
        };
        kick_drift(&mut b.vx, &mut b.x, &b.ax);
        kick_drift(&mut b.vy, &mut b.y, &b.ay);
        self.sort(b);
        self.stats.sort_ms = ms(t0);
    }

    /// Reorder the bodies along the Z-order curve, dropping dead ones, and rebuild the tree.
    fn sort(&mut self, b: &mut Bodies) {
        let n = b.len();
        let span = |v: &Vec<f64>| {
            v.par_iter().with_min_len(GRAIN).fold(|| (f64::MAX, f64::MIN), |a, v| (a.0.min(*v), a.1.max(*v))).reduce(|| (f64::MAX, f64::MIN), |a, b| (a.0.min(b.0), a.1.max(b.1)))
        };
        let ((min_x, max_x), (min_y, max_y)) = (span(&b.x), span(&b.y));
        self.len_unit = (max_x - min_x).max(max_y - min_y).max(1.0);
        self.origin = (min_x, min_y);
        let scale = 65534.0 / self.len_unit;
        self.keys.resize(n, 0);
        let (x, y, m) = (&b.x, &b.y, &b.m);
        let dead = m.par_iter().with_min_len(GRAIN).filter(|m| **m <= 0.0).count();
        self.keys.par_iter_mut().enumerate().with_min_len(GRAIN).for_each(|(i, k)| {
            // The dead sort past every live body and are cut off below.
            let cell = if m[i] > 0.0 {
                let qx = ((x[i] - min_x) * scale) as u32;
                let qy = ((y[i] - min_y) * scale) as u32;
                spread(qx) << 1 | spread(qy)
            } else {
                u32::MAX
            };
            *k = ((cell as u64) << 32) | i as u64;
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
        let fitted: Vec<(Node, f32)> = self
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
                node = Node { cx, cy, vx: svx * inv, vy: svy * inv, mass: m, min_x: x0, max_x: x1, min_y: y0, max_y: y1, qxx, qxy, qyy, ..node };
                (node, rmax)
            })
            .collect();
        let mut rmax = 0.0f32;
        for (k, (node, r)) in fitted.into_iter().enumerate() {
            self.nodes[self.leaves[k] as usize] = node;
            rmax = rmax.max(r);
        }
        self.max_radius = rmax as f64;
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
            (node.qxx, node.qxy, node.qyy) = (qxx, qxy, qyy);
        }
        let (ox, oy) = self.origin;
        let nodes = &self.nodes;
        self.walk.clear();
        self.walk.par_extend(nodes.par_iter().map(|n| {
            let f = |v: f64, o: f64| ((v - o) * inv_lu) as f32;
            let size = (n.size() * inv_lu) as f32;
            Walk { cx: f(n.cx, ox), cy: f(n.cy, oy), x0: f(n.min_x, ox), x1: f(n.max_x, ox), y0: f(n.min_y, oy), y1: f(n.max_y, oy), size2: size * size, left: n.left }
        }));
        self.moments.clear();
        self.moments.par_extend(nodes.par_iter().map(|n| [n.mass as f32, n.qxx as f32, n.qxy as f32, n.qyy as f32]));
    }

    /// Phase 2: accelerations at the current positions, and which bodies overlap.
    pub fn forces(&mut self, b: &Bodies) {
        if b.is_empty() {
            self.pairs.clear();
            self.omega2 = 0.0;
            return;
        }
        let t0 = std::time::Instant::now();
        self.refit(b);
        let n = b.len();
        self.acc_x.clear();
        self.acc_x.resize(n, 0.0);
        self.acc_y.clear();
        self.acc_y.resize(n, 0.0);
        let inv_lu = 1.0 / self.len_unit;
        let inv_mu = (1.0 / self.mass_unit) as f32;
        let inv_lu32 = inv_lu as f32;
        // Softening in scaled units; the floor keeps 1/r^5 inside single precision.
        let eps = (self.softening * inv_lu).max(3.0e-7) as f32;
        let eps2 = eps * eps;
        // A zero opening angle opens every cell: the sum over every pair, through the same
        // code, with each group still measuring from its own centre.
        self.mode = if n <= PAIRWISE_MAX { Mode::Pairwise } else { Mode::Tree };
        let theta = if self.mode == Mode::Pairwise { 0.0 } else { self.theta };
        let theta2 = theta * theta;
        // m / d^3 above which a neighbour may be turning a body faster than the wanted step
        // resolves (in the kernels' scaled units).
        let rate_unit = self.g * self.mass_unit * inv_lu * inv_lu * inv_lu;
        // With a margin, so that a pace that is being raised does not outrun what is known.
        let tight = if self.dt_hint > 0.0 { ((ORBIT_FRACTION / (HINT_MARGIN * self.dt_hint)).powi(2) / rate_unit) as f32 } else { f32::MAX };
        let g = self.g;
        // In a crowd, only orbits around something substantial are worth slowing the whole
        // world down for: chance pairings of small bodies are left to fend for themselves.
        let heavy = if self.mode == Mode::Tree { SUBSTANTIAL } else { 0.0 };
        let heavy_kg = heavy as f64 * self.mass_unit;
        // No body of a cell further than this from a group's box can touch a body in it.
        // In the walk's scaled single precision, with slack for its rounding.
        let clear = (2.0 * self.max_radius * inv_lu + 2.0e-6) as f32;
        let clear2 = clear * clear;
        let to_si = (self.g * self.mass_unit * inv_lu * inv_lu) as f32;
        let (nodes, walk, moments, origin, level) = (&self.nodes, &self.walk, &self.moments, self.origin, self.level);
        let (out_x, out_y) = (Shared(self.acc_x.as_mut_ptr()), Shared(self.acc_y.as_mut_ptr()));
        let (count, mut pairs, omega2) = self
            .leaves
            .par_iter()
            .map_init(Lists::default, |l, &at| {
                let (out_x, out_y) = (&out_x, &out_y);
                let group = &nodes[at as usize];
                let (lo, hi) = (group.lo as usize, group.hi as usize);
                let (ox, oy) = (0.5 * (group.min_x + group.max_x), 0.5 * (group.min_y + group.max_y));
                for v in l.near.iter_mut().chain(l.far.iter_mut()) {
                    v.clear();
                }
                l.ids.clear();
                l.hit.clear();
                l.close.clear();
                l.stack.clear();
                l.stack.push(0);
                let me = walk[at as usize];
                // The strongest m / d^3 among the distant cells heavy enough to matter.
                let mut far_rate = 0.0f32;
                // The group's centre in the walk's coordinates.
                let (gx0, gy0) = (((ox - origin.0) * inv_lu) as f32, ((oy - origin.1) * inv_lu) as f32);
                while let Some(i) = l.stack.pop() {
                    let w = &walk[i as usize];
                    // Distance from the cell's centre of mass to the nearest point of the group.
                    let dx = (me.x0 - w.cx).max(w.cx - me.x1).max(0.0);
                    let dy = (me.y0 - w.cy).max(w.cy - me.y1).max(0.0);
                    let gx = (me.x0 - w.x1).max(w.x0 - me.x1).max(0.0);
                    let gy = (me.y0 - w.y1).max(w.y0 - me.y1).max(0.0);
                    if w.size2 < theta2 * (dx * dx + dy * dy) && gx * gx + gy * gy > clear2 {
                        // Single precision is plenty for where a distant cell is.
                        let m = &moments[i as usize];
                        let d2 = dx * dx + dy * dy;
                        if m[0] >= heavy {
                            far_rate = far_rate.max(m[0] / (d2 * d2.sqrt()));
                        }
                        for (list, v) in l.far.iter_mut().zip([w.cx - gx0, w.cy - gy0, m[0], m[1], m[2], m[3]]) {
                            list.push(v);
                        }
                    } else if w.left == 0 {
                        let node = &nodes[i as usize];
                        let range = node.lo as usize..node.hi as usize;
                        let (from, to) = (l.ids.len(), l.ids.len() + range.len());
                        for list in l.near.iter_mut() {
                            list.resize(to, 0.0);
                        }
                        let [nx, ny, nm, nr] = &mut l.near;
                        for (o, x) in nx[from..].iter_mut().zip(&b.x[range.clone()]) {
                            *o = ((x - ox) * inv_lu) as f32;
                        }
                        for (o, y) in ny[from..].iter_mut().zip(&b.y[range.clone()]) {
                            *o = ((y - oy) * inv_lu) as f32;
                        }
                        for (o, m) in nm[from..].iter_mut().zip(&b.m[range.clone()]) {
                            *o = m * inv_mu;
                        }
                        for (o, r) in nr[from..].iter_mut().zip(&b.r[range]) {
                            *o = r * inv_lu32;
                        }
                        l.ids.extend(node.lo..node.hi);
                    } else {
                        l.stack.push(w.left);
                        l.stack.push(w.left + 1);
                    }
                }
                let sources = l.near[0].len() + l.far[0].len();
                for (k, list) in l.near.iter_mut().chain(l.far.iter_mut()).enumerate() {
                    // Only x is moved out of the way; mass, radius and the rest are zero.
                    let fill = if k == 0 || k == 4 { PAD_X } else { 0.0 };
                    list.resize(list.len().div_ceil(PAD) * PAD, fill);
                }
                let count = hi - lo;
                l.tx.clear();
                l.ty.clear();
                l.tr.clear();
                for i in lo..hi {
                    l.tx.push(((b.x[i] - ox) * inv_lu) as f32);
                    l.ty.push(((b.y[i] - oy) * inv_lu) as f32);
                    l.tr.push(b.r[i] * inv_lu32);
                }
                l.ax.clear();
                l.ax.resize(count, 0.0);
                l.ay.clear();
                l.ay.resize(count, 0.0);
                let near = Near { x: &l.near[0], y: &l.near[1], m: &l.near[2], r: &l.near[3] };
                kernel::near(level, &l.tx, &l.ty, &l.tr, &mut l.ax, &mut l.ay, &near, eps2, tight, &mut l.hit, &mut l.close);
                let mut omega2 = far_rate as f64 * rate_unit;
                if !l.far[0].is_empty() {
                    let far = Far { x: &l.far[0], y: &l.far[1], m: &l.far[2], qxx: &l.far[3], qxy: &l.far[4], qyy: &l.far[5] };
                    kernel::far(level, &l.tx, &l.ty, &mut l.ax, &mut l.ay, &far, eps2);
                }
                // SAFETY: groups cover disjoint ranges of bodies.
                let (ax, ay) = unsafe { (std::slice::from_raw_parts_mut(out_x.0.add(lo), count), std::slice::from_raw_parts_mut(out_y.0.add(lo), count)) };
                for k in 0..count {
                    ax[k] = l.ax[k] * to_si;
                    ay[k] = l.ay[k] * to_si;
                }
                // Rare: find out who exactly the flagged bodies touch, in full precision.
                let mut pairs = Vec::new();
                for &t in &l.hit {
                    let a = lo + t as usize;
                    for &j in &l.ids {
                        let j = j as usize;
                        if a != j && b.m[a] > 0.0 && b.m[j] > 0.0 {
                            let (dx, dy, reach) = (b.x[j] - b.x[a], b.y[j] - b.y[a], (b.r[a] + b.r[j]) as f64);
                            if dx * dx + dy * dy < reach * reach {
                                pairs.push((a.min(j) as u32, a.max(j) as u32));
                            }
                        }
                    }
                }
                // Also rare: of the neighbours that pull a body hard, the ones it is bound to
                // (a passing stranger does not need the step shortened for it).
                for &t in &l.close {
                    let a = lo + t as usize;
                    for &j in &l.ids {
                        let j = j as usize;
                        if a == j || b.m[j] <= 0.0 || ((b.m[a] + b.m[j]) as f64) < heavy_kg {
                            continue;
                        }
                        let (dx, dy) = (b.x[j] - b.x[a], b.y[j] - b.y[a]);
                        let d = (dx * dx + dy * dy).sqrt().max((b.r[a] + b.r[j]) as f64);
                        let pull = g * (b.m[a] + b.m[j]) as f64 / d;
                        let speed2 = (b.vx[j] - b.vx[a]).powi(2) + (b.vy[j] - b.vy[a]).powi(2);
                        if speed2 < 2.0 * pull {
                            omega2 = omega2.max(pull / (d * d));
                        }
                    }
                }
                (sources * count, pairs, omega2)
            })
            .reduce(
                || (0, Vec::new(), 0.0),
                |mut a, mut b| {
                    a.0 += b.0;
                    a.1.append(&mut b.1);
                    a.2 = a.2.max(b.2);
                    a
                },
            );
        self.omega2 = omega2;
        // A pair is seen from the flagged side only; the partner may have been flagged too.
        pairs.par_sort_unstable();
        pairs.dedup();
        self.pairs = pairs;
        self.stats.interactions = count as f32 / n.max(1) as f32;
        self.stats.force_ms = ms(t0);
    }

    /// Phase 3: second half kick, then merge what overlaps.
    pub fn finish(&mut self, b: &mut Bodies, dt: f64) -> Vec<Merge> {
        if b.is_empty() {
            return Vec::new();
        }
        let t0 = std::time::Instant::now();
        std::mem::swap(&mut b.ax, &mut self.acc_x);
        std::mem::swap(&mut b.ay, &mut self.acc_y);
        let half = 0.5 * dt;
        b.vx.par_iter_mut().zip(b.ax.par_iter()).with_min_len(GRAIN).for_each(|(v, a)| *v += *a as f64 * half);
        b.vy.par_iter_mut().zip(b.ay.par_iter()).with_min_len(GRAIN).for_each(|(v, a)| *v += *a as f64 * half);
        let mut merges = Vec::new();
        for &(i, j) in &self.pairs {
            let (i, j) = (i as usize, j as usize);
            if b.m[i] <= 0.0 || b.m[j] <= 0.0 {
                continue; // one of them already merged this step; the rest follows next step
            }
            // Mass and momentum are kept, volumes add up; the heavier body lives on.
            let (s, a) = if b.m[j] > b.m[i] { (j, i) } else { (i, j) };
            let (ms, ma) = (b.m[s] as f64, b.m[a] as f64);
            let inv = 1.0 / (ms + ma);
            b.x[s] = (b.x[s] * ms + b.x[a] * ma) * inv;
            b.y[s] = (b.y[s] * ms + b.y[a] * ma) * inv;
            b.vx[s] = (b.vx[s] * ms + b.vx[a] * ma) * inv;
            b.vy[s] = (b.vy[s] * ms + b.vy[a] * ma) * inv;
            // The pull of the two on each other cancels in the weighted mean, leaving what the
            // rest of the world does to the pair.
            b.ax[s] = ((b.ax[s] as f64 * ms + b.ax[a] as f64 * ma) * inv) as f32;
            b.ay[s] = ((b.ay[s] as f64 * ms + b.ay[a] as f64 * ma) * inv) as f32;
            b.r[s] = ((b.r[s] as f64).powi(3) + (b.r[a] as f64).powi(3)).cbrt() as f32;
            b.m[s] = (ms + ma) as f32;
            merges.push(Merge { x: b.x[s], y: b.y[s], vx: b.vx[s], vy: b.vy[s], mass: b.m[a], survivor: b.id[s], absorbed: b.id[a] });
            b.m[a] = 0.0;
            b.r[a] = 0.0;
        }
        self.steps += 1;
        self.stats.merges = merges.len() as u32;
        self.stats.finish_ms = ms(t0);
        merges
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
        let eps2 = (self.softening.max(3.0e-7 * self.len_unit)).powi(2);
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
                ((b.ax[i] as f64 - sx).powi(2) + (b.ay[i] as f64 - sy).powi(2), sx * sx + sy * sy)
            })
            .reduce(|| (0.0, 0.0), |a, b| (a.0 + b.0, a.1 + b.1));
        if norm > 0.0 { (err / norm).sqrt() } else { 0.0 }
    }
}

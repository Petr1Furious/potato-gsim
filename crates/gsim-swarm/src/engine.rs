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
    /// The tick these positions belong to (maintained by whoever steps the engine).
    pub tick: u64,
    /// Seconds the step that led to `tick` covered: bodies moved `v * dt` to get here.
    pub dt: f64,
}

impl Bodies {
    pub fn len(&self) -> usize {
        self.x.len()
    }

    pub fn is_empty(&self) -> bool {
        self.x.is_empty()
    }

    pub fn push(&mut self, x: f64, y: f64, vx: f64, vy: f64, mass: f64, radius: f64, group: u8) {
        self.id.push(self.x.len() as u32);
        self.x.push(x);
        self.y.push(y);
        self.vx.push(vx);
        self.vy.push(vy);
        self.ax.push(0.0);
        self.ay.push(0.0);
        self.m.push(mass as f32);
        self.r.push(radius as f32);
        self.group.push(group);
    }

    pub fn alive(&self, i: usize) -> bool {
        self.m[i] > 0.0
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

    /// Squared distance from a box to this node's box (0 if they overlap).
    fn gap2(&self, x0: f64, x1: f64, y0: f64, y1: f64) -> f64 {
        let dx = (x0 - self.max_x).max(self.min_x - x1).max(0.0);
        let dy = (y0 - self.max_y).max(self.min_y - y1).max(0.0);
        dx * dx + dy * dy
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

/// Everything that acts on one point: nearby bodies one by one and the rest of the world as
/// a few hundred lumps. Small enough to integrate a ship against, or to simulate ahead.
#[derive(Clone, Default)]
pub struct Local {
    pub x: Vec<f64>,
    pub y: Vec<f64>,
    pub vx: Vec<f64>,
    pub vy: Vec<f64>,
    pub ax: Vec<f64>,
    pub ay: Vec<f64>,
    pub mass: Vec<f64>,
    /// 0 for lumps: only real bodies can be hit.
    pub radius: Vec<f64>,
    /// Body id, or `u32::MAX` for a lump.
    pub id: Vec<u32>,
}

impl Local {
    pub fn len(&self) -> usize {
        self.x.len()
    }

    pub fn is_empty(&self) -> bool {
        self.x.is_empty()
    }

    fn clear(&mut self) {
        for v in [&mut self.x, &mut self.y, &mut self.vx, &mut self.vy, &mut self.ax, &mut self.ay, &mut self.mass, &mut self.radius] {
            v.clear();
        }
        self.id.clear();
    }

    #[allow(clippy::too_many_arguments)]
    fn push(&mut self, x: f64, y: f64, vx: f64, vy: f64, ax: f64, ay: f64, mass: f64, radius: f64, id: u32) {
        self.x.push(x);
        self.y.push(y);
        self.vx.push(vx);
        self.vy.push(vy);
        self.ax.push(ax);
        self.ay.push(ay);
        self.mass.push(mass);
        self.radius.push(radius);
        self.id.push(id);
    }

    pub fn slot_of(&self, id: u32) -> Option<usize> {
        self.id.iter().position(|v| *v == id)
    }

    /// Gravitational acceleration at a point (softened like the engine's).
    pub fn accel_at(&self, px: f64, py: f64, g: f64, softening: f64) -> (f64, f64) {
        let (mut ax, mut ay) = (0.0, 0.0);
        for j in 0..self.len() {
            let (dx, dy) = (self.x[j] - px, self.y[j] - py);
            let d2 = dx * dx + dy * dy + softening * softening;
            let f = g * self.mass[j] / (d2 * d2.sqrt());
            ax += dx * f;
            ay += dy * f;
        }
        (ax, ay)
    }
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
    stack: Vec<u32>,
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

pub struct Engine {
    level: Level,
    /// Largest number of bodies in a group.
    pub leaf: usize,
    /// Opening angle: a cell is one lump when it looks smaller than this from the group.
    pub theta: f32,
    pub g: f64,
    /// Plummer softening (m).
    pub softening: f64,
    /// Bodies beyond this distance from the centre of mass that are moving outwards are
    /// dropped (0 = never).
    pub escape_radius: f64,
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
            escape_radius: 0.0,
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
            steps: 0,
            stats: StepStats::default(),
        }
    }

    pub fn level(&self) -> Level {
        self.level
    }

    /// Compute accelerations for a freshly filled or edited set of bodies.
    pub fn prime(&mut self, b: &mut Bodies) {
        self.mass_unit = b.total_mass().max(1.0);
        self.sort(b);
        self.forces(b);
        std::mem::swap(&mut b.ax, &mut self.acc_x);
        std::mem::swap(&mut b.ay, &mut self.acc_y);
        self.primed = true;
    }

    /// One whole step. See the module docs for running the phases under a lock instead.
    pub fn step(&mut self, b: &mut Bodies, dt: f64) -> Vec<Merge> {
        self.advance(b, dt);
        self.forces(b);
        self.finish(b, dt)
    }

    /// Phase 1: half kick, drift, sort. Afterwards positions are those of the next tick.
    pub fn advance(&mut self, b: &mut Bodies, dt: f64) {
        if !self.primed {
            self.prime(b);
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
        for v in [&mut b.m, &mut b.r] {
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
        let theta2 = self.theta * self.theta;
        // No body of a cell further than this from a group's box can touch a body in it.
        // In the walk's scaled single precision, with slack for its rounding.
        let clear = (2.0 * self.max_radius * inv_lu + 2.0e-6) as f32;
        let clear2 = clear * clear;
        let to_si = (self.g * self.mass_unit * inv_lu * inv_lu) as f32;
        let (nodes, walk, moments, origin, level) = (&self.nodes, &self.walk, &self.moments, self.origin, self.level);
        let (out_x, out_y) = (Shared(self.acc_x.as_mut_ptr()), Shared(self.acc_y.as_mut_ptr()));
        let (count, mut pairs) = self
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
                l.stack.clear();
                l.stack.push(0);
                let me = walk[at as usize];
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
                kernel::near(level, &l.tx, &l.ty, &l.tr, &mut l.ax, &mut l.ay, &near, eps2, &mut l.hit);
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
                (sources * count, pairs)
            })
            .reduce(
                || (0, Vec::new()),
                |mut a, mut b| {
                    a.0 += b.0;
                    a.1.append(&mut b.1);
                    a
                },
            );
        // A pair is seen from the flagged side only; the partner may have been flagged too.
        pairs.par_sort_unstable();
        pairs.dedup();
        self.pairs = pairs;
        self.stats.interactions = count as f32 / n.max(1) as f32;
        self.stats.force_ms = ms(t0);
    }

    /// Phase 3: second half kick, then merge what overlaps.
    pub fn finish(&mut self, b: &mut Bodies, dt: f64) -> Vec<Merge> {
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
        let mut removed = 0;
        if self.escape_radius > 0.0 && self.steps.is_multiple_of(64) {
            let root = self.nodes[0];
            let limit2 = self.escape_radius * self.escape_radius;
            removed = (0..b.len())
                .into_par_iter()
                .filter(|&i| {
                    let (dx, dy) = (b.x[i] - root.cx, b.y[i] - root.cy);
                    b.m[i] > 0.0 && dx * dx + dy * dy > limit2 && dx * (b.vx[i] - root.vx) + dy * (b.vy[i] - root.vy) > 0.0
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|i| {
                    b.m[i] = 0.0;
                    b.r[i] = 0.0;
                })
                .count() as u32;
        }
        self.stats.merges = merges.len() as u32;
        self.stats.removed = removed;
        self.stats.finish_ms = ms(t0);
        merges
    }

    /// Centre of mass of the world and its velocity, as of the last force pass.
    pub fn barycentre(&self) -> (f64, f64, f64, f64) {
        self.nodes.first().map_or((0.0, 0.0, 0.0, 0.0), |n| (n.cx, n.cy, n.vx, n.vy))
    }

    /// Half-width of the world as of the last sort.
    pub fn extent(&self) -> f64 {
        self.len_unit
    }

    /// What acts on the point `(px, py)`: bodies within `near` metres (and those in cells
    /// that look large from there) one by one, everything else as lumps. Bodies listed in
    /// `always` (by id) are included individually wherever they are.
    ///
    /// Valid between steps, when the tree matches the bodies.
    pub fn local(&self, b: &Bodies, px: f64, py: f64, theta: f64, near: f64, always: &[u32], out: &mut Local) {
        out.clear();
        if self.nodes.is_empty() || !self.primed {
            return;
        }
        let wanted: Vec<usize> = always.iter().filter_map(|id| b.locate(*id)).collect();
        let (theta2, near2) = (theta * theta, near * near);
        let mut stack = vec![0u32];
        while let Some(i) = stack.pop() {
            let node = &self.nodes[i as usize];
            if node.mass <= 0.0 {
                continue;
            }
            let (dx, dy) = (node.cx - px, node.cy - py);
            let s = node.size();
            let holds_wanted = wanted.iter().any(|w| (node.lo as usize..node.hi as usize).contains(w));
            if !holds_wanted && s * s < theta2 * (dx * dx + dy * dy) && node.gap2(px, px, py, py) > near2 {
                out.push(node.cx, node.cy, node.vx, node.vy, 0.0, 0.0, node.mass * self.mass_unit, 0.0, u32::MAX);
            } else if node.left == 0 {
                for j in (node.lo as usize..node.hi as usize).filter(|j| b.alive(*j)) {
                    out.push(b.x[j], b.y[j], b.vx[j], b.vy[j], b.ax[j] as f64, b.ay[j] as f64, b.m[j] as f64, b.r[j] as f64, b.id[j]);
                }
            } else {
                stack.push(node.left);
                stack.push(node.right);
            }
        }
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

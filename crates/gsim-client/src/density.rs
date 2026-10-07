//! Software rasteriser for worlds with too many bodies to draw one by one: every body adds
//! light to a floating-point image, which is then given a soft glow, tone-mapped and shown
//! as one texture. Bodies large enough on screen to have a shape are returned instead, to be
//! drawn properly on top.

use crate::game::View;
use crate::gpu::{Gpu, Vertex};
use crate::style;
use gsim_swarm::Bodies;
use macroquad::prelude::*;
use rayon::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColorMode {
    Mass,
    Speed,
    Origin,
}

impl ColorMode {
    pub const ALL: [ColorMode; 3] = [ColorMode::Mass, ColorMode::Speed, ColorMode::Origin];

    pub fn name(self) -> &'static str {
        match self {
            ColorMode::Mass => "mass",
            ColorMode::Speed => "speed",
            ColorMode::Origin => "origin",
        }
    }
}

/// A body big enough on screen to be drawn as a disc.
#[derive(Clone, Copy)]
pub struct Big {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    pub r: f32,
    pub mass: f32,
    /// 1 here to stay .. 0 gone.
    pub fade: f32,
}

/// A brief light where two bodies merged, in buffer pixels.
pub struct Flash {
    pub x: f64,
    pub y: f64,
    /// 1 fresh .. 0 gone.
    pub life: f32,
    pub mass: f32,
}

const PALETTE: usize = 64;
/// Most bodies drawn individually; beyond that the smallest fall back to light.
const MAX_BIG: usize = 5000;
/// Bodies at least this many screen pixels in radius are drawn individually.
const BIG_PX: f32 = 1.6;
/// The image is rendered at reduced resolution above this many pixels.
const MAX_PIXELS: f32 = 1.6e6;
const GLOW_DIV: usize = 4;
const NOWHERE: u32 = u32::MAX;
/// How long lingering light takes to fade to about a third, and how bright one frame of it is
/// next to the body that left it.
const TRAIL_SECONDS: f32 = 2.5;
const TRAIL_STRENGTH: f32 = 0.12;

pub struct Density {
    w: usize,
    h: usize,
    /// Screen pixels per buffer pixel.
    scale: f32,
    acc: Vec<[f32; 4]>,
    glow: Vec<[f32; 4]>,
    blur: Vec<[f32; 4]>,
    /// Per buffer column: the glow column to its left and how far towards the next it lies.
    across: Vec<(u32, f32)>,
    /// From the last projection: palette, and the world position of the buffer's corner with
    /// buffer pixels per metre.
    colors: [[f32; 3]; PALETTE],
    corner: (f64, f64, f64),
    /// Per body: buffer pixel (`y << 16 | x`) or `NOWHERE`, and palette entry with brightness.
    pix: Vec<u32>,
    tint: Vec<(u8, f32)>,
    image: Image,
    texture: Option<Texture2D>,
    exposure: f32,
    pool: rayon::ThreadPool,
    gpu: Option<Gpu>,
    gpu_wanted: bool,
    /// A world of a few thousand bodies, where each one should be easy to see: dots are
    /// larger and brighter and glow more than in a crowd of hundreds of thousands.
    pub sparse: bool,
    /// Light lingers and fades slowly, so that moving bodies draw streaks.
    pub long_exposure: bool,
    /// The lingering light (rasteriser only; the graphics card keeps its own).
    trail: Vec<[f32; 4]>,
    trail_stale: bool,
    last_frame: std::time::Instant,
    /// Physical pixels across one body when the graphics card draws.
    dot: u32,
    notice: Option<String>,
    vertices: Vec<Vertex>,
    /// Light added up per pixel, for when many bodies share few pixels.
    merged: Vec<[f32; 3]>,
    /// Milliseconds the last frame took to rasterise.
    pub last_ms: f32,
}

fn palette(mode: ColorMode) -> [[f32; 3]; PALETTE] {
    let mut out = [[0.0; 3]; PALETTE];
    for (i, c) in out.iter_mut().enumerate() {
        let t = i as f32 / (PALETTE - 1) as f32;
        let color = match mode {
            // 1e19 kg .. 1e31 kg, with the body renderer's own colours.
            ColorMode::Mass => style::mass_color(10f64.powf(19.0 + 12.0 * t as f64)),
            ColorMode::Speed => {
                const STOPS: [(f32, f32, f32); 5] = [(0.25, 0.35, 0.9), (0.2, 0.8, 0.95), (0.6, 0.95, 0.5), (1.0, 0.8, 0.3), (1.0, 0.35, 0.3)];
                let x = t * (STOPS.len() - 1) as f32;
                let k = (x.floor() as usize).min(STOPS.len() - 2);
                let (a, b, f) = (STOPS[k], STOPS[k + 1], x - k as f32);
                Color::new(a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f, a.2 + (b.2 - a.2) * f, 1.0)
            }
            ColorMode::Origin => match i {
                0 => Color::new(1.0, 0.72, 0.42, 1.0),
                1 => Color::new(0.42, 0.74, 1.0, 1.0),
                2 => Color::new(0.6, 1.0, 0.6, 1.0),
                _ => Color::new(0.9, 0.6, 1.0, 1.0),
            },
        };
        *c = [color.r, color.g, color.b];
    }
    out
}

/// Threads that turn bodies into pixels. They get cores of their own (see
/// [`simulation_threads`]): sharing them with the simulation halves the frame rate.
pub fn raster_threads() -> usize {
    (std::thread::available_parallelism().map_or(4, |n| n.get()) / 3).clamp(2, 6)
}

/// Threads left for simulating once the picture and the main thread have theirs. With the
/// graphics card drawing, the picture needs one for a moment each frame instead of several.
pub fn simulation_threads(gpu: bool) -> usize {
    let all = std::thread::available_parallelism().map_or(4, |n| n.get());
    all.saturating_sub(if gpu { 1 } else { raster_threads() } + 1).max(2)
}

impl Density {
    pub fn new() -> Self {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(raster_threads()).thread_name(|i| format!("gsim-raster-{i}")).build().expect("raster threads");
        Self {
            w: 0,
            h: 0,
            scale: 1.0,
            acc: Vec::new(),
            glow: Vec::new(),
            blur: Vec::new(),
            across: Vec::new(),
            colors: [[0.0; 3]; PALETTE],
            corner: (0.0, 0.0, 1.0),
            pix: Vec::new(),
            tint: Vec::new(),
            image: Image::empty(),
            texture: None,
            exposure: 1.0,
            pool,
            gpu: None,
            gpu_wanted: false,
            sparse: false,
            long_exposure: false,
            trail: Vec::new(),
            trail_stale: false,
            last_frame: std::time::Instant::now(),
            dot: 1,
            notice: None,
            vertices: Vec::new(),
            merged: Vec::new(),
            last_ms: 0.0,
        }
    }

    fn resize(&mut self) {
        let (sw, sh) = (screen_width().max(16.0), screen_height().max(16.0));
        // On the graphics card every body is a square of a whole number of physical pixels,
        // on a grid of exactly that pitch. Anything else leaves hairline gaps and overlaps
        // between neighbours, which show as a faint lattice wherever bodies are dense.
        let dpi = screen_dpi_scale().max(0.25);
        self.dot = if self.gpu_wanted { dpi.round().max(1.0) as u32 } else { 1 };
        let scale = if self.gpu_wanted { self.dot as f32 / dpi } else { (sw * sh / MAX_PIXELS).sqrt().max(1.0) };
        let (w, h) = (((sw / scale).ceil() as usize).min(65_535), ((sh / scale).ceil() as usize).min(65_535));
        self.scale = scale;
        if (w, h) != (self.w, self.h) {
            (self.w, self.h) = (w, h);
            // The rasteriser's own images are made when it next draws.
            self.acc = Vec::new();
        }
    }

    /// Forget the light that has lingered so far: the view moved, and it would smear.
    pub fn clear_trail(&mut self) {
        self.trail_stale = true;
    }

    /// What share of lingering light survives this frame, and how strongly it is shown.
    fn linger(&mut self) -> Option<(f32, f32)> {
        let frame = self.last_frame.elapsed().as_secs_f32().min(0.1);
        self.last_frame = std::time::Instant::now();
        self.long_exposure.then(|| ((-frame / TRAIL_SECONDS).exp(), TRAIL_STRENGTH))
    }

    /// How bright a typical lit pixel should come out, before tone mapping.
    fn brightness(&self) -> f32 {
        if self.sparse { 3.0 } else { 0.5 }
    }

    /// How much stronger than usual the glow is.
    fn halo(&self) -> f32 {
        if self.sparse { 2.5 } else { 1.0 }
    }

    /// Images for drawing without the graphics card, sized for the current buffer.
    fn allocate(&mut self) {
        let (w, h) = (self.w, self.h);
        if self.acc.len() != w * h {
            self.acc = vec![[0.0; 4]; w * h];
            self.across = (0..w)
                .map(|x| {
                    let gx = ((x as f32 + 0.5) / GLOW_DIV as f32 - 0.5).max(0.0);
                    (gx as u32, gx.fract())
                })
                .collect();
            let (gw, gh) = (w.div_ceil(GLOW_DIV) + 1, h.div_ceil(GLOW_DIV) + 1);
            self.glow = vec![[0.0; 4]; gw * gh];
            self.blur = vec![[0.0; 4]; gw * gh];
            self.image = Image::gen_image_color(w as u16, h as u16, BLACK);
            self.texture = None;
        }
    }

    /// First half, and the only one that looks at the bodies (so the only one to do while
    /// holding them): find where each lands on screen as seen through `view`, moved by `tau`
    /// seconds along its velocity. Returns the bodies large enough to be drawn individually.
    pub fn project(&mut self, bodies: &Bodies, view: &View, tau: f64, mode: ColorMode) -> Vec<Big> {
        let started = std::time::Instant::now();
        self.resize();
        let (w, h, scale) = (self.w, self.h, self.scale);
        let n = bodies.len();
        self.pix.resize(n, NOWHERE);
        self.tint.resize(n, (0, 0.0));
        let colors = palette(mode);
        // Buffer pixels per metre, and the world position of the buffer's corner.
        let ppm = 1.0 / (view.mpp * scale as f64);
        let (x0, y0) = (view.cx - 0.5 * screen_width() as f64 * view.mpp, view.cy - 0.5 * screen_height() as f64 * view.mpp);
        let big_r = BIG_PX * scale;
        let faintest = if self.sparse { 0.5 } else { 0.02 };
        let chunk = n.div_ceil(self.pool.current_num_threads() * 4).max(1024);
        let (pix, tint) = (&mut self.pix, &mut self.tint);
        let mut big: Vec<Big> = self.pool.install(|| {
            pix.par_chunks_mut(chunk)
                .zip(tint.par_chunks_mut(chunk))
                .enumerate()
                .map(|(c, (pix, tint))| {
                    let mut big = Vec::new();
                    let base = c * chunk;
                    for (k, (p, t)) in pix.iter_mut().zip(tint.iter_mut()).enumerate() {
                        let i = base + k;
                        *p = NOWHERE;
                        let bx = ((bodies.x[i] + bodies.vx[i] * tau - x0) * ppm) as f32;
                        let by = ((bodies.y[i] + bodies.vy[i] * tau - y0) * ppm) as f32;
                        let mass = bodies.m[i];
                        // Radius in buffer pixels.
                        let r = bodies.r[i] * ppm as f32;
                        if mass <= 0.0 || !(bx > -r && by > -r && bx < w as f32 + r && by < h as f32 + r) {
                            continue;
                        }
                        if r * scale >= big_r {
                            big.push(Big { id: bodies.id[i], x: bx * scale, y: by * scale, r: r * scale, mass, fade: 1.0 - bodies.fade[i] });
                            continue;
                        }
                        if bx < 0.0 || by < 0.0 || bx >= w as f32 || by >= h as f32 {
                            continue;
                        }
                        *p = (by as u32) << 16 | bx as u32;
                        let shade = match mode {
                            ColorMode::Mass => ((mass.log10() - 19.0) / 12.0).clamp(0.0, 1.0),
                            ColorMode::Speed => {
                                let v = ((bodies.vx[i] * bodies.vx[i] + bodies.vy[i] * bodies.vy[i]) as f32).sqrt();
                                ((v.max(1.0).log10() - 3.3) / 1.5).clamp(0.0, 1.0)
                            }
                            ColorMode::Origin => bodies.group[i].min(3) as f32 / (PALETTE - 1) as f32,
                        };
                        // A dot's light grows with its area until it fills a pixel; heavier
                        // bodies shine a little more so they stand out when zoomed far out.
                        let area = (r * r * std::f32::consts::PI).clamp(faintest, 1.0);
                        let weight = area.sqrt() * (0.35 + 0.65 * ((mass.log10() - 19.0) / 8.0).clamp(0.0, 1.0)) * (1.0 - bodies.fade[i]);
                        *t = ((shade * (PALETTE - 1) as f32 + 0.5) as u8, weight);
                    }
                    big
                })
                .reduce(Vec::new, |mut a, mut b| {
                    a.append(&mut b);
                    a
                })
        });
        if big.len() > MAX_BIG {
            big.sort_unstable_by(|a, b| b.r.total_cmp(&a.r));
            big.truncate(MAX_BIG);
        }

        self.colors = colors;
        self.corner = (x0, y0, ppm);
        self.last_ms = started.elapsed().as_secs_f32() * 1e3;
        big
    }

    /// Second half, which no longer needs the bodies: turn what [`Density::project`] found
    /// into light, add the glow, tone-map and draw the picture over the whole screen.
    pub fn present(&mut self, flashes: &[Flash]) {
        let started = std::time::Instant::now();
        if self.gpu_wanted && self.gpu.is_none() {
            match Gpu::new() {
                Ok(gpu) => self.gpu = Some(gpu),
                Err(why) => {
                    self.gpu_wanted = false;
                    self.notice = Some(why);
                }
            }
        }
        if self.gpu.is_some() {
            self.present_gpu(flashes);
            self.last_ms += started.elapsed().as_secs_f32() * 1e3;
            return;
        }
        self.allocate();
        let linger = self.linger();
        let (w, h, scale) = (self.w, self.h, self.scale);
        let (x0, y0, ppm) = self.corner;
        let colors = self.colors;
        // Each band of rows is owned by one task, which picks its bodies out of the list.
        let bands = self.pool.current_num_threads();
        let rows = h.div_ceil(bands).max(1);
        let span: u32 = if self.sparse { 2 } else { 1 };
        let (pix, tint) = (&self.pix, &self.tint);
        self.pool.install(|| {
            self.acc.par_chunks_mut(rows * w).enumerate().for_each(|(band, acc)| {
                acc.fill([0.0; 4]);
                let (first, last) = ((band * rows) as u32, (band * rows + acc.len() / w) as u32);
                for (p, (shade, weight)) in pix.iter().zip(tint) {
                    let (x, y) = ((p & 0xFFFF) as usize, p >> 16);
                    if *p == NOWHERE || y + span <= first || y >= last {
                        continue;
                    }
                    let c = &colors[*shade as usize];
                    // A dot may straddle two bands; each band lights its own rows of it.
                    for row in y.max(first)..(y + span).min(last) {
                        let start = (row - first) as usize * w + x;
                        for at in &mut acc[start..start + (span as usize).min(w - x)] {
                            for k in 0..3 {
                                at[k] += c[k] * weight;
                            }
                        }
                    }
                }
            });
        });
        let mut flash_glow: Vec<((usize, usize), f32)> = Vec::new();
        // Merge flashes: a small hot disc that fades.
        for f in flashes {
            let (fx, fy) = (((f.x - x0) * ppm) as f32, ((f.y - y0) * ppm) as f32);
            let r = (1.0 + 2.5 * (1.0 - f.life) + ((f.mass.max(1.0).log10() - 19.0) * 0.25).clamp(0.0, 2.5)) / scale.sqrt();
            if !(fx > -r && fy > -r && fx < w as f32 + r && fy < h as f32 + r) {
                continue;
            }
            let glow = 6.0 * f.life * f.life / self.exposure.max(1e-3);
            for y in ((fy - r).floor().max(0.0) as usize)..=((fy + r).ceil() as usize).min(h - 1) {
                for x in ((fx - r).floor().max(0.0) as usize)..=((fx + r).ceil() as usize).min(w - 1) {
                    let d2 = (x as f32 + 0.5 - fx).powi(2) + (y as f32 + 0.5 - fy).powi(2);
                    let k = glow * (1.0 - d2 / (r * r)).max(0.0);
                    let at = &mut self.acc[y * w + x];
                    at[0] += k;
                    at[1] += 0.85 * k;
                    at[2] += 0.55 * k;
                }
            }
            // The glow is made from the bodies alone; give it the flash as one lump.
            let total = 0.5 * std::f32::consts::PI * r * r * glow;
            let at = (((fy / GLOW_DIV as f32) as usize).min(h.div_ceil(GLOW_DIV)), ((fx / GLOW_DIV as f32) as usize).min(w.div_ceil(GLOW_DIV)));
            flash_glow.push((at, total));
        }

        // Glow: a small image of the same light, blurred, added back in. Each body's light is
        // shared between the four small pixels around it according to where exactly it lies,
        // so the glow moves as smoothly as the body does instead of a small pixel at a time.
        let (gw, gh) = (w.div_ceil(GLOW_DIV) + 1, h.div_ceil(GLOW_DIV) + 1);
        let acc = &self.acc;
        let (glow, blur) = (&mut self.glow, &mut self.blur);
        let bands = self.pool.current_num_threads();
        let glow_rows = gh.div_ceil(bands).max(1);
        // A body's position in the small image, and how much light it is in all.
        let (half, lit) = (0.5 * span as f32, (span * span) as f32);
        let small = |v: usize| {
            let g = ((v as f32 + half) / GLOW_DIV as f32 - 0.5).max(0.0);
            (g as usize, g.fract())
        };
        self.pool.install(|| {
            glow.par_chunks_mut(glow_rows * gw).enumerate().for_each(|(band, out)| {
                out.fill([0.0; 4]);
                let (first, last) = (band * glow_rows, band * glow_rows + out.len() / gw);
                for (p, (shade, weight)) in pix.iter().zip(tint) {
                    if *p == NOWHERE {
                        continue;
                    }
                    let (gy, fy) = small((p >> 16) as usize);
                    if gy + 1 < first || gy >= last {
                        continue;
                    }
                    let (gx, fx) = small((p & 0xFFFF) as usize);
                    let c = &colors[*shade as usize];
                    for (row, wy) in [(gy, 1.0 - fy), (gy + 1, fy)] {
                        if row < first || row >= last {
                            continue;
                        }
                        for (col, wx) in [(gx, 1.0 - fx), ((gx + 1).min(gw - 1), fx)] {
                            let at = &mut out[(row - first) * gw + col];
                            let k = weight * lit * wx * wy;
                            for i in 0..3 {
                                at[i] += c[i] * k;
                            }
                        }
                    }
                }
            });
            for ((gy, gx), total) in &flash_glow {
                let at = &mut glow[gy * gw + gx];
                (at[0], at[1], at[2]) = (at[0] + total, at[1] + 0.85 * total, at[2] + 0.55 * total);
            }
            // Two passes of a 1-2-1 kernel, across then down.
            let mix = |a: [f32; 4], b: [f32; 4], c: [f32; 4]| [0.25 * (a[0] + c[0]) + 0.5 * b[0], 0.25 * (a[1] + c[1]) + 0.5 * b[1], 0.25 * (a[2] + c[2]) + 0.5 * b[2], 0.0];
            for _ in 0..2 {
                blur.par_chunks_mut(gw).zip(glow.par_chunks(gw)).for_each(|(out, row)| {
                    for x in 0..gw {
                        out[x] = mix(row[x.saturating_sub(1)], row[x], row[(x + 1).min(gw - 1)]);
                    }
                });
                let src = &*blur;
                glow.par_chunks_mut(gw).enumerate().for_each(|(y, out)| {
                    let (up, down) = (y.saturating_sub(1), (y + 1).min(gh - 1));
                    for x in 0..gw {
                        out[x] = mix(src[up * gw + x], src[y * gw + x], src[down * gw + x]);
                    }
                });
            }
        });

        if let Some((keep, _)) = linger {
            if self.trail.len() != self.acc.len() || self.trail_stale {
                self.trail = vec![[0.0; 4]; self.acc.len()];
            }
            let (trail, acc) = (&mut self.trail, &self.acc);
            self.pool.install(|| {
                trail.par_chunks_mut(w).zip(acc.par_chunks(w)).for_each(|(trail, acc)| {
                    for (t, a) in trail.iter_mut().zip(acc) {
                        for k in 0..4 {
                            t[k] = t[k] * keep + a[k];
                        }
                    }
                });
            });
        } else {
            self.trail = Vec::new();
        }
        self.trail_stale = false;
        // Without lingering light the image is read twice, the second time for nothing.
        let (lingering, strength) = match linger {
            Some((_, strength)) => (&self.trail[..], strength * self.exposure),
            None => (&self.acc[..], 0.0),
        };
        // Tone mapping. Exposure follows the typical brightness of lit pixels, smoothly.
        let exposure = self.exposure;
        let glow = &self.glow;
        let gain = 0.045 / (GLOW_DIV * GLOW_DIV) as f32 * 4.0 * exposure * self.halo();
        let bg = [style::BACKGROUND.r * 255.0, style::BACKGROUND.g * 255.0, style::BACKGROUND.b * 255.0];
        let span = [255.0 - bg[0], 255.0 - bg[1], 255.0 - bg[2]];
        let across = &self.across;
        let (lit, light) = self.pool.install(|| {
            self.image
                .bytes
                .par_chunks_mut(w * 4)
                .zip(acc.par_chunks(w))
                .zip(lingering.par_chunks(w))
                .enumerate()
                .map_init(
                    || vec![[0.0f32; 4]; gw],
                    |halo, (y, ((out, row), past))| {
                        // The glow for this row: blend the two rows of the small image it lies
                        // between once, then only across for each pixel.
                        let gy = ((y as f32 + 0.5) / GLOW_DIV as f32 - 0.5).max(0.0);
                        let (gy0, fy) = (gy as usize, gy.fract());
                        let gy1 = (gy0 + 1).min(gh - 1);
                        for (o, (a, b)) in halo.iter_mut().zip(glow[gy0 * gw..(gy0 + 1) * gw].iter().zip(&glow[gy1 * gw..(gy1 + 1) * gw])) {
                            for k in 0..4 {
                                o[k] = (a[k] + (b[k] - a[k]) * fy) * gain;
                            }
                        }
                        let mut light = [0.0f32; 4];
                        let mut lit = 0u32;
                        for (((px, a), (gx, fx)), t) in out.chunks_exact_mut(4).zip(row).zip(across).zip(past) {
                            let (h0, h1) = (halo[*gx as usize], halo[*gx as usize + 1]);
                            lit += (a[0] + a[1] + a[2] > 0.0) as u32;
                            let mut v = [0.0f32; 4];
                            for k in 0..4 {
                                light[k] += a[k];
                                v[k] = a[k] * exposure + t[k] * strength + h0[k] + (h1[k] - h0[k]) * fx;
                                // Compress highlights, lift faint light.
                                v[k] = (v[k] / (1.0 + v[k])).sqrt();
                            }
                            px[0] = (bg[0] + span[0] * v[0]) as u8;
                            px[1] = (bg[1] + span[1] * v[1]) as u8;
                            px[2] = (bg[2] + span[2] * v[2]) as u8;
                            px[3] = 255;
                        }
                        (lit, light[0] + light[1] + light[2])
                    },
                )
                .reduce(|| (0, 0.0), |a, b| (a.0 + b.0, a.1 + b.1))
        });
        if lit > 0 {
            // A typical lit pixel lands a third of the way up the scale.
            let wanted = (self.brightness() / (light / lit as f32 / 3.0)).clamp(0.05, 200.0);
            self.exposure += 0.08 * (wanted - self.exposure);
        }
        match &self.texture {
            Some(t) => t.update(&self.image),
            None => {
                let t = Texture2D::from_image(&self.image);
                t.set_filter(FilterMode::Linear);
                self.texture = Some(t);
            }
        }
        let params = DrawTextureParams { dest_size: Some(vec2(w as f32 * scale, h as f32 * scale)), ..Default::default() };
        draw_texture_ex(self.texture.as_ref().unwrap(), 0.0, 0.0, WHITE, params);
        self.last_ms += started.elapsed().as_secs_f32() * 1e3;
    }

    /// The GPU version of [`Density::present`]: hand the projected bodies to the graphics
    /// card. Exposure cannot be read off the finished image there, so it is estimated from
    /// how many bodies share each small block of the screen.
    fn present_gpu(&mut self, flashes: &[Flash]) {
        const CELL: usize = 16;
        let (w, h, scale) = (self.w, self.h, self.scale);
        let (cw, ch) = (w.div_ceil(CELL), h.div_ceil(CELL));
        let colors = self.colors;
        let n = self.pix.len();
        self.vertices.resize(n, [0.0; 5]);
        let chunk = n.div_ceil(self.pool.current_num_threads() * 4).max(1024);
        let span: u32 = if self.sparse { 2 } else { 1 };
        let centre = 0.5 * span as f32;
        let (pix, tint, vertices) = (&self.pix, &self.tint, &mut self.vertices);
        let grid = self.pool.install(|| {
            vertices
                .par_chunks_mut(chunk)
                .zip(pix.par_chunks(chunk))
                .zip(tint.par_chunks(chunk))
                .map(|((out, pix), tint)| {
                    let mut grid = vec![(0u32, 0.0f32); cw * ch];
                    for ((v, p), (shade, weight)) in out.iter_mut().zip(pix).zip(tint) {
                        if *p == NOWHERE {
                            // Off screen: the card clips it.
                            *v = [-10.0, -10.0, 0.0, 0.0, 0.0];
                            continue;
                        }
                        let (x, y) = ((p & 0xFFFF) as usize, (p >> 16) as usize);
                        let c = &colors[*shade as usize];
                        *v = [x as f32 + centre, y as f32 + centre, c[0] * weight, c[1] * weight, c[2] * weight];
                        let cell = &mut grid[(y / CELL) * cw + x / CELL];
                        cell.0 += 1;
                        cell.1 += (c[0] + c[1] + c[2]) * weight;
                    }
                    grid
                })
                .reduce_with(|mut a, b| {
                    for (x, y) in a.iter_mut().zip(b) {
                        *x = (x.0 + y.0, x.1 + y.1);
                    }
                    a
                })
                .unwrap_or_default()
        });
        // Bodies in a block light at most as many pixels as the block has.
        let (lit, light) = grid.iter().fold((0u32, 0.0f32), |s, c| (s.0 + c.0.min((CELL * CELL) as u32), s.1 + c.1));
        if lit > 0 {
            let wanted = (self.brightness() / (light / lit as f32 / 3.0)).clamp(0.05, 200.0);
            self.exposure += 0.08 * (wanted - self.exposure);
        }
        // Hundreds of thousands of points piled onto a few pixels are slow for a graphics
        // card to blend one by one. When bodies share pixels that heavily, add their light up
        // here and send one point for each lit pixel instead.
        let seen: u32 = grid.iter().map(|c| c.0).sum();
        if span == 1 && seen > 2 * lit {
            let (mut x0, mut x1, mut y0, mut y1) = (usize::MAX, 0, usize::MAX, 0);
            for (i, _) in grid.iter().enumerate().filter(|(_, c)| c.0 > 0) {
                (x0, x1, y0, y1) = (x0.min(i % cw), x1.max(i % cw), y0.min(i / cw), y1.max(i / cw));
            }
            // The part of the screen that has anything on it.
            let (left, top) = (x0 * CELL, y0 * CELL);
            let (bw, bh) = (((x1 + 1) * CELL).min(w) - left, ((y1 + 1) * CELL).min(h) - top);
            self.merged.clear();
            self.merged.resize(bw * bh, [0.0; 3]);
            let rows = bh.div_ceil(self.pool.current_num_threads()).max(1);
            let (pix, tint, merged) = (&self.pix, &self.tint, &mut self.merged);
            self.vertices = self.pool.install(|| {
                merged.par_chunks_mut(rows * bw).enumerate().for_each(|(band, out)| {
                    let (first, last) = (top + band * rows, top + band * rows + out.len() / bw);
                    for (p, (shade, weight)) in pix.iter().zip(tint) {
                        let y = (p >> 16) as usize;
                        if *p == NOWHERE || y < first || y >= last {
                            continue;
                        }
                        let at = &mut out[(y - first) * bw + (p & 0xFFFF) as usize - left];
                        let c = &colors[*shade as usize];
                        for k in 0..3 {
                            at[k] += c[k] * weight;
                        }
                    }
                });
                merged
                    .par_chunks(bw)
                    .enumerate()
                    .flat_map_iter(|(row, line)| {
                        line.iter().enumerate().filter(|(_, l)| l[0] + l[1] + l[2] > 0.0).map(move |(col, l)| [(left + col) as f32 + 0.5, (top + row) as f32 + 0.5, l[0], l[1], l[2]])
                    })
                    .collect()
            });
        }
        let glow = 0.045 / (GLOW_DIV * GLOW_DIV) as f32 * 4.0 * self.exposure * self.halo();
        let linger = self.linger();
        if std::mem::take(&mut self.trail_stale) {
            self.gpu.as_mut().unwrap().clear_trail();
        }
        self.gpu.as_mut().unwrap().draw(&self.vertices, (w, h), self.dot, span, GLOW_DIV, (w as f32 * scale, h as f32 * scale), self.exposure, glow, linger, style::BACKGROUND);
        let (x0, y0, ppm) = self.corner;
        for f in flashes {
            let (fx, fy) = (((f.x - x0) * ppm) as f32 * scale, ((f.y - y0) * ppm) as f32 * scale);
            let r = (1.0 + 2.5 * (1.0 - f.life) + ((f.mass.max(1.0).log10() - 19.0) * 0.25).clamp(0.0, 2.5)) * scale.sqrt();
            if fx > -r && fy > -r && fx < screen_width() + r && fy < screen_height() + r {
                // A hot core inside a wider, fainter halo.
                style::disc(fx, fy, 2.4 * r, Color::new(1.0, 0.8, 0.5, 0.22 * f.life * f.life));
                style::disc(fx, fy, r, Color::new(1.0, 0.92, 0.7, f.life));
            }
        }
    }

    /// Switch between the graphics card and the rasteriser threads. Takes effect on the
    /// next frame; if the card cannot do it, [`Density::take_notice`] says why.
    pub fn set_gpu(&mut self, on: bool) {
        self.gpu_wanted = on;
        if !on {
            if let Some(mut gpu) = self.gpu.take() {
                gpu.delete();
            }
        }
    }

    /// Why GPU drawing was given up, once.
    pub fn take_notice(&mut self) -> Option<String> {
        self.notice.take()
    }
}

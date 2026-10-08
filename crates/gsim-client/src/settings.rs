//! Small `key=value` settings file in the platform's config directory.

use crate::density::ColorMode;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// What the camera stays with in a world with a ship.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Camera {
    Ship,
    Selection,
    /// Stays with the selection like `Selection`, and moves and zooms by itself so that the
    /// ship, its predicted path and the selected body are all on screen.
    Auto,
}

impl Camera {
    pub const ALL: [Camera; 3] = [Camera::Ship, Camera::Selection, Camera::Auto];

    pub fn name(self) -> &'static str {
        match self {
            Camera::Ship => "ship",
            Camera::Selection => "selection",
            Camera::Auto => "auto",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub name: String,
    pub server: String,
    pub preset: String,
    pub mouse_aim: bool,
    pub fullscreen: bool,
    /// The colour of our ship, as other players see it too.
    pub ship_color: [u8; 3],
    /// Multiplier on top of the automatic, window-height-proportional UI size.
    pub ui_scale: f32,
    /// Zoom steps per unit of mouse-wheel delta.
    pub zoom_speed: f32,
    /// How much delta-v (km/s) the line drawn while thrusting assumes the burn will spend.
    pub burn_preview: f32,
    /// Draw large-scale worlds on the graphics card instead of on rasteriser threads.
    pub gpu: bool,
    // What is drawn; see `options` for what each one means.
    pub prediction: bool,
    pub shell_prediction: bool,
    pub trails: bool,
    pub trails_relative: bool,
    pub body_names: bool,
    pub camera: Camera,
    /// The F3 panel: network details in exact worlds, statistics in large ones.
    pub details: bool,
    pub color: ColorMode,
    /// Large-scale worlds: light lingers, so that moving bodies draw streaks.
    pub long_exposure: bool,
    /// Per single-player world: the parameters changed from their defaults.
    pub params: BTreeMap<String, BTreeMap<String, f64>>,
    /// Per large-scale world: how many bodies this machine was measured to hold.
    pub measured: BTreeMap<String, usize>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { name: "Player".into(), server: "localhost".into(), preset: "random".into(), mouse_aim: true, fullscreen: false, ship_color: random_color(), ui_scale: 1.2, zoom_speed: default_zoom_speed(), burn_preview: 5.0, gpu: true, prediction: true, shell_prediction: false, trails: false, trails_relative: true, body_names: false, camera: Camera::Selection, details: false, color: ColorMode::Mass, long_exposure: false, params: BTreeMap::new(), measured: BTreeMap::new() }
    }
}

fn path() -> Option<PathBuf> {
    let dir = if cfg!(target_os = "windows") {
        PathBuf::from(std::env::var_os("APPDATA")?)
    } else if cfg!(target_os = "macos") {
        PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support")
    } else if let Some(x) = std::env::var_os("XDG_CONFIG_HOME").filter(|x| !x.is_empty()) {
        PathBuf::from(x)
    } else {
        PathBuf::from(std::env::var_os("HOME")?).join(".config")
    };
    Some(dir.join("potato_gsim").join("settings.txt"))
}

impl Settings {
    /// Where the settings file lives: saved worlds go next to it.
    pub fn directory() -> Option<PathBuf> {
        path().and_then(|p| p.parent().map(PathBuf::from))
    }
}

impl Settings {
    pub fn load() -> Self {
        let mut s = Self::default();
        let text = path().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim();
            match k.trim() {
                "name" if !v.is_empty() => s.name = v.to_string(),
                "server" if !v.is_empty() => s.server = v.to_string(),
                "preset" if !v.is_empty() => s.preset = v.to_string(),
                "mouse_aim" => s.mouse_aim = v != "0",
                "fullscreen" => s.fullscreen = v == "1",
                "ship_color" => s.ship_color = u32::from_str_radix(v, 16).map_or(s.ship_color, |c| [(c >> 16) as u8, (c >> 8) as u8, c as u8]),
                "ui_scale" => s.ui_scale = v.parse().unwrap_or(s.ui_scale).clamp(0.6, 2.5),
                "zoom_speed" => s.zoom_speed = v.parse().unwrap_or(s.zoom_speed).clamp(0.002, 3.0),
                "burn_preview" => s.burn_preview = v.parse().unwrap_or(s.burn_preview).clamp(0.1, 100.0),
                "gpu_drawing" => s.gpu = v != "0",
                "prediction" => s.prediction = v != "0",
                "shell_prediction" => s.shell_prediction = v == "1",
                "trails" => s.trails = v == "1",
                "trails_relative" => s.trails_relative = v != "0",
                "body_names" => s.body_names = v == "1",
                "camera" => s.camera = Camera::ALL.into_iter().find(|m| m.name() == v).unwrap_or(s.camera),
                "details" => s.details = v == "1",
                "long_exposure" => s.long_exposure = v == "1",
                "color" => s.color = ColorMode::ALL.into_iter().find(|m| m.name() == v).unwrap_or(s.color),
                key => {
                    // `param.<world>.<key>` and `measured.<world>`
                    let mut parts = key.split('.');
                    match (parts.next(), parts.next(), parts.next()) {
                        (Some("param"), Some(world), Some(name)) => {
                            if let Ok(value) = v.parse::<f64>() {
                                s.params.entry(world.to_string()).or_default().insert(name.to_string(), value);
                            }
                        }
                        (Some("measured"), Some(world), None) => {
                            if let Ok(count) = v.parse::<usize>() {
                                s.measured.insert(world.to_string(), count);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        if !text.lines().any(|l| l.starts_with("ship_color=")) {
            // The colour picked at random the first time stays.
            s.save();
        }
        s
    }

    pub fn save(&self) {
        let Some(p) = path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut text = format!(
            "name={}\nserver={}\npreset={}\nmouse_aim={}\nfullscreen={}\nui_scale={}\nzoom_speed={}\nburn_preview={}\ngpu_drawing={}\n",
            self.name, self.server, self.preset, self.mouse_aim as u8, self.fullscreen as u8, self.ui_scale, self.zoom_speed, self.burn_preview, self.gpu as u8
        );
        for (key, on) in [
            ("prediction", self.prediction),
            ("shell_prediction", self.shell_prediction),
            ("trails", self.trails),
            ("trails_relative", self.trails_relative),
            ("body_names", self.body_names),
            ("details", self.details),
            ("long_exposure", self.long_exposure),
        ] {
            text.push_str(&format!("{key}={}\n", on as u8));
        }
        text.push_str(&format!("color={}\n", self.color.name()));
        text.push_str(&format!("ship_color={}\n", gsim_proto::identity::to_hex(&self.ship_color)));
        text.push_str(&format!("camera={}\n", self.camera.name()));
        for (world, params) in &self.params {
            for (key, value) in params {
                text.push_str(&format!("param.{world}.{key}={value}\n"));
            }
        }
        for (world, count) in &self.measured {
            text.push_str(&format!("measured.{world}={count}\n"));
        }
        let _ = std::fs::write(p, text);
    }
}

/// A bright colour of any hue.
fn random_color() -> [u8; 3] {
    let hue = gsim_proto::identity::random_bytes()[0] as f32 / 256.0 * 6.0;
    // One channel full, one at the floor, one in between.
    let (lo, mid) = (90.0, 90.0 + 165.0 * (1.0 - (hue % 2.0 - 1.0).abs()));
    let (r, g, b) = match hue as u32 {
        0 => (255.0, mid, lo),
        1 => (mid, 255.0, lo),
        2 => (lo, 255.0, mid),
        3 => (lo, mid, 255.0),
        4 => (mid, lo, 255.0),
        _ => (255.0, lo, mid),
    };
    [r as u8, g as u8, b as u8]
}

/// macOS reports scroll deltas in points (tens per gesture frame); elsewhere one wheel
/// notch is one unit.
fn default_zoom_speed() -> f32 {
    if cfg!(target_os = "macos") {
        0.02
    } else {
        1.0
    }
}

impl Settings {
    /// Scale for text, markers and HUD: proportional to the window height so everything
    /// covers the same share of the screen at any resolution, times the user's preference.
    pub fn ui_factor(&self) -> f32 {
        (macroquad::window::screen_height() / 800.0).clamp(0.8, 3.0) * self.ui_scale
    }
}

impl Settings {
    /// Scale for things drawn in the world (ship icon, far-away body dots, labels). Follows
    /// the UI size only weakly: these should stay small next to what they mark.
    pub fn marker_factor(&self) -> f32 {
        self.ui_factor().sqrt().clamp(0.9, 1.6)
    }
}

impl Settings {
    /// This player's key, created on first use and kept next to the settings file. Whoever
    /// holds this file owns the player's names on every server, so it is written owner-only.
    pub fn identity(&self) -> gsim_proto::Identity {
        use gsim_proto::identity::{from_hex, to_hex};
        let file = path().map(|p| p.with_file_name("identity.key"));
        if let Some(secret) = file.as_ref().and_then(|f| std::fs::read_to_string(f).ok()).and_then(|s| from_hex(&s)) {
            return gsim_proto::Identity::from_secret(secret);
        }
        let identity = gsim_proto::Identity::generate();
        if let Some(file) = &file {
            if let Some(dir) = file.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if std::fs::write(file, to_hex(&identity.secret())).is_ok() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600));
                }
            }
        }
        identity
    }
}

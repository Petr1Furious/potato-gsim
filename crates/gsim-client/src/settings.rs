//! Small `key=value` settings file in the platform's config directory.

use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Settings {
    pub name: String,
    pub server: String,
    pub preset: String,
    pub mouse_aim: bool,
    pub fullscreen: bool,
    /// Multiplier on top of the automatic, window-height-proportional UI size.
    pub ui_scale: f32,
    /// Zoom steps per unit of mouse-wheel delta.
    pub zoom_speed: f32,
    /// Draw large-scale worlds on the graphics card instead of on rasteriser threads.
    pub gpu: bool,
    /// Per single-player world: the parameters changed from their defaults.
    pub params: BTreeMap<String, BTreeMap<String, f64>>,
    /// Per large-scale world: how many bodies this machine was measured to hold.
    pub measured: BTreeMap<String, usize>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { name: "Player".into(), server: "localhost".into(), preset: "random".into(), mouse_aim: true, fullscreen: false, ui_scale: 1.2, zoom_speed: default_zoom_speed(), gpu: false, params: BTreeMap::new(), measured: BTreeMap::new() }
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
    pub fn load() -> Self {
        let mut s = Self::default();
        let Some(text) = path().and_then(|p| std::fs::read_to_string(p).ok()) else { return s };
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim();
            match k.trim() {
                "name" if !v.is_empty() => s.name = v.to_string(),
                "server" if !v.is_empty() => s.server = v.to_string(),
                "preset" if !v.is_empty() => s.preset = v.to_string(),
                "mouse_aim" => s.mouse_aim = v != "0",
                "fullscreen" => s.fullscreen = v == "1",
                "ui_scale" => s.ui_scale = v.parse().unwrap_or(s.ui_scale).clamp(0.6, 2.5),
                "zoom_speed" => s.zoom_speed = v.parse().unwrap_or(s.zoom_speed).clamp(0.002, 3.0),
                "gpu" => s.gpu = v == "1",
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
        s
    }

    pub fn save(&self) {
        let Some(p) = path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut text = format!(
            "name={}\nserver={}\npreset={}\nmouse_aim={}\nfullscreen={}\nui_scale={}\nzoom_speed={}\ngpu={}\n",
            self.name, self.server, self.preset, self.mouse_aim as u8, self.fullscreen as u8, self.ui_scale, self.zoom_speed, self.gpu as u8
        );
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

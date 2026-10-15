//! Small `key=value` settings file in the platform's config directory.

use crate::density::ColorMode;
use std::collections::BTreeMap;
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;

/// What the camera stays with in a world with a ship.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Camera {
    /// Stays with the selected body, or with one's own ship while nothing is selected.
    Selection,
    /// Stays with the selection like `Selection`, and moves and zooms by itself so that the
    /// ship, its predicted path and the selected body are all on screen.
    Auto,
}

impl Camera {
    pub const ALL: [Camera; 2] = [Camera::Selection, Camera::Auto];

    pub fn name(self) -> &'static str {
        match self {
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
    /// The color of our ship, as other players see it too.
    pub ship_color: [u8; 3],
    /// Multiplier on top of the automatic, window-height-proportional UI size.
    pub ui_scale: f32,
    /// Zoom steps per unit of mouse-wheel delta.
    pub zoom_speed: f32,
    /// How much delta-v (km/s) the line drawn while thrusting assumes the burn will spend.
    pub burn_preview: f32,
    /// Draw large-scale worlds on the graphics card instead of on rasterizer threads.
    pub gpu: bool,
    // What is drawn; see `options` for what each one means.
    pub prediction: bool,
    pub shell_prediction: bool,
    pub trails: bool,
    pub trails_relative: bool,
    pub body_names: bool,
    pub camera: Camera,
    /// Large-scale worlds: the camera stays with the selection (or else where it is).
    pub follow: bool,
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
        Self { name: "Player".into(), server: "localhost".into(), preset: "random".into(), mouse_aim: true, fullscreen: false, ship_color: random_color(), ui_scale: 1.5, zoom_speed: default_zoom_speed(), burn_preview: 5.0, gpu: true, prediction: true, shell_prediction: false, trails: false, trails_relative: true, body_names: false, camera: Camera::Selection, follow: true, details: false, color: ColorMode::Mass, long_exposure: false, params: BTreeMap::new(), measured: BTreeMap::new() }
    }
}

/// Where what is kept between runs lives.
#[cfg(not(target_arch = "wasm32"))]
fn directory() -> Option<PathBuf> {
    let dir = if cfg!(target_os = "windows") {
        PathBuf::from(std::env::var_os("APPDATA")?)
    } else if cfg!(target_os = "macos") {
        PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support")
    } else if let Some(x) = std::env::var_os("XDG_CONFIG_HOME").filter(|x| !x.is_empty()) {
        PathBuf::from(x)
    } else {
        PathBuf::from(std::env::var_os("HOME")?).join(".config")
    };
    Some(dir.join("potato_gsim"))
}

/// Something kept between runs under a name: a file in the settings directory.
#[cfg(not(target_arch = "wasm32"))]
fn kept(name: &str) -> Option<String> {
    std::fs::read_to_string(directory()?.join(name)).ok()
}

/// `private`: nobody else on the machine may read it.
#[cfg(not(target_arch = "wasm32"))]
fn keep(name: &str, text: &str, private: bool) {
    let Some(dir) = directory() else { return };
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join(name);
    if std::fs::write(&file, text).is_ok() && private {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600));
        }
    }
}

/// In a browser: the page's own storage (see `web/gsim.js`).
#[cfg(target_arch = "wasm32")]
mod page {
    extern "C" {
        /// Length of what is kept under a name, or -1; `gsim_store_take` then copies it out.
        pub fn gsim_store_get(name: *const u8, len: usize) -> i32;
        pub fn gsim_store_take(to: *mut u8);
        pub fn gsim_store_set(name: *const u8, len: usize, text: *const u8, text_len: usize);
    }
}

#[cfg(target_arch = "wasm32")]
fn kept(name: &str) -> Option<String> {
    // SAFETY: the script reads `name` during the call and writes as many bytes as it said.
    unsafe {
        let len = page::gsim_store_get(name.as_ptr(), name.len());
        let mut bytes = vec![0u8; usize::try_from(len).ok()?];
        page::gsim_store_take(bytes.as_mut_ptr());
        String::from_utf8(bytes).ok()
    }
}

#[cfg(target_arch = "wasm32")]
fn keep(name: &str, text: &str, _private: bool) {
    unsafe { page::gsim_store_set(name.as_ptr(), name.len(), text.as_ptr(), text.len()) };
}

#[cfg(not(target_arch = "wasm32"))]
impl Settings {
    /// Where the settings file lives: saved worlds go next to it.
    pub fn directory() -> Option<PathBuf> {
        directory()
    }
}

impl Settings {
    pub fn load() -> Self {
        let mut s = Self::default();
        let text = kept("settings.txt").unwrap_or_default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim();
            match k.trim() {
                "name" if !v.is_empty() => s.name = v.to_string(),
                "server" if !v.is_empty() => s.server = v.to_string(),
                "preset" if !v.is_empty() => s.preset = v.to_string(),
                "mouse_aim" => s.mouse_aim = v != "0",
                "fullscreen" => s.fullscreen = v == "1",
                "ship_color" => s.ship_color = u32::from_str_radix(v, 16).map_or(s.ship_color, |c| gsim_proto::visible_color([(c >> 16) as u8, (c >> 8) as u8, c as u8])),
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
                "follow" => s.follow = v == "1",
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
            // The color picked at random the first time stays.
            s.save();
        }
        s
    }

    pub fn save(&self) {
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
            ("follow", self.follow),
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
        keep("settings.txt", &text, false);
    }
}

/// A bright color of any hue.
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

/// What a copied player starts with: the rest is `<key>:<color>:<name>`, the first two in hex.
const PLAYER_PREFIX: &str = "gsim1:";

impl Settings {
    /// This player in one line, to be copied into the game elsewhere (the program, a browser):
    /// the key, the ship's color and the name.
    pub fn exported(&self) -> String {
        use gsim_proto::identity::to_hex;
        format!("{PLAYER_PREFIX}{}:{}:{}", to_hex(&self.identity().secret()), to_hex(&self.ship_color), self.name)
    }

    /// Become the player in `text` (see [`player`]). Their key is used from the next game on.
    /// False if `text` is not a player.
    pub fn adopt(&mut self, text: &str) -> bool {
        let Some(Copied { secret, color, name }) = player(text) else { return false };
        keep("identity.key", &gsim_proto::identity::to_hex(&secret), true);
        if let Some(color) = color {
            self.ship_color = gsim_proto::visible_color(color);
        }
        if let Some(name) = name.filter(|name| !name.is_empty()) {
            self.name = name;
        }
        true
    }
}

/// A player copied from the game elsewhere.
pub struct Copied {
    pub secret: [u8; 32],
    pub color: Option<[u8; 3]>,
    pub name: Option<String>,
}

/// The player in `text`: a line from [`Settings::exported`], or a bare key (with no name or
/// color to go with it).
pub fn player(text: &str) -> Option<Copied> {
    use gsim_proto::identity::from_hex;
    let text = text.trim();
    let Some(rest) = text.strip_prefix(PLAYER_PREFIX) else { return Some(Copied { secret: from_hex(text)?, color: None, name: None }) };
    let mut parts = rest.splitn(3, ':');
    let (secret, color, name) = (from_hex(parts.next()?)?, parts.next()?, parts.next()?);
    let color = u32::from_str_radix(color, 16).ok().filter(|_| color.len() == 6)?;
    Some(Copied { secret, color: Some([(color >> 16) as u8, (color >> 8) as u8, color as u8]), name: Some(gsim_proto::clean_name(name)) })
}

impl Settings {
    /// This player's key, created on first use and kept next to the settings. Whoever holds
    /// it owns the player's names on every server, so nobody else may read it.
    pub fn identity(&self) -> gsim_proto::Identity {
        use gsim_proto::identity::{from_hex, to_hex};
        if let Some(secret) = kept("identity.key").and_then(|s| from_hex(&s)) {
            return gsim_proto::Identity::from_secret(secret);
        }
        let identity = gsim_proto::Identity::generate();
        keep("identity.key", &to_hex(&identity.secret()), true);
        identity
    }
}

#[cfg(test)]
mod copied_players {
    use super::*;

    #[test]
    fn a_copied_player_brings_key_color_and_name() {
        let key = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let Copied { secret, color, name } = player(&format!(" gsim1:{key}:ff8040:Ann: the second ")).unwrap();
        assert_eq!(gsim_proto::identity::to_hex(&secret), key);
        assert_eq!(color, Some([0xff, 0x80, 0x40]));
        assert_eq!(name.as_deref(), Some("Ann: the second"));
        // A bare key, as copied before: only the key.
        assert!(player(key).is_some_and(|p| p.color.is_none() && p.name.is_none()));
        for wrong in ["", "gsim1:", "gsim1:abc:ff8040:Ann", &format!("gsim1:{key}:ff80:Ann"), &format!("gsim1:{key}:ff8040"), "Ann"] {
            assert!(player(wrong).is_none(), "{wrong:?}");
        }
    }
}

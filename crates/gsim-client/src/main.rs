//! The game's client: a program, or (built for `wasm32`) a page in a browser, which joins
//! servers and leaves out what a page cannot do: worlds of its own, and updating itself.

// (A page uses a part of the code.)
#![cfg_attr(target_arch = "wasm32", allow(dead_code, unused_imports))]
// No console window behind the game on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod chat;
mod density;
mod fmt;
mod game;
mod gpu;
#[cfg(not(target_arch = "wasm32"))]
mod menu;
mod options;
mod predictor;
#[cfg(not(target_arch = "wasm32"))]
mod sandbox;
mod settings;
mod style;
#[cfg(target_arch = "wasm32")]
mod worker;
#[cfg(not(target_arch = "wasm32"))]
mod updater;

use clap::Parser;
use egui_macroquad::egui;
use game::{Game, Outcome};
use macroquad::prelude::*;
#[cfg(not(target_arch = "wasm32"))]
use menu::{Choice, SinglePlayer, Started};
#[cfg(not(target_arch = "wasm32"))]
use sandbox::Sandbox;
use settings::Settings;

/// What players see: window title, menu heading, app bundle.
const APP_NAME: &str = "Potato Gravity Simulator";

/// Potato Gravity Simulator client.
#[derive(Parser, Debug, Clone)]
#[command(version)]
struct Args {
    /// Join this server straight away (host or host:port)
    #[arg(long)]
    connect: Option<String>,
    /// Start a single-player world straight away (a preset, or galaxy, collision, cloud)
    #[arg(long)]
    solo: Option<String>,
    /// A parameter of the `--solo` world, as key=value (repeatable)
    #[arg(long = "set", value_name = "KEY=VALUE")]
    set: Vec<String>,
    /// Draw the `--solo` world on processor threads, whatever the saved setting says
    #[arg(long)]
    no_gpu: bool,
    /// Measure how many bodies the large-scale engine holds on this machine, then exit
    #[arg(long)]
    bench: bool,
    /// Player name (overrides the saved one)
    #[arg(long)]
    name: Option<String>,
    /// World seed for single-player worlds (0 = random)
    #[arg(long, default_value_t = 0)]
    seed: u64,
    /// Seconds of ephemeris to keep ahead of the present (bounds the prediction line)
    #[arg(long, default_value_t = 20.0)]
    lookahead: f32,
    #[arg(long)]
    fullscreen: bool,
    /// Do not check for or install updates
    #[arg(long, env = "GSIM_NO_UPDATE")]
    no_update: bool,
    /// Show how bodies of different masses and sizes are drawn, instead of the game
    #[arg(long)]
    gallery: bool,
    /// Run the simulation self-test, print the result and exit (no window)
    #[arg(long)]
    selftest: bool,
    /// Save a screenshot to this path after `--screenshot-after` seconds, then quit
    #[arg(long)]
    screenshot: Option<String>,
    #[arg(long, default_value_t = 6.0)]
    screenshot_after: f64,
}

fn main() {
    // (In a browser, the worker beside the page has a program to load: the sooner the better.)
    #[cfg(target_arch = "wasm32")]
    worker::start();
    let args = Args::parse();
    let got = gsim_core::selftest::compute();
    let ok = got == gsim_core::selftest::GOLDEN;
    if args.selftest {
        println!("self-test: computed {got:#018x}, expected {:#018x} -> {}", gsim_core::selftest::GOLDEN, if ok { "OK" } else { "MISMATCH" });
        std::process::exit(if ok { 0 } else { 2 });
    }
    let mut settings = Settings::load();
    #[cfg(not(target_arch = "wasm32"))]
    {
        // One thread per core in total, not one per core for the simulation alone: the picture
        // needs some too, and fighting over them costs more frames than it gains steps.
        let _ = rayon::ThreadPoolBuilder::new().num_threads(density::simulation_threads(settings.gpu)).thread_name(|i| format!("gsim-sim-{i}")).build_global();
        if args.bench {
            println!("kernels: {}, {} simulation threads", gsim_swarm::Level::detect().name(), rayon::current_num_threads());
            for sc in gsim_swarm::scenario::SCENARIOS {
                match gsim_swarm::bench::suggest(sc.name, &Default::default(), gsim_swarm::bench::STEP_BUDGET_MS) {
                    Ok(count) => println!("{:10} about {count} bodies at {:.0} steps per second", sc.name, 1000.0 / gsim_swarm::bench::STEP_BUDGET_MS),
                    Err(e) => println!("{:10} {e}", sc.name),
                }
            }
            return;
        }
    }
    if !ok {
        eprintln!("warning: simulation self-test failed; servers will refuse this build");
    }
    if let Some(n) = &args.name {
        settings.name = n.clone();
    }
    settings.fullscreen |= args.fullscreen;
    let conf = Conf {
        window_title: APP_NAME.into(),
        icon: Some(miniquad::conf::Icon {
            small: *include_bytes!("../assets/icon-16.rgba"),
            medium: *include_bytes!("../assets/icon-32.rgba"),
            big: *include_bytes!("../assets/icon-64.rgba"),
        }),
        window_width: 1400,
        window_height: 900,
        high_dpi: true,
        fullscreen: settings.fullscreen,
        sample_count: 4,
        // In a browser: the newer WebGL, which can draw into floating-point images (what the
        // glowing picture of the small bodies is made in).
        platform: miniquad::conf::Platform { webgl_version: miniquad::conf::WebGLVersion::WebGL2, ..Default::default() },
        ..Default::default()
    };
    macroquad::Window::from_config(conf, run(args, settings));
}

enum Screen {
    Menu,
    #[cfg(not(target_arch = "wasm32"))]
    Single,
    Settings,
    Game(Box<Game>),
    #[cfg(not(target_arch = "wasm32"))]
    Sandbox(Box<Sandbox>),
}

#[cfg(not(target_arch = "wasm32"))]
impl From<Started> for Screen {
    fn from(started: Started) -> Self {
        match started {
            Started::Exact(game) => Screen::Game(game),
            Started::Large(sandbox) => Screen::Sandbox(sandbox),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn seed_or_random(seed: u64) -> u64 {
    if seed != 0 {
        return seed;
    }
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64 | 1).unwrap_or(1)
}

fn join(settings: &Settings, args: &Args) -> Result<Game, String> {
    // (A page joins the server it came from.)
    let server = if cfg!(target_arch = "wasm32") { "" } else { settings.server.as_str() };
    Game::connect(server, settings, None, args.lookahead)
}

/// `--solo <world> [--set key=value ...]`: the settings file is left alone.
#[cfg(not(target_arch = "wasm32"))]
fn solo(settings: &Settings, args: &Args, world: &str) -> Result<Started, String> {
    let mut settings = settings.clone();
    settings.preset = world.to_string();
    settings.gpu &= !args.no_gpu;
    let params = settings.params.entry(world.to_string()).or_default();
    for setting in &args.set {
        let (key, value) = gsim_server::scenario::parse_setting(setting)?;
        params.insert(key, value);
    }
    menu::launch(&settings, seed_or_random(args.seed), args.lookahead)
}

/// The window library hands macOS a 64 pixel icon, which the Dock draws far larger; give it
/// the full-size picture instead.
#[cfg(target_os = "macos")]
fn dock_icon() {
    use miniquad::native::apple::frameworks::*;
    static PNG: &[u8] = include_bytes!("../assets/icon.png");
    // SAFETY: plain AppKit calls on the main thread, after the application object exists.
    unsafe {
        let data: ObjcId = msg_send![class!(NSData), dataWithBytes: PNG.as_ptr() length: PNG.len()];
        let image: ObjcId = msg_send![class!(NSImage), alloc];
        let image: ObjcId = msg_send![image, initWithData: data];
        if !image.is_null() {
            let app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
            let () = msg_send![app, setApplicationIconImage: image];
        }
    }
}

async fn run(args: Args, mut settings: Settings) {
    style::init();
    #[cfg(target_os = "macos")]
    dock_icon();
    let mut message = String::new();
    #[cfg(not(target_arch = "wasm32"))]
    let updater = updater::Updater::start(!args.no_update);
    // Closing the window or pressing Cmd+Q should still say goodbye to the server and
    // install a pending update, so take over the quit request.
    prevent_quit();
    let mut screen = Screen::Menu;
    #[cfg(not(target_arch = "wasm32"))]
    let mut single = SinglePlayer::default();
    if let Some(server) = &args.connect {
        settings.server = server.clone();
        match join(&settings, &args) {
            Ok(g) => screen = Screen::Game(Box::new(g)),
            Err(e) => message = e,
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let (None, Some(world)) = (&args.connect, &args.solo) {
        match solo(&settings, &args, world) {
            Ok(started) => screen = started.into(),
            Err(e) => message = e,
        }
    }
    let started = get_time();

    loop {
        if args.gallery {
            gallery(settings.marker_factor());
            if let Some(path) = &args.screenshot {
                if get_time() - started >= args.screenshot_after {
                    get_screen_data().export_png(path);
                    break;
                }
            }
            if is_quit_requested() || is_key_pressed(KeyCode::Escape) {
                break;
            }
            next_frame().await;
            continue;
        }
        let mut next: Option<Screen> = None;
        let mut quit = false;
        let outcome = match &mut screen {
            Screen::Game(game) => game.frame(&mut settings),
            #[cfg(not(target_arch = "wasm32"))]
            Screen::Sandbox(sandbox) => sandbox.frame(&mut settings),
            _ => Outcome::Continue,
        };
        match outcome {
            Outcome::Continue => {}
            Outcome::ToMenu(msg) => {
                message = msg;
                next = Some(Screen::Menu);
            }
            Outcome::Quit => quit = true,
        }
        match &mut screen {
            Screen::Game(_) => {}
            #[cfg(not(target_arch = "wasm32"))]
            Screen::Sandbox(_) => {}
            Screen::Settings => {
                clear_background(style::BACKGROUND);
                let factor = settings.ui_factor();
                let mut back = false;
                egui_macroquad::ui(|ctx| {
                    ctx.set_zoom_factor(factor);
                    egui::Window::new("SETTINGS").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                        ui.set_width(330.0);
                        options::scrolled(ui, |ui| options::show(ui, &mut settings, options::World::Any));
                        ui.separator();
                        back = ui.button("Back").clicked();
                    });
                });
                egui_macroquad::draw();
                if back || is_key_pressed(KeyCode::Escape) {
                    settings.save();
                    next = Some(Screen::Menu);
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            Screen::Single => {
                clear_background(style::BACKGROUND);
                let factor = settings.ui_factor();
                let mut choice = Choice::Stay;
                egui_macroquad::ui(|ctx| {
                    ctx.set_zoom_factor(factor);
                    choice = single.show(ctx, &mut settings);
                    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                        choice = Choice::Back;
                    }
                    if !message.is_empty() {
                        egui::Area::new(egui::Id::new("error")).anchor(egui::Align2::CENTER_BOTTOM, [0.0, -30.0]).show(ctx, |ui| {
                            ui.colored_label(egui::Color32::from_rgb(255, 140, 120), &message);
                        });
                    }
                });
                egui_macroquad::draw();
                match choice {
                    Choice::Stay => {}
                    Choice::Back => {
                        settings.save();
                        message.clear();
                        next = Some(Screen::Menu);
                    }
                    Choice::Start => {
                        settings.name = settings.name.trim().to_string();
                        settings.save();
                        let seed = seed_or_random(if single.seed() != 0 { single.seed() } else { args.seed });
                        match menu::launch(&settings, seed, args.lookahead) {
                            Ok(started) => {
                                message.clear();
                                next = Some(started.into());
                            }
                            Err(e) => message = e,
                        }
                    }
                }
            }
            Screen::Menu => {
                // Between games is the one moment a restart costs the player nothing.
                #[cfg(not(target_arch = "wasm32"))]
                let update = updater.status();
                #[cfg(not(target_arch = "wasm32"))]
                if update == updater::Status::Ready {
                    settings.save();
                    updater.apply(true);
                }
                clear_background(style::BACKGROUND);
                let mut action: Option<Result<Game, String>> = None;
                let factor = settings.ui_factor();
                egui_macroquad::ui(|ctx| {
                    ctx.set_zoom_factor(factor);
                    egui::Window::new(APP_NAME)
                        .collapsible(false)
                        .resizable(false)
                        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                        .show(ctx, |ui| {
                            ui.set_width(340.0);
                            // One full-width button per way into the game, each under the
                            // fields it uses.
                            let wide = |ui: &mut egui::Ui, text: &str| ui.add_sized([ui.available_width(), 32.0], egui::Button::new(text)).clicked();
                            style::section(ui, "PLAYER");
                            ui.horizontal(|ui| {
                                // The colour button keeps its size; the name takes the rest.
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    options::ship_color(ui, &mut settings).on_hover_text("The colour of your ship.");
                                    ui.add(egui::TextEdit::singleline(&mut settings.name).desired_width(f32::INFINITY).hint_text("name"));
                                });
                            });
                            style::section(ui, "MULTIPLAYER");
                            #[cfg(not(target_arch = "wasm32"))]
                            ui.add(egui::TextEdit::singleline(&mut settings.server).desired_width(f32::INFINITY).hint_text("server address"));
                            ui.add_space(2.0);
                            if wide(ui, "Join server") {
                                action = Some(join(&settings, &args));
                            }
                            #[cfg(not(target_arch = "wasm32"))]
                            {
                                style::section(ui, "SINGLE PLAYER");
                                if wide(ui, "Choose a world") {
                                    message.clear();
                                    next = Some(Screen::Single);
                                }
                            }
                            ui.add_space(10.0);
                            ui.separator();
                            // (A page is left by closing it.)
                            ui.columns(if cfg!(target_arch = "wasm32") { 1 } else { 2 }, |columns| {
                                if wide(&mut columns[0], "Settings") {
                                    message.clear();
                                    next = Some(Screen::Settings);
                                }
                                #[cfg(not(target_arch = "wasm32"))]
                                if wide(&mut columns[1], "Quit") {
                                    quit = true;
                                }
                            });
                            if !message.is_empty() {
                                ui.add_space(6.0);
                                ui.colored_label(egui::Color32::from_rgb(255, 140, 120), &message);
                            }
                            #[cfg(not(target_arch = "wasm32"))]
                            {
                                let note = match &update {
                                    updater::Status::Disabled => format!("build {}", short(updater::build_version())),
                                    updater::Status::Checking => "checking for updates...".to_string(),
                                    updater::Status::UpToDate => format!("up to date ({})", short(updater::build_version())),
                                    updater::Status::Downloading { percent } => format!("downloading update... {percent} %"),
                                    updater::Status::Ready => "installing update...".to_string(),
                                    updater::Status::Failed(e) => e.clone(),
                                };
                                ui.add_space(6.0);
                                // Wrapped: an error can be much longer than the menu is wide.
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(egui::RichText::new(note).small().color(style::c32(style::DIM)));
                                    if updater.can_check() && ui.small_button("check for updates").clicked() {
                                        updater.check();
                                    }
                                });
                            }
                        });
                });
                egui_macroquad::draw();
                match action {
                    Some(Ok(g)) => {
                        settings.name = settings.name.trim().to_string();
                        settings.save();
                        message.clear();
                        next = Some(Screen::Game(Box::new(g)));
                    }
                    Some(Err(e)) => message = e,
                    None => {}
                }
            }
        }
        if is_quit_requested() {
            quit = true;
        }
        if let Some(path) = &args.screenshot {
            if get_time() - started >= args.screenshot_after {
                get_screen_data().export_png(path);
                quit = true;
            }
        }
        if quit {
            if let Screen::Game(game) = &mut screen {
                game.leave();
            }
            // Install a finished download on the way out; the next start runs the new version.
            #[cfg(not(target_arch = "wasm32"))]
            updater.apply(false);
            settings.save();
            break;
        }
        if let Some(s) = next {
            screen = s;
        }
        next_frame().await
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn short(version: &str) -> &str {
    version.get(..7).unwrap_or(version)
}

/// Reference sheet for the body renderer: one column per mass, one row per on-screen size.
fn gallery(ui: f32) {
    clear_background(style::BACKGROUND);
    let masses = [3.0e19, 1.0e22, 6.0e24, 8.0e25, 6.0e26, 2.0e27, 4.0e28, 1.2e29, 2.5e29, 8.0e29, 2.0e30, 5.0e30, 2.0e31, -1.0e24];
    let sizes = [0.5, 3.0, 8.0, 20.0, 40.0];
    let (w, h) = (screen_width(), screen_height());
    let (dx, mut y) = (w / (masses.len() as f32 + 1.0), 70.0 * ui);
    for (row, size) in sizes.iter().enumerate() {
        let r = size * ui;
        y += r.max(8.0 * ui);
        for (col, mass) in masses.iter().enumerate() {
            let x = dx * (col as f32 + 1.0);
            style::body((x, y), r, *mass, 17 + 31 * col as u32 + 7 * row as u32, 1.0, ui, style::NO_LIGHT);
            if row == 0 {
                style::centered(&fmt::mass(*mass), x, 40.0 * ui, 12.0 * ui, style::DIM);
            }
        }
        y += r.max(8.0 * ui) + h * 0.05;
    }
    // Bases: health from full to fallen, under siege, at two sizes.
    let colours = [[230, 80, 230], [90, 200, 255], [255, 190, 60]];
    let cases = [(1.0, false), (0.6, false), (0.6, true), (0.2, true), (0.0, false)];
    for (k, size) in [8.0, 30.0].into_iter().enumerate() {
        y += 40.0 * ui;
        for (col, (health, siege)) in cases.iter().enumerate() {
            let x = w / (cases.len() as f32 + 1.0) * (col as f32 + 1.0);
            let colour = style::rgb(colours[(col + k) % colours.len()]);
            style::base((x, y), size * ui, colour, *health, *siege, col as u32, ui);
        }
        y += 40.0 * ui;
    }
    // Special bodies, with and without a hold under way.
    y += 50.0 * ui;
    for (col, (kind, held)) in gsim_proto::SpecialKind::ALL.iter().flat_map(|k| [(*k, 0.0), (*k, 0.6)]).enumerate() {
        let x = w / 7.0 * (col as f32 + 1.0);
        let r = if col % 2 == 0 { 8.0 } else { 20.0 } * ui;
        style::body((x, y), r, 6.0e24, 3 + col as u32, 1.0, ui, style::NO_LIGHT);
        style::special((x, y), r, kind, held, ui);
    }
    // Ships with a shield, and one breaking.
    y += 70.0 * ui;
    for (k, colour) in [[230u8, 80, 230], [90, 200, 255], [255, 255, 255]].into_iter().enumerate() {
        let x = w / 5.0 * (k as f32 + 1.0);
        game::draw_ship((x, y), -0.6, style::rgb(colour), k == 1, ui);
        style::ship_shield((x, y), k as u32, ui);
    }
    game::draw_ship((w * 0.8, y), 0.4, style::rgb([255, 190, 60]), false, ui);
    style::shield_break((w * 0.8, y), 0.35, ui);
    // Scrap and crystals turning, and being taken.
    y += 50.0 * ui;
    let t = get_time() as f32;
    for (k, kind) in [gsim_proto::PieceKind::Scrap, gsim_proto::PieceKind::Crystal].into_iter().enumerate() {
        for col in 0..5 {
            let x = w / 12.0 * (col as f32 + 1.0 + 6.0 * k as f32);
            style::piece((x, y), kind, t + 1.3 * col as f32, ui);
        }
        style::pickup((w / 12.0 * (6.0 * k as f32 + 5.5), y + 30.0 * ui), kind, 0.3, ui);
    }
}

/// The page's script (`web/gsim.js`) is told apart by this.
#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn gsim_crate_version() -> u32 {
    1
}

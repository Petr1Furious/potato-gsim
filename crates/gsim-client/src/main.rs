// No console window behind the game on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod fmt;
mod game;
mod predictor;
mod settings;
mod updater;

use clap::Parser;
use egui_macroquad::egui;
use game::{Game, Outcome, Solo};
use gsim_client_core::net::resolve;
use gsim_server::scenario::PRESETS;
use macroquad::prelude::*;
use settings::Settings;

/// potato-gsim client.
#[derive(Parser, Debug, Clone)]
#[command(version)]
struct Args {
    /// Join this server straight away (host or host:port)
    #[arg(long)]
    connect: Option<String>,
    /// Start a solo game straight away with this preset
    #[arg(long)]
    solo: Option<String>,
    /// Player name (overrides the saved one)
    #[arg(long)]
    name: Option<String>,
    /// World seed for solo games (0 = random)
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
    /// Show the network/sync overlay from the start (F3 toggles it)
    #[arg(long)]
    net_overlay: bool,
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
    let args = Args::parse();
    let got = gsim_core::selftest::compute();
    let ok = got == gsim_core::selftest::GOLDEN;
    if args.selftest {
        println!("self-test: computed {got:#018x}, expected {:#018x} -> {}", gsim_core::selftest::GOLDEN, if ok { "OK" } else { "MISMATCH" });
        std::process::exit(if ok { 0 } else { 2 });
    }
    if !ok {
        eprintln!("warning: simulation self-test failed; servers will refuse this build");
    }
    let mut settings = Settings::load();
    if let Some(n) = &args.name {
        settings.name = n.clone();
    }
    settings.fullscreen |= args.fullscreen;
    let conf = Conf {
        window_title: "potato-gsim".into(),
        window_width: 1400,
        window_height: 900,
        high_dpi: true,
        fullscreen: settings.fullscreen,
        sample_count: 4,
        ..Default::default()
    };
    macroquad::Window::from_config(conf, run(args, settings));
}

enum Screen {
    Menu,
    Game(Box<Game>),
}

fn seed_or_random(seed: u64) -> u64 {
    if seed != 0 {
        return seed;
    }
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64 | 1).unwrap_or(1)
}

fn join(settings: &Settings, args: &Args) -> Result<Game, String> {
    Game::connect(resolve(&settings.server)?, settings, None, args.lookahead, args.net_overlay)
}

fn solo(settings: &Settings, args: &Args) -> Result<Game, String> {
    let (server, addr) = Solo::start(&settings.preset, seed_or_random(args.seed))?;
    Game::connect(addr, settings, Some(server), args.lookahead, args.net_overlay)
}

async fn run(args: Args, mut settings: Settings) {
    let mut message = String::new();
    let updater = updater::Updater::start(!args.no_update);
    let mut screen = Screen::Menu;
    let auto = if let Some(server) = &args.connect {
        settings.server = server.clone();
        Some(join(&settings, &args))
    } else if let Some(preset) = &args.solo {
        settings.preset = preset.clone();
        Some(solo(&settings, &args))
    } else {
        None
    };
    match auto {
        Some(Ok(g)) => screen = Screen::Game(Box::new(g)),
        Some(Err(e)) => message = e,
        None => {}
    }
    let started = get_time();

    loop {
        let mut next: Option<Screen> = None;
        let mut quit = false;
        match &mut screen {
            Screen::Game(game) => match game.frame(&mut settings) {
                Outcome::Continue => {}
                Outcome::ToMenu(msg) => {
                    message = msg;
                    next = Some(Screen::Menu);
                }
                Outcome::Quit => quit = true,
            },
            Screen::Menu => {
                // Between games is the one moment a restart costs the player nothing.
                let update = updater.status();
                if update == updater::Status::Ready {
                    settings.save();
                    updater.apply(true);
                }
                clear_background(Color::from_rgba(8, 10, 16, 255));
                let mut action: Option<Result<Game, String>> = None;
                let factor = settings.ui_factor();
                egui_macroquad::ui(|ctx| {
                    ctx.set_zoom_factor(factor);
                    egui::Window::new("potato-gsim")
                        .collapsible(false)
                        .resizable(false)
                        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                        .show(ctx, |ui| {
                            ui.set_width(320.0);
                            ui.label("Newtonian gravity, shared with everyone on the server.");
                            ui.add_space(6.0);
                            egui::Grid::new("form").num_columns(2).show(ui, |ui| {
                                ui.label("Name");
                                ui.text_edit_singleline(&mut settings.name);
                                ui.end_row();
                                ui.label("Server");
                                ui.text_edit_singleline(&mut settings.server);
                                ui.end_row();
                                ui.label("UI size");
                                ui.add(egui::Slider::new(&mut settings.ui_scale, 0.6..=2.5));
                                ui.end_row();
                                ui.label("Solo world");
                                egui::ComboBox::from_id_salt("preset").selected_text(settings.preset.clone()).show_ui(ui, |ui| {
                                    for (name, about) in PRESETS {
                                        ui.selectable_value(&mut settings.preset, name.to_string(), *name).on_hover_text(*about);
                                    }
                                });
                                ui.end_row();
                            });
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                if ui.button("Join server").clicked() {
                                    action = Some(join(&settings, &args));
                                }
                                if ui.button("Play solo").clicked() {
                                    action = Some(solo(&settings, &args));
                                }
                                if ui.button("Quit").clicked() {
                                    quit = true;
                                }
                            });
                            if !message.is_empty() {
                                ui.add_space(6.0);
                                ui.colored_label(egui::Color32::from_rgb(255, 140, 120), &message);
                            }
                            let note = match &update {
                                updater::Status::Disabled => format!("build {}", short(updater::build_version())),
                                updater::Status::Checking => "checking for updates...".to_string(),
                                updater::Status::UpToDate => format!("up to date ({})", short(updater::build_version())),
                                updater::Status::Downloading { percent } => format!("downloading update... {percent} %"),
                                updater::Status::Ready => "installing update...".to_string(),
                                updater::Status::Failed(e) => e.clone(),
                            };
                            ui.add_space(4.0);
                            ui.small(note);
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
        if let Some(path) = &args.screenshot {
            if get_time() - started >= args.screenshot_after {
                get_screen_data().export_png(path);
                quit = true;
            }
        }
        if quit {
            // Install a finished download on the way out; the next start runs the new version.
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

fn short(version: &str) -> &str {
    version.get(..7).unwrap_or(version)
}

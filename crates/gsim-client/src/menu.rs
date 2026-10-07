//! The single-player screen: pick a world, tune its parameters, start.

use crate::fmt;
use crate::game::{Game, Solo};
use crate::sandbox::{group_digits, Sandbox};
use crate::settings::Settings;
use crate::style;
use egui_macroquad::egui;
use gsim_server::scenario as exact;
use gsim_swarm::scenario as large;
use std::collections::BTreeMap;
use std::thread::JoinHandle;

/// Share of a 60 Hz step the suggested body count may take, leaving room for drawing.
const BUDGET_MS: f64 = 1000.0 / 60.0 * 0.7;

/// One tunable of either kind of world.
struct Spec {
    key: &'static str,
    label: &'static str,
    help: &'static str,
    unit: &'static str,
    whole: bool,
    log: bool,
    default: f64,
    min: f64,
    max: f64,
}

struct World {
    name: &'static str,
    about: &'static str,
    /// Run by the fast approximate engine rather than the exact one.
    large: bool,
    specs: Vec<Spec>,
}

fn worlds() -> Vec<World> {
    let exact = exact::PRESETS.iter().map(|p| World {
        name: p.name,
        about: p.about,
        large: false,
        specs: p
            .params
            .iter()
            .map(|s| Spec {
                key: s.key,
                label: s.label,
                help: s.help,
                unit: s.unit,
                whole: s.kind == exact::ParamKind::Count,
                log: s.kind == exact::ParamKind::Log,
                default: s.default,
                min: s.min,
                max: s.max,
            })
            .collect(),
    });
    let large = large::SCENARIOS.iter().map(|p| World {
        name: p.name,
        about: p.about,
        large: true,
        specs: p
            .params
            .iter()
            .map(|s| Spec {
                key: s.key,
                label: s.label,
                help: s.help,
                unit: s.unit,
                whole: s.kind == large::Kind::Count || s.key == "count",
                log: s.kind == large::Kind::Log,
                default: s.default,
                min: s.min,
                max: s.max,
            })
            .collect(),
    });
    large.chain(exact).collect()
}

pub fn is_large(name: &str) -> bool {
    large::scenario(name).is_some()
}

/// The saved parameters of a world, without anything it no longer understands.
pub fn params_of(settings: &Settings, name: &str) -> BTreeMap<String, f64> {
    let Some(world) = worlds().into_iter().find(|w| w.name == name) else { return BTreeMap::new() };
    let saved = settings.params.get(name).cloned().unwrap_or_default();
    saved.into_iter().filter(|(k, v)| world.specs.iter().any(|s| s.key == k && (s.min..=s.max).contains(v))).collect()
}

pub enum Started {
    Exact(Box<Game>),
    Large(Box<Sandbox>),
}

/// Start the world selected in the settings.
pub fn launch(settings: &Settings, seed: u64, lookahead: f32) -> Result<Started, String> {
    let name = settings.preset.as_str();
    let mut params = params_of(settings, name);
    if is_large(name) {
        if let Some(count) = settings.measured.get(name).filter(|_| !params.contains_key("count")) {
            params.insert("count".into(), *count as f64);
        }
        let setup = large::build(name, seed, &params)?;
        Ok(Started::Large(Box::new(Sandbox::start(setup, seed, settings, lookahead))))
    } else {
        let (server, addr) = Solo::start(name, seed, &params)?;
        Ok(Started::Exact(Box::new(Game::connect(addr, settings, Some(server), lookahead)?)))
    }
}

pub enum Choice {
    Stay,
    Back,
    Start,
}

#[derive(Default)]
pub struct SinglePlayer {
    pub seed: String,
    /// A benchmark of this world is running.
    measuring: Option<(String, JoinHandle<Result<usize, String>>)>,
    note: String,
}

fn show_value(spec: &Spec, v: f64) -> String {
    match spec.unit {
        "kg" if v != 0.0 => fmt::mass(v),
        "m" => fmt::distance(v),
        _ if spec.whole => group_digits(v.round() as usize),
        "x" if v >= 100.0 => format!("x{}", group_digits(v.round() as usize)),
        _ if v != 0.0 && (v.abs() >= 1.0e5 || v.abs() < 1.0e-2) => format!("{v:.2e}"),
        _ => format!("{v:.2}"),
    }
}

impl SinglePlayer {
    fn measure(&mut self, settings: &Settings, name: &'static str) {
        let mut params = params_of(settings, name);
        params.remove("count");
        // Measure with as many threads as the world will get.
        let threads = crate::density::simulation_threads(settings.gpu);
        let handle = std::thread::Builder::new().name("gsim-measure".into()).spawn(move || {
            let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().map_err(|e| e.to_string())?;
            pool.install(|| gsim_swarm::bench::suggest(name, &params, BUDGET_MS))
        });
        self.measuring = handle.ok().map(|h| (name.to_string(), h));
    }

    pub fn show(&mut self, ctx: &egui::Context, settings: &mut Settings) -> Choice {
        use egui::RichText;
        let worlds = worlds();
        if !worlds.iter().any(|w| w.name == settings.preset) {
            settings.preset = "random".into();
        }
        if self.measuring.as_ref().is_some_and(|m| m.1.is_finished()) {
            let (name, handle) = self.measuring.take().unwrap();
            match handle.join() {
                Ok(Ok(count)) => {
                    settings.measured.insert(name.clone(), count);
                    settings.params.entry(name).or_default().remove("count");
                    self.note.clear();
                }
                Ok(Err(e)) => self.note = e,
                Err(_) => self.note = "the measurement crashed".into(),
            }
        }
        let dim = style::c32(style::DIM);
        let mut choice = Choice::Stay;
        egui::Window::new("SINGLE PLAYER").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.set_width(640.0);
            // A fixed height: the divider would otherwise stretch the window off the screen.
            ui.allocate_ui_with_layout(egui::vec2(640.0, 400.0), egui::Layout::left_to_right(egui::Align::TOP), |ui| {
                ui.vertical(|ui| {
                    ui.set_width(170.0);
                    for (large, heading) in [(true, "LARGE SCALE"), (false, "EXACT")] {
                        style::section(ui, heading);
                        for w in worlds.iter().filter(|w| w.large == large) {
                            ui.selectable_value(&mut settings.preset, w.name.to_string(), w.name).on_hover_text(w.about);
                        }
                    }
                });
                ui.separator();
                ui.vertical(|ui| {
                    let world = worlds.iter().find(|w| w.name == settings.preset).unwrap();
                    ui.label(RichText::new(world.name.to_uppercase()).heading().color(style::c32(style::GOLD)));
                    ui.label(world.about);
                    ui.add_space(6.0);
                    let measured = settings.measured.get(world.name).copied();
                    let saved = settings.params.entry(world.name.to_string()).or_default();
                    egui::Grid::new("params").num_columns(2).spacing([12.0, 5.0]).show(ui, |ui| {
                        for spec in &world.specs {
                            let default = if spec.key == "count" && world.large { measured.map_or(spec.default, |m| m as f64) } else { spec.default };
                            let mut value = saved.get(spec.key).copied().unwrap_or(default).clamp(spec.min, spec.max);
                            let before = value;
                            ui.label(spec.label).on_hover_text(spec.help);
                            // A logarithmic slider cannot start at zero.
                            let mut slider = egui::Slider::new(&mut value, spec.min..=spec.max).logarithmic(spec.log && spec.min > 0.0).custom_formatter(|v, _| show_value(spec, v));
                            if spec.whole {
                                slider = slider.integer();
                            }
                            ui.add(slider).on_hover_text(spec.help);
                            ui.end_row();
                            if value != before {
                                saved.insert(spec.key.to_string(), value);
                            }
                            if value == default {
                                saved.remove(spec.key);
                            }
                        }
                        ui.label("Seed").on_hover_text("The same seed gives the same starting world; empty picks one at random");
                        ui.add(egui::TextEdit::singleline(&mut self.seed).desired_width(120.0).hint_text("random"));
                        ui.end_row();
                    });
                    if world.large {
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            match (&self.measuring, measured) {
                                (Some((name, _)), _) if name == world.name => {
                                    ui.spinner();
                                    ui.label(RichText::new("measuring this machine...").color(dim));
                                }
                                (_, Some(count)) => {
                                    ui.label(RichText::new(format!("this machine holds about {} bodies at full speed", group_digits(count))).color(dim));
                                    if ui.small_button("measure again").clicked() {
                                        self.measure(settings, world.name);
                                    }
                                }
                                _ => {
                                    if ui.small_button("measure this machine").clicked() {
                                        self.measure(settings, world.name);
                                    }
                                }
                            }
                        });
                    }
                    if !self.note.is_empty() {
                        ui.colored_label(style::c32(style::EMBER), &self.note);
                    }
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Start").clicked() {
                    choice = Choice::Start;
                }
                if ui.button("Defaults").clicked() {
                    settings.params.remove(&settings.preset);
                }
                if ui.button("Back").clicked() {
                    choice = Choice::Back;
                }
            });
        });
        choice
    }

    /// 0 = pick one from the clock.
    pub fn seed(&self) -> u64 {
        self.seed.trim().parse().unwrap_or(0)
    }
}

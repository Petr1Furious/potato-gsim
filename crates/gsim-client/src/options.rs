//! Everything that is kept between sessions, shown the same way in the main menu's settings
//! and in the menus of both kinds of world.

use crate::density::ColorMode;
use crate::settings::{Camera, Settings};
use crate::style;
use egui_macroquad::egui;

/// Which entries apply: some only mean something in one kind of world.
#[derive(Clone, Copy, PartialEq)]
pub enum World {
    /// The main menu: show everything.
    Any,
    Exact,
    Large,
}

pub fn show(ui: &mut egui::Ui, settings: &mut Settings, world: World) {
    ui.spacing_mut().slider_width = 190.0;
    let (exact, large) = (world != World::Large, world != World::Exact);
    style::section(ui, "VIEW");
    if exact {
        style::toggle(ui, &mut settings.prediction, "Ship trajectory", "P").on_hover_text("Where your ship is heading, and its closest approach to the selected body.");
        style::toggle(ui, &mut settings.shell_prediction, "Shell trajectory preview", "O").on_hover_text("The path a shell would take if you fired at the cursor now.");
        style::toggle(ui, &mut settings.trails, "Trails", "L").on_hover_text("Where bodies and ships have just been.");
        style::toggle(ui, &mut settings.trails_relative, "Trails relative to selection", "K")
            .on_hover_text("Trails as seen from the selected body: an orbit around it draws a loop.");
        style::toggle(ui, &mut settings.body_names, "Body names", "N").on_hover_text("Names next to named bodies.");
    }
    if world == World::Large {
        let mut follow = settings.camera != Camera::Ship;
        style::toggle(ui, &mut follow, "Camera follows selection", "F").on_hover_text("On: the camera stays with the selection. Off: it stays where it is.");
        if follow != (settings.camera != Camera::Ship) {
            settings.camera = if follow { Camera::Selection } else { Camera::Ship };
        }
    } else {
        ui.horizontal(|ui| {
            ui.label("Camera follows");
            for mode in Camera::ALL {
                ui.selectable_value(&mut settings.camera, mode, mode.name());
            }
            style::key_hint(ui, "F");
        })
        .response
        .on_hover_text(
            "Ship: stays with your ship. Selection: stays with the selected body. \
             Auto: as Selection, and it moves and zooms to keep your ship, its path and the selected body in view.",
        );
    }
    let details = match world {
        World::Any => "Details panel",
        World::Exact => "Network details",
        World::Large => "Statistics",
    };
    style::toggle(ui, &mut settings.details, details, "F3").on_hover_text("Frame rate, step times and other figures. In the sandbox, F3 + 1 to 4 shows more.");
    if large {
        style::toggle(ui, &mut settings.long_exposure, "Long exposure", "X")
            .on_hover_text("Sandbox: light lingers and fades, so orbits draw streaks.");
    }
    ui.horizontal(|ui| {
        ui.label("Colour shows");
        for mode in ColorMode::ALL {
            // Exact worlds have no groups to tell apart.
            if large || mode != ColorMode::Origin {
                ui.selectable_value(&mut settings.color, mode, mode.name());
            }
        }
        style::key_hint(ui, "C");
    })
    .response
    .on_hover_text("What colour shows: mass, speed, or (sandbox) the galaxy or cloud a body started in.");

    style::section(ui, "INTERFACE");
    ui.horizontal(|ui| {
        // A text field, not a slider: a slider would move under the pointer as the
        // interface it is part of changes size.
        ui.add(egui::DragValue::new(&mut settings.ui_scale).range(0.6..=2.5).speed(0.0).fixed_decimals(2).update_while_editing(false));
        ui.label("size");
    })
    .response
    .on_hover_text(
        "Size of menus, panels and text. Default 1.2.",
    );
    ui.add(egui::Slider::new(&mut settings.zoom_speed, 0.002..=3.0).logarithmic(true).text("zoom speed")).on_hover_text(
        "How far one wheel step zooms. Trackpads want a much lower value.",
    );
    if exact {
        style::section(ui, "SHIP");
        ui.horizontal(|ui| {
            ship_color(ui, settings);
            ui.label("ship colour");
        })
        .response
        .on_hover_text("Colour of your ship, name and shells, as everyone sees them. Dark colours are lightened.");
        ui.add(egui::Slider::new(&mut settings.burn_preview, 0.5..=20.0).suffix(" km/s").text("burn preview")).on_hover_text(
            "While thrusting: the yellow line shows your path if the burn went on for this much delta-v.",
        );
        style::toggle(ui, &mut settings.mouse_aim, "Mouse aim", "M").on_hover_text(
        "On: the ship points at the cursor. Off: A and D turn it.",
    );
    }
    if world == World::Any {
        player_key(ui, settings);
    }
    style::section(ui, "GRAPHICS");
    ui.checkbox(&mut settings.gpu, "GPU drawing").on_hover_text(
        "On: the graphics card draws the small bodies, and more processor threads go to the simulation. Off: the processor draws them.",
    );

    style::section(ui, "CONTROLS");
    if exact {
        if world == World::Any {
            style::caption(ui, "WITH A SHIP");
        }
        style::key_row(ui, "Thrust", "W / UP");
        style::key_row(ui, "Throttle, cut, full", "SHIFT / CTRL, X, Z");
        style::key_row(ui, "Fire towards cursor", "SPACE");
        style::key_row(ui, "Zoom, pan, select", "WHEEL, DRAG, CLICK");
        style::key_row(ui, "Chat, command", "T, /");
        style::key_row(ui, "Point at the map", "G");
        style::key_row(ui, "Self-destruct (hold)", "BACKSPACE");
    }
    if large {
        if world == World::Any {
            style::caption(ui, "SANDBOX WORLDS");
        }
        style::key_row(ui, "Move the view", "DRAG, W A S D");
        style::key_row(ui, "Zoom", "WHEEL, Q / E");
        style::key_row(ui, "Tools", "1 - 6");
        style::key_row(ui, "Select more bodies", "SHIFT + CLICK");
        style::key_row(ui, "Pan with a tool", "RIGHT DRAG");
        style::key_row(ui, "Pause", "SPACE");
        style::key_row(ui, "Slower, faster", "< >");
        style::key_row(ui, "Delete the selection", "DEL");
        style::key_row(ui, "Detail pages", "F3 + 1 - 4");
    }
    style::key_row(ui, "Fullscreen", "F11");
    style::section(ui, "BODIES");
    style::mass_legend(ui);
}

#[cfg(target_arch = "wasm32")]
extern "C" {
    /// Put text on the clipboard, through the page's script (`web/gsim.js`).
    fn gsim_copy(ptr: *const u8, len: usize);
}

fn copy(ui: &egui::Ui, text: &str) {
    // (A browser has the page put it there, not the interface library.)
    #[cfg(target_arch = "wasm32")]
    {
        let _ = ui;
        // SAFETY: the script reads the text during the call.
        unsafe { gsim_copy(text.as_ptr(), text.len()) };
    }
    #[cfg(not(target_arch = "wasm32"))]
    ui.ctx().copy_text(text.to_string());
}

/// The player's key: what makes them the same player in the program and in a browser, or on
/// another machine. Copied out of one and pasted into the other.
fn player_key(ui: &mut egui::Ui, settings: &Settings) {
    use gsim_proto::identity::{fingerprint, from_hex, to_hex};
    style::section(ui, "PLAYER KEY");
    // (What is shown and what has been typed, kept from frame to frame.)
    let (shown_id, entry_id) = (ui.id().with("key shown"), ui.id().with("key entry"));
    let shown = ui.data_mut(|d| d.get_temp::<String>(shown_id)).unwrap_or_else(|| fingerprint(&settings.identity().public()));
    let mut entry = ui.data_mut(|d| d.get_temp::<String>(entry_id)).unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label(format!("key {shown}")).on_hover_text(
            "Your names on servers belong to this key. Copy it into the game elsewhere (the program, a browser) to play there as the same player.",
        );
        if ui.small_button("Copy").on_hover_text("Anyone with your key can play as you: keep it to yourself.").clicked() {
            copy(ui, &to_hex(&settings.identity().secret()));
        }
    });
    let valid = from_hex(&entry).is_some();
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut entry).desired_width(200.0).hint_text("paste a key here"));
        if ui.add_enabled(valid, egui::Button::new("Use")).on_hover_text("Play as the player that key belongs to, from the next game on. Your current key is replaced.").clicked() && settings.adopt_identity(&entry) {
            entry.clear();
            ui.data_mut(|d| d.remove::<String>(shown_id));
            return;
        }
        ui.data_mut(|d| d.insert_temp(shown_id, shown.clone()));
    });
    ui.data_mut(|d| d.insert_temp(entry_id, entry));
}

/// The button that opens the picker for the ship's colour.
pub fn ship_color(ui: &mut egui::Ui, settings: &mut Settings) -> egui::Response {
    let button = ui.color_edit_button_srgb(&mut settings.ship_color);
    settings.ship_color = gsim_proto::visible_color(settings.ship_color);
    button
}

/// A menu body that scrolls when the window is too short for it, leaving room for the
/// title above and a row of buttons below.
pub fn scrolled(ui: &mut egui::Ui, body: impl FnOnce(&mut egui::Ui)) {
    let room = (ui.ctx().screen_rect().height() - 130.0).max(120.0);
    // A bar with a lane of its own: the floating kind is drawn over the right end of the rows.
    ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
    // (As tall as that from the first frame, if there is that much to show: left to find
    // its height by itself in a window that fits its contents, it gains a row a frame.)
    egui::ScrollArea::vertical().max_height(room).min_scrolled_height(room).show(ui, body);
}

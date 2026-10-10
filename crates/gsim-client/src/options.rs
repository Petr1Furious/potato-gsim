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
        style::toggle(ui, &mut settings.prediction, "Ship trajectory", "P").on_hover_text("The green line ahead of your ship: where it will go if you do nothing, and where it comes closest to the selected body.");
        style::toggle(ui, &mut settings.shell_prediction, "Shell trajectory preview", "O").on_hover_text("The path a shell would take if you fired at the cursor now.");
        style::toggle(ui, &mut settings.trails, "Trails", "L").on_hover_text("Where every body and ship has been over the last few seconds.");
        style::toggle(ui, &mut settings.trails_relative, "Trails relative to selection", "K")
            .on_hover_text("Draw trails as seen from the selected body, so that an orbit around it looks like a loop rather than a wave.");
        style::toggle(ui, &mut settings.body_names, "Body names", "N").on_hover_text("Names next to named bodies, and as the first line of the selected body's label.");
    }
    if world == World::Large {
        let mut follow = settings.camera != Camera::Ship;
        style::toggle(ui, &mut follow, "Camera follows selection", "F").on_hover_text("On: the camera stays with the body you clicked. Off: it stays where it is, and a selected body is only marked.");
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
            "Ship: the camera stays with your ship, and a selected body is only marked. Selection: it stays with the body you clicked. \
             Auto: it stays with the selection too, and moves and zooms by itself whenever your ship, its predicted path (as far as the closest approach to the selected body) or the selected body \
             would leave the screen, or have become small; it holds still while you thrust, and a view you set by hand (drag or wheel) is left alone for a few seconds.",
        );
    }
    let details = match world {
        World::Any => "Details panel",
        World::Exact => "Network details",
        World::Large => "Statistics",
    };
    style::toggle(ui, &mut settings.details, details, "F3").on_hover_text("Numbers for the curious: frame rate, step times, and how well this client is keeping in step.");
    if large {
        style::toggle(ui, &mut settings.long_exposure, "Long exposure", "X")
            .on_hover_text("In sandbox worlds, light lingers for a few seconds and fades: orbits draw themselves as streaks. Moving the view starts it afresh.");
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
    .on_hover_text("What the colour of small bodies tells: their mass, how fast they move, or (in sandbox worlds) which galaxy or cloud they started in.");

    style::section(ui, "INTERFACE");
    ui.horizontal(|ui| {
        // A text field, not a slider: a slider would move under the pointer as the
        // interface it is part of changes size.
        ui.add(egui::DragValue::new(&mut settings.ui_scale).range(0.6..=2.5).speed(0.0).fixed_decimals(2).update_while_editing(false));
        ui.label("size");
    })
    .response
    .on_hover_text(
        "How large menus, panels and text are drawn, on top of the automatic scaling with the window's height. \
         1.2 is the default; type a value from 0.6 to 2.5 and press Enter.",
    );
    ui.add(egui::Slider::new(&mut settings.zoom_speed, 0.002..=3.0).logarithmic(true).text("zoom speed")).on_hover_text(
        "How far one step of the mouse wheel or trackpad zooms the map. Trackpads send many small steps, \
         so they usually want a much lower value than a wheel.",
    );
    if exact {
        style::section(ui, "FLYING");
        ui.horizontal(|ui| {
            ship_color(ui, settings);
            ui.label("ship colour");
        })
        .response
        .on_hover_text("The colour of your ship, its name and its shells, for you and for everyone else on a server. Colours too dark to see against the sky are lightened.");
        ui.add(egui::Slider::new(&mut settings.burn_preview, 0.5..=20.0).suffix(" km/s").text("burn preview")).on_hover_text(
            "While you thrust, a yellow line shows where the ship would go if you kept thrusting like this. \
             It assumes the burn stops once it has spent this much delta-v, and that the ship coasts from there.",
        );
        style::toggle(ui, &mut settings.mouse_aim, "Mouse aim", "M").on_hover_text(
        "On: the ship points at the cursor. Off: A and D turn it, and the cursor is free for looking around. \
         Thrust always goes where the ship points.",
        );
    }
    style::section(ui, "GRAPHICS");
    ui.checkbox(&mut settings.gpu, "GPU drawing").on_hover_text(
        "On: the graphics card draws the glowing picture of the small bodies, at the screen's full \
         resolution, and the processor threads that would have drawn it go to the simulation (which in \
         sandbox worlds means more bodies at full speed). Off: those threads draw it. If the card \
         cannot do it, the game says so and switches this off.",
    );

    style::section(ui, "CONTROLS");
    if exact {
        if world == World::Any {
            style::caption(ui, "EXACT WORLDS");
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
        style::key_row(ui, "Pan with a tool in hand", "RIGHT DRAG");
        style::key_row(ui, "Pause", "SPACE");
        style::key_row(ui, "Slow time down, speed it up", "< >");
        style::key_row(ui, "Delete the selected body", "DEL");
    }
    style::key_row(ui, "Fullscreen", "F11");
    style::section(ui, "MASS");
    style::mass_legend(ui);
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
    egui::ScrollArea::vertical().max_height(room).show(ui, body);
}

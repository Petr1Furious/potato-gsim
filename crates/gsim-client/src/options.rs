//! The settings that are kept between sessions, shown the same way in the main menu and in
//! the menus of both kinds of world.

use crate::settings::Settings;
use crate::style;
use egui_macroquad::egui;

pub fn show(ui: &mut egui::Ui, settings: &mut Settings) {
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
    style::section(ui, "FLYING");
    style::toggle(ui, &mut settings.mouse_aim, "Mouse aim", "M").on_hover_text(
        "On: the ship points at the cursor. Off: A and D turn it, and the cursor is free for looking around. \
         Thrust always goes where the ship points.",
    );
    style::toggle(ui, &mut settings.auto_zoom, "Auto zoom", "").on_hover_text(
        "While a body is selected and on screen, the view zooms by itself to keep your ship in the picture: \
         out as the ship nears the edge or is teleported away, back in when it is close to the body. \
         The body stays where you put it. Using the wheel pauses this for a moment; dragging the body off \
         screen stops it.",
    );
    style::section(ui, "GRAPHICS");
    ui.checkbox(&mut settings.gpu, "GPU drawing").on_hover_text(
        "On: the graphics card draws the glowing picture of the small bodies, at the screen's full \
         resolution, and the processor threads that would have drawn it go to the simulation (which in \
         large-scale worlds means more bodies at full speed). Off: those threads draw it. If the card \
         cannot do it, the game says so and switches this off.",
    );
}

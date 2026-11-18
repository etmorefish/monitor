//! The optional egui shell for minitor.
//!
//! Built only with `--features gui`, so the default binary stays dependency
//! free. Sampling, formatting and the data model all come from the library.

pub mod app;
pub mod font;
pub mod chart;
pub mod theme;

use eframe::egui;

pub fn run() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1320.0, 830.0])
            .with_min_inner_size([1000.0, 660.0])
            .with_title("minitor"),
        // Vsync stays on: with it off, eframe renders unbounded whenever a
        // repaint is pending, which idles at 20-40% instead of 0.6%.
        glow_options: eframe::egui_glow::GlowConfiguration {
            vsync: true,
            ..Default::default()
        },
        ..Default::default()
    };

    eframe::run_native(
        "minitor",
        options,
        Box::new(|cc| {
            font::install(&cc.egui_ctx);
            style(&cc.egui_ctx);
            Ok(Box::new(app::Dashboard::new()))
        }),
    )
}

fn style(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(6.0, 6.0);
        style.spacing.button_padding = egui::vec2(9.0, 3.0);
        style.text_styles = [
            (egui::TextStyle::Body, egui::FontId::proportional(12.5)),
            (egui::TextStyle::Button, egui::FontId::proportional(12.0)),
            (egui::TextStyle::Small, egui::FontId::proportional(10.5)),
            (egui::TextStyle::Heading, egui::FontId::proportional(16.0)),
            (egui::TextStyle::Monospace, egui::FontId::monospace(11.5)),
        ]
        .into();
    });
    theme::Theme::dark().apply(ctx);
}

//! The optional egui shell for minitor.
//!
//! Built only with `--features gui`, so the default binary stays dependency
//! free. Sampling, formatting and the data model all come from the library.

pub mod app;
pub mod chart;
pub mod theme;

use eframe::egui;

pub fn run() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1320.0, 830.0])
            .with_min_inner_size([1000.0, 660.0])
            .with_title("minitor"),
        ..Default::default()
    };

    eframe::run_native(
        "minitor",
        options,
        Box::new(|cc| {
            install_fonts(&cc.egui_ctx);
            style(&cc.egui_ctx);
            Ok(Box::new(app::Dashboard::new()))
        }),
    )
}

/// Windows ships a CJK font; egui's built-in fonts are Latin-only, so Chinese
/// labels would render as boxes without this. Missing font simply means the
/// default typeface stays in place.
fn install_fonts(ctx: &egui::Context) {
    for path in [
        r"C:\Windows\Fonts\msyh.ttc",
        r"C:\Windows\Fonts\msyh.ttf",
        r"C:\Windows\Fonts\simhei.ttf",
        r"C:\Windows\Fonts\simsun.ttc",
    ] {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "cjk".to_owned(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .push("cjk".to_owned());
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push("cjk".to_owned());
        ctx.set_fonts(fonts);
        return;
    }
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

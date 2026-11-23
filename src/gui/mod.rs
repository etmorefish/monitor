//! The optional egui shell for monitor.
//!
//! Built only with `--features gui`, so the default binary stays dependency
//! free. Sampling, formatting and the data model all come from the library.

pub mod app;
pub mod font;
pub mod icons;
pub mod chart;
pub mod theme;
pub mod throttle;

use eframe::egui;

pub fn run() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1320.0, 830.0])
            .with_min_inner_size([1000.0, 660.0])
            .with_title("monitor"),
        // `vsync: false` on purpose, together with `throttle::wait_for_vblank`
        // in `Dashboard::ui`: with vsync on, the Intel Arc OpenGL ICD busy-waits
        // inside `SwapBuffers` (~16 ms of CPU per frame), so even 1 fps idle
        // frames spin. DwmFlush re-adds the pacing as a proper kernel wait
        // without the spin. Input is throttled separately, so there is no
        // unbounded rendering (see docs/高帧率重绘问题.md).
        glow_options: eframe::egui_glow::GlowConfiguration {
            vsync: false,
            ..Default::default()
        },
        ..Default::default()
    };

    eframe::run_native(
        "monitor",
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

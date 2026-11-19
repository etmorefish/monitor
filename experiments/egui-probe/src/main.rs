use eframe::egui;
use std::time::Duration;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([520.0, 360.0])
            .with_position([300.0, 300.0])
            .with_title("monitor egui probe"),
        ..Default::default()
    };
    eframe::run_native(
        "monitor egui probe",
        options,
        Box::new(|cc| {
            install_cjk_font(&cc.egui_ctx);
            Ok(Box::new(Probe::default()))
        }),
    )
}

fn install_cjk_font(ctx: &egui::Context) {
    if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\msyh.ttc") {
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
        ctx.set_fonts(fonts);
    }
}

fn mode() -> String {
    std::env::var("MODE").unwrap_or_else(|_| "labels".to_owned())
}

#[derive(Default)]
struct Probe {
    frame: u64,
}

impl eframe::App for Probe {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.frame += 1;
        let ctx = ui.ctx().clone();
        let mode = mode();
        egui::Frame::central_panel(ui.style()).show(ui, |ui| {
            ui.heading("monitor 外壳探针");
            ui.label("CPU 12.4%   内存 66.5%   温度 95°C");
            if mode == "progress" {
                ui.add(egui::ProgressBar::new(0.665).text("内存 21.0 / 31.5 GB"));
                ui.add(egui::ProgressBar::new(0.744).text("C: 131 / 177 GB"));
            }
            ui.label(format!("第 {} 帧", self.frame));
        });
        match mode.as_str() {
            "event" => {}
            "repaint" => ctx.request_repaint(),
            _ => ctx.request_repaint_after(Duration::from_secs(1)),
        }
    }
}

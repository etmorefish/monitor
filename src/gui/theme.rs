//! Dashboard palette. RGB rather than 256-colour indices, but the same
//! semantics as the TUI so both shells read alike.

use eframe::egui::{self, Color32};

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

#[derive(Clone, Copy)]
pub struct Theme {
    pub dark: bool,
    pub bg: Color32,
    pub card: Color32,
    pub border: Color32,
    pub grid: Color32,
    pub text: Color32,
    pub dim: Color32,
    pub faint: Color32,
    pub accent: Color32,
    pub cpu: Color32,
    pub memory: Color32,
    pub disk: Color32,
    pub net: Color32,
    pub temp: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub bad: Color32,
    pub battery: Color32,
    pub tooltip: Color32,
    /// Distinct pairs: the old disk/net colours were too close to tell apart.
    pub io_read: Color32,
    pub io_write: Color32,
    pub net_rx: Color32,
    pub net_tx: Color32,
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            dark: true,
            bg: rgb(10, 14, 20),
            card: rgb(18, 26, 35),
            border: rgb(31, 42, 55),
            grid: rgb(26, 36, 46),
            text: rgb(230, 237, 243),
            dim: rgb(147, 164, 184),
            faint: rgb(86, 104, 122),
            accent: rgb(79, 156, 249),
            cpu: rgb(79, 143, 247),
            memory: rgb(45, 212, 191),
            disk: rgb(251, 191, 36),
            net: rgb(34, 211, 238),
            temp: rgb(251, 113, 133),
            ok: rgb(52, 211, 153),
            warn: rgb(251, 191, 36),
            bad: rgb(248, 113, 113),
            battery: rgb(163, 230, 53),
            tooltip: rgb(24, 34, 46),
            io_read: rgb(102, 187, 106),
            io_write: rgb(255, 167, 38),
            net_rx: rgb(79, 195, 247),
            net_tx: rgb(255, 112, 67),
        }
    }

    pub fn light() -> Self {
        Self {
            dark: false,
            bg: rgb(244, 247, 251),
            card: rgb(255, 255, 255),
            border: rgb(226, 232, 240),
            grid: rgb(237, 242, 247),
            text: rgb(15, 23, 42),
            dim: rgb(100, 116, 139),
            faint: rgb(148, 163, 184),
            accent: rgb(37, 99, 235),
            cpu: rgb(37, 99, 235),
            memory: rgb(13, 148, 136),
            disk: rgb(217, 119, 6),
            net: rgb(8, 145, 178),
            temp: rgb(225, 29, 72),
            ok: rgb(5, 150, 105),
            warn: rgb(217, 119, 6),
            bad: rgb(220, 38, 38),
            battery: rgb(101, 163, 13),
            tooltip: rgb(255, 255, 255),
            io_read: rgb(46, 125, 50),
            io_write: rgb(245, 127, 23),
            net_rx: rgb(25, 118, 210),
            net_tx: rgb(230, 74, 25),
        }
    }

    /// Load-based colour: calm until it matters.
    pub fn load(&self, percent: f32) -> Color32 {
        if percent >= 85.0 {
            self.bad
        } else if percent >= 60.0 {
            self.warn
        } else {
            self.ok
        }
    }

    pub fn temperature(&self, celsius: f32) -> Color32 {
        if celsius >= 90.0 {
            self.bad
        } else if celsius >= 75.0 {
            self.warn
        } else {
            self.ok
        }
    }

    /// Colour ramp for a core's current clock: cool when idle, warm near turbo.
    ///
    /// The reference is the package base clock, so the same core reads blue at
    /// 0.5× base and red at 3× base regardless of the machine.
    pub fn frequency(&self, mhz: u32, base_mhz: u32) -> Color32 {
        let base = base_mhz.max(800) as f32;
        let ratio = (mhz as f32 / base).clamp(0.3, 3.2);
        let t = ((ratio - 0.3) / 2.9).clamp(0.0, 1.0);
        let stops: [(f32, Color32); 5] = if self.dark {
            [
                (0.0, rgb(59, 130, 246)),
                (0.35, rgb(34, 211, 238)),
                (0.60, rgb(52, 211, 153)),
                (0.82, rgb(251, 191, 36)),
                (1.0, rgb(239, 68, 68)),
            ]
        } else {
            [
                (0.0, rgb(37, 99, 235)),
                (0.35, rgb(8, 145, 178)),
                (0.60, rgb(5, 150, 105)),
                (0.82, rgb(217, 119, 6)),
                (1.0, rgb(220, 38, 38)),
            ]
        };
        ramp(t, &stops)
    }

    pub fn alpha(color: Color32, alpha: u8) -> Color32 {
        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
    }

    /// Push the palette into egui's global visuals (buttons, selections,
    /// scrollbars) so widgets match the hand-painted cards.
    pub fn apply(&self, ctx: &egui::Context) {
        ctx.set_theme(if self.dark {
            egui::ThemePreference::Dark
        } else {
            egui::ThemePreference::Light
        });
        let mut visuals = if self.dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        visuals.panel_fill = self.bg;
        visuals.window_fill = self.card;
        visuals.extreme_bg_color = self.card;
        visuals.faint_bg_color = self.grid;
        visuals.selection.bg_fill = self.accent;
        visuals.selection.stroke = egui::Stroke::new(1.0, Color32::WHITE);
        visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, self.text);
        visuals.widgets.inactive.weak_bg_fill = self.grid;
        visuals.widgets.inactive.bg_fill = self.grid;
        visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, self.text);
        visuals.widgets.hovered.weak_bg_fill = Theme::alpha(self.accent, 40);
        visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, self.accent);
        visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, self.text);
        visuals.widgets.active.weak_bg_fill = Theme::alpha(self.accent, 70);
        visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, self.text);
        visuals.override_text_color = None;
        ctx.set_visuals(visuals);
    }
}
/// Piecewise-linear colour ramp over `stops` (sorted by position).
fn ramp(t: f32, stops: &[(f32, Color32)]) -> Color32 {
    let mut previous = stops[0];
    for stop in stops {
        if t <= stop.0 {
            let span = (stop.0 - previous.0).max(0.0001);
            let k = ((t - previous.0) / span).clamp(0.0, 1.0);
            let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * k).round() as u8;
            return Color32::from_rgb(
                mix(previous.1.r(), stop.1.r()),
                mix(previous.1.g(), stop.1.g()),
                mix(previous.1.b(), stop.1.b()),
            );
        }
        previous = *stop;
    }
    stops[stops.len() - 1].1
}
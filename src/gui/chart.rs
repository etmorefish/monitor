//! Hand-drawn charts.
//!
//! A monitoring dashboard needs four shapes: a line/area chart, a ring, a
//! horizontal bar and a row of small bars. That is a few dozen lines of
//! painter calls, so no plotting crate is pulled in.

use eframe::egui::{
    self, pos2, vec2, Align2, Color32, FontId, Id, Mesh, Painter, Pos2, Rect, Sense, Shape, Stroke,
    Ui,
};

use super::theme::Theme;
use crate::format;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Percent,
    Celsius,
    BytesPerSec,
}

pub fn format_value(value: f32, unit: Unit) -> String {
    match unit {
        Unit::Percent => format!("{value:.0}%"),
        Unit::Celsius => format!("{value:.0}°C"),
        Unit::BytesPerSec => format::rate(value as f64),
    }
}

/// Round a peak up so the axis maximum is a readable number.
pub fn nice_max(peak: f32, floor: f32) -> f32 {
    let peak = peak.max(floor);
    if !peak.is_finite() || peak <= 0.0 {
        return floor;
    }
    let magnitude = 10f32.powf(peak.log10().floor());
    let normalized = peak / magnitude;
    // Fine ladder: a 103 °C peak should become a 120 °C axis, not 200.
    const STEPS: [f32; 10] = [1.0, 1.2, 1.5, 2.0, 2.5, 3.0, 4.0, 5.0, 6.0, 8.0];
    let step = STEPS
        .iter()
        .copied()
        .find(|step| *step >= normalized)
        .unwrap_or(10.0);
    step * magnitude
}

/// Smoothed axis maximum: follows the recent peak so an idle chart is not
/// pinned to the ceiling, and eases instead of jumping when load changes.
pub struct AutoScale {
    max: f32,
}

impl Default for AutoScale {
    fn default() -> Self {
        Self { max: 0.0 }
    }
}

impl AutoScale {
    pub fn update(&mut self, peak: f32, floor: f32) -> f32 {
        let target = nice_max(peak * 1.15, floor);
        if self.max <= 0.0 {
            self.max = target;
        } else {
            self.max += (target - self.max) * 0.3;
        }
        self.max
    }
}

/// Quantile of the visible window, ignoring `NaN`.
pub fn percentile(values: &[f32], samples: usize, quantile: f32) -> f32 {
    let start = values.len().saturating_sub(samples);
    let mut visible: Vec<f32> = values[start..]
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .collect();
    if visible.is_empty() {
        return 0.0;
    }
    visible.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let index = ((visible.len() - 1) as f32 * quantile).round() as usize;
    visible[index]
}

pub struct Series<'a> {
    pub values: &'a [f32],
    pub color: Color32,
    pub label: &'a str,
}

pub struct LineChart<'a> {
    pub series: &'a [Series<'a>],
    /// Axis floor: temperature plots 30–110 °C instead of 0–200 °C.
    pub min: f32,
    pub max: f32,
    pub unit: Unit,
    pub samples: usize,
    pub interval: f64,
    /// Seconds since local midnight, for the hover clock.
    pub now_secs: u32,
    pub fill: bool,
}

pub fn line_chart(ui: &mut Ui, rect: Rect, theme: &Theme, chart: &LineChart, id: Id) {
    if rect.width() < 40.0 || rect.height() < 40.0 {
        return;
    }
    let painter = ui.painter_at(rect);
    let response = ui.interact(rect, id, Sense::hover());
    let plot = Rect::from_min_max(
        pos2(rect.left() + 46.0, rect.top() + 6.0),
        pos2(rect.right() - 8.0, rect.bottom() - 17.0),
    );
    if plot.width() < 16.0 || plot.height() < 16.0 {
        return;
    }

    // Fewer grid lines in short cards, otherwise the labels collide.
    let divisions = ((plot.height() / 34.0).floor() as usize).clamp(2, 4);
    let label_font = FontId::proportional(if plot.height() < 70.0 { 9.0 } else { 10.0 });
    for step in 0..=divisions {
        let fraction = step as f32 / divisions as f32;
        let y = plot.bottom() - fraction * plot.height();
        painter.hline(plot.x_range(), y, Stroke::new(1.0, theme.grid));
        painter.text(
            pos2(plot.left() - 7.0, y),
            Align2::RIGHT_CENTER,
            format_value(chart.min + (chart.max - chart.min) * fraction, chart.unit),
            label_font.clone(),
            theme.faint,
        );
    }

    let window_seconds = chart.samples as f64 * chart.interval;
    if plot.bottom() + 9.0 < rect.bottom() - 2.0 {
        painter.text(
            pos2(plot.left(), plot.bottom() + 9.0),
            Align2::LEFT_CENTER,
            format!("-{}", short_duration(window_seconds)),
            label_font.clone(),
            theme.faint,
        );
    }
    painter.text(
        pos2(plot.right(), plot.bottom() + 9.0),
        Align2::RIGHT_CENTER,
        "现在",
        label_font,
        theme.faint,
    );

    let longest = chart
        .series
        .iter()
        .map(|series| series.values.len())
        .max()
        .unwrap_or(0);
    let count = chart.samples.min(longest);
    if count < 2 {
        return;
    }

    for (index, series) in chart.series.iter().enumerate() {
        let start = series.values.len().saturating_sub(count);
        let values = &series.values[start..];
        let mut points = Vec::with_capacity(values.len());
        for (position, value) in values.iter().enumerate() {
            if !value.is_finite() {
                continue;
            }
            let x = plot.left() + (position as f32 / (count - 1) as f32) * plot.width();
            let normalized = (value - chart.min) / (chart.max - chart.min).max(0.001);
            let y = plot.bottom() - normalized.clamp(0.0, 1.0) * plot.height();
            points.push(pos2(x, y));
        }
        if points.len() < 2 {
            continue;
        }
        if index == 0 && chart.fill {
            area(&painter, &points, plot.bottom(), series.color);
        }
        painter.add(Shape::line(points, Stroke::new(1.8, series.color)));
    }

    if response.hovered() {
        if let Some(pointer) = response.hover_pos() {
            if plot.contains(pointer) {
                let fraction = ((pointer.x - plot.left()) / plot.width()).clamp(0.0, 1.0);
                let index = (fraction * (count - 1) as f32).round() as usize;
                let x = plot.left() + index as f32 / (count - 1) as f32 * plot.width();
                painter.vline(x, plot.y_range(), Stroke::new(1.0, Theme::alpha(theme.dim, 110)));

                let mut rows: Vec<(String, String, Color32)> = Vec::new();
                for series in chart.series {
                    let start = series.values.len().saturating_sub(count);
                    if let Some(value) = series.values.get(start + index) {
                        if !value.is_finite() {
                            continue;
                        }
                        let normalized = (value - chart.min) / (chart.max - chart.min).max(0.001);
                        let y = plot.bottom() - normalized.clamp(0.0, 1.0) * plot.height();
                        painter.circle_filled(pos2(x, y), 3.0, series.color);
                        rows.push((
                            series.label.to_string(),
                            format_value(*value, chart.unit),
                            series.color,
                        ));
                    }
                }
                let offset = (count - 1 - index) as f32 * chart.interval as f32;
                let title = clock(chart.now_secs, offset);
                tooltip(&painter, rect, pointer, &title, &rows, theme);
            }
        }
    }
}

/// Area under a series, as a triangle strip to the baseline.
fn area(painter: &Painter, points: &[Pos2], baseline: f32, color: Color32) {
    let mut mesh = Mesh::default();
    let top = Theme::alpha(color, 70);
    let bottom = Theme::alpha(color, 4);
    for point in points {
        mesh.colored_vertex(*point, top);
        mesh.colored_vertex(pos2(point.x, baseline), bottom);
    }
    for index in 0..points.len() - 1 {
        let base = index as u32 * 2;
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base + 1, base + 2, base + 3);
    }
    painter.add(Shape::mesh(mesh));
}

/// Ring made of overlapping arc segments; `segments` are fractions summing to 1.
pub fn ring(
    painter: &Painter,
    center: Pos2,
    radius: f32,
    thickness: f32,
    segments: &[(f32, Color32)],
    track: Color32,
) {
    arc(painter, center, radius, thickness, 0.0, std::f32::consts::TAU, track);
    let mut start = -std::f32::consts::FRAC_PI_2;
    for (fraction, color) in segments {
        if *fraction <= 0.0 {
            continue;
        }
        let sweep = fraction.min(1.0) * std::f32::consts::TAU;
        arc(painter, center, radius, thickness, start, sweep, *color);
        start += sweep;
    }
}

fn arc(
    painter: &Painter,
    center: Pos2,
    radius: f32,
    thickness: f32,
    start: f32,
    sweep: f32,
    color: Color32,
) {
    if sweep.abs() < 0.001 {
        return;
    }
    let steps = ((sweep.abs() / 0.05).ceil() as usize).max(2);
    let mut previous = center + vec2(start.cos(), start.sin()) * radius;
    for step in 1..=steps {
        let angle = start + sweep * step as f32 / steps as f32;
        let next = center + vec2(angle.cos(), angle.sin()) * radius;
        painter.line_segment([previous, next], Stroke::new(thickness, color));
        previous = next;
    }
}

pub fn hbar(painter: &Painter, rect: Rect, fraction: f32, color: Color32, track: Color32) {
    painter.rect_filled(rect, egui::CornerRadius::same(3), track);
    let width = rect.width() * fraction.clamp(0.0, 1.0);
    if width >= 1.0 {
        painter.rect_filled(
            Rect::from_min_size(rect.min, vec2(width, rect.height())),
            egui::CornerRadius::same(3),
            color,
        );
    }
}

/// One thin bar per logical CPU; hovering shows load and current clock.
pub fn core_bars(
    ui: &mut Ui,
    rect: Rect,
    values: &[f32],
    frequencies: &[u32],
    theme: &Theme,
    id: Id,
) {
    if values.is_empty() || rect.height() < 8.0 {
        return;
    }
    let response = ui.interact(rect, id, Sense::hover());
    let painter = ui.painter_at(rect);
    let count = values.len();
    let gap = 2.0f32;
    let width = ((rect.width() - gap * (count - 1) as f32) / count as f32).max(2.0);

    for (index, value) in values.iter().enumerate() {
        let x = rect.left() + index as f32 * (width + gap);
        painter.rect_filled(
            Rect::from_min_size(pos2(x, rect.top()), vec2(width, rect.height())),
            egui::CornerRadius::same(2),
            theme.grid,
        );
        let height = (value / 100.0).clamp(0.0, 1.0) * rect.height();
        let bar = Rect::from_min_size(
            pos2(x, rect.bottom() - height.max(1.5)),
            vec2(width, height.max(1.5)),
        );
        painter.rect_filled(bar, egui::CornerRadius::same(2), theme.load(*value));
    }

    if response.hovered() {
        if let Some(pointer) = response.hover_pos() {
            let index = ((pointer.x - rect.left()) / (width + gap)).floor() as usize;
            if let Some(value) = values.get(index) {
                let mut rows = vec![(
                    "使用率".to_string(),
                    format!("{value:.0}%"),
                    theme.load(*value),
                )];
                if let Some(mhz) = frequencies.get(index) {
                    if *mhz > 0 {
                        rows.push((
                            "频率".to_string(),
                            format!("{:.2} GHz", *mhz as f32 / 1000.0),
                            theme.dim,
                        ));
                    }
                }
                let title = format!("核心 {index}");
                tooltip(&painter, rect.expand(40.0), pointer, &title, &rows, theme);
            }
        }
    }
}

fn tooltip(
    painter: &Painter,
    bounds: Rect,
    anchor: Pos2,
    title: &str,
    rows: &[(String, String, Color32)],
    theme: &Theme,
) {
    let font = FontId::proportional(11.0);
    let title_galley = painter.layout_no_wrap(title.to_owned(), font.clone(), theme.text);
    let title_height = title_galley.size().y;
    let mut width = title_galley.size().x;
    let mut height = title_height;
    let mut galleys = Vec::with_capacity(rows.len());
    for (label, value, color) in rows {
        let galley = painter.layout_no_wrap(format!("{label}  {value}"), font.clone(), *color);
        width = width.max(galley.size().x);
        height += galley.size().y + 2.0;
        galleys.push((galley, *color));
    }

    let padding = 8.0;
    let size = vec2(width + padding * 2.0, height + padding * 2.0);
    let mut origin = anchor + vec2(14.0, 14.0);
    if origin.x + size.x > bounds.right() {
        origin.x = anchor.x - size.x - 14.0;
    }
    if origin.y + size.y > bounds.bottom() {
        origin.y = anchor.y - size.y - 14.0;
    }
    let rect = Rect::from_min_size(origin, size);
    painter.rect_filled(rect, egui::CornerRadius::same(7), theme.tooltip);
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(7),
        Stroke::new(1.0, theme.border),
        egui::StrokeKind::Inside,
    );

    let mut y = rect.top() + padding;
    painter.galley(
        pos2(rect.left() + padding, y),
        title_galley,
        theme.text,
    );
    y += title_height + 2.0;
    for (galley, color) in galleys {
        let advance = galley.size().y;
        painter.galley(pos2(rect.left() + padding, y), galley, color);
        y += advance + 2.0;
    }
}

fn short_duration(seconds: f64) -> String {
    if seconds >= 90.0 {
        format!("{:.0}分", seconds / 60.0)
    } else {
        format!("{:.0}s", seconds)
    }
}

/// `now_secs` minus an offset, as `HH:MM:SS`.
fn clock(now_secs: u32, offset_seconds: f32) -> String {
    let seconds = now_secs as f32 - offset_seconds.max(0.0);
    let seconds = if seconds < 0.0 {
        seconds + 86_400.0
    } else {
        seconds
    } as u32
        % 86_400;
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60
    )
}

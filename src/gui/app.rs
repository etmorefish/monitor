//! The dashboard window: layout, cards and interactions.
//!
//! Everything is painted into absolute rectangles computed from the window
//! size, so the grid always fills the page and degrades in two breakpoints
//! instead of scrolling.

use std::time::{Duration, Instant};

use eframe::egui::{
    self, pos2, vec2, Align, Align2, Color32, CornerRadius, FontId, Id, Layout, Painter, Pos2, Rect,
    RichText, Sense, Stroke, StrokeKind, Ui, UiBuilder,
};

use super::chart::{self, LineChart, Series, Unit};
use super::theme::Theme;
use crate::format;
use crate::metrics::{History, Sampler, Snapshot};

const GAP: f32 = 12.0;
const MARGIN: f32 = 16.0;
const HEADER_H: f32 = 52.0;
const FOOTER_H: f32 = 26.0;

#[derive(Clone, Copy)]
struct Toggles {
    read: bool,
    write: bool,
    rx: bool,
    tx: bool,
}

impl Default for Toggles {
    fn default() -> Self {
        Self {
            read: true,
            write: true,
            rx: true,
            tx: true,
        }
    }
}

pub struct Dashboard {
    sampler: Sampler,
    dark: bool,
    paused: bool,
    window_secs: f64,
    interval: f64,
    last_sample: Instant,
    toggles: Toggles,
    disk_scale: chart::AutoScale,
    net_scale: chart::AutoScale,
    applied_dark: bool,
    frames: u32,
    fps_since: Instant,
    fps: f32,
    frame_micros: u64,
}

impl Dashboard {
    pub fn new() -> Self {
        let arguments: Vec<String> = std::env::args().skip(1).collect();
        let _ = std::fs::write("args-debug.txt", format!("{arguments:?}"));
        // `--light` / `--dark` win; otherwise follow the Windows app theme.
        let forced = if arguments.iter().any(|argument| argument == "--light") {
            Some(false)
        } else if arguments.iter().any(|argument| argument == "--dark") {
            Some(true)
        } else {
            None
        };
        let dark = forced
            .or_else(crate::sys::registry::system_is_dark)
            .unwrap_or(true);
        let lean = arguments.iter().any(|argument| argument == "--lean");
        let interval = 1.0;
        let mut sampler = Sampler::new(interval, None, !lean);
        sampler.model.interval = interval;
        Self {
            sampler,
            dark,
            paused: false,
            window_secs: 60.0,
            interval,
            last_sample: Instant::now(),
            toggles: Toggles::default(),
            disk_scale: chart::AutoScale::default(),
            net_scale: chart::AutoScale::default(),
            // `gui::style` has already applied the dark palette; let the first
            // frame switch it if we started with `--light`.
            applied_dark: true,
            frames: 0,
            fps_since: Instant::now(),
            fps: 0.0,
            frame_micros: 0,
        }
    }

    fn theme(&self) -> Theme {
        if self.dark {
            Theme::dark()
        } else {
            Theme::light()
        }
    }

    fn samples(&self) -> usize {
        ((self.window_secs / self.interval).round() as usize)
            .clamp(10, crate::metrics::HISTORY_LEN)
    }

    fn header(&mut self, ui: &mut Ui, rect: Rect, theme: &Theme) {
        let host = self.sampler.model.host.clone();
        let stamp = format!(
            "{} {}",
            self.sampler.model.snap.date, self.sampler.model.snap.time
        );
        let uptime = if self.sampler.model.snap.uptime > 0 {
            format!(
                "运行 {}",
                crate::sys::host::uptime_human(self.sampler.model.snap.uptime)
            )
        } else {
            String::new()
        };
        let paused = self.paused;
        let dark = self.dark;
        let interval = self.interval;
        let window = self.window_secs;

        let mut ui =
            ui.new_child(UiBuilder::new().max_rect(rect).layout(Layout::left_to_right(Align::Center)));
        ui.label(RichText::new("minitor").size(19.0).strong().color(theme.accent));
        ui.add_space(2.0);
        ui.label(RichText::new(&host).size(11.5).color(theme.dim));
        if !uptime.is_empty() {
            ui.label(RichText::new(&uptime).size(10.5).color(theme.faint));
        }
        if paused {
            ui.add_space(6.0);
            ui.label(RichText::new("● 已暂停").size(10.5).color(theme.warn));
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(&stamp).size(11.5).color(theme.dim));
            ui.add_space(4.0);
            if ui.button(if dark { "浅色" } else { "深色" }).clicked() {
                self.dark = !self.dark;
            }
            if ui.button(if paused { "继续" } else { "暂停" }).clicked() {
                self.paused = !self.paused;
            }
            for option in [5.0f64, 2.0, 1.0, 0.5] {
                let selected = (interval - option).abs() < 0.05;
                if ui.selectable_label(selected, format!("{option}s")).clicked() {
                    self.interval = option;
                    self.sampler.model.interval = option;
                }
            }
            ui.label(RichText::new("刷新").size(9.5).color(theme.faint));
            for (label, seconds) in [("15分", 900.0f64), ("5分", 300.0), ("1分", 60.0)] {
                let selected = (window - seconds).abs() < 1.0;
                if ui.selectable_label(selected, label).clicked() {
                    self.window_secs = seconds;
                }
            }
            ui.label(RichText::new("范围").size(9.5).color(theme.faint));
        });
    }

    fn cards(&mut self, ui: &mut Ui, body: Rect, theme: &Theme) {
        let samples = self.samples();
        let now = parse_clock(&self.sampler.model.snap.time);
        let interval = self.interval;
        let snap = &self.sampler.model.snap;
        let history = &self.sampler.model.history;

        // Axis maxima follow the 90th percentile of what is on screen.
        let disk_peak = chart::percentile(history.io_read.slice(), samples, 0.9)
            .max(chart::percentile(history.io_write.slice(), samples, 0.9));
        let disk_max = self.disk_scale.update(disk_peak, 1024.0 * 1024.0);
        let net_peak = chart::percentile(history.net_rx.slice(), samples, 0.9)
            .max(chart::percentile(history.net_tx.slice(), samples, 0.9));
        let net_max = self.net_scale.update(net_peak, 2048.0);

        if body.width() >= 1120.0 {
            let top_h = (body.height() - GAP) * 0.45;
            let bottom_h = body.height() - top_h - GAP;
            let top = split_row(body.left(), body.top(), body.width(), top_h, &[0.58, 0.42]);
            let bottom = split_row(
                body.left(),
                body.top() + top_h + GAP,
                body.width(),
                bottom_h,
                &[1.0, 1.0, 1.0],
            );
            cpu_card(ui, top[0], theme, snap, history, samples, interval, now);
            memory_card(ui, top[1], theme, snap);
            disk_card(ui, bottom[0], theme, snap, history, &mut self.toggles, disk_max, samples, interval, now);
            net_card(ui, bottom[1], theme, snap, history, &mut self.toggles, net_max, samples, interval, now);
            thermal_card(ui, bottom[2], theme, snap, history, samples, interval, now);
        } else {
            let usable = body.height() - 2.0 * GAP;
            let first = usable * 0.32;
            let second = usable * 0.34;
            let third = usable - first - second;
            let row1 = split_row(body.left(), body.top(), body.width(), first, &[1.0]);
            let row2 = split_row(body.left(), body.top() + first + GAP, body.width(), second, &[1.0, 1.0]);
            let row3 = split_row(
                body.left(),
                body.top() + first + second + 2.0 * GAP,
                body.width(),
                third,
                &[1.0, 1.0],
            );
            cpu_card(ui, row1[0], theme, snap, history, samples, interval, now);
            memory_card(ui, row2[0], theme, snap);
            disk_card(ui, row2[1], theme, snap, history, &mut self.toggles, disk_max, samples, interval, now);
            net_card(ui, row3[0], theme, snap, history, &mut self.toggles, net_max, samples, interval, now);
            thermal_card(ui, row3[1], theme, snap, history, samples, interval, now);
        }
    }

    fn footer(&self, ui: &mut Ui, rect: Rect, theme: &Theme) {
        let snap = &self.sampler.model.snap;
        let painter = ui.painter_at(rect);
        let left = format!(
            "自身 {}  ·  {:.2}% CPU  ·  采样 {}  ·  {:.1} 帧/秒",
            format::bytes(snap.self_private),
            snap.self_cpu,
            format::micros(snap.tick_us),
            self.fps
        );
        text(&painter, pos2(rect.left(), rect.center().y), Align2::LEFT_CENTER, left, 10.0, theme.faint);
        text(
            &painter,
            pos2(rect.right(), rect.center().y),
            Align2::RIGHT_CENTER,
            "空格 暂停  ·  T 主题  ·  悬停图表看数值",
            10.0,
            theme.faint,
        );
    }
}

impl eframe::App for Dashboard {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let frame_started = Instant::now();
        self.frames += 1;
        if self.fps_since.elapsed().as_secs_f32() >= 1.0 {
            self.fps = self.frames as f32 / self.fps_since.elapsed().as_secs_f32();
            self.frames = 0;
            self.fps_since = Instant::now();
        }

        let now = Instant::now();
        if !self.paused && now.duration_since(self.last_sample).as_secs_f64() >= self.interval {
            self.sampler.sample();
            self.last_sample = now;
        }
        // One frame per sample is all the dashboard needs.
        ui.ctx().request_repaint_after(Duration::from_secs_f64(self.interval));

        if ui.ctx().input(|input| input.key_pressed(egui::Key::Space)) {
            self.paused = !self.paused;
        }
        if ui.ctx().input(|input| input.key_pressed(egui::Key::T)) {
            self.dark = !self.dark;
        }

        let theme = self.theme();
        if self.applied_dark != self.dark {
            theme.apply(ui.ctx());
            self.applied_dark = self.dark;
        }
        let full = ui.max_rect();
        ui.painter().rect_filled(full, 0.0, theme.bg);

        let header = Rect::from_min_size(
            full.min + vec2(MARGIN, 4.0),
            vec2((full.width() - 2.0 * MARGIN).max(10.0), HEADER_H - 6.0),
        );
        self.header(ui, header, &theme);

        let body = Rect::from_min_max(
            pos2(full.left() + MARGIN, full.top() + HEADER_H),
            pos2(full.right() - MARGIN, full.bottom() - FOOTER_H),
        );
        self.cards(ui, body, &theme);

        let footer = Rect::from_min_size(
            pos2(full.left() + MARGIN, full.bottom() - FOOTER_H + 3.0),
            vec2((full.width() - 2.0 * MARGIN).max(10.0), FOOTER_H - 6.0),
        );
        self.footer(ui, footer, &theme);
        self.frame_micros = frame_started.elapsed().as_micros() as u64;

    }
}

// --- cards ------------------------------------------------------------------

fn cpu_card(
    ui: &mut Ui,
    rect: Rect,
    theme: &Theme,
    snap: &Snapshot,
    history: &History,
    samples: usize,
    interval: f64,
    now: u32,
) {
    let content = card(ui, rect, theme, "CPU", &snap.cpu.brand, theme.cpu);
    if content.height() < 60.0 {
        return;
    }
    let painter = ui.painter_at(content);

    text(
        &painter,
        content.left_top(),
        Align2::LEFT_TOP,
        format!("{:.1}%", snap.cpu.total),
        34.0,
        theme.load(snap.cpu.total),
    );

    let mut x = content.left() + 132.0;
    let frequency = format!("{:.2} GHz", snap.cpu.mhz as f32 / 1000.0);
    let chip_rect = chip(&painter, pos2(x, content.top() + 5.0), &frequency, theme.cpu, theme);
    x = chip_rect.right() + 6.0;
    if let Some(hottest) = hottest(snap) {
        let label = format!("{hottest:.0}°C");
        chip(
            &painter,
            pos2(x, content.top() + 5.0),
            &label,
            theme.temperature(hottest),
            theme,
        );
    }

    // Topology and caches, the same set Task Manager lists.
    let cores = snap.cpu.physical.unwrap_or(snap.cpu.logical);
    let info = format!(
        "{} 核 / {} 线程 · L1 {} · L2 {} · L3 {}",
        cores,
        snap.cpu.logical,
        format::bytes(snap.cpu.cache_l1),
        format::bytes(snap.cpu.cache_l2),
        format::bytes(snap.cpu.cache_l3),
    );
    let legend_width = if content.width() > 460.0 { 150.0 } else { 0.0 };
    let text_width = ((content.width() - legend_width) / 5.8).max(16.0) as usize;
    text(
        &painter,
        pos2(content.left(), content.top() + 48.0),
        Align2::LEFT_TOP,
        format::truncate(&info, text_width),
        9.5,
        theme.faint,
    );

    if legend_width > 0.0 {
        // Colour key for the core bars: blue = near idle, red = near turbo.
        let base = snap.cpu.base_mhz.max(800) as f32;
        let bar_width = 44.0;
        let label = format!("{:.1}–{:.1} GHz", base * 0.3 / 1000.0, base * 3.2 / 1000.0);
        let label_left = content.right() - 74.0;
        let bar_left = label_left - bar_width - 6.0;
        text(
            &painter,
            pos2(bar_left - 6.0, content.top() + 48.0),
            Align2::RIGHT_TOP,
            "频率",
            9.0,
            theme.faint,
        );
        for step in 0..bar_width as i32 {
            let t = step as f32 / bar_width;
            let mhz = (base * (0.3 + t * 2.9)) as u32;
            painter.rect_filled(
                Rect::from_min_size(
                    pos2(bar_left + step as f32, content.top() + 49.0),
                    vec2(1.0, 6.0),
                ),
                CornerRadius::same(0),
                theme.frequency(mhz, snap.cpu.base_mhz),
            );
        }
        text(
            &painter,
            pos2(content.right(), content.top() + 48.0),
            Align2::RIGHT_TOP,
            &label,
            9.0,
            theme.faint,
        );
    }

    let chart_rect = Rect::from_min_max(
        pos2(content.left(), content.top() + 68.0),
        pos2(content.right(), (content.bottom() - 30.0).max(content.top() + 90.0)),
    );
    let series = [Series {
        values: history.cpu.slice(),
        color: theme.cpu,
        label: "CPU",
    }];
    chart::line_chart(
        ui,
        chart_rect,
        theme,
        &LineChart {
            series: &series,
            min: 0.0,
            max: 100.0,
            unit: Unit::Percent,
            samples,
            interval,
            now_secs: now,
            fill: true,
        },
        Id::unique("cpu-chart"),
    );

    let core_rect = Rect::from_min_max(
        pos2(content.left(), content.bottom() - 24.0),
        pos2(content.right(), content.bottom()),
    );
    chart::core_bars(ui, core_rect, &snap.cpu.per_core, &snap.cpu.core_mhz, snap.cpu.base_mhz, theme, Id::unique("cpu-cores"));
}

fn memory_card(ui: &mut Ui, rect: Rect, theme: &Theme, snap: &Snapshot) {
    let subtitle = format!("{} 进程", snap.memory.processes);
    let content = card(ui, rect, theme, "内存", &subtitle, theme.memory);
    if content.height() < 60.0 {
        return;
    }
    let painter = ui.painter_at(content);
    let memory = &snap.memory;

    let diameter = (content.height() - 6.0).min(content.width() * 0.52).max(48.0);
    let center = pos2(content.left() + diameter * 0.5, content.center().y);
    let radius = (diameter * 0.5 - 9.0).max(18.0);
    let total = memory.total.max(1) as f32;
    let used = memory.used() as f32;
    let available = memory.available as f32;
    // `cache` is already inside what Windows reports as available, so free is
    // the remainder: counting both would exceed the installed total.
    let cache = (memory.cache as f32).min(available);
    let free = (available - cache).max(0.0);
    // Two contrasting slices; the remaining arc is the track (free memory).
    let segments = [
        (used / total, theme.memory),
        (cache / total, Theme::alpha(theme.accent, 150)),
    ];
    chart::ring(&painter, center, radius, 12.0, &segments, theme.grid);
    text(
        &painter,
        pos2(center.x, center.y - 4.0),
        Align2::CENTER_CENTER,
        format!("{:.0}%", memory.percent()),
        19.0,
        theme.text,
    );
    text(
        &painter,
        pos2(center.x, center.y + 15.0),
        Align2::CENTER_CENTER,
        "已用",
        9.0,
        theme.faint,
    );

    let x = center.x + radius + 22.0;
    let width = content.right() - x;
    if width < 80.0 {
        return;
    }
    let rows = [
        ("已用", format::bytes(memory.used()), theme.memory),
        ("缓存", format::bytes(cache as u64), theme.dim),
        ("空闲", format::bytes(free as u64), theme.ok),
        (
            "提交",
            format!("{} / {}", format::bytes(memory.commit), format::bytes(memory.commit_limit)),
            theme.text,
        ),
        ("线程", format!("{} 个 · {} 句柄", memory.threads, memory.handles), theme.dim),
    ];
    let row_height = 25.0;
    let start = content.center().y - rows.len() as f32 * row_height * 0.5 + row_height * 0.5;
    for (index, (label, value, color)) in rows.iter().enumerate() {
        let y = start + index as f32 * row_height;
        text(&painter, pos2(x, y), Align2::LEFT_CENTER, *label, 9.5, theme.faint);
        text(&painter, pos2(content.right(), y), Align2::RIGHT_CENTER, value.clone(), 11.0, *color);
    }
}

fn disk_card(
    ui: &mut Ui,
    rect: Rect,
    theme: &Theme,
    snap: &Snapshot,
    history: &History,
    toggles: &mut Toggles,
    max: f32,
    samples: usize,
    interval: f64,
    now: u32,
) {
    let content = card(ui, rect, theme, "磁盘", "PhysicalDisk", theme.disk);
    if content.height() < 90.0 {
        return;
    }
    let painter = ui.painter_at(content);

    // Short cards keep the volume bars and drop the IO chart entirely.
    let compact = content.height() < 190.0;
    let row_height = if compact { 34.0 } else { 52.0 };
    let mut y = content.top();
    for volume in snap.volumes.iter().take(2) {
        let percent = volume.percent();
        text(
            &painter,
            pos2(content.left(), y),
            Align2::LEFT_TOP,
            format!("{}:", volume.letter),
            12.0,
            theme.text,
        );
        text(
            &painter,
            pos2(content.left() + 19.0, y + 1.5),
            Align2::LEFT_TOP,
            format::truncate(&volume.label, 8),
            9.5,
            theme.faint,
        );
        text(
            &painter,
            pos2(content.right(), y),
            Align2::RIGHT_TOP,
            format!("{percent:.0}%"),
            10.5,
            theme.load(percent),
        );
        let bar = Rect::from_min_size(pos2(content.left(), y + 22.0), vec2(content.width(), 6.0));
        chart::hbar(&painter, bar, percent / 100.0, theme.load(percent), theme.grid);
        if !compact {
            text(
                &painter,
                pos2(content.right(), y + 32.0),
                Align2::RIGHT_TOP,
                format!("{} / {}", format::bytes(volume.used()), format::bytes(volume.total)),
                9.5,
                theme.dim,
            );
        }
        y += row_height;
    }
    if compact {
        return;
    }

    let mut x = content.left();
    let read_chip = legend_chip(ui, &painter, pos2(x, y), "读", theme.io_read, toggles.read, theme, "legend-disk-read");
    if read_chip.clicked() {
        toggles.read = !toggles.read;
    }
    x = read_chip.rect.right() + 6.0;
    let write_chip = legend_chip(ui, &painter, pos2(x, y), "写", theme.io_write, toggles.write, theme, "legend-disk-write");
    if write_chip.clicked() {
        toggles.write = !toggles.write;
    }

    let chart_rect = Rect::from_min_max(
        pos2(content.left(), y + 26.0),
        pos2(content.right(), content.bottom()),
    );
    if chart_rect.height() < 40.0 {
        return;
    }

    let read_series = Series {
        values: history.io_read.slice(),
        color: theme.io_read,
        label: "读",
    };
    let write_series = Series {
        values: history.io_write.slice(),
        color: theme.io_write,
        label: "写",
    };
    let mut active: Vec<Series> = Vec::with_capacity(2);
    if toggles.read {
        active.push(read_series);
    }
    if toggles.write {
        active.push(write_series);
    }
    chart::line_chart(
        ui,
        chart_rect,
        theme,
        &LineChart {
            series: &active,
            min: 0.0,
            max,
            unit: Unit::BytesPerSec,
            samples,
            interval,
            now_secs: now,
            fill: active.len() == 1,
        },
        Id::unique("disk-chart"),
    );
}

fn net_card(
    ui: &mut Ui,
    rect: Rect,
    theme: &Theme,
    snap: &Snapshot,
    history: &History,
    toggles: &mut Toggles,
    max: f32,
    samples: usize,
    interval: f64,
    now: u32,
) {
    let interface = snap
        .nets
        .iter()
        .find(|net| net.up)
        .or_else(|| snap.nets.first());
    let subtitle = interface.map(|net| pretty(&net.name)).unwrap_or_default();
    let content = card(ui, rect, theme, "网络", &subtitle, theme.net);
    if content.height() < 90.0 {
        return;
    }
    let painter = ui.painter_at(content);

    let rx: f64 = snap.nets.iter().map(|net| net.rx_bps).sum();
    let tx: f64 = snap.nets.iter().map(|net| net.tx_bps).sum();
    text(
        &painter,
        content.left_top(),
        Align2::LEFT_TOP,
        format!("↓ {}", format::rate(rx)),
        17.0,
        theme.net,
    );
    text(
        &painter,
        pos2(content.left() + 110.0, content.top() + 3.0),
        Align2::LEFT_TOP,
        format!("↑ {}", format::rate(tx)),
        14.0,
        theme.dim,
    );
    text(
        &painter,
        pos2(content.right(), content.top() + 2.0),
        Align2::RIGHT_TOP,
        format!("累计 ↓{} ↑{}", format::bytes(interface.map_or(0, |net| net.rx_total)), format::bytes(interface.map_or(0, |net| net.tx_total))),
        9.0,
        theme.faint,
    );

    let mut x = content.left();
    let rx_chip = legend_chip(ui, &painter, pos2(x, content.top() + 24.0), "下行", theme.net_rx, toggles.rx, theme, "legend-net-rx");
    if rx_chip.clicked() {
        toggles.rx = !toggles.rx;
    }
    x = rx_chip.rect.right() + 6.0;
    let tx_chip = legend_chip(ui, &painter, pos2(x, content.top() + 24.0), "上行", theme.net_tx, toggles.tx, theme, "legend-net-tx");
    if tx_chip.clicked() {
        toggles.tx = !toggles.tx;
    }

    let interface_height = 22.0;
    let chart_rect = Rect::from_min_max(
        pos2(content.left(), content.top() + 48.0),
        pos2(content.right(), content.bottom() - interface_height * 2.0 - 4.0),
    );
    if chart_rect.height() >= 40.0 {

        let rx_series = Series {
            values: history.net_rx.slice(),
            color: theme.net_rx,
            label: "下行",
        };
        let tx_series = Series {
            values: history.net_tx.slice(),
            color: theme.net_tx,
            label: "上行",
        };
        let mut active: Vec<Series> = Vec::with_capacity(2);
        if toggles.rx {
            active.push(rx_series);
        }
        if toggles.tx {
            active.push(tx_series);
        }
        chart::line_chart(
            ui,
            chart_rect,
            theme,
            &LineChart {
                series: &active,
                min: 0.0,
                max,
                unit: Unit::BytesPerSec,
                samples,
                interval,
                now_secs: now,
                fill: active.len() == 1,
            },
            Id::unique("net-chart"),
        );
    }

    let mut y = content.bottom() - interface_height * 2.0;
    for net in snap.nets.iter().take(2) {
        text(
            &painter,
            pos2(content.left(), y),
            Align2::LEFT_TOP,
            format::truncate(&pretty(&net.name), 22),
            9.5,
            if net.up { theme.dim } else { theme.faint },
        );
        text(
            &painter,
            pos2(content.right(), y),
            Align2::RIGHT_TOP,
            format!("↓{} ↑{}", format::rate(net.rx_bps), format::rate(net.tx_bps)),
            9.5,
            theme.faint,
        );
        y += interface_height;
    }
}

fn thermal_card(
    ui: &mut Ui,
    rect: Rect,
    theme: &Theme,
    snap: &Snapshot,
    history: &History,
    samples: usize,
    interval: f64,
    now: u32,
) {
    let content = card(ui, rect, theme, "散热", "ACPI", theme.temp);
    if content.height() < 90.0 {
        return;
    }
    let painter = ui.painter_at(content);

    let hottest = hottest(snap);
    match hottest {
        Some(celsius) => {
            text(
                &painter,
                content.left_top(),
                Align2::LEFT_TOP,
                format!("{celsius:.0}°C"),
                30.0,
                theme.temperature(celsius),
            );
            if let Some(zone) = snap.zones.first() {
                text(
                    &painter,
                    pos2(content.left() + 82.0, content.top() + 8.0),
                    Align2::LEFT_TOP,
                    &zone.name,
                    9.5,
                    theme.faint,
                );
            }
        }
        None => {
            text(
                &painter,
                content.left_top(),
                Align2::LEFT_TOP,
                if snap.counters_enabled { "不可用" } else { "已关闭" },
                16.0,
                theme.faint,
            );
        }
    }

    let footer_height = 58.0;
    let chart_rect = Rect::from_min_max(
        pos2(content.left(), content.top() + 40.0),
        pos2(content.right(), (content.bottom() - footer_height).max(content.top() + 80.0)),
    );
    if chart_rect.height() >= 40.0 {
        let (min, max) = temperature_axis(history.temp.slice(), samples, hottest);
        let series = [Series {
            values: history.temp.slice(),
            color: theme.temp,
            label: "温度",
        }];
        chart::line_chart(
            ui,
            chart_rect,
            theme,
            &LineChart {
                series: &series,
                min,
                max,
                unit: Unit::Celsius,
                samples,
                interval,
                now_secs: now,
                fill: true,
            },
            Id::unique("temp-chart"),
        );
    }

    let mut y = content.bottom() - footer_height + 8.0;
    match snap.fans.first() {
        Some(fan) => {
            text(&painter, pos2(content.left(), y), Align2::LEFT_TOP, "风扇", 9.5, theme.faint);
            text(
                &painter,
                pos2(content.right(), y),
                Align2::RIGHT_TOP,
                format!("{} RPM", fan.rpm),
                10.5,
                theme.dim,
            );
        }
        None => {
            text(&painter, pos2(content.left(), y), Align2::LEFT_TOP, "风扇", 9.5, theme.faint);
            text(
                &painter,
                pos2(content.right(), y),
                Align2::RIGHT_TOP,
                "—  固件未暴露",
                9.5,
                theme.faint,
            );
        }
    }

    y += 22.0;
    match snap.power {
        Some(power) if power.available() => {
            let percent = power.battery_percent.unwrap_or(0) as f32;
            let state = match power.state {
                crate::sys::power::BatteryState::Charging => "充电中",
                crate::sys::power::BatteryState::Discharging => "放电中",
                crate::sys::power::BatteryState::Idle => "交流供电",
                _ => "",
            };
            text(&painter, pos2(content.left(), y), Align2::LEFT_TOP, "电池", 9.5, theme.faint);
            text(
                &painter,
                pos2(content.right(), y),
                Align2::RIGHT_TOP,
                format!("{percent:.0}%  {state}"),
                10.5,
                theme.battery,
            );
            let bar = Rect::from_min_size(pos2(content.left(), y + 16.0), vec2(content.width(), 6.0));
            chart::hbar(&painter, bar, percent / 100.0, theme.battery, theme.grid);
        }
        _ => {
            text(&painter, pos2(content.left(), y), Align2::LEFT_TOP, "电池", 9.5, theme.faint);
            text(&painter, pos2(content.right(), y), Align2::RIGHT_TOP, "—", 10.5, theme.faint);
        }
    }
}

// --- helpers ----------------------------------------------------------------

fn card(ui: &mut Ui, rect: Rect, theme: &Theme, title: &str, subtitle: &str, accent: Color32) -> Rect {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(12), theme.card);
    painter.rect_stroke(
        rect,
        CornerRadius::same(12),
        Stroke::new(1.0, theme.border),
        StrokeKind::Inside,
    );
    painter.circle_filled(pos2(rect.left() + 16.0, rect.top() + 19.0), 3.5, accent);
    painter.text(
        pos2(rect.left() + 26.0, rect.top() + 12.0),
        Align2::LEFT_TOP,
        title,
        FontId::proportional(13.0),
        theme.text,
    );
    if !subtitle.is_empty() {
        painter.text(
            pos2(rect.right() - 14.0, rect.top() + 13.5),
            Align2::RIGHT_TOP,
            format::truncate(subtitle, 42),
            FontId::proportional(9.5),
            theme.faint,
        );
    }
    Rect::from_min_max(rect.min + vec2(15.0, 38.0), rect.max - vec2(15.0, 12.0))
}

fn text(
    painter: &Painter,
    pos: Pos2,
    anchor: Align2,
    value: impl Into<String>,
    size: f32,
    color: Color32,
) {
    painter.text(pos, anchor, value.into(), FontId::proportional(size), color);
}

fn chip(painter: &Painter, pos: Pos2, label: &str, color: Color32, theme: &Theme) -> Rect {
    let galley = painter.layout_no_wrap(label.to_owned(), FontId::proportional(9.5), color);
    let rect = Rect::from_min_size(pos, galley.size() + vec2(16.0, 8.0));
    painter.rect_filled(rect, CornerRadius::same(9), Theme::alpha(color, 26));
    painter.galley(pos + vec2(8.0, 4.0), galley, color);
    let _ = theme;
    rect
}

fn legend_chip(
    ui: &Ui,
    painter: &Painter,
    pos: Pos2,
    label: &str,
    color: Color32,
    on: bool,
    theme: &Theme,
    id: &str,
) -> egui::Response {
    let text_color = if on { color } else { theme.faint };
    let galley = painter.layout_no_wrap(label.to_owned(), FontId::proportional(9.5), text_color);
    let rect = Rect::from_min_size(pos, galley.size() + vec2(26.0, 8.0));
    painter.rect_filled(
        rect,
        CornerRadius::same(9),
        if on { Theme::alpha(color, 24) } else { theme.grid },
    );
    painter.circle_filled(
        pos2(rect.left() + 9.0, rect.center().y),
        3.0,
        if on { color } else { theme.faint },
    );
    painter.galley(
        pos2(rect.left() + 16.0, rect.center().y - galley.size().y * 0.5),
        galley,
        text_color,
    );
    ui.interact(rect, Id::unique(id), Sense::click())
}

fn hottest(snap: &Snapshot) -> Option<f32> {
    snap.zones
        .iter()
        .map(|zone| zone.celsius)
        .fold(None, |current: Option<f32>, value| {
            Some(current.map_or(value, |best| best.max(value)))
        })
}

fn pretty(name: &str) -> String {
    let mut cleaned = name.to_string();
    for noise in ["(R)", "(TM)", "(r)", "(tm)"] {
        cleaned = cleaned.replace(noise, "");
    }
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Horizontal split with fractional weights.
fn split_row(x: f32, y: f32, width: f32, height: f32, weights: &[f32]) -> Vec<Rect> {
    let total: f32 = weights.iter().sum();
    let available = width - GAP * (weights.len() as f32 - 1.0);
    let mut rects = Vec::with_capacity(weights.len());
    let mut cursor = x;
    for weight in weights {
        let w = available * weight / total;
        rects.push(Rect::from_min_size(pos2(cursor, y), vec2(w, height)));
        cursor += w + GAP;
    }
    rects
}

/// Window the temperature axis around what is actually on screen: a fixed
/// 30–100 °C range wastes half the card when the machine idles at 70 °C.
fn temperature_axis(values: &[f32], samples: usize, current: Option<f32>) -> (f32, f32) {
    let start = values.len().saturating_sub(samples);
    let mut low = f32::INFINITY;
    let mut high = f32::NEG_INFINITY;
    for value in &values[start..] {
        if value.is_finite() {
            low = low.min(*value);
            high = high.max(*value);
        }
    }
    if !low.is_finite() {
        let fallback = current.unwrap_or(60.0);
        low = fallback;
        high = fallback;
    }

    let span = ((high - low) + 12.0).clamp(40.0, 70.0);
    let center = ((low + high) * 0.5 / 5.0).round() * 5.0;
    let mut min = (center - span * 0.5).max(20.0);
    let mut max = min + span;
    if max > 105.0 {
        max = 105.0;
        min = (max - span).max(20.0);
    }
    (min.floor(), max.ceil())
}

fn parse_clock(value: &str) -> u32 {
    let mut parts = value.split(':');
    let hours: u32 = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
    let minutes: u32 = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
    let seconds: u32 = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
    hours * 3600 + minutes * 60 + seconds
}

//! The dashboard window: layout, cards and interactions.
//!
//! Everything is painted into absolute rectangles computed from the window
//! size, so the grid always fills the page and degrades in two breakpoints
//! instead of scrolling.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, pos2, vec2, Align, Align2, Color32, CornerRadius, FontId, Id, Layout, Painter, Pos2, Rect,
    RichText, Sense, Stroke, StrokeKind, TextureHandle, Ui, UiBuilder,
};

use super::chart::{self, LineChart, Series, Unit};
use super::icons;
use super::lang;
use super::theme::Theme;
use super::throttle::{self, MouseThrottle};
use crate::format;
use crate::metrics::{History, Sampler, Snapshot};
use crate::sys::gpu::Gpu;
use crate::sys::process::{self, ProcessEntry, ProcessList, ProcessSampler};

const GAP: f32 = 12.0;
const MARGIN: f32 = 16.0;
const HEADER_H: f32 = 52.0;
const FOOTER_H: f32 = 26.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Overview,
    Hardware,
    Processes,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProcessSort {
    Cpu,
    Memory,
    Name,
    Pid,
    Age,
    Threads,
    Handles,
}

/// Minimum widths for name, path, PID, CPU, memory, age, threads, handles.
const PROCESS_WIDTH_MIN: [f32; 8] = [130.0, 100.0, 50.0, 60.0, 90.0, 60.0, 45.0, 45.0];

struct IconRequest {
    pid: u32,
    create_time: u64,
}

struct IconResult {
    pid: u32,
    create_time: u64,
    path: Option<String>,
    image: Option<egui::ColorImage>,
}

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
    page: Page,
    copied: Option<(String, Instant)>,
    process_sampler: ProcessSampler,
    processes: ProcessList,
    process_at: Instant,
    process_search: String,
    process_sort: ProcessSort,
    process_ascending: bool,
    process_selected: Option<u32>,
    process_selected_auto: bool,
    process_widths: [f32; 8],
    process_widths_total: f32,
    process_pending: HashMap<u32, u64>,
    process_scroll_at: Instant,
    icon_tx: Sender<IconRequest>,
    icon_rx: Receiver<IconResult>,
    process_paths: HashMap<u32, (u64, Option<String>)>,
    process_icons: HashMap<String, Option<TextureHandle>>,
    process_confirm: Option<u32>,
    process_message: Option<(String, bool)>,
    frames: u32,
    fps_since: Instant,
    fps: f32,
    frame_micros: u64,
    mouse: MouseThrottle,
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
        if arguments
            .iter()
            .any(|argument| argument == "--en" || argument == "--english")
        {
            lang::set(lang::Lang::En);
        } else if arguments
            .iter()
            .any(|argument| argument == "--zh" || argument == "--chinese")
        {
            lang::set(lang::Lang::Zh);
        }
        // Handy for screenshots and for opening straight on the details page.
        let page = if arguments.iter().any(|argument| argument == "--hardware") {
            Page::Hardware
        } else if arguments
            .iter()
            .any(|argument| argument == "--processes" || argument == "--process")
        {
            Page::Processes
        } else {
            Page::Overview
        };
        let interval = 1.0;
        let mut sampler = Sampler::new(interval, None, !lean);
        sampler.model.interval = interval;
        let (icon_tx, icon_rx) = spawn_icon_worker();
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
            page,
            copied: None,
            process_sampler: ProcessSampler::new(),
            processes: ProcessList::default(),
            process_at: Instant::now() - Duration::from_secs(10),
            process_search: String::new(),
            process_sort: ProcessSort::Cpu,
            process_ascending: false,
            process_selected: None,
            process_selected_auto: true,
            process_widths: [0.0; 8],
            process_widths_total: 0.0,
            process_pending: HashMap::new(),
            process_scroll_at: Instant::now() - Duration::from_secs(10),
            icon_tx,
            icon_rx,
            process_paths: HashMap::new(),
            process_icons: HashMap::new(),
            process_confirm: None,
            process_message: None,
            frames: 0,
            fps_since: Instant::now(),
            fps: 0.0,
            frame_micros: 0,
            mouse: MouseThrottle::new(),
        }
    }

    fn theme(&self) -> Theme {
        if self.dark {
            Theme::dark()
        } else {
            Theme::light()
        }
    }

    fn poll_icon_events(&mut self, ctx: &egui::Context) {
        let mut processed = 0;
        while processed < 6 {
            let Ok(result) = self.icon_rx.try_recv() else {
                break;
            };
            processed += 1;
            self.process_pending.remove(&result.pid);
            if let Some(path) = result.path.clone() {
                if !self.process_icons.contains_key(&path) {
                    let image = result
                        .image
                        .map(|image| ctx.load_texture(path.clone(), image, egui::TextureOptions::LINEAR));
                    self.process_icons.insert(path.clone(), image);
                }
                self.process_paths
                    .insert(result.pid, (result.create_time, Some(path)));
            } else {
                self.process_paths
                    .insert(result.pid, (result.create_time, None));
            }
        }
        if processed > 0 {
            ctx.request_repaint();
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
                "{} {}",
                lang::t("运行", "up"),
                lang::uptime(self.sampler.model.snap.uptime)
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
        ui.label(RichText::new("monitor").size(19.0).strong().color(theme.accent));
        ui.add_space(2.0);
        ui.label(RichText::new(&host).size(11.5).color(theme.dim));
        if !uptime.is_empty() {
            ui.label(RichText::new(&uptime).size(10.5).color(theme.faint));
        }
        if paused {
            ui.add_space(6.0);
            ui.label(
                RichText::new(lang::t("● 已暂停", "● paused"))
                    .size(10.5)
                    .color(theme.warn),
            );
        }
        ui.add_space(14.0);
        for (page, label) in [
            (Page::Overview, lang::t("总览", "Overview")),
            (Page::Hardware, lang::t("硬件", "Hardware")),
            (Page::Processes, lang::t("进程", "Processes")),
        ] {
            if ui.selectable_label(self.page == page, label).clicked() {
                self.page = page;
            }
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(&stamp).size(11.5).color(theme.dim));
            ui.add_space(4.0);
            if ui
                .button(if dark {
                    lang::t("浅色", "Light")
                } else {
                    lang::t("深色", "Dark")
                })
                .clicked()
            {
                self.dark = !self.dark;
            }
            if ui
                .button(if lang::is_en() { "EN" } else { "中" })
                .on_hover_text(lang::t("切换语言", "Switch language"))
                .clicked()
            {
                lang::toggle();
            }
            if ui
                .button(if paused {
                    lang::t("继续", "Resume")
                } else {
                    lang::t("暂停", "Pause")
                })
                .clicked()
            {
                self.paused = !self.paused;
            }
            for option in [5.0f64, 2.0, 1.0, 0.5] {
                let selected = (interval - option).abs() < 0.05;
                if ui.selectable_label(selected, format!("{option}s")).clicked() {
                    self.interval = option;
                    self.sampler.model.interval = option;
                }
            }
            ui.label(
                RichText::new(lang::t("刷新", "refresh"))
                    .size(9.5)
                    .color(theme.faint),
            );
            for (label, seconds) in [
                (lang::t("15分", "15m"), 900.0f64),
                (lang::t("5分", "5m"), 300.0),
                (lang::t("1分", "1m"), 60.0),
            ] {
                let selected = (window - seconds).abs() < 1.0;
                if ui.selectable_label(selected, label).clicked() {
                    self.window_secs = seconds;
                }
            }
            ui.label(
                RichText::new(lang::t("范围", "range"))
                    .size(9.5)
                    .color(theme.faint),
            );
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
            net_card(ui, bottom[1], theme, snap, history, &mut self.toggles, &mut self.copied, net_max, samples, interval, now);
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
            net_card(ui, row3[0], theme, snap, history, &mut self.toggles, &mut self.copied, net_max, samples, interval, now);
            thermal_card(ui, row3[1], theme, snap, history, samples, interval, now);
        }
    }

    fn process_page(&mut self, ui: &mut Ui, body: Rect, theme: &Theme) {
        let total = self.processes.entries.len();
        let subtitle = format!(
            "{} · {} · 5s {}",
            if lang::is_en() {
                format!("{total} processes")
            } else {
                format!("{total} 进程")
            },
            if lang::is_en() {
                format!("{} threads", self.processes.total_threads)
            } else {
                format!("{} 线程", self.processes.total_threads)
            },
            lang::t("刷新", "refresh")
        );
        let content = card(ui, body, theme, lang::t("进程", "Processes"), &subtitle, theme.accent);
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(content)
                .layout(Layout::top_down(Align::Min)),
        );
        child.spacing_mut().item_spacing = vec2(8.0, 4.0);
        let summary_rect = Rect::from_min_size(content.min, vec2(content.width(), 54.0));
        paint_process_summary(
            ui.painter(),
            summary_rect,
            theme,
            &self.sampler.model.snap,
            &self.sampler.model.history,
        );
        child.add_space(62.0);

        let mut refresh = false;
        let mut sort_request: Option<(ProcessSort, bool)> = None;
        child.horizontal(|ui| {
            ui.label(
                RichText::new(lang::t("搜索", "Search"))
                    .size(10.0)
                    .color(theme.dim),
            );
            egui::Frame::new()
                .fill(theme.bg)
                .stroke(Stroke::new(1.0, theme.border))
                .inner_margin(egui::Margin::symmetric(6, 2))
                .corner_radius(6.0)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.process_search)
                            .frame(egui::Frame::NONE)
                            .hint_text(lang::t("进程名 / PID", "name / PID"))
                            .desired_width(220.0)
                            .font(FontId::proportional(12.0))
                            .text_color(theme.text),
                    );
                });
            if ui.button(lang::t("刷新", "Refresh")).clicked() {
                refresh = true;
            }
            ui.add_space(6.0);
            stat_pill(
                ui,
                &lang::count(total, "进程", "processes"),
                theme.accent,
            );
            stat_pill(
                ui,
                &lang::count(
                    self.processes.total_threads as usize,
                    "线程",
                    "threads",
                ),
                theme.dim,
            );
            if let Some((message, ok)) = &self.process_message {
                let color = if *ok { theme.ok } else { theme.bad };
                ui.label(RichText::new(message).size(10.5).color(color));
            }
        });

        let current_sort = self.process_sort;
        let current_ascending = self.process_ascending;
        let gap = 8.0;
        let icon_w = 26.0;
        let kill_w = 54.0;
        let target = (content.width() - icon_w - kill_w - gap * 9.0).max(580.0);
        fit_process_widths(&mut self.process_widths, &mut self.process_widths_total, target);
        let mut widths = self.process_widths;
        let query = self.process_search.trim().to_lowercase();
        let visible_count = self
            .processes
            .entries
            .iter()
            .filter(|entry| {
                query.is_empty()
                    || entry.name.to_lowercase().contains(&query)
                    || entry.pid.to_string().contains(&query)
            })
            .count();
        child.label(
            RichText::new(if lang::is_en() {
                format!("showing {visible_count} / {total} processes")
            } else {
                format!("显示 {visible_count} / {total} 个进程")
            })
            .size(9.5)
            .color(theme.faint),
        );

        let table_width =
            icon_w + kill_w + gap * 9.0 + widths.iter().sum::<f32>();
        let (header_rect, _) =
            child.allocate_exact_size(vec2(table_width, 24.0), Sense::hover());
        let painter = child.painter().clone();
        painter.rect_filled(
            header_rect,
            CornerRadius::same(5),
            Theme::alpha(theme.grid, 16),
        );
        let headers: [(&str, Option<ProcessSort>, bool); 8] = [
            (lang::t("名称", "Name"), Some(ProcessSort::Name), false),
            (lang::t("路径", "Path"), None, false),
            ("PID", Some(ProcessSort::Pid), true),
            ("CPU", Some(ProcessSort::Cpu), true),
            (lang::t("内存", "Memory"), Some(ProcessSort::Memory), true),
            (lang::t("启动", "Started"), Some(ProcessSort::Age), true),
            (lang::t("线程", "Threads"), Some(ProcessSort::Threads), true),
            (lang::t("句柄", "Handles"), Some(ProcessSort::Handles), true),
        ];
        let mut x = header_rect.left() + icon_w + gap;
        for (index, (label, sort, right)) in headers.iter().enumerate() {
            let width = widths[index];
            let col = Rect::from_min_size(
                pos2(x, header_rect.top()),
                vec2(width, header_rect.height()),
            );
            let active = *sort == Some(current_sort);
            let arrow = if active {
                if current_ascending {
                    " ↑"
                } else {
                    " ↓"
                }
            } else {
                ""
            };
            let color = if active { theme.accent } else { theme.dim };
            let (pos, anchor) = if index == 3 {
                (col.center(), Align2::CENTER_CENTER)
            } else if *right {
                (
                    pos2(col.right() - 2.0, col.center().y),
                    Align2::RIGHT_CENTER,
                )
            } else {
                (
                    pos2(col.left() + 2.0, col.center().y),
                    Align2::LEFT_CENTER,
                )
            };
            painter.text(
                pos,
                anchor,
                format!("{label}{arrow}"),
                FontId::proportional(10.0),
                color,
            );
            if let Some(sort) = sort {
                let click_rect = Rect::from_min_size(
                    col.min,
                    vec2((width - gap * 0.75).max(24.0), col.height()),
                );
                let response = child.interact(
                    click_rect,
                    Id::unique(("process-header", index)),
                    Sense::click(),
                );
                if response.hovered() {
                    child.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if response.clicked() {
                    let descending = *sort != ProcessSort::Name;
                    sort_request = Some((
                        *sort,
                        if active { !current_ascending } else { !descending },
                    ));
                }
            }
            x += width + gap;
        }

        let mut boundary_x = header_rect.left() + icon_w + gap;
        for index in 0..widths.len() {
            boundary_x += widths[index];
            if index + 1 < widths.len() {
                let handle = Rect::from_center_size(
                    pos2(boundary_x + gap * 0.5, header_rect.center().y),
                    vec2(12.0, 28.0),
                );
                let response = child.interact(
                    handle,
                    Id::unique(("process-resize", index)),
                    Sense::drag(),
                );
                if response.hovered() || response.dragged() {
                    child
                        .ctx()
                        .set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                    painter.rect_filled(
                        Rect::from_center_size(
                            handle.center(),
                            vec2(2.0, 15.0),
                        ),
                        CornerRadius::same(1),
                        theme.accent,
                    );
                } else {
                    painter.rect_filled(
                        Rect::from_center_size(
                            handle.center(),
                            vec2(1.5, 15.0),
                        ),
                        CornerRadius::same(1),
                        Theme::alpha(theme.dim, 90),
                    );
                }
                if response.dragged() {
                    let dx = child.input(|input| input.pointer.delta().x);
                    if dx.abs() > 0.01 {
                        resize_process_widths(&mut widths, index, dx);
                    }
                    child.ctx().request_repaint();
                }
            }
            boundary_x += gap;
        }
        self.process_widths = widths;
        painter.hline(
            header_rect.x_range(),
            header_rect.bottom() - 0.5,
            Stroke::new(1.0, theme.border),
        );

        let name_w = widths[0];
        let path_w = widths[1];
        let pid_w = widths[2];
        let cpu_w = widths[3];
        let mem_w = widths[4];
        let age_w = widths[5];
        let threads_w = widths[6];
        let handles_w = widths[7];
        let (sort, ascending) = sort_request.unwrap_or((current_sort, current_ascending));
        let mut rows: Vec<&ProcessEntry> = self
            .processes
            .entries
            .iter()
            .filter(|entry| {
                query.is_empty()
                    || entry.name.to_lowercase().contains(&query)
                    || entry.pid.to_string().contains(&query)
            })
            .collect();
        rows.sort_by(|a, b| {
            let order = match sort {
                ProcessSort::Cpu => a.cpu_percent.total_cmp(&b.cpu_percent),
                ProcessSort::Memory => a.private_bytes.cmp(&b.private_bytes),
                ProcessSort::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                ProcessSort::Pid => a.pid.cmp(&b.pid),
                ProcessSort::Age => a.age_secs.cmp(&b.age_secs),
                ProcessSort::Threads => a.threads.cmp(&b.threads),
                ProcessSort::Handles => a.handles.cmp(&b.handles),
            };
            if ascending {
                order
            } else {
                order.reverse()
            }
        });

        let selected_pid = if self.process_selected_auto {
            rows.first().map(|entry| entry.pid)
        } else {
            self.process_selected
                .filter(|pid| rows.iter().any(|entry| entry.pid == *pid))
                .or_else(|| rows.first().map(|entry| entry.pid))
        };
        let mut select_request = None;
        let mut kill_request = None;
        let max_memory = rows
            .iter()
            .map(|entry| entry.private_bytes)
            .max()
            .unwrap_or(1)
            .max(1);
        let mut paths = std::mem::take(&mut self.process_paths);
        let process_icons = std::mem::take(&mut self.process_icons);
        let mut pending = std::mem::take(&mut self.process_pending);
        let icon_tx = self.icon_tx.clone();
        let row_height = 28.0;

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(&mut child, row_height, rows.len(), |ui, range| {
                for index in range {
                    let entry = rows[index];
                    let path = request_path(
                        &mut paths,
                        &mut pending,
                        &icon_tx,
                        entry.pid,
                        entry.create_time,
                    );
                    let icon = path
                        .as_deref()
                        .and_then(|path| process_icons.get(path).cloned().flatten());
                    let (row_rect, response) =
                        ui.allocate_exact_size(vec2(table_width, row_height), Sense::click());
                    let selected = selected_pid == Some(entry.pid);
                    let hovered = response.hovered();
                    if response.clicked() {
                        select_request = Some(entry.pid);
                    }
                    if hovered {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    let background = if selected {
                        Theme::alpha(theme.accent, 28)
                    } else if hovered {
                        Theme::alpha(theme.accent, 12)
                    } else if index % 2 == 0 {
                        Theme::alpha(theme.grid, 20)
                    } else {
                        Color32::TRANSPARENT
                    };
                    if background != Color32::TRANSPARENT {
                        ui.painter()
                            .rect_filled(row_rect, CornerRadius::same(6), background);
                    }
                    if selected {
                        let bar = Rect::from_min_size(
                            pos2(row_rect.left(), row_rect.top() + 4.0),
                            vec2(2.5, row_rect.height() - 8.0),
                        );
                        ui.painter()
                            .rect_filled(bar, CornerRadius::same(2), theme.accent);
                    }

                    let center = row_rect.center().y;
                    let mut x = row_rect.left() + 6.0;
                    let icon_rect =
                        Rect::from_center_size(pos2(x + 10.0, center), vec2(18.0, 18.0));
                    ui.painter().rect_filled(
                        icon_rect.expand(1.5),
                        CornerRadius::same(5),
                        Theme::alpha(theme.grid, 45),
                    );
                    if let Some(icon) = icon {
                        ui.painter().image(
                            icon.id(),
                            icon_rect,
                            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    } else {
                        paint_fallback_icon(ui.painter(), icon_rect, &entry.name, theme);
                    }
                    x += icon_w;

                    let name = format::truncate(&entry.name, (name_w / 6.2) as usize);
                    ui.painter().text(
                        pos2(x + 2.0, center),
                        Align2::LEFT_CENTER,
                        name,
                        FontId::proportional(11.0),
                        theme.text,
                    );
                    x += name_w + gap;

                    let path_text = path
                        .as_deref()
                        .map(|path| format::truncate(path, (path_w / 5.8) as usize))
                        .unwrap_or_else(|| "—".to_string());
                    ui.painter().text(
                        pos2(x + 2.0, center),
                        Align2::LEFT_CENTER,
                        path_text,
                        FontId::proportional(9.5),
                        if selected { theme.dim } else { theme.faint },
                    );
                    x += path_w + gap;

                    ui.painter().text(
                        pos2(x + pid_w - 2.0, center),
                        Align2::RIGHT_CENTER,
                        entry.pid.to_string(),
                        FontId::proportional(10.0),
                        if selected { theme.text } else { theme.dim },
                    );
                    x += pid_w + gap;

                    let cpu_color = theme.load(entry.cpu_percent);
                    let pill = Rect::from_center_size(
                        pos2(x + cpu_w * 0.5, center),
                        vec2(cpu_w, 18.0),
                    );
                    ui.painter().rect_filled(
                        pill,
                        CornerRadius::same(7),
                        Theme::alpha(cpu_color, 28),
                    );
                    ui.painter().text(
                        pill.center(),
                        Align2::CENTER_CENTER,
                        format!("{:.1}%", entry.cpu_percent),
                        FontId::proportional(10.0),
                        cpu_color,
                    );
                    x += cpu_w + gap;

                    ui.painter().text(
                        pos2(x + mem_w - 2.0, center - 5.0),
                        Align2::RIGHT_CENTER,
                        format::bytes(entry.private_bytes),
                        FontId::proportional(10.0),
                        if selected { theme.text } else { theme.dim },
                    );
                    let fraction = (entry.private_bytes as f32 / max_memory as f32).clamp(0.0, 1.0);
                    let track = Rect::from_min_size(
                        pos2(x + 2.0, center + 7.0),
                        vec2(mem_w - 4.0, 3.0),
                    );
                    ui.painter()
                        .rect_filled(track, CornerRadius::same(2), theme.grid);
                    if fraction > 0.0 {
                        let fill = Rect::from_min_size(
                            track.min,
                            vec2(track.width() * fraction, track.height()),
                        );
                        ui.painter()
                            .rect_filled(fill, CornerRadius::same(2), theme.memory);
                    }
                    x += mem_w + gap;

                    ui.painter().text(
                        pos2(x + age_w - 2.0, center),
                        Align2::RIGHT_CENTER,
                        format_age(entry.age_secs),
                        FontId::proportional(10.0),
                        theme.dim,
                    );
                    x += age_w + gap;
                    ui.painter().text(
                        pos2(x + threads_w - 2.0, center),
                        Align2::RIGHT_CENTER,
                        entry.threads.to_string(),
                        FontId::proportional(10.0),
                        theme.faint,
                    );
                    x += threads_w + gap;
                    ui.painter().text(
                        pos2(x + handles_w - 2.0, center),
                        Align2::RIGHT_CENTER,
                        entry.handles.to_string(),
                        FontId::proportional(10.0),
                        theme.faint,
                    );
                    x += handles_w + gap;

                    let kill_rect = Rect::from_center_size(
                        pos2(x + kill_w * 0.5, center),
                        vec2(kill_w, 20.0),
                    );
                    let button = egui::Button::new(
                        RichText::new(lang::t("结束", "End"))
                            .size(9.5)
                            .color(theme.bad),
                    )
                    .fill(Theme::alpha(theme.bad, 10))
                    .stroke(Stroke::new(1.0, Theme::alpha(theme.bad, 60)))
                    .corner_radius(6.0);
                    if ui.put(kill_rect, button).clicked() {
                        kill_request = Some(entry.pid);
                    }
                }
            });

        self.process_paths = paths;
        self.process_icons = process_icons;
        self.process_pending = pending;
        if let Some(pid) = select_request {
            self.process_selected = Some(pid);
            self.process_selected_auto = false;
        } else {
            self.process_selected = selected_pid;
        }
        if let Some((sort, ascending)) = sort_request {
            self.process_sort = sort;
            self.process_ascending = ascending;
        }
        if refresh {
            self.processes = self.process_sampler.sample();
            self.process_at = Instant::now();
        }
        if let Some(pid) = kill_request {
            self.process_selected = Some(pid);
            self.process_selected_auto = false;
            self.process_confirm = Some(pid);
        }

        if let Some(pid) = self.process_confirm {
            let target = self
                .processes
                .entries
                .iter()
                .find(|entry| entry.pid == pid)
                .cloned();
            let name = target
                .as_ref()
                .map(|entry| entry.name.clone())
                .unwrap_or_else(|| format!("PID {pid}"));
            let path = self
                .process_paths
                .get(&pid)
                .and_then(|(_, path)| path.clone());
            let icon = path
                .as_deref()
                .and_then(|path| self.process_icons.get(path).cloned().flatten());
            let cpu = target.as_ref().map(|entry| entry.cpu_percent).unwrap_or(0.0);
            let memory = target
                .as_ref()
                .map(|entry| entry.private_bytes)
                .unwrap_or(0);
            let threads = target.as_ref().map(|entry| entry.threads).unwrap_or(0);
            let mut confirm = false;
            let mut cancel = false;

            let dialog = egui::Frame::new()
                .fill(theme.card)
                .stroke(Stroke::new(1.0, theme.border))
                .corner_radius(14.0)
                .inner_margin(egui::Margin::symmetric(22, 18));
            let modal = egui::Modal::new(Id::unique("monitor-kill-confirm"))
                .backdrop_color(Color32::from_black_alpha(70))
                .frame(dialog)
                .show(ui.ctx(), |ui| {
                    ui.set_width(420.0);
                    ui.horizontal(|ui| {
                        let (icon_rect, _) =
                            ui.allocate_exact_size(vec2(42.0, 42.0), Sense::hover());
                        if let Some(icon) = &icon {
                            ui.painter().image(
                                icon.id(),
                                icon_rect,
                                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                                Color32::WHITE,
                            );
                        } else {
                            paint_fallback_icon(ui.painter(), icon_rect, &name, theme);
                        }
                        ui.add_space(10.0);
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new(lang::t("结束进程", "End process"))
                                    .size(10.5)
                                    .color(theme.bad),
                            );
                            ui.label(
                                RichText::new(&name)
                                    .size(17.0)
                                    .strong()
                                    .color(theme.text),
                            );
                            ui.label(
                                RichText::new(format!("PID {pid}"))
                                    .size(10.0)
                                    .color(theme.faint),
                            );
                        });
                    });
                    ui.add_space(8.0);
                    if let Some(path) = &path {
                        ui.label(
                            RichText::new(format::truncate(path, 64))
                                .size(9.5)
                                .color(theme.faint),
                        );
                    }
                    ui.add_space(10.0);
                    ui.columns_const::<3, _>(|columns| {
                        dialog_stat(
                            &mut columns[0],
                            "CPU",
                            &format!("{cpu:.1}%"),
                            theme.load(cpu),
                            theme,
                        );
                        dialog_stat(
                            &mut columns[1],
                            lang::t("内存", "Memory"),
                            &format::bytes(memory),
                            theme.memory,
                            theme,
                        );
                        dialog_stat(
                            &mut columns[2],
                            lang::t("线程", "Threads"),
                            &threads.to_string(),
                            theme.dim,
                            theme,
                        );
                    });
                    ui.add_space(10.0);
                    egui::Frame::new()
                        .fill(Theme::alpha(theme.warn, 8))
                        .stroke(Stroke::new(1.0, Theme::alpha(theme.warn, 35)))
                        .corner_radius(8.0)
                        .inner_margin(egui::Margin::symmetric(10, 7))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(lang::t(
                                    "结束后，该进程未保存的数据会丢失。",
                                    "Unsaved data in this process will be lost.",
                                ))
                                .size(10.0)
                                .color(theme.warn),
                            );
                        });
                    ui.add_space(12.0);
                    let button_width = (ui.available_width() - 8.0) * 0.5;
                    ui.horizontal(|ui| {
                        if ui
                            .add_sized(
                                [button_width, 32.0],
                                egui::Button::new(lang::t("取消", "Cancel")),
                            )
                            .clicked()
                        {
                            cancel = true;
                        }
                        let end = egui::Button::new(
                            RichText::new(lang::t("结束进程", "End process"))
                                .size(11.0)
                                .color(Color32::WHITE),
                        )
                        .fill(theme.bad)
                        .stroke(Stroke::new(1.0, theme.bad));
                        if ui.add_sized([button_width, 32.0], end).clicked() {
                            confirm = true;
                        }
                    });
                });
            if modal.should_close() {
                cancel = true;
            }
            if confirm {
                match process::kill(pid) {
                    Ok(()) => {
                        self.process_message = Some((
                            if lang::is_en() {
                                format!("Ended {name} (PID {pid})")
                            } else {
                                format!("已结束 {name}（PID {pid}）")
                            },
                            true,
                        ));
                        self.processes = self.process_sampler.sample();
                        self.process_at = Instant::now();
                        self.sampler.sample();
                        self.last_sample = Instant::now();
                    }
                    Err(error) => {
                        let reason = match error {
                            process::KillError::Idle => lang::t(
                                "系统进程不能结束",
                                "system process cannot be ended",
                            )
                            .to_string(),
                            process::KillError::SelfProcess => lang::t(
                                "不能结束 monitor 自身",
                                "cannot end monitor itself",
                            )
                            .to_string(),
                            process::KillError::AccessDenied => lang::t(
                                "拒绝访问（该进程可能需要管理员权限）",
                                "access denied (administrator rights may be required)",
                            )
                            .to_string(),
                            process::KillError::NotFound => lang::t(
                                "进程不存在或已经退出",
                                "process does not exist or has exited",
                            )
                            .to_string(),
                            process::KillError::Other(code) => {
                                if lang::is_en() {
                                    format!("failed (Windows error {code})")
                                } else {
                                    format!("结束失败（Windows 错误 {code}）")
                                }
                            }
                        };
                        self.process_message = Some((
                            if lang::is_en() {
                                format!("{name}: {reason}")
                            } else {
                                format!("{name}：{reason}")
                            },
                            false,
                        ));
                    }
                }
                self.process_confirm = None;
            } else if cancel {
                self.process_confirm = None;
            }
        }
    }
    fn footer(&self, ui: &mut Ui, rect: Rect, theme: &Theme) {
        let snap = &self.sampler.model.snap;
        let painter = ui.painter_at(rect);
        let left = if lang::is_en() {
            format!(
                "self {}  ·  {:.2}% CPU  ·  sample {}  ·  {:.1} fps",
                format::bytes(snap.self_private),
                snap.self_cpu,
                format::micros(snap.tick_us),
                self.fps
            )
        } else {
            format!(
                "自身 {}  ·  {:.2}% CPU  ·  采样 {}  ·  {:.1} 帧/秒",
                format::bytes(snap.self_private),
                snap.self_cpu,
                format::micros(snap.tick_us),
                self.fps
            )
        };
        text(&painter, pos2(rect.left(), rect.center().y), Align2::LEFT_CENTER, left, 10.0, theme.faint);
        text(
            &painter,
            pos2(rect.right(), rect.center().y),
            Align2::RIGHT_CENTER,
            lang::t(
                "空格 暂停  ·  T 主题  ·  悬停图表看数值",
                "space pause · T theme · hover charts for values",
            ),
            10.0,
            theme.faint,
        );
    }
}

impl eframe::App for Dashboard {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        // Cap hover-driven repaints: at most one pointer event per
        // `egui_winit::MOVE_REPAINT_DELAY_MS` reaches egui, so a moving mouse
        // cannot chain full frames (see src/gui/throttle.rs).
        self.mouse.hook(ctx, raw_input);
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        // Frame pacing: `vsync: false` keeps the GL swap off the busy-waiting
        // driver path; DwmFlush re-adds the vblank wait as a proper block.
        throttle::wait_for_vblank();

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
        self.poll_icon_events(ui.ctx());

        let typing = ui.ctx().text_edit_focused();
        if !typing && ui.ctx().input(|input| input.key_pressed(egui::Key::Space)) {
            self.paused = !self.paused;
        }
        if !typing && ui.ctx().input(|input| input.key_pressed(egui::Key::T)) {
            self.dark = !self.dark;
        }
        if !typing && ui.ctx().input(|input| input.key_pressed(egui::Key::L)) {
            lang::toggle();
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
        if self.page == Page::Processes && self.process_at.elapsed() >= Duration::from_secs(5) {
            self.processes = self.process_sampler.sample();
            self.process_at = Instant::now();
        }
        match self.page {
            Page::Overview => self.cards(ui, body, &theme),
            Page::Hardware => hardware_page(ui, body, &theme, &self.sampler.model.snap, &mut self.copied),
            Page::Processes => self.process_page(ui, body, &theme),
        }

        let footer = Rect::from_min_size(
            pos2(full.left() + MARGIN, full.bottom() - FOOTER_H + 3.0),
            vec2((full.width() - 2.0 * MARGIN).max(10.0), FOOTER_H - 6.0),
        );
        self.footer(ui, footer, &theme);
        self.frame_micros = frame_started.elapsed().as_micros() as u64;

        if self.page == Page::Processes {
            let scrolling = ui.ctx().input(|input| {
                input.pointer.any_down() || input.smooth_scroll_delta().length_sq() > 0.0
            });
            if scrolling {
                self.process_scroll_at = Instant::now();
            }
        }
        let smooth = self.page == Page::Processes
            && self.process_scroll_at.elapsed() < Duration::from_millis(400);
        let pending_icons =
            self.page == Page::Processes && !self.process_pending.is_empty();
        let delay = if smooth {
            Duration::from_millis(16)
        } else if pending_icons {
            Duration::from_millis(80)
        } else {
            Duration::from_secs_f64(self.interval)
        };
        ui.ctx().request_repaint_after(delay);
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
    let info = if lang::is_en() {
        format!(
            "{} cores / {} threads · L1 {} · L2 {} · L3 {}",
            cores,
            snap.cpu.logical,
            format::bytes(snap.cpu.cache_l1),
            format::bytes(snap.cpu.cache_l2),
            format::bytes(snap.cpu.cache_l3),
        )
    } else {
        format!(
            "{} 核 / {} 线程 · L1 {} · L2 {} · L3 {}",
            cores,
            snap.cpu.logical,
            format::bytes(snap.cpu.cache_l1),
            format::bytes(snap.cpu.cache_l2),
            format::bytes(snap.cpu.cache_l3),
        )
    };
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
            lang::t("频率", "Freq"),
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
    chart::core_bars(ui, core_rect, content, &snap.cpu.per_core, &snap.cpu.core_mhz, snap.cpu.base_mhz, theme, Id::unique("cpu-cores"));
}

fn memory_card(ui: &mut Ui, rect: Rect, theme: &Theme, snap: &Snapshot) {
    let subtitle = lang::count(snap.memory.processes as usize, "进程", "processes");
    let content = card(ui, rect, theme, lang::t("内存", "Memory"), &subtitle, theme.memory);
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
        lang::t("已用", "used"),
        9.0,
        theme.faint,
    );

    let x = center.x + radius + 22.0;
    let width = content.right() - x;
    if width < 80.0 {
        return;
    }
    let rows = [
        (lang::t("已用", "used"), format::bytes(memory.used()), theme.memory),
        (lang::t("缓存", "cache"), format::bytes(cache as u64), theme.dim),
        (lang::t("空闲", "free"), format::bytes(free as u64), theme.ok),
        (
            lang::t("提交", "commit"),
            format!("{} / {}", format::bytes(memory.commit), format::bytes(memory.commit_limit)),
            theme.text,
        ),
        (
            lang::t("线程", "threads"),
            if lang::is_en() {
                format!("{} · {} handles", memory.threads, memory.handles)
            } else {
                format!("{} 个 · {} 句柄", memory.threads, memory.handles)
            },
            theme.dim,
        ),
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
    let content = card(ui, rect, theme, lang::t("磁盘", "Disk"), "PhysicalDisk", theme.disk);
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
    let read_chip = legend_chip(ui, &painter, pos2(x, y), lang::t("读", "read"), theme.io_read, toggles.read, theme, "legend-disk-read");
    if read_chip.clicked() {
        toggles.read = !toggles.read;
    }
    x = read_chip.rect.right() + 6.0;
    let write_chip = legend_chip(ui, &painter, pos2(x, y), lang::t("写", "write"), theme.io_write, toggles.write, theme, "legend-disk-write");
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
        label: lang::t("读", "read"),
    };
    let write_series = Series {
        values: history.io_write.slice(),
        color: theme.io_write,
        label: lang::t("写", "write"),
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
    copied: &mut Option<(String, Instant)>,
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
    let subtitle = {
        let name = interface.map(|net| pretty(&net.name)).unwrap_or_default();
        match &snap.wifi {
            Some(wifi) if !wifi.ssid.is_empty() => format!("{name} · {}", wifi.ssid),
            _ => name,
        }
    };
    let content = card(ui, rect, theme, lang::t("网络", "Network"), &subtitle, theme.net);
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
        if lang::is_en() {
            format!("total ↓{} ↑{}", format::bytes(interface.map_or(0, |net| net.rx_total)), format::bytes(interface.map_or(0, |net| net.tx_total)))
        } else {
            format!("累计 ↓{} ↑{}", format::bytes(interface.map_or(0, |net| net.rx_total)), format::bytes(interface.map_or(0, |net| net.tx_total)))
        },
        9.0,
        theme.faint,
    );

    let mut x = content.left();
    let rx_chip = legend_chip(ui, &painter, pos2(x, content.top() + 24.0), lang::t("下行", "down"), theme.net_rx, toggles.rx, theme, "legend-net-rx");
    if rx_chip.clicked() {
        toggles.rx = !toggles.rx;
    }
    x = rx_chip.rect.right() + 6.0;
    let tx_chip = legend_chip(ui, &painter, pos2(x, content.top() + 24.0), lang::t("上行", "up"), theme.net_tx, toggles.tx, theme, "legend-net-tx");
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
            label: lang::t("下行", "down"),
        };
        let tx_series = Series {
            values: history.net_tx.slice(),
            color: theme.net_tx,
            label: lang::t("上行", "up"),
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
        // Prefer the address: aggregate rates are already shown at the top.
        if net.ipv4.is_empty() {
            text(
                &painter,
                pos2(content.right(), y),
                Align2::RIGHT_TOP,
                format!("↓{} ↑{}", format::rate(net.rx_bps), format::rate(net.tx_bps)),
                9.5,
                theme.faint,
            );
        } else {
            copy_chip(
                ui,
                &painter,
                pos2(content.right(), y),
                Align2::RIGHT_TOP,
                &net.ipv4,
                9.5,
                theme.net,
                theme,
                copied,
            );
        }
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
    let content = card(ui, rect, theme, lang::t("散热", "Thermals"), "ACPI", theme.temp);
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
                if snap.counters_enabled {
                    lang::t("不可用", "unavailable")
                } else {
                    lang::t("已关闭", "disabled")
                },
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
            label: lang::t("温度", "temp"),
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
            text(&painter, pos2(content.left(), y), Align2::LEFT_TOP, lang::t("风扇", "fan"), 9.5, theme.faint);
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
            text(&painter, pos2(content.left(), y), Align2::LEFT_TOP, lang::t("风扇", "fan"), 9.5, theme.faint);
            text(
                &painter,
                pos2(content.right(), y),
                Align2::RIGHT_TOP,
                lang::t("—  固件未暴露", "—  firmware unavailable"),
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
                crate::sys::power::BatteryState::Charging => lang::t("充电中", "charging"),
                crate::sys::power::BatteryState::Discharging => lang::t("放电中", "discharging"),
                crate::sys::power::BatteryState::Idle => lang::t("交流供电", "AC power"),
                _ => "",
            };
            text(&painter, pos2(content.left(), y), Align2::LEFT_TOP, lang::t("电池", "battery"), 9.5, theme.faint);
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
            text(&painter, pos2(content.left(), y), Align2::LEFT_TOP, lang::t("电池", "battery"), 9.5, theme.faint);
            text(&painter, pos2(content.right(), y), Align2::RIGHT_TOP, "—", 10.5, theme.faint);
        }
    }
}

// --- helpers ----------------------------------------------------------------

fn fit_process_widths(widths: &mut [f32; 8], total: &mut f32, target: f32) {
    let mins = PROCESS_WIDTH_MIN;
    if *total <= 0.0 {
        let numeric = 58.0 + 76.0 + 140.0 + 78.0 + 54.0 + 54.0;
        let flexible = (target - numeric).max(200.0);
        *widths = [
            flexible * 0.58,
            flexible * 0.42,
            58.0,
            76.0,
            140.0,
            78.0,
            54.0,
            54.0,
        ];
    } else if (*total - target).abs() > 0.5 {
        let sum: f32 = widths.iter().sum();
        if sum > 0.0 {
            let scale = target / sum;
            for (width, min) in widths.iter_mut().zip(mins.iter()) {
                *width = (*width * scale).max(*min);
            }
        }
    }
    let sum: f32 = widths.iter().sum();
    widths[0] = (widths[0] + target - sum).max(mins[0]);
    *total = target;
}

fn resize_process_widths(widths: &mut [f32; 8], boundary: usize, delta: f32) {
    if boundary + 1 >= widths.len() {
        return;
    }
    let mins = PROCESS_WIDTH_MIN;
    let left = widths[boundary];
    let right = widths[boundary + 1];
    let delta = delta.clamp(
        -(left - mins[boundary]),
        right - mins[boundary + 1],
    );
    widths[boundary] = left + delta;
    widths[boundary + 1] = right - delta;
}

fn format_age(seconds: u64) -> String {
    lang::age(seconds)
}

fn paint_process_summary(
    painter: &Painter,
    rect: Rect,
    theme: &Theme,
    snap: &Snapshot,
    history: &History,
) {
    let gap = 10.0;
    let width = (rect.width() - gap) * 0.5;
    let cpu_rect = Rect::from_min_size(rect.min, vec2(width, rect.height()));
    let mem_rect = Rect::from_min_size(
        pos2(rect.left() + width + gap, rect.top()),
        vec2(width, rect.height()),
    );

    let cpu = snap.cpu.total;
    let cpu_color = theme.load(cpu);
    paint_summary_tile(painter, cpu_rect, theme, theme.cpu);
    text(
        painter,
        pos2(cpu_rect.left() + 12.0, cpu_rect.top() + 7.0),
        Align2::LEFT_TOP,
        lang::t("CPU 总占用", "CPU total"),
        9.5,
        theme.dim,
    );
    text(
        painter,
        pos2(cpu_rect.left() + 12.0, cpu_rect.top() + 18.0),
        Align2::LEFT_TOP,
        format!("{cpu:.1}%"),
        20.0,
        cpu_color,
    );
    let spark = Rect::from_min_max(
        pos2(cpu_rect.left() + 104.0, cpu_rect.top() + 9.0),
        pos2(cpu_rect.right() - 12.0, cpu_rect.top() + 31.0),
    );
    paint_sparkline(painter, spark, history.cpu.slice(), 100.0, cpu_color);
    let bar = Rect::from_min_size(
        pos2(cpu_rect.left() + 12.0, cpu_rect.bottom() - 10.0),
        vec2(cpu_rect.width() - 24.0, 4.0),
    );
    chart::hbar(painter, bar, cpu / 100.0, cpu_color, theme.grid);

    let memory = &snap.memory;
    paint_summary_tile(painter, mem_rect, theme, theme.memory);
    text(
        painter,
        pos2(mem_rect.left() + 12.0, mem_rect.top() + 7.0),
        Align2::LEFT_TOP,
        lang::t("内存", "Memory"),
        9.5,
        theme.dim,
    );
    text(
        painter,
        pos2(mem_rect.left() + 12.0, mem_rect.top() + 18.0),
        Align2::LEFT_TOP,
        format::bytes(memory.used()),
        20.0,
        theme.text,
    );
    text(
        painter,
        pos2(mem_rect.right() - 12.0, mem_rect.top() + 24.0),
        Align2::RIGHT_TOP,
        format!(" / {}", format::bytes(memory.total)),
        9.5,
        theme.faint,
    );
    let bar = Rect::from_min_size(
        pos2(mem_rect.left() + 12.0, mem_rect.bottom() - 10.0),
        vec2(mem_rect.width() - 24.0, 4.0),
    );
    chart::hbar(
        painter,
        bar,
        memory.percent() / 100.0,
        theme.memory,
        theme.grid,
    );
}

fn paint_summary_tile(painter: &Painter, rect: Rect, theme: &Theme, accent: Color32) {
    painter.rect_filled(rect, CornerRadius::same(8), Theme::alpha(accent, 8));
    painter.rect_stroke(
        rect,
        CornerRadius::same(8),
        Stroke::new(1.0, Theme::alpha(theme.border, 160)),
        StrokeKind::Inside,
    );
}

fn paint_sparkline(painter: &Painter, rect: Rect, values: &[f32], max: f32, color: Color32) {
    let visible = values.len().saturating_sub(60);
    let values = &values[visible..];
    if values.len() < 2 || rect.width() < 8.0 {
        return;
    }
    let mut previous = None;
    for (index, value) in values.iter().enumerate() {
        let x = rect.left()
            + rect.width() * index as f32 / (values.len() - 1) as f32;
        let y = rect.bottom() - rect.height() * (value / max).clamp(0.0, 1.0);
        let point = pos2(x, y);
        if let Some(previous) = previous {
            painter.line_segment([previous, point], Stroke::new(1.2, color));
        }
        previous = Some(point);
    }
}

fn dialog_stat(
    ui: &mut Ui,
    label: &str,
    value: &str,
    color: Color32,
    theme: &Theme,
) {
    egui::Frame::new()
        .fill(Theme::alpha(color, 10))
        .stroke(Stroke::new(1.0, Theme::alpha(color, 35)))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(label).size(9.0).color(theme.faint));
            ui.label(RichText::new(value).size(14.5).strong().color(color));
        });
}

fn stat_pill(ui: &mut Ui, label: &str, color: Color32) {
    egui::Frame::new()
        .fill(Theme::alpha(color, 18))
        .corner_radius(9.0)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(label).size(9.5).color(color));
        });
}

fn request_path(
    cache: &mut HashMap<u32, (u64, Option<String>)>,
    pending: &mut HashMap<u32, u64>,
    tx: &Sender<IconRequest>,
    pid: u32,
    create_time: u64,
) -> Option<String> {
    if let Some((stamp, path)) = cache.get(&pid) {
        if *stamp == create_time {
            return path.clone();
        }
    }
    if pending.get(&pid) != Some(&create_time) {
        if tx
            .send(IconRequest {
                pid,
                create_time,
            })
            .is_ok()
        {
            pending.insert(pid, create_time);
        }
    }
    None
}

fn spawn_icon_worker() -> (Sender<IconRequest>, Receiver<IconResult>) {
    let (request_tx, request_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("monitor-icons".to_string())
        .spawn(move || icon_worker(request_rx, result_tx));
    (request_tx, result_rx)
}

fn icon_worker(rx: Receiver<IconRequest>, tx: Sender<IconResult>) {
    let mut cache: HashMap<String, Option<egui::ColorImage>> = HashMap::new();
    while let Ok(request) = rx.recv() {
        let path = process::image_path(request.pid);
        let image = path.as_deref().and_then(|path| {
            if let Some(image) = cache.get(path) {
                return image.clone();
            }
            let image = icons::load(path);
            cache.insert(path.to_string(), image.clone());
            image
        });
        if tx
            .send(IconResult {
                pid: request.pid,
                create_time: request.create_time,
                path,
                image,
            })
            .is_err()
        {
            break;
        }
    }
}

fn paint_fallback_icon(painter: &Painter, rect: Rect, name: &str, theme: &Theme) {
    let color = process_color(name, theme);
    painter.circle_filled(rect.center(), rect.width() * 0.5, Theme::alpha(color, 36));
    let initial = name
        .chars()
        .next()
        .map(|ch| ch.to_uppercase().collect::<String>())
        .unwrap_or_else(|| "?".to_string());
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        initial,
        FontId::proportional(9.5),
        color,
    );
}

fn process_color(name: &str, theme: &Theme) -> Color32 {
    let hash = name.bytes().fold(0u32, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(byte as u32)
    });
    let colors = [
        theme.accent,
        theme.cpu,
        theme.memory,
        theme.disk,
        theme.net,
        theme.temp,
    ];
    colors[hash as usize % colors.len()]
}

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
// --- hardware page ----------------------------------------------------------

fn gpu_card(ui: &mut Ui, rect: Rect, theme: &Theme, gpus: &[Gpu]) {
    let Some(first) = gpus.first() else {
        return;
    };
    let subtitle = if gpus.len() > 1 {
        format!("{} · +{}", pretty(&first.name), gpus.len() - 1)
    } else {
        pretty(&first.name)
    };
    let content = card(ui, rect, theme, "GPU", &subtitle, theme.accent);
    let painter = ui.painter_at(content);
    let mut y = content.top() + 2.0;

    text(
        &painter,
        pos2(content.left(), y),
        Align2::LEFT_TOP,
        format!("{:.0}%", first.utilization),
        28.0,
        theme.load(first.utilization),
    );
    text(
        &painter,
        pos2(content.left() + 76.0, y + 22.0),
        Align2::LEFT_TOP,
        lang::t("利用率", "load"),
        10.0,
        theme.dim,
    );
    let bar = Rect::from_min_size(pos2(content.left(), y + 38.0), vec2(content.width(), 6.0));
    chart::hbar(&painter, bar, first.utilization / 100.0, theme.load(first.utilization), theme.grid);
    y += 54.0;

    let (memory_label, memory_used, memory_limit) = if first.dedicated_limit > 0 {
        (
            lang::t("专用显存", "dedicated"),
            first.dedicated_used,
            first.dedicated_limit,
        )
    } else {
        (
            lang::t("共享显存", "shared"),
            first.shared_used,
            0,
        )
    };
    text(
        &painter,
        pos2(content.left(), y),
        Align2::LEFT_TOP,
        memory_label,
        9.5,
        theme.faint,
    );
    let memory_text = if memory_limit > 0 {
        format!("{} / {}", format::bytes(memory_used), format::bytes(memory_limit))
    } else {
        format::bytes(memory_used)
    };
    text(
        &painter,
        pos2(content.right(), y),
        Align2::RIGHT_TOP,
        memory_text,
        10.0,
        theme.text,
    );
    if memory_limit > 0 {
        let bar = Rect::from_min_size(pos2(content.left(), y + 18.0), vec2(content.width(), 5.0));
        chart::hbar(
            &painter,
            bar,
            (memory_used as f32 / memory_limit as f32).clamp(0.0, 1.0),
            theme.memory,
            theme.grid,
        );
    }
    y += 34.0;

    if first.dedicated_limit > 0 {
        text(
            &painter,
            pos2(content.left(), y),
            Align2::LEFT_TOP,
            lang::t("共享显存", "shared"),
            9.5,
            theme.faint,
        );
        text(
            &painter,
            pos2(content.right(), y),
            Align2::RIGHT_TOP,
            format::bytes(first.shared_used),
            10.0,
            theme.dim,
        );
        y += 28.0;
    }

    text(
        &painter,
        pos2(content.left(), y),
        Align2::LEFT_TOP,
        lang::t("引擎", "engines"),
        9.5,
        theme.faint,
    );
    y += 18.0;
    let mut x = content.left();
    for engine in first.engines.iter().take(5) {
        let label = format!("{} {:.0}%", engine.name, engine.utilization);
        let color = if engine.utilization > 0.05 {
            theme.load(engine.utilization)
        } else {
            theme.faint
        };
        let chip_rect = chip(&painter, pos2(x, y), &label, color, theme);
        x = chip_rect.right() + 6.0;
        if x >= content.right() - 20.0 {
            break;
        }
    }
    if gpus.len() > 1 {
        text(
            &painter,
            pos2(content.left(), content.bottom() - 2.0),
            Align2::LEFT_BOTTOM,
            if lang::is_en() {
                format!("+{} adapters", gpus.len() - 1)
            } else {
                format!("+{} 个适配器", gpus.len() - 1)
            },
            9.0,
            theme.faint,
        );
    }
}

fn hardware_page(
    ui: &mut Ui,
    body: Rect,
    theme: &Theme,
    snap: &Snapshot,
    copied: &mut Option<(String, Instant)>,
) {
    let hardware = &snap.hardware;
    let gap = GAP;
    let row_height = (body.height() - gap) * 0.5;
    let has_gpu = !snap.gpus.is_empty();
    let (top_left, top_mid, top_right) = if has_gpu {
        let top = split_row(body.left(), body.top(), body.width(), row_height, &[1.0, 1.0, 1.0]);
        (top[0], Some(top[1]), top[2])
    } else {
        let half = (body.width() - gap) * 0.5;
        (
            Rect::from_min_size(body.min, vec2(half, row_height)),
            None,
            Rect::from_min_size(
                pos2(body.left() + half + gap, body.top()),
                vec2(half, row_height),
            ),
        )
    };
    let bottom_left = Rect::from_min_size(
        pos2(body.left(), body.top() + row_height + gap),
        vec2((body.width() - gap) * 0.5, row_height),
    );
    let bottom_right = Rect::from_min_size(
        pos2(body.left() + (body.width() - gap) * 0.5 + gap, body.top() + row_height + gap),
        vec2((body.width() - gap) * 0.5, row_height),
    );

    // Board + BIOS
    let content = card(ui, top_left, theme, lang::t("主板与 BIOS", "Board & BIOS"), &hardware.board_product, theme.cpu);
    let painter = ui.painter_at(content);
    let mut y = content.top() + 10.0;
    let mut row = |label: &str, value: &str| {
        info_row(&painter, content, y, theme, label, value);
        y += 34.0;
    };
    row(lang::t("主板", "board"), &format!("{} {}", hardware.board_vendor, hardware.board_product));
    row(lang::t("板型版本", "revision"), &hardware.board_version);
    row("BIOS", &format!("{} {}", hardware.bios_vendor, hardware.bios_version));
    row(lang::t("发布日期", "release"), &hardware.bios_date);
    row("SMBIOS", &hardware.smbios_version);
    let slots = if lang::is_en() {
        format!("{} / {} used", hardware.slots_used(), hardware.slots_total)
    } else {
        format!("{}/{} 已用", hardware.slots_used(), hardware.slots_total)
    };
    row(lang::t("内存插槽", "slots"), &slots);
    text(
        &painter,
        pos2(content.left(), content.bottom() - 4.0),
        Align2::LEFT_BOTTOM,
        lang::t(
            "来源：SMBIOS 固件表 / 存储设备描述 / 注册表，免管理员",
            "Source: SMBIOS / storage descriptors / registry, no admin required",
        ),
        9.0,
        theme.faint,
    );

    if let Some(rect) = top_mid {
        gpu_card(ui, rect, theme, &snap.gpus);
    }

    // Memory modules
    let content = card(
        ui,
        top_right,
        theme,
        lang::t("内存", "Memory"),
        &lang::count(hardware.modules.len(), "条", "modules"),
        theme.memory,
    );
    let painter = ui.painter_at(content);
    let mut y = content.top() + 6.0;
    let speed = hardware.modules.iter().map(|module| module.speed_mts).max().unwrap_or(0);
    let kind = hardware.modules.first().map(|module| module.kind.clone()).unwrap_or_default();
    info_row(&painter, content, y, theme, lang::t("总容量", "capacity"), &format::bytes(hardware.memory_total()));
    y += 30.0;
    info_row(
        &painter,
        content,
        y,
        theme,
        lang::t("类型 / 速度", "type / speed"),
        &format!("{kind} {speed} MT/s"),
    );
    y += 30.0;
    info_row(
        &painter,
        content,
        y,
        theme,
        lang::t("插槽", "slots"),
        &format!("{}/{}", hardware.slots_used(), hardware.slots_total),
    );
    y += 34.0;

    text(&painter, pos2(content.left(), y), Align2::LEFT_TOP, lang::t("每条明细", "modules"), 9.0, theme.faint);
    y += 20.0;
    for module in hardware.modules.iter().take(8) {
        let label = format!(
            "{} · {} MB · {} MT/s",
            module.slot,
            module.size_bytes / 1024 / 1024,
            module.speed_mts
        );
        text(
            &painter,
            pos2(content.left(), y),
            Align2::LEFT_TOP,
            format::truncate(&label, (content.width() / 6.6) as usize),
            10.0,
            theme.dim,
        );
        y += 21.0;
    }

    // Storage
    let content = card(ui, bottom_left, theme, lang::t("存储", "Storage"), "", theme.disk);
    let painter = ui.painter_at(content);
    let mut y = content.top() + 6.0;
    for model in hardware.disks.iter().take(4) {
        text(
            &painter,
            pos2(content.left(), y),
            Align2::LEFT_TOP,
            format::truncate(model, (content.width() / 6.2) as usize),
            11.0,
            theme.text,
        );
        y += 24.0;
    }
    y += 6.0;
    for volume in snap.volumes.iter().take(4) {
        let label = format!(
            "{}: {} / {} · {:.0}%",
            volume.letter,
            format::bytes(volume.used()),
            format::bytes(volume.total),
            volume.percent()
        );
        text(&painter, pos2(content.left(), y), Align2::LEFT_TOP, label, 10.0, theme.dim);
        y += 20.0;
    }

    // Network
    let content = card(ui, bottom_right, theme, lang::t("网络", "Network"), "", theme.net);
    let painter = ui.painter_at(content);
    let mut y = content.top() + 4.0;
    for net in snap.nets.iter().filter(|net| net.up).take(3) {
        text(
            &painter,
            pos2(content.left(), y),
            Align2::LEFT_TOP,
            format::truncate(&pretty(&net.name), (content.width() / 6.4) as usize),
            10.5,
            theme.text,
        );
        y += 18.0;
        let speed = link_speed(net.link_bits);
        let speed_rect = painter.text(
            pos2(content.left() + 8.0, y),
            Align2::LEFT_TOP,
            &speed,
            FontId::proportional(9.5),
            theme.dim,
        );
        if net.ipv4.is_empty() {
            painter.text(
                pos2(speed_rect.right() + 8.0, y),
                Align2::LEFT_TOP,
                lang::t("未获取到 IPv4", "no IPv4 address"),
                FontId::proportional(9.5),
                theme.faint,
            );
        } else {
            copy_chip(
                ui,
                &painter,
                pos2(speed_rect.right() + 12.0, y),
                Align2::LEFT_TOP,
                &net.ipv4,
                10.0,
                theme.net,
                theme,
                copied,
            );
        }
        y += 24.0;
    }

    match &snap.wifi {
        Some(wifi) => {
            let label = if lang::is_en() {
                format!(
                    "{} · signal {}% · {} · ↓{} ↑{}",
                    wifi.ssid,
                    wifi.signal,
                    wifi.phy,
                    rate_from_kbps(wifi.rx_kbps),
                    rate_from_kbps(wifi.tx_kbps)
                )
            } else {
                format!(
                    "{} · 信号 {}% · {} · ↓{} ↑{}",
                    wifi.ssid,
                    wifi.signal,
                    wifi.phy,
                    rate_from_kbps(wifi.rx_kbps),
                    rate_from_kbps(wifi.tx_kbps)
                )
            };
            text(
                &painter,
                pos2(content.left(), y + 4.0),
                Align2::LEFT_TOP,
                format::truncate(&label, (content.width() / 6.2) as usize),
                10.0,
                theme.net,
            );
        }
        None => {
            if snap.nets.iter().any(|net| net.name.contains("Wi-Fi") || net.name.contains("WLAN")) {
                let hint = painter.text(
                    pos2(content.left(), y + 6.0),
                    Align2::LEFT_TOP,
                    lang::t("Wi-Fi 名称需要定位权限", "Wi-Fi name needs location permission"),
                    FontId::proportional(9.5),
                    theme.faint,
                );
                let button = chip_button(
                    ui,
                    &painter,
                    pos2(hint.right() + 8.0, y + 2.0),
                    Align2::LEFT_TOP,
                    lang::t("打开定位设置", "Open location settings"),
                    theme.accent,
                );
                if button.clicked() {
                    crate::sys::clipboard::open_uri("ms-settings:privacy-location");
                }
            }
        }
    }
}

fn info_row(painter: &Painter, content: Rect, y: f32, theme: &Theme, label: &str, value: &str) {
    text(painter, pos2(content.left(), y), Align2::LEFT_TOP, label, 9.5, theme.faint);
    text(
        painter,
        pos2(content.right(), y),
        Align2::RIGHT_TOP,
        format::truncate(value, (content.width() / 12.0) as usize),
        11.0,
        theme.text,
    );
}

fn link_speed(bits_per_sec: u64) -> String {
    if bits_per_sec >= 1_000_000_000 {
        format!("{:.1} Gbps", bits_per_sec as f64 / 1e9)
    } else if bits_per_sec >= 1_000_000 {
        format!("{:.0} Mbps", bits_per_sec as f64 / 1e6)
    } else {
        "—".to_string()
    }
}

fn rate_from_kbps(kbps: u32) -> String {
    if kbps >= 1000 {
        format!("{:.0} Mbps", kbps as f64 / 1000.0)
    } else {
        format!("{kbps} Kbps")
    }
}
/// A value that copies itself to the clipboard when clicked.
#[allow(clippy::too_many_arguments)]
fn copy_chip(
    ui: &mut Ui,
    painter: &Painter,
    pos: Pos2,
    anchor: Align2,
    value: &str,
    size: f32,
    color: Color32,
    theme: &Theme,
    copied: &mut Option<(String, Instant)>,
) {
    let label = if lang::is_en() {
        format!("{value}  copy")
    } else {
        format!("{value}  复制")
    };
    let galley = painter.layout_no_wrap(label.clone(), FontId::proportional(size), color);
    let rect = anchor.anchor_size(pos, galley.size() + vec2(34.0, 10.0));
    let response = ui.put(
        rect,
        egui::Button::new(RichText::new(label).size(size).color(color))
            .wrap_mode(egui::TextWrapMode::Extend),
    );
    if response.clicked() && crate::sys::clipboard::set_text(value) {
        *copied = Some((value.to_string(), Instant::now()));
        ui.ctx().request_repaint_after(Duration::from_millis(1600));
    }

    if let Some((text, at)) = copied.as_ref() {
        if text == value && at.elapsed() < Duration::from_millis(1500) {
            painter.text(
                pos2(rect.right() + 6.0, rect.center().y),
                Align2::LEFT_CENTER,
                lang::t("已复制", "copied"),
                FontId::proportional(9.0),
                theme.ok,
            );
        }
    }
}

/// Small rounded label that behaves like a button.
fn chip_button(
    ui: &mut Ui,
    painter: &Painter,
    pos: Pos2,
    anchor: Align2,
    label: &str,
    color: Color32,
) -> egui::Response {
    let galley = painter.layout_no_wrap(label.to_owned(), FontId::proportional(9.5), color);
    let rect = anchor.anchor_size(pos, galley.size() + vec2(34.0, 10.0));
    ui.put(
        rect,
        egui::Button::new(RichText::new(label).size(9.5).color(color))
            .wrap_mode(egui::TextWrapMode::Extend),
    )
}
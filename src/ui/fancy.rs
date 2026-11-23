//! "Control room" shell: a 24-bit card dashboard for the terminal.
//!
//! This is an optional renderer on top of the same [`Model`] and history rings
//! as the classic TUI. It draws a full-screen dark canvas, six rounded cards and
//! a few small charts; every frame is still assembled into [`Line`]s and written
//! with a single `WriteConsoleW` by `term::Terminal`.
//!
//! The layout needs room: below [`MIN_WIDTH`] × [`MIN_HEIGHT`] the caller keeps
//! the classic renderer, which is deliberately compact.

use crate::format;
use crate::metrics::{Model, Net};

use super::style::{Color, Style};
use super::text::{self, Line};

/// Narrower or shorter windows keep the classic layout.
pub const MIN_WIDTH: usize = 100;
pub const MIN_HEIGHT: usize = 30;

const HEIGHT_BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

pub fn fits(width: usize, height: usize) -> bool {
    width >= MIN_WIDTH && height >= MIN_HEIGHT
}

// --------------------------------------------------------------------- palette

/// Colours for the card shell, in truecolor or an xterm-256 approximation.
#[derive(Clone, Copy)]
pub struct Palette {
    pub truecolor: bool,
    pub screen: Color,
    pub card: Color,
    pub border: Color,
    pub track: Color,
    pub text: Color,
    pub dim: Color,
    pub faint: Color,
    pub accent: Color,
    pub cpu: Color,
    pub memory: Color,
    pub disk: Color,
    pub net: Color,
    pub temp: Color,
    pub battery: Color,
    pub ok: Color,
    pub warn: Color,
    pub bad: Color,
}

impl Palette {
    pub fn new(theme: &str, truecolor: bool) -> Self {
        if truecolor {
            if theme == "light" {
                Self::light_true()
            } else {
                Self::dark_true()
            }
        } else if theme == "light" {
            Self::light_256()
        } else {
            Self::dark_256()
        }
    }

    fn dark_true() -> Self {
        Self {
            truecolor: true,
            screen: Color::True(11, 15, 22),
            card: Color::True(17, 24, 38),
            border: Color::True(38, 52, 74),
            track: Color::True(30, 42, 61),
            text: Color::True(230, 237, 243),
            dim: Color::True(139, 152, 169),
            faint: Color::True(91, 105, 120),
            accent: Color::True(76, 201, 240),
            cpu: Color::True(94, 234, 212),
            memory: Color::True(167, 139, 250),
            disk: Color::True(251, 191, 36),
            net: Color::True(56, 189, 248),
            temp: Color::True(251, 113, 133),
            battery: Color::True(52, 211, 153),
            ok: Color::True(52, 211, 153),
            warn: Color::True(251, 191, 36),
            bad: Color::True(251, 113, 133),
        }
    }

    fn light_true() -> Self {
        Self {
            truecolor: true,
            screen: Color::True(244, 246, 249),
            card: Color::True(255, 255, 255),
            border: Color::True(213, 220, 230),
            track: Color::True(232, 236, 242),
            text: Color::True(26, 34, 48),
            dim: Color::True(91, 101, 117),
            faint: Color::True(138, 149, 165),
            accent: Color::True(14, 116, 144),
            cpu: Color::True(15, 118, 110),
            memory: Color::True(109, 40, 217),
            disk: Color::True(180, 83, 9),
            net: Color::True(3, 105, 161),
            temp: Color::True(190, 18, 60),
            battery: Color::True(21, 128, 61),
            ok: Color::True(21, 128, 61),
            warn: Color::True(180, 83, 9),
            bad: Color::True(190, 18, 60),
        }
    }

    fn dark_256() -> Self {
        Self {
            truecolor: false,
            screen: Color::Indexed(232),
            card: Color::Indexed(234),
            border: Color::Indexed(238),
            track: Color::Indexed(236),
            text: Color::Indexed(252),
            dim: Color::Indexed(245),
            faint: Color::Indexed(240),
            accent: Color::Indexed(45),
            cpu: Color::Indexed(44),
            memory: Color::Indexed(141),
            disk: Color::Indexed(214),
            net: Color::Indexed(39),
            temp: Color::Indexed(203),
            battery: Color::Indexed(42),
            ok: Color::Indexed(42),
            warn: Color::Indexed(214),
            bad: Color::Indexed(203),
        }
    }

    fn light_256() -> Self {
        Self {
            truecolor: false,
            screen: Color::Indexed(254),
            card: Color::Indexed(255),
            border: Color::Indexed(250),
            track: Color::Indexed(253),
            text: Color::Indexed(235),
            dim: Color::Indexed(242),
            faint: Color::Indexed(245),
            accent: Color::Indexed(30),
            cpu: Color::Indexed(30),
            memory: Color::Indexed(91),
            disk: Color::Indexed(130),
            net: Color::Indexed(25),
            temp: Color::Indexed(124),
            battery: Color::Indexed(28),
            ok: Color::Indexed(28),
            warn: Color::Indexed(130),
            bad: Color::Indexed(124),
        }
    }

    fn mix(&self, from: Color, to: Color, t: f32) -> Color {
        match (from, to) {
            (Color::True(r1, g1, b1), Color::True(r2, g2, b2)) => {
                let t = t.clamp(0.0, 1.0);
                let fade = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
                Color::True(fade(r1, r2), fade(g1, g2), fade(b1, b2))
            }
            _ => {
                if t < 0.5 {
                    from
                } else {
                    to
                }
            }
        }
    }

    /// Load-based colour, same thresholds as the classic palette.
    fn load(&self, percent: f32) -> Color {
        if percent >= 85.0 {
            self.bad
        } else if percent >= 60.0 {
            self.warn
        } else {
            self.ok
        }
    }

    fn temperature(&self, celsius: f32) -> Color {
        if celsius >= 90.0 {
            self.bad
        } else if celsius >= 75.0 {
            self.warn
        } else {
            self.ok
        }
    }

    /// Blue → mint → amber → red, for the per-core heat strip.
    fn heat(&self, percent: f32) -> Color {
        if self.truecolor {
            let stops = [
                Color::True(56, 189, 248),
                Color::True(52, 211, 153),
                Color::True(251, 191, 36),
                Color::True(251, 113, 133),
            ];
            let position = percent.clamp(0.0, 100.0) / 100.0 * 3.0;
            let index = (position as usize).min(2);
            self.mix(stops[index], stops[index + 1], position - index as f32)
        } else if percent >= 85.0 {
            self.bad
        } else if percent >= 60.0 {
            self.warn
        } else if percent >= 25.0 {
            self.cpu
        } else {
            self.net
        }
    }

    fn grade(&self, celsius: f32) -> Color {
        if self.truecolor {
            let stops = [
                Color::True(52, 211, 153),
                Color::True(251, 191, 36),
                Color::True(251, 113, 133),
            ];
            let position = ((celsius - 20.0) / 85.0).clamp(0.0, 1.0) * 2.0;
            let index = (position as usize).min(1);
            self.mix(stops[index], stops[index + 1], position - index as f32)
        } else {
            self.temperature(celsius)
        }
    }
}

// ------------------------------------------------------------------------ grid

#[derive(Clone, Copy)]
struct Cell {
    ch: char,
    style: Style,
}

struct Grid {
    width: usize,
    height: usize,
    cells: Vec<Cell>,
}

impl Grid {
    fn new(width: usize, height: usize, style: Style) -> Self {
        Self {
            width,
            height,
            cells: vec![Cell { ch: ' ', style }; width * height],
        }
    }

    fn set(&mut self, x: usize, y: usize, ch: char, style: Style) {
        if x >= self.width || y >= self.height {
            return;
        }
        self.cells[y * self.width + x] = Cell { ch, style };
    }

    fn put(&mut self, x: usize, y: usize, value: &str, style: Style) {
        let mut column = x;
        for ch in value.chars() {
            let width = text::char_width(ch);
            if width == 0 {
                continue;
            }
            if column + width > self.width {
                break;
            }
            self.set(column, y, ch, style);
            if width == 2 {
                self.set(column + 1, y, '\0', style);
            }
            column += width;
        }
    }

    fn fill(&mut self, x: usize, y: usize, width: usize, height: usize, ch: char, style: Style) {
        for row in y..(y + height).min(self.height) {
            for column in x..(x + width).min(self.width) {
                self.set(column, row, ch, style);
            }
        }
    }

    fn to_lines(&self) -> Vec<Line> {
        let mut lines = Vec::with_capacity(self.height);
        for row in 0..self.height {
            let mut line = Line::new();
            let mut text = String::new();
            let mut run = Style::PLAIN;
            let mut started = false;
            for cell in &self.cells[row * self.width..(row + 1) * self.width] {
                if cell.ch == '\0' {
                    continue;
                }
                if !started {
                    run = cell.style;
                    started = true;
                } else if cell.style != run {
                    line.push(&text, run);
                    text.clear();
                    run = cell.style;
                }
                text.push(cell.ch);
            }
            if !text.is_empty() {
                line.push(&text, run);
            }
            lines.push(line);
        }
        lines
    }
}

// --------------------------------------------------------------------- helpers

fn pair(fg: Color, bg: Color) -> Style {
    Style::pair(fg, bg)
}

fn pair_bold(fg: Color, bg: Color) -> Style {
    Style::pair_bold(fg, bg)
}

fn advance(g: &mut Grid, line: &str, style: Style, x: &mut usize, y: usize) {
    g.put(*x, y, line, style);
    *x += text::display_width(line);
}

/// Append when `text` still fits before `limit`; returns whether it was drawn.
fn try_advance(
    g: &mut Grid,
    line: &str,
    style: Style,
    x: &mut usize,
    y: usize,
    limit: usize,
) -> bool {
    let width = text::display_width(line);
    if *x + width > limit {
        return false;
    }
    advance(g, line, style, x, y);
    true
}

fn put_right(g: &mut Grid, end: usize, y: usize, value: &str, style: Style) {
    let width = text::display_width(value);
    if width <= end {
        g.put(end - width, y, value, style);
    }
}

/// Last 60 seconds of a history ring, honouring the current refresh interval.
fn window<'a>(values: &'a [f32], interval: f64) -> &'a [f32] {
    let keep = ((60.0 / interval.max(0.1)).round() as usize)
        .max(1)
        .min(values.len());
    &values[values.len() - keep..]
}

struct Rect {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

/// Draw one rounded card and return its interior.
fn panel(
    g: &mut Grid,
    rect: Rect,
    palette: &Palette,
    title: &str,
    title_color: Color,
    right: &str,
) -> Option<Rect> {
    if rect.w < 6 || rect.h < 3 {
        return None;
    }
    g.fill(
        rect.x,
        rect.y,
        rect.w,
        rect.h,
        ' ',
        pair(palette.text, palette.card),
    );
    let border = pair(palette.border, palette.card);
    g.put(rect.x, rect.y, "╭", border);
    g.put(rect.x + rect.w - 1, rect.y, "╮", border);
    g.put(rect.x, rect.y + rect.h - 1, "╰", border);
    g.put(rect.x + rect.w - 1, rect.y + rect.h - 1, "╯", border);
    g.put(rect.x + 1, rect.y, &"─".repeat(rect.w - 2), border);
    g.put(
        rect.x + 1,
        rect.y + rect.h - 1,
        &"─".repeat(rect.w - 2),
        border,
    );
    for row in rect.y + 1..rect.y + rect.h - 1 {
        g.put(rect.x, row, "│", border);
        g.put(rect.x + rect.w - 1, row, "│", border);
    }
    g.put(
        rect.x + 2,
        rect.y,
        &format!(" {title} "),
        pair_bold(title_color, palette.card),
    );
    if !right.is_empty() {
        let width = text::display_width(right) + 2;
        if rect.w > width + text::display_width(title) + 8 {
            put_right(
                g,
                rect.x + rect.w - 2,
                rect.y,
                &format!(" {right} "),
                pair(palette.faint, palette.card),
            );
        }
    }
    Some(Rect {
        x: rect.x + 1,
        y: rect.y + 1,
        w: rect.w - 2,
        h: rect.h - 2,
    })
}

fn gauge(
    g: &mut Grid,
    x: usize,
    y: usize,
    width: usize,
    percent: f32,
    from: Color,
    to: Color,
    palette: &Palette,
) {
    if width == 0 {
        return;
    }
    let percent = percent.clamp(0.0, 100.0);
    let filled = ((percent / 100.0) * width as f32).round() as usize;
    for column in 0..width {
        let color = if column < filled {
            palette.mix(from, to, column as f32 / width.max(1) as f32)
        } else {
            palette.track
        };
        g.set(x + column, y, ' ', pair(color, color));
    }
}

fn area_chart(
    g: &mut Grid,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    values: &[f32],
    max: f32,
    bottom: Color,
    top: Color,
    palette: &Palette,
) {
    if width == 0 || height == 0 || values.is_empty() {
        return;
    }
    for column in 0..width {
        let index = ((column as f32 / width as f32) * values.len() as f32) as usize;
        let value = values.get(index).copied().unwrap_or(0.0);
        let scaled = (value / max.max(1.0)).clamp(0.0, 1.0) * height as f32;
        let full = scaled as usize;
        let fraction = scaled - full as f32;
        for row in 0..height {
            let target = y + height - 1 - row;
            let color = palette.mix(bottom, top, row as f32 / height.max(1) as f32);
            if row < full {
                g.set(x + column, target, '█', pair(color, palette.card));
            } else if row == full && fraction > 0.02 {
                let level = (fraction * 8.0).ceil().clamp(1.0, 8.0) as usize;
                g.set(
                    x + column,
                    target,
                    HEIGHT_BLOCKS[level - 1],
                    pair(color, palette.card),
                );
            }
        }
    }
}

fn spark(
    g: &mut Grid,
    x: usize,
    y: usize,
    width: usize,
    values: &[f32],
    max: f32,
    color: Color,
    palette: &Palette,
) {
    if width == 0 || values.is_empty() {
        return;
    }
    for column in 0..width {
        let index = ((column as f32 / width as f32) * values.len() as f32) as usize;
        let value = values.get(index).copied().unwrap_or(0.0);
        let level = ((value / max.max(1.0)).clamp(0.0, 1.0) * 7.0).round() as usize;
        g.set(
            x + column,
            y,
            HEIGHT_BLOCKS[level],
            pair(color, palette.card),
        );
    }
}

// -------------------------------------------------------------------- sections

pub fn render(model: &Model, width: usize, height: usize, palette: &Palette) -> Vec<Line> {
    let background = pair(palette.text, palette.screen);
    let mut grid = Grid::new(width, height, background);
    header(&mut grid, model, width, palette);
    footer(&mut grid, model, width, height, palette);

    let inner = width.saturating_sub(2);
    let gap = 2;
    let left = ((inner.saturating_sub(gap)) as f32 * 0.585).round() as usize;
    let left = left.clamp(40, inner.saturating_sub(40));
    let right = inner.saturating_sub(gap + left);
    let right_x = 1 + left + gap;

    let tall = height >= 33;
    let card_end = if tall { height - 4 } else { height - 3 };
    let area_height = card_end.saturating_sub(3) + 1;
    let sum = area_height.saturating_sub(2);
    let (a_height, b_height) = if sum >= 26 {
        (11, 8)
    } else if sum >= 24 {
        (10, 8)
    } else {
        (10, 7)
    };
    let c_height = sum.saturating_sub(a_height + b_height).max(4);

    let a = Rect {
        x: 1,
        y: 3,
        w: left,
        h: a_height,
    };
    let b = Rect {
        x: 1,
        y: 3 + a_height + 1,
        w: left,
        h: b_height,
    };
    let c = Rect {
        x: 1,
        y: 3 + a_height + 1 + b_height + 1,
        w: left,
        h: c_height,
    };
    let a_right = Rect {
        x: right_x,
        y: 3,
        w: right,
        h: a_height,
    };
    let b_right = Rect {
        x: right_x,
        y: 3 + a_height + 1,
        w: right,
        h: b_height,
    };
    let c_right = Rect {
        x: right_x,
        y: 3 + a_height + 1 + b_height + 1,
        w: right,
        h: c_height,
    };

    cpu_card(&mut grid, a, model, palette);
    memory_card(&mut grid, a_right, model, palette);
    disk_card(&mut grid, b, model, palette);
    network_card(&mut grid, b_right, model, palette);
    thermal_card(&mut grid, c, model, palette);
    self_card(&mut grid, c_right, model, palette);
    grid.to_lines()
}

fn header(grid: &mut Grid, model: &Model, width: usize, palette: &Palette) {
    let bg = palette.screen;
    grid.put(1, 0, "◆", pair_bold(palette.accent, bg));
    grid.put(3, 0, "monitor", pair_bold(palette.text, bg));
    grid.put(11, 0, &model.host, pair(palette.dim, bg));
    let mut x = 11 + text::display_width(&model.host) + 1;
    try_advance(grid, "·", pair(palette.faint, bg), &mut x, 0, width);
    x += 1;
    let uptime = crate::sys::host::uptime_human(model.snap.uptime);
    let text = format!("运行 {uptime}");
    try_advance(grid, &text, pair(palette.dim, bg), &mut x, 0, width);
    let right = if model.paused {
        format!("{} {}  已暂停", model.snap.date, model.snap.time)
    } else {
        format!("{} {}", model.snap.date, model.snap.time)
    };
    let style = if model.paused {
        pair_bold(palette.warn, bg)
    } else {
        pair(palette.dim, bg)
    };
    let needed = text::display_width(&right);
    if x + needed + 2 <= width {
        put_right(grid, width - 1, 0, &right, style);
    }

    for column in 0..width {
        let color = palette.mix(
            palette.accent,
            palette.border,
            column as f32 / (width.max(1) as f32 * 0.55),
        );
        grid.put(column, 1, "─", pair(color, bg));
    }
}

fn footer(grid: &mut Grid, model: &Model, width: usize, height: usize, palette: &Palette) {
    if height < 3 {
        return;
    }
    let bg = palette.screen;
    grid.put(0, height - 2, &"─".repeat(width), pair(palette.border, bg));
    let keys = if width >= 116 {
        "q 退出   p 暂停   +/- 间隔   c 核心   v 视图   t 主题   r 清空"
    } else {
        "q 退出   p 暂停   v 视图   t 主题   r 清空"
    };
    grid.put(1, height - 1, keys, pair(palette.faint, bg));
    let mode = if palette.truecolor {
        "24-bit"
    } else {
        "256 色"
    };
    let right = format!("{mode} · {:.1}s", model.interval);
    put_right(grid, width - 1, height - 1, &right, pair(palette.faint, bg));
}

fn cpu_card(grid: &mut Grid, rect: Rect, model: &Model, palette: &Palette) {
    let Some(inner) = panel(grid, rect, palette, "CPU", palette.cpu, "1.0s") else {
        return;
    };
    let cpu = &model.snap.cpu;
    let mut y = inner.y;

    let big = format!("{:.1}%", cpu.total);
    grid.put(inner.x, y, &big, pair_bold(palette.cpu, palette.card));
    let mut x = inner.x + text::display_width(&big) + 1;
    try_advance(
        grid,
        "总占用",
        pair(palette.dim, palette.card),
        &mut x,
        y,
        inner.x + inner.w,
    );
    if inner.w >= 46 {
        let brand = cpu.brand.replace("(R)", "").replace("(TM)", "");
        put_right(
            grid,
            inner.x + inner.w - 1,
            y,
            &format::truncate(&brand, inner.w.saturating_sub(24)),
            pair(palette.faint, palette.card),
        );
    }
    y += 1;

    let chart_height = if inner.h >= 9 {
        4
    } else if inner.h >= 8 {
        3
    } else {
        2
    };
    let chart_width = inner.w.saturating_sub(4);
    if chart_width >= 8 && y + chart_height <= inner.y + inner.h {
        area_chart(
            grid,
            inner.x,
            y,
            chart_width,
            chart_height,
            window(model.history.cpu.slice(), model.interval),
            100.0,
            palette.mix(palette.cpu, palette.card, 0.55),
            palette.cpu,
            palette,
        );
        put_right(
            grid,
            inner.x + inner.w - 1,
            y,
            "60s",
            pair(palette.faint, palette.card),
        );
        y += chart_height;
    }

    if y < inner.y + inner.h && model.show_cores && !cpu.per_core.is_empty() {
        grid.put(inner.x, y, "核心", pair(palette.dim, palette.card));
        let available = inner.w.saturating_sub(5);
        let width_per_core = if available >= cpu.per_core.len() * 2 {
            2
        } else {
            1
        };
        let count = cpu.per_core.len().min(available / width_per_core.max(1));
        for (index, usage) in cpu.per_core.iter().take(count).enumerate() {
            let color = palette.heat(*usage);
            for offset in 0..width_per_core {
                grid.set(
                    inner.x + 5 + index * width_per_core + offset,
                    y,
                    ' ',
                    pair(color, color),
                );
            }
        }
        let end = inner.x + 5 + count * width_per_core;
        if end + 6 <= inner.x + inner.w {
            grid.put(end + 1, y, "低", pair(palette.faint, palette.card));
            grid.put(end + 4, y, "高", pair(palette.faint, palette.card));
        }
        y += 1;
    }

    if y < inner.y + inner.h {
        let physical = cpu.physical.unwrap_or(cpu.logical);
        let peak = cpu.per_core.iter().copied().fold(0.0, f32::max);
        let values = model.history.cpu.slice();
        let average = if values.is_empty() {
            0.0
        } else {
            values.iter().copied().sum::<f32>() / values.len() as f32
        };
        let hottest = model
            .snap
            .zones
            .iter()
            .map(|zone| zone.celsius)
            .fold(0.0f32, f32::max);
        let mut stats = format!(
            "{physical}C/{}T · {:.2} GHz · 峰值 {peak:.0}% · 平均 {average:.1}%",
            cpu.logical,
            cpu.mhz as f32 / 1000.0
        );
        if hottest > 0.0 {
            stats.push_str(&format!(" · {hottest:.0}°C"));
        }
        grid.put(
            inner.x,
            y,
            &format::truncate(&stats, inner.w),
            pair(palette.dim, palette.card),
        );
    }
}

fn memory_card(grid: &mut Grid, rect: Rect, model: &Model, palette: &Palette) {
    let Some(inner) = panel(grid, rect, palette, "内存", palette.memory, "RAM") else {
        return;
    };
    let memory = &model.snap.memory;
    let percent = memory.percent();
    let mut y = inner.y;

    grid.put(
        inner.x,
        y,
        &format!("{percent:.1}%"),
        pair_bold(palette.memory, palette.card),
    );
    if inner.w >= 28 {
        put_right(
            grid,
            inner.x + inner.w - 1,
            y,
            &format!(
                "{} / {}",
                format::bytes(memory.used()),
                format::bytes(memory.total)
            ),
            pair_bold(palette.text, palette.card),
        );
    }
    y += 1;

    gauge(
        grid,
        inner.x,
        y,
        inner.w,
        percent,
        palette.mix(palette.memory, palette.accent, 0.25),
        palette.memory,
        palette,
    );
    y += 1;

    if y < inner.y + inner.h {
        let mut x = inner.x;
        advance(grid, "■", pair(palette.memory, palette.card), &mut x, y);
        let used = format!(" 已用 {}", format::bytes(memory.used()));
        try_advance(
            grid,
            &used,
            pair(palette.dim, palette.card),
            &mut x,
            y,
            inner.x + inner.w,
        );
        if x + 10 < inner.x + inner.w {
            x += 1;
            advance(grid, "■", pair(palette.net, palette.card), &mut x, y);
            let cache = format!(" 缓存 {}", format::bytes(memory.cache));
            try_advance(
                grid,
                &cache,
                pair(palette.dim, palette.card),
                &mut x,
                y,
                inner.x + inner.w,
            );
        }
        y += 1;
    }

    if y < inner.y + inner.h {
        let mut x = inner.x;
        advance(grid, "■", pair(palette.faint, palette.card), &mut x, y);
        let available = format!(" 可用 {}", format::bytes(memory.available));
        try_advance(
            grid,
            &available,
            pair(palette.dim, palette.card),
            &mut x,
            y,
            inner.x + inner.w,
        );
        if x + 16 < inner.x + inner.w {
            x += 1;
            let commit = format!(
                "提交 {} / {}",
                format::bytes(memory.commit),
                format::bytes(memory.commit_limit)
            );
            try_advance(
                grid,
                &commit,
                pair(palette.dim, palette.card),
                &mut x,
                y,
                inner.x + inner.w,
            );
        }
    }

    if inner.h >= 8 {
        y = inner.y + inner.h - 3;
        let ratio = if memory.commit_limit == 0 {
            0.0
        } else {
            memory.commit as f32 / memory.commit_limit as f32 * 100.0
        };
        let mut x = inner.x;
        advance(grid, "提交", pair(palette.dim, palette.card), &mut x, y);
        put_right(
            grid,
            inner.x + inner.w - 1,
            y,
            &format!("{ratio:.0}%"),
            pair(palette.faint, palette.card),
        );
        y += 1;
        gauge(
            grid,
            inner.x,
            y,
            inner.w,
            ratio,
            palette.mix(palette.accent, palette.net, 0.35),
            palette.accent,
            palette,
        );
    }
}

fn disk_card(grid: &mut Grid, rect: Rect, model: &Model, palette: &Palette) {
    let title = if model.snap.volumes.is_empty() {
        "—".to_string()
    } else {
        format!("{} 卷", model.snap.volumes.len())
    };
    let Some(inner) = panel(grid, rect, palette, "磁盘", palette.disk, &title) else {
        return;
    };
    let mut y = inner.y;
    let label_width = if inner.w >= 58 { 8 } else { 4 };
    for volume in model.snap.volumes.iter().take(2) {
        if y >= inner.y + inner.h {
            break;
        }
        let percent = volume.percent();
        let label = format::truncate(&volume.label, label_width);
        let label_pad = label_width.saturating_sub(text::display_width(&label));
        let sizes = format!(
            "{} / {}",
            format::bytes(volume.used()),
            format::bytes(volume.total)
        );
        let pct = format!("{:.1}%", percent);
        let fixed =
            3 + label_width + 1 + text::display_width(&pct) + 1 + text::display_width(&sizes);
        let gauge_width = inner.w.saturating_sub(fixed + 1).clamp(5, 22);
        let mut x = inner.x;
        advance(
            grid,
            &format!("{}: ", volume.letter),
            pair_bold(palette.text, palette.card),
            &mut x,
            y,
        );
        advance(grid, &label, pair(palette.dim, palette.card), &mut x, y);
        grid.put(
            x,
            y,
            &" ".repeat(label_pad),
            pair(palette.dim, palette.card),
        );
        x += label_pad;
        x += 1;
        gauge(
            grid,
            x,
            y,
            gauge_width,
            percent,
            palette.mix(palette.disk, palette.bad, 0.35),
            palette.disk,
            palette,
        );
        x += gauge_width + 1;
        advance(
            grid,
            &pct,
            pair(palette.load(percent), palette.card),
            &mut x,
            y,
        );
        x += 1;
        try_advance(
            grid,
            &sizes,
            pair(palette.dim, palette.card),
            &mut x,
            y,
            inner.x + inner.w,
        );
        y += 2;
    }

    if y < inner.y + inner.h {
        let mut x = inner.x;
        advance(grid, "IO", pair(palette.dim, palette.card), &mut x, y);
        advance(grid, " 读 ", pair(palette.faint, palette.card), &mut x, y);
        advance(
            grid,
            &format::rate(model.snap.io_read),
            pair(palette.text, palette.card),
            &mut x,
            y,
        );
        advance(grid, "  写 ", pair(palette.faint, palette.card), &mut x, y);
        advance(
            grid,
            &format::rate(model.snap.io_write),
            pair(palette.text, palette.card),
            &mut x,
            y,
        );
        let remaining = (inner.x + inner.w).saturating_sub(x + 2);
        if remaining >= 6 {
            let scale = model.history.io.max(64.0 * 1024.0);
            spark(
                grid,
                x + 2,
                y,
                remaining.min(30),
                window(model.history.io.slice(), model.interval),
                scale,
                palette.disk,
                palette,
            );
        }
        y += 1;
    }

    if y < inner.y + inner.h && inner.w >= 24 {
        let filesystem = model
            .snap
            .volumes
            .first()
            .map(|volume| volume.filesystem.as_str())
            .filter(|value| !value.is_empty())
            .unwrap_or("—");
        let note = format!("{filesystem} · {} 个卷 · 5s 刷新", model.snap.volumes.len());
        grid.put(
            inner.x,
            y,
            &format::truncate(&note, inner.w),
            pair(palette.faint, palette.card),
        );
    }
}

fn active_net(model: &Model) -> Option<&Net> {
    model
        .snap
        .nets
        .iter()
        .find(|net| net.up || net.rx_bps + net.tx_bps > 1.0)
        .or_else(|| model.snap.nets.first())
}

fn network_card(grid: &mut Grid, rect: Rect, model: &Model, palette: &Palette) {
    let Some(inner) = panel(grid, rect, palette, "网络", palette.net, "Wi-Fi") else {
        return;
    };
    let Some(net) = active_net(model) else {
        grid.put(
            inner.x,
            inner.y,
            "无活动连接",
            pair(palette.faint, palette.card),
        );
        return;
    };
    let mut y = inner.y;
    grid.put(
        inner.x,
        y,
        &format::truncate(&super::pretty_adapter(&net.name), inner.w),
        pair(palette.text, palette.card),
    );
    y += 1;
    if y < inner.y + inner.h {
        let mut detail = net.ipv4.clone();
        if net.link_bits > 0 && net.link_bits < 10_000_000_000 {
            if !detail.is_empty() {
                detail.push_str(" · ");
            }
            detail.push_str(&format!("{} Mbps", net.link_bits / 1_000_000));
        }
        grid.put(
            inner.x,
            y,
            &format::truncate(&detail, inner.w),
            pair(palette.faint, palette.card),
        );
        y += 1;
    }

    let scale = model.history.net.max(32.0 * 1024.0);
    for (arrow, value, history, color) in [
        (
            "↓",
            net.rx_bps,
            window(model.history.net_rx.slice(), model.interval),
            palette.net,
        ),
        (
            "↑",
            net.tx_bps,
            window(model.history.net_tx.slice(), model.interval),
            palette.temp,
        ),
    ] {
        if y >= inner.y + inner.h {
            break;
        }
        let mut x = inner.x;
        advance(grid, arrow, pair_bold(color, palette.card), &mut x, y);
        advance(grid, " ", pair(palette.text, palette.card), &mut x, y);
        advance(
            grid,
            &format::truncate(&format::rate(value), 10),
            pair_bold(color, palette.card),
            &mut x,
            y,
        );
        let spark_x = inner.x + 13;
        if inner.x + inner.w > spark_x + 4 {
            spark(
                grid,
                spark_x,
                y,
                (inner.x + inner.w).saturating_sub(spark_x),
                history,
                scale,
                color,
                palette,
            );
        }
        y += 1;
    }
    if y < inner.y + inner.h && inner.w >= 24 {
        let totals = format!(
            "累计 ↓ {} ↑ {}",
            format::bytes(net.rx_total),
            format::bytes(net.tx_total)
        );
        grid.put(
            inner.x,
            y,
            &format::truncate(&totals, inner.w),
            pair(palette.dim, palette.card),
        );
    }
}

fn battery_state_text(state: crate::sys::power::BatteryState) -> Option<&'static str> {
    use crate::sys::power::BatteryState::*;
    match state {
        Charging => Some("充电中"),
        Discharging => Some("放电中"),
        Idle => Some("交流供电"),
        NoBattery | Unknown => None,
    }
}

fn thermal_card(grid: &mut Grid, rect: Rect, model: &Model, palette: &Palette) {
    let right = if model.snap.counters_enabled {
        "ACPI"
    } else {
        "OFF"
    };
    let Some(inner) = panel(grid, rect, palette, "温度与电源", palette.temp, right) else {
        return;
    };
    let mut y = inner.y;
    let hottest = model
        .snap
        .zones
        .iter()
        .max_by(|a, b| a.celsius.total_cmp(&b.celsius));

    let mut x = inner.x;
    if let Some(zone) = hottest {
        advance(
            grid,
            &format!("{:.0}°C", zone.celsius),
            pair_bold(palette.temperature(zone.celsius), palette.card),
            &mut x,
            y,
        );
        try_advance(
            grid,
            &format!("  {}", zone.name),
            pair(palette.dim, palette.card),
            &mut x,
            y,
            inner.x + inner.w,
        );
        let status = if zone.celsius >= 90.0 {
            "偏热"
        } else if zone.celsius >= 75.0 {
            "温热"
        } else {
            "正常"
        };
        try_advance(
            grid,
            &format!("  {status}"),
            pair(palette.faint, palette.card),
            &mut x,
            y,
            inner.x + inner.w,
        );
    } else {
        advance(
            grid,
            if model.snap.counters_enabled {
                "温度不可用"
            } else {
                "温度已关闭"
            },
            pair(palette.faint, palette.card),
            &mut x,
            y,
        );
    }
    if let Some(fan) = model.snap.fans.first() {
        let text = format!("  风扇 {} RPM", fan.rpm);
        try_advance(
            grid,
            &text,
            pair(palette.dim, palette.card),
            &mut x,
            y,
            inner.x + inner.w,
        );
    } else if x + 14 <= inner.x + inner.w {
        try_advance(
            grid,
            "  风扇 —",
            pair(palette.faint, palette.card),
            &mut x,
            y,
            inner.x + inner.w,
        );
    }
    y += 1;

    if y < inner.y + inner.h {
        let temperature = hottest.map(|zone| zone.celsius).unwrap_or(0.0);
        for column in 0..inner.w {
            let value = 20.0 + column as f32 / inner.w.max(1) as f32 * 85.0;
            let color = if temperature > 0.0 && value <= temperature {
                palette.grade(value)
            } else {
                palette.track
            };
            grid.set(inner.x + column, y, ' ', pair(color, color));
        }
        y += 1;
    }

    if y < inner.y + inner.h {
        let mut x = inner.x;
        if let Some(power) = model.snap.power {
            if power.available() {
                let text = match power.battery_percent {
                    Some(percent) => format!("电池 {percent}%"),
                    None => "电池 —".to_string(),
                };
                advance(
                    grid,
                    &text,
                    pair_bold(palette.battery, palette.card),
                    &mut x,
                    y,
                );
                if let Some(state) = battery_state_text(power.state) {
                    try_advance(
                        grid,
                        &format!(" {state}"),
                        pair(palette.dim, palette.card),
                        &mut x,
                        y,
                        inner.x + inner.w,
                    );
                }
                let remaining = (inner.x + inner.w).saturating_sub(x + 2);
                if remaining >= 8 {
                    let percent = power.battery_percent.unwrap_or(0) as f32;
                    gauge(
                        grid,
                        x + 2,
                        y,
                        remaining.min(20),
                        percent,
                        palette.mix(palette.battery, palette.ok, 0.4),
                        palette.battery,
                        palette,
                    );
                }
            } else {
                advance(
                    grid,
                    "未检测到电池",
                    pair(palette.faint, palette.card),
                    &mut x,
                    y,
                );
            }
        } else {
            advance(
                grid,
                "电源状态不可用",
                pair(palette.faint, palette.card),
                &mut x,
                y,
            );
        }
        y += 1;
    }

    if y < inner.y + inner.h {
        grid.put(
            inner.x,
            y,
            &format::truncate("ACPI 热区 · 5s 刷新", inner.w),
            pair(palette.faint, palette.card),
        );
    }
}

fn self_card(grid: &mut Grid, rect: Rect, model: &Model, palette: &Palette) {
    let Some(inner) = panel(grid, rect, palette, "自身", palette.accent, "TUI") else {
        return;
    };
    let snapshot = &model.snap;
    let mut y = inner.y;
    let mut x = inner.x;
    advance(
        grid,
        &format::bytes(snapshot.self_private),
        pair_bold(palette.text, palette.card),
        &mut x,
        y,
    );
    try_advance(
        grid,
        " 私有内存",
        pair(palette.dim, palette.card),
        &mut x,
        y,
        inner.x + inner.w,
    );
    y += 1;

    let mut x = inner.x;
    advance(
        grid,
        &format!("{:.2}%", snapshot.self_cpu),
        pair_bold(palette.text, palette.card),
        &mut x,
        y,
    );
    try_advance(
        grid,
        " CPU",
        pair(palette.dim, palette.card),
        &mut x,
        y,
        inner.x + inner.w,
    );
    try_advance(
        grid,
        &format!("  采样 {}", format::micros(snapshot.tick_us)),
        pair(palette.dim, palette.card),
        &mut x,
        y,
        inner.x + inner.w,
    );
    y += 1;

    if y < inner.y + inner.h {
        let mut x = inner.x;
        advance(
            grid,
            &format!("{:.1}s 刷新", model.interval),
            pair(palette.dim, palette.card),
            &mut x,
            y,
        );
        try_advance(
            grid,
            "  历史环 900 点",
            pair(palette.faint, palette.card),
            &mut x,
            y,
            inner.x + inner.w,
        );
        y += 1;
    }
    if y < inner.y + inner.h {
        grid.put(
            inner.x,
            y,
            &format::truncate("单帧写入 · 零依赖", inner.w),
            pair(palette.faint, palette.card),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::Sampler;

    fn sample_model() -> Model {
        let mut sampler = Sampler::new(1.0, None, true);
        sampler.sample();
        sampler.model
    }

    #[test]
    fn fits_needs_room_for_the_grid() {
        assert!(!fits(99, 40));
        assert!(!fits(120, 29));
        assert!(fits(100, 30));
        assert!(fits(160, 44));
    }

    #[test]
    fn every_line_fills_the_screen() {
        let model = sample_model();
        let palette = Palette::new("dark", true);
        for (width, height) in [
            (100, 30),
            (100, 31),
            (120, 31),
            (120, 32),
            (120, 33),
            (120, 34),
            (160, 44),
            (100, 40),
        ] {
            let lines = render(&model, width, height, &palette);
            assert_eq!(lines.len(), height, "height mismatch at {width}x{height}");
            for line in &lines {
                assert_eq!(line.width(), width, "width mismatch at {width}x{height}");
            }
        }
    }

    #[test]
    fn light_palette_also_renders() {
        let model = sample_model();
        let lines = render(&model, 120, 34, &Palette::new("light", true));
        assert_eq!(lines.len(), 34);
        for line in &lines {
            assert_eq!(line.width(), 120);
        }
    }

    #[test]
    fn all_sections_survive_the_smallest_layout() {
        let model = sample_model();
        let palette = Palette::new("dark", false);
        let lines = render(&model, 100, 30, &palette);
        let text = super::super::plain(&lines, 100);
        for needle in ["CPU", "内存", "磁盘", "网络", "温度", "自身"] {
            assert!(text.contains(needle), "missing {needle}");
        }
    }

    #[test]
    fn truecolor_frames_emit_24_bit_sgr() {
        let model = sample_model();
        let lines = render(&model, 120, 34, &Palette::new("dark", true));
        let frame = super::super::serialize(&lines, 120, 34);
        assert!(frame.contains(";38;2;"));
        assert!(frame.contains(";48;2;"));
    }

    #[test]
    fn indexed_frames_stay_on_the_256_colour_palette() {
        let model = sample_model();
        let lines = render(&model, 120, 34, &Palette::new("dark", false));
        let frame = super::super::serialize(&lines, 120, 34);
        assert!(!frame.contains(";2;"));
        assert!(frame.contains(";48;5;"));
        assert!(frame.contains(";38;5;"));
    }
}

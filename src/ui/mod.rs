//! Screen layout. Renders a [`Model`] into width-exact, styled lines.
//!
//! Every section is built as `(priority, Line)` blocks: priority 0 is essential,
//! 1 is useful detail, 2 is extra. When the window is short the extra blocks are
//! dropped in that order, so the layout degrades instead of scrolling.

pub mod fancy;
pub mod style;
pub mod text;

use crate::format;
use crate::metrics::{Fan, Model, Net};
use style::{Palette, Style};
use text::Line;

/// Width of the leading `" CPU "` label column, in terminal cells.
const LABEL: usize = 6;

pub fn render(model: &Model, width: usize, height: usize, palette: &Palette) -> Vec<Line> {
    let width = width.max(24);
    let compact = width < 76;
    let mut blocks: Vec<(u8, Line)> = Vec::with_capacity(height + 6);

    header(&mut blocks, model, width, palette);
    cpu_section(&mut blocks, model, width, palette, compact);
    memory_section(&mut blocks, model, width, palette, compact);
    disk_section(&mut blocks, model, width, palette, compact);
    net_section(&mut blocks, model, width, palette, compact);
    thermal_section(&mut blocks, model, width, palette, compact);
    gpu_section(&mut blocks, model, width, palette, compact);
    footer(&mut blocks, model, width, palette, compact);

    fit(blocks, width, height)
}

fn header(blocks: &mut Vec<(u8, Line)>, model: &Model, width: usize, palette: &Palette) {
    let snapshot = &model.snap;
    let mut line = Line::new();
    line.push(" monitor", Style::bold(palette.accent));
    line.push("  ", Style::PLAIN);
    line.push(&snapshot_name(model, width), Style::fg(palette.dim));

    let mut right = format!("{} {}", snapshot.date, snapshot.time);
    if model.paused {
        right.push_str("  已暂停");
    }
    let right_style = if model.paused {
        Style::bold(palette.warn)
    } else {
        Style::fg(palette.dim)
    };
    // Only place the clock when it fits with a separator; otherwise it would be
    // glued to the host name and clipped mid-digit.
    if line.width() + text::display_width(&right) + 1 <= width {
        push_right(&mut line, &right, right_style, width);
    }
    blocks.push((0, line));

    let mut rule = Line::new();
    rule.push(&"─".repeat(width), Style::fg(palette.faint));
    blocks.push((0, rule));
}

fn snapshot_name(model: &Model, width: usize) -> String {
    let mut name = model.host.clone();
    // Uptime is the first thing to go: at narrow widths the host and clock
    // already fill the line.
    if model.snap.uptime > 0 && width >= 64 {
        name.push_str("  运行 ");
        name.push_str(&crate::sys::host::uptime_human(model.snap.uptime));
    }
    name
}

fn cpu_section(
    blocks: &mut Vec<(u8, Line)>,
    model: &Model,
    width: usize,
    palette: &Palette,
    compact: bool,
) {
    let cpu = &model.snap.cpu;

    let mut line = Line::new();
    label(&mut line, "CPU", Style::bold(palette.cpu));
    line.push(
        &clip_words(&cpu.brand, if compact { 24 } else { 34 }),
        Style::fg(palette.text),
    );
    line.push("  ", Style::PLAIN);
    let topology = match cpu.physical {
        Some(cores) => format!("{cores}C/{}T", cpu.logical),
        None => format!("{}T", cpu.logical),
    };
    line.push(&topology, Style::fg(palette.dim));
    if cpu.mhz > 0 {
        line.push("  ", Style::PLAIN);
        line.push(&format!("{:.2} GHz", cpu.mhz as f32 / 1000.0), Style::fg(palette.text));
    }
    let usage = format!("{:>5.1}%", cpu.total);
    push_right(&mut line, &usage, Style::bold(palette.load(cpu.total)), width);
    blocks.push((0, line));

    if compact {
        return;
    }

    // Gauge stretches to fill the line, with the load history pinned right.
    let spark_width = if width >= 100 { 36 } else { 0 };
    let gauge_width = width
        .saturating_sub(LABEL + if spark_width > 0 { spark_width + 2 } else { 0 })
        .clamp(10, 80);
    let (filled, rest) = text::gauge_parts(cpu.total, gauge_width);
    let mut line = Line::new();
    label(&mut line, "", Style::PLAIN);
    line.push(&filled, Style::fg(palette.load(cpu.total)));
    line.push(&rest, Style::fg(palette.faint));

    if spark_width > 0 {
        line.push("  ", Style::PLAIN);
        line.push(
            &text::sparkline(model.history.cpu.slice(), spark_width, 100.0),
            Style::fg(palette.cpu),
        );
    }
    blocks.push((1, line));

    // Per-core grid, compact enough to show every thread.
    if model.show_cores && !cpu.per_core.is_empty() && width >= 72 {
        let per_row = ((width - LABEL) / 8).max(4);
        for (row, chunk) in cpu.per_core.chunks(per_row).enumerate() {
            let mut line = Line::new();
            if row == 0 {
                label(&mut line, "", Style::PLAIN);
            } else {
                indent(&mut line, LABEL);
            }
            for (offset, usage) in chunk.iter().enumerate() {
                let index = row * per_row + offset;
                line.push(&format!("{index:>2}:"), Style::fg(palette.faint));
                line.push(
                    &format!("{usage:>3.0}%"),
                    Style::fg(palette.load(*usage)),
                );
                line.push(" ", Style::PLAIN);
            }
            blocks.push((2, line));
        }
    }
}

fn memory_section(
    blocks: &mut Vec<(u8, Line)>,
    model: &Model,
    width: usize,
    palette: &Palette,
    compact: bool,
) {
    let memory = &model.snap.memory;
    let mut line = Line::new();
    label(&mut line, "内存", Style::bold(palette.memory));
    let gauge_width = ((width.saturating_sub(LABEL)) / 3).clamp(8, 30);
    let (filled, rest) = text::gauge_parts(memory.percent(), gauge_width);
    line.push(&filled, Style::fg(palette.load(memory.percent())));
    line.push(&rest, Style::fg(palette.faint));
    line.push("  ", Style::PLAIN);
    line.push(&format!("{:>5.1}%", memory.percent()), Style::fg(palette.text));
    line.push("  ", Style::PLAIN);
    line.push(
        &format!("{} / {}", format::bytes(memory.used()), format::bytes(memory.total)),
        Style::fg(palette.dim),
    );

    if !compact {
        push_pair_fit(
            &mut line,
            "   缓存 ",
            Style::fg(palette.faint),
            &format::bytes(memory.cache),
            Style::fg(palette.dim),
            width,
        );
        push_pair_fit(
            &mut line,
            "   提交 ",
            Style::fg(palette.faint),
            &format!(
                "{} / {}",
                format::bytes(memory.commit),
                format::bytes(memory.commit_limit)
            ),
            Style::fg(palette.dim),
            width,
        );
    }
    blocks.push((0, line));
}

fn disk_section(
    blocks: &mut Vec<(u8, Line)>,
    model: &Model,
    width: usize,
    palette: &Palette,
    compact: bool,
) {
    let snapshot = &model.snap;
    let gauge_width = (width.saturating_sub(60)).clamp(8, 22);

    for (index, volume) in snapshot.volumes.iter().take(4).enumerate() {
        let mut line = Line::new();
        if index == 0 {
            label(&mut line, "磁盘", Style::bold(palette.disk));
        } else {
            indent(&mut line, LABEL);
            line.spaces(2, Style::PLAIN);
        }
        line.push(&format!("{}:", volume.letter), Style::fg(palette.text));
        line.push(" ", Style::PLAIN);
        if !compact {
            let (filled, rest) = text::gauge_parts(volume.percent(), gauge_width);
            line.push(&filled, Style::fg(palette.load(volume.percent())));
            line.push(&rest, Style::fg(palette.faint));
            line.push("  ", Style::PLAIN);
        }
        line.push(
            &format!("{:>5.1}%", volume.percent()),
            Style::fg(palette.load(volume.percent())),
        );
        line.push("  ", Style::PLAIN);
        line.push(
            &format!(
                "{} / {}",
                format::bytes(volume.used()),
                format::bytes(volume.total)
            ),
            Style::fg(palette.dim),
        );
        if !compact && !volume.label.is_empty() {
            if push_pair_fit(
                &mut line,
                "   ",
                Style::PLAIN,
                &format::truncate(&volume.label, 10),
                Style::fg(palette.faint),
                width,
            ) {
                if !volume.filesystem.is_empty() {
                    push_fit(
                        &mut line,
                        &format!(" ({})", volume.filesystem),
                        Style::fg(palette.faint),
                        width,
                    );
                }
            }
        }
        blocks.push((if index == 0 { 0 } else { 1 }, line));
    }

    if compact || !snapshot.counters_enabled {
        return;
    }
    let mut line = Line::new();
    indent(&mut line, LABEL);
    line.push("IO ", Style::fg(palette.faint));
    line.push("读 ", Style::fg(palette.faint));
    line.push(&format::rate(snapshot.io_read), Style::fg(palette.text));
    line.push("   写 ", Style::fg(palette.faint));
    line.push(&format::rate(snapshot.io_write), Style::fg(palette.text));

    let spark_width = if width >= 96 { 20 } else { 0 };
    if spark_width > 0 {
        line.push("  ", Style::PLAIN);
        let scale = model.history.io.max(64.0 * 1024.0);
        line.push(
            &text::sparkline(model.history.io.slice(), spark_width, scale),
            Style::fg(palette.disk),
        );
    }
    blocks.push((1, line));
}

fn net_section(
    blocks: &mut Vec<(u8, Line)>,
    model: &Model,
    width: usize,
    palette: &Palette,
    compact: bool,
) {
    let nets: Vec<&Net> = model
        .snap
        .nets
        .iter()
        .filter(|net| net.up || net.rx_bps + net.tx_bps > 1.0)
        .take(if compact { 1 } else { 2 })
        .collect();

    if nets.is_empty() {
        let mut line = Line::new();
        label(&mut line, "网络", Style::bold(palette.net));
        line.push("无活动连接", Style::fg(palette.faint));
        blocks.push((1, line));
        return;
    }

    for (index, net) in nets.iter().enumerate() {
        let mut line = Line::new();
        if index == 0 {
            label(&mut line, "网络", Style::bold(palette.net));
        } else {
            indent(&mut line, LABEL);
        }
        let name = pretty_adapter(&net.name);
        let name_width = if compact { 16 } else { 30 }.min(width / 3);
        line.push(&clip_words(&name, name_width), Style::fg(palette.text));

        if !net.up {
            line.push(" 未连接", Style::fg(palette.faint));
        }
        line.push("  ↓ ", Style::fg(palette.faint));
        line.push(
            &format::rate(net.rx_bps),
            Style::fg(if net.rx_bps > 1.0 { palette.ok } else { palette.dim }),
        );
        line.push("  ↑ ", Style::fg(palette.faint));
        line.push(
            &format::rate(net.tx_bps),
            Style::fg(if net.tx_bps > 1.0 { palette.ok } else { palette.dim }),
        );

        if !compact && index == 0 {
            push_pair_fit(
                &mut line,
                "   累计 ",
                Style::fg(palette.faint),
                &format!(
                    "↓{} ↑{}",
                    format::bytes(net.rx_total),
                    format::bytes(net.tx_total)
                ),
                Style::fg(palette.dim),
                width,
            );
        }

        let spark_width = if !compact && index == 0 && line.width() + 21 <= width {
            18
        } else {
            0
        };
        if spark_width > 0 {
            line.push("  ", Style::PLAIN);
            let scale = model.history.net.max(32.0 * 1024.0);
            line.push(
                &text::sparkline(model.history.net.slice(), spark_width, scale),
                Style::fg(palette.net),
            );
        }
        blocks.push((if index == 0 { 0 } else { 1 }, line));
    }
}

fn thermal_section(
    blocks: &mut Vec<(u8, Line)>,
    model: &Model,
    width: usize,
    palette: &Palette,
    compact: bool,
) {
    let snapshot = &model.snap;
    let mut line = Line::new();
    label(&mut line, "散热", Style::bold(palette.temp));

    if snapshot.zones.is_empty() {
        let note = if snapshot.counters_enabled {
            "温度不可用"
        } else {
            "温度已关闭"
        };
        line.push(note, Style::fg(palette.faint));
    } else {
        for (index, zone) in snapshot.zones.iter().take(if compact { 1 } else { 2 }).enumerate() {
            if index > 0 {
                line.push("  ", Style::PLAIN);
            }
            line.push(
                &format!("{:.0}°C", zone.celsius),
                Style::bold(palette.temperature(zone.celsius)),
            );
            if !compact {
                line.push(" ", Style::PLAIN);
                line.push(&zone.name, Style::fg(palette.faint));
            }
        }
    }

    line.push("   风扇 ", Style::fg(palette.faint));
    if snapshot.fans.is_empty() {
        line.push("—", Style::fg(palette.faint));
        if !compact && width >= 96 {
            line.push(" 固件未暴露", Style::fg(palette.faint));
        }
    } else {
        push_fans(&mut line, &snapshot.fans, palette);
    }

    if let Some(power) = snapshot.power {
        if power.available() {
            let percent_text = match power.battery_percent {
                Some(percent) => format!("{percent}%"),
                None => "—".to_string(),
            };
            if push_pair_fit(
                &mut line,
                "   电池 ",
                Style::fg(palette.faint),
                &percent_text,
                Style::fg(match power.battery_percent {
                    Some(percent) if percent <= 15 => palette.bad,
                    Some(percent) if percent <= 30 => palette.warn,
                    _ => palette.battery,
                }),
                width,
            ) {
                if let Some(state) = battery_state_text(power.state) {
                    push_fit(&mut line, &format!(" {state}"), Style::fg(palette.dim), width);
                }
            }
        }
    }

    blocks.push((0, line));
}

/// One compact line for the first GPU: name, utilisation and memory split.
/// Hidden entirely when the machine has no GPU performance counters.
fn gpu_section(
    blocks: &mut Vec<(u8, Line)>,
    model: &Model,
    width: usize,
    palette: &Palette,
    compact: bool,
) {
    let Some(gpu) = model.snap.gpus.first() else {
        return;
    };
    let mut line = Line::new();
    label(&mut line, "GPU", Style::bold(palette.accent));
    let name_width = if compact { 18 } else { 34 };
    line.push(&clip_words(&gpu.name, name_width), Style::fg(palette.text));
    line.push("  ", Style::PLAIN);
    line.push(
        &format!("{:>5.1}%", gpu.utilization),
        Style::bold(palette.load(gpu.utilization)),
    );
    if !compact {
        line.push("  专用 ", Style::fg(palette.faint));
        if gpu.dedicated_limit > 0 {
            line.push(
                &format!(
                    "{} / {}",
                    format::bytes(gpu.dedicated_used),
                    format::bytes(gpu.dedicated_limit)
                ),
                Style::fg(palette.dim),
            );
        } else {
            line.push(&format::bytes(gpu.dedicated_used), Style::fg(palette.dim));
        }
        line.push("  共享 ", Style::fg(palette.faint));
        line.push(&format::bytes(gpu.shared_used), Style::fg(palette.dim));
    }
    if let Some(engine) = gpu.engines.first() {
        let text = format!("  {} {:.0}%", engine.name, engine.utilization);
        push_fit(&mut line, &text, Style::fg(palette.faint), width);
    }
    if model.snap.gpus.len() > 1 {
        push_fit(
            &mut line,
            &format!("  +{}", model.snap.gpus.len() - 1),
            Style::fg(palette.faint),
            width,
        );
    }
    blocks.push((1, line));
}

fn push_fans(line: &mut Line, fans: &[Fan], palette: &Palette) {
    for (index, fan) in fans.iter().take(2).enumerate() {
        if index > 0 {
            line.push("  ", Style::PLAIN);
        }
        line.push(&format!("{} RPM", fan.rpm), Style::fg(palette.fan));
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

fn footer(
    blocks: &mut Vec<(u8, Line)>,
    model: &Model,
    width: usize,
    palette: &Palette,
    compact: bool,
) {
    let snapshot = &model.snap;
    let mut line = Line::new();
    label(&mut line, "自身", Style::bold(palette.accent));
    line.push(&format::bytes(snapshot.self_private), Style::fg(palette.text));
    line.push("  ", Style::PLAIN);
    line.push(&format!("{:.2}% CPU", snapshot.self_cpu), Style::fg(palette.dim));
    if !compact {
        line.push("  采样 ", Style::fg(palette.faint));
        line.push(&format::micros(snapshot.tick_us), Style::fg(palette.dim));
        line.push("  ", Style::PLAIN);
        line.push(&format!("{:.1}s 刷新", model.interval), Style::fg(palette.dim));
    }

    if !compact && width >= 100 {
        let keys = "q 退出  p 暂停  +/- 间隔  c 核心  t 主题";
        push_right(&mut line, keys, Style::fg(palette.faint), width);
    }
    blocks.push((0, line));
}

/// Keep the highest-priority blocks that fit, in their original order.
fn fit(blocks: Vec<(u8, Line)>, width: usize, height: usize) -> Vec<Line> {
    let height = height.max(1);
    let mut keep = vec![false; blocks.len()];
    let mut used = 0usize;
    for priority in [0u8, 1, 2] {
        for (index, (level, _)) in blocks.iter().enumerate() {
            if *level != priority || keep[index] {
                continue;
            }
            if used >= height {
                break;
            }
            keep[index] = true;
            used += 1;
        }
    }

    blocks
        .into_iter()
        .zip(keep)
        .filter_map(|((_, mut line), keep)| {
            if !keep {
                return None;
            }
            line.truncate(width);
            line.pad_to(width, Style::PLAIN);
            Some(line)
        })
        .collect()
}

/// `label` occupies exactly [`LABEL`] columns so every section lines up.
fn label(line: &mut Line, text: &str, style: Style) {
    let width = text::display_width(text);
    line.spaces(1, Style::PLAIN);
    line.push(text, style);
    line.spaces(LABEL.saturating_sub(1 + width), Style::PLAIN);
}

fn indent(line: &mut Line, columns: usize) {
    line.spaces(columns, Style::PLAIN);
}

fn push_right(line: &mut Line, text: &str, style: Style, width: usize) {
    let text_width = text::display_width(text);
    let used = line.width() + text_width;
    if used < width {
        line.spaces(width - used, Style::PLAIN);
    }
    line.push(text, style);
}

/// Append only when it still fits inside `limit`, so optional detail is dropped
/// instead of being cut mid-word by the final truncation pass.
fn push_fit(line: &mut Line, text: &str, style: Style, limit: usize) -> bool {
    if line.width() + text::display_width(text) > limit {
        return false;
    }
    line.push(text, style);
    true
}

fn push_pair_fit(
    line: &mut Line,
    label: &str,
    label_style: Style,
    value: &str,
    value_style: Style,
    limit: usize,
) -> bool {
    let needed = text::display_width(label) + text::display_width(value);
    if line.width() + needed > limit {
        return false;
    }
    line.push(label, label_style);
    line.push(value, value_style);
    true
}

/// Truncate without leaving half a word behind when a boundary is near.
pub fn clip_words(text: &str, width: usize) -> String {
    let clipped = format::truncate(text, width);
    if clipped == text {
        return clipped;
    }
    if let Some(position) = clipped.rfind(' ') {
        // Only honour a word boundary that keeps most of the available space.
        if position * 10 >= width * 6 {
            return clipped[..position].to_string();
        }
    }
    clipped
}

/// "Intel(R) Wi-Fi 6E AX211 160MHz" -> "Intel Wi-Fi 6E AX211".
pub fn pretty_adapter(name: &str) -> String {
    let mut cleaned = crate::sys::net::shorten_adapter(name).to_string();
    for noise in ["(R)", "(TM)", "(r)", "(tm)"] {
        while let Some(position) = cleaned.find(noise) {
            cleaned.replace_range(position..position + noise.len(), "");
        }
    }
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.len() > 40 {
        cleaned[..40].to_string()
    } else {
        cleaned
    }
}

/// Serialise a frame: absolute cursor addressing per line, then clear what is
/// left below, so a shrinking layout never leaves stale rows behind.
pub fn serialize(lines: &[Line], width: usize, height: usize) -> String {
    use std::fmt::Write;

    let mut out = String::with_capacity(width * height * 2 + 96);
    for (row, line) in lines.iter().take(height).enumerate() {
        let _ = write!(out, "\x1b[{};1H", row + 1);
        line.render(width, &mut out);
    }
    out.push_str("\x1b[0J");
    out
}

/// Plain-text frame for `--demo` and logs: no escapes, exact widths.
pub fn plain(lines: &[Line], width: usize) -> String {
    let mut out = String::new();
    for line in lines {
        let mut text = String::new();
        for span in line.spans() {
            text.push_str(&span.text);
        }
        let padding = width.saturating_sub(text::display_width(&text));
        out.push_str(text.trim_end());
        out.push_str(&" ".repeat(padding));
        out.push('\n');
    }
    out
}


#[cfg(test)]
mod tests {
    use super::*;

    fn sample_model() -> Model {
        let mut sampler = crate::metrics::Sampler::new(1.0, None, true);
        sampler.sample();
        sampler.model
    }

    #[test]
    fn every_line_is_exactly_the_screen_width() {
        let model = sample_model();
        let palette = Palette::dark();
        for width in [40usize, 60, 80, 100, 120, 160] {
            let lines = render(&model, width, 40, &palette);
            assert!(!lines.is_empty(), "width={width}");
            for line in &lines {
                assert_eq!(line.width(), width, "width={width} line={:?}", line.spans());
            }
        }
    }

    #[test]
    fn short_windows_drop_details_instead_of_scrolling() {
        let model = sample_model();
        let palette = Palette::dark();
        for height in [4usize, 8, 12, 18, 30] {
            let lines = render(&model, 100, height, &palette);
            assert!(lines.len() <= height, "height={height} lines={}", lines.len());
        }
    }

    #[test]
    fn essential_sections_survive_narrow_terminals() {
        let model = sample_model();
        let palette = Palette::dark();
        let text = plain(&render(&model, 60, 40, &palette), 60);
        assert!(text.contains("CPU"));
        assert!(text.contains("内存"));
        assert!(text.contains("磁盘"));
        assert!(text.contains("网络"));
        assert!(text.contains("散热"));
    }

    #[test]
    fn adapter_names_are_cleaned() {
        assert_eq!(
            pretty_adapter("Intel(R) Wi-Fi 6E AX211 160MHz"),
            "Intel Wi-Fi 6E AX211 160MHz"
        );
        assert_eq!(pretty_adapter("Realtek -NDIS 6.70"), "Realtek");
    }

    #[test]
    fn serialize_positions_every_row() {
        let model = sample_model();
        let palette = Palette::dark();
        let lines = render(&model, 80, 30, &palette);
        let frame = serialize(&lines, 80, 30);
        assert!(frame.starts_with("\x1b[1;1H"));
        assert!(frame.ends_with("\x1b[0J"));
        assert!(frame.contains("\x1b[2;1H"));
    }

    /// Split a frame on its cursor-positioning sequences, keeping only the
    /// visible text: proves every escape sequence is well formed and that the
    /// styled frame shows exactly what `plain` shows.
    fn visible_rows(frame: &str) -> Vec<String> {
        let mut rows = Vec::new();
        let mut current = String::new();
        let mut started = false;
        let mut chars = frame.chars();
        while let Some(ch) = chars.next() {
            if ch != '\x1b' {
                current.push(ch);
                continue;
            }
            let mut sequence = String::new();
            for next in chars.by_ref() {
                sequence.push(next);
                if matches!(next, 'm' | 'H' | 'J') {
                    break;
                }
            }
            assert!(
                sequence.starts_with('['),
                "malformed escape sequence: {sequence:?}"
            );
            if sequence.ends_with('H') {
                // Rows are delimited by their cursor moves; the first move
                // precedes row one and carries no text of its own.
                if started {
                    rows.push(std::mem::take(&mut current));
                } else {
                    started = true;
                    current.clear();
                }
            }
        }
        if !current.is_empty() {
            rows.push(current);
        }
        rows
    }

    #[test]
    fn styled_frame_matches_plain_text() {
        let model = sample_model();
        let palette = Palette::dark();
        let lines = render(&model, 96, 30, &palette);
        let frame = serialize(&lines, 96, 30);
        let rows = visible_rows(&frame);
        assert_eq!(rows.len(), lines.len());

        let plain_text = plain(&lines, 96);
        let plain_rows: Vec<&str> = plain_text.lines().collect();
        for (row, plain_row) in rows.iter().zip(plain_rows.iter()) {
            assert_eq!(
                text::display_width(row),
                96,
                "row must fill the width exactly: {row:?}"
            );
            assert_eq!(row.trim_end(), plain_row.trim_end());
        }
    }
}

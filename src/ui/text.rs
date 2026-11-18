//! Width-aware text buffer.
//!
//! Every line is a list of styled spans; the display width is tracked as spans
//! are pushed so wide (CJK) characters cannot misalign a panel. Lines are
//! serialised once per frame, never measured afterwards.

use super::style::Style;

#[derive(Clone, Debug)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

#[derive(Clone, Default)]
pub struct Line {
    spans: Vec<Span>,
    width: usize,
}

impl Line {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_text(text: &str, style: Style) -> Self {
        let mut line = Self::new();
        line.push(text, style);
        line
    }

    pub fn push(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        self.width += display_width(text);
        match self.spans.last_mut() {
            Some(span) if span.style == style => span.text.push_str(text),
            _ => self.spans.push(Span {
                text: text.to_string(),
                style,
            }),
        }
    }

    pub fn spaces(&mut self, count: usize, style: Style) {
        if count == 0 {
            return;
        }
        let spaces = " ".repeat(count);
        self.push(&spaces, style);
    }

    pub fn width(&self) -> usize {
        self.width
    }

    /// Fill with spaces up to `width` (no-op when already wider).
    pub fn pad_to(&mut self, width: usize, style: Style) {
        if self.width < width {
            self.spaces(width - self.width, style);
        }
    }

    /// Hard-clip to `width` display columns, splitting a wide glyph into a
    /// space rather than letting it overflow the panel.
    pub fn truncate(&mut self, width: usize) {
        if self.width <= width {
            return;
        }
        let mut used = 0usize;
        let mut kept: usize = 0;
        let mut rebuilt: Vec<Span> = Vec::with_capacity(self.spans.len());
        for span in &self.spans {
            if used >= width {
                break;
            }
            let mut text = String::new();
            for ch in span.text.chars() {
                let w = char_width(ch);
                if used + w > width {
                    if used < width {
                        text.push(' ');
                        used += 1;
                    }
                    break;
                }
                text.push(ch);
                used += w;
            }
            if !text.is_empty() {
                rebuilt.push(Span {
                    text,
                    style: span.style,
                });
            }
            kept += 1;
        }
        let _ = kept;
        self.spans = rebuilt;
        self.width = used;
    }

    pub fn spans(&self) -> &[Span] {
        &self.spans
    }

    /// Serialise with SGR codes, padding to `width`.
    pub fn render(&self, width: usize, out: &mut String) {
        let mut current = Style::PLAIN;
        for span in &self.spans {
            if span.style != current {
                super::style::sgr(span.style, out);
                current = span.style;
            }
            out.push_str(&span.text);
        }
        if current != Style::PLAIN {
            out.push_str("\x1b[0m");
        }
        for _ in self.width..width {
            out.push(' ');
        }
    }
}

const EIGHTHS: [char; 8] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];

/// A cell-accurate progress bar: filled part, then the empty remainder.
pub fn gauge_parts(percent: f32, width: usize) -> (String, String) {
    if width == 0 {
        return (String::new(), String::new());
    }
    let percent = percent.clamp(0.0, 100.0);
    let eighths = (percent / 100.0 * width as f32 * 8.0).round() as usize;
    let mut full = eighths / 8;
    let mut fraction = eighths % 8;
    if full >= width {
        full = width;
        fraction = 0;
    }
    if full + 1 > width {
        fraction = 0;
    }

    let mut filled = String::with_capacity(width * 3);
    for _ in 0..full {
        filled.push(EIGHTHS[7]);
    }
    if fraction > 0 {
        filled.push(EIGHTHS[fraction - 1]);
    }
    let used = full + usize::from(fraction > 0);
    (filled, "░".repeat(width.saturating_sub(used)))
}

const SPARKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// History strip: right-aligned, newest value on the right edge.
pub fn sparkline(values: &[f32], width: usize, max: f32) -> String {
    if width == 0 {
        return String::new();
    }
    let start = values.len().saturating_sub(width);
    let slice = &values[start..];
    let mut out = String::with_capacity(width * 3);
    for _ in 0..width.saturating_sub(slice.len()) {
        out.push(' ');
    }
    let max = if max <= 0.0 { 1.0 } else { max };
    for &value in slice {
        let level = ((value / max).clamp(0.0, 1.0) * 7.0).round() as usize;
        out.push(SPARKS[level.min(7)]);
    }
    out
}

/// Display width in terminal cells: East Asian wide and fullwidth glyphs count
/// as two, combining marks as zero.
pub fn display_width(text: &str) -> usize {
    text.chars().map(char_width).sum()
}

pub fn char_width(ch: char) -> usize {
    let code = ch as u32;
    if code == 0 {
        return 0;
    }
    // Zero-width: combining marks and variation selectors.
    if (0x0300..=0x036F).contains(&code) || (0x200B..=0x200F).contains(&code) || code == 0xFEFF {
        return 0;
    }
    if is_wide(code) {
        2
    } else {
        1
    }
}

fn is_wide(code: u32) -> bool {
    matches!(code,
        0x1100..=0x115F
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xA960..=0xA97F
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE10..=0xFE19
        | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1F64F
        | 0x1F900..=0x1F9FF
        | 0x20000..=0x3FFFD
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_coalesce_and_measure() {
        let mut line = Line::new();
        line.push("abc", Style::PLAIN);
        line.push("de", Style::PLAIN);
        assert_eq!(line.width(), 5);
        assert_eq!(line.spans().len(), 1);
        line.push("中", Style::PLAIN);
        assert_eq!(line.width(), 7);
    }

    #[test]
    fn padding_fills_exactly() {
        let mut line = Line::new();
        line.push("abc", Style::PLAIN);
        line.pad_to(10, Style::PLAIN);
        assert_eq!(line.width(), 10);
        let mut out = String::new();
        line.render(10, &mut out);
        assert_eq!(out, "abc       ");
    }

    #[test]
    fn truncation_stops_on_glyph_boundary() {
        let mut line = Line::new();
        line.push("中文abc", Style::PLAIN);
        line.truncate(5);
        assert_eq!(line.width(), 5);
        let mut out = String::new();
        line.render(5, &mut out);
        assert_eq!(out, "中文a");

        // A wide glyph that would straddle the edge becomes a space.
        let mut line = Line::new();
        line.push("中文abc", Style::PLAIN);
        line.truncate(3);
        assert_eq!(line.width(), 3);
        let mut out = String::new();
        line.render(3, &mut out);
        assert_eq!(out, "中 ");
    }

    #[test]
    fn gauge_is_cell_exact() {
        for percent in [0.0, 1.0, 23.7, 50.0, 99.9, 100.0] {
            for width in [1usize, 5, 12, 40] {
                let (filled, rest) = gauge_parts(percent, width);
                let total = display_width(&filled) + display_width(&rest);
                assert_eq!(total, width, "percent={percent} width={width}");
            }
        }
    }

    #[test]
    fn gauge_fills_at_hundred() {
        let (filled, rest) = gauge_parts(100.0, 10);
        assert_eq!(display_width(&filled), 10);
        assert!(rest.is_empty());
    }

    #[test]
    fn sparkline_is_width_exact() {
        let values = [1.0, 5.0, 10.0, 3.0];
        let text = sparkline(&values, 10, 10.0);
        assert_eq!(display_width(&text), 10);
    }

    #[test]
    fn cjk_widths() {
        assert_eq!(display_width("CPU"), 3);
        assert_eq!(display_width("内存"), 4);
        assert_eq!(display_width("内存 50%"), 8);
    }
}

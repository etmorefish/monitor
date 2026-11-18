//! Number formatting shared by every output mode.

/// Binary units, one decimal from KB up: `1.4 MB`, `131.4 GB`, `0 B`.
pub fn bytes(value: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut scaled = value as f64;
    let mut unit = 0;
    while scaled >= 1024.0 && unit < UNITS.len() - 1 {
        scaled /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        return format!("{value} B");
    }
    if scaled >= 100.0 {
        format!("{scaled:.0} {}", UNITS[unit])
    } else {
        format!("{scaled:.1} {}", UNITS[unit])
    }
}

pub fn rate(bytes_per_second: f64) -> String {
    if !bytes_per_second.is_finite() || bytes_per_second <= 0.5 {
        return "0 B/s".to_string();
    }
    format!("{}/s", bytes(bytes_per_second as u64))
}


/// Elapsed time as `2.4 µs` / `18 µs` / `1.2 ms`.
pub fn micros(value: u64) -> String {
    if value >= 1_000 {
        format!("{:.1} ms", value as f64 / 1_000.0)
    } else {
        format!("{value} µs")
    }
}

/// Clip to a display width, keeping the text intact when it already fits.
pub fn truncate(text: &str, width: usize) -> String {
    if crate::ui::text::display_width(text) <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = crate::ui::text::char_width(ch);
        if used + w > width {
            break;
        }
        out.push(ch);
        used += w;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_scale() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1024), "1.0 KB");
        assert_eq!(bytes(1024 * 1024 * 3 / 2), "1.5 MB");
        assert_eq!(bytes(176 * 1024 * 1024 * 1024), "176 GB");
    }

    #[test]
    fn rates_stay_readable() {
        assert_eq!(rate(0.0), "0 B/s");
        assert_eq!(rate(1536.0), "1.5 KB/s");
    }

    #[test]
    fn truncation_respects_wide_chars() {
        assert_eq!(truncate("Intel Core", 5), "Intel");
        assert_eq!(truncate("中文测试", 4), "中文");
        assert_eq!(truncate("short", 10), "short");
    }
}

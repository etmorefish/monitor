//! Non-interactive output: one text snapshot, one JSON document, one-line
//! status, and CSV logging.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

use crate::format;
use crate::metrics::{Model, Snapshot};

pub fn json(snapshot: &Snapshot, host: &str) -> String {
    let mut out = String::with_capacity(4096);
    out.push('{');
    string_field(&mut out, "timestamp", &crate::sys::host::local_timestamp());
    out.push(',');
    string_field(&mut out, "host", host);
    out.push(',');
    number_field(&mut out, "uptime_s", snapshot.uptime as f64);

    // CPU
    out.push_str(",\"cpu\":{");
    string_field(&mut out, "brand", &snapshot.cpu.brand);
    out.push(',');
    int_field(&mut out, "logical", snapshot.cpu.logical as u64);
    out.push(',');
    match snapshot.cpu.physical {
        Some(cores) => int_field(&mut out, "physical", cores as u64),
        None => out.push_str("\"physical\":null"),
    }
    out.push(',');
    number_field(&mut out, "usage_percent", snapshot.cpu.total as f64);
    out.push(',');
    int_field(&mut out, "current_mhz", snapshot.cpu.mhz as u64);
    out.push(',');
    int_field(&mut out, "base_mhz", snapshot.cpu.base_mhz as u64);
    out.push_str(",\"per_core\":[");
    for (index, usage) in snapshot.cpu.per_core.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        push_number(&mut out, *usage as f64);
    }
    out.push_str("]}");

    // Memory
    let memory = &snapshot.memory;
    out.push_str(",\"memory\":{");
    int_field(&mut out, "total_bytes", memory.total);
    out.push(',');
    int_field(&mut out, "used_bytes", memory.used());
    out.push(',');
    int_field(&mut out, "available_bytes", memory.available);
    out.push(',');
    int_field(&mut out, "cache_bytes", memory.cache);
    out.push(',');
    int_field(&mut out, "commit_bytes", memory.commit);
    out.push(',');
    int_field(&mut out, "commit_limit_bytes", memory.commit_limit);
    out.push(',');
    number_field(&mut out, "usage_percent", memory.percent() as f64);
    out.push(',');
    int_field(&mut out, "processes", memory.processes as u64);
    out.push(',');
    int_field(&mut out, "threads", memory.threads as u64);
    out.push('}');

    // Disks
    out.push_str(",\"disks\":[");
    for (index, volume) in snapshot.volumes.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push('{');
        string_field(&mut out, "letter", &volume.letter.to_string());
        out.push(',');
        string_field(&mut out, "label", &volume.label);
        out.push(',');
        string_field(&mut out, "filesystem", &volume.filesystem);
        out.push(',');
        int_field(&mut out, "total_bytes", volume.total);
        out.push(',');
        int_field(&mut out, "free_bytes", volume.free);
        out.push(',');
        number_field(&mut out, "used_percent", volume.percent() as f64);
        out.push('}');
    }
    out.push(']');

    out.push_str(",\"disk_io\":{");
    number_field(&mut out, "read_bytes_per_sec", snapshot.io_read);
    out.push(',');
    number_field(&mut out, "write_bytes_per_sec", snapshot.io_write);
    out.push('}');

    // Network
    out.push_str(",\"network\":[");
    for (index, net) in snapshot.nets.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push('{');
        string_field(&mut out, "name", &net.name);
        out.push(',');
        out.push_str("\"up\":");
        out.push_str(if net.up { "true" } else { "false" });
        out.push(',');
        int_field(&mut out, "link_bits_per_sec", net.link_bits);
        out.push(',');
        number_field(&mut out, "rx_bytes_per_sec", net.rx_bps);
        out.push(',');
        number_field(&mut out, "tx_bytes_per_sec", net.tx_bps);
        out.push(',');
        int_field(&mut out, "rx_bytes_total", net.rx_total);
        out.push(',');
        int_field(&mut out, "tx_bytes_total", net.tx_total);
        out.push('}');
    }
    out.push(']');

    // Thermal
    out.push_str(",\"thermal\":{\"zones\":[");
    for (index, zone) in snapshot.zones.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push('{');
        string_field(&mut out, "name", &zone.name);
        out.push(',');
        number_field(&mut out, "celsius", zone.celsius as f64);
        out.push('}');
    }
    out.push_str("],\"fans\":[");
    for (index, fan) in snapshot.fans.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push('{');
        string_field(&mut out, "name", &fan.name);
        out.push(',');
        int_field(&mut out, "rpm", fan.rpm as u64);
        out.push('}');
    }
    out.push_str("]}");

    // Power
    out.push_str(",\"power\":");
    match snapshot.power {
        Some(power) if power.available() => {
            out.push('{');
            out.push_str("\"ac_online\":");
            out.push_str(if power.ac_online { "true" } else { "false" });
            out.push(',');
            match power.battery_percent {
                Some(percent) => int_field(&mut out, "battery_percent", percent as u64),
                None => out.push_str("\"battery_percent\":null"),
            }
            out.push(',');
            string_field(&mut out, "state", power_state(power.state));
            out.push('}');
        }
        _ => out.push_str("null"),
    }

    out.push_str(",\"self\":{");
    int_field(&mut out, "private_bytes", snapshot.self_private);
    out.push(',');
    number_field(&mut out, "cpu_percent", snapshot.self_cpu as f64);
    out.push(',');
    int_field(&mut out, "sample_micros", snapshot.tick_us);
    out.push('}');

    out.push('}');
    out
}

fn power_state(state: crate::sys::power::BatteryState) -> &'static str {
    use crate::sys::power::BatteryState::*;
    match state {
        Charging => "charging",
        Discharging => "discharging",
        Idle => "idle",
        NoBattery => "none",
        Unknown => "unknown",
    }
}

/// One compact line, meant for status bars and `grep`.
pub fn mini_line(snapshot: &Snapshot) -> String {
    let max_temp = snapshot
        .zones
        .iter()
        .map(|zone| zone.celsius)
        .fold(f32::MIN, f32::max);
    let hottest = if max_temp > f32::MIN { max_temp } else { 0.0 };

    let net = snapshot
        .nets
        .iter()
        .find(|net| net.up)
        .map(|net| format!("↓{} ↑{}", format::rate(net.rx_bps), format::rate(net.tx_bps)))
        .unwrap_or_else(|| "网络 —".to_string());

    let system_drive = snapshot
        .volumes
        .iter()
        .find(|volume| volume.letter.eq_ignore_ascii_case(&'C'))
        .or_else(|| snapshot.volumes.first());

    let mut parts: Vec<String> = vec![
        format!("CPU {:>5.1}%", snapshot.cpu.total),
        format!(
            "内存 {:>5.1}% {}",
            snapshot.memory.percent(),
            format::bytes(snapshot.memory.used())
        ),
    ];
    if let Some(drive) = system_drive {
        parts.push(format!("{}: {:.0}%", drive.letter, drive.percent()));
    }
    parts.push(net);
    if max_temp > f32::MIN {
        parts.push(format!("{hottest:.0}°C"));
    }
    if let Some(power) = snapshot.power {
        if power.available() {
            if let Some(percent) = power.battery_percent {
                parts.push(format!("电池 {percent}%"));
            }
        }
    }
    parts.join(" │ ")
}

/// One line for `--watch`, prefixed with the wall clock.
pub fn watch_line(snapshot: &Snapshot) -> String {
    let net_rx: f64 = snapshot.nets.iter().map(|net| net.rx_bps).sum();
    let net_tx: f64 = snapshot.nets.iter().map(|net| net.tx_bps).sum();
    let max_temp = snapshot
        .zones
        .iter()
        .map(|zone| zone.celsius)
        .fold(f32::MIN, f32::max);
    format!(
        "{}  CPU {:>5.1}%  内存 {:>5.1}%  磁盘R {:>10}  磁盘W {:>10}  网络↓ {:>10} ↑ {:>10}  温度 {}",
        snapshot.time,
        snapshot.cpu.total,
        snapshot.memory.percent(),
        format::rate(snapshot.io_read),
        format::rate(snapshot.io_write),
        format::rate(net_rx),
        format::rate(net_tx),
        if max_temp > f32::MIN {
            format!("{max_temp:.0}°C")
        } else {
            "—".to_string()
        }
    )
}

pub const CSV_HEADER: &str = "timestamp,host,cpu_percent,cpu_mhz,mem_percent,mem_used_bytes,\
disk_read_bps,disk_write_bps,net_rx_bps,net_tx_bps,temp_c,fan_rpm,battery_percent,ac_online";

pub fn csv_row(snapshot: &Snapshot, host: &str) -> String {
    let net_rx: f64 = snapshot.nets.iter().map(|net| net.rx_bps).sum();
    let net_tx: f64 = snapshot.nets.iter().map(|net| net.tx_bps).sum();
    let max_temp = snapshot
        .zones
        .iter()
        .map(|zone| zone.celsius)
        .fold(f32::MIN, f32::max);
    let fan = snapshot.fans.first().map_or(0, |fan| fan.rpm);
    let (battery, ac) = match snapshot.power {
        Some(power) => (
            power.battery_percent.map_or(String::new(), |p| p.to_string()),
            if power.ac_online { "1" } else { "0" },
        ),
        None => (String::new(), "0"),
    };
    format!(
        "{},{},{:.1},{},{:.1},{},{:.0},{:.0},{:.0},{:.0},{},{},{},{}",
        crate::sys::host::local_timestamp(),
        csv_escape(host),
        snapshot.cpu.total,
        snapshot.cpu.mhz,
        snapshot.memory.percent(),
        snapshot.memory.used(),
        snapshot.io_read,
        snapshot.io_write,
        net_rx,
        net_tx,
        if max_temp > f32::MIN {
            format!("{max_temp:.1}")
        } else {
            String::new()
        },
        fan,
        battery,
        ac
    )
}

fn csv_escape(value: &str) -> String {
    if value.contains(',') || value.contains('"') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

pub struct Logger {
    file: Option<File>,
    needs_header: bool,
    host: String,
}

impl Logger {
    pub fn open(path: &Path, host: &str) -> std::io::Result<Self> {
        let needs_header = std::fs::metadata(path).map(|meta| meta.len() == 0).unwrap_or(true);
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            file: Some(file),
            needs_header,
            host: host.to_string(),
        })
    }

    /// The host name is only known once the sampler exists.
    pub fn set_host(&mut self, host: &str) {
        self.host = host.to_string();
    }

    pub fn write(&mut self, snapshot: &Snapshot) {
        let Some(file) = self.file.as_mut() else {
            return;
        };
        if self.needs_header {
            self.needs_header = false;
            let _ = writeln!(file, "{CSV_HEADER}");
        }
        let _ = writeln!(file, "{}", csv_row(snapshot, &self.host));
    }
}

// --- JSON helpers -----------------------------------------------------------

fn key(out: &mut String, name: &str) {
    out.push('"');
    escape_into(out, name);
    out.push_str("\":");
}

fn string_field(out: &mut String, name: &str, value: &str) {
    key(out, name);
    out.push('"');
    escape_into(out, value);
    out.push('"');
}

fn int_field(out: &mut String, name: &str, value: u64) {
    key(out, name);
    out.push_str(&value.to_string());
}

fn number_field(out: &mut String, name: &str, value: f64) {
    key(out, name);
    push_number(out, value);
}

fn push_number(out: &mut String, value: f64) {
    if !value.is_finite() {
        out.push('0');
        return;
    }
    if value.fract().abs() < 0.005 {
        out.push_str(&format!("{value:.0}"));
    } else {
        out.push_str(&format!("{value:.2}"));
    }
}

fn escape_into(out: &mut String, value: &str) {
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
}

/// `--once` text body: the same layout as the interactive screen, unstyled.
pub fn text_frame(model: &Model, width: usize) -> String {
    let palette = crate::ui::style::Palette::dark();
    let lines = crate::ui::render(model, width, 60, &palette);
    crate::ui::plain(&lines, width)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> (Snapshot, String) {
        let mut sampler = crate::metrics::Sampler::new(1.0, None, true);
        sampler.sample();
        (sampler.model.snap.clone(), sampler.model.host.clone())
    }

    #[test]
    fn json_is_well_formed_and_complete() {
        let (snapshot, host) = sample();
        let text = json(&snapshot, &host);
        assert!(text.starts_with('{') && text.ends_with('}'));
        assert!(text.contains("\"cpu\":{"));
        assert!(text.contains("\"memory\":{"));
        assert!(text.contains("\"disks\":["));
        assert!(text.contains("\"network\":["));
        assert!(text.contains("\"thermal\":{"));
        assert!(text.contains("\"self\":{"));
        // Braces balance once string contents are ignored.
        let mut depth = 0i32;
        for ch in text.chars() {
            match ch {
                '{' | '[' => depth += 1,
                '}' | ']' => depth -= 1,
                _ => {}
            }
            assert!(depth >= 0);
        }
        assert_eq!(depth, 0);
    }

    #[test]
    fn json_escapes_strings() {
        let mut out = String::new();
        string_field(&mut out, "k", "a\"b\\c");
        assert_eq!(out, "\"k\":\"a\\\"b\\\\c\"");
    }

    #[test]
    fn csv_row_has_the_header_columns() {
        let (snapshot, host) = sample();
        let columns = CSV_HEADER.split(',').count();
        assert_eq!(csv_row(&snapshot, &host).split(',').count(), columns);
    }

    #[test]
    fn mini_line_is_one_line() {
        let (snapshot, _) = sample();
        let line = mini_line(&snapshot);
        assert!(!line.contains('\n'));
        assert!(line.contains("CPU"));
    }
}

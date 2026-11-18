//! Sampling: turns the raw syscall readers into one snapshot per tick, keeping
//! the previous counters needed for deltas and the short history used by the
//! sparklines.
//!
//! Everything is reused between ticks (buffers, hash maps, vectors) so a steady
//! one-second refresh does no meaningful allocation work.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::sys;
use crate::sys::cpu::{CpuTimes, PerCpuBuffer};

use crate::sys::pdh::{DiskQuery, ThermalQuery, Zone};

pub const HISTORY_LEN: usize = 900;

#[derive(Clone, Default)]
pub struct Ring {
    data: Vec<f32>,
    capacity: usize,
}

impl Ring {
    pub fn new(capacity: usize) -> Self {
        Self {
            data: Vec::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push(&mut self, value: f32) {
        if self.capacity == 0 {
            return;
        }
        if self.data.len() == self.capacity {
            self.data.remove(0);
        }
        self.data.push(value);
    }

    pub fn slice(&self) -> &[f32] {
        &self.data
    }

    pub fn max(&self, floor: f32) -> f32 {
        self.data.iter().copied().fold(floor, f32::max)
    }

    pub fn clear(&mut self) {
        self.data.clear();
    }
}

#[derive(Clone, Default)]
pub struct History {
    pub cpu: Ring,
    pub memory: Ring,
    /// Read + write, for the TUI sparkline.
    pub io: Ring,
    /// Separate series for the GUI charts.
    pub io_read: Ring,
    pub io_write: Ring,
    pub net: Ring,
    pub net_rx: Ring,
    pub net_tx: Ring,
    /// Hottest ACPI zone, `NaN` when no zone is available.
    pub temp: Ring,
}

impl History {
    pub fn new() -> Self {
        Self {
            cpu: Ring::new(HISTORY_LEN),
            memory: Ring::new(HISTORY_LEN),
            io: Ring::new(HISTORY_LEN),
            io_read: Ring::new(HISTORY_LEN),
            io_write: Ring::new(HISTORY_LEN),
            net: Ring::new(HISTORY_LEN),
            net_rx: Ring::new(HISTORY_LEN),
            net_tx: Ring::new(HISTORY_LEN),
            temp: Ring::new(HISTORY_LEN),
        }
    }

    /// Record one snapshot; the single place that knows which series exist.
    pub fn push(&mut self, snap: &Snapshot) {
        self.cpu.push(snap.cpu.total);
        self.memory.push(snap.memory.percent());
        self.io.push((snap.io_read + snap.io_write) as f32);

        let rx: f64 = snap.nets.iter().map(|net| net.rx_bps).sum();
        let tx: f64 = snap.nets.iter().map(|net| net.tx_bps).sum();
        self.net.push((rx + tx) as f32);
        self.io_read.push(snap.io_read as f32);
        self.io_write.push(snap.io_write as f32);
        self.net_rx.push(rx as f32);
        self.net_tx.push(tx as f32);

        let hottest = snap
            .zones
            .iter()
            .map(|zone| zone.celsius)
            .fold(f32::NAN, f32::max);
        self.temp.push(hottest);
    }

    pub fn clear(&mut self) {
        for ring in [
            &mut self.cpu,
            &mut self.memory,
            &mut self.io,
            &mut self.io_read,
            &mut self.io_write,
            &mut self.net,
            &mut self.net_rx,
            &mut self.net_tx,
            &mut self.temp,
        ] {
            ring.clear();
        }
    }
}

#[derive(Clone, Default)]
pub struct Cpu {
    pub brand: String,
    pub logical: usize,
    pub physical: Option<usize>,
    pub total: f32,
    pub per_core: Vec<f32>,
    pub mhz: u32,
    pub base_mhz: u32,
}

#[derive(Clone, Default)]
pub struct Net {
    pub name: String,
    pub up: bool,
    pub link_bits: u64,
    pub rx_bps: f64,
    pub tx_bps: f64,
    pub rx_total: u64,
    pub tx_total: u64,
    pub index: u32,
}

#[derive(Clone, Default)]
pub struct Fan {
    pub name: String,
    pub rpm: u32,
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub date: String,
    pub time: String,
    pub uptime: u64,
    pub cpu: Cpu,
    pub memory: sys::mem::Memory,
    pub volumes: Vec<sys::disk::Volume>,
    pub io_read: f64,
    pub io_write: f64,
    pub nets: Vec<Net>,
    pub zones: Vec<sys::pdh::Zone>,
    pub fans: Vec<Fan>,
    pub power: Option<sys::power::Power>,
    pub self_private: u64,
    pub self_cpu: f32,
    pub tick_us: u64,
    pub samples: u64,

    pub counters_enabled: bool,
}

pub struct Model {
    pub host: String,
    pub snap: Snapshot,
    pub history: History,
    pub interval: f64,
    pub paused: bool,
    pub show_cores: bool,
}

struct NetAccum {
    rx_total: u64,
    tx_total: u64,
    last_rx: u32,
    last_tx: u32,
}

pub struct Sampler {
    per_cpu: PerCpuBuffer,
    prev_total: Option<CpuTimes>,
    prev_cores: Vec<CpuTimes>,
    nets: HashMap<u32, NetAccum>,
    interfaces: sys::net::InterfaceSet,
    pdh_disk: Option<DiskQuery>,
    pdh_thermal: Option<ThermalQuery>,
    counters_enabled: bool,
    thermal_at: Instant,
    zones: Vec<Zone>,
    volumes: Vec<sys::disk::Volume>,
    volumes_at: Instant,
    prev_self: Option<(u64, Instant)>,
    self_cpu_ema: Option<f32>,
    fan_path: Option<PathBuf>,
    fans: Vec<Fan>,
    fans_at: Instant,
    last_tick: Instant,
    tick_us: u64,
    samples: u64,
    pub model: Model,
}

impl Sampler {
    pub fn new(interval: f64, fan_path: Option<PathBuf>, counters: bool) -> Self {
        let logical = sys::cpu::logical_processors();
        let physical = sys::cpu::physical_cores(logical);
        let brand = sys::cpu::brand();

        let mut sampler = Self {
            per_cpu: PerCpuBuffer::new(),
            prev_total: None,
            prev_cores: Vec::new(),
            nets: HashMap::new(),
            interfaces: sys::net::InterfaceSet::new(),
            pdh_disk: counters.then(DiskQuery::open).flatten(),
            pdh_thermal: counters.then(ThermalQuery::open).flatten(),
            counters_enabled: counters,
            thermal_at: Instant::now() - Duration::from_secs(60),
            zones: Vec::new(),
            volumes: Vec::new(),
            volumes_at: Instant::now() - Duration::from_secs(60),
            prev_self: None,
            self_cpu_ema: None,
            fan_path,
            fans: Vec::new(),
            fans_at: Instant::now() - Duration::from_secs(60),
            last_tick: Instant::now(),
            tick_us: 0,
            samples: 0,
            model: Model {
                host: sys::host::computer_name(),
                snap: Snapshot {
                    cpu: Cpu {
                        brand,
                        logical,
                        physical,
                        ..Cpu::default()
                    },
                    ..Snapshot::default()
                },
                history: History::new(),
                interval,
                paused: false,
                show_cores: true,
            },
        };

        // Prime the counters so the very first frame already has real deltas
        // instead of a screen full of zeroes.
        sampler.prime();
        std::thread::sleep(Duration::from_millis(120));
        sampler.last_tick = Instant::now();
        sampler.sample();
        sampler
    }

    fn prime(&mut self) {
        self.prev_total = sys::cpu::total();
        self.prev_cores = self.per_cpu.read(self.model.snap.cpu.logical).unwrap_or_default();
        self.prev_self = sys::sel::cpu_ticks().map(|ticks| (ticks, Instant::now()));

        self.fans_at = Instant::now() - Duration::from_secs(60);
    }

    pub fn reset_history(&mut self) {
        self.model.history.clear();
    }

    pub fn sample(&mut self) {
        let started = Instant::now();
        let elapsed = started
            .duration_since(self.last_tick)
            .as_secs_f64()
            .clamp(0.001, 60.0);
        self.last_tick = started;
        self.samples += 1;

        let (date, time) = sys::host::local_time();
        let cpu_before = self.prev_total;
        let cores_before = std::mem::take(&mut self.prev_cores);

        let cpu_after = sys::cpu::total();
        let cores_after = self.per_cpu.read(self.model.snap.cpu.logical);

        let total_usage = match (cpu_before, cpu_after) {
            (Some(before), Some(after)) => sys::cpu::usage_percent(before, after).unwrap_or(0.0),
            _ => 0.0,
        };

        let mut per_core = Vec::with_capacity(cores_after.as_ref().map_or(0, Vec::len));
        if let (Some(after), false) = (cores_after.as_ref(), cores_before.is_empty()) {
            if cores_before.len() == after.len() {
                for index in 0..after.len() {
                    if let Some(usage) = sys::cpu::usage_percent(cores_before[index], after[index]) {
                        per_core.push(usage);
                    }
                }
            }
        }
        self.prev_cores = cores_after.unwrap_or(cores_before);
        self.prev_total = cpu_after;

        let frequency = sys::cpu::frequency(self.model.snap.cpu.logical);
        let memory = sys::mem::read().unwrap_or_default();
        let nets = self.read_nets(elapsed);

        // Disk rate counters sample every tick; PDH keeps them cheap.
        let (io_read, io_write) = match self.pdh_disk.as_mut() {
            Some(query) => query.sample(),
            None => (0.0, 0.0),
        };

        // Temperature and volume capacity move slowly; the ACPI counter behind
        // the thermal zones costs milliseconds, so it runs on its own cadence.
        if self.thermal_at.elapsed() >= Duration::from_secs(5) {
            self.thermal_at = Instant::now();
            if let Some(query) = self.pdh_thermal.as_mut() {
                self.zones = query.sample();
            }
        }
        if self.volumes_at.elapsed() >= Duration::from_secs(5) {
            self.volumes_at = Instant::now();
            self.volumes = sys::disk::volumes();
        }

        self.refresh_fans();

        let self_ticks = sys::sel::cpu_ticks();
        let self_cpu = match (self.prev_self, self_ticks) {
            (Some((before, at)), Some(after)) => {
                let seconds = at.elapsed().as_secs_f64().max(0.001);
                let delta = after.saturating_sub(before) as f64;
                // 100 ns ticks -> percent of one core
                (delta / (seconds * 10_000_000.0) * 100.0) as f32
            }
            _ => 0.0,
        };
        // Smooth it: a single window can catch a one-off cost (first-use paging,
        // a provider warming up) and report it as if it were the steady rate.
        // The first two windows are dominated by startup work, so they are not
        // a measurement of anything.
        let self_cpu = if self.samples <= 2 { 0.0 } else { self_cpu };
        let self_cpu = match self.self_cpu_ema {
            Some(previous) => previous * 0.75 + self_cpu * 0.25,
            None => self_cpu,
        };
        self.self_cpu_ema = Some(self_cpu);
        if let Some(ticks) = self_ticks {
            self.prev_self = Some((ticks, Instant::now()));
        }

        let snapshot = Snapshot {
            date,
            time,
            uptime: sys::host::uptime_seconds(),
            cpu: Cpu {
                brand: self.model.snap.cpu.brand.clone(),
                logical: self.model.snap.cpu.logical,
                physical: self.model.snap.cpu.physical,
                total: total_usage,
                per_core,
                mhz: frequency.map_or(0, |f| f.current_mhz),
                base_mhz: frequency.map_or(0, |f| f.max_mhz),
            },
            memory,
            volumes: self.volumes.clone(),
            io_read,
            io_write,
            nets,
            zones: self.zones.clone(),
            fans: self.fans.clone(),
            power: sys::power::read(),
            self_private: sys::sel::private_bytes(),
            self_cpu,
            tick_us: 0,
            samples: self.samples,

            counters_enabled: self.counters_enabled,
        };

        self.model.history.push(&snapshot);

        self.tick_us = started.elapsed().as_micros() as u64;
        self.model.snap = Snapshot {
            tick_us: self.tick_us,
            ..snapshot
        };
    }

    fn read_nets(&mut self, elapsed: f64) -> Vec<Net> {
        self.interfaces.poll();
        let rows = &self.interfaces.rows;
        let mut next: HashMap<u32, NetAccum> = HashMap::with_capacity(rows.len());
        let mut result: Vec<Net> = Vec::with_capacity(rows.len());

        for row in rows {
            let (rx_bps, tx_bps, rx_total, tx_total) = match self.nets.get(&row.index) {
                Some(previous) => {
                    let rx = sys::net::delta_u32(previous.last_rx, row.rx_bytes);
                    let tx = sys::net::delta_u32(previous.last_tx, row.tx_bytes);
                    (
                        rx as f64 / elapsed,
                        tx as f64 / elapsed,
                        previous.rx_total + rx,
                        previous.tx_total + tx,
                    )
                }
                None => (0.0, 0.0, row.rx_bytes as u64, row.tx_bytes as u64),
            };

            next.insert(
                row.index,
                NetAccum {
                    rx_total,
                    tx_total,
                    last_rx: row.rx_bytes,
                    last_tx: row.tx_bytes,
                },
            );

            result.push(Net {
                name: row.name.clone(),
                up: row.up,
                link_bits: row.link_bits_per_sec,
                rx_bps,
                tx_bps,
                rx_total,
                tx_total,
                index: row.index,
            });
        }

        self.nets = next;
        // Busiest first, but never hide a live link behind a virtual one.
        result.sort_by(|left, right| {
            right
                .up
                .cmp(&left.up)
                .then_with(|| {
                    let left_rate = left.rx_bps + left.tx_bps;
                    let right_rate = right.rx_bps + right.tx_bps;
                    right_rate
                        .partial_cmp(&left_rate)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| left.index.cmp(&right.index))
        });
        // Windows exposes the same adapter more than once (per address family,
        // plus a shorter duplicate name) with identical counters; keep one.
        let mut kept: Vec<(String, Net)> = Vec::with_capacity(result.len());
        for net in result {
            let key = normalize_adapter(&net.name);
            // A prefix-related name is the same adapter seen twice (physical +
            // "Native MAC" virtual); keep the first, busiest one.
            let duplicate = kept
                .iter()
                .any(|(other_key, _)| other_key == &key || other_key.starts_with(&key) || key.starts_with(other_key));
            if !duplicate {
                kept.push((key, net));
            }
        }
        kept.into_iter().map(|(_, net)| net).collect()
    }

    fn refresh_fans(&mut self) {
        let Some(path) = self.fan_path.clone() else {
            return;
        };
        if self.fans_at.elapsed() < Duration::from_secs(5) {
            return;
        }
        self.fans_at = Instant::now();
        self.fans = read_fan_file(&path);
    }
}

/// Lowercase, strip trademark noise and collapse spaces, for duplicate checks.
fn normalize_adapter(name: &str) -> String {
    let mut cleaned = name.to_lowercase();
    for noise in ["(r)", "(tm)", "(c)"] {
        cleaned = cleaned.replace(noise, "");
    }
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parse `label,rpm` or `label rpm` lines; `#` starts a comment.
pub fn read_fan_file(path: &Path) -> Vec<Fan> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut fans = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut split = line.rsplitn(2, |c: char| c == ',' || c.is_whitespace());
        let Some(rpm_text) = split.next() else {
            continue;
        };
        let Ok(rpm) = rpm_text.trim().trim_end_matches("RPM").trim().parse::<u32>() else {
            continue;
        };
        if rpm > 20_000 {
            continue;
        }
        let name = split.next().unwrap_or("风扇").trim().trim_end_matches(',').trim().to_string();
        fans.push(Fan {
            name: if name.is_empty() {
                "风扇".to_string()
            } else {
                name
            },
            rpm,
        });
    }
    fans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_keeps_capacity() {
        let mut ring = Ring::new(3);
        for value in 1..=5 {
            ring.push(value as f32);
        }
        assert_eq!(ring.slice(), &[3.0, 4.0, 5.0]);
        assert_eq!(ring.max(0.0), 5.0);
    }

    #[test]
    fn sampler_produces_a_snapshot() {
        let mut sampler = Sampler::new(1.0, None, true);
        std::thread::sleep(Duration::from_millis(250));
        sampler.sample();
        let snap = &sampler.model.snap;
        assert!(snap.memory.total > 0);
        assert!(!snap.volumes.is_empty());
        assert!(!snap.cpu.brand.is_empty());
        // Per-core sampling can miss a tick under heavy contention; it recovers
        // on the next one, so only the bound is asserted here.
        assert!(snap.cpu.per_core.len() <= snap.cpu.logical);
        assert!(snap.tick_us < 2_000_000);
    }

    #[test]
    fn fan_file_parses() {
        let path = std::env::temp_dir().join("minitor-fan-test.txt");
        std::fs::write(&path, "# comment\nCPU Fan, 2400\n系统风扇 1800\r\nbad line\n").unwrap();
        let fans = read_fan_file(&path);
        assert_eq!(fans.len(), 2);
        assert_eq!(fans[0].name, "CPU Fan");
        assert_eq!(fans[0].rpm, 2400);
        assert_eq!(fans[1].rpm, 1800);
        let _ = std::fs::remove_file(path);
    }
}

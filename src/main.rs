//! monitor — a Windows resource monitor built to be forgotten about.
//!
//! Sampling is a handful of syscalls per tick (~100 µs), the UI is a single
//! UTF-16 frame written once per refresh, and the process keeps itself at
//! below-normal priority. Typical steady state: a few MB of RSS and well under
//! 0.1 % of one core.

use monitor::{cli, format, metrics, output, sys, term, ui};

use std::process::ExitCode;
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::time::{Duration, Instant};

use ui::style::Palette;

fn main() -> ExitCode {
    let options = match cli::parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("monitor: {message}");
            eprintln!("试试 monitor --help");
            return ExitCode::from(2);
        }
    };

    if options.help {
        print!("{}", cli::HELP);
        return ExitCode::SUCCESS;
    }
    if options.version {
        println!("monitor {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    if options.low_priority {
        sys::sel::set_low_priority();
    }

    // A panic must not leave the terminal in the alternate screen.
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        term::emergency_restore();
        previous_hook(info);
    }));

    if options.bench {
        return run_bench();
    }
    if options.demo {
        return run_demo(&options);
    }
    if options.once {
        return run_once(&options);
    }
    if let Some(seconds) = options.watch {
        return run_watch(&options, seconds);
    }
    run_tui(&options)
}

fn run_once(options: &cli::Options) -> ExitCode {
    let mut sampler = metrics::Sampler::new(options.interval, options.fan_file.clone(), !options.lean);
    // One more interval gives the rate counters a meaningful window.
    std::thread::sleep(Duration::from_millis(900));
    sampler.sample();

    if options.json {
        println!(
            "{}",
            output::json(&sampler.model.snap, &sampler.model.host)
        );
    } else if options.mini {
        println!("{}", output::mini_line(&sampler.model.snap));
    } else {
        let width = term::current_width(100);
        print!("{}", output::text_frame(&sampler.model, width));
    }
    ExitCode::SUCCESS
}

fn run_watch(options: &cli::Options, seconds: f64) -> ExitCode {
    let mut sampler = metrics::Sampler::new(options.interval, options.fan_file.clone(), !options.lean);
    let mut logger = match &options.log {
        Some(path) => match output::Logger::open(path, &sampler.model.host) {
            Ok(logger) => Some(logger),
            Err(error) => {
                eprintln!("monitor: 无法写入日志 {}: {error}", path.display());
                None
            }
        },
        None => None,
    };

    loop {
        sampler.sample();
        if let Some(logger) = logger.as_mut() {
            logger.write(&sampler.model.snap);
        }
        if options.trace {
            eprintln!(
                "# tick {:.3} ms, sample {}",
                sampler.model.snap.tick_us as f64 / 1000.0,
                sampler.model.snap.samples
            );
        }
        println!("{}", output::watch_line(&sampler.model.snap));
        std::thread::sleep(Duration::from_secs_f64(seconds));
    }
}

fn run_tui(options: &cli::Options) -> ExitCode {
    // Open the log before the screen switches to the alternate buffer, so a
    // failure can still be reported on the normal screen.
    let mut logger = match &options.log {
        Some(path) => match output::Logger::open(path, "") {
            Ok(logger) => Some(logger),
            Err(error) => {
                eprintln!("monitor: 无法写入日志 {}: {error}", path.display());
                return ExitCode::from(1);
            }
        },
        None => None,
    };

    let mut terminal = match term::Terminal::enter() {
        Some(terminal) => terminal,
        None => {
            eprintln!("monitor: 当前输出不是交互式控制台（可能被重定向或运行在无终端环境）。");
            eprintln!("         请直接在本机终端里运行，或改用：");
            eprintln!("           monitor --once     打印一次快照");
            eprintln!("           monitor --json     输出一次 JSON");
            eprintln!("           monitor -w 5       无界面持续输出");
            return ExitCode::from(1);
        }
    };

    let mut theme = options.theme.clone();
    let mut palette = Palette::by_name(&theme);

    // First paint right away: opening the temperature provider takes a moment
    // and a blank console for half a second feels like a slow program.
    {
        let (width, height) = terminal.size();
        let lines = vec![ui::text::Line::from_text(
            "  monitor  正在读取传感器…",
            ui::style::Style::fg(palette.dim),
        )];
        terminal.draw(&ui::serialize(&lines, width, height));
    }

    let (sender, receiver) = channel();
    term::Terminal::spawn_input(sender);

    let mut sampler = metrics::Sampler::new(
        options.interval,
        options.fan_file.clone(),
        !options.lean,
    );
    if let Some(logger) = logger.as_mut() {
        logger.set_host(&sampler.model.host);
    }

    let mut next_tick = Instant::now();
    loop {
        let now = Instant::now();
        if now >= next_tick {
            if !sampler.model.paused {
                sampler.sample();
                if let Some(logger) = logger.as_mut() {
                    logger.write(&sampler.model.snap);
                }
            }
            next_tick = now + Duration::from_secs_f64(sampler.model.interval);
        }

        let (width, height) = terminal.size();
        let lines = ui::render(&sampler.model, width, height, &palette);
        let frame = ui::serialize(&lines, width, height);
        terminal.draw(&frame);

        let wait = next_tick.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(wait) {
            Ok(key) => match key {
                b'q' | b'Q' | 0x03 => break,
                b'p' | b'P' => {
                    sampler.model.paused = !sampler.model.paused;
                    if !sampler.model.paused {
                        next_tick = Instant::now();
                    }
                }
                b'+' | b'=' => {
                    sampler.model.interval = (sampler.model.interval - 0.1).max(0.1);
                    next_tick = Instant::now();
                }
                b'-' | b'_' => {
                    sampler.model.interval = (sampler.model.interval + 0.1).min(10.0);
                    next_tick = Instant::now();
                }
                b'c' | b'C' => sampler.model.show_cores = !sampler.model.show_cores,
                b't' | b'T' => {
                    theme = if theme == "dark" {
                        "light".to_string()
                    } else {
                        "dark".to_string()
                    };
                    palette = Palette::by_name(&theme);
                }
                b'r' | b'R' => sampler.reset_history(),
                _ => {}
            },
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    terminal.restore();
    ExitCode::SUCCESS
}

/// Per-sensor cost, so "low overhead" is a measured claim rather than a hope.
fn run_bench() -> ExitCode {
    use metrics::Sampler;
    use std::time::Instant;

    fn measure<F: FnMut()>(name: &str, iterations: u32, mut operation: F) {
        // Warm up: first calls page in code and open lazy kernel paths.
        for _ in 0..3 {
            operation();
        }
        let mut worst = 0u128;
        let started = Instant::now();
        for _ in 0..iterations {
            let tick = Instant::now();
            operation();
            worst = worst.max(tick.elapsed().as_micros());
        }
        let average = started.elapsed().as_micros() / iterations as u128;
        println!("  {name:<28} 平均 {average:>7} µs   最慢 {worst:>7} µs");
    }

    println!(
        "  [内存] 刚启动         私有 {} 工作集 {}",
        format::bytes(sys::sel::private_bytes()),
        format::bytes(sys::sel::working_set())
    );

    let logical = sys::cpu::logical_processors();
    let mut per_cpu = sys::cpu::PerCpuBuffer::new();
    let mut scratch = Vec::new();
    let mut disk_query = sys::pdh::DiskQuery::open();
    let mut thermal_query = sys::pdh::ThermalQuery::open();
    println!(
        "  [内存] 采集器全部打开 私有 {}",
        format::bytes(sys::sel::private_bytes())
    );

    println!("monitor 采集器耗时（每项 200 次）");
    measure("GetSystemTimes (CPU 总量)", 200, || {
        sys::cpu::total();
    });
    measure("NtQuerySystemInformation (每核)", 200, || {
        per_cpu.read(logical);
    });
    measure("CallNtPowerInformation (频率)", 200, || {
        sys::cpu::frequency(logical);
    });
    measure("GlobalMemoryStatus + 内存统计", 200, || {
        sys::mem::read();
    });
    measure("卷容量 (GetDiskFreeSpaceEx)", 200, || {
        sys::disk::volumes();
    });
    measure("GetIfTable (网络)", 200, || {
        sys::net::interfaces(&mut scratch);
    });
    measure("GetIfTable2 (对照, 未解析)", 100, || {
        let mut table: *mut core::ffi::c_void = std::ptr::null_mut();
        let status = unsafe { sys::ffi::GetIfTable2(&mut table) };
        if status == 0 && !table.is_null() {
            unsafe { sys::ffi::FreeMibTable(table) };
        }
    });
    // Per-interface refresh, for comparison with the full table enumeration.
    let rows = sys::net::interfaces(&mut scratch);
    if let Some(first) = rows.first().map(|row| row.index) {
        measure("GetIfEntry (单接口)", 200, || {
            let mut row: sys::ffi::MIB_IFROW = unsafe { core::mem::zeroed() };
            row.dwIndex = first;
            unsafe { sys::ffi::GetIfEntry(&mut row) };
        });
    }
    measure("PDH 磁盘速率", 200, || {
        if let Some(query) = disk_query.as_mut() {
            query.sample();
        }
    });
    measure("PDH 热区温度", 50, || {
        if let Some(query) = thermal_query.as_mut() {
            query.sample();
        }
    });
    measure("GetSystemPowerStatus (电源)", 200, || {
        sys::power::read();
    });
    measure("GetProcessMemoryInfo (自身)", 200, || {
        sys::sel::working_set();
    });
    measure("GetLocalTime (时间)", 200, || {
        sys::host::local_time();
    });

    let mut sampler = Sampler::new(1.0, None, true);
    measure("完整一次采样 (sample)", 100, || {
        sampler.sample();
    });
    println!(
        "  最近一次完整采样             {} µs / {} 次采样",
        sampler.model.snap.tick_us, sampler.model.snap.samples
    );

    // Realistic cadence: what does a second of monitoring actually cost?
    let cpu_before = sys::sel::cpu_ticks().unwrap_or(0);
    let wall = Instant::now();
    let mut sample_micros = 0u64;
    for _ in 0..10 {
        std::thread::sleep(std::time::Duration::from_secs(1));
        sampler.sample();
        sample_micros += sampler.model.snap.tick_us;
    }
    let cpu_seconds = (sys::sel::cpu_ticks().unwrap_or(0).saturating_sub(cpu_before)) as f64 / 1e7;
    let wall_seconds = wall.elapsed().as_secs_f64();
    println!(
        "  1 秒节奏 10 次: 采样合计 {} µs, 进程 CPU {:.1} ms, 墙钟 {:.1}s => 单核 {:.3}%",
        sample_micros,
        cpu_seconds * 1000.0,
        wall_seconds,
        cpu_seconds / wall_seconds * 100.0
    );
    println!(
        "  自身内存: 工作集 {} / 私有 {}",
        format::bytes(sys::sel::working_set()),
        format::bytes(sys::sel::private_bytes())
    );

    // Growth check: does a minute of monitoring cost anything (leaks, caches)?
    let mut probe = metrics::Sampler::new(1.0, None, true);
    let before_private = sys::sel::private_bytes();
    for _ in 0..6 {
        std::thread::sleep(std::time::Duration::from_millis(900));
        probe.sample();
    }
    println!(
        "  6 秒后内存变化: 私有 {} -> {}",
        format::bytes(before_private),
        format::bytes(sys::sel::private_bytes())
    );
    ExitCode::SUCCESS
}

/// Render frames without a terminal, for layout checks and screenshots.
fn run_demo(options: &cli::Options) -> ExitCode {
    let mut sampler = metrics::Sampler::new(options.interval, options.fan_file.clone(), !options.lean);
    let palette = Palette::by_name(&options.theme);
    let width = options.width.unwrap_or(110).max(30);
    let height = options.height.unwrap_or(30).max(8);
    let frames = options.frames.max(1);

    for index in 0..frames {
        if index > 0 {
            std::thread::sleep(Duration::from_millis(700));
            sampler.sample();
        }
        let lines = ui::render(&sampler.model, width, height, &palette);
        print!("{}", ui::plain(&lines, width));
        if index + 1 < frames {
            println!("{}", "─".repeat(width));
        }
    }
    ExitCode::SUCCESS
}

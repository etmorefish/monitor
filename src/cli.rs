//! Command line parsing. Hand-rolled: one small struct, no argument crate.

use std::path::PathBuf;

/// Colour depth for the fancy shell; `Auto` sniffs Windows Terminal / COLORTERM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorMode {
    Auto,
    Indexed,
    Truecolor,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub interval: f64,
    pub once: bool,
    pub json: bool,
    pub watch: Option<f64>,
    pub log: Option<PathBuf>,
    pub fan_file: Option<PathBuf>,
    pub theme: String,
    pub fancy: bool,
    pub color: ColorMode,
    pub lean: bool,
    pub low_priority: bool,
    pub mini: bool,
    pub demo: bool,
    pub bench: bool,
    pub trace: bool,
    pub width: Option<usize>,
    pub height: Option<usize>,
    pub frames: usize,
    pub help: bool,
    pub version: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            interval: 1.0,
            once: false,
            json: false,
            watch: None,
            log: None,
            fan_file: None,
            theme: "dark".to_string(),
            fancy: false,
            color: ColorMode::Auto,
            lean: false,
            low_priority: true,
            mini: false,
            demo: false,
            bench: false,
            trace: false,
            width: None,
            height: None,
            frames: 3,
            help: false,
            version: false,
        }
    }
}

pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Options, String> {
    let mut options = Options::default();
    let mut args = args.into_iter().peekable();

    while let Some(arg) = args.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) => (flag.to_string(), Some(value.to_string())),
            None => (arg.clone(), None),
        };

        let mut take_value = |name: &str| -> Result<String, String> {
            if let Some(value) = inline.clone() {
                return Ok(value);
            }
            args.next()
                .ok_or_else(|| format!("{name} 需要一个参数"))
        };

        match flag.as_str() {
            "-h" | "--help" => options.help = true,
            "-V" | "--version" => options.version = true,
            "-o" | "--once" => options.once = true,
            "--json" => {
                options.json = true;
                options.once = true;
            }
            "--mini" => {
                options.mini = true;
                options.once = true;
            }
            "--demo" => options.demo = true,
            "--fancy" => options.fancy = true,
            "--color" => {
                let value = take_value("--color")?;
                options.color = match value.as_str() {
                    "auto" => ColorMode::Auto,
                    "256" | "indexed" => ColorMode::Indexed,
                    "truecolor" | "24bit" | "24-bit" => ColorMode::Truecolor,
                    _ => return Err("--color 只支持 auto / 256 / truecolor".to_string()),
                };
            }
            "--bench" => options.bench = true,
            "--lean" => options.lean = true,
            "--trace" => options.trace = true,
            "--normal-priority" => options.low_priority = false,
            "-i" | "--interval" => {
                let value = take_value("--interval")?;
                let interval: f64 = value
                    .parse()
                    .map_err(|_| format!("间隔必须是数字，收到：{value}"))?;
                if !(0.1..=60.0).contains(&interval) {
                    return Err("间隔范围 0.1 – 60 秒".to_string());
                }
                options.interval = interval;
            }
            "-w" | "--watch" => {
                let value = take_value("--watch")?;
                let seconds: f64 = value
                    .parse()
                    .map_err(|_| format!("--watch 需要秒数，收到：{value}"))?;
                if !(0.1..=3600.0).contains(&seconds) {
                    return Err("--watch 范围 0.1 – 3600 秒".to_string());
                }
                options.watch = Some(seconds);
            }
            "--log" => options.log = Some(PathBuf::from(take_value("--log")?)),
            "--fan-file" => options.fan_file = Some(PathBuf::from(take_value("--fan-file")?)),
            "--theme" => {
                let value = take_value("--theme")?;
                match value.as_str() {
                    "dark" | "light" => options.theme = value,
                    _ => return Err("--theme 只支持 dark 或 light".to_string()),
                }
            }
            "--width" => {
                options.width = Some(
                    take_value("--width")?
                        .parse()
                        .map_err(|_| "--width 需要数字".to_string())?,
                );
            }
            "--height" => {
                options.height = Some(
                    take_value("--height")?
                        .parse()
                        .map_err(|_| "--height 需要数字".to_string())?,
                );
            }
            "--frames" => {
                options.frames = take_value("--frames")?
                    .parse()
                    .map_err(|_| "--frames 需要数字".to_string())?;
            }
            other => return Err(format!("未知参数：{other}（用 --help 查看用法）")),
        }
    }

    Ok(options)
}

pub const HELP: &str = "\
monitor — 极低开销的 Windows 资源监控 (CPU / 内存 / 磁盘 / 网络 / 温度 / 风扇 / 电池)

用法:
  monitor [选项]

常用:
  monitor                 打开监控界面 (每 1 秒刷新)
  monitor -i 0.5          刷新间隔 0.5 秒 (可 0.1 – 60)
  monitor --once          输出一次快照后退出
  monitor --json          输出一次 JSON (给脚本/状态栏用)
  monitor --mini          输出一行精简状态
  monitor -w 5            无界面持续输出，每 5 秒一行
  monitor --log perf.csv  同时把每次采样追加到 CSV

选项:
  -i, --interval <秒>     界面刷新间隔，默认 1.0
  -o, --once              打印一次快照（终端宽度自适应）
      --json              打印一次 JSON（隐含 --once）
      --mini              打印一行精简状态（隐含 --once）
  -w, --watch <秒>        无界面模式，按秒数持续打印
      --log <文件>        追加 CSV 记录（表头自动写入）
      --fan-file <文件>   从文件读取风扇转速，每行 “名称,转速”
      --theme <dark|light> 配色，默认 dark
      --fancy             美化 TUI：24-bit 卡片仪表盘（需要至少 100×30 的窗口）
      --color <模式>      颜色深度：auto / 256 / truecolor（默认 auto）
      --lean              极简模式：不读温度和磁盘速率（不初始化性能计数器，内存约 1.9 MB）
      --normal-priority   不降低进程优先级（默认已设为低于正常）
      --demo              无终端渲染若干帧，便于检查布局（可加 --fancy）
      --bench             测量每个采集器的单次耗时（诊断用）
      --trace             配合 -w 输出每次采样耗时到标准错误（诊断用）
      --width/--height    配合 --demo 指定尺寸（默认 110x30）
      --frames <n>        配合 --demo 指定帧数（默认 3）
  -h, --help              显示帮助
  -V, --version           显示版本

界面快捷键:
  q 退出    p 暂停/继续    +/- 调速    c 核心网格/热力条    v 视图    t 主题    r 清空历史
";

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<Options, String> {
        parse(args.iter().map(|value| value.to_string()))
    }

    #[test]
    fn defaults() {
        let options = parse_args(&[]).unwrap();
        assert_eq!(options.interval, 1.0);
        assert!(options.low_priority);
        assert!(!options.once);
    }

    #[test]
    fn interval_accepts_both_forms() {
        assert_eq!(parse_args(&["-i", "0.5"]).unwrap().interval, 0.5);
        assert_eq!(parse_args(&["--interval=2"]).unwrap().interval, 2.0);
    }

    #[test]
    fn json_implies_once() {
        let options = parse_args(&["--json"]).unwrap();
        assert!(options.json && options.once);
    }

    #[test]
    fn rejects_bad_values() {
        assert!(parse_args(&["--interval", "abc"]).is_err());
        assert!(parse_args(&["--interval", "0"]).is_err());
        assert!(parse_args(&["--theme", "neon"]).is_err());
        assert!(parse_args(&["--color", "pink"]).is_err());
        assert!(parse_args(&["--nope"]).is_err());
    }

    #[test]
    fn fancy_and_colour_modes_parse() {
        let options = parse_args(&["--fancy"]).unwrap();
        assert!(options.fancy);
        assert_eq!(options.color, ColorMode::Auto);

        assert_eq!(parse_args(&["--color", "256"]).unwrap().color, ColorMode::Indexed);
        assert_eq!(
            parse_args(&["--color=truecolor"]).unwrap().color,
            ColorMode::Truecolor
        );
    }

    #[test]
    fn paths_are_kept() {
        let options = parse_args(&["--log", "out.csv", "--fan-file", "fans.txt"]).unwrap();
        assert_eq!(options.log.unwrap().to_string_lossy(), "out.csv");
        assert_eq!(options.fan_file.unwrap().to_string_lossy(), "fans.txt");
    }
}

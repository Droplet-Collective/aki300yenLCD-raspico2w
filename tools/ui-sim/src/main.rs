//! ticker の画面シミュレータ (docs/ui-sim.md)
//!
//! 本体の `src/ui/` (描画) と `src/font/` (東雲フォント) を `#[path]` で取り込み、ファームウェアと
//! 同じコードで 400×96 の RGB565 画面を描く。LCD が受け取る値 (RGB565 → RGB666) を 8 bit に
//! 引き延ばして PNG / GIF に書くので、画素単位で実機の画面と同じになる。
//!
//! ```text
//! ui-sim [--scenario FILE] [--out DIR] [--layout glass|dock|classic] [--bg FILE.BMP] [--name NAME]
//!     → NAME.png (1 倍)、NAME@3x.png (3 倍、最近傍)、NAME.gif (流れる文字 + スライドの切り替え)
//! ui-sim sheet [--scenario FILE] [--out DIR] --bg A.BMP --bg B.BMP ...
//!     → sheet.png (レイアウト 3 種 × 背景の比較表、2 倍)
//! ui-sim samples [--out samples/]
//!     → 見本の背景 BMP (手続き生成、著作権の無い画像) を作り直す
//! ```

#[path = "../../../src/font/mod.rs"]
#[allow(dead_code)]
mod font;
#[path = "../../../src/ui/mod.rs"]
#[allow(dead_code)]
mod ui;

mod output;
mod samples;
mod scenario;

use std::path::{Path, PathBuf};

use scenario::Scenario;
use ui::screen::Layout;

struct Args {
    command: String,
    scenario: Option<PathBuf>,
    out: PathBuf,
    layout: Option<Layout>,
    bgs: Vec<PathBuf>,
    name: Option<String>,
    no_gif: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        command: "render".into(),
        scenario: None,
        out: PathBuf::from("out"),
        layout: None,
        bgs: Vec::new(),
        name: None,
        no_gif: false,
    };
    let mut it = std::env::args().skip(1).peekable();
    if let Some(first) = it.peek()
        && !first.starts_with('-')
    {
        args.command = it.next().unwrap();
    }
    while let Some(arg) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--scenario" => args.scenario = Some(value()?.into()),
            "--out" => args.out = value()?.into(),
            "--layout" => {
                let v = value()?;
                args.layout = Some(Layout::parse(&v).ok_or_else(|| format!("unknown layout {v}"))?);
            }
            "--bg" => args.bgs.push(value()?.into()),
            "--name" => args.name = Some(value()?),
            "--no-gif" => args.no_gif = true,
            "-h" | "--help" => {
                println!("{}", include_str!("usage.txt"));
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other} (--help)")),
        }
    }
    Ok(args)
}

fn load_scenario(path: Option<&Path>) -> Result<(Scenario, PathBuf), String> {
    match path {
        Some(p) => {
            let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
            let s: Scenario = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))?;
            Ok((s, p.parent().unwrap_or(Path::new(".")).to_path_buf()))
        }
        None => Ok((Scenario::default(), PathBuf::from("."))),
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("ui-sim: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    match args.command.as_str() {
        "samples" => {
            let dir = if args.out == Path::new("out") { PathBuf::from("samples") } else { args.out.clone() };
            samples::write_all(&dir)
        }
        "render" => {
            let (mut sc, base) = load_scenario(args.scenario.as_deref())?;
            if let Some(l) = args.layout {
                sc.layout = l.name().into();
            }
            if let Some(bg) = args.bgs.first() {
                sc.background = Some(bg.display().to_string());
            }
            if let Some(bg) = args.bgs.get(1) {
                sc.next_background = Some(bg.display().to_string());
            }
            let name = args.name.clone().unwrap_or_else(|| {
                args.scenario
                    .as_deref()
                    .and_then(|p| p.file_stem())
                    .map_or("screen".into(), |s| s.to_string_lossy().into_owned())
            });
            std::fs::create_dir_all(&args.out).map_err(|e| e.to_string())?;
            output::render_scenario(&sc, &base, &args.out, &name, !args.no_gif)
        }
        "sheet" => {
            let (sc, base) = load_scenario(args.scenario.as_deref())?;
            std::fs::create_dir_all(&args.out).map_err(|e| e.to_string())?;
            let name = args.name.clone().unwrap_or_else(|| "sheet".into());
            output::render_sheet(&sc, &base, &args.bgs, &args.out, &name)
        }
        other => Err(format!("unknown command {other} (render / sheet / samples)")),
    }
}

#[cfg(test)]
mod tests;

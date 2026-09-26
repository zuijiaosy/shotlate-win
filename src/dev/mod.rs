//! Command-line developer tools. They run before the app starts and must come first on the command line:
//! `--ui-demo [bg.png] out/ [--scale 2]`, `--ocr image.png [--models dir]`,
//! `--translate-image in.png out.png [--scale 2]`, `--check [name] [out-dir]`.

pub mod check;
pub mod demo;

use std::path::{Path, PathBuf};

/// Handles developer flags; returns Some(exit code) when one ran.
pub fn run_if_requested() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let first = args.first()?.as_str();
    let scale = args.iter().position(|a| a == "--scale").and_then(|i| args.get(i + 1)).and_then(|s| s.parse::<f32>().ok()).unwrap_or(1.0);
    let positional: Vec<&String> = args[1..].iter().filter(|a| !a.starts_with("--")).filter(|a| a.parse::<f32>().is_err()).collect();
    match first {
        "--ui-demo" => {
            let (bg, out) = match positional.as_slice() {
                [bg, out, ..] => (Some(Path::new(bg.as_str())), PathBuf::from(out.as_str())),
                [out] => (None, PathBuf::from(out.as_str())),
                [] => (None, PathBuf::from("demo-out")),
            };
            Some(if demo::run(bg, &out, scale) { 0 } else { 1 })
        }
        "--check" => {
            let name = positional.first().map_or("all", |s| s.as_str());
            let out = positional.get(1).map_or_else(|| std::env::temp_dir().join("shotlate-check"), |p| PathBuf::from(p.as_str()));
            Some(check::run(name, &out))
        }
        #[cfg(windows)]
        "--e2e" => {
            let out = positional.first().map_or_else(|| std::env::temp_dir().join("shotlate-e2e"), |p| PathBuf::from(p.as_str()));
            Some(crate::win::e2e::run(&out))
        }
        "--bench-render" => {
            demo::bench();
            Some(0)
        }
        "--ocr" => {
            let Some(image) = positional.first() else {
                eprintln!("usage: --ocr image.png [--models dir]");
                return Some(2);
            };
            Some(ocr_cli(Path::new(image.as_str()), &models_dir(&args)))
        }
        _ => None,
    }
}

pub fn models_dir(args: &[String]) -> PathBuf {
    if let Some(i) = args.iter().position(|a| a == "--models") {
        if let Some(d) = args.get(i + 1) {
            return PathBuf::from(d);
        }
    }
    if let Ok(d) = std::env::var("SHOTLATE_MODELS") {
        return PathBuf::from(d);
    }
    crate::app_paths::models_dir()
}

fn ocr_cli(image: &Path, models: &Path) -> i32 {
    let Some(img) = demo::load_png(image) else {
        eprintln!("cannot read {}", image.display());
        return 1;
    };
    let t = std::time::Instant::now();
    let engine = match crate::ocr::OcrEngine::load(models) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    println!("load {:?}", t.elapsed());
    let t = std::time::Instant::now();
    match engine.recognize(&img) {
        Ok(lines) => {
            println!("recognize {:?}", t.elapsed());
            for l in lines {
                println!("{:>6.2}  {}", l.score, l.text);
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

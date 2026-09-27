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
        "--ocr-memory" => {
            let Some(image) = positional.first() else {
                eprintln!("usage: --ocr-memory image.png [--models dir]");
                return Some(2);
            };
            Some(ocr_memory(Path::new(image.as_str()), &models_dir(&args)))
        }
        _ => None,
    }
}

/// (working set, private bytes) of this process in MB; zeros where not measured.
pub fn process_memory_mb() -> (f64, f64) {
    #[cfg(windows)]
    {
        use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
        use windows::Win32::System::Threading::GetCurrentProcess;
        let mut c = PROCESS_MEMORY_COUNTERS_EX { cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32, ..Default::default() };
        let ok = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut c as *mut _ as *mut PROCESS_MEMORY_COUNTERS, c.cb) }.is_ok();
        if ok {
            return (c.WorkingSetSize as f64 / 1048576.0, c.PrivateUsage as f64 / 1048576.0);
        }
    }
    (0.0, 0.0)
}

/// `--ocr-memory image.png`: memory and time of each stage of the OCR engine's life, to size the
/// idle-release trade-off (see AGENTS.md).
fn ocr_memory(image: &Path, models: &Path) -> i32 {
    let Some(img) = demo::load_png(image) else {
        eprintln!("cannot read {}", image.display());
        return 1;
    };
    let report = |stage: &str, t: std::time::Duration| {
        let (ws, private) = process_memory_mb();
        println!("{stage:<28} {:>8.0} ms   working set {ws:>6.1} MB   private {private:>6.1} MB", t.as_secs_f64() * 1000.0);
    };
    report("start", Default::default());
    for round in 1..=2 {
        let t = std::time::Instant::now();
        let engine = match crate::ocr::OcrEngine::load(models) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("{e}");
                return 1;
            }
        };
        report(&format!("[{round}] load"), t.elapsed());
        if round == 1 {
            let t = std::time::Instant::now();
            let _ = engine.prepare_det(1920, 1088);
            report("[1] det plan 1920x1088", t.elapsed());
            for w in [320, 640, 960] {
                let t = std::time::Instant::now();
                let _ = engine.prepare_rec(w);
                report(&format!("[1] rec plan {w}"), t.elapsed());
            }
            let t = std::time::Instant::now();
            engine.warm_up();
            report("[1] warm up (runs them)", t.elapsed());
        }
        let t = std::time::Instant::now();
        let lines = engine.recognize(&img).map(|l| l.len()).unwrap_or(0);
        report(&format!("[{round}] recognize ({lines} lines)"), t.elapsed());
        let t = std::time::Instant::now();
        let _ = engine.recognize(&img);
        report(&format!("[{round}] recognize again"), t.elapsed());
        drop(engine);
        report(&format!("[{round}] dropped"), Default::default());
    }
    0
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

//! The OCR model files: where they come from, how to check them and how to download them.
//! Models are not bundled with the installer (see AGENTS.md); they are fetched on first use.

use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};

pub struct ModelFile {
    pub name: &'static str,
    pub size: u64,
    pub sha256: &'static str,
    /// Primary (ModelScope) first, then our own GitHub Release copy.
    pub urls: [&'static str; 2],
}

pub const DET_MODEL: &str = "PP-OCRv6_det_tiny.onnx";
pub const REC_MODEL: &str = "PP-OCRv6_rec_small.onnx";

pub const FILES: [ModelFile; 2] = [
    ModelFile {
        name: DET_MODEL,
        size: 1_829_618,
        sha256: "f42c0fbd294d95eac1a550e131b277dac97462c8025fa4b6c3cec1b7894bd3d5",
        urls: [
            "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/onnx/PP-OCRv6/det/PP-OCRv6_det_tiny.onnx",
            "https://github.com/zuijiaosy/shotlate-win/releases/download/models-v1/PP-OCRv6_det_tiny.onnx",
        ],
    },
    ModelFile {
        name: REC_MODEL,
        size: 21_234_383,
        sha256: "6f327246b50388f3c176ae304bd95767ea6dc0c9ae92153ef8cbe210b3c14884",
        urls: [
            "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/onnx/PP-OCRv6/rec/PP-OCRv6_rec_small.onnx",
            "https://github.com/zuijiaosy/shotlate-win/releases/download/models-v1/PP-OCRv6_rec_small.onnx",
        ],
    },
];

pub const TOTAL_BYTES: u64 = FILES[0].size + FILES[1].size;

/// Both files present with the expected size. Hashes are checked once, when downloading;
/// hashing 23 MB on every launch would be wasted work.
pub fn is_ready(dir: &Path) -> bool {
    FILES.iter().all(|f| file_len(&dir.join(f.name)) == Some(f.size))
}

/// Full check including SHA-256; the error names the first bad file.
pub fn verify(dir: &Path) -> Result<(), String> {
    for f in &FILES {
        verify_file(&dir.join(f.name), f)?;
    }
    Ok(())
}

fn file_len(path: &Path) -> Option<u64> {
    fs::metadata(path).ok().filter(|m| m.is_file()).map(|m| m.len())
}

fn verify_file(path: &Path, f: &ModelFile) -> Result<(), String> {
    match file_len(path) {
        None => return Err(format!("{} 不存在", f.name)),
        Some(len) if len != f.size => return Err(format!("{} 大小不对（{len} 字节，应为 {}）", f.name, f.size)),
        _ => {}
    }
    let hash = sha256_file(path).map_err(|e| format!("{} 读取失败：{e}", f.name))?;
    if hash != f.sha256 {
        return Err(format!("{} 校验失败", f.name));
    }
    Ok(())
}

pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug)]
pub enum DownloadError {
    /// Every URL failed; `host` is the last one tried, `detail` its error.
    Network { host: String, detail: String },
    HashMismatch { name: String },
    Cancelled,
    Disk(String),
}

impl fmt::Display for DownloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DownloadError::Network { host, detail } => write!(f, "下载失败，无法连接 {host}：{detail}"),
            DownloadError::HashMismatch { name } => write!(f, "下载的文件 {name} 校验失败，请重试"),
            DownloadError::Cancelled => write!(f, "下载已取消"),
            DownloadError::Disk(detail) => write!(f, "无法写入磁盘：{detail}"),
        }
    }
}

impl std::error::Error for DownloadError {}

/// Downloads the missing files into `dir`. `progress(done, total)` counts bytes over all files,
/// already present ones included, so a resumed download starts part-way.
pub fn download(dir: &Path, progress: &mut dyn FnMut(u64, u64), cancel: &AtomicBool) -> Result<(), DownloadError> {
    fs::create_dir_all(dir).map_err(|e| DownloadError::Disk(e.to_string()))?;
    let agent = agent();
    let mut done = 0u64;
    for f in &FILES {
        let target = dir.join(f.name);
        if file_len(&target) == Some(f.size) {
            done += f.size;
            progress(done, TOTAL_BYTES);
            continue;
        }
        let mut last_error = None;
        for url in f.urls {
            if cancel.load(Ordering::Relaxed) {
                return Err(DownloadError::Cancelled);
            }
            match fetch_one(&agent, url, f, &target, done, progress, cancel) {
                Ok(()) => {
                    last_error = None;
                    break;
                }
                // Only network trouble is worth another mirror; the rest would fail there too.
                Err(e @ DownloadError::Network { .. }) | Err(e @ DownloadError::HashMismatch { .. }) => last_error = Some(e),
                Err(e) => return Err(e),
            }
        }
        if let Some(e) = last_error {
            return Err(e);
        }
        done += f.size;
        progress(done, TOTAL_BYTES);
    }
    Ok(())
}

fn agent() -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        // ureq 3 has no per-read timeout; this bounds a stalled body instead (21 MB at ~40 KB/s).
        .timeout_recv_body(Some(Duration::from_secs(600)))
        .user_agent(concat!("Shotlate/", env!("CARGO_PKG_VERSION")))
        .tls_config(crate::kit::http::tls());
    config.build().new_agent()
}

fn host_of(url: &str) -> String {
    url.split("://").nth(1).and_then(|rest| rest.split('/').next()).unwrap_or(url).to_string()
}

fn part_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".part");
    target.with_file_name(name)
}

fn fetch_one(
    agent: &ureq::Agent,
    url: &str,
    f: &ModelFile,
    target: &Path,
    done_before: u64,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &AtomicBool,
) -> Result<(), DownloadError> {
    let part = part_path(target);
    let result = stream_to(agent, url, f, &part, done_before, progress, cancel).and_then(|()| {
        verify_file(&part, f).map_err(|_| DownloadError::HashMismatch { name: f.name.to_string() })?;
        fs::rename(&part, target).map_err(|e| DownloadError::Disk(e.to_string()))
    });
    if result.is_err() {
        let _ = fs::remove_file(&part);
    }
    result
}

fn stream_to(
    agent: &ureq::Agent,
    url: &str,
    f: &ModelFile,
    part: &Path,
    done_before: u64,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &AtomicBool,
) -> Result<(), DownloadError> {
    let network = |detail: String| DownloadError::Network { host: host_of(url), detail };
    let mut response = agent.get(url).call().map_err(|e| network(e.to_string()))?;
    let mut out = File::create(part).map_err(|e| DownloadError::Disk(e.to_string()))?;
    // Streams: the body never sits in memory as a whole.
    let mut reader = response.body_mut().as_reader();
    let mut buf = vec![0u8; 64 * 1024];
    let mut written = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(DownloadError::Cancelled);
        }
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(network(e.to_string())),
        };
        written += n as u64;
        // A wrong or hijacked URL (an HTML error page, say) must not fill the disk.
        if written > f.size {
            return Err(DownloadError::HashMismatch { name: f.name.to_string() });
        }
        out.write_all(&buf[..n]).map_err(|e| DownloadError::Disk(e.to_string()))?;
        progress(done_before + written, TOTAL_BYTES);
    }
    out.sync_all().map_err(|e| DownloadError::Disk(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("shotlate-models-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn hashes_files() {
        let dir = temp_dir("hash");
        let path = dir.join("abc.bin");
        fs::write(&path, b"abc").expect("write");
        assert_eq!(sha256_file(&path).expect("hash"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_rejects_missing_wrong_size_and_wrong_hash() {
        let dir = temp_dir("verify");
        assert!(!is_ready(&dir));
        assert!(verify(&dir).is_err());
        // Right sizes, wrong content: is_ready trusts sizes, verify does not.
        for f in &FILES {
            let file = File::create(dir.join(f.name)).expect("create");
            file.set_len(f.size).expect("set_len");
        }
        assert!(is_ready(&dir));
        let err = verify(&dir).expect_err("zeros must not match");
        assert!(err.contains(DET_MODEL), "{err}");
        let file = File::create(dir.join(DET_MODEL)).expect("create");
        file.set_len(10).expect("set_len");
        assert!(!is_ready(&dir));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn part_names_and_hosts() {
        assert_eq!(part_path(Path::new("/m/a.onnx")), PathBuf::from("/m/a.onnx.part"));
        assert_eq!(host_of(FILES[0].urls[0]), "www.modelscope.cn");
        assert_eq!(host_of(FILES[1].urls[1]), "github.com");
        assert_eq!(TOTAL_BYTES, 23_064_001);
    }

    #[test]
    fn cancelled_before_start() {
        let dir = temp_dir("cancel");
        let cancel = AtomicBool::new(true);
        let r = download(&dir, &mut |_, _| {}, &cancel);
        assert!(matches!(r, Err(DownloadError::Cancelled)));
        assert!(fs::read_dir(&dir).expect("read_dir").next().is_none(), "no partial files left");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[ignore = "downloads ~23 MB from the network"]
    fn real_download() {
        let dir = temp_dir("real");
        let cancel = AtomicBool::new(false);
        let mut last = (0, 0);
        download(&dir, &mut |d, t| last = (d, t), &cancel).expect("download");
        assert_eq!(last, (TOTAL_BYTES, TOTAL_BYTES));
        verify(&dir).expect("verify");
        let _ = fs::remove_dir_all(&dir);
    }
}

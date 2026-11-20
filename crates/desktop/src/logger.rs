//! Plain file logger: %LOCALAPPDATA%\Deskboxes\logs\deskboxes.log (one previous file kept),
//! milliseconds since process start on every line.

use std::io::Write;
use std::sync::Mutex;
use std::time::Instant;

struct L {
    f: Mutex<std::fs::File>,
    t0: Instant,
}

impl log::Log for L {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, r: &log::Record) {
        let line = format!("{:>9.1} {:<5} {}\n", self.t0.elapsed().as_secs_f64() * 1000.0, r.level(), r.args());
        if let Ok(mut f) = self.f.lock() {
            let _ = f.write_all(line.as_bytes());
        }
    }
    fn flush(&self) {
        if let Ok(f) = self.f.lock() {
            let _ = f.sync_data();
        }
    }
}

pub fn init(dir: &std::path::Path, t0: Instant) {
    let _ = std::fs::create_dir_all(dir);
    let p = dir.join("deskboxes.log");
    if std::fs::metadata(&p).map(|m| m.len() > 1 << 20).unwrap_or(false) {
        let _ = std::fs::rename(&p, dir.join("deskboxes.1.log"));
    }
    if let Ok(f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = log::set_logger(Box::leak(Box::new(L { f: Mutex::new(f), t0 })));
        log::set_max_level(log::LevelFilter::Info);
    }
}

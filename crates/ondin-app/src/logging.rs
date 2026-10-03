//! **The app's log file** (§15 D954): `ondin.log`, beside `prefs.json` under
//! `dirs::config_dir()/ondin`, rotated once to `ondin.log.1` past a megabyte.
//!
//! **Why a file and not stderr.** A release build is a GUI-subsystem binary on
//! Windows (§15 D953): it has no console, so anything written to stderr is gone.
//! The updater is the reason this exists — a background check that fails is
//! deliberately invisible in the window (`update`), so the log is the only place
//! the question *"why didn't it update?"* can be answered from. Velopack logs
//! through the `log` facade under its own `velopack` target, and that target is
//! admitted on purpose: Schemaic lost a real field failure to a filter that left
//! it out.
//!
//! **What is written**: `warn` and above from everything, `info` and above from
//! the app's own `ondin` target and Velopack's. Panics too, through a hook that
//! records the message and location before the default hook runs.
//!
//! **Not opened by a headless app or a test** — `init` is called from `main`
//! alone — so a test can log through the facade without touching a real file.

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Mutex;

/// Past this many bytes the log is rotated once, at startup.
const ROTATE_AT: u64 = 1024 * 1024;

/// Where the log lives, or `None` with no config directory to put it in.
pub(crate) fn path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("ondin").join("ondin.log"))
}

/// Whether a record from `target` at `level` is written — the whole filter.
fn admits(target: &str, level: log::Level) -> bool {
    let ours = target == "ondin" || target.starts_with("ondin::") || target.starts_with("velopack");
    level
        <= if ours {
            log::Level::Info
        } else {
            log::Level::Warn
        }
}

struct FileLog {
    file: Mutex<std::fs::File>,
}

impl log::Log for FileLog {
    fn enabled(&self, meta: &log::Metadata<'_>) -> bool {
        admits(meta.target(), meta.level())
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        if let Ok(mut f) = self.file.lock() {
            // A failed write has nowhere better to go; the app carries on.
            let _ = writeln!(
                f,
                "{secs} {} {}: {}",
                record.level(),
                record.target(),
                record.args()
            );
        }
    }

    fn flush(&self) {
        if let Ok(mut f) = self.file.lock() {
            let _ = f.flush();
        }
    }
}

/// Open the log and install it, with the panic hook. Called once, from `main`,
/// before anything that might log. A log that cannot be opened is not a reason
/// to stop the app: it runs on, unlogged.
pub(crate) fn init() {
    let Some(path) = path() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > ROTATE_AT) {
        let _ = std::fs::rename(&path, path.with_extension("log.1"));
    }
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    if log::set_boxed_logger(Box::new(FileLog {
        file: Mutex::new(file),
    }))
    .is_err()
    {
        return;
    }
    log::set_max_level(log::LevelFilter::Info);

    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!(target: "ondin", "panic: {info}");
        log::logger().flush();
        default(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Velopack's own records are written, and so are the app's**, at
    /// `info`; everything else only from `warn` — the filter Schemaic learned
    /// the hard way (a field failure logged under `velopack` at `info` and
    /// dropped). Flip run: the `velopack` prefix dropped from `admits` fails on
    /// *"velopack's info"*, the predicted site.
    #[test]
    fn the_filter_admits_velopack_and_the_app_at_info() {
        use log::Level::{Debug, Info, Warn};
        assert!(admits("velopack", Info), "velopack's info");
        assert!(admits("velopack::manager", Info));
        assert!(admits("ondin", Info) && admits("ondin::update", Info));
        assert!(!admits("ondin", Debug), "not debug");
        assert!(!admits("wgpu_core", Info), "a dependency's info is noise");
        assert!(admits("wgpu_core", Warn), "and its warnings are not");
    }
}

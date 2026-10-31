//! The top bar's resource meter (Settings › Appearance › *Show resource meter*):
//! the app's share of every core, its memory, and how many canvas frames were
//! drawn in the last second.
//!
//! CPU and memory are read from the OS once a second while the meter shows, through
//! `windows-sys` and `libc` rather than a system-information crate. The frame count
//! is kept whether or not it shows: one timestamp per frame that drew the page, so
//! a frame that kept the canvas texture (a hover, a caret blink) is not counted.

use crate::theme::{self, icon};
use eframe::egui;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How often CPU and memory are read, and the window frames are counted over.
const PERIOD: Duration = Duration::from_secs(1);
/// A CPU share above this reads amber.
const CPU_HOT: f32 = 80.0;
/// A frame rate below this reads amber.
const FPS_LOW: u32 = 30;

/// The readouts' glyph size, value size, and the room between glyph and value and
/// between two readouts.
const GLYPH_PT: f32 = 15.0;
const VALUE_PT: f32 = 11.5;
const GLYPH_GAP: f32 = 5.0;
const READOUT_GAP: f32 = 14.0;
/// Each value's slot, wide enough for its longest figure so the bar never shifts
/// as the numbers change: `100%`, `1023 MB`, `240`.
const CPU_SLOT: f32 = 28.0;
const MEMORY_SLOT: f32 = 46.0;
const FPS_SLOT: f32 = 20.0;

/// The readings and the frame clock.
#[derive(Default)]
pub(crate) struct Meter {
    /// When CPU and memory were last read; `None` while the meter is off.
    read_at: Option<Instant>,
    /// The process's CPU time at `read_at`.
    cpu_time: Option<Duration>,
    /// The CPU share between the last two readings, percent of every core.
    cpu: Option<f32>,
    /// Bytes resident: the working set on Windows.
    memory: Option<u64>,
    /// When each canvas frame of the last [`PERIOD`] was drawn.
    frames: VecDeque<Instant>,
}

impl Meter {
    /// Count a canvas frame drawn at `now`.
    pub(crate) fn frame(&mut self, now: Instant) {
        while self
            .frames
            .front()
            .is_some_and(|t| now.duration_since(*t) >= PERIOD)
        {
            self.frames.pop_front();
        }
        self.frames.push_back(now);
    }

    /// How many frames the clock holds, for the canvas tests that pin what counts.
    #[cfg(test)]
    pub(crate) fn counted(&self) -> usize {
        self.frames.len()
    }

    /// Canvas frames drawn in the [`PERIOD`] before `now`, or `None` under two: one
    /// is a single change drawn, not a frame rate.
    fn fps(&self, now: Instant) -> Option<u32> {
        let n = self
            .frames
            .iter()
            .filter(|t| now.duration_since(**t) < PERIOD)
            .count() as u32;
        (n >= 2).then_some(n)
    }

    /// Read CPU and memory if a [`PERIOD`] has passed since the last reading.
    /// Returns how long until the next one is due.
    fn tick(&mut self, now: Instant) -> Duration {
        if let Some(left) = self
            .read_at
            .and_then(|at| PERIOD.checked_sub(now.duration_since(at)))
            .filter(|left| !left.is_zero())
        {
            return left;
        }
        let cpu_time = process_cpu_time();
        if let (Some(at), Some(was), Some(is)) = (self.read_at, self.cpu_time, cpu_time) {
            self.cpu = Some(cpu_share(
                is.saturating_sub(was),
                now.duration_since(at),
                cores(),
            ));
        }
        self.read_at = Some(now);
        self.cpu_time = cpu_time;
        self.memory = process_memory();
        PERIOD
    }

    /// Forget the readings, so a meter switched back on starts afresh rather than
    /// averaging over the time it was off.
    pub(crate) fn stop(&mut self) {
        if self.read_at.is_some() {
            *self = Self {
                frames: std::mem::take(&mut self.frames),
                ..Self::default()
            };
        }
    }

    /// The three readouts, for a `right_to_left` layout, so added frame rate first;
    /// and the repaint that keeps them current.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        let now = Instant::now();
        let next = self.tick(now);
        ui.ctx().request_repaint_after(next);
        // The layout's own spacing is already between two readouts.
        let gap = READOUT_GAP - ui.spacing().item_spacing.x;

        let fps = self.fps(now);
        readout(
            ui,
            icon::GAUGE,
            &fps.map_or_else(|| "—".to_string(), |n| n.to_string()),
            FPS_SLOT,
            fps.is_some_and(|n| n < FPS_LOW),
            fps.is_none(),
            &match fps {
                Some(n) => format!("Canvas: {n} frames in the last second"),
                None => "Canvas: nothing drawn in the last second".to_string(),
            },
        );
        ui.add_space(gap);
        readout(
            ui,
            icon::MEMORY,
            &self.memory.map_or_else(|| "—".to_string(), memory_label),
            MEMORY_SLOT,
            false,
            self.memory.is_none(),
            &match self.memory {
                Some(b) => format!("Memory: {:.1} MB in use", b as f64 / MIB),
                None => "Memory: not available".to_string(),
            },
        );
        ui.add_space(gap);
        readout(
            ui,
            icon::CPU,
            &self.cpu.map_or_else(|| "—".to_string(), cpu_label),
            CPU_SLOT,
            self.cpu.is_some_and(|c| c > CPU_HOT),
            self.cpu.is_none(),
            &match self.cpu {
                Some(c) => format!("CPU: {c:.1}% of all {} cores", cores()),
                None => "CPU: measuring".to_string(),
            },
        );
    }
}

/// One readout: a glyph, then the value in its slot; amber when `hot`, quieter
/// when there is no reading.
fn readout(
    ui: &mut egui::Ui,
    glyph: &str,
    value: &str,
    slot: f32,
    hot: bool,
    quiet: bool,
    tip: &str,
) {
    let galley = ui.painter().layout_no_wrap(
        value.to_string(),
        egui::FontId::proportional(VALUE_PT),
        egui::Color32::PLACEHOLDER,
    );
    let w = GLYPH_PT + GLYPH_GAP + slot.max(galley.size().x);
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(w, ui.available_height()), egui::Sense::hover());
    let (glyph_ink, value_ink) = match (hot, quiet) {
        (true, _) => (theme::color::warn(), theme::color::warn()),
        (false, true) => (theme::text::dim(), theme::text::faint()),
        (false, false) => (theme::text::dim(), theme::text::muted()),
    };
    let p = ui.painter();
    let cy = rect.center().y;
    p.text(
        egui::pos2(rect.left() + GLYPH_PT / 2.0, cy),
        egui::Align2::CENTER_CENTER,
        glyph,
        theme::icon_font(GLYPH_PT),
        glyph_ink,
    );
    p.galley(
        egui::pos2(
            rect.left() + GLYPH_PT + GLYPH_GAP,
            cy - galley.size().y / 2.0,
        ),
        galley,
        value_ink,
    );
    resp.on_hover_text(tip);
}

const MIB: f64 = 1024.0 * 1024.0;

/// `92%`: the share, whole.
fn cpu_label(share: f32) -> String {
    format!("{}%", share.round() as u32)
}

/// `412 MB` under a gigabyte, `2.4 GB` from one.
fn memory_label(bytes: u64) -> String {
    let mb = bytes as f64 / MIB;
    if mb < 1024.0 {
        format!("{} MB", mb.round() as u64)
    } else {
        format!("{:.1} GB", mb / 1024.0)
    }
}

/// The share of every core that `cpu` of process time over `wall` makes,
/// percent, as a task manager reports it.
fn cpu_share(cpu: Duration, wall: Duration, cores: usize) -> f32 {
    if wall.is_zero() || cores == 0 {
        return 0.0;
    }
    (cpu.as_secs_f64() / wall.as_secs_f64() / cores as f64 * 100.0).clamp(0.0, 100.0) as f32
}

fn cores() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

/// The process's CPU time so far, user and kernel.
#[cfg(windows)]
fn process_cpu_time() -> Option<Duration> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: the pseudo-handle needs no closing, and each pointer is a live local.
    let ok = unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    };
    // FILETIME counts 100 ns.
    let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
    (ok != 0).then(|| Duration::from_nanos((ticks(kernel) + ticks(user)) * 100))
}

/// The process's working set.
#[cfg(windows)]
fn process_memory() -> Option<u64> {
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    // SAFETY: all-zero is a valid PROCESS_MEMORY_COUNTERS, and `cb` names its size.
    let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    counters.cb = size;
    // SAFETY: the pointer is a live local of the size passed.
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, size) };
    (ok != 0).then_some(counters.WorkingSetSize as u64)
}

/// The process's CPU time so far, user and system.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn process_cpu_time() -> Option<Duration> {
    // SAFETY: all-zero is a valid rusage, filled by the call.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: the pointer is a live local.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
        return None;
    }
    let time = |t: libc::timeval| {
        Duration::from_secs(t.tv_sec as u64) + Duration::from_micros(t.tv_usec as u64)
    };
    Some(time(usage.ru_utime) + time(usage.ru_stime))
}

/// The process's resident pages, from `/proc/self/statm`'s second field.
#[cfg(target_os = "linux")]
fn process_memory() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    // SAFETY: a read of a constant.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    (page > 0).then(|| pages * page as u64)
}

/// The process's resident size.
#[cfg(target_os = "macos")]
fn process_memory() -> Option<u64> {
    let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
    // SAFETY: all-zero is a valid proc_taskinfo, filled by the call.
    let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
    // SAFETY: the pointer is a live local of the size passed.
    let got = unsafe {
        libc::proc_pidinfo(
            libc::getpid(),
            libc::PROC_PIDTASKINFO,
            0,
            (&mut info as *mut libc::proc_taskinfo).cast(),
            size,
        )
    };
    (got == size).then_some(info.pti_resident_size)
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn process_cpu_time() -> Option<Duration> {
    None
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn process_memory() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A share is of every core, and clamps.
    #[test]
    fn the_cpu_share_is_of_every_core() {
        let s = Duration::from_secs;
        assert_eq!(cpu_share(s(2), s(1), 8), 25.0, "two cores' worth of eight");
        assert_eq!(cpu_share(s(9), s(1), 8), 100.0, "clamped");
        assert_eq!(cpu_share(s(1), Duration::ZERO, 8), 0.0, "no time passed");
    }

    /// Memory reads in MB under a gigabyte and GB with one decimal from one.
    #[test]
    fn memory_reads_in_mb_then_gb() {
        assert_eq!(memory_label(412 * 1024 * 1024), "412 MB");
        assert_eq!(memory_label(1023 * 1024 * 1024), "1023 MB");
        assert_eq!(memory_label(2_576_980_378), "2.4 GB");
    }

    /// The frame rate counts the last second's frames, and reads nothing under two.
    /// flip: `fps` without its two-frame floor → fails at "one frame is no rate".
    #[test]
    fn the_frame_rate_counts_the_last_second() {
        let mut m = Meter::default();
        let t0 = Instant::now();
        m.frame(t0);
        assert_eq!(m.fps(t0), None, "one frame is no rate");
        for i in 1..60 {
            m.frame(t0 + Duration::from_millis(i * 16));
        }
        let end = t0 + Duration::from_millis(59 * 16);
        assert_eq!(m.fps(end), Some(60));
        assert_eq!(m.fps(end + Duration::from_secs(2)), None, "at rest");
        assert!(m.frames.len() <= 60, "the clock keeps one second");
    }

    /// The OS answers both readings here, and a second tick a period on yields a
    /// CPU share.
    #[test]
    fn the_platform_reports_cpu_and_memory() {
        let mut m = Meter::default();
        let t0 = Instant::now();
        m.tick(t0);
        assert!(m.memory.is_some_and(|b| b > 0), "memory: {:?}", m.memory);
        assert!(m.cpu.is_none(), "one reading is no share");
        assert_eq!(m.tick(t0), PERIOD, "not due again at once");
        m.tick(t0 + PERIOD);
        assert!(m.cpu.is_some(), "a share after a period");
    }
}

//! Processes: who owns a leftover, which processes belong to a session, and
//! starting and ending a harness's children.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The process that owns something outside its memory: a pid and that pid's
/// start time, since pids are reissued and a dead owner's pid can come to name
/// some live, unrelated process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Owner {
    pub pid: u32,
    pub started: u64,
}

impl Owner {
    /// This process, if its start time can be read.
    pub(crate) fn me() -> Option<Owner> {
        let pid = std::process::id();
        Some(Owner {
            pid,
            started: process_start(pid)?,
        })
    }

    /// Is this owner still the process on its pid?
    pub(crate) fn is_live(self) -> bool {
        process_start(self.pid) == Some(self.started)
    }
}

/// A running process's start time, in the platform's own units.
#[cfg(windows)]
pub(crate) fn process_start(pid: u32) -> Option<u64> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    let mut code = 0u32;
    // SAFETY: plain Win32 calls on a handle opened and closed here; every
    // pointer is a live local.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h == 0 {
            return None;
        }
        // An exited process stays openable while anyone holds a handle to it.
        let running = GetExitCodeProcess(h, &mut code) != 0 && code == STILL_ACTIVE as u32;
        let ok = GetProcessTimes(h, &mut created, &mut exited, &mut kernel, &mut user);
        CloseHandle(h);
        (running && ok != 0)
            .then(|| (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
    }
}

/// A running process's start time: field 22 of `/proc/<pid>/stat`, read past
/// the command name, which may itself hold spaces and parentheses.
#[cfg(target_os = "linux")]
pub(crate) fn process_start(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &stat[stat.rfind(')')? + 1..];
    // `rest` starts at field 3 (state); field 22 is the 20th from there.
    rest.split_whitespace().nth(19)?.parse().ok()
}

/// A running process's start time, in microseconds.
#[cfg(target_os = "macos")]
pub(crate) fn process_start(pid: u32) -> Option<u64> {
    let info = bsd_info(pid)?;
    Some(info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec)
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub(crate) fn process_start(_pid: u32) -> Option<u64> {
    None
}

#[cfg(target_os = "macos")]
fn bsd_info(pid: u32) -> Option<libc::proc_bsdinfo> {
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: all-zero is a valid proc_bsdinfo, filled by the call.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    // SAFETY: the pointer is a live local of the size passed.
    let got = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    (got == size).then_some(info)
}

/// A process's parent, for walking a harness's tree off Windows.
#[cfg(target_os = "linux")]
fn parent_pid(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(target_os = "macos")]
fn parent_pid(pid: u32) -> Option<u32> {
    Some(bsd_info(pid)?.pbi_ppid)
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn parent_pid(_pid: u32) -> Option<u32> {
    None
}

/// Which processes belong to one session, asked from any thread and answering
/// `false` once the session has ended.
pub(crate) struct Tracker {
    alive: AtomicBool,
    #[cfg(windows)]
    job: Option<Job>,
    /// Each live child's own job, nested in `job`, so one child's tree can be
    /// ended without ending another's still in its grace.
    #[cfg(windows)]
    children: Mutex<Vec<(u32, Job)>>,
    /// The session's children; their descendants are found by walking up.
    #[cfg(not(windows))]
    roots: Mutex<Vec<u32>>,
}

impl Tracker {
    pub(crate) fn new() -> Arc<Tracker> {
        Arc::new(Tracker {
            alive: AtomicBool::new(true),
            #[cfg(windows)]
            job: Job::new(true),
            #[cfg(windows)]
            children: Mutex::new(Vec::new()),
            #[cfg(not(windows))]
            roots: Mutex::new(Vec::new()),
        })
    }

    /// Does `pid` belong to this session's harness? On Windows its Job Object
    /// holds every process a child starts, however it was launched; elsewhere
    /// the parent chain is walked, which a process that left its tree escapes.
    pub(crate) fn owns(&self, pid: u32) -> bool {
        if !self.alive.load(Ordering::Acquire) {
            return false;
        }
        #[cfg(windows)]
        {
            self.job.as_ref().is_some_and(|j| j.contains(pid))
        }
        #[cfg(not(windows))]
        {
            let roots = self.roots.lock().map(|r| r.clone()).unwrap_or_default();
            let mut at = pid;
            for _ in 0..64 {
                if roots.contains(&at) {
                    return true;
                }
                match parent_pid(at) {
                    Some(p) if p > 1 && p != at => at = p,
                    _ => return false,
                }
            }
            false
        }
    }

    /// End every process of the session and stop answering for it.
    pub(crate) fn end(&self) {
        self.alive.store(false, Ordering::Release);
        #[cfg(windows)]
        if let Some(j) = &self.job {
            j.terminate();
        }
        #[cfg(not(windows))]
        if let Ok(mut roots) = self.roots.lock() {
            for pid in roots.drain(..) {
                kill_group(pid);
            }
        }
    }

    /// Start `cmd` as one of the session's processes, with no console window.
    /// On Windows it starts suspended and is resumed once it is in its jobs, so
    /// nothing it starts can escape before then, and `cmd.exe` behind a shim
    /// never looks a bare command up in its working folder.
    pub(crate) fn spawn(&self, cmd: &mut Command) -> std::io::Result<Child> {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            const CREATE_SUSPENDED: u32 = 0x0000_0004;
            let Some(job) = &self.job else {
                return Err(std::io::Error::other("no job object for the session"));
            };
            let Some(own) = Job::new(false) else {
                return Err(std::io::Error::last_os_error());
            };
            cmd.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED)
                .env("NoDefaultCurrentDirectoryInExePath", "1");
            let mut child = cmd.spawn()?;
            if let Err(e) = job.adopt(&child, &own) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
            if let Ok(mut c) = self.children.lock() {
                c.push((child.id(), own));
            }
            Ok(child)
        }
        #[cfg(not(windows))]
        {
            use std::os::unix::process::CommandExt;
            // Its own group, so the whole tree can be ended at once.
            cmd.process_group(0);
            let child = cmd.spawn()?;
            if let Ok(mut roots) = self.roots.lock() {
                roots.push(child.id());
            }
            Ok(child)
        }
    }

    /// End one child's tree (behind an npm shim the agent is a grandchild)
    /// and forget it as a root.
    pub(crate) fn kill(&self, child: &mut Child) {
        #[cfg(windows)]
        if let Ok(c) = self.children.lock()
            && let Some((_, own)) = c.iter().find(|(p, _)| *p == child.id())
        {
            own.terminate();
        }
        #[cfg(not(windows))]
        kill_group(child.id());
        let _ = child.kill();
        self.forget(child.id());
    }

    /// A child that has exited is no longer a root: its pid may be reissued.
    pub(crate) fn forget(&self, pid: u32) {
        #[cfg(windows)]
        if let Ok(mut c) = self.children.lock() {
            c.retain(|(p, _)| *p != pid);
        }
        #[cfg(not(windows))]
        if let Ok(mut roots) = self.roots.lock() {
            roots.retain(|p| *p != pid);
        }
    }

    /// How many children are still roots.
    #[cfg(test)]
    fn roots(&self) -> usize {
        #[cfg(windows)]
        let n = self.children.lock().map_or(0, |c| c.len());
        #[cfg(not(windows))]
        let n = self.roots.lock().map_or(0, |r| r.len());
        n
    }
}

/// Wait up to `grace` for `child` to exit, then kill its tree; its exit code.
/// Either way it is forgotten as a root, since its pid may be reissued.
pub(crate) fn wait_bounded(
    tracker: &Tracker,
    child: &std::sync::Mutex<Child>,
    grace: std::time::Duration,
) -> Option<i32> {
    let until = std::time::Instant::now() + grace;
    loop {
        {
            let mut c = child.lock().unwrap_or_else(|e| e.into_inner());
            let code = match c.try_wait() {
                Ok(Some(s)) => Some(s.code()),
                Ok(None) if std::time::Instant::now() >= until => {
                    tracker.kill(&mut c);
                    let _ = c.wait();
                    Some(None)
                }
                Ok(None) => None,
                Err(_) => Some(None),
            };
            if let Some(code) = code {
                tracker.forget(c.id());
                return code;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// What a helper run printed, once it exited.
pub(crate) struct Helper {
    pub status: std::process::ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Run a short helper (a `--help` probe, `agy mcp …`, `cursor-agent mcp
/// enable`) outside any session: in a private empty folder unless the caller
/// chose one, its tree ended after `timeout`, both pipes drained. `Ok(None)`
/// when it did not finish in time.
pub(crate) fn run_bounded(
    cmd: &mut Command,
    timeout: std::time::Duration,
) -> std::io::Result<Option<Helper>> {
    let scratch = match cmd.get_current_dir() {
        Some(_) => None,
        None => Some(Scratch::new()?),
    };
    if let Some(s) = &scratch {
        cmd.current_dir(s.path());
    }
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let tracker = Tracker::new();
    let mut child = tracker.spawn(cmd)?;
    let drain = |pipe: Option<Box<dyn std::io::Read + Send>>| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut buf);
            }
            let _ = tx.send(buf);
        });
        rx
    };
    let out = drain(child.stdout.take().map(|p| Box::new(p) as _));
    let err = drain(child.stderr.take().map(|p| Box::new(p) as _));
    let child = std::sync::Mutex::new(child);
    let started = std::time::Instant::now();
    let status = loop {
        let mut c = child.lock().unwrap_or_else(|e| e.into_inner());
        match c.try_wait()? {
            Some(s) => break Some(s),
            None if started.elapsed() >= timeout => break None,
            None => {}
        }
        drop(c);
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let Some(status) = status else {
        wait_bounded(&tracker, &child, std::time::Duration::ZERO);
        tracker.end();
        return Ok(None);
    };
    // A grandchild left holding a pipe would keep its reader waiting.
    tracker.end();
    let read = |rx: std::sync::mpsc::Receiver<Vec<u8>>| {
        rx.recv_timeout(std::time::Duration::from_secs(2))
            .unwrap_or_default()
    };
    Ok(Some(Helper {
        status,
        stdout: read(out),
        stderr: read(err),
    }))
}

/// A helper's working folder, so nothing beside the document Ondin was opened
/// from is taken for a program or a project file. On Windows a fresh, empty
/// folder in the person's own temp folder, removed on drop; elsewhere `/`,
/// since the shared `/tmp` is not the person's alone.
struct Scratch {
    path: PathBuf,
    owned: bool,
}

impl Scratch {
    fn new() -> std::io::Result<Scratch> {
        if cfg!(windows) {
            let root = std::env::temp_dir().join("ondin-ai-helper");
            let path = crate::run::make(&root).map_err(std::io::Error::other)?;
            return Ok(Scratch { path, owned: true });
        }
        Ok(Scratch {
            path: PathBuf::from("/"),
            owned: false,
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if self.owned {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

#[cfg(not(windows))]
fn kill_group(pid: u32) {
    // SAFETY: a signal to the process group this crate created for the child.
    unsafe {
        libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
    }
}

/// A Job Object that ends its processes when it is closed.
#[cfg(windows)]
struct Job(windows_sys::Win32::Foundation::HANDLE);

// SAFETY: a job handle is a kernel handle, usable from any thread.
#[cfg(windows)]
unsafe impl Send for Job {}
// SAFETY: as above; the calls made on it are thread-safe.
#[cfg(windows)]
unsafe impl Sync for Job {}

#[cfg(windows)]
impl Job {
    /// A new job; `kill_on_close` ends its processes when its handle closes.
    fn new(kill_on_close: bool) -> Option<Job> {
        use windows_sys::Win32::System::JobObjects::{
            CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };
        // SAFETY: plain Win32 calls on a handle created here and owned by `Job`.
        unsafe {
            let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if h == 0 {
                return None;
            }
            let job = Job(h);
            if !kill_on_close {
                return Some(job);
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let set = SetInformationJobObject(
                h,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            (set != 0).then_some(job)
        }
    }

    /// Put a suspended child in this job and then in `own`, which the second
    /// assignment nests inside this one, then resume its threads.
    fn adopt(&self, child: &Child, own: &Job) -> std::io::Result<()> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
        };
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
        use windows_sys::Win32::System::Threading::{
            OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
        };
        // SAFETY: Win32 calls on the child's handle (borrowed for the call) and
        // on handles opened and closed here; every pointer is a live local.
        unsafe {
            for job in [self.0, own.0] {
                if AssignProcessToJobObject(job, child.as_raw_handle() as _) == 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snap == INVALID_HANDLE_VALUE {
                return Err(std::io::Error::last_os_error());
            }
            let mut entry: THREADENTRY32 = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
            let mut resumed = 0;
            let mut more = Thread32First(snap, &mut entry) != 0;
            while more {
                if entry.th32OwnerProcessID == child.id() {
                    let t = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                    if t != 0 {
                        if ResumeThread(t) != u32::MAX {
                            resumed += 1;
                        }
                        CloseHandle(t);
                    }
                }
                more = Thread32Next(snap, &mut entry) != 0;
            }
            CloseHandle(snap);
            match resumed {
                0 => Err(std::io::Error::other(
                    "could not resume the agent's process",
                )),
                _ => Ok(()),
            }
        }
    }

    fn contains(&self, pid: u32) -> bool {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::IsProcessInJob;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        // SAFETY: a handle opened and closed here; the out-pointer is a local.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h == 0 {
                return false;
            }
            let mut inside = 0;
            let ok = IsProcessInJob(h, self.0, &mut inside);
            CloseHandle(h);
            ok != 0 && inside != 0
        }
    }

    fn terminate(&self) {
        // SAFETY: the job handle is owned by `self` and still open.
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0, 1);
        }
    }
}

#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: the handle came from `CreateJobObjectW` and is closed once.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This process has a readable start time, and a claim with another start
    /// time on the same pid is a reissued pid, not a live owner.
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_reissued_pid_is_not_a_live_owner() {
        let me = Owner::me().expect("this process's start time");
        assert!(me.is_live());
        let reissued = Owner {
            started: me.started.wrapping_add(1),
            ..me
        };
        assert!(!reissued.is_live());
    }

    /// A spawned child belongs to the session, this process does not, and
    /// nothing does once the session has ended.
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_child_belongs_to_its_session_until_it_ends() {
        let tracker = Tracker::new();
        let mut cmd = match cfg!(windows) {
            true => {
                let mut c = Command::new("cmd");
                c.args(["/C", "ping -n 30 127.0.0.1 >NUL"]);
                c
            }
            false => {
                let mut c = Command::new("sleep");
                c.arg("30");
                c
            }
        };
        cmd.stdout(std::process::Stdio::null());
        let mut child = tracker.spawn(&mut cmd).expect("spawn");
        assert!(tracker.owns(child.id()));
        assert!(!tracker.owns(std::process::id()));
        tracker.end();
        assert!(!tracker.owns(child.id()));
        let _ = child.kill();
        let _ = child.wait();
    }

    /// Killing a child started through a `.cmd` ends the program the batch
    /// file runs too, so its stdout closes at once rather than when it finishes.
    /// flip: `Tracker::kill` without `own.terminate()` → fails at `stdout closes`.
    #[cfg(windows)]
    #[test]
    fn killing_a_shim_ends_the_program_behind_it() {
        use std::io::Read;
        let dir = std::env::temp_dir().join(format!("ondin-ai-kill-{}", crate::run::random_tag()));
        std::fs::create_dir_all(&dir).unwrap();
        let shim = dir.join("agent.cmd");
        std::fs::write(&shim, "@ping -n 30 127.0.0.1\r\n").unwrap();
        let tracker = Tracker::new();
        let mut cmd = Command::new(&shim);
        cmd.stdout(std::process::Stdio::piped())
            .stdin(std::process::Stdio::null());
        let mut child = tracker.spawn(&mut cmd).expect("spawn");
        let mut out = child.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = out.read_to_end(&mut buf);
            let _ = tx.send(buf);
        });
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let killed = std::time::Instant::now();
        tracker.kill(&mut child);
        let _ = child.wait();
        let buf = rx
            .recv_timeout(std::time::Duration::from_secs(20))
            .expect("stdout closes");
        assert!(!buf.is_empty(), "ping was running when it was killed");
        assert!(
            killed.elapsed() < std::time::Duration::from_secs(3),
            "stdout closed {:?} after the kill",
            killed.elapsed()
        );
        assert_eq!(tracker.roots(), 0);
        tracker.end();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A child reaped by `wait_bounded` is no longer a root, however it ended.
    /// flip: `wait_bounded` without its `forget` → fails at the first `roots`.
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_reaped_child_is_forgotten() {
        let tracker = Tracker::new();
        let mut cmd = match cfg!(windows) {
            true => {
                let mut c = Command::new("cmd");
                c.args(["/C", "exit 0"]);
                c
            }
            false => Command::new("true"),
        };
        let child = Mutex::new(tracker.spawn(&mut cmd).expect("spawn"));
        assert_eq!(tracker.roots(), 1);
        let code = wait_bounded(&tracker, &child, std::time::Duration::from_secs(10));
        assert_eq!(code, Some(0));
        assert_eq!(tracker.roots(), 0);
    }

    /// A helper that does not finish in time is ended and reported as such.
    /// flip: `run_bounded` without its timeout arm → the test hangs ~30 s.
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_helper_past_its_timeout_is_ended() {
        let mut cmd = match cfg!(windows) {
            true => {
                let mut c = Command::new("cmd");
                c.args(["/C", "ping -n 30 127.0.0.1"]);
                c
            }
            false => {
                let mut c = Command::new("sleep");
                c.arg("30");
                c
            }
        };
        let started = std::time::Instant::now();
        let r = run_bounded(&mut cmd, std::time::Duration::from_millis(500)).expect("spawned");
        assert!(r.is_none());
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }
}

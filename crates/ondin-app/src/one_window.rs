//! One Ondin window per machine. A GUI launch while a window runs hands its file
//! to that window, brings it forward, and exits.
//!
//! Tabs are how one window holds several files; a second window would hold the
//! same ones, and the two would save over each other.
//!
//! The window is the process holding an exclusive lock on `window.lock` in
//! `machine_dir::root()`. It listens on a local socket: a named pipe on Windows,
//! `window.sock` beside the lock elsewhere. A launch that cannot take the lock
//! connects, sends one request line, and exits once the window answers that it
//! has queued it (`opens`). A window that is closing answers so, and the launch
//! waits for its lock instead.
//!
//! The command-line runs (`export`, `render`, …) never come here: they open no
//! window.

use std::fs::File;
use std::io::{BufRead as _, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// The lock file's name in the machine's folder.
const LOCK: &str = "window.lock";

/// How long a launch keeps trying to reach a window or take its lock: long enough
/// for a closing window's settle and teardown.
const PATIENCE: Duration = Duration::from_secs(10);

/// How long a launch waits for a connected window to answer.
const ANSWER_WAIT: Duration = Duration::from_secs(3);

/// The pause between two tries.
const RETRY: Duration = Duration::from_millis(100);

/// The longest request line read, so a stray client cannot fill memory.
const MAX_REQUEST: u64 = 64 * 1024;

/// What a launch asks of the window.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Request {
    /// The file to open, absolute; `None` only brings the window forward.
    pub open: Option<PathBuf>,
}

/// What [`claim`] decided for this launch.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Claim {
    /// This process is the window.
    Window,
    /// The running window took the request; this process has nothing left to do.
    Forwarded,
    /// Another process holds the window and did not take the request.
    Unanswered,
}

/// The window's lock, held until the process ends.
static HELD: OnceLock<File> = OnceLock::new();

/// Set once the window is going, so a later launch waits for it to end.
static CLOSING: AtomicBool = AtomicBool::new(false);

/// Decide whether this launch is the window, handing `open` to the running one
/// if it is not.
///
/// With no machine folder, or a lock that cannot be made, this launch is the
/// window: refusing to start over a broken folder would cost more than the rare
/// second window.
pub(crate) fn claim(open: Option<&Path>) -> Claim {
    let Some(dir) = crate::machine_dir::root() else {
        return Claim::Window;
    };
    let request = Request {
        open: open.map(Path::to_path_buf),
    };
    match claim_in(&dir, &address(&dir), &request, PATIENCE, Arc::new(take)) {
        Outcome::Window(lock) => {
            if let Some(lock) = lock {
                let _ = HELD.set(lock);
            }
            Claim::Window
        }
        Outcome::Forwarded => Claim::Forwarded,
        Outcome::Unanswered => Claim::Unanswered,
    }
}

/// The window is going: a launch from now on waits for it to end.
pub(crate) fn closing() {
    CLOSING.store(true, Ordering::Release);
}

/// The window's answer to a request: queue it and ask to come forward, or refuse
/// it while closing.
fn take(request: Request) -> bool {
    if CLOSING.load(Ordering::Acquire) {
        return false;
    }
    if let Some(path) = request.open {
        crate::opens::push([path]);
    }
    crate::opens::raise();
    true
}

/// What the window does with a request; `false` refuses it.
type Sink = Arc<dyn Fn(Request) -> bool + Send + Sync>;

/// What [`claim_in`] found.
#[derive(Debug)]
enum Outcome {
    /// This process is the window, holding the lock when there was one to hold.
    Window(Option<File>),
    Forwarded,
    Unanswered,
}

/// [`claim`] in `dir`, listening at `addr` when this process becomes the window.
fn claim_in(dir: &Path, addr: &Path, request: &Request, patience: Duration, sink: Sink) -> Outcome {
    let deadline = Instant::now() + patience;
    loop {
        match take_lock(dir) {
            Lock::Taken(lock) => {
                // Taken before the listener exists, so a launch meanwhile finds no
                // listener and tries again.
                if let Err(e) = listen(addr, sink) {
                    log::warn!(target: "ondin", "one window: not listening at {}: {e}", addr.display());
                }
                return Outcome::Window(Some(lock));
            }
            Lock::Failed(e) => {
                log::warn!(target: "ondin", "one window: no lock in {}: {e}", dir.display());
                return Outcome::Window(None);
            }
            Lock::Held => {}
        }
        match send(addr, request) {
            Sent::Taken => return Outcome::Forwarded,
            Sent::Unanswered => return Outcome::Unanswered,
            // A window starting, or ending.
            Sent::NoListener | Sent::Refused => {}
        }
        if Instant::now() >= deadline {
            return Outcome::Unanswered;
        }
        std::thread::sleep(RETRY);
    }
}

enum Lock {
    Taken(File),
    /// Another process holds it.
    Held,
    Failed(std::io::Error),
}

/// Try for the lock in `dir`, creating the folder and the file as needed.
fn take_lock(dir: &Path) -> Lock {
    let open = std::fs::create_dir_all(dir).and_then(|()| {
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(LOCK))
    });
    let file = match open {
        Ok(file) => file,
        Err(e) => return Lock::Failed(e),
    };
    match file.try_lock() {
        Ok(()) => Lock::Taken(file),
        Err(std::fs::TryLockError::WouldBlock) => Lock::Held,
        Err(std::fs::TryLockError::Error(e)) => Lock::Failed(e),
    }
}

/// How a request went.
#[derive(Debug, PartialEq, Eq)]
enum Sent {
    Taken,
    /// The window answered that it is closing, or dropped the connection.
    Refused,
    /// Nothing listening at the address.
    NoListener,
    /// Connected, and no answer in [`ANSWER_WAIT`].
    Unanswered,
}

/// Send `request` to the window at `addr`.
///
/// The exchange runs on its own thread so a window that never answers cannot
/// hold this launch past [`ANSWER_WAIT`]; the process ends with it still blocked.
fn send(addr: &Path, request: &Request) -> Sent {
    let Ok(stream) = connect(addr) else {
        return Sent::NoListener;
    };
    let Ok(mut line) = serde_json::to_string(request) else {
        return Sent::NoListener;
    };
    line.push('\n');
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(exchange(stream, &line));
    });
    match rx.recv_timeout(ANSWER_WAIT) {
        Ok(Ok(reply)) if reply == "ok" => Sent::Taken,
        Ok(_) => Sent::Refused,
        Err(_) => Sent::Unanswered,
    }
}

/// Write `line` and read the one-line answer.
fn exchange(mut stream: impl Read + Write, line: &str) -> std::io::Result<String> {
    stream.write_all(line.as_bytes())?;
    stream.flush()?;
    let mut reply = String::new();
    BufReader::new(stream.take(64)).read_line(&mut reply)?;
    Ok(reply.trim().to_owned())
}

/// Read one request from `stream` and write the answer.
fn answer(mut stream: impl Read + Write, sink: &Sink) -> std::io::Result<()> {
    let mut line = String::new();
    BufReader::new((&mut stream).take(MAX_REQUEST)).read_line(&mut line)?;
    let reply = match serde_json::from_str::<Request>(line.trim()).map(|r| sink(r)) {
        Ok(true) => "ok\n",
        Ok(false) => "closing\n",
        Err(_) => "bad\n",
    };
    stream.write_all(reply.as_bytes())?;
    stream.flush()
}

/// The 64-bit FNV-1a hash of `bytes`: stable across builds and toolchains, which
/// std's `DefaultHasher` does not promise, so every build names the same pipe.
#[cfg_attr(not(windows), allow(dead_code))]
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// Where the window listens for the machine folder `dir`.
///
/// On Windows a pipe named after the folder, which holds the user's profile, so
/// two users on one machine each get their own.
fn address(dir: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(format!(
            r"\\.\pipe\Ondin-window-{:016x}",
            fnv1a(dir.as_os_str().as_encoded_bytes())
        ))
    }
    #[cfg(not(windows))]
    {
        dir.join("window.sock")
    }
}

#[cfg(not(windows))]
fn connect(addr: &Path) -> std::io::Result<std::os::unix::net::UnixStream> {
    std::os::unix::net::UnixStream::connect(addr)
}

/// Listen at `addr` on a thread of its own, one more thread per connection.
#[cfg(not(windows))]
fn listen(addr: &Path, sink: Sink) -> std::io::Result<()> {
    // The lock is held, so a socket file there is a dead window's.
    let _ = std::fs::remove_file(addr);
    let listener = std::os::unix::net::UnixListener::bind(addr)?;
    std::thread::Builder::new()
        .name("one-window".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let sink = sink.clone();
                std::thread::spawn(move || {
                    let _ = stream.set_read_timeout(Some(ANSWER_WAIT));
                    let _ = answer(stream, &sink);
                });
            }
        })?;
    Ok(())
}

/// Connect to the window's pipe, and let it take the foreground: Windows gives
/// that only to a process the user just started, which this one is.
#[cfg(windows)]
fn connect(addr: &Path) -> std::io::Result<File> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;
    use windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;
    let pipe = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(addr)?;
    let mut pid = 0u32;
    // SAFETY: `pipe` is an open pipe handle for the whole call, and `pid` a
    // writable `u32`.
    if unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle() as _, &mut pid) } != 0 {
        // SAFETY: takes a process id by value.
        unsafe { AllowSetForegroundWindow(pid) };
    }
    Ok(pipe)
}

/// Listen at `addr` on a thread of its own, one more thread per connection.
///
/// The first instance is made here, with `FILE_FLAG_FIRST_PIPE_INSTANCE`, so a
/// pipe another process already made is an error rather than a shared name.
#[cfg(windows)]
fn listen(addr: &Path, sink: Sink) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;
    let name: Vec<u16> = addr.as_os_str().encode_wide().chain([0]).collect();
    let first = pipe_instance(&name, true)?;
    std::thread::Builder::new()
        .name("one-window".into())
        .spawn(move || {
            let mut next = Some(first);
            while let Some(pipe) = next.take() {
                if let Ok(pipe) = connected(pipe) {
                    let sink = sink.clone();
                    std::thread::spawn(move || {
                        let mut pipe = pipe;
                        let _ = answer(&mut pipe, &sink);
                        // Until the launch has read the answer: closing first can
                        // drop it.
                        let _ = pipe.sync_all();
                    });
                }
                match pipe_instance(&name, false) {
                    Ok(pipe) => next = Some(pipe),
                    Err(e) => {
                        log::warn!(target: "ondin", "one window: pipe stopped: {e}");
                    }
                }
            }
        })?;
    Ok(())
}

/// One instance of the pipe `name` (NUL-terminated UTF-16), waiting for a client.
#[cfg(windows)]
fn pipe_instance(name: &[u16], first: bool) -> std::io::Result<File> {
    use std::os::windows::io::FromRawHandle as _;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX,
    };
    use windows_sys::Win32::System::Pipes::{
        CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
        PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };
    let mode = PIPE_ACCESS_DUPLEX
        | if first {
            FILE_FLAG_FIRST_PIPE_INSTANCE
        } else {
            0
        };
    // SAFETY: `name` is NUL-terminated and outlives the call; a null security
    // descriptor takes the default, which grants write to this user alone.
    let handle = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            4096,
            4096,
            0,
            std::ptr::null(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: a valid handle this function owns, handed to the `File` that closes it.
    Ok(unsafe { File::from_raw_handle(handle as _) })
}

/// Wait for a client on `pipe`.
#[cfg(windows)]
fn connected(pipe: File) -> std::io::Result<File> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Foundation::ERROR_PIPE_CONNECTED;
    use windows_sys::Win32::System::Pipes::ConnectNamedPipe;
    // SAFETY: `pipe` is an open pipe instance; no overlapped I/O.
    if unsafe { ConnectNamedPipe(pipe.as_raw_handle() as _, std::ptr::null_mut()) } != 0 {
        return Ok(pipe);
    }
    let e = std::io::Error::last_os_error();
    // A client that connected between the create and this call.
    if e.raw_os_error() == Some(ERROR_PIPE_CONNECTED as i32) {
        return Ok(pipe);
    }
    Err(e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A folder and an address of the test's own.
    fn fixture(name: &str) -> (PathBuf, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("ondin-one-window-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        #[cfg(windows)]
        let addr = PathBuf::from(format!(
            r"\\.\pipe\Ondin-window-test-{name}-{}",
            std::process::id()
        ));
        #[cfg(not(windows))]
        let addr = dir.join("window.sock");
        (dir, addr)
    }

    /// A sink that records what it was sent and answers `taking`.
    fn recorder(taking: Arc<AtomicBool>) -> (Sink, Arc<Mutex<Vec<Request>>>) {
        let got = Arc::new(Mutex::new(Vec::new()));
        let seen = got.clone();
        let sink: Sink = Arc::new(move |r| {
            seen.lock().unwrap().push(r);
            taking.load(Ordering::Acquire)
        });
        (sink, got)
    }

    /// The first launch becomes the window; a second hands its file to it and is
    /// told so, and one with no file only asks for the window.
    ///
    /// flip: `claim_in` returning `Window` when the lock is held → fails at
    /// "forwarded".
    #[test]
    fn a_second_launch_hands_its_file_to_the_window() {
        let (dir, addr) = fixture("hand");
        let (sink, got) = recorder(Arc::new(AtomicBool::new(true)));
        let none = Request { open: None };
        let first = claim_in(&dir, &addr, &none, Duration::ZERO, sink.clone());
        assert!(
            matches!(first, Outcome::Window(Some(_))),
            "the first is the window: {first:?}"
        );
        let file = Request {
            open: Some(dir.join("a b.ondin")),
        };
        let second = claim_in(&dir, &addr, &file, PATIENCE, sink.clone());
        assert!(
            matches!(second, Outcome::Forwarded),
            "forwarded: {second:?}"
        );
        let third = claim_in(&dir, &addr, &none, PATIENCE, sink);
        assert!(matches!(third, Outcome::Forwarded), "forwarded: {third:?}");
        assert_eq!(*got.lock().unwrap(), [file, none]);
        drop(first);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A window that is closing refuses the request, and the launch takes the lock
    /// once that window's process lets it go, becoming the window itself.
    ///
    /// flip: `claim_in` returning on `Sent::Refused` → fails at "the launch
    /// becomes the window".
    #[test]
    fn a_launch_waits_out_a_closing_window() {
        let (dir, addr) = fixture("closing");
        let (sink, got) = recorder(Arc::new(AtomicBool::new(false)));
        let none = Request { open: None };
        let Outcome::Window(Some(lock)) =
            claim_in(&dir, &addr, &none, Duration::ZERO, sink.clone())
        else {
            panic!("the fixture's window holds the lock");
        };
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            drop(lock);
        });
        let (later, _) = recorder(Arc::new(AtomicBool::new(true)));
        let next = claim_in(&dir, &addr, &none, PATIENCE, later);
        release.join().unwrap();
        assert!(
            matches!(next, Outcome::Window(Some(_))),
            "the launch becomes the window: {next:?}"
        );
        assert!(
            !got.lock().unwrap().is_empty(),
            "the closing window was asked"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A window that never answers lets the launch go after the wait, rather
    /// than holding it.
    ///
    /// flip: `send` waiting on `recv` with no timeout → fails at `Unanswered`, a
    /// minute late.
    #[test]
    fn a_window_that_never_answers_is_given_up_on() {
        let (dir, addr) = fixture("silent");
        let stall: Sink = Arc::new(|_| {
            std::thread::sleep(Duration::from_secs(60));
            true
        });
        let none = Request { open: None };
        let first = claim_in(&dir, &addr, &none, Duration::ZERO, stall.clone());
        assert!(matches!(first, Outcome::Window(Some(_))));
        let started = Instant::now();
        let second = claim_in(&dir, &addr, &none, PATIENCE, stall);
        assert!(matches!(second, Outcome::Unanswered), "{second:?}");
        assert!(
            started.elapsed() < ANSWER_WAIT * 2,
            "{:?}",
            started.elapsed()
        );
        drop(first);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The hash naming the pipe is FNV-1a's, whose published value for "a" is
    /// fixed: a build that hashed differently would name another pipe.
    #[test]
    fn the_pipe_name_hash_is_stable() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}

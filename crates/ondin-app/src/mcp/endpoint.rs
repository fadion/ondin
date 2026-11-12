//! A host's MCP endpoint (§8.1): a local socket admitting only this user, one
//! thread per connection, and a discovery file the proxy finds it by.

use ondin_mcp::queue::{Conn, Forward, Request};
use ondin_mcp::wire;
use serde::{Deserialize, Serialize};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;

/// Which kind of process hosts an endpoint; the proxy prefers the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Host {
    Window,
    Serve,
}

/// A discovery file's content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Found {
    pub pid: u32,
    pub host: Host,
    /// The socket's path, or the pipe's name.
    pub endpoint: PathBuf,
}

/// The folder holding discovery files and, off Windows, the sockets: beside the
/// window's lock.
pub(crate) fn run_dir() -> Option<PathBuf> {
    Some(crate::one_window::window_dir()?.join("mcp"))
}

/// The tool definitions a host serves: every tool of every phase.
pub(crate) fn tools() -> Vec<serde_json::Value> {
    use ondin_mcp::tools::{PHASE_1, PHASE_2, PHASE_3, PHASE_4};
    PHASE_1
        .iter()
        .chain(&PHASE_2)
        .chain(&PHASE_3)
        .chain(&PHASE_4)
        .filter_map(|name| ondin_mcp::schema::definition(name))
        .collect()
}

/// A listening endpoint; dropping it removes the discovery file.
pub(crate) struct Endpoint {
    file: PathBuf,
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.file);
    }
}

/// Listen for this process as `host`, sending every call to `to`.
pub(crate) fn listen(host: Host, to: Sender<Request>) -> std::io::Result<Endpoint> {
    let dir = run_dir().ok_or_else(|| std::io::Error::other("no folder for the endpoint"))?;
    make_private_dir(&dir)?;
    let pid = std::process::id();
    let endpoint = address(&dir, pid);
    serve_at(&endpoint, to)?;
    let file = dir.join(format!("{pid}.json"));
    let found = Found {
        pid,
        host,
        endpoint,
    };
    let bytes = serde_json::to_vec(&found).map_err(std::io::Error::other)?;
    crate::atomic::write(&file, &bytes)?;
    Ok(Endpoint { file })
}

/// Every discovery file in the run folder, the window's first. A file whose
/// endpoint does not answer is a dead process's and is removed.
pub(crate) fn hosts() -> Vec<Found> {
    let Some(dir) = run_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut found: Vec<(Found, PathBuf)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| {
            let f: Found = serde_json::from_slice(&std::fs::read(&p).ok()?).ok()?;
            Some((f, p))
        })
        .collect();
    found.retain(|(f, file)| {
        let alive = connect(&f.endpoint).is_ok();
        if !alive {
            let _ = std::fs::remove_file(file);
        }
        alive
    });
    found.sort_by_key(|(f, _)| (f.host != Host::Window, f.pid));
    found.into_iter().map(|(f, _)| f).collect()
}

/// A connection to an endpoint, as a reader and a writer.
pub(crate) type Stream = (Box<dyn Read + Send>, Box<dyn Write + Send>);

static NEXT: AtomicU64 = AtomicU64::new(1);

/// Answer one accepted connection on its own thread.
fn spawn_connection(read: Box<dyn Read + Send>, write: Box<dyn Write + Send>, to: Sender<Request>) {
    let conn: Conn = NEXT.fetch_add(1, Ordering::Relaxed);
    let _ = std::thread::Builder::new()
        .name(format!("mcp-{conn}"))
        .spawn(move || {
            let mut handler = Forward {
                conn,
                to,
                tools: tools(),
            };
            let _ = wire::serve(BufReader::new(read), write, &mut handler);
        });
}

#[cfg(windows)]
fn make_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

#[cfg(not(windows))]
fn make_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

/// The pipe's name: per user (the folder holds the profile) and per process.
#[cfg(windows)]
fn address(dir: &Path, pid: u32) -> PathBuf {
    let user = crate::one_window::fnv1a(dir.as_os_str().as_encoded_bytes());
    PathBuf::from(format!(r"\\.\pipe\Ondin-mcp-{user:016x}-{pid}"))
}

#[cfg(not(windows))]
fn address(dir: &Path, pid: u32) -> PathBuf {
    dir.join(format!("{pid}.sock"))
}

/// Accept connections at `addr` on a thread of its own. The pipe rejects remote
/// clients, its default DACL lets only this user write, and a client that runs
/// as anyone else is dropped.
#[cfg(windows)]
fn serve_at(addr: &Path, to: Sender<Request>) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;
    let name: Vec<u16> = addr.as_os_str().encode_wide().chain([0]).collect();
    let first = crate::one_window::pipe_instance(&name, true)?;
    std::thread::Builder::new()
        .name("mcp-listen".into())
        .spawn(move || {
            let mut next = Some(first);
            while let Some(pipe) = next.take() {
                if let Ok(pipe) = crate::one_window::connected(pipe)
                    && client_is_this_user(&pipe)
                    && let Ok(read) = pipe.try_clone()
                {
                    spawn_connection(Box::new(read), Box::new(pipe), to.clone());
                }
                match crate::one_window::pipe_instance(&name, false) {
                    Ok(pipe) => next = Some(pipe),
                    Err(e) => log::warn!(target: "ondin", "mcp: pipe stopped: {e}"),
                }
            }
        })?;
    Ok(())
}

#[cfg(windows)]
fn client_is_this_user(pipe: &std::fs::File) -> bool {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId;
    let mut pid = 0u32;
    // SAFETY: `pipe` is a connected pipe handle for the whole call, `pid` writable.
    let ok = unsafe { GetNamedPipeClientProcessId(pipe.as_raw_handle() as _, &mut pid) } != 0;
    ok && crate::one_window::same_user(pid)
}

#[cfg(not(windows))]
fn serve_at(addr: &Path, to: Sender<Request>) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let _ = std::fs::remove_file(addr);
    let listener = std::os::unix::net::UnixListener::bind(addr)?;
    std::fs::set_permissions(addr, std::fs::Permissions::from_mode(0o600))?;
    std::thread::Builder::new()
        .name("mcp-listen".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                if !peer_is_this_user(&stream) {
                    continue;
                }
                if let Ok(read) = stream.try_clone() {
                    spawn_connection(Box::new(read), Box::new(stream), to.clone());
                }
            }
        })?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn peer_is_this_user(stream: &std::os::unix::net::UnixStream) -> bool {
    use std::os::fd::AsRawFd as _;
    let mut cred = libc::ucred {
        pid: 0,
        uid: u32::MAX,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `cred` and `len` are writable and sized for `SO_PEERCRED`.
    let ok = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut cred).cast(),
            &mut len,
        )
    } == 0;
    // SAFETY: takes no arguments.
    ok && cred.uid == unsafe { libc::getuid() }
}

#[cfg(all(unix, not(target_os = "linux")))]
fn peer_is_this_user(stream: &std::os::unix::net::UnixStream) -> bool {
    use std::os::fd::AsRawFd as _;
    let (mut uid, mut gid) = (u32::MAX, 0);
    // SAFETY: `uid` and `gid` are writable.
    let ok = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } == 0;
    // SAFETY: takes no arguments.
    ok && uid == unsafe { libc::getuid() }
}

/// Connect to the endpoint at `addr`. Every pipe instance is busy for a moment
/// after a client takes one, until the host makes the next, so that is retried.
#[cfg(windows)]
pub(crate) fn connect(addr: &Path) -> std::io::Result<Stream> {
    use windows_sys::Win32::Foundation::ERROR_PIPE_BUSY;
    let started = std::time::Instant::now();
    let pipe = loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(addr)
        {
            Err(e)
                if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32)
                    && started.elapsed() < std::time::Duration::from_secs(2) =>
            {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            other => break other?,
        }
    };
    let read = pipe.try_clone()?;
    Ok((Box::new(read), Box::new(pipe)))
}

#[cfg(not(windows))]
pub(crate) fn connect(addr: &Path) -> std::io::Result<Stream> {
    let stream = std::os::unix::net::UnixStream::connect(addr)?;
    let read = stream.try_clone()?;
    Ok((Box::new(read), Box::new(stream)))
}

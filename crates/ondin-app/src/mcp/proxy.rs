//! `ondin mcp-proxy` (§8.1): the one server a harness configures. It carries
//! lines between the agent's stdio and a host's endpoint: the window's when one
//! runs, else a `serve`, started if none is.
//!
//! It remembers the handshake and the current document, so when its host goes
//! away, or a window starts while it talks to `serve`, it reconnects to the
//! window and restores both. Requests are carried one at a time: the hosts send
//! nothing unasked, and a pipe handle cannot read and write at once.

use super::endpoint::{self, Found, Host};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::time::{Duration, Instant};

/// How long the proxy waits for a `serve` it started to listen.
const START_WAIT: Duration = Duration::from_secs(15);

struct Link {
    host: Found,
    read: BufReader<Box<dyn Read + Send>>,
    write: Box<dyn Write + Send>,
}

impl Link {
    /// Send `line` and, when it asks for one, read the reply.
    fn exchange(&mut self, line: &str, reply: bool) -> std::io::Result<Option<String>> {
        writeln!(self.write, "{line}")?;
        self.write.flush()?;
        if !reply {
            return Ok(None);
        }
        let mut answer = String::new();
        if self.read.read_line(&mut answer)? == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        Ok(Some(answer.trim_end().to_owned()))
    }
}

#[derive(Default)]
struct Remembered {
    initialize: Option<String>,
    initialized: Option<String>,
    document: Option<String>,
}

/// Run the proxy until the agent closes its stdin.
pub(crate) fn run() -> Result<(), String> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut kept = Remembered::default();
    let mut link: Option<Link> = None;
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let (line, message) = rewrite(&line);
        let method = message
            .as_ref()
            .and_then(|m| m.get("method")?.as_str().map(str::to_owned));
        let reply = message.as_ref().is_none_or(|m| m.get("id").is_some());
        match method.as_deref() {
            Some("initialize") => kept.initialize = Some(line.clone()),
            Some("notifications/initialized") => kept.initialized = Some(line.clone()),
            _ => {}
        }
        if link
            .as_ref()
            .is_some_and(|l| l.host.host == Host::Serve && window_listed())
        {
            link = None;
        }
        let mut answer = None;
        for _ in 0..2 {
            if link.is_none() {
                link = Some(connect(&kept, method.as_deref())?);
            }
            match link.as_mut().map(|l| l.exchange(&line, reply)) {
                Some(Ok(a)) => {
                    answer = a;
                    break;
                }
                _ => link = None,
            }
        }
        let Some(answer) = answer else {
            if reply {
                return Err("ondin mcp-proxy: lost the Ondin endpoint".into());
            }
            continue;
        };
        if is_tool_call(&message, "use_document")
            && let Ok(v) = serde_json::from_str::<Value>(&answer)
            && v.pointer("/result/isError") == Some(&json!(false))
            && let Some(doc) = v
                .pointer("/result/structuredContent/document")
                .and_then(Value::as_str)
        {
            kept.document = Some(doc.to_owned());
        }
        writeln!(stdout, "{answer}").map_err(|e| e.to_string())?;
        stdout.flush().map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn is_tool_call(message: &Option<Value>, tool: &str) -> bool {
    message.as_ref().is_some_and(|m| {
        m.get("method").and_then(Value::as_str) == Some("tools/call")
            && m.pointer("/params/name").and_then(Value::as_str) == Some(tool)
    })
}

/// A `use_document` naming a relative path that exists from here, the harness's
/// working folder, with or without `.ondin`, names it absolutely, since the
/// host's folder is another.
fn rewrite(line: &str) -> (String, Option<Value>) {
    let Ok(mut message) = serde_json::from_str::<Value>(line) else {
        return (line.to_owned(), None);
    };
    if is_tool_call(&Some(message.clone()), "use_document")
        && let Some(arg) = message.pointer_mut("/params/arguments/document")
        && let Some(asked) = arg.as_str()
        && std::path::Path::new(asked).is_relative()
        && let Some(full) = [asked.to_owned(), format!("{asked}.ondin")]
            .iter()
            .filter_map(|p| std::path::absolute(p).ok())
            .find(|p| p.is_file())
    {
        *arg = json!(full.display().to_string());
        return (message.to_string(), Some(message));
    }
    (line.to_owned(), Some(message))
}

/// Whether a window's discovery file is there, without connecting.
fn window_listed() -> bool {
    let Some(dir) = endpoint::run_dir() else {
        return false;
    };
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| {
            std::fs::read(e.path())
                .ok()
                .and_then(|b| serde_json::from_slice::<Found>(&b).ok())
                .is_some_and(|f| f.host == Host::Window)
        })
}

/// Connect to the best host, starting a `serve` if none runs, and replay the
/// handshake and the current document, unless `now` is the handshake itself.
fn connect(kept: &Remembered, now: Option<&str>) -> Result<Link, String> {
    let host = match endpoint::hosts().into_iter().next() {
        Some(h) => h,
        None => start_serve()?,
    };
    let (read, write) = endpoint::connect(&host.endpoint)
        .map_err(|e| format!("ondin mcp-proxy: cannot reach Ondin: {e}"))?;
    let mut link = Link {
        host,
        read: BufReader::new(read),
        write,
    };
    if now == Some("initialize") {
        return Ok(link);
    }
    let fail = |e: std::io::Error| format!("ondin mcp-proxy: {e}");
    if let Some(init) = &kept.initialize {
        link.exchange(init, true).map_err(fail)?;
    }
    if let Some(done) = &kept.initialized {
        link.exchange(done, false).map_err(fail)?;
    }
    if let Some(doc) = &kept.document {
        let call = json!({
            "jsonrpc": "2.0",
            "id": "ondin-proxy-restore",
            "method": "tools/call",
            "params": { "name": "use_document", "arguments": { "document": doc } },
        });
        link.exchange(&call.to_string(), true).map_err(fail)?;
    }
    Ok(link)
}

/// Start `ondin serve` and wait for it to listen.
fn start_serve() -> Result<Found, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut command = std::process::Command::new(exe);
    command
        .arg("serve")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        // DETACHED_PROCESS | CREATE_NO_WINDOW: no console flashes up, and the
        // harness closing the proxy's console does not end it.
        command.creation_flags(0x0000_0008 | 0x0800_0000);
    }
    let child = command
        .spawn()
        .map_err(|e| format!("ondin mcp-proxy: cannot start ondin serve: {e}"))?;
    let pid = child.id();
    let started = Instant::now();
    while started.elapsed() < START_WAIT {
        if let Some(h) = endpoint::hosts()
            .into_iter()
            .find(|h| h.pid == pid || h.host == Host::Window)
        {
            return Ok(h);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err("ondin mcp-proxy: ondin serve did not start listening".into())
}

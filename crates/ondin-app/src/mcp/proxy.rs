//! `ondin mcp-proxy` (§8.1): the one server a harness configures. It carries
//! lines between the agent's stdio and a host's endpoint: the window's when one
//! runs, else a `serve`, started if none is.
//!
//! It remembers the handshake and the current document, so when its host goes
//! away, or a window starts while it talks to `serve`, it reconnects to the
//! window and restores both. Requests are carried one at a time: the hosts send
//! nothing unasked, and a pipe handle cannot read and write at once.
//!
//! `--client <key>` puts a key in `initialize`, under which the host keeps the
//! client's document and revisions across its connections; `--document` names
//! the document every call acts on until the agent picks another.

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
    /// Whether the remembered document was named on this link yet.
    restored: bool,
}

/// How an exchange broke: before the host could have read the line, or after,
/// when it may have acted on it.
enum Broke {
    Unsent(std::io::Error),
    Unanswered(std::io::Error),
}

impl Broke {
    fn error(self) -> std::io::Error {
        match self {
            Self::Unsent(e) | Self::Unanswered(e) => e,
        }
    }
}

impl Link {
    /// Send `line` and, when it asks for one, read the reply.
    fn exchange(&mut self, line: &str, reply: bool) -> Result<Option<String>, Broke> {
        writeln!(self.write, "{line}").map_err(Broke::Unsent)?;
        self.write.flush().map_err(Broke::Unsent)?;
        if !reply {
            return Ok(None);
        }
        let mut answer = String::new();
        match self.read.read_line(&mut answer) {
            Ok(0) => Err(Broke::Unanswered(std::io::ErrorKind::UnexpectedEof.into())),
            Ok(_) => Ok(Some(answer.trim_end().to_owned())),
            Err(e) => Err(Broke::Unanswered(e)),
        }
    }
}

#[derive(Default)]
struct Remembered {
    initialize: Option<String>,
    initialized: Option<String>,
    document: Option<String>,
}

/// Run the proxy until the agent closes its stdin, keying the host's state by
/// `client` and starting on `document` when they are given.
pub(crate) fn run(client: Option<String>, document: Option<String>) -> Result<(), String> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut kept = Remembered {
        document: document.map(|d| absolute(&d)),
        ..Remembered::default()
    };
    let mut link: Option<Link> = None;
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let (line, message) = rewrite(&line, client.as_deref());
        let method = message
            .as_ref()
            .and_then(|m| m.get("method")?.as_str().map(str::to_owned));
        // As the host's `wire::answer` decides: anything but an object without
        // an `id` is answered.
        let reply = message
            .as_ref()
            .is_none_or(|m| !m.is_object() || m.get("id").is_some());
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
        let call = method.as_deref() == Some("tools/call");
        let mut answer = Err("lost the Ondin endpoint".to_owned());
        for _ in 0..2 {
            let l = match link.as_mut() {
                Some(l) => l,
                None => match connect(&kept, method.as_deref()) {
                    Ok(l) => link.insert(l),
                    Err(why) => {
                        answer = Err(why);
                        break;
                    }
                },
            };
            if call && restore(l, &kept).is_err() {
                link = None;
                continue;
            }
            match l.exchange(&line, reply) {
                Ok(a) => {
                    answer = Ok(a);
                    break;
                }
                Err(broke) => {
                    link = None;
                    // A tool call the host may have run is not sent again: a
                    // write would apply twice.
                    if call && let Broke::Unanswered(e) = broke {
                        answer = Err(format!(
                            "the link to Ondin broke before it answered ({e}); the call \
                             may have applied, so read the document before trying again"
                        ));
                        break;
                    }
                    answer = Err(format!("lost the Ondin endpoint: {}", broke.error()));
                }
            }
        }
        // A failure is the request's answer, not the proxy's end: the next
        // request connects again.
        let answer = match answer {
            Ok(Some(a)) => a,
            Ok(None) => continue,
            Err(why) => match message.as_ref().and_then(|m| m.get("id")) {
                Some(id) if reply => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32603, "message": format!("ondin mcp-proxy: {why}") },
                })
                .to_string(),
                _ => continue,
            },
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

/// `asked` named absolutely when it is a relative path that exists from here,
/// the harness's working folder, with or without `.ondin`, since the host's
/// folder is another; else as it is, for the host to match by name.
fn absolute(asked: &str) -> String {
    if !std::path::Path::new(asked).is_relative() {
        return asked.to_owned();
    }
    [asked.to_owned(), format!("{asked}.ondin")]
        .iter()
        .filter_map(|p| std::path::absolute(p).ok())
        .find(|p| p.is_file())
        .map_or_else(|| asked.to_owned(), |p| p.display().to_string())
}

/// The line as the host gets it: a `use_document`'s path made absolute, and
/// `initialize` carrying the client key when there is one.
fn rewrite(line: &str, client: Option<&str>) -> (String, Option<Value>) {
    let Ok(mut message) = serde_json::from_str::<Value>(line) else {
        return (line.to_owned(), None);
    };
    let mut changed = false;
    if is_tool_call(&Some(message.clone()), "use_document")
        && let Some(arg) = message.pointer_mut("/params/arguments/document")
        && let Some(asked) = arg.as_str()
    {
        let full = absolute(asked);
        if full != asked {
            *arg = json!(full);
            changed = true;
        }
    }
    if let Some(key) = client
        && message.get("method").and_then(Value::as_str) == Some("initialize")
        && let Some(params) = message.get_mut("params").and_then(Value::as_object_mut)
    {
        let meta = params.entry("_meta").or_insert_with(|| json!({}));
        if let Some(meta) = meta.as_object_mut() {
            meta.insert(ondin_mcp::wire::CLIENT_KEY_META.into(), json!(key));
            changed = true;
        }
    }
    match changed {
        true => (message.to_string(), Some(message)),
        false => (line.to_owned(), Some(message)),
    }
}

/// Name the remembered document on `link` once, before its first tool call, as
/// a restore: the host keeps a keyed client's history for it and reports no
/// `changed`, which the agent's own call then hears.
fn restore(link: &mut Link, kept: &Remembered) -> std::io::Result<()> {
    if link.restored {
        return Ok(());
    }
    if let Some(doc) = &kept.document {
        let call = json!({
            "jsonrpc": "2.0",
            "id": "ondin-proxy-restore",
            "method": "tools/call",
            "params": {
                "name": "use_document",
                "arguments": { "document": doc, "restore": true },
            },
        });
        link.exchange(&call.to_string(), true)
            .map_err(Broke::error)?;
    }
    link.restored = true;
    Ok(())
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
/// handshake, unless `now` is the handshake itself; [`restore`] names the
/// document before the link's first tool call.
fn connect(kept: &Remembered, now: Option<&str>) -> Result<Link, String> {
    let host = match endpoint::hosts().into_iter().next() {
        Some(h) => h,
        None => start_serve()?,
    };
    let (read, write) =
        endpoint::connect(&host.endpoint).map_err(|e| format!("cannot reach Ondin: {e}"))?;
    let mut link = Link {
        host,
        read: BufReader::new(read),
        write,
        restored: false,
    };
    if now == Some("initialize") {
        return Ok(link);
    }
    let fail = |e: Broke| format!("lost the Ondin endpoint: {}", e.error());
    if let Some(init) = &kept.initialize {
        link.exchange(init, true).map_err(fail)?;
    }
    if let Some(done) = &kept.initialized {
        link.exchange(done, false).map_err(fail)?;
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
        .map_err(|e| format!("cannot start ondin serve: {e}"))?;
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
    Err("ondin serve did not start listening".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With a key, `initialize` carries it under the host's `_meta` name beside
    /// whatever `_meta` it had; without one, the line passes untouched.
    ///
    /// flip: `rewrite` ignoring `client` → fails at the key.
    #[test]
    fn initialize_carries_the_client_key() {
        let line = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"_meta":{"x":1}}}"#;
        let (out, _) = rewrite(line, Some("chat-1"));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["params"]["_meta"][ondin_mcp::wire::CLIENT_KEY_META],
            "chat-1"
        );
        assert_eq!(v["params"]["_meta"]["x"], 1);
        assert_eq!(rewrite(line, None).0, line);
        let call =
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_selection"}}"#;
        assert_eq!(rewrite(call, Some("chat-1")).0, call, "only initialize");
    }
}

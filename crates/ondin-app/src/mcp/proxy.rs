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

/// Where the proxy finds its hosts: the machine's endpoints, or a test's.
trait Hosts {
    /// A connection to the best host, starting `serve` when none runs, and which
    /// kind of host it is.
    fn connect(&mut self) -> Result<(Host, endpoint::Stream), String>;
    /// Whether a window lists its endpoint, without connecting.
    fn window_listed(&mut self) -> bool;
}

/// The hosts this machine runs.
struct Machine;

impl Hosts for Machine {
    fn connect(&mut self) -> Result<(Host, endpoint::Stream), String> {
        let found = match endpoint::hosts().into_iter().next() {
            Some(h) => h,
            None => start_serve()?,
        };
        let stream =
            endpoint::connect(&found.endpoint).map_err(|e| format!("cannot reach Ondin: {e}"))?;
        Ok((found.host, stream))
    }

    fn window_listed(&mut self) -> bool {
        window_listed()
    }
}

struct Link {
    host: Host,
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
/// `client` and starting on `document` when they are given, and asking for
/// every tool with `all_tools`.
pub(crate) fn run(
    client: Option<String>,
    document: Option<String>,
    all_tools: bool,
) -> Result<(), String> {
    carry(
        std::io::stdin().lock(),
        std::io::stdout(),
        Ask { client, all_tools },
        document.map(|d| absolute(&d)),
        &mut Machine,
    )
}

/// What the proxy adds to `initialize` for the host: the client key, and a
/// request for every tool rather than the core set.
#[derive(Default)]
struct Ask {
    client: Option<String>,
    all_tools: bool,
}

/// The proxy's loop: each line of `input` carried to a host from `hosts` and its
/// answer written to `stdout`, until `input` ends.
fn carry(
    mut input: impl BufRead,
    mut stdout: impl Write,
    ask: Ask,
    document: Option<String>,
    hosts: &mut impl Hosts,
) -> Result<(), String> {
    let mut kept = Remembered {
        document,
        ..Remembered::default()
    };
    let mut link: Option<Link> = None;
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        if input
            .read_until(b'\n', &mut bytes)
            .map_err(|e| e.to_string())?
            == 0
        {
            return Ok(());
        }
        // A line that is not UTF-8 is answered here, as the host would, rather
        // than ending the agent's MCP server.
        let Ok(line) = std::str::from_utf8(&bytes) else {
            let error = json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": { "code": -32700, "message": "not UTF-8" },
            });
            writeln!(stdout, "{error}").map_err(|e| e.to_string())?;
            stdout.flush().map_err(|e| e.to_string())?;
            continue;
        };
        let line = line.trim_end_matches(['\r', '\n']);
        if line.trim().is_empty() {
            continue;
        }
        let (line, message) = rewrite(line, &ask);
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
            .is_some_and(|l| l.host == Host::Serve && hosts.window_listed())
        {
            link = None;
        }
        let call = method.as_deref() == Some("tools/call");
        let mut answer = Err("lost the Ondin endpoint".to_owned());
        for _ in 0..2 {
            let l = match link.as_mut() {
                Some(l) => l,
                None => match connect(&kept, method.as_deref(), hosts) {
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
/// `initialize` carrying what `ask` adds: the client key when there is one, and
/// the request for every tool.
fn rewrite(line: &str, ask: &Ask) -> (String, Option<Value>) {
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
    if (ask.client.is_some() || ask.all_tools)
        && message.get("method").and_then(Value::as_str) == Some("initialize")
        && let Some(params) = message.get_mut("params").and_then(Value::as_object_mut)
    {
        let meta = params.entry("_meta").or_insert_with(|| json!({}));
        if let Some(meta) = meta.as_object_mut() {
            if let Some(key) = &ask.client {
                meta.insert(ondin_mcp::wire::CLIENT_KEY_META.into(), json!(key));
            }
            if ask.all_tools {
                meta.insert(ondin_mcp::wire::TOOLS_META.into(), json!("all"));
            }
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

/// Connect to the best host `hosts` offers, and replay the
/// handshake, unless `now` is the handshake itself; [`restore`] names the
/// document before the link's first tool call.
fn connect(kept: &Remembered, now: Option<&str>, hosts: &mut impl Hosts) -> Result<Link, String> {
    let (host, (read, write)) = hosts.connect()?;
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
    // Its own process group: a harness ending the proxy's group would otherwise
    // end `serve` too, and the client state it keeps between turns.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("cannot start ondin serve: {e}"))?;
    // Reaped when it ends, so it never lingers as a zombie on Unix.
    let _ = std::thread::Builder::new()
        .name("serve-reaper".into())
        .spawn(move || child.wait());
    let started = Instant::now();
    while started.elapsed() < START_WAIT {
        // Any host will do: the one started may have left at once because
        // another proxy's `serve` holds the lock.
        if let Some(h) = endpoint::hosts().into_iter().next() {
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
    /// whatever `_meta` it had, and with `--tools all` the request for every
    /// tool; with neither, the line passes untouched.
    ///
    /// flip: `rewrite` ignoring `client` → fails at the key.
    #[test]
    fn initialize_carries_the_client_key() {
        let keyed = Ask {
            client: Some("chat-1".into()),
            all_tools: false,
        };
        let line = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"_meta":{"x":1}}}"#;
        let (out, _) = rewrite(line, &keyed);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["params"]["_meta"][ondin_mcp::wire::CLIENT_KEY_META],
            "chat-1"
        );
        assert_eq!(v["params"]["_meta"]["x"], 1);
        assert!(
            v["params"]["_meta"]
                .get(ondin_mcp::wire::TOOLS_META)
                .is_none()
        );
        assert_eq!(rewrite(line, &Ask::default()).0, line);
        let all = Ask {
            client: None,
            all_tools: true,
        };
        let (out, _) = rewrite(line, &all);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["params"]["_meta"][ondin_mcp::wire::TOOLS_META], "all");
        let call =
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_selection"}}"#;
        assert_eq!(rewrite(call, &keyed).0, call, "only initialize");
    }

    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    /// A host a test scripts: it answers every request, keeps the lines it read,
    /// and goes away without answering a call to `drop_on`. A call to `window_on`
    /// lists a window before it is answered.
    struct Scripted {
        host: Host,
        drop_on: Option<&'static str>,
        window_on: Option<&'static str>,
    }

    /// The hosts a test offers, in the order the proxy connects to them.
    struct Fake {
        next: std::collections::VecDeque<Scripted>,
        window: Arc<AtomicBool>,
        /// Each host's lines, by the order it was connected to.
        seen: Vec<Arc<Mutex<Vec<Value>>>>,
    }

    impl Fake {
        fn new(hosts: Vec<Scripted>) -> Self {
            Self {
                next: hosts.into(),
                window: Arc::default(),
                seen: Vec::new(),
            }
        }

        /// The tool calls host `i` read, as `(tool, arguments)`.
        fn calls(&self, i: usize) -> Vec<(String, Value)> {
            let Some(seen) = self.seen.get(i) else {
                return Vec::new();
            };
            seen.lock()
                .unwrap()
                .iter()
                .filter_map(|m| {
                    let name = m.pointer("/params/name")?.as_str()?;
                    Some((name.to_owned(), m["params"]["arguments"].clone()))
                })
                .collect()
        }

        /// The methods host `i` read, in order.
        fn methods(&self, i: usize) -> Vec<String> {
            let Some(seen) = self.seen.get(i) else {
                return Vec::new();
            };
            let seen = seen.lock().unwrap();
            seen.iter()
                .filter_map(|m| m["method"].as_str().map(str::to_owned))
                .collect()
        }
    }

    impl Hosts for Fake {
        fn connect(&mut self) -> Result<(Host, endpoint::Stream), String> {
            let script = self.next.pop_front().ok_or("no host")?;
            let (from_proxy, to_host) = std::io::pipe().map_err(|e| e.to_string())?;
            let (from_host, mut to_proxy) = std::io::pipe().map_err(|e| e.to_string())?;
            let seen = Arc::new(Mutex::new(Vec::new()));
            self.seen.push(seen.clone());
            let window = self.window.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(from_proxy).lines() {
                    let Ok(line) = line else { return };
                    let message: Value = serde_json::from_str(&line).unwrap();
                    seen.lock().unwrap().push(message.clone());
                    let tool = message.pointer("/params/name").and_then(Value::as_str);
                    if tool.is_some() && tool == script.drop_on {
                        return;
                    }
                    let Some(id) = message.get("id") else {
                        continue;
                    };
                    let document = message.pointer("/params/arguments/document");
                    let result =
                        json!({ "isError": false, "structuredContent": { "document": document } });
                    if tool.is_some() && tool == script.window_on {
                        window.store(true, Ordering::SeqCst);
                    }
                    let answer = json!({ "jsonrpc": "2.0", "id": id, "result": result });
                    if writeln!(to_proxy, "{answer}").is_err() {
                        return;
                    }
                }
            });
            Ok((script.host, (Box::new(from_host), Box::new(to_host))))
        }

        fn window_listed(&mut self) -> bool {
            self.window.load(Ordering::SeqCst)
        }
    }

    /// The agent's lines, one request each, and what the proxy answered them.
    fn drive(lines: &[Value], hosts: &mut Fake, document: Option<&str>) -> Vec<Value> {
        let input: String = lines.iter().map(|l| format!("{l}\n")).collect();
        let mut out = Vec::new();
        carry(
            input.as_bytes(),
            &mut out,
            Ask::default(),
            document.map(str::to_owned),
            hosts,
        )
        .unwrap();
        String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn handshake() -> [Value; 2] {
        [
            json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }),
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        ]
    }

    fn tool_call(id: u64, name: &str, arguments: Value) -> Value {
        json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": name, "arguments": arguments } })
    }

    /// Once a window lists itself, the proxy leaves `serve` at the next request,
    /// replays the handshake on the window and names the remembered document
    /// there as a restore before carrying the call.
    ///
    /// flip: `carry` keeping the `serve` link when a window lists → fails at "the
    /// handshake is replayed on the window".
    #[test]
    fn a_window_takes_over_from_serve_with_the_document_restored() {
        let mut hosts = Fake::new(vec![
            Scripted {
                host: Host::Serve,
                drop_on: None,
                window_on: Some("get_selection"),
            },
            Scripted {
                host: Host::Window,
                drop_on: None,
                window_on: None,
            },
        ]);
        let [init, done] = handshake();
        let lines = [
            init,
            done,
            tool_call(2, "use_document", json!({ "document": "site" })),
            tool_call(3, "get_selection", json!({})),
            tool_call(4, "get_document", json!({})),
        ];
        let answers = drive(&lines, &mut hosts, None);
        let ids: Vec<&Value> = answers.iter().map(|a| &a["id"]).collect();
        assert_eq!(ids, [&json!(1), &json!(2), &json!(3), &json!(4)]);
        assert!(
            answers.iter().all(|a| a.get("error").is_none()),
            "{answers:?}"
        );
        assert!(
            hosts
                .methods(1)
                .starts_with(&["initialize".into(), "notifications/initialized".into()]),
            "the handshake is replayed on the window: {:?}",
            hosts.methods(1)
        );
        assert_eq!(
            hosts.calls(1),
            [
                (
                    "use_document".to_owned(),
                    json!({ "document": "site", "restore": true })
                ),
                ("get_document".to_owned(), json!({})),
            ],
            "the window's calls"
        );
    }

    /// A document named with `--document` is restored before the first call, so
    /// the agent need not call `use_document`.
    #[test]
    fn a_named_document_is_restored_before_the_first_call() {
        let mut hosts = Fake::new(vec![Scripted {
            host: Host::Window,
            drop_on: None,
            window_on: None,
        }]);
        let [init, done] = handshake();
        let lines = [init, done, tool_call(2, "get_selection", json!({}))];
        drive(&lines, &mut hosts, Some("/work/site.ondin"));
        let calls = hosts.calls(0);
        assert_eq!(calls[0].0, "use_document");
        assert_eq!(calls[0].1["document"], "/work/site.ondin");
        assert_eq!(calls[1].0, "get_selection");
    }

    /// A call whose answer was lost is answered as maybe applied and never sent
    /// again; the next request reaches a new host.
    ///
    /// flip: `carry` retrying a call on `Broke::Unanswered` → fails at "never
    /// sent again".
    #[test]
    fn a_lost_answer_is_never_sent_again() {
        let mut hosts = Fake::new(vec![
            Scripted {
                host: Host::Serve,
                drop_on: Some("set_name"),
                window_on: None,
            },
            Scripted {
                host: Host::Serve,
                drop_on: None,
                window_on: None,
            },
        ]);
        let [init, done] = handshake();
        let lines = [
            init,
            done,
            tool_call(2, "set_name", json!({ "node": "1:2", "name": "Hero" })),
            tool_call(3, "get_selection", json!({})),
        ];
        let answers = drive(&lines, &mut hosts, None);
        let resent = hosts.calls(1).iter().any(|(t, _)| t == "set_name");
        assert!(!resent, "never sent again: {:?}", hosts.calls(1));
        let lost = &answers[1];
        assert_eq!(lost["id"], 2);
        let why = lost["error"]["message"].as_str().unwrap_or_default();
        assert!(why.contains("may have applied"), "{lost}");
        assert_eq!(answers[2]["id"], 3);
        assert!(answers[2].get("error").is_none());
    }
}

//! Another tool's file read in a child process: this binary run as `ondin
//! reader`, handed the file on standard input, answering on standard output.
//!
//! A reader that overflows its stack or fails an allocation aborts its process,
//! and no `catch_unwind` sees either. Each reader carries its own bounds; this is
//! the guard for the input those bounds miss, which then costs the import and
//! not the app. The child only parses: a Figma file is fetched by the app, so
//! the token never reaches it.

use crate::panels::import_files::{self, Kind, ReportRow};
use ondin_core::foreign::Part;
use ondin_export::pdf_in::{PdfImport, PdfInError, PdfPage};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read as _, Write as _};

/// The subcommand that runs [`serve`]; not in the usage, since only the app
/// itself runs it.
pub(crate) const SUBCOMMAND: &str = "reader";

/// The answer's first bytes, so a test harness's own output ahead of it is
/// skipped and a child that died before answering is told from one that did.
const MAGIC: &[u8] = b"\nONDIN-READER-1\n";

/// What the child is asked to read.
pub(crate) enum Request {
    Sketch(Vec<u8>),
    Penpot(Vec<u8>),
    /// A fetched Figma file and its pictures, by the names `figma_in` asks for.
    Figma(BTreeMap<String, Vec<u8>>),
    Pdf(Vec<u8>),
    /// A child that dies before answering (tests only).
    #[cfg(test)]
    Abort,
}

/// Why the child gave no reading.
#[derive(Debug, PartialEq)]
pub(crate) enum Failure {
    /// The reader refused the file, saying why in its own words.
    Refused(String),
    /// The child died before answering, or could not be started.
    Stopped(String),
}

impl Failure {
    /// The card's line for a failed read.
    pub(crate) fn message(self) -> String {
        match self {
            Self::Refused(why) => why,
            Self::Stopped(why) => format!("Could not be read: {why}"),
        }
    }
}

/// Read a Sketch, Penpot or fetched Figma file in a child process.
pub(crate) fn foreign(request: Request) -> Result<import_files::Read, Failure> {
    let answer = run(&request)?;
    let (meta, doc) = split(&answer).ok_or_else(garbled)?;
    let meta: ForeignMeta = serde_json::from_slice(meta).map_err(|_| garbled())?;
    // Loaded here as any document is, so its limits hold whatever the child did.
    let doc = ondin_core::io::load(doc)
        .map_err(|e| Failure::Refused(format!("Could not be read: {e}")))?;
    let report = meta
        .report
        .into_iter()
        .map(|(part, what, count, how)| (part_of(part), ReportRow { what, count, how }))
        .collect();
    Ok((doc, report, meta.families, meta.name))
}

/// Draw a PDF's pages as SVG (`ondin_export::pdf_in::import`) in a child
/// process: hayro rasterises shadings and decodes pictures there.
pub(crate) fn pdf(bytes: Vec<u8>) -> Result<Result<PdfImport, PdfInError>, Failure> {
    let answer = run(&Request::Pdf(bytes))?;
    let (meta, _) = split(&answer).ok_or_else(garbled)?;
    let meta: PdfMeta = serde_json::from_slice(meta).map_err(|_| garbled())?;
    Ok(match meta {
        PdfMeta::Read {
            pages,
            left_out,
            too_large,
        } => Ok(PdfImport {
            pages: pages
                .into_iter()
                .map(|(svg, w, h)| PdfPage {
                    svg,
                    size: ondin_core::kurbo::Size::new(w, h),
                })
                .collect(),
            left_out,
            too_large,
        }),
        PdfMeta::Encrypted => Err(PdfInError::Encrypted),
        PdfMeta::Unreadable => Err(PdfInError::Unreadable),
        PdfMeta::TooLarge => Err(PdfInError::TooLarge),
    })
}

/// What a foreign reading carries beside its document.
#[derive(Serialize, Deserialize)]
struct ForeignMeta {
    name: Option<String>,
    /// `(part, what, count, how)`, the part by [`part_code`].
    report: Vec<(u8, String, String, String)>,
    families: Vec<(String, usize)>,
}

/// A PDF's reading, whole: its pages are SVG text.
#[derive(Serialize, Deserialize)]
enum PdfMeta {
    Read {
        /// `(svg, width, height)`.
        pages: Vec<(String, f64, f64)>,
        left_out: usize,
        too_large: usize,
    },
    Encrypted,
    Unreadable,
    TooLarge,
}

fn part_code(part: Part) -> u8 {
    match part {
        Part::Layers => 0,
        Part::Components => 1,
        Part::Pictures => 2,
        Part::Styles => 3,
        Part::Variables => 4,
    }
}

fn part_of(code: u8) -> Part {
    match code {
        1 => Part::Components,
        2 => Part::Pictures,
        3 => Part::Styles,
        4 => Part::Variables,
        _ => Part::Layers,
    }
}

fn garbled() -> Failure {
    Failure::Stopped("the reader's answer did not arrive whole".into())
}

/// The child's command line: this binary, or in a test the test binary
/// re-running its `child` test, which serves.
fn command() -> std::io::Result<std::process::Command> {
    let mut cmd = std::process::Command::new(std::env::current_exe()?);
    #[cfg(test)]
    cmd.args(["--exact", "reader::tests::child", "--nocapture"])
        .args(["--test-threads", "1"])
        .env(tests::CHILD_ENV, "1");
    #[cfg(not(test))]
    cmd.arg(SUBCOMMAND);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        // A debug build is a console program, which would open a window of its own.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    Ok(cmd)
}

/// Run `request` in a child and return the answer after [`MAGIC`].
fn run(request: &Request) -> Result<Vec<u8>, Failure> {
    use std::process::Stdio;
    let stopped = |e: std::io::Error| Failure::Stopped(format!("no reader could be started ({e})"));
    let mut child = command()
        .map_err(stopped)?
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(stopped)?;
    let _tie = tie_to_this_process(&child);
    let input = child.stdin.take().expect("piped");
    let mut output = child.stdout.take().expect("piped");
    // The child reads the whole request before it writes, but a child that dies
    // mid-request must not leave this side blocked on a full pipe.
    let mut answer = Vec::new();
    let read = std::thread::scope(|s| {
        s.spawn(move || {
            // Dropping `input` when written tells the child the request is whole.
            let _ = encode(request, &mut std::io::BufWriter::new(input));
        });
        output.read_to_end(&mut answer)
    });
    let status = child.wait();
    let died = || {
        Failure::Stopped(
            "the reader stopped on it, so the file may be damaged or built to crash a reader"
                .into(),
        )
    };
    if read.is_err() || !status.as_ref().is_ok_and(|s| s.success()) {
        log::warn!(target: "ondin", "reader child ended with {status:?}");
        return Err(died());
    }
    let Some(at) = answer.windows(MAGIC.len()).position(|w| w == MAGIC) else {
        return Err(died());
    };
    let mut answer = answer.split_off(at + MAGIC.len());
    match answer.first() {
        Some(0) => Ok(answer.split_off(1)),
        Some(1) => Err(Failure::Refused(
            String::from_utf8_lossy(&answer[1..]).into_owned(),
        )),
        _ => Err(garbled()),
    }
}

/// The child in a job object that ends it when this process ends, so a reader
/// left running by a closed card or a quit does not outlive the app. The job
/// ends the child when the guard drops, which is after it has answered.
#[cfg(windows)]
fn tie_to_this_process(child: &std::process::Child) -> Option<JobGuard> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    // SAFETY: plain Win32 calls on a handle this function creates and on the
    // child's, borrowed for the call; the guard owns the job handle.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job == 0 {
            return None;
        }
        let guard = JobGuard(job);
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let set = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if set == 0 || AssignProcessToJobObject(job, child.as_raw_handle() as _) == 0 {
            return None;
        }
        Some(guard)
    }
}

/// A job object's handle, closed on drop.
#[cfg(windows)]
struct JobGuard(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Drop for JobGuard {
    fn drop(&mut self) {
        // SAFETY: the handle came from `CreateJobObjectW` and is closed once.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

/// Off Windows the child ends when its reading does; nothing ties it sooner.
#[cfg(not(windows))]
fn tie_to_this_process(_child: &std::process::Child) -> Option<()> {
    None
}

/// The child's side: read one request from standard input, answer it on
/// standard output. Returns the process's exit code.
pub(crate) fn serve() -> i32 {
    let mut bytes = Vec::new();
    if std::io::stdin().lock().read_to_end(&mut bytes).is_err() {
        return 2;
    }
    let Some(request) = decode(&bytes) else {
        return 2;
    };
    drop(bytes);
    let answer = answer(request);
    let mut out = std::io::stdout().lock();
    if out
        .write_all(MAGIC)
        .and_then(|()| out.write_all(&answer))
        .and_then(|()| out.flush())
        .is_err()
    {
        return 2;
    }
    0
}

/// `request` read here, as the status byte and what follows it.
fn answer(request: Request) -> Vec<u8> {
    let foreign = |read: Result<import_files::Read, String>| match read {
        Ok((doc, report, families, name)) => match ondin_core::io::save(&doc) {
            Ok(doc) => {
                let meta = ForeignMeta {
                    name,
                    report: report
                        .into_iter()
                        .map(|(part, r)| (part_code(part), r.what, r.count, r.how))
                        .collect(),
                    families,
                };
                ok(&serde_json::to_vec(&meta).unwrap_or_default(), &doc)
            }
            Err(e) => refused(&format!("Could not be read: {e}")),
        },
        Err(why) => refused(&why),
    };
    match request {
        Request::Sketch(bytes) => foreign(import_files::read_foreign_here(Kind::Sketch, &bytes)),
        Request::Penpot(bytes) => foreign(import_files::read_foreign_here(Kind::Penpot, &bytes)),
        Request::Figma(files) => foreign(import_files::read_figma_here(files)),
        Request::Pdf(bytes) => {
            let meta = match ondin_export::pdf_in::import(bytes) {
                Ok(pdf) => PdfMeta::Read {
                    pages: pdf
                        .pages
                        .into_iter()
                        .map(|p| (p.svg, p.size.width, p.size.height))
                        .collect(),
                    left_out: pdf.left_out,
                    too_large: pdf.too_large,
                },
                Err(PdfInError::Encrypted) => PdfMeta::Encrypted,
                Err(PdfInError::Unreadable) => PdfMeta::Unreadable,
                Err(PdfInError::TooLarge) => PdfMeta::TooLarge,
            };
            ok(&serde_json::to_vec(&meta).unwrap_or_default(), &[])
        }
        #[cfg(test)]
        Request::Abort => std::process::abort(),
    }
}

fn ok(meta: &[u8], doc: &[u8]) -> Vec<u8> {
    let mut out = vec![0];
    put(&mut out, meta);
    put(&mut out, doc);
    out
}

fn refused(why: &str) -> Vec<u8> {
    let mut out = vec![1];
    out.extend_from_slice(why.as_bytes());
    out
}

/// An answer's two length-prefixed parts.
fn split(mut answer: &[u8]) -> Option<(&[u8], &[u8])> {
    let meta = take(&mut answer)?;
    let doc = take(&mut answer)?;
    Some((meta, doc))
}

fn put(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(bytes);
}

fn take<'a>(from: &mut &'a [u8]) -> Option<&'a [u8]> {
    let (len, rest) = from.split_first_chunk::<8>()?;
    let len = usize::try_from(u64::from_le_bytes(*len)).ok()?;
    if len > rest.len() {
        return None;
    }
    let (bytes, rest) = rest.split_at(len);
    *from = rest;
    Some(bytes)
}

/// A request as a tag byte and its named entries, written piece by piece so a
/// large file is not copied whole first.
fn encode(request: &Request, out: &mut impl std::io::Write) -> std::io::Result<()> {
    let (tag, entries): (u8, Vec<(&str, &[u8])>) = match request {
        Request::Sketch(b) => (b'S', vec![("", b)]),
        Request::Penpot(b) => (b'P', vec![("", b)]),
        Request::Figma(files) => (
            b'F',
            files
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_slice()))
                .collect(),
        ),
        Request::Pdf(b) => (b'D', vec![("", b)]),
        #[cfg(test)]
        Request::Abort => (b'!', Vec::new()),
    };
    out.write_all(&[tag])?;
    out.write_all(&(entries.len() as u64).to_le_bytes())?;
    for (name, bytes) in entries {
        for part in [name.as_bytes(), bytes] {
            out.write_all(&(part.len() as u64).to_le_bytes())?;
            out.write_all(part)?;
        }
    }
    out.flush()
}

fn decode(bytes: &[u8]) -> Option<Request> {
    let (&tag, mut rest) = bytes.split_first()?;
    let (count, after) = rest.split_first_chunk::<8>()?;
    rest = after;
    let mut entries = BTreeMap::new();
    for _ in 0..u64::from_le_bytes(*count) {
        let name = String::from_utf8(take(&mut rest)?.to_vec()).ok()?;
        entries.insert(name, take(&mut rest)?.to_vec());
    }
    let mut one = || entries.remove("");
    Some(match tag {
        b'S' => Request::Sketch(one()?),
        b'P' => Request::Penpot(one()?),
        b'D' => Request::Pdf(one()?),
        b'F' => Request::Figma(entries),
        #[cfg(test)]
        b'!' => Request::Abort,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The variable that tells the test binary's `child` test to serve.
    pub(super) const CHILD_ENV: &str = "ONDIN_READER_CHILD";

    /// Not a test: the child that `command` runs in a test build. It serves only
    /// when the parent's variable is set, and exits before the harness reports.
    #[test]
    fn child() {
        if std::env::var_os(CHILD_ENV).is_some() {
            std::process::exit(serve());
        }
    }

    /// A child that dies before answering is a failed read, not the app's
    /// death. flip: `run` answering in this process instead of a child → the
    /// test binary aborts.
    #[test]
    fn a_child_that_dies_is_a_stopped_read() {
        let got = run(&Request::Abort);
        assert!(matches!(got, Err(Failure::Stopped(_))), "{got:?}");
    }

    /// A refusal crosses in the reader's own words.
    #[test]
    fn a_refusal_keeps_its_words() {
        let Err(Failure::Refused(why)) = foreign(Request::Sketch(b"not a zip".to_vec())) else {
            panic!("refused");
        };
        assert_eq!(why, "Not a Sketch document: it does not open as one");
    }

    /// A request survives its encoding, entries and all.
    #[test]
    fn a_request_round_trips() {
        let files = BTreeMap::from([
            ("file.json".to_owned(), b"{}".to_vec()),
            ("images/a".to_owned(), vec![0, 1, 2]),
        ]);
        let encoded = |r: &Request| {
            let mut out = Vec::new();
            encode(r, &mut out).expect("a Vec takes it");
            out
        };
        let Some(Request::Figma(back)) = decode(&encoded(&Request::Figma(files.clone()))) else {
            panic!("a Figma request");
        };
        assert_eq!(back, files);
        assert!(
            decode(&encoded(&Request::Pdf(vec![7])))
                .is_some_and(|r| matches!(r, Request::Pdf(b) if b == [7]))
        );
        assert!(decode(b"S\x05").is_none(), "a cut request is refused");
    }
}

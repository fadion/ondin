//! ondin — the `ondin` binary (§9).
//!
//! Subcommands — **fenced, because rustdoc reads a bare usage line as markup**:
//! `[gui]` is an intra-doc link and `<file>` an unclosed HTML tag, which is three
//! of this crate's doc warnings for a block that is not prose at all (§15 D296).
//!
//! ```text
//! ondin [gui]                    launch the GUI shell (default)
//! ondin export <file> [--svg|--png|--json] [-o out]
//!            [--scale <n> | --width <px> | --height <px>]
//!                                render a document with no GPU and no window
//!                                the three sizes are --png only, and one at a time
//! ondin export <file> --all [--scale-override <2x|512w|512h>] [--folders] [-o dir]
//!                                run the document's own export settings (§7),
//!                                optionally forcing every row to one size
//! ondin serve <file>             headless: MCP endpoint over a document (§8.2)
//! ondin mcp-proxy                stdio↔socket bridge for agents (§8.1)
//! ```
//!
//! `export` runs entirely on `ondin-core` + the CPU renderer, so it works on a
//! CI box with no display and no graphics driver — the property the headless
//! core invariant exists to buy. `serve`/`mcp-proxy` land at M6.
//!
//! The rustdoc gate, and why one of the three lints is allowed: see `ondin_core`'s
//! crate doc (§15 D296). **This crate is where it pays** — it holds the largest
//! modules and the densest doc comments, and it is where the gate found a
//! paragraph asserting a contrast between two functions that had stopped being
//! true (`panels::export::export_menu_popup`).
#![deny(rustdoc::broken_intra_doc_links, rustdoc::invalid_html_tags)]
#![allow(rustdoc::private_intra_doc_links)]

mod app;
mod atomic;
mod canvas;
mod cursor;
mod expr;
mod fonts;
mod grid;
mod input;
// **The receipt is spent** (§15 D699, `[S16.5-L3-04]`). It read *"unwired on
// purpose … delete this attribute the moment the first real caller lands —
// leaving it is how an abandoned module comes to look finished"*, and the
// dashboard that calls this module landed on 2026-08-26 (§15 D362–D366). The
// condition ended; the instruction had not been followed, and the blanket
// `allow` was shielding every item in a module that is now thoroughly wired.
//
// ⚠️ **It was masking exactly one thing** — measured with the attribute off,
// under both `cargo check -p ondin-app` and `--all-targets`: `store::versions`,
// which has no production caller anywhere and is read only from tests. It
// carries its own `#[allow(dead_code)]` now, with the argument on it, so the
// plain build is the gate that catches the *next* one — checked with a
// throwaway `fn`, which the plain build duly reported.
mod library;
mod measure;
mod menu;
mod panels;
mod prefs;
mod preview;
mod rulers;
mod session;
mod settings;
mod snap;
mod theme;
mod thumbs;
mod tools;
mod ui;
mod view;

use ondin_core::ExportScale;

#[derive(Debug, PartialEq)]
enum Command {
    Gui,
    Serve {
        file: String,
    },
    Export {
        file: String,
        format: ExportFormat,
        out: Option<String>,
        /// How big the raster comes out — `--scale 2`, `--width 512`, `--height
        /// 512` (§15 D360, D361). The panel's three questions reaching the one
        /// export route that could ask none of them.
        ///
        /// Not an `Option`, because the PNG arm needs an answer either way and the
        /// parser has already refused all three for the two formats that cannot use
        /// one — so `Times(1.0)` here means "unscaled", never "unasked".
        scale: ExportScale,
    },
    /// Run the document's **own** export settings (§7) — every layer that carries
    /// any, at the formats and sizes the file says, into a folder.
    ///
    /// **The reason export settings are document state.** With them in the file
    /// rather than in a dialog, regenerating a project's assets is a command a
    /// build script can run: no window, no GPU, nobody steering a folder picker.
    /// It is the same `ondin_export::plan` the panel's button goes through, so the
    /// bytes are the bytes a designer would have got by clicking.
    ExportAll {
        file: String,
        out: Option<String>,
        folders: bool,
        /// `--scale-override`: force every spec in the file to this size (§15
        /// D361). The document still decides *which* layers export and at what
        /// formats; this decides only how big, which is the one setting a build
        /// step plausibly wants to say from outside the file.
        scale_override: Option<ExportScale>,
    },
    McpProxy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExportFormat {
    Svg,
    Png,
    Json,
}

impl ExportFormat {
    fn extension(self) -> &'static str {
        match self {
            ExportFormat::Svg => "svg",
            ExportFormat::Png => "png",
            ExportFormat::Json => "json",
        }
    }
}

fn parse(args: &[String]) -> Result<Command, String> {
    match args.first().map(String::as_str) {
        None | Some("gui") => Ok(Command::Gui),
        Some("serve") => args
            .get(1)
            .map(|file| Command::Serve { file: file.clone() })
            .ok_or_else(|| "usage: ondin serve <file>".to_string()),
        Some("export") => parse_export(&args[1..]),
        Some("mcp-proxy") => Ok(Command::McpProxy),
        Some(other) => Err(format!("unknown command: {other}")),
    }
}

fn parse_export(args: &[String]) -> Result<Command, String> {
    const USAGE: &str = "usage: ondin export <file.ondin> [--svg|--png|--json] \
         [--scale <n>|--width <px>|--height <px>] [-o <out>]\n       \
         ondin export <file.ondin> --all [--scale-override <2x|512w|512h>] [--folders] [-o <dir>]";
    let mut file = None;
    let mut format = None;
    let mut out = None;
    let mut all = false;
    let mut folders = false;
    let mut scale: Option<(&str, ExportScale)> = None;
    let mut over = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--svg" => format = Some(ExportFormat::Svg),
            "--png" => format = Some(ExportFormat::Png),
            "--json" => format = Some(ExportFormat::Json),
            "--all" => all = true,
            "--folders" => folders = true,
            // **One value each, and mutually exclusive**, because they are three
            // ways of answering one question and a command that took two would have
            // to pick. The flag that was typed is carried with the value so the
            // refusals below can name it back.
            flag @ ("--scale" | "--width" | "--height") => {
                i += 1;
                let text = args.get(i).ok_or_else(|| USAGE.to_string())?;
                // Zero and negative are rejected here rather than left to
                // `factor`'s floor, which exists to keep the renderer safe and
                // would turn `--scale 0` into a one-pixel file nobody asked for.
                let asked = match flag {
                    "--width" => text
                        .parse()
                        .ok()
                        .filter(|px| *px > 0)
                        .map(ExportScale::Width),
                    "--height" => text
                        .parse()
                        .ok()
                        .filter(|px| *px > 0)
                        .map(ExportScale::Height),
                    _ => text
                        .parse()
                        .ok()
                        .filter(|n: &f32| n.is_finite() && *n > 0.0)
                        .map(ExportScale::Times),
                }
                .ok_or_else(|| format!("{flag} wants a positive number, not {text:?}\n{USAGE}"))?;
                if let Some((had, _)) = scale {
                    return Err(format!(
                        "{had} and {flag} are two answers to one question — pick one\n{USAGE}"
                    ));
                }
                scale = Some((flag, asked));
            }
            "--scale-override" => {
                i += 1;
                let text = args.get(i).ok_or_else(|| USAGE.to_string())?;
                // The export row's own vocabulary read backwards, so what a
                // designer sees on the row is what a build script types.
                over = Some(ExportScale::from_label(text).ok_or_else(|| {
                    format!("--scale-override wants 2x, 512w or 512h, not {text:?}\n{USAGE}")
                })?);
            }
            "-o" | "--out" => {
                i += 1;
                let name = args.get(i).ok_or_else(|| USAGE.to_string())?;
                // ⚠️ **A flag is not a filename** (§15 D670, `[S16.5-L1-03]`).
                // Taking the next token unconditionally means `-o --png` sets the
                // output *to* `--png`, and the token is then never seen at the top
                // of this loop — so the `other if other.starts_with('-')` arm below
                // cannot object, the format falls back to the SVG default, and the
                // command writes SVG bytes into a file literally called `--png` and
                // prints `wrote --png`. This is the same refusal the size flags
                // already make with their `ok_or_else`, one argument earlier.
                if name.starts_with('-') {
                    return Err(format!(
                        "-o wants a filename, not the option {name}\n{USAGE}"
                    ));
                }
                out = Some(name.clone());
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown option {other}\n{USAGE}"));
            }
            // **A second document is refused rather than taken** (§15 D670). The
            // arm used to overwrite, so `ondin export a.ondin b.ondin --png`
            // rendered `b.ondin` and never mentioned `a.ondin` — a build step whose
            // glob matched twice exports the wrong document into the right name and
            // has no way to notice. Both names are in the message, because the
            // interesting half is *which* two.
            other if file.is_some() => {
                return Err(format!(
                    "export takes one document; got {} and {other}\n{USAGE}",
                    file.as_deref().unwrap_or_default()
                ));
            }
            other => file = Some(other.to_string()),
        }
        i += 1;
    }
    if all {
        // **A different command, not a fourth format**, and refusing the
        // combination is what says so: `--all` writes *many* files at whatever
        // formats the document names, so `-o` is a folder rather than a filename
        // and `--png` would be an instruction it has nowhere to apply.
        if format.is_some() {
            return Err(format!(
                "--all runs the document's own export settings, so it takes no format\n{USAGE}"
            ));
        }
        // The size flags are refused for the same reason and then **point at the
        // one that is not refused**: overriding every row is a coherent thing to
        // want, and it has its own flag precisely because it is not what
        // `--scale` means anywhere else — there it *is* the size, here it
        // overrules a size the file already carries.
        if let Some((had, _)) = scale {
            return Err(format!(
                "--all takes its sizes from the document; use --scale-override to force them all\n\
                 ({had} sets the size of a one-off export)\n{USAGE}"
            ));
        }
        return Ok(Command::ExportAll {
            file: file.ok_or_else(|| USAGE.to_string())?,
            out,
            folders,
            scale_override: over,
        });
    }
    if folders {
        return Err(format!("--folders applies only to --all\n{USAGE}"));
    }
    if over.is_some() {
        return Err(format!(
            "--scale-override applies only to --all; a one-off export takes --scale\n{USAGE}"
        ));
    }
    // SVG is the default: it is the semantic format and the one that needs
    // no viewport arguments.
    let format = format.unwrap_or(ExportFormat::Svg);
    // **Refused rather than ignored**, which is this parser's rule everywhere else
    // (`--all --png`, `--folders` alone) and matters most here: a build script that
    // believes it asked for 2× and got 1× has no way to notice. The message names
    // the rule rather than only the flag, because the commonest way to hit it is to
    // type no format at all and inherit the SVG default.
    if let Some((had, _)) = scale
        && format != ExportFormat::Png
    {
        return Err(format!(
            "{had} is a raster size, so it applies only to --png\n{USAGE}"
        ));
    }
    Ok(Command::Export {
        file: file.ok_or_else(|| USAGE.to_string())?,
        format,
        out,
        scale: scale.map_or(ExportScale::Times(1.0), |(_, s)| s),
    })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match parse(&args) {
        Ok(Command::Gui) => run_gui().map_err(|e| format!("ondin gui failed: {e}")),
        Ok(Command::Export {
            file,
            format,
            out,
            scale,
        }) => run_export(&file, format, out.as_deref(), scale),
        Ok(Command::ExportAll {
            file,
            out,
            folders,
            scale_override,
        }) => run_export_all(&file, out.as_deref(), folders, scale_override),
        Ok(Command::Serve { file }) => Err(format!("ondin serve {file}: not yet implemented (M6)")),
        Ok(Command::McpProxy) => Err("ondin mcp-proxy: not yet implemented (M6)".to_string()),
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(2);
        }
    };
    if let Err(msg) = result {
        eprintln!("{msg}");
        std::process::exit(1);
    }
}

/// What to say when the raster limit took a scale down, or `None` when it did not.
///
/// **`viewport_at` clamps correctly and silently.** It caps the *multiplier*
/// rather than each side, so an impossible ask comes back as the same picture
/// smaller rather than the same picture squashed — but silence is the wrong half
/// of that for a command a build script runs: asking for 8× on a poster and
/// getting 3.4× is otherwise unnoticeable. `plan` already reports what it clamped;
/// this is the ad-hoc path catching up with it.
///
/// A free function returning the sentence rather than a `fn` that prints it, so
/// the decision — *was this a clamp* — is assertable without capturing stderr.
///
/// It names the size in the **spec's own vocabulary** (`2x`, `512w`) rather than
/// the multiplier it became, because a `--width 20000` reduced to 3.4× is a
/// sentence about a number the person never typed.
///
/// ⚠️ **`>=` rather than `==` is deliberate and currently unreachable**, which was
/// found by flipping it and watching nothing fail. The intent it states is that a
/// result *bigger* than the arithmetic is not a clamp — `px` floors every side at
/// one pixel, so a sub-pixel ask is served rather than reduced. But `px` takes
/// `ceil` of the same positive product this does, and the ceiling of a positive is
/// never below 1, so `got` can never actually exceed `asked`: the two spellings
/// agree on every input the guards above can produce. Kept in the honest form
/// because it is the shape that survives a change to `px`'s clamp, and recorded
/// here so the next reader does not mistake an unreachable branch for a tested one.
fn clamp_note(
    view: ondin_core::kurbo::Rect,
    scale: ExportScale,
    got: (u32, u32),
) -> Option<String> {
    let w = view.width().max(1.0);
    let h = view.height().max(1.0);
    let f = scale.factor(w, h);
    let (asked_w, asked_h) = ((w * f).ceil(), (h * f).ceil());
    if f64::from(got.0) >= asked_w && f64::from(got.1) >= asked_h {
        return None;
    }
    Some(format!(
        "{} wants {asked_w:.0} × {asked_h:.0} px, past the {} limit — wrote {} × {} ({:.3}×)",
        scale.label(),
        ondin_export::png::MAX_RASTER_SIDE,
        got.0,
        got.1,
        f64::from(got.0) / w,
    ))
}

/// Headless export (§7). Loads the document, resolves it, and writes one of the
/// three export formats — no window, no GPU, no font service (text uses the
/// bundled fallback, which is what makes the output deterministic in CI).
///
/// `scale` reaches only the PNG arm; the parser has already refused it for the
/// other two, so there is nothing to ignore here. The **output path is not
/// decorated with it** — `plan`'s `@2x` marker disambiguates a *set* of files
/// written in one go, and letting a flag rename this command's one output would
/// make `-o` the only way to know where the bytes went.
///
/// **`Width`/`Height` are resolved against the same box the render frames**, which
/// is why the multiplier is taken here and not in the parser: `--width 512` is a
/// different number on every document, and taking it anywhere else would mean
/// measuring the drawing twice and hoping the two agreed.
fn run_export(
    file: &str,
    format: ExportFormat,
    out: Option<&str>,
    scale: ExportScale,
) -> Result<(), String> {
    use ondin_core::Resolved;

    let bytes = std::fs::read(file).map_err(|e| format!("cannot read {file}: {e}"))?;
    let doc = ondin_core::io::load(&bytes).map_err(|e| format!("cannot load {file}: {e}"))?;
    let res = Resolved::rebuild(&doc);

    let out_path = out
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::Path::new(file).with_extension(format.extension()));

    let data = match format {
        ExportFormat::Svg => ondin_export::svg::svg(&doc, &res, None).into_bytes(),
        ExportFormat::Json => {
            let snap = ondin_export::snapshot(&doc, &res);
            serde_json::to_vec_pretty(&snap)
                .map_err(|e| format!("cannot serialize snapshot: {e}"))?
        }
        ExportFormat::Png => {
            // Frame the whole document, at `scale` device pixels per world unit.
            // The 512-square is this command's own answer to an empty document —
            // `extent` returns `None` rather than picking one, because the SVG
            // writer wants a different fallback — and the *floor* on a degenerate
            // box lives in `viewport_at`, where the division that needs it is.
            let view = ondin_export::extent(&res, &[doc.root()])
                .unwrap_or_else(|| ondin_core::kurbo::Rect::new(0.0, 0.0, 512.0, 512.0));
            let vp = ondin_export::png::viewport_at(
                view,
                scale.factor(view.width().max(1.0), view.height().max(1.0)),
            );
            if let Some(note) = clamp_note(view, scale, vp.pixel_size) {
                eprintln!("{note}");
            }
            ondin_export::png::png(&doc, &res, &vp)
        }
    };

    std::fs::write(&out_path, data)
        .map_err(|e| format!("cannot write {}: {e}", out_path.display()))?;
    println!("wrote {}", out_path.display());
    Ok(())
}

/// Headless run of the document's **own** export settings (§7).
///
/// **Exit code 1 when a document has no export settings at all**, rather than a
/// cheerful zero over nothing written. A build step that silently succeeds
/// without producing its output is the failure that gets noticed three commits
/// later; a designer who has not set anything up yet gets the message in the same
/// breath.
///
/// The whole plan is rendered before anything is written, so a failure part way
/// through leaves a folder of files that all came from one state of the document.
fn run_export_all(
    file: &str,
    out: Option<&str>,
    folders: bool,
    scale_override: Option<ExportScale>,
) -> Result<(), String> {
    use ondin_core::Resolved;

    let bytes = std::fs::read(file).map_err(|e| format!("cannot read {file}: {e}"))?;
    let doc = ondin_core::io::load(&bytes).map_err(|e| format!("cannot load {file}: {e}"))?;
    let res = Resolved::rebuild(&doc);

    // Every layer carrying settings, in document order — the same set and the same
    // order the panel's *Export all* collects, so the two produce the same files.
    let mut subjects = Vec::new();
    let mut stack = vec![doc.root()];
    while let Some(id) = stack.pop() {
        let Some(node) = doc.get(id) else { continue };
        if !node.exports().is_empty() {
            subjects.push(id);
        }
        stack.extend(node.children().iter().copied());
    }
    // `all_in_document_order`, matching the panel exactly — see
    // `exportable_layers` and §15 D516. The CLI had the same defect by an
    // independent copy of this walk, which is why nothing had noticed: the two
    // front ends agreed about the wrong answer.
    let subjects = ondin_core::build::all_in_document_order(&doc, &subjects);
    if subjects.is_empty() {
        return Err(format!("{file}: no layer has export settings"));
    }

    let dir = out
        .map(std::path::PathBuf::from)
        .or_else(|| std::path::Path::new(file).parent().map(Into::into))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let plan = ondin_export::plan(
        &doc,
        &res,
        &subjects,
        ondin_export::PlanOptions {
            folders_from_names: folders,
            scale_override,
        },
    );

    let mut written = Vec::new();
    let mut skipped = 0usize;
    for f in &plan {
        match ondin_export::plan::render(&doc, &res, f) {
            Some(data) => written.push((dir.join(&f.path), data)),
            None => {
                eprintln!("skipped {} — that layer has no size", f.path);
                skipped += 1;
            }
        }
    }
    // ⚠️ **Every file skipped is a failure, and it was exit 0** (§15 D531).
    // `subjects.is_empty()` above fires on the set of *layers*, not on the set of
    // *files produced* — so a document whose only exporting layer has no ink (an
    // empty group, a fully hidden layer, a zero-size shape) carrying a **raster**
    // spec wrote nothing, did not even create the output directory, said so on
    // **stderr** where a build step does not read it, and returned `Ok(())`.
    // Which is the failure this function's own doc says the exit-1 path exists to
    // prevent, three paragraphs up: *"a cheerful zero over nothing written. A
    // build step that silently succeeds without producing its output is the
    // failure that gets noticed three commits later."*
    //
    // The raster arm is the reachable one: `png::raster_of` opens with
    // `extent(res, origins)?`, so an extent-less layer skips every raster spec,
    // while the SVG arm always returns `Some`.
    if written.is_empty() {
        return Err(format!(
            "{file}: every export was skipped — no layer with export settings has a size \
             ({skipped} skipped)"
        ));
    }
    // ⚠️ **The other no-dialog door, and §15 D636 did not reach it** (§15 D652,
    // `[S8.2-L5-07]`). D636's ruling is that the disclosure belongs to the door:
    // a *File* or an *Archive* was named in a save dialog and the OS has already
    // warned, and a door that writes with **no dialog at all** owes a count
    // instead. It gave the count to the panel's folder arm; this is the other
    // such door, and the worse of the two, because `dir` above falls back to
    // **the folder holding the `.ondin`** when `-o` is absent — in the library a
    // project folder the user keeps other things in. So `--all` on a document
    // with a layer named `logo` replaced a hand-authored `logo.svg` beside it and
    // said `wrote 1 file`.
    //
    // **Asked before the write**, which is the whole of it: afterwards every one
    // of these exists. Same sentence as `write_export`'s, deliberately — the two
    // walks are separate implementations (D626) and this is the second time that
    // has cost a fix.
    let replaced = write_planned(&written)?;
    println!(
        "{}",
        export_all_report(written.len(), replaced, skipped, &dir)
    );
    Ok(())
}

/// Write a rendered plan to disk, answering **how many of those paths already
/// held a file** (§15 D652, `[S8.2-L5-07]`).
///
/// **Asked before the write, which is the whole of it**: afterwards every one of
/// them exists. The same sentence `panels::export::write_export`'s folder arm
/// carries, and deliberately the same shape — the two walks are separate
/// implementations (§15 D626), and this is the second time that has cost a fix
/// twice over.
///
/// **Lifted out of `run_export_all` so the count can be asserted** (§15 D269's
/// shape): the caller's only report of it is a `println!`, which a test cannot
/// read.
fn write_planned(files: &[(std::path::PathBuf, Vec<u8>)]) -> Result<usize, String> {
    let mut replaced = 0usize;
    for (path, data) in files {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        if path.exists() {
            replaced += 1;
        }
        std::fs::write(path, data).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    }
    Ok(replaced)
}

/// What `--all` says when it is done.
///
/// **The count first, the loss next, the path last** is the panel's rule and not
/// this one's: a terminal line is not truncated, so the folder stays where a
/// reader looks for it and the two parenthetical counts follow it. What the two
/// doors do share is *which* counts are owed — `(n replaced)` is D636's, and it
/// belongs to any door that writes without a dialog.
fn export_all_report(
    written: usize,
    replaced: usize,
    skipped: usize,
    dir: &std::path::Path,
) -> String {
    let mut out = format!(
        "wrote {} file{} to {}",
        written,
        if written == 1 { "" } else { "s" },
        dir.display(),
    );
    if replaced > 0 {
        out.push_str(&format!(" ({replaced} replaced)"));
    }
    if skipped > 0 {
        out.push_str(&format!(" ({skipped} skipped)"));
    }
    out
}

/// The running window's icon — title bar, taskbar button, Alt-Tab.
///
/// Distinct from the icon on `ondin.exe` itself, which `build.rs` compiles into
/// the binary's resource table as a multi-size `.ico`; winit has no way to read
/// that container, and Windows has no window to read this one from. Both are
/// needed, and neither covers the other's surfaces: with only the resource icon
/// the taskbar shows the generic application glyph, because winit registers its
/// window class with `hIcon: 0` rather than falling back to the executable's.
///
/// One size has to be chosen here, since `WM_SETICON` gets the same pixels for
/// `ICON_BIG` and `ICON_SMALL` and Windows scales for everything below it. 256 is
/// the largest the set offers, so it is the one that has something left to give
/// at 200% display scaling; the cost is that the 16px title-bar copy is an 8×
/// downscale done by the OS rather than the artwork drawn at that size.
fn window_icon() -> eframe::egui::IconData {
    static PNG: &[u8] = include_bytes!("../../../icons/convertico-Ondin_256x256.png");
    // Compiled in, so this either works on every run or on none. The test below
    // holds the "every" end, which is what makes the panic honest.
    eframe::icon_data::from_png_bytes(PNG).expect("the bundled window icon is not a valid PNG")
}

fn run_gui() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_title("Ondin")
            .with_icon(window_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "Ondin",
        options,
        Box::new(|cc| Ok(Box::new(app::OndinApp::new(cc)))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `window_icon` panics on a bad PNG, and the bytes are compiled in, so the
    /// panic is reachable only through a build — a wrong path, a re-exported
    /// file, an `icons/` folder regenerated into a different format. This is the
    /// gate that catches it before a user does.
    ///
    /// It asserts the *artwork*, not just that something decoded: a transparent
    /// corner and an opaque point on the ring. A blank buffer passes neither, and
    /// `appstore.png` — the obvious wrong file to reach for, and the one sitting
    /// beside it — is an opaque 1024px square, so it fails both.
    #[test]
    fn window_icon_is_the_ondin_mark_at_the_size_windows_scales_from() {
        let icon = window_icon();
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);

        let alpha_at = |x: usize, y: usize| icon.rgba[(y * 256 + x) * 4 + 3];
        assert_eq!(alpha_at(0, 0), 0, "the rounded mark must not fill its box");
        assert_eq!(alpha_at(128, 128), 0, "the O is a ring, not a disc");
        assert_eq!(alpha_at(128, 32), 255, "the top of the ring is solid ink");
    }

    #[test]
    fn defaults_to_gui() {
        assert!(matches!(parse(&[]), Ok(Command::Gui)));
        assert!(matches!(parse(&["gui".into()]), Ok(Command::Gui)));
    }

    #[test]
    fn serve_requires_file() {
        assert!(parse(&["serve".into()]).is_err());
        assert!(matches!(
            parse(&["serve".into(), "a.ondin".into()]),
            Ok(Command::Serve { .. })
        ));
    }

    #[test]
    fn unknown_command_errors() {
        assert!(parse(&["frobnicate".into()]).is_err());
    }

    fn export(args: &[&str]) -> Result<Command, String> {
        let mut v = vec!["export".to_string()];
        v.extend(args.iter().map(|s| s.to_string()));
        parse(&v)
    }

    #[test]
    fn export_defaults_to_svg() {
        assert_eq!(
            export(&["a.ondin"]).unwrap(),
            Command::Export {
                file: "a.ondin".into(),
                format: ExportFormat::Svg,
                out: None,
                scale: ExportScale::Times(1.0),
            }
        );
    }

    #[test]
    fn export_accepts_a_format_and_an_output_path() {
        assert_eq!(
            export(&["a.ondin", "--png", "-o", "out.png"]).unwrap(),
            Command::Export {
                file: "a.ondin".into(),
                format: ExportFormat::Png,
                out: Some("out.png".into()),
                scale: ExportScale::Times(1.0),
            }
        );
        assert_eq!(
            export(&["--json", "a.ondin"]).unwrap(),
            Command::Export {
                file: "a.ondin".into(),
                format: ExportFormat::Json,
                out: None,
                scale: ExportScale::Times(1.0),
            }
        );
    }

    /// `--scale` is the density the panel's per-row combo already offers
    /// (`ExportScale::Times`), reaching the one export path that had no way to ask
    /// for it. The default is 1.0 and not an `Option`, because every arm below
    /// needs a number and only one of them can use it.
    ///
    /// **It is refused wherever it cannot apply rather than ignored**, which is
    /// this parser's rule everywhere else and the reason the flag is worth having
    /// at all: the failure it guards against is a build script that thinks it
    /// asked for 2×.
    #[test]
    fn scale_is_a_png_argument_and_is_refused_everywhere_else() {
        assert_eq!(
            export(&["a.ondin", "--png", "--scale", "2"]).unwrap(),
            Command::Export {
                file: "a.ondin".into(),
                format: ExportFormat::Png,
                out: None,
                scale: ExportScale::Times(2.0),
            }
        );
        // Fractional, because Figma's ladder starts at 0.5×.
        assert_eq!(
            export(&["a.ondin", "--png", "--scale", "0.5"]).unwrap(),
            Command::Export {
                file: "a.ondin".into(),
                format: ExportFormat::Png,
                out: None,
                scale: ExportScale::Times(0.5),
            }
        );

        // The three ways it cannot apply. The last is the one a person actually
        // hits: no format at all inherits the SVG default, so the message names
        // the rule rather than only the flag they typed.
        for args in [
            vec!["a.ondin", "--svg", "--scale", "2"],
            vec!["a.ondin", "--json", "--scale", "2"],
            vec!["a.ondin", "--scale", "2"],
            vec!["a.ondin", "--svg", "--width", "512"],
            vec!["a.ondin", "--height", "512"],
        ] {
            let err = export(&args).unwrap_err();
            assert!(err.contains("applies only to --png"), "{args:?}: {err}");
        }
        // And on `--all`, where every layer already carries its own — where the
        // message **names the flag that would have worked**, since forcing every
        // row is a coherent thing to have wanted.
        let err = export(&["a.ondin", "--all", "--scale", "2"]).unwrap_err();
        assert!(err.contains("--scale-override"), "{err}");

        // A number that cannot be a size is a parse error, not a silent 1× and not
        // a one-pixel file out of `factor`'s floor.
        for bad in ["0", "-2", "abc", "", "nan"] {
            assert!(
                export(&["a.ondin", "--png", "--scale", bad]).is_err(),
                "--scale {bad:?} was accepted"
            );
        }
        for flag in ["--scale", "--width", "--height"] {
            assert!(
                export(&["a.ondin", "--png", flag]).is_err(),
                "{flag} with nothing after it"
            );
            assert!(
                export(&["a.ondin", "--png", flag, "0"]).is_err(),
                "{flag} 0 was accepted"
            );
        }
    }

    /// `--width` and `--height` are the panel's other two questions: *whatever it
    /// takes to be this big*, where `--scale` is *the same drawing, denser*. They
    /// are three answers to one question, so two of them together is a refusal
    /// rather than a precedence rule nobody would remember.
    #[test]
    fn width_and_height_are_the_other_two_ways_to_ask_and_only_one_may_be_asked() {
        let png = |args: &[&str], want: ExportScale| {
            assert_eq!(
                export(args).unwrap(),
                Command::Export {
                    file: "a.ondin".into(),
                    format: ExportFormat::Png,
                    out: None,
                    scale: want,
                },
                "{args:?}"
            );
        };
        png(
            &["a.ondin", "--png", "--width", "512"],
            ExportScale::Width(512),
        );
        png(
            &["a.ondin", "--png", "--height", "1024"],
            ExportScale::Height(1024),
        );

        // Every unordered pair, because a rule that only fired one way round would
        // pass a test written in the order the author happened to think of.
        for pair in [
            ["--scale", "--width"],
            ["--width", "--scale"],
            ["--scale", "--height"],
            ["--height", "--scale"],
            ["--width", "--height"],
            ["--height", "--width"],
        ] {
            let err = export(&["a.ondin", "--png", pair[0], "2", pair[1], "2"]).unwrap_err();
            assert!(
                err.contains("two answers to one question"),
                "{pair:?}: {err}"
            );
            // The message names **both** flags, which is the whole of its use.
            assert!(err.contains(pair[0]) && err.contains(pair[1]), "{err}");
        }

        // The same flag twice is the same mistake, and is caught by the same guard
        // rather than silently taking the last one — the shape `--png --svg` does
        // take, and the one worth being different from here, since two sizes have
        // no ordering anyone would guess.
        assert!(
            export(&["a.ondin", "--png", "--scale", "2", "--scale", "3"]).is_err(),
            "--scale twice"
        );
    }

    /// `--scale-override` is the flag `--all` gets instead: the document still says
    /// which layers export and at what formats, and this says how big — the one
    /// setting a build step plausibly wants to say from outside the file.
    ///
    /// It takes the **export row's own vocabulary** (`ExportScale::from_label`), so
    /// what a designer reads off a row is what a script types, and one flag covers
    /// all three questions where the one-off export spends three.
    #[test]
    fn scale_override_belongs_to_all_and_speaks_the_rows_vocabulary() {
        let all = |args: &[&str], want: Option<ExportScale>| {
            assert_eq!(
                export(args).unwrap(),
                Command::ExportAll {
                    file: "a.ondin".into(),
                    out: None,
                    folders: false,
                    scale_override: want,
                },
                "{args:?}"
            );
        };
        all(&["a.ondin", "--all"], None);
        all(
            &["a.ondin", "--all", "--scale-override", "1x"],
            Some(ExportScale::Times(1.0)),
        );
        all(
            &["a.ondin", "--all", "--scale-override", "2"],
            Some(ExportScale::Times(2.0)),
        );
        all(
            &["a.ondin", "--all", "--scale-override", "512w"],
            Some(ExportScale::Width(512)),
        );

        // Not on a one-off, where `--scale` is the flag and overriding nothing is
        // what it would mean.
        let err = export(&["a.ondin", "--png", "--scale-override", "2x"]).unwrap_err();
        assert!(err.contains("applies only to --all"), "{err}");

        for bad in ["", "2px", "abc", "0x", "-1"] {
            assert!(
                export(&["a.ondin", "--all", "--scale-override", bad]).is_err(),
                "--scale-override {bad:?} was accepted"
            );
        }
    }

    #[test]
    fn export_rejects_missing_input_and_bad_flags() {
        assert!(export(&[]).is_err(), "no input file");
        assert!(export(&["a.ondin", "--jpeg"]).is_err(), "unknown format");
        assert!(export(&["a.ondin", "-o"]).is_err(), "-o without a value");

        // §15 D670, `[S16.5-L1-03]` — the two this parser used to accept in
        // silence, which is the one failure its own comment says it does not
        // make: *"a build script that believes it asked for 2× and got 1× has no
        // way to notice."*
        //
        // **Flip:** put back the unconditional `args.get(i)` in the `-o` arm and
        // the first fails; put back `other => file = Some(…)` without the
        // `file.is_some()` guard and the second fails. Both predicted correctly.
        // ⚠️ The `-o --png` case is worth reading rather than counting: without
        // the guard it does not merely mis-set the output, it **loses the format
        // flag entirely**, so the assertion is about a silent SVG, not about a
        // silly filename.
        assert!(
            export(&["a.ondin", "-o", "--png"]).is_err(),
            "-o swallowed the format flag"
        );
        assert!(
            export(&["a.ondin", "b.ondin"]).is_err(),
            "a second document overwrote the first"
        );
        // The order does not matter: the second name is refused wherever it sits.
        assert!(
            export(&["a.ondin", "--png", "b.ondin", "-o", "out.png"]).is_err(),
            "a second document after the flags"
        );
    }

    /// The headless path end to end: no window, no GPU, no font service.
    #[test]
    fn export_writes_each_format_without_a_display() {
        use ondin_core::kurbo::Size;
        use ondin_core::{Document, Fill, IdSource, NodeKind, Operation, Transaction};

        let mut ids = IdSource::new(0xE7);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let ab = ids.mint();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: ab,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(32.0, 24.0),
                },
                transform: None,
                name: None,
            },
            Operation::SetFills {
                id: ab,
                fills: vec![Fill {
                    brush: ondin_core::peniko::Brush::Solid(ondin_core::peniko::Color::from_rgba8(
                        10, 20, 30, 255,
                    )),
                    visible: true,
                }],
            },
        ]))
        .unwrap();

        let dir = std::env::temp_dir().join(format!("ondin-export-cli-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("doc.ondin");
        std::fs::write(&src, ondin_core::io::save(&doc).unwrap()).unwrap();
        let src = src.to_string_lossy().into_owned();

        for (format, ext) in [
            (ExportFormat::Svg, "svg"),
            (ExportFormat::Png, "png"),
            (ExportFormat::Json, "json"),
        ] {
            let out = dir.join(format!("out.{ext}"));
            run_export(
                &src,
                format,
                Some(&out.to_string_lossy()),
                ExportScale::Times(1.0),
            )
            .expect("export succeeds");
            let written = std::fs::read(&out).expect("output exists");
            assert!(!written.is_empty(), "{ext} export was empty");
        }

        // PNG really is a PNG, not an empty stub.
        let png = std::fs::read(dir.join("out.png")).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "PNG signature");
    }

    /// `--scale 2` doubles both sides of the raster, read out of the PNG header
    /// rather than out of the `Viewport` the code just built — the assertion has to
    /// survive the whole write, since the flag's only job is to change the bytes on
    /// disk.
    ///
    /// ⚠️ **The predicted flip was wrong about which half fails.** Pinning
    /// `run_export` back to `viewport_over` was expected to fail on the 2× file
    /// being too small; it fails on `2 * 1× == 2×` with *both* files identical,
    /// which is the better failure: it says the flag did nothing rather than that
    /// one number was off, and it would still bite if the 1× path broke too.
    ///
    /// ⚠️ **The pinned-axis pair is flip-checked against the axes being swapped**
    /// — `factor`'s `Width`/`Height` arms exchanged — and bites at `(128, 96)`
    /// against `(96, 72)`, on the `Width` line. It only bites because the fixture
    /// is 32×24: on a square one both spellings agree, and the test would have
    /// been about nothing.
    #[test]
    fn scale_multiplies_the_pixels_the_png_actually_carries() {
        use ondin_core::kurbo::Size;
        use ondin_core::{Document, Fill, IdSource, NodeKind, Operation, Transaction};

        let mut ids = IdSource::new(0x5CA1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let ab = ids.mint();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: ab,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(32.0, 24.0),
                },
                transform: None,
                name: None,
            },
            Operation::SetFills {
                id: ab,
                fills: vec![Fill {
                    brush: ondin_core::peniko::Brush::Solid(ondin_core::peniko::Color::from_rgba8(
                        10, 20, 30, 255,
                    )),
                    visible: true,
                }],
            },
        ]))
        .unwrap();

        // ⚠️ **Process-unique, which this was not.** A fixed shared path makes the
        // test unrunnable beside a second `cargo test` on the same machine: both
        // runs write `doc.ondin` and `one.png` into one directory, and a read that
        // lands between another process's `create` and its `write` panics at
        // `png[16..20]` with *"range start index 16 out of range for slice of length
        // 0"*. Seen for real on 2026-09-10, twice, while an agent ran the workspace
        // suite beside an interactive one — and it reads exactly like a defect in
        // the export path, which is the expensive part. The house pattern is
        // `ondin-<what>-<pid>` and almost every other fixture in this workspace
        // already uses it.
        let dir = std::env::temp_dir().join(format!("ondin-export-scale-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("doc.ondin");
        std::fs::write(&src, ondin_core::io::save(&doc).unwrap()).unwrap();
        let src = src.to_string_lossy().into_owned();

        let sides = |scale: ExportScale, name: &str| {
            let out = dir.join(name);
            run_export(&src, ExportFormat::Png, Some(&out.to_string_lossy()), scale)
                .expect("export succeeds");
            let png = std::fs::read(&out).unwrap();
            // IHDR's width and height, big-endian, at a fixed offset in every PNG.
            let n = |at: usize| u32::from_be_bytes(png[at..at + 4].try_into().unwrap());
            (n(16), n(20))
        };

        let (w1, h1) = sides(ExportScale::Times(1.0), "one.png");
        let (w2, h2) = sides(ExportScale::Times(2.0), "two.png");
        assert!(
            w1 > 1 && h1 > 1,
            "the fixture has no size to double: {w1}×{h1}"
        );
        assert_eq!((w2, h2), (w1 * 2, h1 * 2), "2× is not twice 1×");

        // **The pinned axis answers the question and the other one follows**, which
        // is the whole difference between `--width` and `--scale`: the fixture is
        // 32×24, so 96 wide is 3× and 72 tall, and 96 *tall* is 4× and 128 wide.
        // Asserting both sides of both is what catches the axes being swapped —
        // a square fixture would have passed under either.
        assert_eq!(sides(ExportScale::Width(96), "w.png"), (96, 72));
        assert_eq!(sides(ExportScale::Height(96), "h.png"), (128, 96));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The clamp is `viewport_at`'s and correct; what this pins is that the ad-hoc
    /// path *says so*, which `plan` already does and this command did not.
    ///
    /// ⚠️ **The third case is a floor rather than a clamp, and the flip on it did
    /// not bite** — which is the finding, not a failed experiment. It was written to
    /// show that `>=` beats `==` in `clamp_note`, on the theory that `px`'s
    /// one-pixel floor can serve *more* than the arithmetic asked for. Flipping to
    /// `==` changed nothing, because `px` takes `ceil` of the same positive product
    /// and the ceiling of a positive is never below 1: 0.01× of 100×50 asks for
    /// `ceil(1.0) × ceil(0.5)` = 1×1 and gets 1×1 exactly. So the case is real, the
    /// silence is right, and **no input can tell the two spellings apart**. What the
    /// third case does still pin is that a sub-pixel scale is silent at all.
    #[test]
    fn a_clamped_scale_is_reported_and_a_floored_one_is_not() {
        use ondin_core::kurbo::Rect;
        let view = Rect::new(0.0, 0.0, 100.0, 50.0);

        // Served exactly: nothing to say.
        assert_eq!(clamp_note(view, ExportScale::Times(2.0), (200, 100)), None);

        // Past `MAX_RASTER_SIDE` on the long side. `viewport_at` is the authority
        // on what comes back, so the note is checked against what it actually did
        // rather than against a number written here.
        let big = ExportScale::Times(2000.0);
        let vp = ondin_export::png::viewport_at(view, big.factor(100.0, 50.0));
        let note = clamp_note(view, big, vp.pixel_size).expect("2000× is past the limit");
        assert!(
            note.contains(&format!(
                "past the {} limit",
                ondin_export::png::MAX_RASTER_SIDE
            )),
            "{note}"
        );
        // **Named in the vocabulary that was typed**, not in the multiplier it
        // became — the point of which shows on the `--width` line below.
        assert!(note.starts_with("2000x "), "{note}");
        // The effective multiplier, which is what a script needs and the raw pixel
        // counts do not give at a glance. Derived from the cap rather than typed,
        // so a change to `MAX_RASTER_SIDE` moves this with it — it has moved once
        // already (§15, `[S8.2-L1-01]`).
        assert!(
            note.contains(&format!(
                "{:.3}×",
                f64::from(ondin_export::png::MAX_RASTER_SIDE) / 100.0
            )),
            "{note}"
        );
        // Proportions kept — the reason the cap is on the scale and not per axis.
        // ⚠️ **Kept to within a pixel *per side*, not exactly**, and asserting
        // `h * 2 == w` is the form somebody would reach for and is wrong: both
        // sides are `ceil`ed independently after the one multiplier is applied,
        // and the multiplier is a float, so each side can round up on its own. At
        // the old cap that showed as 65536 against 65535; at this one both sides
        // land a hair over an exact integer and the doubled short side is 2 past
        // the long one.
        assert!(
            vp.pixel_size.1 * 2 - vp.pixel_size.0 <= 2,
            "aspect drifted: {:?}",
            vp.pixel_size
        );

        // A pinned axis says so in its own words. `--width 200000` is a clamp like
        // any other, and reporting it as "655.35×" would name a number nobody
        // typed — which is the reason `clamp_note` takes the spec and not the
        // factor it resolved to.
        let wide = ExportScale::Width(200_000);
        let vp = ondin_export::png::viewport_at(view, wide.factor(100.0, 50.0));
        let note = clamp_note(view, wide, vp.pixel_size).expect("200000w is past the limit");
        assert!(note.starts_with("200000w "), "{note}");

        // Floored, not clamped: 0.01× of 100×50 is 1×0.5, and every side is lifted
        // to a whole pixel. Bigger than asked, and silent.
        let vp = ondin_export::png::viewport_at(view, 0.01);
        assert_eq!(vp.pixel_size, (1, 1));
        assert_eq!(
            clamp_note(view, ExportScale::Times(0.01), vp.pixel_size),
            None
        );
    }

    #[test]
    fn export_reports_a_missing_file_instead_of_panicking() {
        let err = run_export(
            "does-not-exist.ondin",
            ExportFormat::Svg,
            None,
            ExportScale::Times(1.0),
        )
        .unwrap_err();
        assert!(err.contains("cannot read"), "{err}");
    }

    /// `--all` is its own command, and the two flags that only make sense with it
    /// are refused rather than ignored — an option silently doing nothing is how a
    /// build script comes to believe it asked for something it did not.
    #[test]
    fn export_all_is_a_command_of_its_own() {
        assert_eq!(
            export(&["a.ondin", "--all"]).unwrap(),
            Command::ExportAll {
                file: "a.ondin".into(),
                out: None,
                folders: false,
                scale_override: None,
            }
        );
        assert_eq!(
            export(&["a.ondin", "--all", "--folders", "-o", "assets"]).unwrap(),
            Command::ExportAll {
                file: "a.ondin".into(),
                out: Some("assets".into()),
                folders: true,
                scale_override: None,
            }
        );
        assert!(
            export(&["a.ondin", "--all", "--png"]).is_err(),
            "--all takes its formats from the document"
        );
        assert!(
            export(&["a.ondin", "--folders"]).is_err(),
            "--folders means nothing without --all"
        );
    }

    /// The reason export settings live in the document: a build step can run them
    /// with no window, no GPU and nobody steering a dialog.
    ///
    /// It asserts the **filenames**, not just that files appeared — the suffix
    /// rule, the extension per format and the de-collision are the whole of what a
    /// script depends on, and a test that only counted files would pass with all
    /// three of them wrong.
    ///
    /// The last two thirds are `--scale-override` (§15 D361), on the same fixture
    /// so the forced pass can be read against the unforced one.
    ///
    /// ⚠️ **The collapse is flip-checked and bites exactly where it should.**
    /// Dropping the dedupe leaves `["Close (2).png", "Close.png", "Close.svg"]` —
    /// the de-collided junk file the rule exists to prevent, and the reason
    /// overriding is not simply "set every scale": at 1× the marker that kept two
    /// rows apart is gone, so without the collapse the pass writes the same asset
    /// twice under a name nobody chose.
    #[test]
    fn export_all_runs_the_documents_own_settings() {
        use ondin_core::kurbo::Size;
        use ondin_core::{
            Document, ExportFormat as Fmt, ExportScale, ExportSpec, IdSource, NodeKind, Operation,
            Transaction,
        };

        let mut ids = IdSource::new(0xA11);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (frame, rect) = (ids.mint(), ids.mint());
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: frame,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(64.0, 64.0),
                },
                transform: None,
                name: Some("Board".into()),
            },
            Operation::CreateNode {
                id: rect,
                parent: frame,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(24.0, 24.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: Some("Close".into()),
            },
            Operation::SetExports {
                id: rect,
                exports: vec![
                    ExportSpec::new(Fmt::Png, ExportScale::Times(1.0)),
                    ExportSpec::new(Fmt::Png, ExportScale::Times(2.0)),
                    ExportSpec::new(Fmt::Svg, ExportScale::Times(1.0)),
                ],
            },
        ]))
        .unwrap();

        // Process-unique, per the house pattern — and this one opens by deleting
        // the directory, so a concurrent run's fixture goes with it. This is the
        // test that failed under a parallel workspace run on 2026-09-10.
        let dir = std::env::temp_dir().join(format!("ondin-export-all-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("doc.ondin");
        std::fs::write(&src, ondin_core::io::save(&doc).unwrap()).unwrap();
        let out = dir.join("assets");

        run_export_all(
            &src.to_string_lossy(),
            Some(&out.to_string_lossy()),
            false,
            None,
        )
        .expect("export --all succeeds");

        let mut names: Vec<String> = std::fs::read_dir(&out)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["Close.png", "Close.svg", "Close@2x.png"]);
        // And the 2× really is twice as wide, which is the one claim the filenames
        // cannot make for themselves.
        let big = std::fs::read(out.join("Close@2x.png")).unwrap();
        assert_eq!(
            u32::from_be_bytes(big[16..20].try_into().unwrap()),
            48,
            "a 24pt layer at 2× is 48 pixels"
        );

        // **`--scale-override 1x` collapses the set it was built as.** The same
        // three specs forced to 1× are a PNG, a PNG and an SVG — two of which are
        // now the same file — so the pass writes two files rather than three, and
        // the marker goes with the scale it described.
        let flat = dir.join("flat");
        run_export_all(
            &src.to_string_lossy(),
            Some(&flat.to_string_lossy()),
            false,
            Some(ExportScale::Times(1.0)),
        )
        .expect("export --all --scale-override succeeds");
        let mut names: Vec<String> = std::fs::read_dir(&flat)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            ["Close.png", "Close.svg"],
            "the override should have collapsed the 1× and 2× rows, not de-collided them"
        );
        let small = std::fs::read(flat.join("Close.png")).unwrap();
        assert_eq!(
            u32::from_be_bytes(small[16..20].try_into().unwrap()),
            24,
            "forced to 1×, a 24pt layer is 24 pixels"
        );

        // And a pinned axis overrides just as well, which is the case `--scale` on
        // a one-off deliberately does not cover: across a *set*, "make everything
        // 96 wide" is the question that has an answer.
        let wide = dir.join("wide");
        run_export_all(
            &src.to_string_lossy(),
            Some(&wide.to_string_lossy()),
            false,
            Some(ExportScale::Width(96)),
        )
        .expect("export --all --scale-override 96w succeeds");
        let big = std::fs::read(wide.join("Close@96w.png")).unwrap();
        assert_eq!(
            u32::from_be_bytes(big[16..20].try_into().unwrap()),
            96,
            "96w means 96 pixels wide whatever the layer's own size"
        );

        // A document nobody has set up to export fails rather than succeeding
        // silently: a build step that writes nothing must not exit 0.
        let bare = dir.join("bare.ondin");
        std::fs::write(&bare, ondin_core::io::save(&Document::new(root)).unwrap()).unwrap();
        let err = run_export_all(&bare.to_string_lossy(), None, false, None).unwrap_err();
        assert!(err.contains("no layer has export settings"), "{err}");
    }

    /// **A document set up to export whose every file is skipped fails too**
    /// (§15 D531) — `[S16.5-L1-01]`.
    ///
    /// The gate above fires on the set of *layers*; this is the set of *files
    /// produced*, and they are not the same question. A layer with export
    /// settings and no ink — an empty group, a fully hidden layer, a zero-size
    /// shape — carrying a **raster** spec renders `None`: `png::raster_of`
    /// opens with `extent(res, origins)?`. Before this the run wrote nothing,
    /// did not create the output directory, said *"skipped …"* on **stderr**
    /// where a build step does not read it, and returned `Ok(())`.
    ///
    /// Which is the failure `run_export_all`'s own doc says the exit-1 path
    /// exists to prevent, three paragraphs above the code that did it: *"a
    /// cheerful zero over nothing written. A build step that silently succeeds
    /// without producing its output is the failure that gets noticed three
    /// commits later."*
    ///
    /// ⚠️ **The spec has to be raster and the fixture asserts nothing else
    /// about it, because the SVG arm cannot reach this at all** —
    /// `plan::render`'s SVG branch always answers `Some`, so the same empty
    /// group with an SVG spec writes a file and exits 0, correctly. A version
    /// of this test using the default spec would pass under the defect.
    ///
    /// ⚠️ Flipped by removing the `written.is_empty()` branch: red at
    /// `expect_err`, which panics with the `Ok(())` the finding measured.
    #[test]
    fn an_export_all_that_skips_every_file_fails_rather_than_exiting_zero() {
        use ondin_core::{
            Document, ExportFormat as Fmt, ExportScale, ExportSpec, IdSource, NodeKind, Operation,
            Transaction,
        };
        let mut ids = IdSource::new(0xE0);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let empty = ids.mint();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: empty,
                parent: root,
                index: 0,
                // A group with no children has no extent, which is what makes
                // every raster spec on it render `None`.
                kind: NodeKind::Group,
                transform: None,
                name: Some("Empty".into()),
            },
            Operation::SetExports {
                id: empty,
                exports: vec![ExportSpec::new(Fmt::Png, ExportScale::Times(1.0))],
            },
        ]))
        .unwrap();

        let dir = std::env::temp_dir().join(format!("ondin-export-skipped-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("doc.ondin");
        std::fs::write(&src, ondin_core::io::save(&doc).unwrap()).unwrap();
        let out = dir.join("assets");

        let err = run_export_all(
            &src.to_string_lossy(),
            Some(&out.to_string_lossy()),
            false,
            None,
        )
        .expect_err("every file skipped is a failure");
        assert!(err.contains("every export was skipped"), "{err}");
        assert!(
            !out.exists(),
            "and nothing was created, which is the half stderr could not say"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **`--all` says how many files it replaced** — §15 D652, `[S8.2-L5-07]`,
    /// which is §15 D636's rule at the door D636 did not reach.
    ///
    /// D636 decided that the disclosure belongs to the door: a *File* or an
    /// *Archive* was named in a save dialog and the OS has already warned, so a
    /// door that writes with **no dialog at all** owes a count instead. It gave
    /// that count to the panel's folder arm. The CLI is the other such door and a
    /// separately implemented walk (§15 D626), and it is the worse of the two,
    /// because with no `-o` it writes into **the folder holding the `.ondin`** —
    /// in the library, a project folder. A document with a layer named `logo`
    /// replaced a hand-authored `logo.svg` beside it and said `wrote 1 file`.
    ///
    /// **The count is asserted on `write_planned` rather than on the run**,
    /// because the run's only report of it is a `println!` a test cannot read;
    /// lifting it out is §15 D269's shape and is what makes this assertable at
    /// all.
    ///
    /// ⚠️ **The second `write_planned` is the half that says what the number
    /// means.** A re-run of the same export legitimately replaces everything it
    /// wrote last time, so `(2 replaced)` is the ordinary reading and not an
    /// alarm — the count is a fact about the folder, not an accusation. Asserting
    /// only the first run would leave that unstated and read as "a replacement is
    /// a mistake".
    ///
    /// ⚠️ **Flip run.** Predicted failing assertion: the `1` on the first
    /// `write_planned`. Deleting `if path.exists() { replaced += 1 }` fails there
    /// at 0 against 1, and the report assertions above stay green — they are about
    /// the sentence, and only this one is about the counting.
    #[test]
    fn export_all_says_how_many_files_it_replaced() {
        let out = std::path::Path::new("out");
        assert_eq!(export_all_report(1, 0, 0, out), "wrote 1 file to out");
        assert_eq!(
            export_all_report(4, 2, 0, out),
            "wrote 4 files to out (2 replaced)"
        );
        assert_eq!(
            export_all_report(4, 2, 1, out),
            "wrote 4 files to out (2 replaced) (1 skipped)",
            "both counts, and in the order they happened"
        );

        let dir = std::env::temp_dir().join(format!("ondin-replaced-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // The scene: a hand-authored file sitting where a planned export lands,
        // and a second planned file whose name carries a folder that does not
        // exist yet.
        let decoy = dir.join("logo.svg");
        std::fs::write(&decoy, b"hand-authored").unwrap();
        let files = vec![
            (decoy.clone(), b"exported".to_vec()),
            (dir.join("icons").join("close.png"), b"png".to_vec()),
        ];

        assert_eq!(
            write_planned(&files).expect("writes"),
            1,
            "one of the two paths already held a file"
        );
        assert_eq!(
            std::fs::read(&decoy).unwrap(),
            b"exported",
            "and it really is gone — the count is the only warning there is"
        );
        assert!(
            dir.join("icons").is_dir(),
            "the `/` in a name is still a folder the writer creates"
        );

        assert_eq!(
            write_planned(&files).expect("writes again"),
            2,
            "a re-run replaces everything it wrote last time, which is ordinary"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

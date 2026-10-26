//! ondin — the `ondin` binary (§9).
//!
//! Subcommands — fenced, because rustdoc reads a bare usage line as markup
//! (`[gui]` as an intra-doc link, `<file>` as an unclosed HTML tag):
//!
//! ```text
//! ondin [gui]                    launch the GUI shell (default)
//! ondin export <file> [--svg|--png|--json|--html|--react|--vue] [-o out]
//!            [--scale <n> | --width <px> | --height <px>] [--page <name|n>]
//!                                render a page with no GPU and no window, by
//!                                default the first that is not a draft;
//!                                the three sizes are --png only, and one at a time;
//!                                --html writes a zip of HTML and CSS,
//!                                --react and --vue a zip of components;
//!                                --json describes every page
//! ondin export <file> --all [--scale-override <2x|512w|512h>] [--folders]
//!            [--page <name|n>] [-o dir]
//!                                run the document's own export settings (§7) on
//!                                every page that is not a draft, or on one page,
//!                                optionally forcing every row to one size
//! ondin render <file> --component <name> --data <rows.csv> [--name-by <column>]
//!            [--png|--svg] [--scale <n> | --width <px> | --height <px>] [-o dir]
//!                                the component once per CSV row, each column
//!                                feeding the property of its name (render_cli)
//! ondin render <file> --all-modes [--page <name|n>] [--png|--svg] [size] [-o dir]
//!                                every visible top-level frame once per mode
//! ondin diff <a> <b> [--json]    what changed from document a to b, by layer;
//!                                exit 0 when nothing did, 1 when something did,
//!                                2 when either cannot be read
//! ondin serve <file>             headless: MCP endpoint over a document (§8.2)
//! ondin mcp-proxy                stdio↔socket bridge for agents (§8.1)
//! ```
//!
//! `export` runs on `ondin-core` and the CPU renderer alone, so it works with no
//! display or graphics driver. `serve`/`mcp-proxy` land at M6.
//!
//! The rustdoc gate and the lint it allows are explained in `ondin_core`'s crate doc.
//!
//! A GUI-subsystem binary on Windows in a release build, so a launch from the
//! Start menu or Explorer opens no console. `ondin export …` typed at a prompt is
//! reached through the console twin `ondin-cli` (shipped as `ondin.com`, which
//! `PATHEXT` prefers over the `.exe`), which runs this binary with its own
//! standard handles. A debug build keeps the console, where `eprintln!` is read.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![deny(rustdoc::broken_intra_doc_links, rustdoc::invalid_html_tags)]
#![allow(rustdoc::private_intra_doc_links)]
// `Duration - Duration` panics on underflow in release too. Production only: the
// tests back-date an `Instant` by subtraction on purpose.
#![cfg_attr(not(test), deny(clippy::unchecked_time_subtraction))]

mod app;
mod atomic;
mod canvas;
mod cheatsheet;
mod chrome;
mod colour_text;
mod cursor;
mod expr;
mod eyedropper;
#[cfg(windows)]
mod file_type;
mod fonts;
mod gradient;
mod grid;
mod input;
mod instances;
mod kits;
// No blanket `dead_code` allow here: the plain build is the gate that catches an
// unwired item. `store::versions` (test-only callers) carries its own allow.
mod library;
mod logging;
mod machine_dir;
mod measure;
mod menu;
mod pages;
mod palette;
mod panels;
mod path_bar;
mod prefs;
mod present;
mod preview;
mod render_cli;
mod rulers;
mod session;
mod settings;
mod snap;
mod spacing;
mod theme;
mod thumbs;
mod tools;
mod ui;
mod update;
mod versions;
mod view;

use ondin_core::ExportScale;

#[derive(Debug, PartialEq)]
enum Command {
    /// The window, opening `open` when the command line named a document
    /// (`ondin file.ondin`, which is also what a double-click runs).
    Gui {
        open: Option<std::path::PathBuf>,
    },
    Serve {
        file: String,
    },
    Export {
        file: String,
        format: ExportFormat,
        out: Option<String>,
        /// How big the raster comes out — `--scale 2`, `--width 512`, `--height 512`.
        ///
        /// Not an `Option`: the parser refuses all three for the formats that cannot
        /// use one, so `Times(1.0)` here means "unscaled", never "unasked".
        scale: ExportScale,
        /// `--page`: a page's number from 1, or its exact name.
        page: Option<String>,
    },
    /// Run the document's own export settings (§7) — every layer that carries any,
    /// at the formats and sizes the file says, into a folder.
    ///
    /// Goes through the same `ondin_export::plan` as the panel's button, so a build
    /// script gets the bytes a designer would get by clicking.
    ExportAll {
        file: String,
        out: Option<String>,
        folders: bool,
        /// `--scale-override`: force every spec in the file to this size. The
        /// document still decides which layers export and at what formats.
        scale_override: Option<ExportScale>,
        /// `--page`: run only this page's settings, draft or not.
        page: Option<String>,
    },
    /// `ondin render`: a component once per CSV row, or every frame once per mode.
    Render(render_cli::Render),
    /// Compare two documents (`ondin_core::diff`), `a` the earlier.
    Diff {
        a: String,
        b: String,
        json: bool,
    },
    McpProxy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExportFormat {
    Svg,
    Png,
    Json,
    /// Code export's plain HTML/CSS, zipped.
    Html,
    /// Code export's React components, zipped.
    React,
    /// Code export's Vue components, zipped.
    Vue,
}

impl ExportFormat {
    fn extension(self) -> &'static str {
        match self {
            ExportFormat::Svg => "svg",
            ExportFormat::Png => "png",
            ExportFormat::Json => "json",
            ExportFormat::Html | ExportFormat::React | ExportFormat::Vue => "zip",
        }
    }
}

/// Whether `args` are a launch Velopack's startup hook must see: the GUI's
/// (nothing, `gui`, or a document to open) and Velopack's own `--veloapp-*`
/// re-invocations — an allowlist on the first argument, since the re-invocations
/// carry arguments too.
///
/// It lists what reaches the hook, not what avoids it, so a mistyped
/// `ondin exprot …` or `ondin --version` never reaches it and `parse` refuses it
/// with exit 2.
fn hook_runs(args: &[String]) -> bool {
    match args.first() {
        None => true,
        Some(first) => {
            first == "gui"
                || first.to_ascii_lowercase().starts_with("--veloapp-")
                || names_a_document(first)
        }
    }
}

/// Whether a first argument is a document to open rather than a subcommand: a
/// `.ondin` name, or any file that exists. Anything else stays "unknown command",
/// so a typo of `export` is refused, not opened.
fn names_a_document(arg: &str) -> bool {
    let path = std::path::Path::new(arg);
    !arg.starts_with('-')
        && (path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("ondin"))
            || path.is_file())
}

/// The command line as strings, or the refusal for one that is not Unicode.
///
/// Not `std::env::args`, which panics on such an argument (a Latin-1-named file
/// on Linux): it is refused with `parse`'s exit 2 and a sentence instead. Taking
/// the path through would mean `OsString` all the way down the export path.
fn unicode_args(args: impl Iterator<Item = std::ffi::OsString>) -> Result<Vec<String>, String> {
    args.map(|a| {
        a.into_string().map_err(|bad| {
            format!(
                "ondin: an argument is not valid Unicode: {}",
                bad.to_string_lossy()
            )
        })
    })
    .collect()
}

fn parse(args: &[String]) -> Result<Command, String> {
    match args.first().map(String::as_str) {
        None | Some("gui") => Ok(Command::Gui { open: None }),
        Some("serve") => args
            .get(1)
            .map(|file| Command::Serve { file: file.clone() })
            .ok_or_else(|| "usage: ondin serve <file>".to_string()),
        Some("export") => parse_export(&args[1..]),
        Some("render") => render_cli::parse(&args[1..]).map(Command::Render),
        Some("diff") => parse_diff(&args[1..]),
        Some("mcp-proxy") => Ok(Command::McpProxy),
        // Absolute, so the session's save path and `prefs.last_document` survive a
        // different working directory next launch.
        Some(file) if names_a_document(file) => match args.get(1) {
            Some(extra) => Err(format!("ondin {file}: unexpected argument {extra}")),
            None => Ok(Command::Gui {
                open: Some(std::path::absolute(file).map_err(|e| format!("ondin {file}: {e}"))?),
            }),
        },
        Some(other) => Err(format!("unknown command: {other}")),
    }
}

fn parse_diff(args: &[String]) -> Result<Command, String> {
    const USAGE: &str = "usage: ondin diff <a.ondin> <b.ondin> [--json]";
    let mut files = Vec::new();
    let mut json = false;
    for arg in args {
        match arg.as_str() {
            "--json" => json = true,
            other if other.starts_with('-') => {
                return Err(format!("unknown option {other}\n{USAGE}"));
            }
            other => files.push(other.to_string()),
        }
    }
    match <[String; 2]>::try_from(files) {
        Ok([a, b]) => Ok(Command::Diff { a, b, json }),
        Err(files) => Err(format!(
            "diff takes two documents; got {}\n{USAGE}",
            files.len()
        )),
    }
}

fn parse_export(args: &[String]) -> Result<Command, String> {
    const USAGE: &str = "usage: ondin export <file.ondin> [--svg|--png|--json|--html|--react|--vue] \
         [--scale <n>|--width <px>|--height <px>] [--page <name|n>] [-o <out>]\n       \
         ondin export <file.ondin> --all [--scale-override <2x|512w|512h>] [--folders] \
         [--page <name|n>] [-o <dir>]";
    let mut file = None;
    let mut format = None;
    let mut out = None;
    let mut page = None;
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
            "--html" => format = Some(ExportFormat::Html),
            "--react" => format = Some(ExportFormat::React),
            "--vue" => format = Some(ExportFormat::Vue),
            "--all" => all = true,
            "--folders" => folders = true,
            // One value each and mutually exclusive: three ways of answering one
            // question. The typed flag is carried so the refusals can name it.
            flag @ ("--scale" | "--width" | "--height") => {
                i += 1;
                let text = args.get(i).ok_or_else(|| USAGE.to_string())?;
                let asked = size_flag(flag, text, USAGE)?;
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
                // The export row's own vocabulary, so a script types what the row shows.
                over = Some(ExportScale::from_label(text).ok_or_else(|| {
                    format!("--scale-override wants 2x, 512w or 512h, not {text:?}\n{USAGE}")
                })?);
            }
            "--page" => {
                i += 1;
                let name = args.get(i).ok_or_else(|| USAGE.to_string())?;
                page = Some(name.clone());
            }
            "-o" | "--out" => {
                i += 1;
                let name = args.get(i).ok_or_else(|| USAGE.to_string())?;
                // A flag is not a filename: taking the next token unconditionally
                // would make `-o --png` write SVG bytes into a file called `--png`,
                // since the `other if other.starts_with('-')` arm never sees it.
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
            // A second document is refused, not overwritten: a build step whose glob
            // matched twice would otherwise export the wrong document unnoticed.
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
        // A different command, not a fourth format: `--all` writes many files at
        // the document's formats, so `-o` is a folder and `--png` has nowhere to apply.
        if format.is_some() {
            return Err(format!(
                "--all runs the document's own export settings, so it takes no format\n{USAGE}"
            ));
        }
        // The size flags are refused too, and the message points at
        // `--scale-override`, which overrules a size the file already carries.
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
            page,
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
    // SVG is the default: the semantic format, needing no viewport arguments.
    let format = format.unwrap_or(ExportFormat::Svg);
    // Refused rather than ignored, as everywhere in this parser. The message names
    // the rule, since the commonest way to hit it is typing no format at all.
    if let Some((had, _)) = scale
        && format != ExportFormat::Png
    {
        return Err(format!(
            "{had} is a raster size, so it applies only to --png\n{USAGE}"
        ));
    }
    if page.is_some() && format == ExportFormat::Json {
        return Err(format!(
            "--json describes every page, so it takes no --page\n{USAGE}"
        ));
    }
    Ok(Command::Export {
        file: file.ok_or_else(|| USAGE.to_string())?,
        format,
        out,
        scale: scale.map_or(ExportScale::Times(1.0), |(_, s)| s),
        page,
    })
}

/// The value of `--scale`, `--width` or `--height`, shared by `export` and
/// `render`. Zero and negative are refused here, not left to `factor`'s floor,
/// which would turn `--scale 0` into a one-pixel file.
fn size_flag(flag: &str, text: &str, usage: &str) -> Result<ExportScale, String> {
    match flag {
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
    .ok_or_else(|| format!("{flag} wants a positive number, not {text:?}\n{usage}"))
}

fn main() {
    let args = match unicode_args(std::env::args_os().skip(1)) {
        Ok(args) => args,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(2);
        }
    };
    // Velopack's hook, before anything else in a GUI launch. The installer and
    // updater re-run this binary with `--veloapp-*`; `run` services the flag and
    // ends the process, so it must precede `parse`, which would refuse the flag.
    // With no such flag it returns at once.
    //
    // It applies nothing at startup: Velopack's default would apply a staged
    // update and stop every running Ondin, killing a dirty window with no
    // question. An update applies only through the chip, once nothing else is
    // running (`update::Updater::on_exit`). Other command lines skip it, see
    // `hook_runs`.
    //
    // On Windows the install and update hooks also register the `.ondin` file type
    // (the update re-registers installs that predate it), and uninstall removes it.
    if hook_runs(&args) {
        logging::init();
        let mut hooks = velopack::VelopackApp::build().set_auto_apply_on_startup(false);
        #[cfg(windows)]
        {
            hooks = hooks
                .on_after_install_fast_callback(|_| file_type::register())
                .on_after_update_fast_callback(|_| file_type::register())
                .on_before_uninstall_fast_callback(|_| file_type::unregister());
        }
        hooks.run();
    }
    // Counted by any other Ondin's updater before it applies — the GUI and a
    // headless export alike, since the updater stops both.
    instances::register();
    let result = match parse(&args) {
        Ok(Command::Gui { open }) => run_gui(open).map_err(|e| format!("ondin gui failed: {e}")),
        Ok(Command::Export {
            file,
            format,
            out,
            scale,
            page,
        }) => run_export(&file, format, out.as_deref(), scale, page.as_deref()),
        Ok(Command::ExportAll {
            file,
            out,
            folders,
            scale_override,
            page,
        }) => run_export_all(
            &file,
            out.as_deref(),
            folders,
            scale_override,
            page.as_deref(),
        ),
        Ok(Command::Render(r)) => render_cli::run(&r),
        // `diff`'s own exit codes: 1 is "they differ", so a failure is 2.
        Ok(Command::Diff { a, b, json }) => match diff_files(&a, &b, json) {
            Ok((report, differs)) => {
                print!("{report}");
                let _ = std::io::Write::flush(&mut std::io::stdout());
                std::process::exit(i32::from(differs));
            }
            Err(msg) => {
                eprintln!("{msg}");
                std::process::exit(2);
            }
        },
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
/// `viewport_at` caps the multiplier silently, which a build script would not
/// notice (8× on a poster coming back 3.4×); like `plan`, this reports it.
///
/// Returns the sentence rather than printing it, so the decision is assertable
/// without capturing stderr. The size is named in the spec's own vocabulary
/// (`2x`, `512w`), not the multiplier it became.
///
/// `>=` rather than `==` is deliberate and currently indistinguishable: `px`
/// takes `ceil` of the same positive product, so `got` never exceeds `asked`.
/// The `>=` form survives a change to `px`'s clamp, and no test can tell the two apart.
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

/// The page `--page` names — its number from 1, or its exact name — or, unasked,
/// the first page that is not a draft (the first page when every one is).
fn chosen_page(
    doc: &ondin_core::Document,
    asked: Option<&str>,
) -> Result<ondin_core::NodeId, String> {
    let Some(asked) = asked else {
        return Ok(doc.gathered_pages().next().unwrap_or(doc.first_page()));
    };
    let by_number = asked
        .parse::<usize>()
        .ok()
        .and_then(|n| n.checked_sub(1))
        .and_then(|i| doc.pages().get(i))
        .map(|p| p.id);
    let by_name = || {
        doc.page_ids()
            .find(|id| doc.get(*id).is_some_and(|n| n.name() == asked))
    };
    by_number.or_else(by_name).ok_or_else(|| {
        let names: Vec<&str> = doc
            .page_ids()
            .filter_map(|id| doc.get(id).map(|n| n.name()))
            .collect();
        format!("no page {asked:?}; the pages are {}", names.join(", "))
    })
}

/// `ondin diff`'s output and whether the documents differ. The text says nothing
/// when they do not, as `diff` does; the JSON is written either way.
fn diff_files(a: &str, b: &str, json: bool) -> Result<(String, bool), String> {
    let load = |file: &str| {
        let bytes = std::fs::read(file).map_err(|e| format!("cannot read {file}: {e}"))?;
        ondin_core::io::load(&bytes).map_err(|e| format!("cannot load {file}: {e}"))
    };
    let d = ondin_core::diff::diff(&load(a)?, &load(b)?);
    let report = if json {
        serde_json::to_string_pretty(&d).map_err(|e| format!("cannot serialize the diff: {e}"))?
            + "\n"
    } else {
        d.text()
    };
    Ok((report, !d.is_empty()))
}

/// Headless export (§7). Loads the document, resolves it, and writes one of the
/// export formats of one page — no window, no GPU, no font service (text uses the
/// bundled fallback, which keeps the output deterministic in CI).
///
/// `scale` reaches only the PNG arm, and the output path is not decorated with it:
/// `plan`'s `@2x` marker disambiguates a set of files, not this command's one output.
///
/// `Width`/`Height` are resolved here, against the same box the render frames,
/// since `--width 512` is a different multiplier on every document.
fn run_export(
    file: &str,
    format: ExportFormat,
    out: Option<&str>,
    scale: ExportScale,
    page: Option<&str>,
) -> Result<(), String> {
    use ondin_core::Resolved;

    let bytes = std::fs::read(file).map_err(|e| format!("cannot read {file}: {e}"))?;
    let doc = ondin_core::io::load(&bytes).map_err(|e| format!("cannot load {file}: {e}"))?;
    let res = Resolved::rebuild(&doc);
    let page = chosen_page(&doc, page)?;

    let out_path = out
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::Path::new(file).with_extension(format.extension()));
    // `-o` naming the document itself would replace it with the export.
    let same = |a: &std::path::Path, b: &str| {
        std::fs::canonicalize(a).ok() == std::fs::canonicalize(b).ok() && a.exists()
    };
    if same(&out_path, file) {
        return Err(format!(
            "refusing to write the export over {file}, the document it is made from"
        ));
    }
    let replaced = out_path.exists();

    let data = match format {
        ExportFormat::Svg => ondin_export::svg::svg(&doc, &res, page, None).into_bytes(),
        ExportFormat::Html | ExportFormat::React | ExportFormat::Vue => {
            use ondin_export::code::Target;
            // The page holds the top-level layers, laid out as on the canvas.
            let origins = doc
                .get(page)
                .map(|r| r.children().to_vec())
                .unwrap_or_default();
            let target = match format {
                ExportFormat::React => Target::React,
                ExportFormat::Vue => Target::Vue,
                _ => Target::Html,
            };
            let export = target.export(&doc, &res, &origins);
            if !export.approximated.is_empty()
                || !export.positioned.is_empty()
                || !export.inlined.is_empty()
            {
                eprintln!(
                    "{} drawn differently, {} with fixed positions or as pictures, \
                     {} instances written out as layers — see report.txt",
                    export.approximated.len(),
                    export.positioned.len(),
                    export.inlined.len()
                );
            }
            export
                .zip()
                .map_err(|e| format!("cannot write the zip: {e}"))?
        }
        ExportFormat::Json => {
            let snap = ondin_export::snapshot(&doc, &res);
            serde_json::to_vec_pretty(&snap)
                .map_err(|e| format!("cannot serialize snapshot: {e}"))?
        }
        ExportFormat::Png => {
            // Frame the whole page at `scale` device pixels per world unit. The
            // 512-square is this command's answer to an empty page (`extent`
            // returns `None`; the SVG writer wants a different fallback).
            let view = ondin_export::extent(&res, &[page])
                .unwrap_or_else(|| ondin_core::kurbo::Rect::new(0.0, 0.0, 512.0, 512.0));
            let vp = ondin_export::png::viewport_at(
                view,
                scale.factor(view.width().max(1.0), view.height().max(1.0)),
            );
            if let Some(note) = clamp_note(view, scale, vp.pixel_size) {
                eprintln!("{note}");
            }
            ondin_export::png::png(&doc, &res, &vp, page)
        }
    };

    std::fs::write(&out_path, data)
        .map_err(|e| format!("cannot write {}: {e}", out_path.display()))?;
    // A door that writes with no dialog owes its count of what it replaced.
    let note = if replaced { " (replaced)" } else { "" };
    println!("wrote {}{note}", out_path.display());
    Ok(())
}

/// Headless run of the document's own export settings (§7).
///
/// Exits 1 when no layer has export settings, or when every file was skipped:
/// a build step that succeeds without producing its output goes unnoticed.
///
/// The whole plan is rendered before anything is written, so a failure part way
/// through leaves files that all came from one state of the document.
fn run_export_all(
    file: &str,
    out: Option<&str>,
    folders: bool,
    scale_override: Option<ExportScale>,
    page: Option<&str>,
) -> Result<(), String> {
    use ondin_core::Resolved;

    let bytes = std::fs::read(file).map_err(|e| format!("cannot read {file}: {e}"))?;
    let doc = ondin_core::io::load(&bytes).map_err(|e| format!("cannot load {file}: {e}"))?;
    let res = Resolved::rebuild(&doc);

    // Every layer carrying settings, in document order — the same set and the same
    // order the panel's *Export all* collects (drafts left out), so the two produce
    // the same files. `--page` names the one page instead.
    let mut subjects = Vec::new();
    let mut stack: Vec<ondin_core::NodeId> = match page {
        Some(_) => vec![chosen_page(&doc, page)?],
        None => doc.gathered_pages().collect(),
    };
    while let Some(id) = stack.pop() {
        let Some(node) = doc.get(id) else { continue };
        if !node.exports().is_empty() {
            subjects.push(id);
        }
        stack.extend(node.children().iter().copied());
    }
    // `all_in_document_order`, matching the panel exactly — see `exportable_layers`.
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
            unset: None,
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
    // Every file skipped is a failure: `subjects.is_empty()` above counts layers,
    // not files produced. The raster arm is the reachable one — `png::raster_of`
    // opens with `extent(res, origins)?`, so an ink-less layer skips every raster
    // spec, while the SVG arm always returns `Some`.
    if written.is_empty() {
        return Err(format!(
            "{file}: every export was skipped — no layer with export settings has a size \
             ({skipped} skipped)"
        ));
    }
    // A door that writes with no dialog owes a count of what it replaced (§15 D636),
    // and this one is the worse case: with no `-o`, `dir` is the folder holding the
    // `.ondin`, where a hand-authored `logo.svg` may sit. The count is taken before
    // the write, since afterwards every path exists. Same sentence as
    // `write_export`'s, a separate implementation of the same walk.
    let replaced = write_planned(&written)?;
    println!(
        "{}",
        export_all_report(written.len(), replaced, skipped, &dir)
    );
    Ok(())
}

/// Write a rendered plan to disk, answering how many of those paths already held
/// a file.
///
/// The count is taken before the write, since afterwards every path exists; it has
/// the same shape as `panels::export::write_export`'s folder arm. Separate from
/// `run_export_all` so the count is assertable rather than only printed.
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
/// The folder comes first and the parenthetical counts follow, since a terminal
/// line is not truncated. `(n replaced)` is owed by any door that writes without
/// a dialog.
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

/// The running window's icon — taskbar button, Alt-Tab, and on macOS the Dock.
///
/// Distinct from the icon on `ondin.exe`, which `build.rs` compiles into the
/// resource table as a multi-size `.ico`: winit registers its window class with
/// `hIcon: 0` rather than falling back to the executable's, so each covers
/// surfaces the other does not.
///
/// One size is chosen, since `WM_SETICON` gets the same pixels for `ICON_BIG` and
/// `ICON_SMALL`. `icons/icon.png` is 512 so it has something left to give at any
/// display scaling; the 16px copies are a downscale done by the OS. (The 1024 is
/// for the macOS `.icns` alone, and the 64 is the top bar's mark.)
fn window_icon() -> eframe::egui::IconData {
    static PNG: &[u8] = include_bytes!("../../../icons/icon.png");
    // Compiled in, so this works on every run or on none; the test below holds
    // the "every" end.
    eframe::icon_data::from_png_bytes(PNG).expect("the bundled window icon is not a valid PNG")
}

fn run_gui(open: Option<std::path::PathBuf>) -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        // The system's title bar off, or transparent on macOS, with the app
        // drawing what the host leaves to it (`chrome`).
        viewport: chrome::Chrome::current().viewport(
            eframe::egui::ViewportBuilder::default()
                .with_inner_size([1200.0, 800.0])
                .with_title("Ondin")
                .with_icon(window_icon()),
        ),
        // The window's last size, position and maximized state, beside
        // `prefs.json`; with no file yet, the size above.
        persistence_path: dirs::config_dir().map(|d| d.join("ondin").join("window.ron")),
        ..Default::default()
    };
    eframe::run_native(
        "Ondin",
        options,
        Box::new(|cc| Ok(Box::new(app::OndinApp::new(cc, open)))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only a GUI launch and Velopack's own re-invocations reach the install hook;
    /// a CLI command or a malformed command line (a typo, `--version`) does not.
    ///
    /// flip: `hook_runs` as the negation of a three-subcommand allowlist → fails at
    /// `["exprot", "a.ondin"]`.
    #[test]
    fn only_a_gui_launch_or_velopack_reaches_the_install_hook() {
        let args = |s: &[&str]| s.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        for not in [
            &["export", "a.ondin"][..],
            &["serve", "a.ondin"],
            &["mcp-proxy"],
            &["exprot", "a.ondin"],
            &["--version"],
            &["help"],
        ] {
            assert!(!hook_runs(&args(not)), "{not:?}");
        }
        for hook in [
            &[][..],
            &["gui"],
            &["--veloapp-install", "0.4.0"],
            &["--veloapp-updated", "0.4.0"],
            &["--veloapp-uninstall"],
            &["--VELOAPP-OBSOLETE", "0.4.0"],
        ] {
            assert!(hook_runs(&args(hook)), "{hook:?}");
        }
    }

    /// An argument that is not Unicode is refused, not panicked on — a lone
    /// surrogate on Windows, an invalid byte on Unix.
    #[test]
    fn an_argument_that_is_not_unicode_is_refused() {
        #[cfg(windows)]
        let bad = {
            use std::os::windows::ffi::OsStringExt;
            // A lone trailing surrogate.
            std::ffi::OsString::from_wide(&[0x61, 0xDC00, 0x62])
        };
        #[cfg(unix)]
        let bad = {
            use std::os::unix::ffi::OsStringExt;
            std::ffi::OsString::from_vec(vec![b'a', 0xE9, b'.', b'o'])
        };
        let ok = || std::ffi::OsString::from("export");
        assert_eq!(
            unicode_args([ok(), "a.ondin".into()].into_iter()),
            Ok(vec!["export".to_string(), "a.ondin".to_string()])
        );
        let refused = unicode_args([ok(), bad].into_iter()).unwrap_err();
        assert!(refused.contains("not valid Unicode"), "{refused}");
    }

    /// `window_icon` decodes as the 512px Ondin mark: a transparent corner and
    /// middle, opaque points on the coil. The size pins the file, since
    /// `icon-64.png` and `icon-1024.png` are the same artwork.
    #[test]
    fn window_icon_is_the_ondin_mark_at_the_size_windows_scales_from() {
        let icon = window_icon();
        assert_eq!((icon.width, icon.height), (512, 512));
        assert_eq!(icon.rgba.len(), 512 * 512 * 4);

        let alpha_at = |x: usize, y: usize| icon.rgba[(y * 512 + x) * 4 + 3];
        assert_eq!(alpha_at(0, 0), 0, "the mark must not fill its box");
        assert_eq!(alpha_at(256, 256), 0, "the coil is a ring, not a disc");
        assert_eq!(alpha_at(60, 300), 255, "the left of the coil is solid ink");
        assert_eq!(alpha_at(256, 470), 255, "and so is its bottom");
    }

    #[test]
    fn defaults_to_gui() {
        assert_eq!(parse(&[]), Ok(Command::Gui { open: None }));
        assert_eq!(parse(&["gui".into()]), Ok(Command::Gui { open: None }));
    }

    /// A first argument naming a document opens it, as an absolute path, and reaches
    /// the install hook as a GUI launch does; a typo of a subcommand is still refused.
    /// flip: `names_a_document` answering false → fails at the `.ondin` launch.
    #[test]
    fn a_document_on_the_command_line_opens_in_the_gui() {
        let open = |a: &str| match parse(&[a.to_string()]) {
            Ok(Command::Gui { open: Some(p) }) => Some(p),
            _ => None,
        };
        let named = open("Landing.ONDIN").expect("a .ondin name opens, whatever its case");
        assert!(named.is_absolute() && named.ends_with("Landing.ONDIN"));
        // An existing file of any name opens too (the loader says what is wrong with it).
        let file = std::env::temp_dir().join("ondin-launch-arg-test.txt");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(open(file.to_str().unwrap()), Some(file.clone()));
        let _ = std::fs::remove_file(&file);
        assert!(hook_runs(&["a.ondin".to_string()]));
        assert!(
            parse(&["exprot".into()]).is_err(),
            "not a file, not a .ondin"
        );
        assert!(
            parse(&["--a.ondin".into()]).is_err(),
            "a flag is not a file"
        );
        assert!(
            parse(&["a.ondin".into(), "b.ondin".into()]).is_err(),
            "one document per launch"
        );
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

    /// `diff` takes exactly two documents and one flag, and refuses the rest.
    #[test]
    fn diff_takes_two_documents_and_json() {
        let diff = |s: &[&str]| {
            let mut v = vec!["diff".to_string()];
            v.extend(s.iter().map(|a| a.to_string()));
            parse(&v)
        };
        assert_eq!(
            diff(&["a.ondin", "--json", "b.ondin"]),
            Ok(Command::Diff {
                a: "a.ondin".into(),
                b: "b.ondin".into(),
                json: true,
            })
        );
        for refused in [
            &["a.ondin"][..],
            &["a.ondin", "b.ondin", "c.ondin"],
            &["a.ondin", "b.ondin", "--svg"],
        ] {
            assert!(diff(refused).is_err(), "{refused:?}");
        }
    }

    /// Two saves of one document differ in nothing and print nothing; an edit
    /// differs and names the layer; a missing file is an error, not a difference.
    #[test]
    fn diff_files_answers_whether_two_documents_differ() {
        use ondin_core::{IdSource, NodeKind, Operation, Transaction};
        let dir = std::env::temp_dir().join(format!("ondin-diff-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut ids = IdSource::new(0xD2);
        let root = ids.mint();
        let rect = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: rect,
            parent: root,
            index: 0,
            kind: NodeKind::Ellipse {
                size: ondin_core::kurbo::Size::new(10.0, 10.0),
            },
            transform: None,
            name: Some("Dot".into()),
        }]))
        .unwrap();
        let a = dir.join("a.ondin");
        std::fs::write(&a, ondin_core::io::save(&doc).unwrap()).unwrap();
        doc.apply(&Transaction(vec![Operation::SetName {
            id: rect,
            name: "Spot".into(),
        }]))
        .unwrap();
        let b = dir.join("b.ondin");
        std::fs::write(&b, ondin_core::io::save(&doc).unwrap()).unwrap();
        let (a, b) = (a.to_str().unwrap(), b.to_str().unwrap());

        assert_eq!(diff_files(a, a, false), Ok((String::new(), false)));
        let (text, differs) = diff_files(a, b, false).unwrap();
        assert!(differs);
        assert!(
            text.contains("~ Spot") && text.contains("name: “Dot” → “Spot”"),
            "{text}"
        );
        let (json, _) = diff_files(a, b, true).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["pages"][0]["layers"][0]["status"], "changed");
        let missing = dir.join("missing.ondin");
        assert!(diff_files(a, missing.to_str().unwrap(), false).is_err());
        std::fs::remove_dir_all(&dir).ok();
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
                page: None,
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
                page: None,
            }
        );
        assert_eq!(
            export(&["--json", "a.ondin"]).unwrap(),
            Command::Export {
                file: "a.ondin".into(),
                format: ExportFormat::Json,
                out: None,
                scale: ExportScale::Times(1.0),
                page: None,
            }
        );
        assert_eq!(
            export(&["a.ondin", "--html"]).unwrap(),
            Command::Export {
                file: "a.ondin".into(),
                format: ExportFormat::Html,
                out: None,
                scale: ExportScale::Times(1.0),
                page: None,
            }
        );
        for (flag, format) in [
            ("--react", ExportFormat::React),
            ("--vue", ExportFormat::Vue),
        ] {
            assert_eq!(
                export(&["a.ondin", flag]).unwrap(),
                Command::Export {
                    file: "a.ondin".into(),
                    format,
                    out: None,
                    scale: ExportScale::Times(1.0),
                    page: None,
                }
            );
        }
    }

    /// `--scale` is a PNG argument and is refused wherever it cannot apply rather
    /// than ignored, so a build script cannot think it asked for 2× and get 1×.
    #[test]
    fn scale_is_a_png_argument_and_is_refused_everywhere_else() {
        assert_eq!(
            export(&["a.ondin", "--png", "--scale", "2"]).unwrap(),
            Command::Export {
                file: "a.ondin".into(),
                format: ExportFormat::Png,
                out: None,
                scale: ExportScale::Times(2.0),
                page: None,
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
                page: None,
            }
        );

        // The ways it cannot apply; with no format typed the SVG default is inherited.
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
        // On `--all` the message names the flag that would have worked.
        let err = export(&["a.ondin", "--all", "--scale", "2"]).unwrap_err();
        assert!(err.contains("--scale-override"), "{err}");

        // A number that cannot be a size is a parse error, not a silent 1×.
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

    /// `--width` and `--height` pin an axis; any two of the three size flags
    /// together are refused rather than resolved by a precedence rule.
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
                    page: None,
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

        // Every pair in both orders, so a rule firing one way round fails.
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
            // The message names both flags.
            assert!(err.contains(pair[0]) && err.contains(pair[1]), "{err}");
        }

        // The same flag twice is refused too, not last-one-wins like `--png --svg`.
        assert!(
            export(&["a.ondin", "--png", "--scale", "2", "--scale", "3"]).is_err(),
            "--scale twice"
        );
    }

    /// `--scale-override` belongs to `--all` and takes the export row's own
    /// vocabulary (`ExportScale::from_label`), so one flag covers all three sizes.
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
                    page: None,
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

        // Not on a one-off, where `--scale` is the flag.
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

        // flip: the unconditional `args.get(i)` in the `-o` arm → the first fails;
        // `other => file = Some(…)` without the `file.is_some()` guard → the second
        // fails. The `-o --png` case is about a silent SVG: the format flag is lost.
        assert!(
            export(&["a.ondin", "-o", "--png"]).is_err(),
            "-o swallowed the format flag"
        );
        assert!(
            export(&["a.ondin", "b.ondin"]).is_err(),
            "a second document overwrote the first"
        );
        // The second name is refused wherever it sits.
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
                fills: ondin_core::keyed_by_position([Fill {
                    brush: ondin_core::peniko::Brush::Solid(ondin_core::peniko::Color::from_rgba8(
                        10, 20, 30, 255,
                    ))
                    .into(),
                    visible: true,
                }]),
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
            (ExportFormat::Html, "zip"),
            (ExportFormat::React, "zip"),
            (ExportFormat::Vue, "zip"),
        ] {
            let out = dir.join(format!("out.{ext}"));
            run_export(
                &src,
                format,
                Some(&out.to_string_lossy()),
                ExportScale::Times(1.0),
                None,
            )
            .expect("export succeeds");
            let written = std::fs::read(&out).expect("output exists");
            assert!(!written.is_empty(), "{ext} export was empty");
        }

        // PNG really is a PNG, not an empty stub.
        let png = std::fs::read(dir.join("out.png")).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "PNG signature");
        // And the zip a zip.
        let zip = std::fs::read(dir.join("out.zip")).unwrap();
        assert_eq!(&zip[..4], b"PK\x03\x04", "zip signature");
    }

    /// `--scale 2` doubles both sides of the raster, read out of the PNG header so
    /// the assertion survives the whole write.
    ///
    /// flip: `run_export` pinned back to `viewport_over` → fails at `2× is not
    /// twice 1×`, with both files identical.
    ///
    /// flip: `factor`'s `Width`/`Height` arms exchanged → fails on the `Width` line,
    /// `(128, 96)` against `(96, 72)`. It bites only because the fixture is 32×24.
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
                fills: ondin_core::keyed_by_position([Fill {
                    brush: ondin_core::peniko::Brush::Solid(ondin_core::peniko::Color::from_rgba8(
                        10, 20, 30, 255,
                    ))
                    .into(),
                    visible: true,
                }]),
            },
        ]))
        .unwrap();

        // Process-unique, so a second `cargo test` on the same machine cannot read
        // a half-written file and fail like an export defect.
        let dir = std::env::temp_dir().join(format!("ondin-export-scale-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("doc.ondin");
        std::fs::write(&src, ondin_core::io::save(&doc).unwrap()).unwrap();
        let src = src.to_string_lossy().into_owned();

        let sides = |scale: ExportScale, name: &str| {
            let out = dir.join(name);
            run_export(
                &src,
                ExportFormat::Png,
                Some(&out.to_string_lossy()),
                scale,
                None,
            )
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

        // The pinned axis answers and the other follows: the fixture is 32×24, so 96
        // wide is 3× and 72 tall, and 96 tall is 4× and 128 wide. Asserting both
        // sides of both catches swapped axes.
        assert_eq!(sides(ExportScale::Width(96), "w.png"), (96, 72));
        assert_eq!(sides(ExportScale::Height(96), "h.png"), (128, 96));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The ad-hoc path reports a clamp (`viewport_at`'s), and stays silent on a floor.
    ///
    /// The third case's flip does not bite: `>=` to `==` in `clamp_note` changes
    /// nothing, because `px` takes `ceil` of the same positive product (0.01× of
    /// 100×50 asks for 1×1 and gets 1×1). It pins only that a sub-pixel scale is silent.
    #[test]
    fn a_clamped_scale_is_reported_and_a_floored_one_is_not() {
        use ondin_core::kurbo::Rect;
        let view = Rect::new(0.0, 0.0, 100.0, 50.0);

        // Served exactly: nothing to say.
        assert_eq!(clamp_note(view, ExportScale::Times(2.0), (200, 100)), None);

        // Past `MAX_RASTER_SIDE` on the long side; checked against what
        // `viewport_at` actually returned.
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
        // Named in the vocabulary that was typed, not the multiplier it became.
        assert!(note.starts_with("2000x "), "{note}");
        // The effective multiplier, derived from the cap so a change to
        // `MAX_RASTER_SIDE` moves it too.
        assert!(
            note.contains(&format!(
                "{:.3}×",
                f64::from(ondin_export::png::MAX_RASTER_SIDE) / 100.0
            )),
            "{note}"
        );
        // Proportions kept, within a pixel per side rather than exactly: both sides
        // are `ceil`ed independently after the float multiplier, so `h * 2 == w`
        // would be wrong.
        assert!(
            vp.pixel_size.1 * 2 - vp.pixel_size.0 <= 2,
            "aspect drifted: {:?}",
            vp.pixel_size
        );

        // A pinned axis says so in its own words, which is why `clamp_note` takes the
        // spec and not the factor it resolved to.
        let wide = ExportScale::Width(200_000);
        let vp = ondin_export::png::viewport_at(view, wide.factor(100.0, 50.0));
        let note = clamp_note(view, wide, vp.pixel_size).expect("200000w is past the limit");
        assert!(note.starts_with("200000w "), "{note}");

        // Floored, not clamped: every side is lifted to a whole pixel, and silent.
        let vp = ondin_export::png::viewport_at(view, 0.01);
        assert_eq!(vp.pixel_size, (1, 1));
        assert_eq!(
            clamp_note(view, ExportScale::Times(0.01), vp.pixel_size),
            None
        );
    }

    /// `-o` naming the document is refused and leaves it as it was.
    ///
    /// flip: `run_export` without its same-file check → the document is a zip.
    #[test]
    fn export_refuses_to_write_over_its_document() {
        let dir = std::env::temp_dir().join(format!("ondin-export-self-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("doc.ondin");
        let saved = ondin_core::io::save(&ondin_core::Document::new(
            ondin_core::IdSource::new(7).mint(),
        ))
        .unwrap();
        std::fs::write(&src, &saved).unwrap();
        let src = src.to_string_lossy().into_owned();
        let err = run_export(
            &src,
            ExportFormat::Vue,
            Some(&src),
            ExportScale::Times(1.0),
            None,
        )
        .unwrap_err();
        assert!(err.contains("refusing"), "{err}");
        assert_eq!(
            std::fs::read(&src).unwrap(),
            saved,
            "the document is untouched"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_reports_a_missing_file_instead_of_panicking() {
        let err = run_export(
            "does-not-exist.ondin",
            ExportFormat::Svg,
            None,
            ExportScale::Times(1.0),
            None,
        )
        .unwrap_err();
        assert!(err.contains("cannot read"), "{err}");
    }

    /// `--all` is its own command; a format or `--folders` without it is refused
    /// rather than ignored.
    #[test]
    fn export_all_is_a_command_of_its_own() {
        assert_eq!(
            export(&["a.ondin", "--all"]).unwrap(),
            Command::ExportAll {
                file: "a.ondin".into(),
                out: None,
                folders: false,
                scale_override: None,
                page: None,
            }
        );
        assert_eq!(
            export(&["a.ondin", "--all", "--folders", "-o", "assets"]).unwrap(),
            Command::ExportAll {
                file: "a.ondin".into(),
                out: Some("assets".into()),
                folders: true,
                scale_override: None,
                page: None,
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

    /// `--all` runs the document's own settings headlessly. Asserts the filenames
    /// (suffix rule, extension per format, de-collision), not just that files
    /// appeared; the last two thirds cover `--scale-override` on the same fixture.
    ///
    /// flip: dropping the dedupe in the plan → fails at the override's names,
    /// leaving `["Close (2).png", "Close.png", "Close.svg"]`.
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
                exports: ondin_core::keyed_by_position([
                    ExportSpec::new(Fmt::Png, ExportScale::Times(1.0)),
                    ExportSpec::new(Fmt::Png, ExportScale::Times(2.0)),
                    ExportSpec::new(Fmt::Svg, ExportScale::Times(1.0)),
                ]),
            },
        ]))
        .unwrap();

        // Process-unique: this one opens by deleting the directory.
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
            None,
        )
        .expect("export --all succeeds");

        let mut names: Vec<String> = std::fs::read_dir(&out)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["Close.png", "Close.svg", "Close@2x.png"]);
        // The 2× really is twice as wide, which the filenames cannot say.
        let big = std::fs::read(out.join("Close@2x.png")).unwrap();
        assert_eq!(
            u32::from_be_bytes(big[16..20].try_into().unwrap()),
            48,
            "a 24pt layer at 2× is 48 pixels"
        );

        // `--scale-override 1x` collapses the set: the three specs become a PNG, a
        // PNG and an SVG, two of them the same file, so two files are written.
        let flat = dir.join("flat");
        run_export_all(
            &src.to_string_lossy(),
            Some(&flat.to_string_lossy()),
            false,
            Some(ExportScale::Times(1.0)),
            None,
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

        // A pinned axis overrides just as well: across a set, "everything 96 wide"
        // has an answer.
        let wide = dir.join("wide");
        run_export_all(
            &src.to_string_lossy(),
            Some(&wide.to_string_lossy()),
            false,
            Some(ExportScale::Width(96)),
            None,
        )
        .expect("export --all --scale-override 96w succeeds");
        let big = std::fs::read(wide.join("Close@96w.png")).unwrap();
        assert_eq!(
            u32::from_be_bytes(big[16..20].try_into().unwrap()),
            96,
            "96w means 96 pixels wide whatever the layer's own size"
        );

        // A document with no export settings fails: a build step that writes
        // nothing must not exit 0.
        let bare = dir.join("bare.ondin");
        std::fs::write(&bare, ondin_core::io::save(&Document::new(root)).unwrap()).unwrap();
        let err = run_export_all(&bare.to_string_lossy(), None, false, None, None).unwrap_err();
        assert!(err.contains("no layer has export settings"), "{err}");
    }

    /// A document whose every export file is skipped fails too, and creates no
    /// output directory. The earlier gate counts layers; this counts files produced.
    ///
    /// The spec has to be raster: `plan::render`'s SVG branch always answers
    /// `Some`, so an SVG spec on the same empty group would pass under the defect.
    ///
    /// flip: removing the `written.is_empty()` branch → fails at `expect_err`.
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
                exports: ondin_core::keyed_by_position([ExportSpec::new(
                    Fmt::Png,
                    ExportScale::Times(1.0),
                )]),
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

    /// `--all` says how many files it replaced. The count is asserted on
    /// `write_planned`, since the run only prints it. The second `write_planned`
    /// shows a re-run legitimately replaces everything it wrote last time.
    ///
    /// flip: deleting `if path.exists() { replaced += 1 }` → fails at the first
    /// `write_planned` count, 0 against 1.
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

        // A hand-authored file where a planned export lands, and a second planned
        // file in a folder that does not exist yet.
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

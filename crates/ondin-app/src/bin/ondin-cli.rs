//! `ondin-cli` — the console twin of `ondin` (§15 D953).
//!
//! **Shipped on Windows only, as `ondin.com` beside `ondin.exe`.** A release
//! `ondin.exe` is a GUI-subsystem binary, so launching the app opens no console —
//! and, by the same token, `ondin export …` typed at a prompt has nowhere to
//! print and the prompt returns at once. This is a console-subsystem binary that
//! runs the `ondin` beside it with the same arguments and **its own standard
//! handles**, which the child inherits and writes to, then exits with the child's
//! status. `PATHEXT` lists `.COM` before `.EXE`, so a bare `ondin` at a prompt
//! resolves to this one while shortcuts, the Start menu and Explorer keep
//! launching the window. It is an ordinary PE file: the extension is the whole
//! mechanism, the trick `devenv.com` has used for years.
//!
//! **No logic of its own**, so nothing here can disagree with `ondin`'s argument
//! parsing. Built on every platform so that it never breaks unnoticed; on Linux
//! and macOS `ondin` has a console already and this is not shipped.
#![deny(rustdoc::broken_intra_doc_links, rustdoc::invalid_html_tags)]

use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let app = match std::env::current_exe() {
        Ok(me) => me.with_file_name(if cfg!(windows) { "ondin.exe" } else { "ondin" }),
        Err(e) => {
            eprintln!("ondin: cannot find the ondin binary beside this one: {e}");
            return ExitCode::from(1);
        }
    };
    match Command::new(&app)
        .args(std::env::args_os().skip(1))
        .status()
    {
        Ok(status) => ExitCode::from(status.code().map_or(1, |c| c.clamp(0, 255) as u8)),
        Err(e) => {
            eprintln!("ondin: cannot run {}: {e}", app.display());
            ExitCode::from(1)
        }
    }
}

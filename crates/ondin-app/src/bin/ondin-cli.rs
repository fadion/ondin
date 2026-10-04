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
//!
//! **It supervises the child as a console program's own process would be**
//! (§15 D971): the child's exit code passes through whole ([`exit_code`]), and
//! the child is ended with this process ([`tie_to_this_process`]).
#![deny(rustdoc::broken_intra_doc_links, rustdoc::invalid_html_tags)]

use std::process::Command;

fn main() {
    let app = match std::env::current_exe() {
        Ok(me) => me.with_file_name(if cfg!(windows) { "ondin.exe" } else { "ondin" }),
        Err(e) => {
            eprintln!("ondin: cannot find the ondin binary beside this one: {e}");
            std::process::exit(1);
        }
    };
    let mut child = match Command::new(&app).args(std::env::args_os().skip(1)).spawn() {
        Ok(child) => child,
        Err(e) => {
            eprintln!("ondin: cannot run {}: {e}", app.display());
            std::process::exit(1);
        }
    };
    tie_to_this_process(&child);
    match child.wait() {
        Ok(status) => std::process::exit(exit_code(status.code())),
        Err(e) => {
            eprintln!("ondin: lost {}: {e}", app.display());
            std::process::exit(1);
        }
    }
}

/// The code to exit with for a child that exited with `code`.
///
/// 🚨 **Passed through whole, never squeezed into a byte** (`[R2-L6-01]`).
/// This read `code.clamp(0, 255) as u8`, and a crashed Windows process exits
/// with its NTSTATUS — `0xC0000005` is `-1073741819` as an `i32` — which the
/// clamp turned into **0**: a script running `ondin export` read success from
/// an access violation. `std::process::exit` takes the whole `i32` and Windows
/// keeps all 32 bits; on Unix the child's code is already a byte.
///
/// `None` — a Unix child ended by a signal — is a failure, `1`.
fn exit_code(code: Option<i32>) -> i32 {
    code.unwrap_or(1)
}

/// End the child when this process ends, however it ends (`[X1-L1-01]`).
///
/// **Ctrl+C at the prompt reaches this process and not the child**: a
/// GUI-subsystem `ondin.exe` has no console to be sent the event, so before
/// this the prompt came back while a cancelled `ondin export --all` went on
/// writing files behind it. A job object with *kill on job close* is the
/// Windows mechanism for exactly this: the job's last handle is this
/// process's, the kernel closes it when this process ends — Ctrl+C, the
/// console closed, a kill — and the child goes with it.
///
/// ⚠️ **Assigned after the spawn**, so a child that ends in the first instant
/// is never in the job, which costs nothing. Spawning it suspended would close
/// that gap and needs the main thread's handle, which `std` does not give.
/// Failure is silent: the child runs as it did before.
///
/// **Measured, with a control**: a copy of `ping.exe` named `ondin.exe` beside
/// a debug `ondin-cli.exe`, running `-n 30`, and the twin killed with
/// `Stop-Process -Force` 1.5 s in. With this function the child had ended
/// within 0.8 s; with the `AssignProcessToJobObject` call removed it was still
/// running. A real console's Ctrl+C was not driven.
#[cfg(windows)]
fn tie_to_this_process(child: &std::process::Child) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    // SAFETY: plain Win32 calls on handles this function owns or borrows for
    // the call; the job handle is deliberately never closed, since its closing
    // at process exit is the whole mechanism.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job == 0 {
            return;
        }
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let set = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if set == 0 {
            return;
        }
        AssignProcessToJobObject(job, child.as_raw_handle() as _);
    }
}

/// Off Windows a terminal's Ctrl+C reaches the whole foreground process group,
/// the child included, and this binary is not shipped there anyway.
#[cfg(not(windows))]
fn tie_to_this_process(_child: &std::process::Child) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// A crash's status is a failure, not a success (`[R2-L6-01]`).
    ///
    /// **Flip-check, run**: the old `code.clamp(0, 255)` fails on the access
    /// violation with 0 — the reported symptom, a crash read as success.
    #[test]
    fn a_crashed_childs_status_is_not_success() {
        assert_eq!(exit_code(Some(0)), 0);
        assert_eq!(exit_code(Some(2)), 2, "a refused command line keeps its 2");
        let access_violation = 0xC000_0005_u32 as i32;
        assert_eq!(exit_code(Some(access_violation)), access_violation);
        assert_ne!(exit_code(Some(access_violation)), 0);
        assert_eq!(exit_code(None), 1, "a signal is a failure");
    }
}

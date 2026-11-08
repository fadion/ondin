//! Secrets kept in the system's own secret store and nowhere else: Windows'
//! Credential Manager, the macOS keychain, the Secret Service on Linux. One
//! account each: the Figma personal access token when *Import files* is asked to
//! remember it, and each photo service's key from Settings (`<service>-key`).
//!
//! macOS and Linux go through the platform's own tool (`security`, `secret-tool`)
//! rather than a library, with the secret written to its standard input so it
//! never appears in a process listing. Every call can block on the store, so the
//! card makes them on a worker.

/// The service every secret is filed under, and the Figma token's account.
const SERVICE: &str = "Ondin";
const ACCOUNT: &str = "figma-token";

/// The remembered Figma token, if one is kept.
pub(crate) fn read() -> Option<String> {
    read_account(ACCOUNT)
}

/// Keep the Figma `token`, replacing one kept before.
pub(crate) fn store(token: &str) -> Result<(), String> {
    store_account(ACCOUNT, token)
}

/// Forget the kept Figma token, if there is one.
pub(crate) fn forget() {
    forget_account(ACCOUNT);
}

/// The secret kept under `account`, if any. A test build keeps none, so no test
/// reads or writes the developer's own keychain.
pub(crate) fn read_account(account: &str) -> Option<String> {
    if cfg!(test) {
        return None;
    }
    os::read(account).filter(|t| !t.trim().is_empty())
}

/// Keep `secret` under `account`, replacing one kept before.
pub(crate) fn store_account(account: &str, secret: &str) -> Result<(), String> {
    if cfg!(test) {
        return Ok(());
    }
    os::store(account, secret)
}

/// Forget what `account` keeps, if anything.
pub(crate) fn forget_account(account: &str) {
    if !cfg!(test) {
        os::forget(account);
    }
}

#[cfg(test)]
mod tests {
    /// The store keeps, answers and forgets a token, under an account of its own,
    /// and an empty one reads as none rather than as a slice of nothing.
    /// Ignored: it writes to the real keychain. Run it by hand with `--ignored`.
    #[test]
    #[ignore]
    fn a_token_is_kept_read_and_forgotten() {
        let account = "figma-token-test";
        super::os::store(account, "figd_test").expect("the store keeps it");
        assert_eq!(super::os::read(account).as_deref(), Some("figd_test"));
        super::os::forget(account);
        assert_eq!(super::os::read(account), None);
        if super::os::store(account, "").is_ok() {
            assert_eq!(super::os::read(account).filter(|t| !t.is_empty()), None);
            super::os::forget(account);
        }
    }
}

#[cfg(windows)]
mod os {
    use super::SERVICE;
    use windows_sys::Win32::Security::Credentials::{
        CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree,
        CredReadW, CredWriteW,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn target(account: &str) -> Vec<u16> {
        wide(&format!("{SERVICE}/{account}"))
    }

    pub(super) fn read(account: &str) -> Option<String> {
        let target = target(account);
        let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: `target` is NUL-terminated UTF-16 and outlives the call; on
        // success `cred` points at a block the system owns until `CredFree`.
        let ok = unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut cred) };
        if ok == 0 || cred.is_null() {
            return None;
        }
        // SAFETY: a successful read hands back a valid credential whose blob is
        // `CredentialBlobSize` bytes long; it is copied before being freed. An
        // empty credential may carry a null blob, which no slice may be made of.
        unsafe {
            let c = &*cred;
            let token = if c.CredentialBlob.is_null() || c.CredentialBlobSize == 0 {
                None
            } else {
                let blob =
                    std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize);
                String::from_utf8(blob.to_vec()).ok()
            };
            CredFree(cred.cast());
            token
        }
    }

    pub(super) fn store(account: &str, token: &str) -> Result<(), String> {
        let mut target = target(account);
        let mut user = wide(account);
        let mut blob = token.as_bytes().to_vec();
        // SAFETY: every pointer is to a live buffer owned by this frame; the
        // system copies what it keeps before `CredWriteW` returns.
        let ok = unsafe {
            let mut cred: CREDENTIALW = std::mem::zeroed();
            cred.Type = CRED_TYPE_GENERIC;
            cred.TargetName = target.as_mut_ptr();
            cred.UserName = user.as_mut_ptr();
            cred.CredentialBlobSize = blob.len() as u32;
            cred.CredentialBlob = blob.as_mut_ptr();
            cred.Persist = CRED_PERSIST_LOCAL_MACHINE;
            CredWriteW(&cred, 0)
        };
        if ok == 0 {
            Err("Windows would not keep the secret".into())
        } else {
            Ok(())
        }
    }

    pub(super) fn forget(account: &str) {
        let target = target(account);
        // SAFETY: `target` is NUL-terminated UTF-16 and outlives the call.
        unsafe {
            CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0);
        }
    }
}

#[cfg(not(windows))]
mod os {
    use super::SERVICE;
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    /// Run `program` with `args`, `input` on its standard input; its standard
    /// output when it succeeded.
    fn run(program: &str, args: &[&str], input: Option<&str>) -> Option<String> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
            stdin.write_all(input.as_bytes()).ok()?;
        }
        let out = child.wait_with_output().ok()?;
        out.status.success().then(|| {
            String::from_utf8_lossy(&out.stdout)
                .trim_end_matches('\n')
                .to_owned()
        })
    }

    #[cfg(target_os = "macos")]
    pub(super) fn read(account: &str) -> Option<String> {
        run(
            "security",
            &["find-generic-password", "-s", SERVICE, "-a", account, "-w"],
            None,
        )
    }

    #[cfg(target_os = "macos")]
    pub(super) fn store(account: &str, token: &str) -> Result<(), String> {
        // `-i` reads commands from standard input, which keeps the token out of
        // the argument list; quoting is `security`'s own, so a quote is refused.
        if token.contains(['"', '\n', '\\']) {
            return Err("That secret holds characters the keychain cannot take".into());
        }
        let command =
            format!("add-generic-password -U -s \"{SERVICE}\" -a \"{account}\" -w \"{token}\"\n");
        run("security", &["-i"], Some(&command))
            .map(|_| ())
            .ok_or_else(|| "The keychain would not keep the secret".into())
    }

    #[cfg(target_os = "macos")]
    pub(super) fn forget(account: &str) {
        let _ = run(
            "security",
            &["delete-generic-password", "-s", SERVICE, "-a", account],
            None,
        );
    }

    #[cfg(not(target_os = "macos"))]
    pub(super) fn read(account: &str) -> Option<String> {
        run(
            "secret-tool",
            &["lookup", "service", SERVICE, "account", account],
            None,
        )
    }

    #[cfg(not(target_os = "macos"))]
    pub(super) fn store(account: &str, token: &str) -> Result<(), String> {
        let label = format!("--label=Ondin {account}");
        run(
            "secret-tool",
            &["store", &label, "service", SERVICE, "account", account],
            Some(token),
        )
        .map(|_| ())
        .ok_or_else(|| "No secret store took the secret (is secret-tool installed?)".into())
    }

    #[cfg(not(target_os = "macos"))]
    pub(super) fn forget(account: &str) {
        let _ = run(
            "secret-tool",
            &["clear", "service", SERVICE, "account", account],
            None,
        );
    }
}

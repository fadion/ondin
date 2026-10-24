//! The `.ondin` file type's registration with Windows, so a double-click runs
//! `ondin <file>`. Per user, under `HKCU\Software\Classes`: Velopack's install and
//! update hooks write it and its uninstall hook removes it. Linux declares the same
//! type in its desktop entry and shared-mime-info file (`packaging/linux`).

/// The programmatic id the extension points at.
const PROG_ID: &str = "Ondin.Document";

/// What [`register`] writes: a key under `Software\Classes`, a value name (empty for
/// the key's default) and its data, for the app at `exe`.
fn entries(exe: &str) -> Vec<(String, &'static str, String)> {
    vec![
        (".ondin".into(), "", PROG_ID.into()),
        // Listed for *Open with* too, so another app owning the default leaves it there.
        (r".ondin\OpenWithProgids".into(), PROG_ID, String::new()),
        (PROG_ID.into(), "", "Ondin document".into()),
        (
            format!(r"{PROG_ID}\DefaultIcon"),
            "",
            format!("\"{exe}\",0"),
        ),
        (
            format!(r"{PROG_ID}\shell\open\command"),
            "",
            format!("\"{exe}\" \"%1\""),
        ),
    ]
}

mod os {
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegDeleteTreeW, RegGetValueW,
        RegSetKeyValueW,
    };
    use windows_sys::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn classes(key: &str) -> Vec<u16> {
        wide(&format!(r"Software\Classes\{key}"))
    }

    /// Point `.ondin` at the running executable and tell the shell. A failed write is
    /// logged, not raised: the hook must not fail an install over a file type.
    pub fn register() {
        let Some(exe) = std::env::current_exe()
            .ok()
            .and_then(|p| p.to_str().map(str::to_owned))
        else {
            log::warn!(target: "ondin", "file type: the executable's path is not Unicode");
            return;
        };
        for (key, name, data) in super::entries(&exe) {
            let (key, name, data) = (classes(&key), wide(name), wide(&data));
            // SAFETY: every pointer is a live NUL-terminated UTF-16 buffer and the
            // size is the data's own, terminator included.
            let err = unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    key.as_ptr(),
                    name.as_ptr(),
                    REG_SZ,
                    data.as_ptr().cast(),
                    (data.len() * 2) as u32,
                )
            };
            if err != 0 {
                log::warn!(target: "ondin", "file type: registry write failed ({err})");
            }
        }
        notify();
    }

    /// Remove what [`register`] wrote; `.ondin`'s default only while it is still ours.
    pub fn unregister() {
        let ext = classes(".ondin");
        let mut buf = [0u16; 64];
        let mut size = std::mem::size_of_val(&buf) as u32;
        // SAFETY: `buf` and `size` describe one writable buffer; the names are
        // NUL-terminated.
        let read = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                ext.as_ptr(),
                std::ptr::null(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut size,
            )
        };
        let ours = read == 0 && buf.starts_with(&wide(super::PROG_ID));
        // SAFETY: NUL-terminated names throughout; a missing key or value is an error
        // code, which is the outcome wanted.
        unsafe {
            if ours {
                RegDeleteKeyValueW(HKEY_CURRENT_USER, ext.as_ptr(), std::ptr::null());
            }
            let with = classes(r".ondin\OpenWithProgids");
            RegDeleteKeyValueW(
                HKEY_CURRENT_USER,
                with.as_ptr(),
                wide(super::PROG_ID).as_ptr(),
            );
            RegDeleteTreeW(HKEY_CURRENT_USER, classes(super::PROG_ID).as_ptr());
        }
        notify();
    }

    fn notify() {
        // SAFETY: `SHCNE_ASSOCCHANGED` takes no items.
        unsafe {
            SHChangeNotify(
                SHCNE_ASSOCCHANGED as i32,
                SHCNF_IDLIST,
                std::ptr::null(),
                std::ptr::null(),
            )
        };
    }
}

pub use os::{register, unregister};

#[cfg(test)]
mod tests {
    use super::*;

    /// The open command quotes the executable and the document, so a path with spaces
    /// arrives as one argument, and the extension names the id the command hangs from.
    #[test]
    fn the_open_command_quotes_both_paths() {
        let exe = r"C:\Users\A B\AppData\Local\Ondin\current\ondin.exe";
        let all = entries(exe);
        let value = |key: &str| {
            all.iter()
                .find(|(k, n, _)| k == key && n.is_empty())
                .map(|(_, _, d)| d.clone())
        };
        assert_eq!(value(".ondin").as_deref(), Some(PROG_ID));
        assert_eq!(
            value(r"Ondin.Document\shell\open\command"),
            Some(format!("\"{exe}\" \"%1\""))
        );
        assert_eq!(
            value(r"Ondin.Document\DefaultIcon"),
            Some(format!("\"{exe}\",0"))
        );
    }
}

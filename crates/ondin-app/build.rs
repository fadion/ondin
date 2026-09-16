//! The icon Explorer draws on `ondin.exe`.
//!
//! This is the half of the app icon that `ViewportBuilder::with_icon` cannot
//! reach. That one is RGBA handed to winit at startup and only ever paints a
//! **live window** — title bar, taskbar button, Alt-Tab. A binary sitting in a
//! folder, pinned to the taskbar, or offered as a file association has no window
//! yet, so Windows reads its icon out of the executable's own resource table
//! instead. Only a resource compiler puts one there, which is why this build
//! script exists at all and why the two icons are configured in two places.
//!
//! It has to be the `.ico` and not one of the PNGs beside it, because a resource
//! icon is a *container*: Windows picks the entry matching the size it is about
//! to draw — 16 in a details view, 32 or 48 on the desktop, 256 in the large-icon
//! view — instead of scaling one bitmap to all of them. `convertico-Ondin.ico`
//! carries 16/32/48/64/128/256.
//!
//! `winresource` locates the SDK's `rc.exe` through the registry. That is the
//! same Windows SDK the MSVC Rust toolchain already needs for `link.exe`, so a
//! machine that can build this crate at all can compile the resource — which is
//! why a failure here is fatal rather than a `cargo:warning` and a silently
//! icon-less binary. An icon that is missing on some machines and not others is
//! exactly the kind of thing nobody notices until it ships.

fn main() {
    // Emitting any `rerun-if-changed` replaces cargo's default of re-running this
    // script whenever *anything* in the package changes, so both lines are load
    // bearing: without the second, editing this file would not re-run it.
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(windows)]
    {
        // Relative to `CARGO_MANIFEST_DIR`, which is this crate's root. The icons
        // live at the repository root rather than under `assets/` because two of
        // them (`appstore.png`, `playstore.png`) are store artwork the build never
        // touches; `src/main.rs` reaches for the same folder the same way.
        const ICO: &str = "../../icons/convertico-Ondin.ico";
        println!("cargo:rerun-if-changed={ICO}");

        winresource::WindowsResource::new()
            // Both default to `CARGO_PKG_NAME`, i.e. "ondin-app" — the crate, not
            // the product. `FileDescription` in particular is what Windows shows
            // as the application's name in Task Manager and the UAC prompt, so
            // leaving it is a user-visible wrong name, not just untidy metadata.
            .set("ProductName", "Ondin")
            .set("FileDescription", "Ondin")
            .set_icon(ICO)
            .compile()
            .expect("rc.exe could not compile the executable's icon resource");
    }
}

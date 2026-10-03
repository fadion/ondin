# Ondin

Ondin is a native, open-source 2D design tool written in Rust — Figma-class scope
at a deliberately minimal v1, for Windows, Linux and macOS. Multiple artboards on an
infinite canvas, a nested node tree of shapes, paths and text, multiple
fills and strokes per layer, masks, effects, image adjustments, and SVG/PNG
export. Its document model is API-first: the same operation path that serves the
UI serves AI agents over MCP. The project is foundations-first — identity, the
mutation path, the renderer boundary and serialization matter more than feature
count, because features are cheap to add and seams are expensive to change.

Still early — the v1 feature list is not complete. See
[Releases](https://github.com/fadion/ondin/releases) for what each version brings.

## Installing

Builds are published for **Windows x86_64, Linux x86_64 and macOS on Apple
Silicon** on the [Releases](https://github.com/fadion/ondin/releases) page. The
installed builds **update themselves**: they check GitHub for a new release in the
background, download it, and offer a restart to apply it. Set
`ONDIN_NO_UPDATE_CHECK=1` to stop the app contacting GitHub at all.

**Windows** — run `Ondin-win-x64-Setup.exe`. It installs for your user, with no
administrator prompt. The installer is not code-signed yet, so SmartScreen warns
about an unknown publisher: click *More info*, then *Run anyway*.
`ondin-vX.Y.Z-windows-x86_64.zip` is a portable build that does not update itself.

**macOS** — open `Ondin-osx-arm64.dmg` and drag Ondin to Applications (or run
`Ondin-osx-arm64-Setup.pkg`). The app is not notarized, so the first launch is
refused: open it once, then click **Open Anyway** in **System Settings → Privacy &
Security**, or run `xattr -dr com.apple.quarantine /Applications/Ondin.app`. Or
install from a terminal, which needs no exception:

```sh
curl -fsSL https://raw.githubusercontent.com/fadion/ondin/main/install.sh | bash
```

**Linux** — the same one-liner adds Ondin's signed apt repository on Debian and
Ubuntu, or its dnf/zypper repository on Fedora, RHEL and openSUSE, so your package
manager updates it; anywhere else it installs the self-updating AppImage. Set
`ONDIN_PKG_FAMILY=debian|rpm|appimage` to choose, or `ONDIN_NO_REPO=1` to install
the package without adding a repository. The repositories, and how to add them by
hand, are described at <https://fadion.github.io/ondin>.

| Release file | Updates itself |
| --- | --- |
| `Ondin-win-x64-Setup.exe`, `Ondin-osx-arm64.dmg`, `Ondin-osx-arm64-Setup.pkg` | yes |
| `Ondin-linux-x64.AppImage` | yes |
| `ondin_X.Y.Z_amd64.deb`, `ondin-X.Y.Z-1.x86_64.rpm` | through the repository; not when installed by hand |
| `ondin-vX.Y.Z-windows-x86_64.zip`, `ondin-vX.Y.Z-linux-x86_64.tar.gz` | no |

The `.deb` and `.rpm` on the Releases page are unsigned; the repositories' copies
are signed.

## Building

```sh
cargo build -p ondin-app     # target/debug/ondin(.exe)
cargo test --workspace
```

On Linux the build needs `libxkbcommon-dev libwayland-dev libxcb1-dev libx11-dev
pkg-config` (Debian names).

Rendering can be checked without opening a window — `ondin export` runs the whole
document → pixels/markup path headlessly:

```sh
cargo run -p ondin-app -- export path/to/doc.ondin --svg -o out.svg
```

## Documentation

- [`docs/architecture.md`](docs/architecture.md) — the design and its invariants.
- [`docs/decisions.md`](docs/decisions.md) — every deviation from that design, with a verdict.
- [`docs/roadmap.md`](docs/roadmap.md) — open work, decided non-goals, post-v1.

## License

MIT — see [`LICENSE`](LICENSE). Bundled fonts and other third-party material keep
their own licenses; see [`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md).

# Ondin

A native, open-source 2D design tool written in Rust: frames on an infinite
canvas, vector shapes and paths, rich text, images, effects, and flexbox and
grid layout, with SVG and PNG export.

Still early. The v1 feature list is not complete, and each version's changes are
on the [Releases](https://github.com/fadion/ondin/releases) page.

## Why

- **Fast.** A GPU canvas ([Vello](https://github.com/linebender/vello)) in a
  single native binary. No browser, no runtime to install.
- **Local.** No account, no telemetry. Documents are files on your disk.
- **Real CSS layout.** Flexbox, CSS grid and absolute insets, under their CSS
  names, so a layout means what it will mean in code.
- **Plain files.** `.ondin` is versioned JSON with deterministic bytes, so a
  document diffs and versions like code.

## Features

- **Canvas.** Frames (artboards) on an infinite canvas, nested frames and groups,
  frame presets, rulers, guides, pixel and layout grids, and *Present mode*
  (`Ctrl+\`) to hide every panel.
- **Snapping.** To shapes, equal gaps, text baselines, guides, the grid and
  pixels.
- **Shapes and paths.** Rectangle, ellipse, polygon, star, line and the pen,
  with node editing, per-corner radius, rotate, skew and flip.
- **Boolean operations.** Union, subtract, intersect, exclude, and flatten.
- **Layers.** Drag to reorder and reparent, rename, hide, lock and filter;
  align and distribute; shape and alpha masks.
- **Fills and strokes.** Several of each per layer — solid, linear and radial
  gradients, images — with dashes, caps, joins, inside/centre/outside alignment
  and per-side rectangle strokes.
- **Effects.** Drop shadow, inner shadow, layer blur and colour filters,
  stackable, on shapes and containers.
- **Text.** Rich text with per-range styles, system fonts and the Google Fonts
  catalog on demand, variable-font axes, OpenType features, lists, and text on a
  path.
- **Images.** Place by dialog, drag-and-drop or paste; crop; seven adjustments
  from exposure to shadows.
- **Layout.** Flexbox, CSS grid with its tracks drawn on the canvas, and
  absolute insets.
- **Export.** SVG, PNG and JPEG, per-layer export settings saved in the file, to
  a folder or a ZIP. Copy as SVG or PNG.
- **SVG import.** Paste SVG and get real, editable layers.
- **Library.** A home screen for your documents with search, stars, trash and
  covers. Autosave every 30 seconds and crash recovery.
- **Updates.** Installed builds update themselves.

Ondin is dark-themed only and has had no accessibility pass yet.

## Install

Builds for every release are on the
[Releases page](https://github.com/fadion/ondin/releases/latest), for
**Windows and Linux (x86_64)** and **macOS (Apple Silicon)**. Installed builds
check GitHub for a new release in the background; set `ONDIN_NO_UPDATE_CHECK=1`
to turn that off.

### Windows

Download and run **`Ondin-win-x64-Setup.exe`**. It installs per-user, with no
admin prompt, and updates itself. The installer isn't code-signed, so SmartScreen
warns the first time: click *More info*, then *Run anyway*.

`ondin-vX.Y.Z-windows-x86_64.zip` is a portable build that doesn't update itself.

### macOS

```sh
curl -fsSL https://raw.githubusercontent.com/fadion/ondin/main/install.sh | bash
```

This installs Ondin into `/Applications`, and it updates itself from then on.

To install by hand, take **`Ondin-osx-arm64.dmg`** (or the `.pkg`) and drag Ondin
into `/Applications`. The app isn't signed with an Apple Developer ID, so macOS
blocks the first launch: open it once, then click **Open Anyway** in
**System Settings → Privacy & Security**, or run
`xattr -dr com.apple.quarantine /Applications/Ondin.app`.

### Linux

```sh
curl -fsSL https://raw.githubusercontent.com/fadion/ondin/main/install.sh | bash
```

- **Debian and Ubuntu:** adds the signed apt repository.
- **Fedora, RHEL and openSUSE:** adds the dnf/zypper repository.
- **Anything else:** installs the self-updating AppImage.

Packaged installs update with the rest of your system. Read
[install.sh](install.sh) first if you prefer; it asks for `sudo`.
`ONDIN_PKG_FAMILY=debian|rpm|appimage` overrides its choice, and
`ONDIN_NO_REPO=1` installs a single package without adding a repository.

#### Adding the repository by hand

Debian, Ubuntu and derivatives:

```sh
curl -fsSL https://fadion.github.io/ondin/ondin-archive-keyring.gpg \
  | sudo tee /usr/share/keyrings/ondin-archive-keyring.gpg > /dev/null
curl -fsSL https://fadion.github.io/ondin/ondin.sources \
  | sudo tee /etc/apt/sources.list.d/ondin.sources > /dev/null
sudo apt-get update && sudo apt-get install ondin
```

Fedora, RHEL and CentOS (on openSUSE, `zypper` in place of `dnf`):

```sh
sudo curl -fsSL https://fadion.github.io/ondin/ondin.repo \
  -o /etc/yum.repos.d/ondin.repo
sudo dnf install ondin
```

The first `dnf install` asks to import the signing key. Check that it matches
this fingerprint before you accept:

```
REPLACE_WITH_THE_REPOSITORY_KEY_FINGERPRINT
```

#### Release files

| File | Install | Updates itself |
| --- | --- | --- |
| `Ondin-linux-x64.AppImage` | `chmod +x` and run | Yes |
| `ondin_X.Y.Z_amd64.deb` | `sudo apt-get install ./ondin_*.deb` | No |
| `ondin-X.Y.Z-1.x86_64.rpm` | `sudo dnf install --nogpgcheck ./ondin-*.rpm` | No |
| `ondin-vX.Y.Z-linux-x86_64.tar.gz` | Extract anywhere | No |

The `.deb` and `.rpm` on the Releases page are unsigned; the repository copies
are signed.

## Command line

`ondin export` renders a document without opening a window or needing a GPU:

```sh
ondin export design.ondin --png --scale 2 -o design.png
ondin export design.ondin --svg -o design.svg
ondin export design.ondin --all -o exports/
```

- `--svg` (the default), `--png` or `--json`.
- `--scale`, `--width` or `--height` size a PNG, one at a time.
- `--all` runs the export settings saved in the document; with `--folders`, a
  `/` in a layer's name becomes a subfolder.

## Build & run

Requires a recent Rust toolchain (edition 2024).

```sh
cargo run -p ondin-app
cargo test --workspace
```

On Linux the build also needs a few GUI libraries.

Debian / Ubuntu:

```sh
sudo apt-get install -y libxkbcommon-dev libwayland-dev libxcb1-dev libx11-dev pkg-config
```

Fedora / RHEL:

```sh
sudo dnf install libxkbcommon-devel wayland-devel libxcb-devel libX11-devel pkgconf-pkg-config
```

Arch:

```sh
sudo pacman -S --needed libxkbcommon wayland libxcb libx11 pkgconf
```

## Documentation

- [`docs/architecture.md`](docs/architecture.md) — the design and its invariants.
- [`docs/decisions.md`](docs/decisions.md) — every deviation from that design, with a verdict.
- [`docs/roadmap.md`](docs/roadmap.md) — open work and decided non-goals.
- [`docs/shortcuts.md`](docs/shortcuts.md) — the keymap.

## Licence

MIT, see [LICENSE](LICENSE). Bundled fonts and other third-party material keep
their own licences; see [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).

#!/usr/bin/env bash
#
# Build a Debian package around an already-built Ondin binary.
#
#     packaging/linux/build-deb.sh <version> <binary> <output-dir> [deb-arch]
#
# Deliberately not `cargo-deb`. The binary this wraps is cross-built by
# `cargo-zigbuild` against glibc 2.31 so it runs on Debian 11 / Ubuntu 20.04 and
# newer, and cargo-deb's `depends = "$auto"` resolves shared libraries through
# `dpkg-shlibdeps`, which reads the *build host's* package versions - on a
# ubuntu-latest runner that would stamp the package with Ubuntu 24.04 minimums
# and refuse to install on precisely the distributions the zigbuild target
# exists to reach. So the dependency list below is written by hand, and the
# rules for changing it are in the comment above it.
#
# (Ported from Schemaic, which ships the same winit + wgpu stack this way. The
# glibc 2.31 floor is a promise release.yml's Linux leg has to keep: if that leg
# ever builds natively on the runner instead, the floor below becomes untrue.)
#
set -euo pipefail

if [ "$#" -lt 3 ] || [ "$#" -gt 4 ]; then
    echo "usage: $0 <version> <binary> <output-dir> [deb-arch]" >&2
    exit 2
fi

VERSION="${1#v}"
BINARY="$2"
OUTPUT_DIR="$3"
DEB_ARCH="${4:-amd64}"

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "${HERE}/../.." && pwd)"

# Where a bug report should go. Not a personal address on purpose - this string
# ships inside every package that is ever downloaded and is not retractable
# afterwards. Change it here if a real contact address is wanted.
MAINTAINER="Ondin contributors <fadion@users.noreply.github.com>"

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
ROOT="${STAGE}/ondin"

bash "${HERE}/stage-payload.sh" "$VERSION" "$BINARY" "$ROOT"
mkdir -p "${ROOT}/DEBIAN"

# Debian's machine-readable copyright file. The bundled fonts and icon set have
# their own terms and are named here rather than folded into the MIT blanket -
# Inter and Phosphor, which the app draws with, and egui's four built-in fonts,
# which stay in the binary as fallbacks (THIRD-PARTY-NOTICES.md, "egui's built-in
# fonts"). `sed` turns the LICENSE into the indented, dot-for-blank-line form the
# format requires.
{
    cat <<'EOF'
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: ondin
Source: https://github.com/fadion/ondin

Files: *
Copyright: 2026 Ondin contributors
License: MIT

Files: usr/share/doc/ondin/Inter-OFL.txt
Copyright: 2020 The Inter Project Authors (https://github.com/rsms/inter)
License: OFL-1.1
 The Inter typeface is licensed under the SIL Open Font License 1.1; the full
 text is in /usr/share/doc/ondin/Inter-OFL.txt.

Files: usr/share/doc/ondin/Phosphor-LICENSE.txt
Copyright: 2020 Phosphor Icons
License: MIT
 The Phosphor icon font is licensed under the MIT license; the full text is in
 /usr/share/doc/ondin/Phosphor-LICENSE.txt.

Files: usr/share/doc/ondin/Ubuntu-Font-Licence-1.0.txt
Copyright: Canonical Ltd.
License: Ubuntu-font-1.0
 egui's built-in Ubuntu-Light font is licensed under the Ubuntu Font Licence
 1.0; the full text is in /usr/share/doc/ondin/Ubuntu-Font-Licence-1.0.txt.

Files: usr/share/doc/ondin/Hack-LICENSE.txt
Copyright: 2018 Source Foundry Authors
           2003 Bitstream, Inc.
License: MIT and Bitstream-Vera
 egui's built-in Hack-Regular font is licensed under the MIT license, with
 portions under the Bitstream Vera License and public-domain DejaVu work; the
 full text is in /usr/share/doc/ondin/Hack-LICENSE.txt.

Files: usr/share/doc/ondin/NotoEmoji-OFL-1.1.txt
Copyright: Google Inc.
License: OFL-1.1
 egui's built-in Noto Emoji font is licensed under the SIL Open Font License
 1.1; the full text is in /usr/share/doc/ondin/NotoEmoji-OFL-1.1.txt.

Files: usr/share/doc/ondin/emoji-icon-font-LICENSE.txt
Copyright: 2014 John Slegers
License: MIT
 egui's built-in emoji-icon-font is licensed under the MIT license; the full
 text is in /usr/share/doc/ondin/emoji-icon-font-LICENSE.txt.

License: MIT
EOF
    sed 's/^$/./; s/^/ /' "${REPO}/LICENSE"
} > "${ROOT}/usr/share/doc/ondin/copyright"
chmod 0644 "${ROOT}/usr/share/doc/ondin/copyright"

# Every one of these is loaded with dlopen, not linked: on Schemaic, which this
# list comes from, `readelf -d` on the binary lists glibc and nothing else,
# because winit reaches X11, Wayland and xkbcommon through libloading and wgpu
# reaches Vulkan and EGL the same way. Ondin runs the same two crates through
# eframe, so the same libraries are asked for the same way. That is exactly why
# the list is written out rather than derived - no dependency scanner, Debian's
# or rpm's, can see a library the loader only asks for at runtime, so an
# automatic list produces a package that installs cleanly and then cannot open a
# window.
#
# To regenerate after a dependency change (and to confirm the list for Ondin's
# own binary, which has not been measured):
#     readelf -d target/.../ondin | grep NEEDED
#     strings -a target/.../ondin | grep -oE 'lib[a-zA-Z0-9_+-]+\.so(\.[0-9]+)*' | sort -u
#
# These are Debian's *package* names, where the rpm spec requires *sonames*
# instead. That asymmetry is not a stylistic choice: rpm distributions
# auto-provide `libfoo.so.N()(64bit)` so one spec covers Fedora, RHEL and
# openSUSE, while dpkg has no soname provides and the package name is the only
# thing to depend on.
#
# libc6 (>= 2.31) restates the zigbuild floor rather than the binary's actual
# high-water mark: the build target is the promise being made, and it is the
# number that stays true when the code changes. (Schemaic measured its own
# high-water mark at GLIBC_2.30; Ondin's is unmeasured -
# `objdump -T ondin | grep -oE 'GLIBC_[0-9.]+' | sort -uV | tail -n1` says.)
DEPENDS="libc6 (>= 2.31)"
DEPENDS="${DEPENDS}, libxkbcommon0, libxkbcommon-x11-0"
DEPENDS="${DEPENDS}, libwayland-client0, libwayland-egl1"
DEPENDS="${DEPENDS}, libx11-6, libx11-xcb1, libxcb1"
# winit's X11 backend opens these two beside libX11 and refuses to start
# without either (`XConnection::new`, winit 0.30.13) — read in its source, not
# measured, and missing from the list Schemaic measured (§15 D974).
DEPENDS="${DEPENDS}, libxcursor1, libxi6"
DEPENDS="${DEPENDS}, libvulkan1, libegl1"

INSTALLED_SIZE="$(du -ks --exclude=DEBIAN "$ROOT" | cut -f1)"

cat > "${ROOT}/DEBIAN/control" <<EOF
Package: ondin
Version: ${VERSION}
Architecture: ${DEB_ARCH}
Maintainer: ${MAINTAINER}
Installed-Size: ${INSTALLED_SIZE}
Depends: ${DEPENDS}
Recommends: mesa-vulkan-drivers
Section: graphics
Priority: optional
Homepage: https://github.com/fadion/ondin
Description: Native 2D design tool with a GPU canvas and CSS-style layout
 Ondin is a native, open-source 2D design tool written in Rust. Multiple
 artboards sit on an infinite, GPU-rendered canvas, holding a nested tree of
 frames, shapes, paths and rich text, with boolean operations and masks.
 .
 Every layer takes multiple fills and strokes and a stack of effects. Frames
 lay out their children with flexbox and CSS grid, a project library keeps
 documents together, and drawings export to SVG and PNG.
 .
 Ondin is early in its development: the v1 feature list is not complete.
EOF

mkdir -p "$OUTPUT_DIR"
OUT="${OUTPUT_DIR}/ondin_${VERSION}_${DEB_ARCH}.deb"
# --root-owner-group so the package does not carry the build user's uid, which
# would otherwise be whatever the CI runner happens to use.
dpkg-deb --root-owner-group --build "$ROOT" "$OUT" >/dev/null
echo "$OUT"

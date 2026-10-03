#!/usr/bin/env bash
#
# Lay out the files an Ondin Linux package installs, under a given root.
#
#     packaging/linux/stage-payload.sh <version> <binary> <root>
#
# Shared by build-deb.sh and build-rpm.sh so the two formats cannot drift into
# shipping different files - the packaging metadata differs between them by
# necessity, the payload does not.
#
set -euo pipefail

if [ "$#" -ne 3 ]; then
    echo "usage: $0 <version> <binary> <root>" >&2
    exit 2
fi

VERSION="${1#v}"
BINARY="$2"
ROOT="$3"

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "${HERE}/../.." && pwd)"
APP_ID="io.github.fadion.Ondin"

# The one hicolor icon: icons/icon.png, 512x512. A desktop scales down from the
# largest size it finds, so one large PNG serves every size - Schemaic ships
# the same icon set the same way.
ICON="${REPO}/icons/icon.png"

# The licence texts of the third-party material the binary embeds (Phosphor,
# and egui's built-in fallback fonts - THIRD-PARTY-NOTICES.md has which is
# which). Named one by one so each is checked to exist below, the same as every
# other file here; the check after the loop is what stops this list going stale
# when a new one is added to licenses/.
LICENSE_FILES="Hack-LICENSE.txt NotoEmoji-OFL-1.1.txt Phosphor-LICENSE.txt Ubuntu-Font-Licence-1.0.txt emoji-icon-font-LICENSE.txt"

required=("$BINARY" \
    "${HERE}/${APP_ID}.desktop" \
    "${HERE}/${APP_ID}.metainfo.xml" \
    "${REPO}/LICENSE" \
    "${REPO}/THIRD-PARTY-NOTICES.md" \
    "${REPO}/README.md" \
    "${REPO}/crates/ondin-core/assets/fonts/OFL.txt" \
    "$ICON")
for l in $LICENSE_FILES; do
    required+=("${REPO}/licenses/${l}")
done
for f in "${required[@]}"; do
    [ -f "$f" ] || { echo "missing required file: $f" >&2; exit 1; }
done

# The other direction: a licence text in licenses/ that the list above does not
# name would ship in no package, silently, while THIRD-PARTY-NOTICES.md points
# readers at it. A notice the distribution is obliged to carry is not a thing to
# lose by forgetting one line. (The rpm spec names the same files again, and
# rpmbuild's unpackaged-files check is that list's backstop.)
shopt -s nullglob
for f in "${REPO}"/licenses/*.txt; do
    case " ${LICENSE_FILES} " in
        *" $(basename "$f") "*) ;;
        *)
            echo "licenses/$(basename "$f") is not in stage-payload.sh's LICENSE_FILES" >&2
            echo "(and so would ship in no package). Add it there and to ondin.spec's %files." >&2
            exit 1
            ;;
    esac
done
shopt -u nullglob

mkdir -p "${ROOT}/usr/bin" \
    "${ROOT}/usr/share/applications" \
    "${ROOT}/usr/share/metainfo" \
    "${ROOT}/usr/share/icons/hicolor/512x512/apps" \
    "${ROOT}/usr/share/doc/ondin"

install -m 0755 "$BINARY" "${ROOT}/usr/bin/ondin"
install -m 0644 "${HERE}/${APP_ID}.desktop" "${ROOT}/usr/share/applications/${APP_ID}.desktop"

# icons/icon.png is 512x512 (measured 2026-10-03), which is the hicolor
# directory it goes in. A mismatch here is not a build error - the icon simply
# never resolves, and the app shows a generic one.
install -m 0644 "$ICON" \
    "${ROOT}/usr/share/icons/hicolor/512x512/apps/${APP_ID}.png"

# The checked-in metainfo carries whatever version was current when it was last
# touched; the package must state its own, or a software centre keeps offering
# an update to a version already installed.
sed -E "s|<release version=\"[^\"]*\" date=\"[^\"]*\"/>|<release version=\"${VERSION}\" date=\"$(date -u +%Y-%m-%d)\"/>|" \
    "${HERE}/${APP_ID}.metainfo.xml" > "${ROOT}/usr/share/metainfo/${APP_ID}.metainfo.xml"
chmod 0644 "${ROOT}/usr/share/metainfo/${APP_ID}.metainfo.xml"
# `sed` substitutes zero occurrences without an error, so a <release> line that
# no longer has the exact shape above would ship the stale version unchanged.
grep -q "<release version=\"${VERSION}\" " "${ROOT}/usr/share/metainfo/${APP_ID}.metainfo.xml" \
    || { echo "the metainfo's <release> line was not rewritten to ${VERSION}" >&2; exit 1; }

install -m 0644 "${REPO}/LICENSE" "${ROOT}/usr/share/doc/ondin/LICENSE"
install -m 0644 "${REPO}/THIRD-PARTY-NOTICES.md" "${ROOT}/usr/share/doc/ondin/THIRD-PARTY-NOTICES.md"
install -m 0644 "${REPO}/README.md" "${ROOT}/usr/share/doc/ondin/README.md"
# Renamed on the way in: "OFL.txt" alone does not say whose licence it is once
# it sits beside four other licence texts.
install -m 0644 "${REPO}/crates/ondin-core/assets/fonts/OFL.txt" \
    "${ROOT}/usr/share/doc/ondin/Inter-OFL.txt"
for l in $LICENSE_FILES; do
    install -m 0644 "${REPO}/licenses/${l}" "${ROOT}/usr/share/doc/ondin/${l}"
done

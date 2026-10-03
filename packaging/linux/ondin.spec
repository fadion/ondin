# RPM spec for an already-built Ondin binary. Driven by build-rpm.sh, which
# stages the payload and passes the version in; see that script for why this is
# packaged from a prebuilt binary rather than compiled here.
%global appid io.github.fadion.Ondin

# There is no debuginfo to extract. The release binary is produced by
# cargo-zigbuild with Cargo's default release profile, which emits no DWARF
# (`debug = 0`; Schemaic, built the same way, measured no .debug_* sections
# with `readelf -S`), so leaving the debuginfo machinery on would fail the build
# over an empty package rather than produce anything useful.
%global debug_package %{nil}

# A fallback so the spec parses when read on its own; build-rpm.sh always passes
# the real one with --define.
%{!?ondin_version: %global ondin_version 0.0.0}

Name:           ondin
Version:        %{ondin_version}
Release:        1%{?dist}
Summary:        Native 2D design tool with a GPU canvas and CSS-style layout

License:        MIT
URL:            https://github.com/fadion/ondin

# Every one of these is loaded with dlopen, not linked, so rpm's automatic
# dependency generator cannot see any of them - it reads ELF DT_NEEDED entries,
# and the binary's list is glibc and nothing else (measured on Schemaic's
# binary, which this spec is ported from and which runs the same winit and
# wgpu; build-deb.sh has the commands to confirm it for Ondin's). winit reaches
# X11, Wayland and xkbcommon through libloading, and wgpu reaches Vulkan and EGL
# the same way. Without these lines the package installs cleanly and then cannot
# open a window.
#
# Written as *sonames* rather than package names on purpose. Every rpm
# distribution auto-provides `libfoo.so.N()(64bit)` for the package that ships
# that library, so one spec resolves correctly on Fedora, RHEL and openSUSE
# alike - which matters, because those three disagree on nearly every one of
# these package names (Fedora's libX11 is openSUSE's libX11-6). The .deb has no
# such option: dpkg has no soname provides, so build-deb.sh hard-codes Debian
# package names instead.
Requires:       libxkbcommon.so.0()(64bit)
Requires:       libxkbcommon-x11.so.0()(64bit)
Requires:       libwayland-client.so.0()(64bit)
Requires:       libwayland-egl.so.1()(64bit)
Requires:       libX11.so.6()(64bit)
Requires:       libX11-xcb.so.1()(64bit)
Requires:       libxcb.so.1()(64bit)
Requires:       libvulkan.so.1()(64bit)
Requires:       libEGL.so.1()(64bit)

# The Vulkan loader above is only the dispatch library; something has to
# implement it. Weak rather than hard because a machine may have a vendor
# driver instead, and because wgpu falls back to the GL backend.
Recommends:     mesa-vulkan-drivers

%description
Ondin is a native, open-source 2D design tool written in Rust. Multiple
artboards sit on an infinite, GPU-rendered canvas, holding a nested tree of
frames, shapes, paths and rich text, with boolean operations and masks.

Every layer takes multiple fills and strokes and a stack of effects. Frames
lay out their children with flexbox and CSS grid, a project library keeps
documents together, and drawings export to SVG and PNG.

Ondin is early in its development: the v1 feature list is not complete.

%prep
# Nothing to unpack: build-rpm.sh stages the installed tree under
# %%{_sourcedir}/payload and %%install copies it in whole.

%build
# Nothing to build.

%install
cp -a %{_sourcedir}/payload/. %{buildroot}/

# Every file is named, not globbed: rpmbuild refuses a staged file no line here
# claims ("Installed (but unpackaged) file(s) found"), which is what keeps this
# list and stage-payload.sh's in step - the licence texts above all.
%files
%license %{_datadir}/doc/ondin/LICENSE
%doc %{_datadir}/doc/ondin/README.md
%doc %{_datadir}/doc/ondin/THIRD-PARTY-NOTICES.md
%doc %{_datadir}/doc/ondin/Inter-OFL.txt
%doc %{_datadir}/doc/ondin/Hack-LICENSE.txt
%doc %{_datadir}/doc/ondin/NotoEmoji-OFL-1.1.txt
%doc %{_datadir}/doc/ondin/Phosphor-LICENSE.txt
%doc %{_datadir}/doc/ondin/Ubuntu-Font-Licence-1.0.txt
%doc %{_datadir}/doc/ondin/emoji-icon-font-LICENSE.txt
%dir %{_datadir}/doc/ondin
%{_bindir}/ondin
%{_datadir}/applications/%{appid}.desktop
%{_datadir}/metainfo/%{appid}.metainfo.xml
%{_datadir}/icons/hicolor/512x512/apps/%{appid}.png

%changelog
# Intentionally empty. The release notes on the GitHub Release are the
# changelog, and a second copy maintained by hand here would only go stale.

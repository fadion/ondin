# Third-party notices

Ondin itself is licensed under the MIT License (see [`LICENSE`](LICENSE)).

Distributed builds of Ondin incorporate third-party material. This file records
the notices those licenses require to accompany a distribution. **Nothing below
imposes copyleft on Ondin's own source** — every dependency is permissively
licensed, dual-licensed with a permissive option that Ondin elects, or weak
(file-level) copyleft used unmodified.

Audited 2026-09-16 against the resolved lockfile: **502 third-party packages**,
plus the bundled assets below.

## Bundled assets (embedded in the binary)

### Inter

`Inter-Variable.ttf` and `Inter-Italic-Variable.ttf` are embedded via
`include_bytes!` (`crates/ondin-core/assets/fonts/`). They are the *only* fonts
`ondin-core` shapes with — the crate never scans system fonts, which is what
makes layout deterministic.

- Copyright 2020 The Inter Project Authors (https://github.com/rsms/inter),
  designed by Rasmus Andersson.
- Licensed under the SIL Open Font License, Version 1.1 (`OFL-1.1`).
- Full license text:
  [`crates/ondin-core/assets/fonts/OFL.txt`](crates/ondin-core/assets/fonts/OFL.txt).

The OFL permits bundling and redistribution, including commercially. Inter
remains under the OFL — it is not relicensed under MIT. ⚠️ **Inter declares no
Reserved Font Name**, so the usual OFL rename-on-modification restriction does
not apply here; but "Inter" is a trademark of rsms, per the font's own
`Trademark` name record.

### Phosphor Icons

`Phosphor.ttf` is embedded via `include_bytes!`
(`crates/ondin-app/assets/fonts/`) and provides every icon glyph in the UI.

- Phosphor, Version 2.1 — Tobias Fried & Helena Zhang, https://phosphoricons.com
- MIT License, Copyright (c) 2020 Phosphor Icons.
- Full license text: [`licenses/Phosphor-LICENSE.txt`](licenses/Phosphor-LICENSE.txt).

The font declares `LicenseDescription: MIT` in its own OpenType `name` table;
that record and its `LicenseURL` are quoted at the head of the license file.

### egui's built-in fonts

🚨 **These ship whether or not you expect them to.** `theme::install` starts from
`egui::FontDefinitions::default()` and inserts Inter and Phosphor *in front of*
the built-ins rather than replacing them, so egui's fonts remain in the binary
**and remain live as fallbacks** for any glyph Inter lacks. They arrive through
`epaint_default_fonts`, whose crate license expression is
`(MIT OR Apache-2.0) AND OFL-1.1 AND Ubuntu-font-1.0` — the `AND`s are the fonts.

| Font | License | Text |
| --- | --- | --- |
| Ubuntu-Light | Ubuntu Font Licence 1.0 | [`licenses/Ubuntu-Font-Licence-1.0.txt`](licenses/Ubuntu-Font-Licence-1.0.txt) |
| Hack-Regular | MIT (© 2018 Source Foundry Authors), with Bitstream Vera License portions (© 2003 Bitstream, Inc.) and public-domain DejaVu work | [`licenses/Hack-LICENSE.txt`](licenses/Hack-LICENSE.txt) |
| NotoEmoji-Regular | SIL Open Font License 1.1 | [`licenses/NotoEmoji-OFL-1.1.txt`](licenses/NotoEmoji-OFL-1.1.txt) |
| emoji-icon-font | MIT | [`licenses/emoji-icon-font-LICENSE.txt`](licenses/emoji-icon-font-LICENSE.txt) |

All four are free to bundle and redistribute. Two carry Reserved Font Names —
"Bitstream" and "Vera" via Hack's Bitstream Vera portion — which restrict
distributing a *modified* font under those names; Ondin modifies none of them.
The Ubuntu Font Licence likewise requires a renamed derivative, and permits
embedding and redistribution unchanged.

⚠️ **If these are ever unwanted, the lever is `epaint`'s `default_fonts`
feature**, not a code change — turning it off drops all four fonts and this whole
subsection, at the cost of the fallback coverage.

### Application icon

`icons/convertico-Ondin.ico` is compiled into `ondin.exe` as a Win32 resource by
`crates/ondin-app/build.rs`, and `icons/convertico-Ondin_256x256.png` is embedded
as the runtime window icon. Both are original project artwork and are covered by
Ondin's own MIT license.

## Rust dependencies (statically linked)

A release binary statically links a tree of Rust crates. Their licenses are all
permissive or permissively-electable:

- MIT, Apache-2.0 (incl. `WITH LLVM-exception`), BSD-2-Clause, BSD-3-Clause,
  ISC, Zlib, 0BSD, Unlicense, BSL-1.0, Unicode-3.0, CC0-1.0.
- **Dual-licensed crates are used under a permissive option.** Two mention
  copyleft and neither applies any of its terms:
  - `self_cell` — `Apache-2.0 OR GPL-2.0-only`, used under **Apache-2.0**.
  - `r-efi` — `MIT OR Apache-2.0 OR LGPL-2.1-or-later`, used under **MIT**.
- **One weak-copyleft crate**: `option-ext` (`MPL-2.0`), reached through
  `dirs → dirs-sys`, which is how the font cache and preferences directories are
  located. MPL-2.0 is **file-level** copyleft: it obliges anyone distributing
  *modified versions of those files* to publish them, and does not reach Ondin's
  own source. The crate is used **unmodified** from crates.io, where its source
  is already public. It is in the Windows dependency graph, so it does ship.
- `ring` (`Apache-2.0 AND ISC`) and `webpki-roots` (`CDLA-Permissive-2.0`) arrive
  through `ureq`'s TLS stack, which is how the Font tab downloads a web font.
  `CDLA-Permissive-2.0` is a *data* license covering Mozilla's CA root
  certificate bundle, and a permissive one — use and redistribution with no
  copyleft and no obligation beyond the disclaimer.
- `tiny-skia` and `tiny-skia-path` (`BSD-3-Clause`) are Skia-derived and arrive
  through the CPU raster path.

No `GPL`, `AGPL`, `LGPL`, `CDDL`, `EPL` or `SSPL` terms apply to any part of a
distributed Ondin build.

The complete per-crate manifest can be regenerated from the lockfile:

```sh
cargo install cargo-about
cargo about generate about.hbs > THIRD-PARTY-LICENSES.html
```

The license policy is enforced by `cargo-deny` (see [`deny.toml`](deny.toml)):

```sh
cargo deny check licenses
```

which fails if a dependency's license falls outside the allow-list — so a future
`cargo update` cannot silently pull in a GPL/AGPL crate. ⚠️ **There is no CI, so
nothing runs this for you**; the `release` skill's Phase 0 is where it belongs.

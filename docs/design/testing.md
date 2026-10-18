# 11. Testing strategy

How each layer is tested, what the golden files do and do not cover, and the isolation rules that keep tests off the developer's machine. (Original §11.)

## Layers

- **`ondin-core`**: exhaustive unit and property tests, zero GPU. Random op sequences preserve tree invariants (acyclic, parent/child consistency, id uniqueness); undo/redo is exact; `update ≡ rebuild`; serialization is deterministic (save∘load∘save = save).
- **`ondin-export` and CPU render**: golden files, byte-exact, runnable on any CI box with the bundled font only.
- **GPU**: feature-gated smoke tests with perceptual tolerance; not in the default CI gate.
- **Colour choke point**: per-backend pixel verification, re-run on every renderer version bump (invariant 7).
- **Round-trip gates**: Document → SVG and Document → snapshot cover every NodeKind. `the_fixture_holds_one_node_of_every_kind` walks the golden document and checks the set against `kind_name`'s own arms; the exhaustive `match` makes a new variant a compile error whose repair grows the set.

## Goldens

`crates/ondin-export/tests/goldens.rs` compares `tests/goldens/every-kind.svg` and `every-kind.json` byte for byte against a fixture holding one node of every `NodeKind` (all four effect kinds, a `Normalized` pivot). Regenerate with `ONDIN_UPDATE_GOLDENS=1 cargo test -p ondin-export --test goldens`; read the diff before committing. The fixture is that file's own, not the shared `common::fixture()` (eight kinds, two dozen substring assertions). Every other export test asserts on substrings and in-process measurements; the goldens complement them by catching a value that merely changed.

- **Coverage sets are read off the code.** `every_kind` and `every_effect` are read from the arms of the exhaustive `kind_name` and `effect_name` matches, so a new variant is in neither golden until the fixture grows. Hand-kept lists let a one-arm repair pass with the fixture unchanged.
- **Cross-platform byte stability** requires that no file carries a full-precision float from transcendental arithmetic. The JSON snapshot writes paths through the SVG writer's four-place `path_d`, and rounds `world_bounds` and the transforms to four places. `pivot` is not rounded and is exact in the fixture's one pivot (D968, D972).
- **Line endings.** `.gitattributes` marks the golden directory `-text` and pins every `.rs` to LF, because `goldens.rs` and `theme.rs` each have a test that splits its own source on `"\n}\n"`; CRLF checkouts break it (D967).
- **Standing gaps.** The fixture has no image brush (only `Solid` and `Gradient`), so a new brush kind is not a compile error and image export changes must be checked against a browser. `<mask>`, `<radialGradient>`, `<image>`, `<textPath>` and `<use>` have no golden coverage; one masked node would close most of it, but a mask restructures the surrounding markup and that diff wants reading on its own.
- **PNG golden is deliberately absent.** `vello_cpu` accuracy varies with SIMD level (a stroked path differs between AVX2 and scalar); export pins `Level::baseline()`, which is target-derived, so a checked-in raster would be an x86_64 golden failing on aarch64. NEON is unmeasured (D304).
- **A PNG golden is also per-`vello_cpu`-version.** The `=0.0.9` → `=0.2.0` bump moved 162 bytes over 78 pixels of 571,200 (worst channel delta 1), all from one mitered outside stroke. Whoever bumps next sees a diff on every stroked shape: regenerate, confirm the residual is confined to strokes at delta ≤ 1, and treat a residual anywhere else as the finding. `cpu.rs` `raster_settings` spells out all four rasterizer settings, because the bump moved `RenderMode` to another struct and `..Default::default()` kept compiling while changing meaning (D414).
- A byte comparison cannot see a value that did not change (an optimisation like linear-time def collection changes no bytes) or its own fixture's blind spot (a field elided by `skip_serializing_if` that the fixture never sets, such as `effects` and `pivot` once were).

## Tests must not touch the machine

`OndinApp::headless` swaps in inert versions of every outside resource, so the rule holds by construction rather than discipline:

| Resource | Mechanism |
|---|---|
| wgpu device | inert |
| `FontService` background threads | not started |
| Preferences file | `Prefs::ephemeral` (`#[serde(skip)]`), set by `headless` and checked on `save`'s first line; a flag on the value because `prefs.save()` has nine call sites (D370) |
| OS clipboard | process-wide `CLIPBOARD_OFF` flag checked at the four callers (`system_clipboard_text`, `system_clipboard_has_image`, `paste_image`, `copy_as_png`); `copy_as_png` reports it in the status line. A static because the clipboard is per process; checked at the callers, not inside `app::with_clipboard`, so the lock test still opens a real handle (D796, D798) |
| Per-machine library index | `library::cache::index_is_reachable()` is `!cfg!(test)`, read first in `LocalIndex::load` and `save`. A `cfg!` rather than `headless`'s flag because `Library::open` rebuilds the index and tests call it directly after building the app (D807, §9.5) |

Every `arboard` handle opened by this crate does so under one `Mutex` (`app::with_clipboard`): two threads opening the global clipboard at once corrupt the heap and kill the test binary. "This crate", not "the process": `egui-winit` keeps its own handle for `ctx.copy_text`, safe only while both are driven from the one UI thread (D859).

## Traps

- A golden that stays byte-identical may mean the change is invisible to it, not that the change is safe.
- After a `vello_cpu` bump, stroke diffs are expected; non-stroke diffs are not.
- Clipboard-touching tests collide only at certain interleavings; run a module's filter alone repeatedly.
- A fixture helper that sets a flag in a constructor is discarded by tests that call lower-level opens directly; use a `cfg!` seam there.

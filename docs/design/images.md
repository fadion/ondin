# Images

How pictures live in a document (`ondin_core::image`, §5.5a): the image table, the fill reference, framing and orientation, per-fill adjustments, the decode cache in `ondin-render`, placing, dropping and pasting, and the on-disk form.

## 5.5a Images (`image.rs`)

Built: table, reference, brush, decode cache, framing, image editing (`Tool::ImageEdit`: crop gesture and mode strip), orientation, tile scale, reset crop, drop and paste, SVG export (an image fill becomes a `<pattern>` over the encoded original, written once per picture), the missing-image placeholder with *Replace...*, seven per-pixel adjustments in all three writers, and thumbnails. Not built: fetching a linked image. `ImageStore::prepare` skips linked entries (reading files is not `ondin-render`'s job); whoever has the bytes calls `ImageStore::insert`. A link therefore behaves exactly like a missing picture.

### Model

- **An image is a fill, not a `NodeKind`.** Any shape, path or boolean can carry one. `Paint::image_fill` is the one spelling of "is this layer a picture" (index of the first visible image fill); `tools::croppable_fill` is a one-line wrapper. Interaction is layer-shaped: placing makes a Rect sized to the image.
- **The document owns a table keyed by an opaque `ImageId`**; a `Fill` holds an `ImageRef`. The id is a content hash of the encoded bytes (`sha256:<hex>`, through `ring`, so `sha256sum` of an exported original reproduces it), computed by whoever supplies the bytes; core neither computes nor verifies it. Dedup is the caller's question (`Document::has_image`).
- **Stored bytes are the encoded original**, never a decoded buffer. `ImageEntry::href` gives `data:<mime>;base64,...` for embedded images, the stored path verbatim for linked ones; it lives in core so a document and its export say the same thing. The original goes out, never a re-encode.
- **Four accepted encodings: PNG, JPEG, WebP, GIF** (`ImageFormat`, which owns the MIME type). A GIF is its first frame; animation and video are out. TIFF/AVIF are excluded as a decision. The closed list is enforced on decode: `ondin-render`'s `images::signature_is_accepted` uses `image::guess_format` (a fixed magic-byte table, independent of which decoders cargo compiled) and refuses any recognised format outside the four. Feature unification can compile extra decoders (`bmp` on Windows, `tiff` on macOS); without the guard one document would draw on Windows and show the placeholder on Linux. Unrecognised bytes still go to the hinted decoder.
- **Embedded by default, linked as an escape hatch** (`ImageSource`). A link stores the path as a string.
- **The intrinsic size is stored on the entry**, so layout and layers rows never wait on a decode.
- **Nothing garbage-collects the table.** `AddImage`/`RemoveImage` are explicit and `AddImage` carries the whole entry so undo restores bytes. Unused images are for a *Purge unused* command.
- **A dangling reference is a state, not an error.** A fill naming a missing id loads and saves intact. It draws the missing-image placeholder (grey with a cross, described once by `image::missing_placeholder`, painted by the walk, the CPU export and the SVG writer), the layers row tints its glyph, and *Replace...* in the image popover repairs it. Never silently dropped, never a silent blank. A malformed table (bad base64, zero intrinsic size) is rejected. A boolean whose arithmetic was abandoned draws the same placeholder.

### The brush

The model's brush is `peniko::Brush<ImageBrush<ImageRef>, GradientBrush>`, aliased `ondin_core::Brush`. `GradientBrush` is a `peniko::Gradient` plus an affine from authored space to the shape's space: the identity for a new gradient (`paint::default_brush`) and a round radial, a circle's squash for an elliptical radial (imported, or shaped by the canvas handles), and the box's scale once the layer is resized (§6.2). Consequences:

- **The save format did not move**: `Solid` and `Gradient` are the same variants; the gradient transform is `#[serde(flatten)]` with `skip_serializing_if` at identity.
- `alpha` and `extend` are not restated: `ImageBrush` carries an `ImageSampler` with both plus quality.
- **`ondin_render::color::brush_to_backend` is the one point where a reference becomes pixels**, returning `Option<(peniko::Brush, Affine)>` via `ImageStore::pixels` (not `get`: `get` answers the decode, `pixels` what the fill shows; the affine is the buffer's own grid). `None` means "cannot be resolved yet" (dangling id, unread link, undecodable bytes): a fill is diverted to the placeholder first, a stroke or text run draws nothing. Each backend resolves for itself: `vello_cpu` needs a `vello_cpu::ImageSource`, so `cpu.rs` reads the image arm off the model's brush and memoizes per render keyed on the id and the adjustment's bits.

### Gradient stops

Stop offsets ascend: `image::make_stops_monotonic` clamps each offset up to the running maximum (SVG's rule). A descending ramp otherwise draws as a flat fill of the first stop. It is applied at four independent doors: `svg_in::Gradients::collect`, `color::brush_to_backend`, `ondin_export::svg::gradient_def` (on a copy), and the gradient chips (`ui::paint_ramp`), all sharing `image::monotonic_offset`. It is a clamp, not a sort (`[0,0.8,0.2,1]` clamps to `[0,0.8,0.8,1]`); `panels::paint::with_stops` sorts, because dragging a handle past a neighbour means "put it there". Every gradient read — a load, a paste, an MCP call — is clamped too: `GradientBrush` deserializes through `GradientBrushWire`, so a stored ramp is already the picture the renderers draw.

A stop's alpha is clamped in `svg_in::stop_of` (import) and `panels::paint::alpha_of` (readout); nothing clamps on load.

**A ramp's opacity is a field on `GradientBrush`** (a multiplier beside the stops), not a rewrite of stops (scaling is not invertible through zero). It is folded in at `color::brush_to_backend` and written as `fill-opacity` by `svg::paint_attr`; `picker::ramp` folds it into chips; the stop bar uses `picker::stop_colours` (the named exception, so the handles are not dimmed). The field is `#[serde(default)]`, so `build::brush_is_finite` asks `valid_opacity` to refuse finite out-of-range values. For pictures, chips read `panels::paint::image_brush_alpha` (and `image_alpha`) and the layers row's `picture_of` must carry the sampler.

### Framing

A picture sits in its frame by a fit: `ImageFit` on `ImageRef`, resolved by `ImageRef::framing(frame, width, height)` into `Framing` (brush transform, `peniko::Extend`, optional clip). One function serves both backends and the SVG writer. No mode resamples; the same decoded blob is drawn under a different brush transform.

- **Modes**: `Fill` (cover, default), `Fit` (contain), `Crop`, `Tile`. `ImageFit` is a bare enum; `crop` and `tile_scale` are fields on `ImageRef` and persist while unselected. Fill and Fit re-derive against the current frame; Crop and Tile each read one parameter.
- **Orientation** `ImageOrient { quarters, mirrored }` is read by all four modes. `framing` composes `orient.affine` under the mode's transform and runs the mode against `orient.oriented_size`. Eight states in canonical mirror-then-turn order; ops are `OrientOp::{FlipH, FlipV, Turn}` composed onto the current orientation. The popover offers the two mirrors only; nothing writes `Turn` (four of eight states are reachable). `applying` resolves `FlipV` to `quarters: 2`.
- **A crop rectangle is normalized (0..1) to the picture as oriented**, so *Replace...* keeps framing across a differently sized picture. A flip or turn carries the rectangle (`OrientOp::crop_carried`, reached only through `ImageRef::reoriented`). `image::whole_crop()` is a sentinel meaning "never cropped" (it stretches on a different aspect); *reset crop* restores it. The rectangle maps onto the frame exactly, so aspect is the frame's business.
- **Entering cropping replaces the sentinel with `ImageRef::cover_crop`** (what Fill already shows), so a Fill picture does not move. For a Fit picture it does move: a Crop framing maps its rectangle onto the whole frame, so a letterbox needs a rectangle larger than the source, which `crop_clamped` shrinks to the cover one. A slot spanning a selection (or an unknowable frame) keeps the sentinel and jumps. Cover is the widest reachable rectangle; the frame is clamped to `ImageRef::picture_box`. Pure helpers in `image.rs`: `cover_crop`, `cover_scale`, `picture_box`, `crop_scale`, `crop_panned`, `crop_reframed`, `crop_scaled`, `crop_clamped`. `crop_reframed` is deliberately unclamped (the caller holds the frame); `crop_scaled` is clamped.
- **A tile is anchored at the frame's top-left** at `scale` times the intrinsic size (1.0 = one unit per source pixel).
- **The frame is the shape's local bounding box** (not the ink), the same space as gradients (§6.2), so widening a border does not reframe.
- **Fit costs a layer** (peniko has no transparent extend), emitted only where aspects differ.
- **Degenerate means unusable**: a zero source, empty frame or zero-area crop gives `Framing::identity`; a non-finite or non-positive tile scale draws 1:1. `image::usable_span` (`> 0 && finite`) is the single predicate. Known gap: `Crop`'s `-r.x0 * iw` can overflow with a usable span.
- **All four framing fields** (`fit`, `crop`, `tile_scale`, `orient`) are `#[serde(default, skip_serializing_if)]`, each with its own predicate, so older files are byte-identical.

### Adjustments

`ImageAdjust` on `ImageRef`: seven `f32` (exposure, contrast, saturation, temperature, tint, highlights, shadows), all 0.0 at rest, held in `-1..=1`.

- **Per fill, not per entry**: two layers showing one photograph adjust independently. It is the only `ImageRef` field that is not geometry: it produces a second buffer.
- **`ImageAdjust::pipeline()`** is the one resolution all three writers read. `None` for neutral and for non-finite or out-of-range values (refused, not clamped, so the file draws what it says). Out-of-range matters: `exposure: 1000` made `inf*0 = NaN` coefficients and a black rect, and the SVG writer would emit `NaN` in `feColorMatrix`.
- **`AdjustPipeline`**: an optional 4x5 colour matrix and a per-channel transfer table of `TONE_STEPS = 33` samples, exactly `feColorMatrix type="matrix"` and `feComponentTransfer type="table"`. Pixel backends evaluate the same numbers with `ondin_core::transfer` (SVG's table interpolation). Exposure, contrast, saturation, temperature, tint collapse into the matrix (saturation uses 0.213/0.715/0.072; contrast pivots on mid-grey via column five); highlights and shadows are the curve (strength +-0.4, the largest monotonic). The matrix runs before the curve.
- **`Adjustment`** is the list (`Adjustment::ALL`): panel rows, cache key and snapshot all read it. `Adjustment::set` clamps (non-finite -> 0); `pipeline` refuses; `op_set_fills` checks finiteness through `build::image_ref_is_finite`.
- `adjust` is `#[serde(default, skip_serializing_if)]` and each slider carries its own predicate.

## The decode cache (`ondin_render::images::ImageStore`)

- Lives in `ondin-render` because the app canvas and `ondin-export`'s PNG both need pixels; core has no image dependency.
- Holds `peniko::ImageData` (straight RGBA8, `Arc` blob). The CPU `vello_cpu::ImageSource` premultiplies a whole buffer, so `cpu.rs` memoizes per render.
- **`prepare(doc)` runs every frame** (a hash lookup per entry once decoded).
- **Budget bounds waste, not the working set**: eviction only drops an image the document stopped carrying, oldest first.
- **A refused decode is remembered.** Edges above 65535 are refused (`MAX_EDGE`). `MAX_DECODED_BYTES` weighs `width * height * 4` before `into_rgba8` (a small grayscale PNG can expand hugely and abort); error variant `TooMuchPixelData` is separate from `TooLarge`.
- **Every failure ends at the placeholder** (fills only). Strokes and text with a missing picture draw nothing.
- **Derived pixels** keyed by `(ImageId, AdjustKey)` (the seven values' bit patterns; neutral never reaches a key), behind a `Mutex`, filled where read. `ImageStore::pixels(&ImageRef)` is what backends paint with; `get(&ImageId)` is the decode and the right question for `has_image`.
- **Preview then promote**: a miss is answered with a picture reduced by a whole factor toward `PREVIEW_EDGE = 1024` (a floor: reduced long edge lands in [1024, 2048)), box-averaged premultiplied, rebuilt at full resolution after the same value is asked for `SETTLED_FRAMES = 2` consecutive frames.
- **`Pixels` carries `to_source`**, the map from the buffer grid to source pixels; `color::framed_transform` composes it with the framing and both backends use it.
- **Atlas budget**: vello keeps all a scene's pictures in one 8192-pixel atlas (`ATLAS_EDGE`) and silently drops a picture that does not fit. `prepare` computes one integer factor for the whole page (`images::atlas_downscale`, searching upward from 1) from the store's decoded population, not from what a frame draws, using shelf packing (shelf fitting implies guillotine fitting). Past `MAX_ATLAS_DOWNSCALE` = 64 the silent drop returns; `has_image` does not see atlas failures. Canvas uses `color::Resolution::Atlas`, export uses `Source`.
- **`ImageStore::new()` never previews; `ImageStore::interactive()` does**; `CanvasRenderer` is its only caller. `wants_frame()` says a promotion is pending; `canvas::draw_canvas` owes it a `request_repaint`.
- **Derived buffers**: previews go when stale; full buffers go under `ADJUSTED_BYTES`, stale first then least recently asked. `trim_derived` runs on insert in `pixels` as well as from `prepare` (non-interactive callers prepare once). It evicts a live buffer if no stale one exists.
- **Thumbnails**: `ImageStore::thumbnail(&ImageRef, edge)` touches neither the ask clock nor `wants_frame`. It reduces first, adjusts second, over a `THUMB_BASE = 128` copy per `ImageId` evicted with the picture. Orientation and crop are applied; the framing mode is not (centre-crop).

## Placing (`Tool::Image`, `tools::create_image`, §9.4)

- **Not a persistent mode**: arming = picking files (rail button / `Ctrl+Shift+K` open the dialog and arm only if something loaded). Each click places one in pick order; after the last, the tool returns to Select.
- **The cursor carries the count** (`Bitmap::Loaded(Remaining)`: crosshair plus chip, exact to 99, `99+` beyond; plain crosshair when empty).
- **Loaded images live on the app**, cleared by `choose_tool` for anything but `Tool::Image`.
- **A click fits, a drag draws.** Click: centred, `fitted_image_size` (shrink to fit, never scale up) against the frame under the point or the viewport. Drag: the picture fills the drawn box.
- **Read, hashed and decoded before arming**, so an undrawable file never becomes a placement.
- **Placing the same file twice adds a reference**: caller omits `AddImage` if `Document::has_image`; `build::missing_image_ops` does this for pastes. Ids from other documents are the same key for the same bytes.
- **Layer name**: the filename, else `"Image N"` (`canvas::image_layer_name`).

## Dropping and pasting (§9.4)

- `load_image_bytes(bytes, ext, name)` is the one loader (hash, sniff, decode, name); file reading wraps it. Format sniffing takes `Option<&str>` extension; name is `Option<String>`.
- **Drop or paste onto a selected shape replaces its picture and keeps framing**; elsewhere makes a layer. Only a single file replaces. A locked layer is not a target (`canvas::image_drop_target` uses `query::is_effectively_locked`). A drop must land on the shape (the selection-alone arm is the chord paste). On Windows no drop event carries a position, so `canvas::drop_point` reads `GetCursorPos` mapped against the viewport `inner_rect`; fallback is the viewport centre. Context menu *Paste here* needs a selected shape under the press, else makes a layer at the press.
- **Drag-over announcement**: `canvas::draw_drop_indicator` outlines the landing frame (`target_artboard`), shows a pill naming the outcome, and outlines the shape to be replaced; the count is of files.
- **A pasted image is re-encoded to PNG before hashing** (clipboard RGBA has no original), so its id does not dedupe with a file placement.
- **SVG is refused, not rasterized**, sniffed by `svg_in::looks_like_svg` (the single predicate; `app::looks_like_svg` is only the byte adapter with a 512-byte cap and lossy decode). It skips an XML declaration and leading comments, and refuses a BOM deliberately (open question, wants a real file).
- **A refusal says which thing went wrong and carries the numbers** (`app::ImageRefusal`): unreadable vs. a good picture refused by a bound (e.g. "panorama.png is 70000 x 100; the largest Ondin can draw is 65535 on a side"). `TooMuchPixelData` gets its own sentence. Multi-file failures get one sentence per file. `paste_image` says nothing (the bytes are the app's own encode).

## On disk (§5.11)

The table is written last, sorted by id, one entry per record, embedded bytes as base64 (not serde number arrays). A document with no images writes no key. One file, not a zip (a zip makes every save an opaque blob for version control).

## Traps

- The closed format list is enforced by magic-byte sniffing on decode; an extension fallback or a foreign MIME type alone bypasses it.
- `pixels` vs `get`: `get` is the decode, `pixels` is what the fill shows. Use `pixels` for painting, `get` for `has_image`.
- Canvas (Atlas) and export (Source) intentionally draw at different resolutions.
- Stop monotonicity has four independent doors; a new gradient reader needs the rule too, and gradient opacity must be folded in by any new chip.
- Adjustment out-of-range values are refused, not clamped; a NaN matrix is a backend divergence.
- A crop is normalized to the *oriented* picture; flips must carry the rectangle through `ImageRef::reoriented`.
- A helper returning half a brush (e.g. `picture_of` dropping the sampler) loses the alpha everywhere.

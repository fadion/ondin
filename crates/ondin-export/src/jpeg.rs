//! JPEG export (§7.2) — the same raster [`crate::png`] produces, encoded lossily
//! and without an alpha channel.
//!
//! **No renderer of its own, and that is the whole design.** Everything up to the
//! pixels is `png::raster_of`, so a JPEG and a PNG of the same layer at the same
//! scale differ in exactly one step: this one. There is no second framing rule to
//! drift, and no second place for a trim or a pad to round differently.
//!
//! **The encoder is `image`'s**, already in the workspace lock and already
//! compiled for `ondin-render`'s decode cache (§5.5a), so this declares an edge
//! rather than adding a tree — the reasoning §3 records for `skrifa`, `arboard`
//! and `windows-sys`.

use image::ImageEncoder;
use image::codecs::jpeg::JpegEncoder;

/// Encode a straight-alpha RGBA8 buffer as JPEG at `quality` (1–100).
///
/// **Alpha is dropped, not flattened here.** JPEG has no alpha channel, and the
/// matte a transparent pixel should land on is a *decision* — the export's
/// background — so it is made in `crate::plan` and composited by
/// `png::raster_of` before this is ever called. What arrives here is expected to
/// be opaque already; a buffer that is not simply loses its transparency to
/// whatever colour was underneath it, which is the same thing every JPEG encoder
/// does and is why the decision is not left this late.
pub fn encode_rgba8(rgba: &[u8], width: u32, height: u32, quality: u8) -> Vec<u8> {
    // Three bytes per pixel, dropping the fourth. `as_chunks::<4>()` rather than an
    // index walk so a buffer whose length disagrees with its stated size truncates
    // instead of panicking on the last row.
    let mut rgb = Vec::with_capacity((width as usize * height as usize).saturating_mul(3));
    for px in rgba.as_chunks::<4>().0 {
        rgb.extend_from_slice(&px[..3]);
    }
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100))
        .write_image(&rgb, width, height, image::ExtendedColorType::Rgb8)
        .expect("jpeg encode of an in-memory buffer");
    out
}

#[cfg(test)]
mod tests {
    /// The bytes have to *be* a JPEG, which the magic number says and nothing
    /// else in the pipeline checks — an export writes whatever comes back here
    /// under a `.jpg` name, so a silently empty or mis-encoded buffer would reach
    /// the disk looking like a file.
    #[test]
    fn the_output_is_a_jpeg_of_the_stated_size() {
        // A 2×2 with one transparent pixel: the encoder must take it and must not
        // be handed the alpha byte as colour.
        let rgba = vec![
            255, 0, 0, 255, // red
            0, 255, 0, 255, // green
            0, 0, 255, 255, // blue
            0, 0, 0, 0, // transparent
        ];
        let jpg = super::encode_rgba8(&rgba, 2, 2, 90);
        assert_eq!(&jpg[..2], &[0xFF, 0xD8], "SOI marker: {:02X?}", &jpg[..4]);
        assert_eq!(
            &jpg[jpg.len() - 2..],
            &[0xFF, 0xD9],
            "EOI marker — a truncated file would still start right"
        );

        let decoded = image::load_from_memory(&jpg).expect("our own output must decode");
        assert_eq!((decoded.width(), decoded.height()), (2, 2));
    }

    /// Quality is a real knob rather than a field that is stored and ignored —
    /// asserted by size, because the one thing every JPEG encoder guarantees is
    /// that a lower quality is a smaller file.
    #[test]
    fn quality_changes_the_file() {
        // Noise, not a flat fill: a solid colour compresses to nearly the same
        // size at any quality, which would pass this test for the wrong reason.
        let rgba: Vec<u8> = (0..64 * 64 * 4)
            .map(|i: u32| (i.wrapping_mul(2654435761) >> 16) as u8)
            .collect();
        let low = super::encode_rgba8(&rgba, 64, 64, 20).len();
        let high = super::encode_rgba8(&rgba, 64, 64, 95).len();
        assert!(low < high, "quality 20 gave {low} bytes, quality 95 {high}");
    }
}

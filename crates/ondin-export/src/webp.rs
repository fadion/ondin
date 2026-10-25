//! WebP export — the raster [`crate::png`] produces, encoded as lossless WebP.
//!
//! Like [`crate::jpeg`], no renderer of its own: everything up to the pixels is
//! `png::raster_of`. `image`'s encoder writes VP8L only, so there is no quality
//! knob; a lossy encoder would be libwebp, a C dependency.

use image::ImageEncoder;
use image::codecs::webp::WebPEncoder;

/// The longest side a WebP can hold (libwebp's `WEBP_MAX_DIMENSION`), well under
/// the rasterizer's [`crate::png::MAX_RASTER_SIDE`]; `plan::raster_opts` caps a
/// WebP scale to it. Not 16384: the encoder accepts that and writes a file
/// `image` itself refuses to decode.
pub const MAX_SIDE: u32 = 16383;

/// Encode a straight-alpha RGBA8 buffer as lossless WebP, alpha kept.
///
/// Panics past 16384 on either side; `plan` never hands it such a raster.
pub fn encode_rgba8(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::new();
    WebPEncoder::new_lossless(&mut out)
        .write_image(rgba, width, height, image::ExtendedColorType::Rgba8)
        .expect("webp encode of an in-memory buffer");
    out
}

#[cfg(test)]
mod tests {
    /// The bytes are a WebP of the stated size whose pixels, alpha included,
    /// read back exactly.
    #[test]
    fn the_output_is_a_lossless_webp_of_the_stated_size() {
        let rgba = vec![
            255, 0, 0, 255, // red
            0, 255, 0, 128, // half-transparent green
            0, 0, 255, 255, // blue
            0, 0, 0, 0, // transparent
        ];
        let webp = super::encode_rgba8(&rgba, 2, 2);
        assert_eq!(&webp[..4], b"RIFF", "{:02X?}", &webp[..12]);
        assert_eq!(&webp[8..12], b"WEBP");

        let decoded = image::load_from_memory(&webp)
            .expect("our own output must decode")
            .to_rgba8();
        assert_eq!((decoded.width(), decoded.height()), (2, 2));
        assert_eq!(decoded.into_raw(), rgba, "lossless: every byte survives");
    }
}

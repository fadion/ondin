//! Image fills as `background` layers, and the picture files they link to.
//!
//! A layer reads its framing off `ImageRef::framing`, so the four modes land
//! where the canvas puts them: Fill and Fit are `cover` and `contain`, Crop and
//! Tile an exact size and position. What CSS cannot do to a background — turn or
//! mirror it — is baked into the file instead, which is the original bytes
//! whenever nothing needed baking.

use super::css::px;
use super::dom::slug;
use ondin_core::kurbo::{Rect, Size};
use ondin_core::peniko::{Extend, ImageQuality};
use ondin_core::{Document, ImageBrush, ImageFit, ImageFormat, ImageOrient, ImageRef};

/// An image fill CSS can draw: its `background` layer and the file it needs.
pub(crate) struct Layer {
    pub css: String,
    pub path: String,
    /// Nearest-neighbour sampling, which the element asks for as a whole.
    pub pixelated: bool,
}

/// `brush` as a background layer on a `size` box, or why CSS cannot draw it.
pub(crate) fn layer(doc: &Document, brush: &ImageBrush, size: Size) -> Result<Layer, String> {
    let r = &brush.image;
    let entry = doc.image(&r.id).ok_or("a missing picture")?;
    if entry.is_linked() {
        return Err("a linked picture".into());
    }
    if !r.adjust.is_default() {
        return Err("a picture with colour adjustments".into());
    }
    // CSS has no opacity for one background layer.
    if brush.sampler.alpha < 1.0 {
        return Err("a picture fill under 100%".into());
    }
    // The file carries the orientation, so the framing is the oriented picture's.
    let (iw, ih) = r.oriented_size(entry.width, entry.height);
    let upright = ImageRef {
        orient: ImageOrient::IDENTITY,
        ..r.clone()
    };
    let frame = Rect::from_origin_size((0.0, 0.0), size);
    let framing = upright.framing(frame, iw.round() as u32, ih.round() as u32);
    let [a, b, c, d, e, f] = framing.transform.as_coeffs();
    if b != 0.0 || c != 0.0 || a <= 0.0 || d <= 0.0 {
        return Err("a picture CSS cannot frame".into());
    }
    let (w, h) = (iw * a, ih * d);
    // Past the picture's edge the canvas smears its last pixels, which CSS cannot.
    let tol = 1e-6;
    if framing.extend == Extend::Pad
        && framing.clip.is_none()
        && (e > tol || f > tol || e + w < size.width - tol || f + h < size.height - tol)
    {
        return Err("a crop reaching past the picture".into());
    }
    let path = path_of(r, entry.format);
    let place = match r.fit {
        ImageFit::Fill => "center / cover no-repeat".to_string(),
        ImageFit::Fit => "center / contain no-repeat".to_string(),
        ImageFit::Crop => format!("{} {} / {} {} no-repeat", px(e), px(f), px(w), px(h)),
        ImageFit::Tile => format!("{} {} / {} {} repeat", px(e), px(f), px(w), px(h)),
    };
    Ok(Layer {
        css: format!("url(\"{path}\") {place}"),
        path,
        pixelated: brush.sampler.quality == ImageQuality::Low,
    })
}

/// Where a picture's file goes: one per image and orientation, so two fills of
/// one picture share it.
fn path_of(r: &ImageRef, format: ImageFormat) -> String {
    let mut name = slug(&r.id.0);
    if name.is_empty() {
        name = "picture".into();
    }
    let o = r.orient;
    if o != ImageOrient::IDENTITY {
        name.push_str(&format!(
            "-r{}{}",
            o.quarters(),
            if o.mirrored { "m" } else { "" }
        ));
    }
    let ext = if needs_bake(r, format) {
        "png"
    } else {
        format.extension()
    };
    format!("assets/{name}.{ext}")
}

/// Whether the stored bytes cannot be shipped as they are: a turned or mirrored
/// picture, or a GIF, which the canvas draws as its first frame and a browser
/// would animate.
fn needs_bake(r: &ImageRef, format: ImageFormat) -> bool {
    r.orient != ImageOrient::IDENTITY || format == ImageFormat::Gif
}

/// The file at [`Layer::path`]: the stored bytes, or the decoded picture turned
/// as `r` says and written as PNG. `None` when the bytes do not decode.
pub(crate) fn file(doc: &Document, r: &ImageRef) -> Option<Vec<u8>> {
    let entry = doc.image(&r.id)?;
    let bytes = entry.bytes()?;
    if !needs_bake(r, entry.format) {
        return Some(bytes.to_vec());
    }
    let mut img = image::load_from_memory(bytes).ok()?.to_rgba8();
    // Mirror first, then the clockwise quarter turns, as `ImageOrient` reads.
    if r.orient.mirrored {
        img = image::imageops::flip_horizontal(&img);
    }
    for _ in 0..r.orient.quarters() {
        img = image::imageops::rotate90(&img);
    }
    Some(crate::png::encode_rgba8(
        img.as_raw(),
        img.width(),
        img.height(),
    ))
}

//! The single color-conversion choke point (§6.3, invariant 7).
//!
//! The document stores colors as **sRGB, straight (non-premultiplied) alpha**
//! (`peniko::Color`). VERIFIED against the pinned vello/vello_cpu 0.9/0.2.0:
//! their paint APIs accept `peniko::Color` (sRGB straight) directly and handle
//! the compositing-space conversion internally. So the *input* conversion is the
//! identity — do NOT pre-convert to linear/premultiplied (that would double the
//! conversion and wash out every color; it was a mistaken assumption in the
//! original design notes).
//!
//! The one real conversion is on *output*: vello_cpu writes premultiplied RGBA8,
//! but PNG (and the snapshot's notion of color) use straight alpha, so we
//! un-premultiply here.

use crate::images::ImageStore;
use ondin_core::kurbo;
use ondin_core::peniko::{self, Brush, Color};

/// Identity: the backend consumes document sRGB straight-alpha color directly.
/// Exists as the named, single place the input color path passes through.
#[inline]
pub fn to_backend(color: Color) -> Color {
    color
}

/// The same choke point for a whole brush — and the one place the **model's**
/// brush becomes a **backend's**.
///
/// Solid colours and gradient stops are all sRGB straight alpha, which is what
/// both backends accept, so those two are still the identity in everything but
/// type: this is the place a future backend needing a different representation
/// would convert, and the one place to look when colours come out wrong.
///
/// **The types genuinely differ now, which is what images cost.** A model brush
/// holds an [`ondin_core::ImageRef`] — a key into the document's image table —
/// where a backend brush holds decoded pixels, so an image is the one variant
/// that is not converted by cloning: it is *resolved*, through the decode cache
/// (`images::ImageStore`) that turns the key into a `peniko::ImageData`.
///
/// `None` is "there is nothing to paint from this brush", and the caller draws
/// nothing. An image answers it whenever the store cannot produce pixels — a
/// dangling reference, a link whose file has moved or has not been read yet,
/// bytes that would not decode. All of those are states a document is allowed to
/// be in (§5.5a), and none of them is an error here.
///
/// **This is the GPU-shaped brush.** vello takes `peniko::ImageData` inline, so
/// the resolution ends here; `vello_cpu` takes an `ImageSource` of its own and
/// converts one step further, which `cpu.rs` does with a memo because that step
/// premultiplies a whole buffer.
///
/// **The second half of the answer is a transform, and ignoring it is a bug that
/// only shows while a slider moves.** The store may answer an adjustment with a
/// reduced picture (`images::Pixels`), whose pixel grid is not the source's — and
/// `ImageRef::framing` maps the *source's*. [`framed_transform`] is where the two
/// are composed; every caller owes it.
///
/// ⚠️ **It means a different thing per variant, and a gradient has one now**
/// (§15 D412). For an image it is `to_source`, which the caller composes under
/// the framing; for a gradient it is the brush's *whole* transform, ready to use;
/// for a solid it is the identity and means nothing. **Two meanings in one slot
/// is the cost of one function answering for three brushes** — stated here rather
/// than split, because the alternative is three call sites each remembering which
/// of two compositions their variant wants.
/// Which resolution a backend wants a picture at.
///
/// 🚨 **The one thing the two backends are *meant* to disagree about**
/// (§15 D745, `[S9.2-L1-01]`). Everything else in this module exists so they
/// cannot: one conversion, one monotonic-stops repair, one framing composition.
/// This is the exception and it is a deliberate one — vello's GPU atlas is a
/// single 8192-pixel sheet and silently drops whatever will not fit, so the canvas
/// has to shrink the page's pictures to make them all appear, while
/// `ondin export --png` has no atlas, no such limit, and must write the picture
/// the user actually placed.
///
/// **So a canvas that softens a photograph and an export that does not are both
/// correct**, and a reader finding them different should reach for this rather
/// than for a bug.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Resolution {
    /// The picture as the document holds it. Every non-canvas caller.
    Source,
    /// Shrunk to the page's share of the GPU atlas — [`ImageStore::atlas_pixels`].
    Atlas,
}

#[inline]
pub fn brush_to_backend(
    brush: &ondin_core::Brush,
    images: &ImageStore,
    at: Resolution,
) -> Option<(Brush, kurbo::Affine)> {
    match brush {
        ondin_core::Brush::Solid(c) => Some((Brush::Solid(*c), kurbo::Affine::IDENTITY)),
        // **A gradient's second half is its own transform**, which is the identity
        // for everything the app authors and a squash for an imported ellipse
        // (§15 D412). Answering `IDENTITY` here unconditionally — which this did
        // until 2026-09-03 — draws every elliptical gradient round.
        //
        // ⚠️ **And its stop offsets are made monotonic on the way through**
        // (§15 D455). `[S11.1-L1-02]`: a descending ramp does not merely differ,
        // it **disappears** — measured in release, a 100×20 rect came back the
        // first stop's colour in all 100 columns, and a descent of one part in
        // ten million was enough. Neither backend normalises, so this is the last
        // place it can be done once for both, which is the argument this module's
        // own doc already makes for being the single conversion site. Free: the
        // clone was happening anyway.
        ondin_core::Brush::Gradient(g) => {
            let mut ramp = g.gradient.clone();
            ondin_core::image::make_stops_monotonic(&mut ramp.stops);
            // **The ramp's own opacity, multiplied through here** (§15 D767).
            // `peniko::Gradient` has no alpha field to hand down — `multiply_alpha`
            // scales every stop — so this is where the field becomes pixels, for
            // both backends at once, on the clone that was already being made.
            // Storing the multiplier and applying it at paint time is what makes it
            // reversible, where the panel's old scale-the-stops spelling was not.
            let ramp = ramp.multiply_alpha(g.opacity);
            Some((Brush::Gradient(ramp), g.transform))
        }
        ondin_core::Brush::Image(img) => match at {
            Resolution::Source => images.pixels(&img.image),
            Resolution::Atlas => images.atlas_pixels(&img.image),
        }
        .map(|px| {
            (
                Brush::Image(peniko::ImageBrush {
                    // An `Arc` clone of the resolved blob, not a copy of the pixels.
                    image: px.data,
                    sampler: img.sampler,
                }),
                px.to_source,
            )
        }),
    }
}

/// The brush transform an image draws under: the framing, with the correction
/// that says which grid the pixels are actually on.
///
/// **One function, because both backends set a brush transform and neither may
/// forget the second half.** `to_source` is the identity for every brush that is
/// not a reduced picture, so this is the framing itself in every case but the one
/// it exists for.
#[inline]
pub fn framed_transform(
    framing: Option<ondin_core::Framing>,
    to_source: kurbo::Affine,
) -> Option<kurbo::Affine> {
    framing.map(|f| f.transform * to_source)
}

/// Put `framing`'s extend in force on a resolved image brush.
///
/// **The fit owns the extend, and the model's sampler is not written back.** A
/// tile repeats and every other fit pads, which is a fact about the fit rather
/// than a second setting beside it — so it is applied here, at the boundary,
/// where a stored copy in the document could not drift out of agreement with the
/// mode it belongs to. Everything else on the sampler (quality, alpha) is the
/// model's and is untouched.
///
/// A no-op for any brush that is not an image, and for an image with no framing
/// — which is what the document having no entry for it means (`scene::framing_of`).
#[inline]
pub fn apply_framing(brush: &mut Brush, framing: Option<ondin_core::Framing>) {
    if let Brush::Image(img) = brush {
        img.sampler = framed_sampler(img.sampler, framing);
    }
}

/// [`apply_framing`] for a bare sampler — what the CPU backend needs, since it
/// builds its paint around `vello_cpu`'s own image handle rather than around a
/// `peniko::Brush`. One function so the rule above cannot come to mean two
/// different things on the two backends.
#[inline]
pub fn framed_sampler(
    sampler: peniko::ImageSampler,
    framing: Option<ondin_core::Framing>,
) -> peniko::ImageSampler {
    match framing {
        Some(f) => peniko::ImageSampler {
            x_extend: f.extend,
            y_extend: f.extend,
            ..sampler
        },
        None => sampler,
    }
}

/// Convert a premultiplied-RGBA8 buffer (vello_cpu output) to straight-alpha
/// RGBA8 (PNG's format).
pub fn straight_rgba8_from_premul(premul: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; premul.len()];
    for (dst, px) in out.chunks_exact_mut(4).zip(premul.chunks_exact(4)) {
        let a = px[3];
        if a == 0 {
            // Fully transparent: leave color zeroed.
            dst[3] = 0;
        } else {
            for i in 0..3 {
                // Round-half-up division of (channel * 255) by alpha.
                let v = (px[i] as u32 * 255 + a as u32 / 2) / a as u32;
                dst[i] = v.min(255) as u8;
            }
            dst[3] = a;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The image arm of the brush conversion, which is the GPU path's alone.**
    ///
    /// Worth its own test because the CPU backend does not use it: `cpu.rs` takes
    /// the image branch off the *model's* brush before reaching here, so that it
    /// can memoize the premultiply. That means the pixel tests in
    /// `ondin-export/tests/png.rs` — which run on the CPU backend — cover the
    /// resolution end to end and cover *this function's* image arm not at all.
    /// Turning it off left all of them green, which is how the gap was found.
    #[test]
    fn an_image_brush_resolves_through_the_store_and_a_missing_one_does_not() {
        use crate::images::ImageStore;
        use ondin_core::{ImageFormat, ImageId, image_brush};

        let mut png = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut png, 1, 1);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(&[10, 20, 30, 255]).unwrap();
        }
        let id = ImageId("sha256:one".into());
        let mut store = ImageStore::new();
        store.insert(&id, &png, ImageFormat::Png).unwrap();

        let (resolved, to_source) =
            brush_to_backend(&image_brush(id.clone()), &store, Resolution::Source)
                .expect("an image the store holds resolves");
        assert_eq!(
            to_source,
            kurbo::Affine::IDENTITY,
            "an unadjusted picture is on its own pixel grid"
        );
        let Brush::Image(img) = resolved else {
            panic!("expected an image brush, got {resolved:?}");
        };
        assert_eq!((img.image.width, img.image.height), (1, 1));
        assert_eq!(img.image.data.data(), &[10, 20, 30, 255]);

        assert!(
            brush_to_backend(
                &image_brush(ImageId("sha256:absent".into())),
                &store,
                Resolution::Source
            )
            .is_none(),
            "a reference the store cannot resolve must paint nothing, not black"
        );
        // **And the same is true at `Atlas`**, which is the arm the canvas takes:
        // an unresolvable reference must not become a black rectangle on one
        // backend and nothing on the other (§15 D745).
        assert!(
            brush_to_backend(
                &image_brush(ImageId("sha256:absent".into())),
                &store,
                Resolution::Atlas
            )
            .is_none(),
            "and the canvas's arm answers the same"
        );
    }

    /// **The fit's extend reaches the GPU brush, and nothing else on the sampler
    /// moves.**
    ///
    /// Its own test for the reason the image arm above has one: the CPU backend
    /// takes a different route to the same rule (`cpu.rs` builds `vello_cpu`'s
    /// paint around `framed_sampler` directly), so the pixel tests in
    /// `ondin-export/tests/png.rs` cover the tiling end to end and cover *this*
    /// function not at all.
    ///
    /// The model's sampler is given a deliberately odd quality and alpha, so a
    /// version that rebuilt the sampler from the fit instead of overriding one
    /// field of it fails here rather than losing the quality silently.
    #[test]
    fn a_tiles_extend_reaches_the_brush_and_leaves_the_rest_of_the_sampler_alone() {
        use ondin_core::{ImageFit, ImageId, ImageRef, kurbo::Rect};

        let mut brush = Brush::Image(peniko::ImageBrush {
            image: peniko::ImageData {
                data: peniko::Blob::new(std::sync::Arc::new(vec![0u8; 4])),
                format: peniko::ImageFormat::Rgba8,
                alpha_type: peniko::ImageAlphaType::Alpha,
                width: 1,
                height: 1,
            },
            sampler: peniko::ImageSampler {
                x_extend: peniko::Extend::Pad,
                y_extend: peniko::Extend::Pad,
                quality: peniko::ImageQuality::High,
                alpha: 0.5,
            },
        });
        let tiled = ImageRef {
            fit: ImageFit::Tile,
            tile_scale: 2.0,
            ..ImageRef::new(ImageId("x".into()))
        }
        .framing(Rect::new(0.0, 0.0, 10.0, 10.0), 1, 1);
        apply_framing(&mut brush, Some(tiled));

        let Brush::Image(img) = &brush else {
            panic!("still an image brush");
        };
        assert_eq!(img.sampler.x_extend, peniko::Extend::Repeat);
        assert_eq!(img.sampler.y_extend, peniko::Extend::Repeat);
        assert_eq!(
            img.sampler.quality,
            peniko::ImageQuality::High,
            "quality is the model's and the fit has no opinion about it"
        );
        assert_eq!(img.sampler.alpha, 0.5, "so is the alpha multiplier");

        // A solid is untouched, and so is an image with nothing to frame it.
        let mut solid = Brush::Solid(Color::from_rgba8(1, 2, 3, 255));
        apply_framing(&mut solid, Some(tiled));
        assert_eq!(solid, Brush::Solid(Color::from_rgba8(1, 2, 3, 255)));
        let before = brush.clone();
        apply_framing(&mut brush, None);
        assert_eq!(brush, before, "no framing changes nothing");
    }

    #[test]
    fn unpremultiply_recovers_opaque_and_half_alpha() {
        // Opaque red stays red.
        let opaque = straight_rgba8_from_premul(&[255, 0, 0, 255]);
        assert_eq!(opaque, vec![255, 0, 0, 255]);

        // Half-alpha red is stored premultiplied as (~128, 0, 0, 128);
        // un-premultiplying recovers full-intensity red.
        let half = straight_rgba8_from_premul(&[128, 0, 0, 128]);
        assert_eq!(half[3], 128);
        assert!(
            half[0] >= 254,
            "red should recover to ~255, got {}",
            half[0]
        );

        // Fully transparent stays zero.
        let clear = straight_rgba8_from_premul(&[0, 0, 0, 0]);
        assert_eq!(clear, vec![0, 0, 0, 0]);
    }
}

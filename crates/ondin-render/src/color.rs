//! The single color-conversion choke point (§6.3, invariant 7).
//!
//! The document stores colors as sRGB, straight alpha (`peniko::Color`), and
//! vello/vello_cpu accept that directly and convert internally. So the input
//! conversion is the identity; pre-converting to linear/premultiplied would double
//! the conversion and wash out every color.
//!
//! The one real conversion is on output: vello_cpu writes premultiplied RGBA8, but
//! PNG uses straight alpha, so we un-premultiply here.

use crate::images::ImageStore;
use ondin_core::kurbo;
use ondin_core::peniko::{self, Brush, Color};

/// Identity: the backend consumes document sRGB straight-alpha color directly.
/// Exists as the named, single place the input color path passes through.
#[inline]
pub fn to_backend(color: Color) -> Color {
    color
}

/// Which resolution a backend wants a picture at.
///
/// The one thing the two backends are meant to disagree about (§15 D745): vello's
/// GPU atlas is a single 8192-pixel sheet that silently drops what does not fit,
/// so the canvas shrinks pictures while `ondin export --png` writes the picture as
/// placed. A softer canvas and a sharp export are both correct.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Resolution {
    /// The picture as the document holds it. Every non-canvas caller.
    Source,
    /// Shrunk to the page's share of the GPU atlas — [`ImageStore::atlas_pixels`].
    Atlas,
}

/// The same choke point for a whole brush: the one place the model's brush becomes
/// a backend's.
///
/// Solid colours and gradient stops are the identity in everything but type. An
/// image is resolved through the decode cache (`images::ImageStore`) from the
/// model's [`ondin_core::ImageRef`] key to a `peniko::ImageData`. `None` means
/// nothing to paint (dangling reference, unread link, undecodable bytes), all
/// states a document may be in (§5.5a), not errors.
///
/// This is the GPU-shaped brush; `cpu.rs` converts one step further, memoized
/// because that step premultiplies a whole buffer.
///
/// The returned transform means a different thing per variant (§15 D412): for an
/// image, `to_source`, which the caller composes under the framing via
/// [`framed_transform`] (a reduced picture's pixel grid is not the source's); for a
/// gradient, the brush's whole transform; for a solid, the identity.
#[inline]
pub fn brush_to_backend(
    brush: &ondin_core::Brush,
    images: &ImageStore,
    at: Resolution,
) -> Option<(Brush, kurbo::Affine)> {
    match brush {
        ondin_core::Brush::Solid(c) => Some((Brush::Solid(*c), kurbo::Affine::IDENTITY)),
        // A gradient carries its own transform: identity for authored ones, a
        // squash for an imported ellipse (§15 D412). Stop offsets are made
        // monotonic here (§15 D455) because neither backend normalises and a
        // descending ramp disappears into its first stop's colour.
        ondin_core::Brush::Gradient(g) => {
            let mut ramp = g.gradient.clone();
            ondin_core::image::make_stops_monotonic(&mut ramp.stops);
            // The ramp's opacity is multiplied into the stops here (§15 D767), since
            // `peniko::Gradient` has no alpha field; stored as a multiplier, it
            // stays reversible.
            let mut ramp = ramp.multiply_alpha(g.opacity);
            let transform = match &mut ramp.kind {
                peniko::GradientKind::Sweep(pos) => sweep_from_zero(pos, g.transform),
                _ => g.transform,
            };
            Some((Brush::Gradient(ramp), transform))
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

/// `pos` turned to start at angle 0, and the brush transform that turns it back.
///
/// Both backends measure a sweep's angle in `0..2π` and never wrap it, so a sweep
/// starting anywhere else pads its first colour over the turn before the start
/// instead of going round. Rotating the start into the transform is what CSS's
/// `conic-gradient(from …)` means, which the code export writes.
fn sweep_from_zero(
    pos: &mut peniko::SweepGradientPosition,
    transform: kurbo::Affine,
) -> kurbo::Affine {
    let start = f64::from(pos.start_angle);
    if start == 0.0 || !start.is_finite() {
        return transform;
    }
    pos.end_angle -= pos.start_angle;
    pos.start_angle = 0.0;
    transform * kurbo::Affine::rotate_about(start, pos.center)
}

/// The brush transform an image draws under: the framing, with the correction
/// that says which grid the pixels are actually on.
///
/// One function so neither backend forgets `to_source`, the identity for every
/// brush but a reduced picture.
#[inline]
pub fn framed_transform(
    framing: Option<ondin_core::Framing>,
    to_source: kurbo::Affine,
) -> Option<kurbo::Affine> {
    framing.map(|f| f.transform * to_source)
}

/// Put `framing`'s extend in force on a resolved image brush.
///
/// The fit owns the extend (a tile repeats, every other fit pads) and is applied
/// here rather than stored, so it cannot drift from the mode. Quality and alpha on
/// the sampler are the model's and untouched.
///
/// A no-op for a non-image brush and for an image with no framing
/// (`scene::framing_of`).
#[inline]
pub fn apply_framing(brush: &mut Brush, framing: Option<ondin_core::Framing>) {
    if let Brush::Image(img) = brush {
        img.sampler = framed_sampler(img.sampler, framing);
    }
}

/// [`apply_framing`] for a bare sampler, which the CPU backend needs since it
/// builds its paint around `vello_cpu`'s own image handle.
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
    for (dst, px) in out
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(premul.as_chunks::<4>().0)
    {
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

    /// The image arm of the brush conversion resolves through the store and a
    /// missing one paints nothing. The arm is the GPU path's alone (`cpu.rs` reads
    /// the model's brush first), so the CPU pixel tests in
    /// `ondin-export/tests/png.rs` do not cover it.
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
        // Likewise at `Atlas`, the canvas's arm: not black on one backend and
        // nothing on the other (§15 D745).
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

    /// The fit's extend reaches the GPU brush and nothing else on the sampler
    /// moves. The CPU backend uses `framed_sampler` directly, so its pixel tests do
    /// not cover `apply_framing`. The sampler's odd quality and alpha catch a
    /// version that rebuilds it from the fit.
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

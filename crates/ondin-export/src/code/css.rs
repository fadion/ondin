//! CSS values for the model's numbers, colours and paints.
//!
//! A function here answers `None` when CSS has no exact form for its input; the
//! walk then sends the layer to the next tier.

use crate::svg::fmt;
use ondin_core::kurbo::{Affine, Size, Vec2};
use ondin_core::peniko::{Color, ColorStop, Extend, Gradient, GradientKind};
use ondin_core::{Brush, GradientBrush};

/// `12px`, `0` for zero.
pub(crate) fn px(v: f64) -> String {
    let s = fmt(v);
    if s == "0" { s } else { format!("{s}px") }
}

/// `#rrggbb`, or `#rrggbbaa` when not opaque, after multiplying in `alpha`.
pub(crate) fn color(c: Color, alpha: f32) -> String {
    let c = c.multiply_alpha(alpha.clamp(0.0, 1.0));
    let rgba = c.to_rgba8();
    if rgba.a == 255 {
        format!("#{:02x}{:02x}{:02x}", rgba.r, rgba.g, rgba.b)
    } else {
        format!("#{:02x}{:02x}{:02x}{:02x}", rgba.r, rgba.g, rgba.b, rgba.a)
    }
}

/// One `background` layer for a fill on a `size` box, or `None` when CSS cannot
/// draw it exactly. A solid is a one-colour gradient so it can stack.
pub(crate) fn background_layer(brush: &Brush, size: Size) -> Option<String> {
    match brush {
        Brush::Solid(c) => {
            let c = color(*c, 1.0);
            Some(format!("linear-gradient({c}, {c})"))
        }
        Brush::Gradient(g) => gradient(g, size),
        Brush::Image(_) => None,
    }
}

/// A gradient brush as a CSS gradient on a `size` box, or `None`.
///
/// Reflect has no CSS form, and only sRGB interpolation is CSS's default for
/// these colours; anything else goes to SVG.
pub(crate) fn gradient(brush: &GradientBrush, size: Size) -> Option<String> {
    let g = &brush.gradient;
    if g.extend == Extend::Reflect || g.interpolation_cs != Gradient::default().interpolation_cs {
        return None;
    }
    let mut stops: Vec<ColorStop> = g.stops.iter().copied().collect();
    if stops.is_empty() {
        return None;
    }
    ondin_core::image::make_stops_monotonic(&mut stops);
    let repeat = g.extend == Extend::Repeat;
    if repeat {
        // A repeating CSS gradient repeats from its first stop to its last, where
        // the model repeats the whole 0..1 ramp: pin both ends.
        let first = stops[0];
        let last = stops[stops.len() - 1];
        if first.offset > 0.0 {
            stops.insert(
                0,
                ColorStop {
                    offset: 0.0,
                    ..first
                },
            );
        }
        if last.offset < 1.0 {
            stops.push(ColorStop {
                offset: 1.0,
                ..last
            });
        }
    }
    let prefix = if repeat { "repeating-" } else { "" };
    let ramp = |at: &dyn Fn(f64) -> String| -> String {
        stops
            .iter()
            .map(|s| {
                let c = s.color.to_alpha_color::<ondin_core::peniko::color::Srgb>();
                format!("{} {}", color(c, brush.opacity), at(f64::from(s.offset)))
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let inverse = brush.transform.inverse();
    match &g.kind {
        GradientKind::Linear(p) => {
            // The ramp parameter is affine in shape space, `t = k·x + b`; CSS's is
            // the projection on a line through the box centre at the same angle.
            let d = p.end - p.start;
            let len2 = d.hypot2();
            if len2 <= f64::EPSILON || !len2.is_finite() {
                return None;
            }
            let [a, b, c, dd, e, f] = inverse.as_coeffs();
            // `inverse` maps shape space to gradient space: x' = M x + m.
            let k = Vec2::new(a * d.x + b * d.y, c * d.x + dd * d.y) / len2;
            let m = Vec2::new(e, f);
            let base = (m - p.start.to_vec2()).dot(d) / len2;
            let norm = k.hypot();
            if norm <= f64::EPSILON || !norm.is_finite() {
                return None;
            }
            let u = k / norm;
            // CSS: 0deg points up, angles clockwise; up is -y.
            let angle = u.x.atan2(-u.y).to_degrees();
            let l = (size.width * u.x).abs() + (size.height * u.y).abs();
            if l <= f64::EPSILON {
                return None;
            }
            let centre = Vec2::new(size.width * 0.5, size.height * 0.5);
            let start = centre - u * (l * 0.5);
            let s_dot = start.dot(u);
            let at = |o: f64| pct(((o - base) / norm - s_dot) / l);
            Some(format!(
                "{prefix}linear-gradient({}deg, {})",
                fmt(angle),
                ramp(&at)
            ))
        }
        GradientKind::Radial(p) => {
            // Concentric, growing, under an axis-aligned transform: an ellipse.
            let (r0, r1) = (f64::from(p.start_radius), f64::from(p.end_radius));
            if p.start_center != p.end_center || r1 <= r0 || r1 <= 0.0 {
                return None;
            }
            let [a, b, c, d, _, _] = brush.transform.as_coeffs();
            if b.abs() > 1e-9 || c.abs() > 1e-9 || a <= 0.0 || d <= 0.0 {
                return None;
            }
            let centre = brush.transform * p.end_center;
            let at = |o: f64| pct((r0 + o * (r1 - r0)) / r1);
            Some(format!(
                "{prefix}radial-gradient({} {} at {} {}, {})",
                px(r1 * a),
                px(r1 * d),
                px(centre.x),
                px(centre.y),
                ramp(&at)
            ))
        }
        GradientKind::Sweep(p) => {
            if repeat {
                return None;
            }
            // A conic gradient survives a rotation and a uniform scale, not a
            // squash or a flip.
            let [a, b, c, d, _, _] = brush.transform.as_coeffs();
            let scale = (a * a + b * b).sqrt();
            if (a - d).abs() > 1e-9 || (b + c).abs() > 1e-9 || scale <= 0.0 {
                return None;
            }
            let turn = b.atan2(a);
            let centre = brush.transform * p.center;
            let (s, e) = (f64::from(p.start_angle), f64::from(p.end_angle));
            // Model angles run clockwise on screen from +x; CSS's from up. The
            // start turns `from` and the stops count from it, as the renderer
            // rotates the start into the transform (`sweep_from_zero`).
            let at = |o: f64| format!("{}deg", fmt((o * (e - s)).to_degrees()));
            Some(format!(
                "conic-gradient(from {}deg at {} {}, {})",
                fmt((turn + s).to_degrees() + 90.0),
                px(centre.x),
                px(centre.y),
                ramp(&at)
            ))
        }
    }
}

/// A fraction as a percentage.
fn pct(f: f64) -> String {
    format!("{}%", fmt(f * 100.0))
}

/// `matrix(a, b, c, d, e, f)`, or `None` for the identity.
pub(crate) fn matrix(t: Affine) -> Option<String> {
    if t == Affine::IDENTITY {
        return None;
    }
    let [a, b, c, d, e, f] = t.as_coeffs();
    Some(format!(
        "matrix({}, {}, {}, {}, {}, {})",
        fmt(a),
        fmt(b),
        fmt(c),
        fmt(d),
        fmt(e),
        fmt(f)
    ))
}

/// Whether `t` only moves.
pub(crate) fn is_translation(t: Affine) -> bool {
    let [a, b, c, d, _, _] = t.as_coeffs();
    a == 1.0 && b == 0.0 && c == 0.0 && d == 1.0
}

/// `t`'s linear part, its translation dropped.
pub(crate) fn linear(t: Affine) -> Affine {
    let [a, b, c, d, _, _] = t.as_coeffs();
    Affine::new([a, b, c, d, 0.0, 0.0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::kurbo::Point;
    use ondin_core::peniko::{
        LinearGradientPosition, RadialGradientPosition, SweepGradientPosition,
    };

    fn two_stop(kind: GradientKind) -> GradientBrush {
        GradientBrush {
            gradient: Gradient {
                kind,
                stops: [
                    ColorStop {
                        offset: 0.0,
                        color: Color::BLACK.into(),
                    },
                    ColorStop {
                        offset: 1.0,
                        color: Color::WHITE.into(),
                    },
                ]
                .as_slice()
                .into(),
                ..Default::default()
            },
            transform: Affine::IDENTITY,
            opacity: 1.0,
        }
    }

    /// A left-to-right ramp across a box is CSS's `90deg` at 0% and 100%.
    #[test]
    fn a_horizontal_ramp_spanning_the_box_is_ninety_degrees() {
        let g = two_stop(GradientKind::Linear(LinearGradientPosition {
            start: Point::new(0.0, 0.0),
            end: Point::new(200.0, 0.0),
        }));
        assert_eq!(
            gradient(&g, Size::new(200.0, 100.0)).unwrap(),
            "linear-gradient(90deg, #000000 0%, #ffffff 100%)"
        );
    }

    /// A ramp over the middle half of the box lands at 25% and 75%, and a vertical
    /// one points down at 180deg.
    #[test]
    fn a_partial_vertical_ramp_maps_its_stops_onto_the_css_line() {
        let g = two_stop(GradientKind::Linear(LinearGradientPosition {
            start: Point::new(10.0, 25.0),
            end: Point::new(10.0, 75.0),
        }));
        assert_eq!(
            gradient(&g, Size::new(40.0, 100.0)).unwrap(),
            "linear-gradient(180deg, #000000 25%, #ffffff 75%)"
        );
    }

    /// A focal radial has no CSS form; a concentric one does.
    #[test]
    fn only_a_concentric_radial_is_css() {
        let focal = two_stop(GradientKind::Radial(RadialGradientPosition {
            start_center: Point::new(10.0, 10.0),
            start_radius: 0.0,
            end_center: Point::new(20.0, 20.0),
            end_radius: 30.0,
        }));
        assert!(gradient(&focal, Size::new(40.0, 40.0)).is_none());
        let plain = two_stop(GradientKind::Radial(RadialGradientPosition {
            start_center: Point::new(20.0, 20.0),
            start_radius: 0.0,
            end_center: Point::new(20.0, 20.0),
            end_radius: 30.0,
        }));
        assert_eq!(
            gradient(&plain, Size::new(40.0, 40.0)).unwrap(),
            "radial-gradient(30px 30px at 20px 20px, #000000 0%, #ffffff 100%)"
        );
    }

    /// A sweep's start angle turns `from`, and its stops count from there, as
    /// the renderer rotates the start into the transform.
    #[test]
    fn a_sweeps_start_angle_is_folded_into_from() {
        let start = std::f32::consts::FRAC_PI_2;
        let g = two_stop(GradientKind::Sweep(SweepGradientPosition {
            center: Point::new(20.0, 20.0),
            start_angle: start,
            end_angle: start + std::f32::consts::TAU,
        }));
        assert_eq!(
            gradient(&g, Size::new(40.0, 40.0)).unwrap(),
            "conic-gradient(from 180deg at 20px 20px, #000000 0deg, #ffffff 360deg)"
        );
    }
}

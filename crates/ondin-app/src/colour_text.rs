//! A colour typed or pasted into a text field: hex, and CSS Color 4's sRGB forms
//! (`rgb()`, `rgba()`, `hsl()`, `hsla()` in either syntax, and the named colours).
//!
//! One parser behind every colour field, so the picker, the inspector rows and the
//! variable cells never disagree about what reads as a colour.

use ondin_core::peniko::Color;

/// What a field read: the colour, and its alpha only when the text spelled one —
/// a field without a typed alpha leaves each paint its own opacity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Typed {
    /// sRGB channels, 0..1.
    pub rgb: [f32; 3],
    pub alpha: Option<f32>,
}

impl Typed {
    /// The colour at the typed alpha, or at `alpha` when none was typed.
    pub fn color(self, alpha: f32) -> Color {
        let [r, g, b] = self.rgb;
        Color::new([r, g, b, self.alpha.unwrap_or(alpha)])
    }
}

/// Read `s` as a colour, or `None` when it is not one.
///
/// Hex is `RGB`, `RGBA`, `RRGGBB` or `RRGGBBAA`, the `#` optional. Case and
/// surrounding whitespace never matter; channels outside their range are clamped,
/// as CSS does.
pub(crate) fn parse(s: &str) -> Option<Typed> {
    let s = s.trim().to_ascii_lowercase();
    if let Some(args) = call(&s, &["rgba", "rgb"]) {
        let (c, alpha) = channels(args)?;
        let rgb = [rgb_channel(c[0])?, rgb_channel(c[1])?, rgb_channel(c[2])?];
        return Some(Typed { rgb, alpha });
    }
    if let Some(args) = call(&s, &["hsla", "hsl"]) {
        let (c, alpha) = channels(args)?;
        let rgb = hsl_to_rgb(hue(c[0])?, percent(c[1])?, percent(c[2])?);
        return Some(Typed { rgb, alpha });
    }
    if s == "transparent" {
        return Some(Typed {
            rgb: [0.0; 3],
            alpha: Some(0.0),
        });
    }
    if let Some(&(_, hex)) = NAMED.iter().find(|(name, _)| *name == s) {
        let channel = |shift: u32| unit(((hex >> shift) & 0xFF) as u8);
        return Some(Typed {
            rgb: [channel(16), channel(8), channel(0)],
            alpha: None,
        });
    }
    hex(&s)
}

/// Whether `text` names `current` at 8 bits a channel, alpha included only when
/// typed — compared as bytes, so committing an untouched field over an
/// off-lattice colour writes nothing.
pub(crate) fn matches(text: &str, current: Color) -> bool {
    let Some(t) = parse(text) else {
        return false;
    };
    let k = current.components;
    t.rgb.iter().zip(k).all(|(a, b)| byte(*a) == byte(b))
        && t.alpha.is_none_or(|a| byte(a) == byte(k[3]))
}

/// The status message for committed field text that is not a colour, or `None`
/// when there is nothing to say: it parses, it is empty, it is a field's own
/// *Mixed*, or it is a `--variable` name, which the binding answers itself.
pub(crate) fn refusal(text: &str) -> Option<String> {
    let t = text.trim();
    let quiet = t.is_empty() || t.starts_with('-') || t.eq_ignore_ascii_case("mixed");
    (!quiet && parse(t).is_none()).then(|| format!("Not a colour: {t}"))
}

fn byte(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// A byte as a 0..1 channel, by `Color::from_rgba8`'s own arithmetic, so a hex
/// stores exactly the float it stored before this parser read it.
fn unit(b: u8) -> f32 {
    Color::from_rgba8(b, 0, 0, 255).components[0]
}

/// The text between `name(` and a closing `)` that ends `s`.
fn call<'a>(s: &'a str, names: &[&str]) -> Option<&'a str> {
    let rest = names.iter().find_map(|n| s.strip_prefix(n))?;
    rest.trim_start().strip_prefix('(')?.strip_suffix(')')
}

/// Three channels and an optional alpha, in the comma syntax (`1, 2, 3, .5`) or
/// the space syntax (`1 2 3 / .5`).
fn channels(args: &str) -> Option<([&str; 3], Option<f32>)> {
    let (main, alpha): (Vec<&str>, Option<&str>) = if args.contains(',') {
        let parts: Vec<&str> = args.split(',').map(str::trim).collect();
        match parts.len() {
            3 => (parts, None),
            4 => (parts[..3].to_vec(), Some(parts[3])),
            _ => return None,
        }
    } else {
        let (main, alpha) = match args.split_once('/') {
            Some((m, a)) => (m, Some(a.trim())),
            None => (args, None),
        };
        (main.split_whitespace().collect(), alpha)
    };
    let [a, b, c] = main[..] else {
        return None;
    };
    let alpha = match alpha {
        Some(a) => Some(alpha_value(a)?),
        None => None,
    };
    Some(([a, b, c], alpha))
}

/// A number, or a percentage as a fraction of `full`.
fn number_or_percent(s: &str, full: f32) -> Option<f32> {
    let v = match s.strip_suffix('%') {
        Some(p) => p.trim().parse::<f32>().ok()? / 100.0 * full,
        None => s.parse::<f32>().ok()?,
    };
    v.is_finite().then_some(v)
}

fn rgb_channel(s: &str) -> Option<f32> {
    let v = number_or_percent(s, 255.0)?.clamp(0.0, 255.0);
    // A whole number is a byte, stored as a hex of it would be.
    Some(match v.fract() == 0.0 {
        true => unit(v as u8),
        false => v / 255.0,
    })
}

fn alpha_value(s: &str) -> Option<f32> {
    Some(number_or_percent(s, 1.0)?.clamp(0.0, 1.0))
}

/// Saturation or lightness, 0..1: `59%`, or a bare `59` as CSS 4 allows.
fn percent(s: &str) -> Option<f32> {
    let v = s
        .strip_suffix('%')
        .unwrap_or(s)
        .trim()
        .parse::<f32>()
        .ok()?;
    v.is_finite().then(|| (v / 100.0).clamp(0.0, 1.0))
}

/// A hue in turns, 0..1, from degrees (bare or `deg`), `rad`, `grad` or `turn`.
fn hue(s: &str) -> Option<f32> {
    let (num, per_turn) = [
        ("deg", 360.0),
        ("grad", 400.0),
        ("rad", std::f32::consts::TAU),
        ("turn", 1.0),
    ]
    .iter()
    .find_map(|(unit, per)| s.strip_suffix(unit).map(|n| (n, *per)))
    .unwrap_or((s, 360.0));
    let v = num.trim().parse::<f32>().ok()?;
    v.is_finite().then(|| (v / per_turn).rem_euclid(1.0))
}

/// CSS's own `hsl()` conversion.
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [f32; 3] {
    let f = |n: f32| {
        let k = (n + h * 12.0) % 12.0;
        let a = s * l.min(1.0 - l);
        l - a * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0)
    };
    [f(0.0), f(8.0), f(4.0)]
}

fn hex(s: &str) -> Option<Typed> {
    let s = s.strip_prefix('#').unwrap_or(s).trim();
    if !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let digit = |i: usize, n: usize| u8::from_str_radix(&s[i..i + n], 16).ok();
    // `F` means `FF`, not `F0`: 0x11 per step, so `FFF` is white.
    let short = |i| digit(i, 1).map(|v| unit(v * 17));
    let long = |i| digit(i, 2).map(unit);
    let (rgb, alpha) = match s.len() {
        3 => ([short(0)?, short(1)?, short(2)?], None),
        4 => ([short(0)?, short(1)?, short(2)?], Some(short(3)?)),
        6 => ([long(0)?, long(2)?, long(4)?], None),
        8 => ([long(0)?, long(2)?, long(4)?], Some(long(6)?)),
        _ => return None,
    };
    Some(Typed { rgb, alpha })
}

/// The CSS named colours, `transparent` aside.
const NAMED: &[(&str, u32)] = &[
    ("aliceblue", 0xF0F8FF),
    ("antiquewhite", 0xFAEBD7),
    ("aqua", 0x00FFFF),
    ("aquamarine", 0x7FFFD4),
    ("azure", 0xF0FFFF),
    ("beige", 0xF5F5DC),
    ("bisque", 0xFFE4C4),
    ("black", 0x000000),
    ("blanchedalmond", 0xFFEBCD),
    ("blue", 0x0000FF),
    ("blueviolet", 0x8A2BE2),
    ("brown", 0xA52A2A),
    ("burlywood", 0xDEB887),
    ("cadetblue", 0x5F9EA0),
    ("chartreuse", 0x7FFF00),
    ("chocolate", 0xD2691E),
    ("coral", 0xFF7F50),
    ("cornflowerblue", 0x6495ED),
    ("cornsilk", 0xFFF8DC),
    ("crimson", 0xDC143C),
    ("cyan", 0x00FFFF),
    ("darkblue", 0x00008B),
    ("darkcyan", 0x008B8B),
    ("darkgoldenrod", 0xB8860B),
    ("darkgray", 0xA9A9A9),
    ("darkgreen", 0x006400),
    ("darkgrey", 0xA9A9A9),
    ("darkkhaki", 0xBDB76B),
    ("darkmagenta", 0x8B008B),
    ("darkolivegreen", 0x556B2F),
    ("darkorange", 0xFF8C00),
    ("darkorchid", 0x9932CC),
    ("darkred", 0x8B0000),
    ("darksalmon", 0xE9967A),
    ("darkseagreen", 0x8FBC8F),
    ("darkslateblue", 0x483D8B),
    ("darkslategray", 0x2F4F4F),
    ("darkslategrey", 0x2F4F4F),
    ("darkturquoise", 0x00CED1),
    ("darkviolet", 0x9400D3),
    ("deeppink", 0xFF1493),
    ("deepskyblue", 0x00BFFF),
    ("dimgray", 0x696969),
    ("dimgrey", 0x696969),
    ("dodgerblue", 0x1E90FF),
    ("firebrick", 0xB22222),
    ("floralwhite", 0xFFFAF0),
    ("forestgreen", 0x228B22),
    ("fuchsia", 0xFF00FF),
    ("gainsboro", 0xDCDCDC),
    ("ghostwhite", 0xF8F8FF),
    ("gold", 0xFFD700),
    ("goldenrod", 0xDAA520),
    ("gray", 0x808080),
    ("green", 0x008000),
    ("greenyellow", 0xADFF2F),
    ("grey", 0x808080),
    ("honeydew", 0xF0FFF0),
    ("hotpink", 0xFF69B4),
    ("indianred", 0xCD5C5C),
    ("indigo", 0x4B0082),
    ("ivory", 0xFFFFF0),
    ("khaki", 0xF0E68C),
    ("lavender", 0xE6E6FA),
    ("lavenderblush", 0xFFF0F5),
    ("lawngreen", 0x7CFC00),
    ("lemonchiffon", 0xFFFACD),
    ("lightblue", 0xADD8E6),
    ("lightcoral", 0xF08080),
    ("lightcyan", 0xE0FFFF),
    ("lightgoldenrodyellow", 0xFAFAD2),
    ("lightgray", 0xD3D3D3),
    ("lightgreen", 0x90EE90),
    ("lightgrey", 0xD3D3D3),
    ("lightpink", 0xFFB6C1),
    ("lightsalmon", 0xFFA07A),
    ("lightseagreen", 0x20B2AA),
    ("lightskyblue", 0x87CEFA),
    ("lightslategray", 0x778899),
    ("lightslategrey", 0x778899),
    ("lightsteelblue", 0xB0C4DE),
    ("lightyellow", 0xFFFFE0),
    ("lime", 0x00FF00),
    ("limegreen", 0x32CD32),
    ("linen", 0xFAF0E6),
    ("magenta", 0xFF00FF),
    ("maroon", 0x800000),
    ("mediumaquamarine", 0x66CDAA),
    ("mediumblue", 0x0000CD),
    ("mediumorchid", 0xBA55D3),
    ("mediumpurple", 0x9370DB),
    ("mediumseagreen", 0x3CB371),
    ("mediumslateblue", 0x7B68EE),
    ("mediumspringgreen", 0x00FA9A),
    ("mediumturquoise", 0x48D1CC),
    ("mediumvioletred", 0xC71585),
    ("midnightblue", 0x191970),
    ("mintcream", 0xF5FFFA),
    ("mistyrose", 0xFFE4E1),
    ("moccasin", 0xFFE4B5),
    ("navajowhite", 0xFFDEAD),
    ("navy", 0x000080),
    ("oldlace", 0xFDF5E6),
    ("olive", 0x808000),
    ("olivedrab", 0x6B8E23),
    ("orange", 0xFFA500),
    ("orangered", 0xFF4500),
    ("orchid", 0xDA70D6),
    ("palegoldenrod", 0xEEE8AA),
    ("palegreen", 0x98FB98),
    ("paleturquoise", 0xAFEEEE),
    ("palevioletred", 0xDB7093),
    ("papayawhip", 0xFFEFD5),
    ("peachpuff", 0xFFDAB9),
    ("peru", 0xCD853F),
    ("pink", 0xFFC0CB),
    ("plum", 0xDDA0DD),
    ("powderblue", 0xB0E0E6),
    ("purple", 0x800080),
    ("rebeccapurple", 0x663399),
    ("red", 0xFF0000),
    ("rosybrown", 0xBC8F8F),
    ("royalblue", 0x4169E1),
    ("saddlebrown", 0x8B4513),
    ("salmon", 0xFA8072),
    ("sandybrown", 0xF4A460),
    ("seagreen", 0x2E8B57),
    ("seashell", 0xFFF5EE),
    ("sienna", 0xA0522D),
    ("silver", 0xC0C0C0),
    ("skyblue", 0x87CEEB),
    ("slateblue", 0x6A5ACD),
    ("slategray", 0x708090),
    ("slategrey", 0x708090),
    ("snow", 0xFFFAFA),
    ("springgreen", 0x00FF7F),
    ("steelblue", 0x4682B4),
    ("tan", 0xD2B48C),
    ("teal", 0x008080),
    ("thistle", 0xD8BFD8),
    ("tomato", 0xFF6347),
    ("turquoise", 0x40E0D0),
    ("violet", 0xEE82EE),
    ("wheat", 0xF5DEB3),
    ("white", 0xFFFFFF),
    ("whitesmoke", 0xF5F5F5),
    ("yellow", 0xFFFF00),
    ("yellowgreen", 0x9ACD32),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(s: &str) -> Option<([u8; 3], Option<u8>)> {
        parse(s).map(|t| (t.rgb.map(byte), t.alpha.map(byte)))
    }

    const BLUE: [u8; 3] = [109, 140, 217];

    /// Every hex spelling a field accepted before still reads the same.
    #[test]
    fn hex_reads_as_it_did() {
        assert_eq!(bytes("6D8CD9"), Some((BLUE, None)));
        assert_eq!(bytes(" #6d8cd9 "), Some((BLUE, None)));
        assert_eq!(bytes("fff"), Some(([255; 3], None)));
        assert_eq!(bytes("#6D8CD980"), Some((BLUE, Some(128))));
        assert_eq!(bytes("6D8CD9FF"), Some((BLUE, Some(255))));
        for bad in ["nope", "12345", "6D#8CD9", "#", "", "6D8CD9G0"] {
            assert_eq!(parse(bad), None, "{bad:?}");
        }
    }

    /// Four hex digits are CSS's short form with alpha.
    #[test]
    fn four_digits_carry_an_alpha() {
        assert_eq!(bytes("#68DF"), Some(([0x66, 0x88, 0xDD], Some(255))));
        assert_eq!(bytes("0008"), Some(([0; 3], Some(0x88))));
    }

    /// Both `rgb()` syntaxes, percentages, and an alpha in either form.
    #[test]
    fn rgb_reads_in_both_syntaxes() {
        for s in [
            "rgb(109 140 217)",
            "rgb(109, 140, 217)",
            "RGB( 109 140 217 )",
            "rgba(109,140,217)",
        ] {
            assert_eq!(bytes(s), Some((BLUE, None)), "{s}");
        }
        assert_eq!(bytes("rgb(109 140 217 / 50%)"), Some((BLUE, Some(128))));
        assert_eq!(bytes("rgba(109, 140, 217, .5)"), Some((BLUE, Some(128))));
        assert_eq!(bytes("rgb(100% 0% 50%)"), Some(([255, 0, 128], None)));
        assert_eq!(
            bytes("rgb(300 -5 0)"),
            Some(([255, 0, 0], None)),
            "out-of-range channels clamp, as CSS does"
        );
        for bad in [
            "rgb(109 140)",
            "rgb(1, 2, 3, 4, 5)",
            "rgb(a b c)",
            "rgb(1 2 3",
        ] {
            assert_eq!(parse(bad), None, "{bad:?}");
        }
    }

    /// `hsl()` lands where CSS puts it, in every hue unit.
    ///
    /// flip: `hsl_to_rgb` with the green and blue offsets swapped → fails at the
    /// first assertion.
    #[test]
    fn hsl_reads_as_css_converts_it() {
        assert_eq!(bytes("hsl(223 59% 64%)"), Some(([109, 140, 217], None)));
        assert_eq!(bytes("hsl(0, 100%, 50%)"), Some(([255, 0, 0], None)));
        assert_eq!(bytes("hsl(120deg 100% 25%)"), Some(([0, 128, 0], None)));
        assert_eq!(bytes("hsl(.5turn 100 50)"), Some(([0, 255, 255], None)));
        assert_eq!(
            bytes("hsla(240, 100%, 50%, 0.25)"),
            Some(([0, 0, 255], Some(64)))
        );
        assert_eq!(
            bytes("hsl(-120 100% 50%)"),
            bytes("hsl(240 100% 50%)"),
            "a hue wraps"
        );
    }

    /// Named colours, any case; `transparent` is clear black.
    #[test]
    fn names_read_as_their_colours() {
        assert_eq!(bytes("rebeccapurple"), Some(([0x66, 0x33, 0x99], None)));
        assert_eq!(bytes("  CornflowerBlue "), Some(([0x64, 0x95, 0xED], None)));
        assert_eq!(bytes("transparent"), Some(([0; 3], Some(0))));
        assert_eq!(parse("notacolour"), None);
        assert!(
            NAMED.iter().all(|(n, _)| hex(n).is_none()),
            "no name is also a hex, or the table would shadow one"
        );
    }

    /// `matches` compares bytes, and alpha only when it was typed.
    #[test]
    fn matching_is_by_bytes() {
        let c = Color::from_rgba8(109, 140, 217, 128);
        assert!(matches("6D8CD9", c), "no alpha typed, so the colour alone");
        assert!(matches("rgb(109 140 217)", c));
        assert!(!matches("6D8CD9FF", c), "a typed alpha has to agree too");
        assert!(matches("6D8CD980", c));
        assert!(!matches("nope", c));
    }
}

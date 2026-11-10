//! A value axis's ticks and the way a number is written.

use super::NumberFormat;

/// A value axis: its domain and the ticks across it.
#[derive(Clone, Debug, PartialEq)]
pub struct Ticks {
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub values: Vec<f64>,
}

impl Ticks {
    /// Where `v` falls across the domain, 0 at `min` and 1 at `max`.
    pub fn t(&self, v: f64) -> f64 {
        if self.max > self.min {
            (v - self.min) / (self.max - self.min)
        } else {
            0.0
        }
    }

    /// The decimals the step needs, so 0.25 steps write two places.
    pub fn decimals(&self) -> u8 {
        decimals_of(self.step)
    }
}

/// A step of 1, 2 or 5 times a power of ten, the smallest at least `raw`.
pub fn nice_step(raw: f64) -> f64 {
    if !(raw.is_finite() && raw > 0.0) {
        return 1.0;
    }
    let p = 10f64.powf(raw.log10().floor());
    let f = raw / p;
    let n = if f <= 1.0 + 1e-9 {
        1.0
    } else if f <= 2.0 + 1e-9 {
        2.0
    } else if f <= 5.0 + 1e-9 {
        5.0
    } else {
        10.0
    };
    n * p
}

/// Ticks over `lo..=hi` aiming for `count` intervals on 1-2-5 steps, the domain
/// widened to whole steps. `typed` is a range the author set, kept as given with
/// ticks inside it.
pub fn ticks(lo: f64, hi: f64, count: u32, typed: Option<(f64, f64)>) -> Ticks {
    let count = f64::from(count.max(1));
    if let Some((min, max)) = typed {
        let step = nice_step((max - min) / count);
        let first = (min / step).ceil() * step;
        let values = walk(first, max, step);
        return Ticks {
            min,
            max,
            step,
            values,
        };
    }
    let (mut lo, mut hi) = (lo.min(hi), lo.max(hi));
    if !(lo.is_finite() && hi.is_finite()) {
        (lo, hi) = (0.0, 1.0);
    }
    if hi - lo < 1e-12 {
        // One value: a domain around it, or 0..1 for nothing at all.
        if hi.abs() < 1e-12 {
            (lo, hi) = (0.0, 1.0);
        } else if hi > 0.0 {
            lo = lo.min(0.0);
            hi = hi.max(lo + 1e-12);
        } else {
            hi = 0.0;
        }
    }
    let step = nice_step((hi - lo) / count);
    let min = (lo / step).floor() * step;
    let max = (hi / step).ceil() * step;
    let values = walk(min, max, step);
    Ticks {
        min,
        max,
        step,
        values,
    }
}

fn walk(from: f64, to: f64, step: f64) -> Vec<f64> {
    let mut out = Vec::new();
    let mut i = 0.0;
    loop {
        let v = from + i * step;
        if v > to + step * 1e-9 || out.len() > 200 {
            break;
        }
        // Snapped, so 0.1 × 3 writes 0.3 and a tick at zero is zero.
        let v = (v / step).round() * step;
        out.push(if v.abs() < step * 1e-9 { 0.0 } else { v });
        i += 1.0;
    }
    out
}

/// The decimals a step of `step` needs.
pub fn decimals_of(step: f64) -> u8 {
    let mut d = 0u8;
    let mut s = step.abs();
    while d < 10 && (s - s.round()).abs() > 1e-9 * s.max(1.0) {
        s *= 10.0;
        d += 1;
    }
    d
}

/// `v` written by `f`; `fallback` is the decimals used when `f` names none.
pub fn format(v: f64, f: &NumberFormat, fallback: u8) -> String {
    let mut v = v;
    let mut suffix = f.suffix.clone();
    if f.percent {
        v *= 100.0;
        suffix.insert(0, '%');
    }
    let mut unit = "";
    if f.compact {
        for (div, u) in [(1e12, "T"), (1e9, "B"), (1e6, "M"), (1e3, "k")] {
            if v.abs() >= div {
                v /= div;
                unit = u;
                break;
            }
        }
    }
    let decimals = f.decimals.unwrap_or(if unit.is_empty() {
        fallback
    } else {
        // A compact value keeps one place where it has one: 1.2k, 3M.
        u8::from((v - v.round()).abs() > 1e-9)
    });
    let mut body = format!("{:.*}", usize::from(decimals), v.abs());
    if f.thousands {
        body = grouped(&body);
    }
    let sign = if v < 0.0 && body.chars().any(|c| c.is_ascii_digit() && c != '0') {
        "-"
    } else {
        ""
    };
    format!("{sign}{}{body}{unit}{suffix}", f.prefix)
}

/// `1234567.5` as `1,234,567.5`.
fn grouped(digits: &str) -> String {
    let (int, frac) = match digits.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (digits, None),
    };
    let mut out = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if let Some(f) = frac {
        out.push('.');
        out.push_str(f);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ticks land on 1-2-5 steps and the domain widens to whole steps.
    #[test]
    fn ticks_aim_for_five_on_one_two_five_steps() {
        let t = ticks(0.0, 68.0, 5, None);
        assert_eq!((t.min, t.max, t.step), (0.0, 80.0, 20.0));
        assert_eq!(t.values, vec![0.0, 20.0, 40.0, 60.0, 80.0]);
        let t = ticks(-12.0, 45.0, 5, None);
        assert_eq!((t.min, t.max, t.step), (-20.0, 60.0, 20.0));
        let t = ticks(0.0, 0.7, 5, None);
        assert_eq!(t.step, 0.2);
        assert_eq!(t.values.last(), Some(&0.8));
        let t = ticks(0.0, 0.0, 5, None);
        assert_eq!(
            (t.min, t.max),
            (0.0, 1.0),
            "nothing to draw still has an axis"
        );
        let t = ticks(0.0, 50.0, 5, Some((0.0, 120.0)));
        assert_eq!((t.min, t.max), (0.0, 120.0), "a typed range is kept");
        assert_eq!(t.values, vec![0.0, 50.0, 100.0]);
    }

    /// Numbers written with separators, places, affixes, compact units and
    /// percent.
    #[test]
    fn a_number_is_written_as_its_format_says() {
        let plain = NumberFormat::default();
        assert_eq!(format(1234.5, &plain, 1), "1234.5");
        let f = NumberFormat {
            thousands: true,
            prefix: "$".into(),
            ..NumberFormat::default()
        };
        assert_eq!(format(-1234567.0, &f, 0), "-$1,234,567");
        let compact = NumberFormat {
            compact: true,
            ..NumberFormat::default()
        };
        assert_eq!(format(1200.0, &compact, 0), "1.2k");
        assert_eq!(format(3_000_000.0, &compact, 0), "3M");
        let pct = NumberFormat {
            percent: true,
            ..NumberFormat::default()
        };
        assert_eq!(format(0.25, &pct, 0), "25%");
        assert_eq!(format(-0.0001, &plain, 0), "0", "no minus on a zero");
        assert_eq!(decimals_of(0.25), 2);
        assert_eq!(decimals_of(20.0), 0);
    }
}

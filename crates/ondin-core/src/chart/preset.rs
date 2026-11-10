//! Style presets (`charts.md`, *Options*): a set of option values written over a
//! description in one step, not a link. The Chart card's Style cells and an
//! agent's `set_chart` write the same values.

use super::{LegendPosition, Options, PointStyle, ValuePosition};

/// The presets, in the card's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Preset {
    /// The defaults: solid gridlines, a legend on top, no values.
    Clean,
    /// Rounded marks and a light dashed grid.
    Soft,
    /// Heavy marks, no grid, large values.
    Bold,
    /// No axes, no grid, no legend: a sparkline when the chart is small.
    Minimal,
    /// Marks stroked in their colour over a light tint of it, and thin lines.
    Outline,
}

/// The stroke the *Outline* preset gives marks and lines, and the one the
/// card's *Outline marks* starts at.
pub const OUTLINE_WIDTH: f64 = 1.5;

impl Preset {
    pub const ALL: [Preset; 5] = [
        Preset::Clean,
        Preset::Soft,
        Preset::Bold,
        Preset::Minimal,
        Preset::Outline,
    ];

    /// The preset's name as the UI says it.
    pub fn label(self) -> &'static str {
        match self {
            Preset::Clean => "Clean",
            Preset::Soft => "Soft",
            Preset::Bold => "Bold",
            Preset::Minimal => "Minimal",
            Preset::Outline => "Outline",
        }
    }

    /// `options` with this preset's values written over the style fields it
    /// owns; data, palette, text family and number formats are left as they are.
    pub fn apply(self, options: &Options) -> Options {
        let clean = Options::default();
        let mut o = options.clone();
        o.bars.radius = clean.bars.radius;
        o.bars.radius_all = clean.bars.radius_all;
        o.bars.group_gap = clean.bars.group_gap;
        o.lines.width = clean.lines.width;
        o.lines.points = clean.lines.points;
        o.slices.pad_angle = clean.slices.pad_angle;
        o.slices.radius = clean.slices.radius;
        o.flows.node_radius = clean.flows.node_radius;
        for (axis, default) in [
            (&mut o.value_axis, &clean.value_axis),
            (&mut o.category_axis, &clean.category_axis),
        ] {
            axis.show = default.show;
            axis.grid.show = default.grid.show;
            axis.grid.dashed = default.grid.dashed;
        }
        o.legend.position = clean.legend.position;
        o.values.position = clean.values.position;
        o.text.value_size = clean.text.value_size;
        o.outline = clean.outline;
        match self {
            Preset::Clean => {}
            Preset::Soft => {
                o.bars.radius = 6.0;
                o.bars.radius_all = true;
                o.lines.points = PointStyle::Filled;
                o.slices.pad_angle = 2.0;
                o.slices.radius = 4.0;
                o.flows.node_radius = 6.0;
                o.value_axis.grid.dashed = true;
            }
            Preset::Bold => {
                o.bars.group_gap = 15.0;
                o.lines.width = 4.0;
                o.value_axis.grid.show = false;
                o.category_axis.grid.show = false;
                o.values.position = ValuePosition::Outside;
                o.text.value_size = 14.0;
            }
            Preset::Minimal => {
                o.value_axis.show = false;
                o.category_axis.show = false;
                o.value_axis.grid.show = false;
                o.category_axis.grid.show = false;
                o.legend.position = LegendPosition::Off;
            }
            Preset::Outline => {
                o.outline = Some(OUTLINE_WIDTH);
                o.lines.width = OUTLINE_WIDTH;
            }
        }
        o
    }

    /// The preset `options` is exactly at, for the cell that lights.
    pub fn of(options: &Options) -> Option<Preset> {
        Preset::ALL
            .into_iter()
            .find(|p| p.apply(options) == *options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each preset reads back as itself after it is applied, from any other, so
    /// one cell lights; the defaults are *Clean*.
    #[test]
    fn a_preset_applied_is_the_one_that_lights() {
        assert_eq!(Preset::of(&Options::default()), Some(Preset::Clean));
        for from in Preset::ALL {
            for to in Preset::ALL {
                let o = to.apply(&from.apply(&Options::default()));
                assert_eq!(Preset::of(&o), Some(to), "{from:?} → {to:?}");
            }
        }
        let mut custom = Options::default();
        custom.bars.radius = 9.0;
        assert_eq!(Preset::of(&custom), None, "an edited option lights none");
    }
}

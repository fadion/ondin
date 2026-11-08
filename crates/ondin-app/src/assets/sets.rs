//! The icon sets the assets browser offers, and how one is read once downloaded.
//!
//! Every set comes from its Iconify JSON package, pinned to one version and
//! fetched whole from jsDelivr, so a set searched once works offline and a given
//! build always inserts the same paths. A set's variants (weights, filled or
//! outlined) are icon-name suffixes inside a package, or a second package.

use std::collections::HashMap;

/// One npm package of Iconify JSON, at the version this build reads.
pub(crate) struct Package {
    pub(crate) name: &'static str,
    pub(crate) version: &'static str,
}

impl Package {
    /// Where the package's icons are fetched from.
    pub(crate) fn url(&self) -> String {
        format!(
            "https://cdn.jsdelivr.net/npm/@iconify-json/{}@{}/icons.json",
            self.name, self.version
        )
    }

    /// The cache file the package is kept in.
    pub(crate) fn file_name(&self) -> String {
        format!("{}@{}.json", self.name, self.version)
    }
}

/// One of a set's styles: the icons of `package` whose names end in `suffix`
/// (`""` for the ones ending in no other variant's suffix).
pub(crate) struct Variant {
    pub(crate) label: &'static str,
    pub(crate) package: usize,
    pub(crate) suffix: &'static str,
}

/// One icon set as Settings lists it and the browser offers it.
pub(crate) struct SetSpec {
    /// The key it is remembered by in the preferences; never changes.
    pub(crate) key: &'static str,
    pub(crate) name: &'static str,
    pub(crate) licence: &'static str,
    pub(crate) licence_url: &'static str,
    pub(crate) author: &'static str,
    pub(crate) home: &'static str,
    /// Whether the licence asks for credit on the design (CC BY).
    pub(crate) credit_required: bool,
    pub(crate) packages: &'static [Package],
    /// The first is the default.
    pub(crate) variants: &'static [Variant],
}

impl SetSpec {
    /// The set by its key.
    #[cfg(test)]
    pub(crate) fn by_key(key: &str) -> Option<&'static SetSpec> {
        SETS.iter().find(|s| s.key == key)
    }

    /// What Settings writes as the set's note: the licence, and for one that
    /// asks for credit, that each icon keeps it.
    pub(crate) fn note(&self) -> String {
        match self.credit_required {
            true => format!("{} · each icon keeps its credit", self.licence),
            false => self.licence.to_string(),
        }
    }
}

const fn v(label: &'static str, package: usize, suffix: &'static str) -> Variant {
    Variant {
        label,
        package,
        suffix,
    }
}

const fn p(name: &'static str, version: &'static str) -> Package {
    Package { name, version }
}

/// Every set, in the order Settings lists them.
pub(crate) const SETS: &[SetSpec] = &[
    SetSpec {
        key: "phosphor",
        name: "Phosphor",
        licence: "MIT",
        licence_url: "https://github.com/phosphor-icons/core/blob/main/LICENSE",
        author: "Phosphor Icons",
        home: "https://phosphoricons.com",
        credit_required: false,
        packages: &[p("ph", "1.2.2")],
        variants: &[
            v("Regular", 0, ""),
            v("Thin", 0, "-thin"),
            v("Light", 0, "-light"),
            v("Bold", 0, "-bold"),
            v("Fill", 0, "-fill"),
            v("Duotone", 0, "-duotone"),
        ],
    },
    SetSpec {
        key: "lucide",
        name: "Lucide",
        licence: "ISC",
        licence_url: "https://github.com/lucide-icons/lucide/blob/main/LICENSE",
        author: "Lucide Contributors",
        home: "https://lucide.dev",
        credit_required: false,
        packages: &[p("lucide", "1.2.152")],
        variants: &[v("Regular", 0, "")],
    },
    SetSpec {
        key: "tabler",
        name: "Tabler",
        licence: "MIT",
        licence_url: "https://github.com/tabler/tabler-icons/blob/main/LICENSE",
        author: "Paweł Kuna",
        home: "https://tabler.io/icons",
        credit_required: false,
        packages: &[p("tabler", "1.2.43")],
        variants: &[v("Outline", 0, ""), v("Filled", 0, "-filled")],
    },
    SetSpec {
        key: "material-symbols",
        name: "Material Symbols",
        licence: "Apache 2.0",
        licence_url: "https://github.com/google/material-design-icons/blob/master/LICENSE",
        author: "Google",
        home: "https://fonts.google.com/icons",
        credit_required: false,
        packages: &[p("material-symbols", "1.2.98")],
        variants: &[
            v("Filled", 0, ""),
            v("Outlined", 0, "-outline"),
            v("Rounded", 0, "-rounded"),
            v("Outlined rounded", 0, "-outline-rounded"),
            v("Sharp", 0, "-sharp"),
            v("Outlined sharp", 0, "-outline-sharp"),
        ],
    },
    SetSpec {
        key: "heroicons",
        name: "Heroicons",
        licence: "MIT",
        licence_url: "https://github.com/tailwindlabs/heroicons/blob/master/LICENSE",
        author: "Refactoring UI Inc",
        home: "https://heroicons.com",
        credit_required: false,
        packages: &[p("heroicons", "1.2.3")],
        variants: &[
            v("Outline", 0, ""),
            v("Solid", 0, "-solid"),
            v("Mini", 0, "-20-solid"),
            v("Micro", 0, "-16-solid"),
        ],
    },
    SetSpec {
        key: "font-awesome",
        name: "Font Awesome Free",
        licence: "CC BY 4.0",
        licence_url: "https://creativecommons.org/licenses/by/4.0/",
        author: "Fonticons, Inc.",
        home: "https://fontawesome.com",
        credit_required: true,
        packages: &[p("fa6-solid", "1.2.4"), p("fa6-regular", "1.2.4")],
        variants: &[v("Solid", 0, ""), v("Regular", 1, "")],
    },
];

/// One icon of a loaded set.
#[derive(Clone, Debug)]
pub(crate) struct Icon {
    /// Its name without the variant's suffix, which is what is searched and shown.
    pub(crate) name: String,
    /// Which of the set's variants it is.
    pub(crate) variant: usize,
    /// The SVG markup inside its `<svg>`, as Iconify gives it.
    pub(crate) body: String,
    /// Its `viewBox`.
    pub(crate) view: [f64; 4],
    /// The other names it answers to, Iconify's aliases.
    pub(crate) aliases: Vec<String>,
}

/// A set, read: its icons in name order.
#[derive(Debug)]
pub(crate) struct IconSet {
    pub(crate) icons: Vec<Icon>,
    /// The grid its icons are drawn on, the size field's default.
    pub(crate) grid: f64,
}

#[derive(serde::Deserialize)]
struct RawIcon {
    body: String,
    width: Option<f64>,
    height: Option<f64>,
    left: Option<f64>,
    top: Option<f64>,
    #[serde(default)]
    hidden: bool,
}

#[derive(serde::Deserialize)]
struct RawAlias {
    parent: String,
}

#[derive(serde::Deserialize)]
struct RawPackage {
    icons: HashMap<String, RawIcon>,
    #[serde(default)]
    aliases: HashMap<String, RawAlias>,
    width: Option<f64>,
    height: Option<f64>,
}

/// The variant of `spec` an icon named `name` in package `package` belongs to,
/// and its name without the suffix: the longest suffix that fits wins, so
/// `-outline-rounded` is not read as `-rounded`.
fn variant_of(spec: &SetSpec, package: usize, name: &str) -> Option<(usize, String)> {
    let mut best: Option<(usize, &str)> = None;
    for (i, var) in spec.variants.iter().enumerate() {
        if var.package != package {
            continue;
        }
        let fits = name.len() > var.suffix.len() && name.ends_with(var.suffix);
        if fits && best.is_none_or(|(_, s)| var.suffix.len() > s.len()) {
            best = Some((i, var.suffix));
        }
    }
    let (i, suffix) = best?;
    Some((i, name[..name.len() - suffix.len()].to_string()))
}

/// Read `spec`'s packages, `jsons[i]` being `spec.packages[i]`'s bytes. Hidden
/// (deprecated) icons are left out; an alias becomes a search word of its parent.
pub(crate) fn read_set(spec: &'static SetSpec, jsons: &[Vec<u8>]) -> Result<IconSet, String> {
    let mut icons = Vec::new();
    let mut grid = None;
    for (package, bytes) in jsons.iter().enumerate() {
        let raw: RawPackage = serde_json::from_slice(bytes)
            .map_err(|e| format!("{} could not be read: {e}", spec.name))?;
        let (dw, dh) = (raw.width.unwrap_or(16.0), raw.height.unwrap_or(16.0));
        grid.get_or_insert(dh);
        let mut aliases: HashMap<&str, Vec<String>> = HashMap::new();
        for (alias, a) in &raw.aliases {
            aliases
                .entry(a.parent.as_str())
                .or_default()
                .push(alias.clone());
        }
        for (name, icon) in &raw.icons {
            if icon.hidden {
                continue;
            }
            let Some((variant, base)) = variant_of(spec, package, name) else {
                continue;
            };
            let mut alias_words: Vec<String> = aliases
                .remove(name.as_str())
                .unwrap_or_default()
                .into_iter()
                .filter_map(|a| variant_of(spec, package, &a).map(|(_, b)| b))
                .filter(|b| *b != base)
                .collect();
            alias_words.sort();
            alias_words.dedup();
            icons.push(Icon {
                name: base,
                variant,
                body: icon.body.clone(),
                view: [
                    icon.left.unwrap_or(0.0),
                    icon.top.unwrap_or(0.0),
                    icon.width.unwrap_or(dw),
                    icon.height.unwrap_or(dh),
                ],
                aliases: alias_words,
            });
        }
    }
    icons.sort_by(|a, b| a.name.cmp(&b.name).then(a.variant.cmp(&b.variant)));
    Ok(IconSet {
        icons,
        // Iconify's own sets declare 24 or 32 here; Phosphor's 256 is its path
        // units, drawn at 24.
        grid: grid.map_or(24.0, |g| if g > 64.0 { 24.0 } else { g }),
    })
}

impl Icon {
    /// The icon as standalone SVG markup at `size` points square-ish (its aspect
    /// kept), `currentColor` reading as `ink`.
    pub(crate) fn svg(&self, size: f64, ink: &str) -> String {
        let [x, y, w, h] = self.view;
        let scale = size / w.max(h).max(f64::MIN_POSITIVE);
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}" viewBox="{x} {y} {w} {h}" color="{ink}">{}</svg>"#,
            w * scale,
            h * scale,
            self.body
        )
    }
}

/// How well `icon` answers `words` (every word must match): 0 is best. A name
/// starting with the query beats one with a word starting with it, which beats
/// one merely containing it; an alias match ranks after the name's.
pub(crate) fn rank(icon: &Icon, query: &str, words: &[&str]) -> Option<u8> {
    let in_name = |w: &str| icon.name.contains(w);
    let in_alias = |w: &str| icon.aliases.iter().any(|a| a.contains(w));
    if !words.iter().all(|w| in_name(w) || in_alias(w)) {
        return None;
    }
    let joined = query.replace(' ', "-");
    Some(if icon.name == joined {
        0
    } else if icon.name.starts_with(&joined) {
        1
    } else if icon
        .name
        .split('-')
        .any(|part| words.first().is_some_and(|w| part.starts_with(w)))
    {
        2
    } else if words.iter().all(|w| in_name(w)) {
        3
    } else {
        4
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(key: &str) -> &'static SetSpec {
        SetSpec::by_key(key).unwrap()
    }

    /// The longest fitting suffix decides the variant; a bare name is the default.
    #[test]
    fn the_longest_suffix_names_the_variant() {
        let m = spec("material-symbols");
        assert_eq!(
            variant_of(m, 0, "home-outline-rounded"),
            Some((3, "home".into()))
        );
        assert_eq!(variant_of(m, 0, "home-rounded"), Some((2, "home".into())));
        assert_eq!(variant_of(m, 0, "home"), Some((0, "home".into())));
        let h = spec("heroicons");
        assert_eq!(variant_of(h, 0, "bell-20-solid"), Some((2, "bell".into())));
        // Font Awesome's variants are packages, not suffixes.
        let fa = spec("font-awesome");
        assert_eq!(variant_of(fa, 1, "bell"), Some((1, "bell".into())));
    }

    /// A package reads into icons by base name and variant, aliases as search
    /// words, hidden ones left out, the set's grid kept.
    #[test]
    fn a_package_reads_into_variants_and_aliases() {
        let json = br#"{"prefix":"ph","width":256,"height":256,
            "icons":{"acorn":{"body":"<path d='M0 0h1'/>"},
                     "acorn-bold":{"body":"<path d='M0 0h2'/>"},
                     "gone":{"body":"<path/>","hidden":true},
                     "tall":{"body":"<path/>","height":300}},
            "aliases":{"nut":{"parent":"acorn"}}}"#;
        let set = read_set(spec("phosphor"), &[json.to_vec()]).unwrap();
        let names: Vec<(&str, usize)> = set
            .icons
            .iter()
            .map(|i| (i.name.as_str(), i.variant))
            .collect();
        assert_eq!(names, [("acorn", 0), ("acorn", 3), ("tall", 0)]);
        assert_eq!(set.icons[0].aliases, ["nut"]);
        assert_eq!(set.icons[2].view, [0.0, 0.0, 256.0, 300.0]);
        assert_eq!(set.grid, 24.0, "256 path units draw at 24");
    }

    /// Search ranks an exact name, then a prefix, then a word start, then a
    /// substring, then an alias; every word must match somewhere.
    #[test]
    fn search_ranks_exact_prefix_word_substring_alias() {
        let icon = |name: &str, aliases: &[&str]| Icon {
            name: name.into(),
            variant: 0,
            body: String::new(),
            view: [0.0, 0.0, 24.0, 24.0],
            aliases: aliases.iter().map(|a| a.to_string()).collect(),
        };
        let r = |i: &Icon, q: &str| rank(i, q, &q.split(' ').collect::<Vec<_>>());
        assert_eq!(r(&icon("arrow", &[]), "arrow"), Some(0));
        assert_eq!(r(&icon("arrow-right", &[]), "arrow"), Some(1));
        assert_eq!(r(&icon("circle-arrow", &[]), "arrow"), Some(2));
        assert_eq!(r(&icon("sparrow", &[]), "arrow"), Some(3));
        assert_eq!(r(&icon("nut", &["acorn"]), "acorn"), Some(4));
        assert_eq!(r(&icon("arrow-right", &[]), "arrow left"), None);
        assert_eq!(r(&icon("arrow-right", &[]), "arrow right"), Some(0));
        assert_eq!(r(&icon("arrow-right-bold", &[]), "arrow right"), Some(1));
    }

    /// The markup scales the view box to the size asked, keeping the aspect.
    #[test]
    fn markup_scales_to_the_size_asked() {
        let i = Icon {
            name: "x".into(),
            variant: 0,
            body: "<path/>".into(),
            view: [0.0, 0.0, 256.0, 128.0],
            aliases: Vec::new(),
        };
        let svg = i.svg(24.0, "#112233");
        assert!(svg.contains(r##"width="24" height="12" viewBox="0 0 256 128" color="#112233""##));
    }
}

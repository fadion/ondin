//! Sketch's older text: an `NSAttributedString` kept by `NSKeyedArchiver` as a
//! base64 binary plist, read into the JSON shape later Sketch writes, so
//! `sketch_in` reads both the same way.
//!
//! Sketch wrote text this way until version 48: a layer's
//! `attributedString.archivedAttributedString._archive`, and the archived values
//! in its `style.textStyle.encodedAttributes`.

use base64::Engine as _;
use serde_json::{Map, Value, json};

/// A layer's archived attributed string as the `attributedString` later Sketch
/// writes: `string`, then `attributes` as runs of `location`, `length` and
/// `attributes`, counted in UTF-16 units as Sketch counts them.
pub fn attributed_string(archive: &str) -> Option<Value> {
    let a = Archive::open(archive)?;
    let root = a.root()?;
    let text = a.string(a.get(root, "NSString")?)?;
    let attrs = a.get(root, "NSAttributes")?;
    let runs = match a.get(root, "NSAttributeInfo") {
        // One dictionary over the whole string.
        None => vec![(text.encode_utf16().count(), a.attrs(attrs))],
        Some(info) => {
            let list = a.list(attrs);
            let dicts: Vec<Value> = list.iter().map(|d| a.attrs(d)).collect();
            varints(&a.data(info)?)
                .chunks(2)
                .filter_map(|r| Some((*r.first()?, dicts.get(*r.get(1)?)?.clone())))
                .collect()
        }
    };
    let mut at = 0;
    let runs: Vec<Value> = runs
        .into_iter()
        .map(|(len, attributes)| {
            let run = json!({ "location": at, "length": len, "attributes": attributes });
            at += len;
            run
        })
        .collect();
    Some(json!({ "string": text, "attributes": runs }))
}

/// A text style's `encodedAttributes` with each archived value read and each
/// `NS` key renamed as later Sketch keys them; a later file's pass unchanged.
pub fn encoded_attributes(encoded: &Value) -> Value {
    let Some(map) = encoded.as_object() else {
        return encoded.clone();
    };
    let mut out = Map::new();
    for (k, v) in map {
        let read = match v.get("_archive").and_then(Value::as_str) {
            Some(archive) => Archive::open(archive).and_then(|a| a.attr(k, a.root()?)),
            None => attr_value(k, v.clone()),
        };
        match read {
            Some((key, value)) => out.insert(key.into(), value),
            None => out.insert(k.clone(), v.clone()),
        };
    }
    Value::Object(out)
}

/// A plain (unarchived) value under an `NS` key, renamed.
fn attr_value(key: &str, v: Value) -> Option<(&'static str, Value)> {
    match key {
        "NSKern" => Some(("kerning", v)),
        "NSUnderline" => Some(("underlineStyle", v)),
        "NSStrikethrough" => Some(("strikethroughStyle", v)),
        _ => None,
    }
}

/// A binary plist's objects.
#[derive(Clone, Debug, PartialEq)]
enum P {
    Null,
    Bool(bool),
    Int(i64),
    Real(f64),
    Data(Vec<u8>),
    Str(String),
    Uid(usize),
    Array(Vec<P>),
    Dict(Vec<(P, P)>),
}

/// An `NSKeyedArchiver` archive: its object table, read whole.
struct Archive {
    objects: Vec<P>,
    root: Option<usize>,
}

impl Archive {
    fn open(b64: &str) -> Option<Archive> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .ok()?;
        let top = bplist(&bytes)?;
        let field = |k: &str| match &top {
            P::Dict(d) => d
                .iter()
                .find(|(key, _)| *key == P::Str(k.into()))
                .map(|(_, v)| v.clone()),
            _ => None,
        };
        let P::Array(objects) = field("$objects")? else {
            return None;
        };
        let root = match field("$top")? {
            P::Dict(d) => d.into_iter().find_map(|(k, v)| match (k, v) {
                (P::Str(k), P::Uid(u)) if k == "root" => Some(u),
                _ => None,
            }),
            _ => None,
        };
        Some(Archive { objects, root })
    }

    fn root(&self) -> Option<&P> {
        self.objects.get(self.root?)
    }

    /// `p` with a reference followed.
    fn deref<'a>(&'a self, p: &'a P) -> &'a P {
        match p {
            P::Uid(u) => self.objects.get(*u).unwrap_or(&P::Null),
            other => other,
        }
    }

    /// The value under `key` of the archived object `obj`, its reference followed.
    fn get<'a>(&'a self, obj: &'a P, key: &str) -> Option<&'a P> {
        let P::Dict(d) = self.deref(obj) else {
            return None;
        };
        d.iter()
            .find(|(k, _)| matches!(k, P::Str(s) if s == key))
            .map(|(_, v)| self.deref(v))
            .filter(|v| !matches!(v, P::Str(s) if s == "$null"))
    }

    fn string(&self, p: &P) -> Option<String> {
        match self.deref(p) {
            P::Str(s) => Some(s.clone()),
            obj => self.get(obj, "NS.string").and_then(|s| self.string(s)),
        }
    }

    fn data(&self, p: &P) -> Option<Vec<u8>> {
        match self.deref(p) {
            P::Data(d) => Some(d.clone()),
            obj => self.get(obj, "NS.data").and_then(|d| self.data(d)),
        }
    }

    fn number(&self, p: &P) -> Option<f64> {
        match self.deref(p) {
            P::Int(i) => Some(*i as f64),
            P::Real(r) => Some(*r),
            _ => None,
        }
    }

    /// An `NSArray`'s items, or a dictionary's values, references followed.
    fn list<'a>(&'a self, p: &'a P) -> Vec<&'a P> {
        match self.get(p, "NS.objects") {
            Some(P::Array(items)) => items.iter().map(|i| self.deref(i)).collect(),
            _ => Vec::new(),
        }
    }

    /// An `NSDictionary`'s keys and values, references followed.
    fn dict<'a>(&'a self, p: &'a P) -> Vec<(String, &'a P)> {
        let keys = match self.get(p, "NS.keys") {
            Some(P::Array(k)) => k.iter().filter_map(|k| self.string(k)).collect(),
            _ => Vec::new(),
        };
        keys.into_iter().zip(self.list(p)).collect()
    }

    /// One run's attributes as later Sketch keys them.
    fn attrs(&self, p: &P) -> Value {
        Value::Object(
            self.dict(p)
                .into_iter()
                .filter_map(|(k, v)| self.attr(&k, v))
                .map(|(k, v)| (k.to_owned(), v))
                .collect(),
        )
    }

    fn attr(&self, key: &str, v: &P) -> Option<(&'static str, Value)> {
        match key {
            "MSAttributedStringFontAttribute" => {
                let font = self.get(v, "NSFontDescriptorAttributes")?;
                let mut name = Value::Null;
                let mut size = Value::Null;
                for (k, x) in self.dict(font) {
                    match k.as_str() {
                        "NSFontNameAttribute" => name = self.string(x).into(),
                        "NSFontSizeAttribute" => size = self.number(x).into(),
                        _ => {}
                    }
                }
                Some((
                    "MSAttributedStringFontAttribute",
                    json!({ "attributes": { "name": name, "size": size } }),
                ))
            }
            "NSColor" => Some(("MSAttributedStringColorAttribute", self.colour(v)?)),
            "NSParagraphStyle" => {
                let n = |k: &str| self.get(v, k).and_then(|x| self.number(x));
                let mut p = Map::new();
                if let Some(a) = n("NSAlignment") {
                    p.insert("alignment".into(), json!(a));
                }
                if let Some(h) = n("NSMaxLineHeight") {
                    p.insert("maximumLineHeight".into(), json!(h));
                }
                if let Some(h) = n("NSMinLineHeight") {
                    p.insert("minimumLineHeight".into(), json!(h));
                }
                Some(("paragraphStyle", Value::Object(p)))
            }
            _ => attr_value(key, self.number(v).map_or(Value::Null, Value::from)),
        }
    }

    /// An `NSColor` as `{red, green, blue, alpha}`: its components when it
    /// keeps them, which carry the alpha, else its RGB or white string.
    fn colour(&self, v: &P) -> Option<Value> {
        let numbers = |k: &str| -> Option<Vec<f64>> {
            let d = self.data(self.get(v, k)?)?;
            let s = String::from_utf8_lossy(&d);
            Some(
                s.trim_end_matches('\0')
                    .split_whitespace()
                    .filter_map(|x| x.parse().ok())
                    .collect(),
            )
        };
        let c = numbers("NSComponents")
            .filter(|c| c.len() == 4)
            .or_else(|| numbers("NSRGB").filter(|c| c.len() >= 3))
            .or_else(|| {
                numbers("NSWhite")
                    .filter(|c| !c.is_empty())
                    .map(|w| vec![w[0], w[0], w[0], w.get(1).copied().unwrap_or(1.0)])
            })?;
        Some(json!({
            "red": c[0], "green": c[1], "blue": c[2],
            "alpha": c.get(3).copied().unwrap_or(1.0),
        }))
    }
}

/// `NSAttributeInfo`'s run lengths and attribute indices, each a little-endian
/// base-128 number.
fn varints(b: &[u8]) -> Vec<usize> {
    let mut out = Vec::new();
    let (mut v, mut shift) = (0usize, 0);
    for &byte in b {
        v |= usize::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            out.push(v);
            v = 0;
            shift = 0;
        } else {
            shift += 7;
        }
    }
    out
}

/// A binary property list's top object.
fn bplist(b: &[u8]) -> Option<P> {
    if !b.starts_with(b"bplist00") || b.len() < 40 {
        return None;
    }
    let t = &b[b.len() - 32..];
    let off_size = usize::from(t[6]);
    let ref_size = usize::from(t[7]);
    let be = |s: &[u8]| s.iter().fold(0usize, |v, x| (v << 8) | usize::from(*x));
    let count = be(&t[8..16]);
    let top = be(&t[16..24]);
    let table = be(&t[24..32]);
    let offsets: Vec<usize> = (0..count)
        .map(|i| {
            b.get(table + i * off_size..table + (i + 1) * off_size)
                .map(be)
        })
        .collect::<Option<_>>()?;
    let mut seen = vec![false; count];
    read_object(b, &offsets, ref_size, top, &mut seen)
}

fn read_object(b: &[u8], offsets: &[usize], refs: usize, i: usize, seen: &mut [bool]) -> Option<P> {
    // A reference back into the object being read would recurse for ever.
    if *seen.get(i)? {
        return None;
    }
    seen[i] = true;
    let be = |s: &[u8]| s.iter().fold(0usize, |v, x| (v << 8) | usize::from(*x));
    let mut at = *offsets.get(i)?;
    let marker = *b.get(at)?;
    at += 1;
    let low = usize::from(marker & 0x0f);
    let mut len = || -> Option<usize> {
        if low != 0x0f {
            return Some(low);
        }
        let n = 1 << (b.get(at)? & 0x0f);
        let v = be(b.get(at + 1..at + 1 + n)?);
        at += 1 + n;
        Some(v)
    };
    let p = match marker >> 4 {
        0x0 => match low {
            8 => P::Bool(false),
            9 => P::Bool(true),
            _ => P::Null,
        },
        // Eight bytes are signed, fewer unsigned; both fit an `i64` this way.
        0x1 => {
            let raw = b.get(at..at + (1 << low))?;
            P::Int(raw.iter().fold(0u64, |v, x| (v << 8) | u64::from(*x)) as i64)
        }
        0x2 => match low {
            2 => P::Real(f64::from(f32::from_be_bytes(
                b.get(at..at + 4)?.try_into().ok()?,
            ))),
            _ => P::Real(f64::from_be_bytes(b.get(at..at + 8)?.try_into().ok()?)),
        },
        0x4 => {
            let n = len()?;
            P::Data(b.get(at..at + n)?.to_vec())
        }
        0x5 => {
            let n = len()?;
            P::Str(b.get(at..at + n)?.iter().map(|c| char::from(*c)).collect())
        }
        0x6 => {
            let n = len()?;
            let units: Vec<u16> = b
                .get(at..at + 2 * n)?
                .chunks(2)
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect();
            P::Str(String::from_utf16_lossy(&units))
        }
        0x8 => P::Uid(be(b.get(at..at + low + 1)?)),
        0xa => {
            let n = len()?;
            let items = (0..n)
                .map(|k| {
                    let r = be(b.get(at + k * refs..at + (k + 1) * refs)?);
                    read_object(b, offsets, refs, r, seen)
                })
                .collect::<Option<_>>()?;
            P::Array(items)
        }
        0xd => {
            let n = len()?;
            let entry = |k: usize, seen: &mut [bool]| {
                let r = be(b.get(at + k * refs..at + (k + 1) * refs)?);
                read_object(b, offsets, refs, r, seen)
            };
            let mut d = Vec::with_capacity(n);
            for k in 0..n {
                d.push((entry(k, seen)?, entry(n + k, seen)?));
            }
            P::Dict(d)
        }
        _ => P::Null,
    };
    seen[i] = false;
    Some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A binary plist written by hand: two-byte offsets and one-byte
    /// references, so up to 255 objects.
    #[derive(Default)]
    struct Plist(Vec<Vec<u8>>);

    impl Plist {
        fn add(&mut self, bytes: Vec<u8>) -> u8 {
            self.0.push(bytes);
            (self.0.len() - 1) as u8
        }
        fn s(&mut self, s: &str) -> u8 {
            let mut b = Self::marker(0x50, s.len());
            b.extend(s.as_bytes());
            self.add(b)
        }
        fn real(&mut self, x: f64) -> u8 {
            let mut b = vec![0x23];
            b.extend(x.to_be_bytes());
            self.add(b)
        }
        fn data(&mut self, d: &[u8]) -> u8 {
            let mut b = Self::marker(0x40, d.len());
            b.extend(d);
            self.add(b)
        }
        fn uid(&mut self, u: u8) -> u8 {
            self.add(vec![0x80, u])
        }
        /// A marker of `kind` and length `n`, which past 14 follows as an int.
        fn marker(kind: u8, n: usize) -> Vec<u8> {
            if n < 15 {
                vec![kind | n as u8]
            } else {
                vec![kind | 0x0f, 0x10, n as u8]
            }
        }
        fn array(&mut self, items: &[u8]) -> u8 {
            let mut b = Self::marker(0xa0, items.len());
            b.extend(items);
            self.add(b)
        }
        fn dict(&mut self, pairs: &[(u8, u8)]) -> u8 {
            let mut b = Self::marker(0xd0, pairs.len());
            b.extend(pairs.iter().map(|p| p.0));
            b.extend(pairs.iter().map(|p| p.1));
            self.add(b)
        }
        fn finish(&self, top: u8) -> String {
            let mut out = b"bplist00".to_vec();
            let mut offsets = Vec::new();
            for o in &self.0 {
                offsets.extend((out.len() as u16).to_be_bytes());
                out.extend(o);
            }
            let table = out.len() as u64;
            out.extend(&offsets);
            out.extend([0, 0, 0, 0, 0, 0, 2, 1]);
            out.extend((self.0.len() as u64).to_be_bytes());
            out.extend(u64::from(top).to_be_bytes());
            out.extend(table.to_be_bytes());
            base64::engine::general_purpose::STANDARD.encode(out)
        }
    }

    /// An archive whose `$objects` are built by `objects`, which returns the
    /// root's index among them; every object is a plist value, references
    /// between them `uid`s.
    struct Keyed {
        p: Plist,
        objects: Vec<u8>,
    }

    impl Keyed {
        fn new() -> Self {
            let mut p = Plist::default();
            let null = p.s("$null");
            Keyed {
                p,
                objects: vec![null],
            }
        }
        /// Add `plist` to `$objects`, returning the uid that names it.
        fn obj(&mut self, plist: u8) -> u8 {
            self.objects.push(plist);
            let index = (self.objects.len() - 1) as u8;
            self.p.uid(index)
        }
        fn finish(mut self, root: u8) -> String {
            let objects = self.p.array(&self.objects);
            let k_objects = self.p.s("$objects");
            let k_top = self.p.s("$top");
            let k_root = self.p.s("root");
            let top = self.p.dict(&[(k_root, root)]);
            let doc = self.p.dict(&[(k_objects, objects), (k_top, top)]);
            self.p.finish(doc)
        }
    }

    /// An `NSDictionary` of `pairs`, its keys strings, its values uids.
    fn ns_dict(k: &mut Keyed, pairs: &[(&str, u8)]) -> u8 {
        let keys: Vec<u8> = pairs
            .iter()
            .map(|(key, _)| {
                let s = k.p.s(key);
                k.obj(s)
            })
            .collect();
        let values: Vec<u8> = pairs.iter().map(|p| p.1).collect();
        let (nk, no) = (k.p.s("NS.keys"), k.p.s("NS.objects"));
        let (ka, va) = (k.p.array(&keys), k.p.array(&values));
        let d = k.p.dict(&[(nk, ka), (no, va)]);
        k.obj(d)
    }

    /// One run's attributes: a font, a colour by components and a kerning.
    fn run_attrs(k: &mut Keyed, font: &str, size: f64, rgba: &str) -> u8 {
        let name = k.p.s(font);
        let name = k.obj(name);
        let size = k.p.real(size);
        let size = k.obj(size);
        let font_attrs = ns_dict(
            k,
            &[("NSFontNameAttribute", name), ("NSFontSizeAttribute", size)],
        );
        let key = k.p.s("NSFontDescriptorAttributes");
        let descriptor = k.p.dict(&[(key, font_attrs)]);
        let descriptor = k.obj(descriptor);
        let comps = k.p.data(rgba.as_bytes());
        let comps = k.obj(comps);
        let key = k.p.s("NSComponents");
        let colour = k.p.dict(&[(key, comps)]);
        let colour = k.obj(colour);
        let kern = k.p.real(1.5);
        let kern = k.obj(kern);
        ns_dict(
            k,
            &[
                ("MSAttributedStringFontAttribute", descriptor),
                ("NSColor", colour),
                ("NSKern", kern),
            ],
        )
    }

    /// Two runs over "Hi there", their lengths and attribute indices from
    /// `NSAttributeInfo`, read as later Sketch writes them.
    /// flip: give every run the first attributes in `attributed_string` →
    /// fails at the second run's font.
    #[test]
    fn runs_come_out_as_later_sketch_writes_them() {
        let mut k = Keyed::new();
        let a = run_attrs(&mut k, "Avenir-Black", 12.0, "1 0 0 1");
        let b = run_attrs(&mut k, "Avenir-Book", 9.0, "0 0 1 0.5");
        let string = k.p.s("Hi there");
        let string = k.obj(string);
        let no = k.p.s("NS.objects");
        let list = k.p.array(&[a, b]);
        let attrs = k.p.dict(&[(no, list)]);
        let attrs = k.obj(attrs);
        let info = k.p.data(&[3, 0, 5, 1]);
        let info = k.obj(info);
        let (ks, ka, ki) = (
            k.p.s("NSString"),
            k.p.s("NSAttributes"),
            k.p.s("NSAttributeInfo"),
        );
        let root = k.p.dict(&[(ks, string), (ka, attrs), (ki, info)]);
        let root = k.obj(root);
        let v = attributed_string(&k.finish(root)).expect("the archive reads");
        assert_eq!(v["string"], "Hi there");
        let runs = v["attributes"].as_array().unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(
            (runs[1]["location"].clone(), runs[1]["length"].clone()),
            (json!(3), json!(5))
        );
        let font =
            |r: &Value| r["attributes"]["MSAttributedStringFontAttribute"]["attributes"].clone();
        assert_eq!(
            font(&runs[0]),
            json!({ "name": "Avenir-Black", "size": 12.0 })
        );
        assert_eq!(
            font(&runs[1]),
            json!({ "name": "Avenir-Book", "size": 9.0 })
        );
        assert_eq!(
            runs[1]["attributes"]["MSAttributedStringColorAttribute"],
            json!({ "red": 0.0, "green": 0.0, "blue": 1.0, "alpha": 0.5 })
        );
        assert_eq!(runs[0]["attributes"]["kerning"], 1.5);
    }

    /// One attribute dictionary with no `NSAttributeInfo` covers the whole
    /// string, counted in UTF-16 units.
    #[test]
    fn one_dictionary_covers_the_whole_string() {
        let mut k = Keyed::new();
        let a = run_attrs(&mut k, "Avenir-Black", 6.0, "1 1 1 1");
        let string = k.p.s("ab");
        let string = k.obj(string);
        let (ks, ka) = (k.p.s("NSString"), k.p.s("NSAttributes"));
        let root = k.p.dict(&[(ks, string), (ka, a)]);
        let root = k.obj(root);
        let v = attributed_string(&k.finish(root)).unwrap();
        assert_eq!(v["attributes"][0]["length"], 2);
    }

    #[test]
    fn attribute_info_is_base_128() {
        assert_eq!(varints(&[7, 0, 0x81, 0x01, 1]), [7, 0, 129, 1]);
    }

    #[test]
    fn something_that_is_not_an_archive_reads_as_nothing() {
        assert!(attributed_string("bm90IGEgcGxpc3Q=").is_none());
        assert!(attributed_string("%%%").is_none());
    }
}

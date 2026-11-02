//! Penpot's older binary export (its binfile v1, written until the zip export
//! replaced it) unpacked into the entries of the zip export, so
//! `penpot_in::read` reads both.
//!
//! The file is a mark byte, Penpot's magic number and a version, then one zstd
//! stream, which the caller inflates (core holds no decompressor). The stream is
//! a run of marked sections: a label (`:v1/metadata`, `:v1/files`, `:v1/rels`,
//! `:v1/sobjects`) and the objects under it, each object Fressian-encoded
//! Clojure data, and the stored pictures as raw byte streams. The data is
//! Penpot's own model, the one its zip export writes as JSON, so translating it
//! is mostly renaming: keyword keys become camelCase, keywords and UUIDs strings.

use crate::foreign::Archive;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// Penpot's binfile magic number, after the leading mark byte.
const MAGIC: u64 = 0x0B1A_8650_63A1_5FC5;

/// The bytes ahead of the zstd stream: a mark, the magic and the version.
pub const HEADER: usize = 17;

/// Whether `bytes` open as a version-1 Penpot binfile.
pub fn is_binfile(bytes: &[u8]) -> bool {
    bytes.len() > HEADER
        && bytes[0] == 1
        && bytes[1..9] == MAGIC.to_be_bytes()
        && bytes[9..17] == 1u64.to_be_bytes()
}

/// The entries of a Penpot zip export, by name.
#[derive(Default)]
pub struct Entries(BTreeMap<String, Vec<u8>>);

impl Archive for Entries {
    fn entry(&mut self, name: &str) -> Option<Vec<u8>> {
        self.0.get(name).cloned()
    }

    fn names(&mut self) -> Vec<String> {
        self.0.keys().cloned().collect()
    }
}

impl Entries {
    fn put(&mut self, name: String, v: &Value) {
        self.0.insert(name, v.to_string().into_bytes());
    }
}

/// Unpack `body`, the inflated stream after the header, into the entries
/// Penpot's zip export would hold.
pub fn unpack(body: &[u8]) -> Result<Entries, String> {
    unpack_within(body, MAX_VALUES)
}

/// [`unpack`], with the whole stream producing at most `budget` values.
fn unpack_within(body: &[u8], budget: usize) -> Result<Entries, String> {
    let mut sections: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut objects: Vec<(String, String, Vec<u8>)> = Vec::new();
    let mut label = String::new();
    let mut object = String::new();
    let mut left = budget;
    let mut at = 0;
    while at < body.len() {
        let mark = body[at];
        at += 1;
        match mark {
            // A label is an object naming the section that follows.
            4 | 5 => {
                if mark == 4 {
                    at += 1;
                }
                let n = usize::try_from(long(body, at)?).map_err(|_| "a bad length")?;
                at += 8;
                let bytes = body
                    .get(at..at.saturating_add(n))
                    .ok_or("an object cut short")?;
                at += n;
                let mut reader = Fressian::within(bytes, left);
                let v = to_json(&reader.read()?);
                left = reader.left;
                if mark == 4 {
                    label = v.as_str().unwrap_or_default().to_owned();
                } else {
                    sections.entry(label.clone()).or_default().push(v);
                }
            }
            3 => {
                let id = body.get(at..at + 16).ok_or("an id cut short")?;
                object = hyphenated(id);
                at += 16;
            }
            2 => {
                let n = usize::try_from(long(body, at)?).map_err(|_| "a bad length")?;
                at += 8;
                let bytes = body
                    .get(at..at.saturating_add(n))
                    .ok_or("a picture cut short")?;
                at += n;
                let kind = sections
                    .get("v1/sobjects")
                    .and_then(|s| s.last())
                    .and_then(|m| m.get("contentType"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                objects.push((object.clone(), kind, bytes.to_vec()));
            }
            m => return Err(format!("an unknown section mark {m}")),
        }
    }
    let empty = Vec::new();
    let section = |name: &str| sections.get(name).unwrap_or(&empty);
    let ids: Vec<String> = section("v1/metadata")
        .first()
        .and_then(|m| m.get("files"))
        .and_then(Value::as_array)
        .map(|f| {
            f.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    // Each file is its own object, then the list of its pictures.
    let files = section("v1/files");
    let mut out = Entries::default();
    let mut manifest = Vec::new();
    for (i, id) in ids.iter().enumerate() {
        let Some(file) = files.get(i * 2) else {
            break;
        };
        let name = file.get("name").cloned().unwrap_or(Value::Null);
        manifest.push(json!({ "id": id, "name": name }));
        out.put(
            format!("files/{id}.json"),
            &json!({ "id": id, "name": name }),
        );
        let data = file.get("data").unwrap_or(&Value::Null);
        let pages = data.get("pages").and_then(Value::as_array);
        for (index, pid) in pages.into_iter().flatten().enumerate() {
            let Some(pid) = pid.as_str() else { continue };
            let Some(page) = data.get("pagesIndex").and_then(|p| p.get(pid)) else {
                continue;
            };
            let name = page.get("name").cloned().unwrap_or(Value::Null);
            out.put(
                format!("files/{id}/pages/{pid}.json"),
                &json!({ "id": pid, "name": name, "index": index }),
            );
            for (sid, shape) in page
                .get("objects")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                out.put(format!("files/{id}/pages/{pid}/{sid}.json"), shape);
            }
        }
        for kind in ["colors", "typographies"] {
            for (aid, asset) in data
                .get(kind)
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                out.put(format!("files/{id}/{kind}/{aid}.json"), asset);
            }
        }
        let media = files.get(i * 2 + 1).and_then(Value::as_array);
        for m in media.into_iter().flatten() {
            if let Some(mid) = m.get("id").and_then(Value::as_str) {
                out.put(format!("files/{id}/media/{mid}.json"), m);
            }
        }
    }
    let relations: Vec<Value> = section("v1/rels")
        .first()
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|r| json!([r.get("fileId"), r.get("libraryFileId")]))
        .collect();
    out.put(
        "manifest.json".into(),
        &json!({ "type": "penpot/export-files", "version": 1,
                 "files": manifest, "relations": relations }),
    );
    for (id, kind, bytes) in objects {
        let ext = kind.rsplit('/').next().unwrap_or("bin");
        out.0.insert(format!("objects/{id}.{ext}"), bytes);
    }
    Ok(out)
}

fn long(body: &[u8], at: usize) -> Result<u64, String> {
    body.get(at..at + 8)
        .and_then(|b| b.try_into().ok())
        .map(u64::from_be_bytes)
        .ok_or_else(|| "a length cut short".to_owned())
}

/// A UUID's sixteen bytes as Penpot writes them in JSON.
fn hyphenated(b: &[u8]) -> String {
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

/// A decoded Fressian value, Penpot's records already taken apart.
#[derive(Clone, Debug, PartialEq)]
enum F {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Bytes(Vec<u8>),
    Keyword(String),
    Uuid(String),
    List(Vec<F>),
    Map(Vec<(F, F)>),
}

/// The JSON Penpot's zip export writes for `f`: keyword keys camelCased, keywords
/// and UUIDs as strings, bytes dropped.
fn to_json(f: &F) -> Value {
    match f {
        F::Null | F::Bytes(_) => Value::Null,
        F::Bool(b) => Value::Bool(*b),
        F::Int(i) => json!(i),
        F::Float(x) => serde_json::Number::from_f64(*x).map_or(Value::Null, Value::Number),
        F::Str(s) | F::Keyword(s) | F::Uuid(s) => Value::String(s.clone()),
        F::List(l) => Value::Array(l.iter().map(to_json).collect()),
        F::Map(m) => Value::Object(
            m.iter()
                .map(|(k, v)| (key(k), to_json(v)))
                .collect::<Map<String, Value>>(),
        ),
    }
}

fn key(k: &F) -> String {
    match k {
        F::Keyword(s) => camel(s),
        F::Str(s) | F::Uuid(s) => s.clone(),
        other => to_json(other).to_string(),
    }
}

/// `fill-color-ref-id` → `fillColorRefId`, as Penpot's JSON export names keys.
fn camel(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut up = false;
    for c in s.trim_end_matches('?').chars() {
        if c == '-' {
            up = true;
        } else if up {
            out.extend(c.to_uppercase());
            up = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// How deeply one object's values may nest. Penpot's data nests a dozen deep;
/// each level is a stack frame here and again in `to_json`.
const MAX_DEPTH: usize = 128;

/// How many values the whole stream may produce, a cached value counted at its
/// full size each time it is used and a string or byte run at one per
/// [`BYTES_PER_VALUE`] bytes: a few bytes of nested cache references otherwise
/// double the value per level, and every object's data is kept to the end.
/// Twenty million sits far above a real file's data (a few hundred values per
/// shape) and well inside memory.
const MAX_VALUES: usize = 20_000_000;

/// How many bytes of a cached string or byte run weigh as one value.
const BYTES_PER_VALUE: usize = 32;

/// A Fressian reader over one object's bytes, with Penpot's and Clojure's
/// handlers for the tagged structs it writes.
struct Fressian<'a> {
    buf: &'a [u8],
    at: usize,
    priority: Vec<F>,
    structs: Vec<(String, usize)>,
    /// How many `read`s are under way.
    depth: usize,
    /// How many more values the stream may produce.
    left: usize,
}

impl<'a> Fressian<'a> {
    #[cfg(test)]
    fn new(buf: &'a [u8]) -> Self {
        Self::within(buf, MAX_VALUES)
    }

    /// A reader over one object that may produce `left` values, what the
    /// objects before it in the stream left of the budget.
    fn within(buf: &'a [u8], left: usize) -> Self {
        Fressian {
            buf,
            at: 0,
            priority: Vec::new(),
            structs: Vec::new(),
            depth: 0,
            left,
        }
    }

    /// Count `n` values made, refusing once the budget is spent.
    fn made(&mut self, n: usize) -> Result<(), String> {
        self.left = self
            .left
            .checked_sub(n)
            .ok_or("an object too large to read")?;
        Ok(())
    }

    fn byte(&mut self) -> Result<u8, String> {
        let b = *self.buf.get(self.at).ok_or("an object cut short")?;
        self.at += 1;
        Ok(b)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let b = self
            .buf
            .get(self.at..self.at.saturating_add(n))
            .ok_or("an object cut short")?;
        self.at += n;
        Ok(b)
    }

    /// `n` big-endian bytes as an unsigned number.
    fn be(&mut self, n: usize) -> Result<u64, String> {
        Ok(self
            .take(n)?
            .iter()
            .fold(0, |v, b| (v << 8) | u64::from(*b)))
    }

    fn count(&mut self) -> Result<usize, String> {
        match self.read()? {
            F::Int(n) => usize::try_from(n).map_err(|_| "a negative count".to_owned()),
            _ => Err("a count that is not a number".into()),
        }
    }

    fn list(&mut self, n: usize) -> Result<Vec<F>, String> {
        (0..n).map(|_| self.read()).collect()
    }

    /// Java writes strings a UTF-16 unit at a time, so a character outside the
    /// BMP arrives as two three-byte surrogates rather than four UTF-8 bytes.
    fn string(&mut self, n: usize) -> Result<String, String> {
        let b = self.take(n)?;
        if let Ok(s) = std::str::from_utf8(b) {
            return Ok(s.to_owned());
        }
        let mut units = Vec::with_capacity(n);
        let mut i = 0;
        while i < b.len() {
            let c = u16::from(b[i]);
            let (unit, len) = match b[i] >> 4 {
                0..=7 => (c, 1),
                12 | 13 => (
                    (c & 0x1f) << 6 | u16::from(*b.get(i + 1).unwrap_or(&0) & 0x3f),
                    2,
                ),
                _ => (
                    (c & 0x0f) << 12
                        | u16::from(*b.get(i + 1).unwrap_or(&0) & 0x3f) << 6
                        | u16::from(*b.get(i + 2).unwrap_or(&0) & 0x3f),
                    3,
                ),
            };
            units.push(unit);
            i += len;
        }
        Ok(String::from_utf16_lossy(&units))
    }

    fn packed_int(&mut self, c: u8) -> Result<i64, String> {
        let (hi, n, centre): (i64, usize, i64) = match c {
            0x00..=0x3f => return Ok(i64::from(c)),
            0xff => return Ok(-1),
            0x40..=0x5f => (i64::from(c), 1, 0x50),
            0x60..=0x6f => (i64::from(c), 2, 0x68),
            0x70..=0x73 => (i64::from(c), 3, 0x72),
            0x74..=0x77 => (i64::from(c), 4, 0x76),
            0x78..=0x7b => (i64::from(c), 5, 0x7a),
            _ => (i64::from(c), 6, 0x7e),
        };
        let low = self.be(n)? as i64;
        Ok(((hi - centre) << (8 * n)) | low)
    }

    /// One value, refused once it nests past [`MAX_DEPTH`] or the stream has
    /// spent its budget of values.
    fn read(&mut self) -> Result<F, String> {
        if self.depth >= MAX_DEPTH {
            return Err("an object nested too deeply".into());
        }
        self.made(1)?;
        self.depth += 1;
        let v = self.value();
        self.depth -= 1;
        v
    }

    fn value(&mut self) -> Result<F, String> {
        let c = self.byte()?;
        Ok(match c {
            0x00..=0x7f | 0xff => F::Int(self.packed_int(c)?),
            0x80..=0x9f => self.cached(usize::from(c - 0x80))?,
            0xa0..=0xaf => {
                let (tag, n) = self
                    .structs
                    .get(usize::from(c - 0xa0))
                    .cloned()
                    .ok_or("an unknown struct")?;
                self.tagged(&tag, n)?
            }
            0xd0..=0xd7 => F::Bytes(self.take(usize::from(c - 0xd0))?.to_vec()),
            0xda..=0xe1 => F::Str(self.string(usize::from(c - 0xda))?),
            0xe4..=0xeb => F::List(self.list(usize::from(c - 0xe4))?),
            // A value cached for later: its slot is taken before it is read, so
            // what it holds is cached after it.
            0xcd => {
                let slot = self.priority.len();
                self.priority.push(F::Null);
                let v = self.read()?;
                self.priority[slot] = v.clone();
                v
            }
            0xcc => {
                let i = self.count()?;
                self.cached(i)?
            }
            0xef => {
                let tag = match self.read()? {
                    F::Str(s) => s,
                    _ => return Err("a struct with no tag".into()),
                };
                let n = self.count()?;
                self.structs.push((tag.clone(), n));
                self.tagged(&tag, n)?
            }
            0xf0 => {
                let i = self.count()?;
                let (tag, n) = self.structs.get(i).cloned().ok_or("an unknown struct")?;
                self.tagged(&tag, n)?
            }
            0xc0 => pairs(self.read()?),
            0xc1 => self.read()?,
            0xc3 => match self.read()? {
                F::Bytes(b) if b.len() == 16 => F::Uuid(hyphenated(&b)),
                _ => return Err("a malformed id".into()),
            },
            0xca => {
                let ns = self.read()?;
                let name = self.read()?;
                keyword(ns, name)
            }
            0xc8 => self.read()?,
            0xd9 => {
                let n = self.count()?;
                F::Bytes(self.take(n)?.to_vec())
            }
            0xd8 => {
                let n = self.count()?;
                let mut b = self.take(n)?.to_vec();
                if let F::Bytes(rest) = self.read()? {
                    b.extend(rest);
                }
                F::Bytes(b)
            }
            0xe3 => {
                let n = self.count()?;
                F::Str(self.string(n)?)
            }
            0xe2 => {
                let n = self.count()?;
                let mut s = self.string(n)?;
                if let F::Str(rest) = self.read()? {
                    s.push_str(&rest);
                }
                F::Str(s)
            }
            0xec => {
                let n = self.count()?;
                F::List(self.list(n)?)
            }
            0xed | 0xee => {
                let mut l = Vec::new();
                while self.buf.get(self.at).is_some_and(|b| *b != 0xfd) {
                    l.push(self.read()?);
                }
                self.at += 1;
                F::List(l)
            }
            0xf5 => F::Bool(true),
            0xf6 => F::Bool(false),
            0xf7 => F::Null,
            0xf8 => F::Int(self.be(8)? as i64),
            0xf9 => F::Float(f64::from(f32::from_bits(self.be(4)? as u32))),
            0xfa => F::Float(f64::from_bits(self.be(8)?)),
            0xfb => F::Float(0.0),
            0xfc => F::Float(1.0),
            0xb0 | 0xb3 | 0xb5 => {
                let n = self.count()?;
                F::List(self.list(n)?)
            }
            0xb1 => {
                let n = self.count()?;
                F::List(
                    (0..n)
                        .map(|_| self.be(8).map(|b| F::Float(f64::from_bits(b))))
                        .collect::<Result<_, _>>()?,
                )
            }
            0xb4 => {
                let n = self.count()?;
                F::List(
                    (0..n)
                        .map(|_| {
                            self.be(4)
                                .map(|b| F::Float(f64::from(f32::from_bits(b as u32))))
                        })
                        .collect::<Result<_, _>>()?,
                )
            }
            // Metadata, then the value it rides on.
            0xf1 => {
                self.read()?;
                self.read()?
            }
            0xfe => {
                self.priority.clear();
                self.structs.clear();
                self.read()?
            }
            c => return Err(format!("an unknown Fressian code {c:#x}")),
        })
    }

    /// A copy of cached value `i`, counted at its size and refused where it
    /// would nest the value it lands in past [`MAX_DEPTH`].
    fn cached(&mut self, i: usize) -> Result<F, String> {
        let (n, depth) = extent(self.priority.get(i).ok_or("an unknown cached value")?);
        if self.depth + depth > MAX_DEPTH {
            return Err("an object nested too deeply".into());
        }
        self.made(n)?;
        Ok(self.priority[i].clone())
    }

    /// A tagged struct of `n` fields. Penpot's records are one field holding
    /// their keys and values in turn, or their numbers in order; a tag no
    /// handler knows reads as nothing.
    fn tagged(&mut self, tag: &str, n: usize) -> Result<F, String> {
        let mut fields = self.list(n)?;
        let one = |fields: &mut Vec<F>| {
            if fields.is_empty() {
                F::Null
            } else {
                fields.swap_remove(0)
            }
        };
        Ok(match tag {
            "clj/keyword" if n == 2 => {
                let name = fields.pop().unwrap_or(F::Null);
                keyword(fields.pop().unwrap_or(F::Null), name)
            }
            "clj/map" | "penpot/shape" | "penpot/geom/rect" => pairs(one(&mut fields)),
            "clj/vector" | "clj/set" | "clj/list" | "clj/seq" | "java/instant" => one(&mut fields),
            "penpot/point" => named(one(&mut fields), &["x", "y"]),
            "penpot/matrix" => named(one(&mut fields), &["a", "b", "c", "d", "e", "f"]),
            _ => F::Null,
        })
    }
}

/// How many values `f` holds, itself included, and how deeply they nest.
fn extent(f: &F) -> (usize, usize) {
    let inner = |fs: &mut dyn Iterator<Item = &F>| {
        fs.map(extent).fold((1usize, 0usize), |(n, d), (m, e)| {
            (n.saturating_add(m), d.max(e))
        })
    };
    let (n, d) = match f {
        F::List(l) => inner(&mut l.iter()),
        F::Map(m) => inner(&mut m.iter().flat_map(|(k, v)| [k, v])),
        F::Str(s) | F::Keyword(s) | F::Uuid(s) => (1 + s.len() / BYTES_PER_VALUE, 0),
        F::Bytes(b) => (1 + b.len() / BYTES_PER_VALUE, 0),
        _ => (1, 0),
    };
    (n, d + 1)
}

/// A list of keys and values in turn as a map.
fn pairs(list: F) -> F {
    let F::List(l) = list else {
        return F::Map(Vec::new());
    };
    let mut it = l.into_iter();
    let mut m = Vec::new();
    while let (Some(k), Some(v)) = (it.next(), it.next()) {
        m.push((k, v));
    }
    F::Map(m)
}

/// A record's numbers in order, keyed by `names`.
fn named(list: F, names: &[&str]) -> F {
    let F::List(l) = list else {
        return F::Null;
    };
    F::Map(
        names
            .iter()
            .zip(l)
            .map(|(n, v)| (F::Keyword((*n).to_owned()), v))
            .collect(),
    )
}

fn keyword(ns: F, name: F) -> F {
    match (ns, name) {
        (F::Str(ns), F::Str(n)) => F::Keyword(format!("{ns}/{n}")),
        (_, F::Str(n)) => F::Keyword(n),
        _ => F::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Fressian a Penpot binfile holds, written by hand: just the codes
    /// these tests need, every struct tag written afresh.
    #[derive(Default)]
    struct W(Vec<u8>);

    impl W {
        fn s(&mut self, s: &str) -> &mut Self {
            self.0.push(0xe3);
            self.int(s.len() as i64);
            self.0.extend(s.as_bytes());
            self
        }
        fn int(&mut self, i: i64) -> &mut Self {
            assert!((0..=0x3f).contains(&i), "only small ints");
            self.0.push(i as u8);
            self
        }
        fn double(&mut self, x: f64) -> &mut Self {
            self.0.push(0xfa);
            self.0.extend(x.to_bits().to_be_bytes());
            self
        }
        fn tag(&mut self, tag: &str, fields: i64) -> &mut Self {
            self.0.push(0xef);
            self.s(tag);
            self.int(fields)
        }
        fn kw(&mut self, name: &str) -> &mut Self {
            self.tag("clj/keyword", 2);
            self.0.push(0xf7);
            self.s(name)
        }
        fn list(&mut self, n: usize) -> &mut Self {
            self.0.push(0xec);
            self.int(n as i64)
        }
        fn map(&mut self, n: usize) -> &mut Self {
            self.tag("clj/map", 1);
            self.list(n * 2)
        }
        fn uuid(&mut self, last: u8) -> &mut Self {
            self.0.extend([0xc3, 0xd9, 16]);
            self.0.extend([0; 15]);
            self.0.push(last);
            self
        }
    }

    fn section(body: &mut Vec<u8>, label: Option<&str>, obj: &W) {
        if let Some(l) = label {
            let mut w = W::default();
            w.kw(l);
            body.extend([4, 5]);
            body.extend((w.0.len() as u64).to_be_bytes());
            body.extend(&w.0);
        }
        body.push(5);
        body.extend((obj.0.len() as u64).to_be_bytes());
        body.extend(&obj.0);
    }

    fn id(last: u8) -> String {
        format!("00000000-0000-0000-0000-0000000000{last:02x}")
    }

    /// A file of one page holding one rect becomes the manifest, the page and
    /// the shape Penpot's zip export would hold, keys camelCased and records
    /// opened. flip: return the keyword unchanged from `camel` → fails at the
    /// page's entry, `pagesIndex` not found.
    #[test]
    fn a_binfile_unpacks_into_the_zip_exports_entries() {
        let mut body = Vec::new();
        let mut meta = W::default();
        meta.map(1).kw("files").tag("clj/vector", 1).list(1).uuid(1);
        section(&mut body, Some("v1/metadata"), &meta);
        let mut file = W::default();
        file.map(3)
            .kw("id")
            .uuid(1)
            .kw("name")
            .s("Kit")
            .kw("data")
            .map(2);
        file.kw("pages").tag("clj/vector", 1).list(1).uuid(2);
        file.kw("pages-index").map(1).uuid(2).map(2);
        file.kw("name").s("Cover").kw("objects").map(1).uuid(3);
        file.tag("penpot/shape", 1).list(8);
        file.kw("type").kw("rect").kw("parent-id").uuid(0);
        file.kw("selrect")
            .tag("penpot/geom/rect", 1)
            .list(2)
            .kw("width")
            .double(4.5);
        file.kw("points")
            .list(1)
            .tag("penpot/point", 1)
            .list(2)
            .double(1.0)
            .double(2.0);
        section(&mut body, Some("v1/files"), &file);
        let mut media = W::default();
        media.list(0);
        section(&mut body, None, &media);
        let mut e = unpack(&body).expect("the stream reads");
        let read = |e: &mut Entries, n: &str| -> Value {
            serde_json::from_slice(&e.entry(n).unwrap_or_else(|| panic!("{n}"))).unwrap()
        };
        let manifest = read(&mut e, "manifest.json");
        assert_eq!(manifest["files"][0], json!({ "id": id(1), "name": "Kit" }));
        let page = read(&mut e, &format!("files/{}/pages/{}.json", id(1), id(2)));
        assert_eq!(page, json!({ "id": id(2), "name": "Cover", "index": 0 }));
        let rect = read(
            &mut e,
            &format!("files/{}/pages/{}/{}.json", id(1), id(2), id(3)),
        );
        assert_eq!(rect["type"], "rect");
        assert_eq!(rect["parentId"], id(0));
        assert_eq!(rect["selrect"]["width"], 4.5);
        assert_eq!(rect["points"][0], json!({ "x": 1.0, "y": 2.0 }));
    }

    /// A cached value takes its slot before it is read, so a keyword cached
    /// whole, its name cached inside it, is found again at the first slot.
    /// flip: push the slot after reading in `read`'s `0xcd` arm → fails at the
    /// second key.
    #[test]
    fn a_cached_keyword_is_found_at_its_own_slot() {
        let mut w = W::default();
        w.tag("clj/vector", 1).list(2);
        w.0.push(0xcd);
        w.tag("clj/keyword", 2);
        w.0.push(0xf7);
        w.0.push(0xcd);
        w.s("name");
        w.0.push(0x80);
        let name = F::Keyword("name".into());
        assert_eq!(
            Fressian::new(&w.0).read().unwrap(),
            F::List(vec![name.clone(), name])
        );
    }

    /// A character outside the BMP, written by Java as two surrogates, reads
    /// back whole.
    #[test]
    fn a_surrogate_pair_reads_as_one_character() {
        let mut w = W::default();
        // U+1F7EA as the surrogates D83D DFEA, each in three bytes.
        let b = [0xed, 0xa0, 0xbd, 0xed, 0xbf, 0xaa];
        w.0.push(0xda + 6);
        w.0.extend(b);
        assert_eq!(Fressian::new(&w.0).read().unwrap(), F::Str("🟪".into()));
    }

    /// A long run of values each wrapping the next is refused, not read until
    /// the stack runs out. flip: return `self.value()` straight from `read` →
    /// the test binary aborts with a stack overflow.
    #[test]
    fn a_deep_object_is_refused() {
        let mut w = W::default();
        w.0.extend(std::iter::repeat_n(0xc1, 200_000));
        w.0.push(0xf7);
        let mut body = Vec::new();
        section(&mut body, Some("v1/files"), &w);
        let err = unpack(&body).err().expect("refused");
        assert!(err.contains("nested too deeply"), "{err}");
    }

    /// Cached values that each hold two copies of the one before are refused
    /// once their copies pass the budget, in bounded time, rather than doubling
    /// to the end of memory. flip: return the clone from `cached` without
    /// `made` → the read runs on past the time limit and memory.
    #[test]
    fn cache_references_that_double_are_refused() {
        let mut w = W::default();
        w.0.extend([0xee, 0xcd, 0xf7]);
        for k in 1..=40 {
            w.0.extend([0xcd, 0xe6, 0xcc, k - 1, 0xcc, k - 1]);
        }
        w.0.push(0xfd);
        let started = std::time::Instant::now();
        let err = Fressian::new(&w.0).read().expect_err("refused");
        assert!(err.contains("too large"), "{err}");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(20),
            "{:?}",
            started.elapsed()
        );
    }

    /// The value budget runs across the whole stream, not afresh per object:
    /// objects that each fit are refused once together they pass it. flip:
    /// start each object's reader at the full budget in `unpack_within` →
    /// fails at `expect_err`.
    #[test]
    fn the_value_budget_spans_every_object() {
        let mut obj = W::default();
        obj.list(10);
        for _ in 0..10 {
            obj.int(1);
        }
        let mut body = Vec::new();
        section(&mut body, Some("v1/files"), &obj);
        unpack_within(&body, 20).expect("one object fits");
        section(&mut body, None, &obj);
        let err = unpack_within(&body, 20).err().expect("refused");
        assert!(err.contains("too large"), "{err}");
    }

    /// A cached string weighs by its length each time it is used, so a long
    /// string referenced over and over is refused rather than copied. flip:
    /// count a `Str` as one in `extent` → fails at `expect_err`.
    #[test]
    fn a_long_cached_string_weighs_by_its_length() {
        let mut w = W::default();
        // A cached string of 1920 bytes, its length packed in two bytes.
        w.0.extend([0xee, 0xcd, 0xe3, 0x57, 0x80]);
        w.0.extend([b'x'; 1920]);
        w.0.extend([0x80; 10]);
        w.0.push(0xfd);
        let err = Fressian::within(&w.0, 500).read().expect_err("refused");
        assert!(err.contains("too large"), "{err}");
    }

    struct Placed;

    impl crate::svg_in::ImageSink for Placed {
        fn place(
            &mut self,
            bytes: Vec<u8>,
            format: crate::image::ImageFormat,
        ) -> Option<(crate::image::ImageId, crate::image::ImageEntry)> {
            Some((
                crate::image::ImageId(format!("test:{}", bytes.len())),
                crate::image::ImageEntry {
                    source: crate::image::ImageSource::Embedded(bytes.into()),
                    format,
                    width: 4,
                    height: 4,
                },
            ))
        }
    }

    /// A binfile's rect, its picture fill named through the file's media list
    /// and the picture stored after `v1/sobjects`, reads through `penpot_in`
    /// with the picture on it. flip: read the media list from `files.get(i * 2)`
    /// in `unpack` → fails at the picture fill.
    #[test]
    fn a_binfiles_picture_reaches_the_reader() {
        let mut body = Vec::new();
        let mut meta = W::default();
        meta.map(1).kw("files").tag("clj/vector", 1).list(1).uuid(1);
        section(&mut body, Some("v1/metadata"), &meta);
        let mut file = W::default();
        file.map(3)
            .kw("id")
            .uuid(1)
            .kw("name")
            .s("Kit")
            .kw("data")
            .map(2);
        file.kw("pages").tag("clj/vector", 1).list(1).uuid(2);
        file.kw("pages-index").map(1).uuid(2).map(2);
        file.kw("name").s("Cover").kw("objects").map(2);
        file.uuid(0).tag("penpot/shape", 1).list(8);
        file.kw("id")
            .uuid(0)
            .kw("type")
            .kw("frame")
            .kw("name")
            .s("Root");
        file.kw("shapes").tag("clj/vector", 1).list(1).uuid(3);
        file.uuid(3).tag("penpot/shape", 1).list(18);
        file.kw("id")
            .uuid(3)
            .kw("type")
            .kw("rect")
            .kw("name")
            .s("R");
        file.kw("parent-id").uuid(0);
        file.kw("width").int(10).kw("height").int(10);
        file.kw("x").int(0).kw("y").int(0);
        file.kw("fills").tag("clj/vector", 1).list(1).map(1);
        file.kw("fill-image")
            .map(2)
            .kw("id")
            .uuid(5)
            .kw("keep-aspect-ratio")
            .0
            .push(0xf5);
        section(&mut body, Some("v1/files"), &file);
        let mut media = W::default();
        media.tag("clj/vector", 1).list(1).map(2);
        media.kw("id").uuid(5).kw("media-id").uuid(6);
        section(&mut body, None, &media);
        let mut stored = W::default();
        stored.map(1).kw("content-type").s("image/png");
        section(&mut body, Some("v1/sobjects"), &stored);
        body.push(3);
        body.extend([0; 15]);
        body.push(6);
        let png = b"\x89PNG\r\n\x1a\nrest";
        body.push(2);
        body.extend((png.len() as u64).to_be_bytes());
        body.extend(png);
        let mut entries = unpack(&body).expect("the stream reads");
        let mut ids = crate::IdSource::new(7);
        let read =
            crate::penpot_in::read(&mut entries, &mut ids, &mut Placed).expect("the file reads");
        let doc = &read.doc;
        let page = doc.page_ids().next().unwrap();
        let rect = crate::build::subtree_nodes(doc, &[page])
            .into_iter()
            .find(|&id| doc.get(id).is_some_and(|n| n.name() == "R"))
            .expect("the rect lands");
        let fills = &doc.get(rect).unwrap().paint().fills;
        assert!(
            matches!(
                &fills.iter().next().map(|f| &f.brush),
                Some(crate::BrushSource::Literal(crate::peniko::Brush::Image(b)))
                    if b.image.id.0 == format!("test:{}", png.len())
                        && b.image.fit == crate::image::ImageFit::Fill
            ),
            "{fills:?}"
        );
    }

    #[test]
    fn keys_are_camel_cased_as_penpot_exports_them() {
        assert_eq!(camel("fill-color-ref-id"), "fillColorRefId");
        assert_eq!(camel("hidden?"), "hidden");
        assert_eq!(camel("r1"), "r1");
    }

    #[test]
    fn a_binfile_header_is_told_apart() {
        let mut b = vec![1];
        b.extend(MAGIC.to_be_bytes());
        b.extend(1u64.to_be_bytes());
        b.extend([0x28, 0xb5, 0x2f, 0xfd]);
        assert!(is_binfile(&b));
        b[16] = 3;
        assert!(!is_binfile(&b), "another version");
        assert!(!is_binfile(b"PK\x03\x04rest of a zip"));
    }
}

//! The photo services the assets browser searches: their requests, their answers
//! read into one shape, and the credit each photo carries.
//!
//! Searched online each time; a key, where a service needs one, is sent to that
//! service alone, in a header, never in a URL.

use ondin_core::{Credit, CreditKind};

/// One photo service as Settings lists it.
pub(crate) struct ServiceSpec {
    /// The key it is remembered by in the preferences, and its keychain account
    /// is `<key>-key`; never changes.
    pub(crate) key: &'static str,
    pub(crate) name: &'static str,
    /// What its key is called, `None` for a service that needs none.
    pub(crate) key_label: Option<&'static str>,
    /// Where a key is made, as the caption names it.
    pub(crate) key_home: &'static str,
    /// Settings' note under the name.
    pub(crate) note: &'static str,
    /// The licence every photo of it comes under, `None` when it varies by photo.
    pub(crate) licence: Option<&'static str>,
    pub(crate) home: &'static str,
}

/// Every service, in the order Settings lists them.
pub(crate) const SERVICES: &[ServiceSpec] = &[
    ServiceSpec {
        key: "unsplash",
        name: "Unsplash",
        key_label: Some("Access key"),
        key_home: "unsplash.com/developers",
        note: "Needs an access key",
        licence: Some("Unsplash License"),
        home: "https://unsplash.com",
    },
    ServiceSpec {
        key: "pexels",
        name: "Pexels",
        key_label: Some("API key"),
        key_home: "pexels.com/api",
        note: "Needs an API key",
        licence: Some("Pexels License"),
        home: "https://www.pexels.com",
    },
    ServiceSpec {
        key: "openverse",
        name: "Openverse",
        key_label: None,
        key_home: "",
        note: "No key · licence varies by photo",
        licence: None,
        home: "https://openverse.org",
    },
];

impl ServiceSpec {
    pub(crate) fn by_key(key: &str) -> Option<&'static ServiceSpec> {
        SERVICES.iter().find(|s| s.key == key)
    }

    /// The keychain account its key is kept under.
    pub(crate) fn account(&self) -> String {
        format!("{}-key", self.key)
    }
}

/// The services' shared filter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Orientation {
    #[default]
    Any,
    Landscape,
    Portrait,
    Square,
}

impl Orientation {
    pub(crate) const ALL: [Orientation; 4] = [
        Orientation::Any,
        Orientation::Landscape,
        Orientation::Portrait,
        Orientation::Square,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Orientation::Any => "Any orientation",
            Orientation::Landscape => "Landscape",
            Orientation::Portrait => "Portrait",
            Orientation::Square => "Square",
        }
    }
}

/// The size a photo is fetched at, Settings' *Photo size*.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PhotoSize {
    Small,
    Regular,
    Full,
}

impl PhotoSize {
    pub(crate) const ALL: [PhotoSize; 3] = [PhotoSize::Small, PhotoSize::Regular, PhotoSize::Full];

    pub(crate) fn key(self) -> &'static str {
        match self {
            PhotoSize::Small => "small",
            PhotoSize::Regular => "regular",
            PhotoSize::Full => "full",
        }
    }

    pub(crate) fn from_key(key: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|s| s.key() == key)
            .unwrap_or(PhotoSize::Regular)
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            PhotoSize::Small => "Small · 400 px",
            PhotoSize::Regular => "Regular · 1080 px",
            PhotoSize::Full => "Full size",
        }
    }

    /// The width asked of a service that scales on request; `None` is the original.
    fn width(self) -> Option<u32> {
        match self {
            PhotoSize::Small => Some(400),
            PhotoSize::Regular => Some(1080),
            PhotoSize::Full => None,
        }
    }
}

/// One photo of a page of results.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Photo {
    pub(crate) service: &'static str,
    pub(crate) id: String,
    /// Its pixel size, when the service says; the tile takes its aspect.
    pub(crate) size: Option<(u32, u32)>,
    /// The picture the grid shows.
    pub(crate) thumb: String,
    /// The picture inserted, by [`PhotoSize`]: small, regular, full.
    pub(crate) urls: [String; 3],
    /// What it shows, in the service's words, for the layer's name.
    pub(crate) title: String,
    pub(crate) author: String,
    pub(crate) author_url: String,
    pub(crate) page_url: String,
    pub(crate) licence: String,
    pub(crate) licence_url: String,
    /// The address an insert is reported to (Unsplash's `download_location`).
    pub(crate) report_url: Option<String>,
}

/// A page of results and whether more follow.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Page {
    pub(crate) photos: Vec<Photo>,
    pub(crate) total: u64,
    pub(crate) more: bool,
}

/// Why a search came back empty-handed, each worded as its cure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PhotoError {
    /// No key is kept for a service that needs one.
    NoKey,
    /// The service refused the key (401).
    Refused,
    /// The hourly limit is reached (403 or 429).
    Limited,
    /// The network, or an answer that was not what the service sends.
    Unreachable,
}

/// How many photos a page asks for.
pub(crate) const PER_PAGE: u32 = 30;
/// Openverse's page: it refuses more than 20 to a request without a key.
const OPENVERSE_PAGE: u32 = 20;

/// Append `utm_source=Ondin&utm_medium=referral` to an Unsplash link, as its
/// guidelines ask of every link back.
fn referral(url: &str) -> String {
    if url.is_empty() {
        return String::new();
    }
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}utm_source=Ondin&utm_medium=referral")
}

fn encode(q: &str) -> String {
    let mut out = String::new();
    for b in q.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            b => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The search request for `service`: its URL and the authorization header's
/// value, if it needs one. `key` is the service's own key.
pub(crate) fn request(
    service: &ServiceSpec,
    query: &str,
    page: u32,
    orientation: Orientation,
    key: Option<&str>,
) -> Result<(String, Option<String>), PhotoError> {
    let q = encode(query.trim());
    match service.key {
        "unsplash" => {
            let key = key
                .filter(|k| !k.trim().is_empty())
                .ok_or(PhotoError::NoKey)?;
            let o = match orientation {
                Orientation::Any => "",
                Orientation::Landscape => "&orientation=landscape",
                Orientation::Portrait => "&orientation=portrait",
                Orientation::Square => "&orientation=squarish",
            };
            Ok((
                format!(
                    "https://api.unsplash.com/search/photos?query={q}&page={page}&per_page={PER_PAGE}{o}"
                ),
                Some(format!("Client-ID {}", key.trim())),
            ))
        }
        "pexels" => {
            let key = key
                .filter(|k| !k.trim().is_empty())
                .ok_or(PhotoError::NoKey)?;
            let o = match orientation {
                Orientation::Any => "",
                Orientation::Landscape => "&orientation=landscape",
                Orientation::Portrait => "&orientation=portrait",
                Orientation::Square => "&orientation=square",
            };
            Ok((
                format!(
                    "https://api.pexels.com/v1/search?query={q}&page={page}&per_page={PER_PAGE}{o}"
                ),
                Some(key.trim().to_string()),
            ))
        }
        _ => {
            let o = match orientation {
                Orientation::Any => "",
                Orientation::Landscape => "&aspect_ratio=wide",
                Orientation::Portrait => "&aspect_ratio=tall",
                Orientation::Square => "&aspect_ratio=square",
            };
            // Only licences that allow commercial use and changes: a design is both.
            Ok((
                format!(
                    "https://api.openverse.org/v1/images/?q={q}&page={page}&page_size={OPENVERSE_PAGE}&license_type=commercial,modification{o}"
                ),
                None,
            ))
        }
    }
}

/// The error an HTTP status stands for.
pub(crate) fn status_error(status: u16) -> PhotoError {
    match status {
        401 => PhotoError::Refused,
        403 | 429 => PhotoError::Limited,
        _ => PhotoError::Unreachable,
    }
}

/// A service's answer read into a [`Page`].
pub(crate) fn read_page(service: &'static ServiceSpec, body: &[u8], page: u32) -> Option<Page> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    let s = |v: &serde_json::Value| v.as_str().unwrap_or_default().to_string();
    let size = |w: &serde_json::Value, h: &serde_json::Value| {
        Some((
            u32::try_from(w.as_u64()?).ok()?,
            u32::try_from(h.as_u64()?).ok()?,
        ))
        .filter(|(w, h)| *w > 0 && *h > 0)
    };
    let mut photos = Vec::new();
    let (total, pages) = match service.key {
        "unsplash" => {
            for r in v["results"].as_array()? {
                let user = &r["user"];
                photos.push(Photo {
                    service: service.key,
                    id: s(&r["id"]),
                    size: size(&r["width"], &r["height"]),
                    thumb: s(&r["urls"]["small"]),
                    urls: [
                        s(&r["urls"]["small"]),
                        s(&r["urls"]["regular"]),
                        s(&r["urls"]["full"]),
                    ],
                    title: [&r["alt_description"], &r["description"]]
                        .into_iter()
                        .map(s)
                        .find(|t| !t.is_empty())
                        .unwrap_or_default(),
                    author: s(&user["name"]),
                    author_url: referral(&s(&user["links"]["html"])),
                    page_url: referral(&s(&r["links"]["html"])),
                    licence: "Unsplash License".into(),
                    licence_url: "https://unsplash.com/license".into(),
                    report_url: Some(s(&r["links"]["download_location"])).filter(|u| !u.is_empty()),
                });
            }
            (
                v["total"].as_u64().unwrap_or(0),
                v["total_pages"].as_u64().unwrap_or(0),
            )
        }
        "pexels" => {
            for r in v["photos"].as_array()? {
                let original = s(&r["src"]["original"]);
                let at = |size: PhotoSize| match size.width() {
                    Some(w) => format!("{original}?auto=compress&cs=tinysrgb&w={w}"),
                    None => original.clone(),
                };
                photos.push(Photo {
                    service: service.key,
                    id: r["id"].to_string(),
                    size: size(&r["width"], &r["height"]),
                    thumb: s(&r["src"]["medium"]),
                    urls: [
                        at(PhotoSize::Small),
                        at(PhotoSize::Regular),
                        at(PhotoSize::Full),
                    ],
                    title: s(&r["alt"]),
                    author: s(&r["photographer"]),
                    author_url: s(&r["photographer_url"]),
                    page_url: s(&r["url"]),
                    licence: "Pexels License".into(),
                    licence_url: "https://www.pexels.com/license/".into(),
                    report_url: None,
                });
            }
            let total = v["total_results"].as_u64().unwrap_or(0);
            (total, total.div_ceil(u64::from(PER_PAGE)))
        }
        _ => {
            for r in v["results"].as_array()? {
                let url = s(&r["url"]);
                photos.push(Photo {
                    service: service.key,
                    id: s(&r["id"]),
                    size: size(&r["width"], &r["height"]),
                    thumb: Some(s(&r["thumbnail"]))
                        .filter(|t| !t.is_empty())
                        .unwrap_or_else(|| url.clone()),
                    // Openverse serves one size, the original.
                    urls: [url.clone(), url.clone(), url],
                    title: s(&r["title"]),
                    author: s(&r["creator"]),
                    author_url: s(&r["creator_url"]),
                    page_url: s(&r["foreign_landing_url"]),
                    licence: licence_name(&s(&r["license"]), &s(&r["license_version"])),
                    licence_url: s(&r["license_url"]),
                    report_url: None,
                });
            }
            (
                v["result_count"].as_u64().unwrap_or(0),
                v["page_count"].as_u64().unwrap_or(0),
            )
        }
    };
    Some(Page {
        photos,
        total,
        more: u64::from(page) < pages,
    })
}

/// A Creative Commons licence code as people write it: `by-sa` 2.0 → *CC BY-SA 2.0*.
fn licence_name(code: &str, version: &str) -> String {
    let name = match code.to_ascii_lowercase().as_str() {
        "cc0" => "CC0".to_string(),
        "pdm" => return "Public Domain Mark".into(),
        "" => return String::new(),
        other => format!("CC {}", other.to_ascii_uppercase()),
    };
    match version.is_empty() {
        true => name,
        false => format!("{name} {version}"),
    }
}

/// Whether a licence asks for the photo to be credited.
fn needs_credit(licence: &str) -> bool {
    !matches!(licence, "CC0" | "CC0 1.0" | "Public Domain Mark")
}

impl Photo {
    /// The photo's credit, inserted on `today` (`YYYY-MM-DD`). An address that is
    /// not a web link is left out rather than refused.
    pub(crate) fn credit(&self, today: &str) -> Credit {
        let service = ServiceSpec::by_key(self.service);
        let provider = service.map_or("", |s| s.name).to_string();
        let author = match self.author.trim() {
            "" => "an unknown photographer".to_string(),
            a => a.to_string(),
        };
        let text = match self.service {
            "openverse" => {
                let title = match self.title.trim() {
                    "" => "Untitled".to_string(),
                    t => t.to_string(),
                };
                match self.licence.is_empty() {
                    true => format!("“{title}” by {author}, via Openverse"),
                    false => format!("“{title}” by {author}, {}, via Openverse", self.licence),
                }
            }
            _ => format!("Photo by {author} on {provider}"),
        };
        let web = |u: &str| {
            ondin_core::node::web_link(u)
                .ok()
                .filter(|l| l == u)
                .unwrap_or_default()
        };
        Credit {
            kind: CreditKind::Photo,
            text,
            author,
            author_url: web(&self.author_url),
            provider_url: web(&match self.service {
                "unsplash" => referral(service.map_or("", |s| s.home)),
                _ => service.map_or("", |s| s.home).to_string(),
            }),
            provider,
            source_url: web(&self.page_url),
            licence: self.licence.clone(),
            licence_url: web(&self.licence_url),
            required: needs_credit(&self.licence),
            inserted: today.to_string(),
        }
    }

    /// The layer name an inserted photo takes.
    pub(crate) fn layer_name(&self) -> String {
        let title = self.title.trim();
        let title: String = title.chars().take(48).collect();
        match title.is_empty() {
            true => format!("Photo by {}", self.author.trim()),
            false => title,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svc(key: &str) -> &'static ServiceSpec {
        ServiceSpec::by_key(key).unwrap()
    }

    /// A keyed service refuses to build a request without a key, and the key goes
    /// in the header, never the URL.
    #[test]
    fn a_key_goes_in_the_header_and_only_there() {
        assert_eq!(
            request(svc("unsplash"), "coffee", 1, Orientation::Any, None),
            Err(PhotoError::NoKey)
        );
        let (url, auth) = request(
            svc("unsplash"),
            "coffee shop",
            2,
            Orientation::Square,
            Some("abc"),
        )
        .unwrap();
        assert!(!url.contains("abc"));
        assert!(url.contains("query=coffee+shop&page=2"));
        assert!(url.contains("orientation=squarish"));
        assert_eq!(auth.as_deref(), Some("Client-ID abc"));
        let (url, auth) = request(svc("openverse"), "a&b", 1, Orientation::Portrait, None).unwrap();
        assert!(url.contains("q=a%26b"));
        assert!(url.contains("aspect_ratio=tall"));
        // Openverse answers 400 to more than 20 without a key.
        assert!(url.contains("page_size=20&"), "{url}");
        assert_eq!(auth, None);
    }

    /// An Unsplash answer reads into photos whose credit and links carry the
    /// referral, and whose insert reports to its download location.
    #[test]
    fn an_unsplash_page_reads_with_its_credit() {
        let body = br#"{"total":2184,"total_pages":73,"results":[{"id":"x1","width":4000,"height":3000,
            "alt_description":"a coffee cup","urls":{"small":"https://i/s","regular":"https://i/r","full":"https://i/f"},
            "links":{"html":"https://unsplash.com/photos/x1","download_location":"https://api.unsplash.com/photos/x1/download"},
            "user":{"name":"Ana Ruiz","links":{"html":"https://unsplash.com/@ana"}}}]}"#;
        let page = read_page(svc("unsplash"), body, 1).unwrap();
        assert_eq!(page.total, 2184);
        assert!(page.more);
        let p = &page.photos[0];
        assert_eq!(p.size, Some((4000, 3000)));
        assert_eq!(p.urls[1], "https://i/r");
        assert_eq!(
            p.report_url.as_deref(),
            Some("https://api.unsplash.com/photos/x1/download")
        );
        let c = p.credit("2026-11-08");
        assert_eq!(c.text, "Photo by Ana Ruiz on Unsplash");
        assert_eq!(
            c.author_url,
            "https://unsplash.com/@ana?utm_source=Ondin&utm_medium=referral"
        );
        assert!(c.required);
        assert!(c.is_well_formed());
    }

    /// An Openverse photo is credited by title, creator and licence; CC0 owes none.
    #[test]
    fn an_openverse_photo_names_its_licence() {
        let body = br#"{"result_count":3,"page_count":1,"results":[
            {"id":"o1","title":"Market stall","creator":"J. Ng","creator_url":"javascript:x",
             "url":"https://o/full.jpg","thumbnail":"https://o/t","foreign_landing_url":"https://flickr.com/p/1",
             "license":"by","license_version":"2.0","license_url":"https://creativecommons.org/licenses/by/2.0/",
             "width":800,"height":600},
            {"id":"o2","title":"","creator":"","url":"https://o/2.jpg","license":"cc0","license_version":"1.0","width":null,"height":null}]}"#;
        let page = read_page(svc("openverse"), body, 1).unwrap();
        assert!(!page.more);
        let c = page.photos[0].credit("2026-11-08");
        assert_eq!(c.text, "“Market stall” by J. Ng, CC BY 2.0, via Openverse");
        assert_eq!(c.author_url, "", "a non-web address is left out");
        assert!(c.required);
        let free = page.photos[1].credit("2026-11-08");
        assert_eq!(free.licence, "CC0 1.0");
        assert!(!free.required);
        assert_eq!(page.photos[1].size, None);
        assert_eq!(page.photos[1].thumb, "https://o/2.jpg");
    }

    /// Statuses map to the errors that name their cure.
    #[test]
    fn statuses_name_their_cure() {
        assert_eq!(status_error(401), PhotoError::Refused);
        assert_eq!(status_error(403), PhotoError::Limited);
        assert_eq!(status_error(429), PhotoError::Limited);
        assert_eq!(status_error(500), PhotoError::Unreachable);
    }
}

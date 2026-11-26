// SPDX-License-Identifier: GPL-2.0-or-later
//! Recommended feeds: a static JSON document published with the website, so the feeds a new
//! reader is offered can change without an app release and can differ by language.
//!
//! ```json
//! {
//!   "version": 1,
//!   "categories": {
//!     "news": { "en": "News", "fr": "Actualités" }
//!   },
//!   "feeds": [
//!     {
//!       "id": "le-monde",
//!       "url": "https://www.lemonde.fr/rss/une.xml",
//!       "name": "Le Monde",
//!       "description": { "en": "French daily newspaper.", "fr": "Quotidien français." },
//!       "site": "https://www.lemonde.fr",
//!       "category": "news",
//!       "locales": ["fr"],
//!       "starter": true
//!     }
//!   ]
//! }
//! ```
//!
//! - `version`: the format's major version. A document with a later one is ignored whole, so a
//!   breaking change cannot be half-read by this build. Unknown fields are ignored everywhere.
//! - `categories`: optional display names for the `category` ids the feeds use.
//! - `feeds`: the recommendations, in the order they are offered. An entry this build cannot
//!   read is skipped; it never costs the reader the rest of the list.
//! - `id`: stable for the life of the recommendation.
//! - `url`: the feed itself (RSS, Atom, RDF or JSON Feed), HTTP or HTTPS.
//! - `name`, `description`, and each category name: one string for every language, or a map
//!   from BCP 47 tags to strings. The reader's first preferred language with an entry wins,
//!   then `en`, then any entry. `description` is optional.
//! - `site`: optional, the publication's home page.
//! - `category`: optional id grouping the feed.
//! - `locales`: optional BCP 47 language ranges. When present, the feed is offered only to a
//!   reader with a matching preferred language, in any position of the device's language list.
//!   A range matches a tag that carries its subtags in order, so `pt` matches `pt-BR` and
//!   `zh-TW` matches `zh-Hant-TW`, while `en-GB` does not match `en-US`. When absent, the feed
//!   is offered to everyone.
//! - `starter`: optional. `true` preselects the feed in the first-run offer.
//!
//! The source is `website/public/recommended-feeds.json`;
//! `resource/assets/recommended-feeds.json` is the same file, bundled for a first launch with
//! no network.
use day::prelude::*;
use serde_json::Value;

/// Where the list is published. `FLEET_RECOMMENDED_URL` at build time points a development
/// build at a local preview of the website.
const PUBLISHED: &str = match option_env!("FLEET_RECOMMENDED_URL") {
    Some(url) => url,
    None => "https://fleet-reader.app/recommended-feeds.json",
};
const VERSION: u64 = 1;
/// Larger documents are refused unread.
const LIMIT: usize = 512 * 1024;
/// Entries past this many are dropped, so a runaway document cannot flood the offer.
const MAX_FEEDS: usize = 200;
const MAX_NAME: usize = 120;
const MAX_DESCRIPTION: usize = 300;
const MAX_URL: usize = 2048;
/// The last document fetched, so a launch with no network keeps what the last one showed.
const CACHE: &str = "fleet.recommended.published";

/// One recommended feed, as it applies to this reader.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recommended {
    pub id: String,
    pub url: String,
    pub title: String,
    pub description: String,
    pub site: Option<String>,
    /// The category's display name, when the feed names one.
    pub category: Option<String>,
    pub starter: bool,
}

/// Why a document was refused whole.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    TooLarge,
    NotJson(String),
    NotADocument,
    Version(u64),
}
impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge => write!(f, "larger than {LIMIT} bytes"),
            Self::NotJson(e) => write!(f, "not JSON: {e}"),
            Self::NotADocument => write!(f, "no version and feeds"),
            Self::Version(v) => write!(f, "version {v} is newer than {VERSION}"),
        }
    }
}

/// A language tag or range as lowercase subtags; `en_GB` and `en-gb` read alike.
fn subtags(tag: &str) -> Vec<String> {
    tag.trim()
        .split(['-', '_'])
        .filter(|s| !s.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

/// Whether `tag` falls in the language range `range` (RFC 4647 extended filtering): the
/// languages agree, and the range's other subtags appear in the tag in order.
fn in_range(range: &str, tag: &str) -> bool {
    let (range, tag) = (subtags(range), subtags(tag));
    let (Some(want), Some(have)) = (range.first(), tag.first()) else {
        return false;
    };
    if want != "*" && want != have {
        return false;
    }
    let mut rest = tag[1..].iter();
    range[1..].iter().all(|want| {
        want == "*"
            || rest
                .by_ref()
                // A singleton opens an extension or private use; a range never reaches into one.
                .take_while(|have| have.len() > 1)
                .any(|have| have == want)
    })
}

/// Text with its controls removed and its length bounded, or `None` when nothing is left.
fn clean(text: &str, limit: usize) -> Option<String> {
    let text: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text: String = text.chars().take(limit).collect();
    (!text.is_empty()).then_some(text)
}

/// A string for every language, or the best entry of a language map.
fn localized(value: Option<&Value>, preferred: &[String], limit: usize) -> Option<String> {
    let map = match value? {
        Value::String(text) => return clean(text, limit),
        Value::Object(map) => map,
        _ => return None,
    };
    let entries: Vec<(&str, String)> = map
        .iter()
        .filter_map(|(tag, text)| Some((tag.as_str(), clean(text.as_str()?, limit)?)))
        .collect();
    let exact = |tag: &[String]| {
        entries
            .iter()
            .find(|(key, _)| subtags(key) == tag)
            .map(|(_, text)| text.clone())
    };
    for tag in preferred {
        // `pt-BR` reads its own entry, then `pt`; `pt` alone accepts a `pt-BR` entry.
        let mut parent = subtags(tag);
        while !parent.is_empty() {
            if let Some(text) = exact(&parent) {
                return Some(text);
            }
            parent.pop();
        }
        // Then any entry in the same language: `zh-TW` reads a `zh-Hant` entry.
        let language = subtags(tag).into_iter().next();
        if let Some((_, text)) = entries
            .iter()
            .find(|(key, _)| in_range(tag, key))
            .or_else(|| {
                entries
                    .iter()
                    .find(|(key, _)| subtags(key).into_iter().next() == language)
            })
        {
            return Some(text.clone());
        }
    }
    exact(&["en".to_owned()]).or_else(|| entries.first().map(|(_, text)| text.clone()))
}

/// An absolute HTTP or HTTPS address with a host, or `None`.
fn web_address(value: Option<&Value>) -> Option<String> {
    let text = value?.as_str()?.trim();
    if text.len() > MAX_URL || text.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return None;
    }
    let rest = text
        .strip_prefix("https://")
        .or_else(|| text.strip_prefix("http://"))?;
    let host = rest.split(['/', '?', '#']).next()?;
    // No credentials in a published address, and a host that could be a domain.
    (!host.is_empty() && !host.contains('@') && host.contains('.')).then(|| text.to_owned())
}

/// Addresses that name one feed compare equal: scheme, `www.`, case and a trailing slash aside.
fn address_key(url: &str) -> String {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    rest.trim_end_matches('/').to_ascii_lowercase()
}

/// Whether the entry is offered to a reader with these languages. A `locales` that is present
/// but not a list of ranges offers the feed to no one: a gate that cannot be read stays shut.
fn offered(locales: Option<&Value>, preferred: &[String]) -> bool {
    match locales {
        None | Some(Value::Null) => true,
        Some(Value::Array(ranges)) => ranges
            .iter()
            .filter_map(Value::as_str)
            .any(|range| preferred.iter().any(|tag| in_range(range, tag))),
        Some(_) => false,
    }
}

fn entry(
    value: &Value,
    categories: Option<&serde_json::Map<String, Value>>,
    preferred: &[String],
) -> Option<Recommended> {
    let entry = value.as_object()?;
    if !offered(entry.get("locales"), preferred) {
        return None;
    }
    let id = clean(entry.get("id")?.as_str()?, MAX_NAME)?;
    let url = web_address(entry.get("url"))?;
    let title = localized(entry.get("name"), preferred, MAX_NAME)?;
    let category = entry
        .get("category")
        .and_then(Value::as_str)
        .and_then(|id| localized(categories.and_then(|c| c.get(id)), preferred, MAX_NAME));
    Some(Recommended {
        id,
        url,
        title,
        description: localized(entry.get("description"), preferred, MAX_DESCRIPTION)
            .unwrap_or_default(),
        site: web_address(entry.get("site")),
        category,
        starter: entry.get("starter").and_then(Value::as_bool) == Some(true),
    })
}

/// The feeds `json` recommends to a reader with these languages, most preferred first.
///
/// Only a document that is not one at all, or is from a later version, is refused. Everything
/// else degrades an entry at a time: a malformed, duplicate or unreadable entry is skipped.
pub fn parse(json: &[u8], preferred: &[String]) -> Result<Vec<Recommended>, Refused> {
    if json.len() > LIMIT {
        return Err(Refused::TooLarge);
    }
    let json = json.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(json);
    let document: Value =
        serde_json::from_slice(json).map_err(|e| Refused::NotJson(e.to_string()))?;
    let version = document.get("version").and_then(Value::as_u64);
    let feeds = document.get("feeds").and_then(Value::as_array);
    let (Some(version), Some(feeds)) = (version, feeds) else {
        return Err(Refused::NotADocument);
    };
    if version > VERSION {
        return Err(Refused::Version(version));
    }
    let categories = document.get("categories").and_then(Value::as_object);
    let mut out = Vec::<Recommended>::new();
    for value in feeds {
        let Some(feed) = entry(value, categories, preferred) else {
            continue;
        };
        let key = address_key(&feed.url);
        if out
            .iter()
            .any(|f| f.id == feed.id || address_key(&f.url) == key)
        {
            continue;
        }
        out.push(feed);
        if out.len() == MAX_FEEDS {
            break;
        }
    }
    Ok(out)
}

/// The recommendations for this reader, shared by every window.
#[derive(Clone, Copy)]
pub struct Recommendations {
    pub feeds: Signal<Vec<Recommended>>,
}
impl Ambient for Recommendations {
    fn create() -> Self {
        Self {
            feeds: Signal::global(Vec::new()),
        }
    }
}

/// The app's language, then every language the device lists.
fn preferred() -> Vec<String> {
    let mut tags = vec![day::locale().get()];
    for tag in day::launch_locales() {
        if !tags.contains(&tag) {
            tags.push(tag);
        }
    }
    tags
}

fn bundled() -> Option<Vec<u8>> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        day::resource(crate::res::assets::recommended_feeds_json).map(|r| r.as_slice().to_vec())
    }
    // The browser has no synchronous resource opener; the published list is same-origin there.
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
}

fn apply(json: &[u8]) -> Result<(), Refused> {
    let feeds = parse(json, &preferred())?;
    Recommendations::app().feeds.set_if_changed(feeds);
    Ok(())
}

/// The list this install last saw, or the bundled one before its first fetch.
fn apply_known() {
    let cached = day::prefs::get(CACHE).map(String::into_bytes);
    if let Some(json) = cached
        && apply(&json).is_ok()
    {
        return;
    }
    if let Some(json) = bundled()
        && let Err(e) = apply(&json)
    {
        day::warn!("Bundled recommended feeds refused: {e}");
    }
}

/// Offer the known recommendations now, refresh them from the website, and follow the app's
/// language.
pub fn start() {
    watch(|| day::locale().get(), |_, _| apply_known());
    apply_known();
    // A scripted run stays off the network: its HTTP is simulated and its screens are compared.
    if ["DAY_NEWS_HTTP_TEST", "DAY_NEWS_SYNC_TEST"]
        .iter()
        .any(|name| day::env(name).as_deref() == Some("1"))
    {
        return;
    }
    day::task(async {
        let response =
            match day_part_http::fetch_future(day_part_http::Request::get(PUBLISHED)).await {
                Ok(response) if response.status == 200 => response,
                Ok(response) => {
                    return day::warn!("Recommended feeds not refreshed: HTTP {}", response.status);
                }
                Err(_) => return day::warn!("Recommended feeds not refreshed: no response"),
            };
        match apply(&response.body) {
            // Cached only once it has been read, so a bad deployment never replaces a good list.
            Ok(()) => {
                if let Ok(text) = String::from_utf8(response.body) {
                    day::prefs::set(CACHE, &text);
                }
            }
            Err(e) => day::warn!("Published recommended feeds ignored: {e}"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(tags: &[&str]) -> Vec<String> {
        tags.iter().map(|t| (*t).to_owned()).collect()
    }
    fn ids(json: &str, preferred: &[&str]) -> Vec<String> {
        parse(json.as_bytes(), &tags(preferred))
            .unwrap()
            .into_iter()
            .map(|f| f.id)
            .collect()
    }

    const LIST: &str = r#"{"version":1,"later":{"anything":[1,2]},
      "categories":{"news":{"en":"News","fr":"Actualités"},"broken":7},
      "feeds":[
        {"id":"fr","url":"https://fr.test/rss","name":"Le Test","category":"news","locales":["fr"],
         "description":{"en":"French daily.","fr":"Quotidien français."},"starter":true},
        {"id":"gb","url":"https://gb.test/rss","name":"UK Test","locales":["en-GB"]},
        {"id":"tw","url":"https://tw.test/rss","name":{"zh-Hant":"測試","en":"TW Test"},"locales":["zh-TW"]},
        {"id":"all","url":"https://all.test/feed","name":"Everyone","category":"broken",
         "description":{"en":"English","pt-BR":"Brasil"},"site":"https://all.test","starter":"yes"}
      ]}"#;

    #[test]
    fn feeds_are_offered_by_any_preferred_language() {
        assert_eq!(ids(LIST, &["en-US"]), ["all"]);
        assert_eq!(ids(LIST, &["en-US", "fr-CA"]), ["fr", "all"]);
        assert_eq!(ids(LIST, &["en-GB"]), ["gb", "all"]);
        assert_eq!(ids(LIST, &["en_gb"]), ["gb", "all"]);
        // A region range is not satisfied by the bare language or by another region.
        assert_eq!(ids(LIST, &["en", "en-AU"]), ["all"]);
        // The range's subtags may have others between them.
        assert_eq!(ids(LIST, &["zh-Hant-TW"]), ["tw", "all"]);
        assert_eq!(ids(LIST, &["zh-Hans-CN"]), ["all"]);
        // A range is whole subtags, not a prefix.
        assert_eq!(ids(LIST, &["frr"]), ["all"]);
        assert_eq!(ids(LIST, &[]), ["all"]);
    }

    #[test]
    fn ranges_follow_extended_filtering() {
        assert!(in_range("de", "de-DE"));
        assert!(in_range("DE-de", "de-Latn-DE"));
        assert!(in_range("*", "ja"));
        assert!(in_range("*-CH", "de-CH"));
        assert!(in_range("de-*-DE", "de-DE"));
        assert!(!in_range("de-DE", "de"));
        assert!(!in_range("de-DE", "de-AT"));
        // A private-use or extension subtag is not a region.
        assert!(!in_range("de-DE", "de-x-DE"));
        assert!(!in_range("", "de"));
        assert!(!in_range("de", ""));
    }

    #[test]
    fn text_resolves_in_preference_order_with_fallbacks() {
        let first = |preferred: &[&str]| parse(LIST.as_bytes(), &tags(preferred)).unwrap();
        let french = first(&["fr-FR", "en"]);
        assert_eq!(french[0].description, "Quotidien français.");
        assert_eq!(french[0].category.as_deref(), Some("Actualités"));
        assert!(french[0].starter);
        let english = first(&["en", "fr"]);
        assert_eq!(english[0].description, "French daily.");
        assert_eq!(english[0].category.as_deref(), Some("News"));
        let all = |preferred: &[&str]| first(preferred).pop().unwrap();
        // Another region of the same language is closer than English.
        assert_eq!(all(&["pt-PT"]).description, "Brasil");
        assert_eq!(all(&["pt"]).description, "Brasil");
        assert_eq!(all(&["de"]).description, "English");
        // A category whose name cannot be read is no category, and a non-boolean is not true.
        assert_eq!(all(&["en"]).category, None);
        assert!(!all(&["en"]).starter);
        assert_eq!(all(&["en"]).site.as_deref(), Some("https://all.test"));
        assert_eq!(first(&["zh-TW"])[0].title, "測試");
        assert_eq!(first(&["zh-TW", "en"])[0].title, "測試");
    }

    #[test]
    fn a_bad_entry_costs_only_itself() {
        let list = r#"{"version":1,"feeds":[
            7, null, "https://string.test/feed", [],
            {"id":"no-url","name":"No address"},
            {"id":"file","url":"file:///etc/passwd","name":"Not a web address"},
            {"id":"script","url":"javascript:alert(1)","name":"Not a web address"},
            {"id":"auth","url":"https://user:secret@host.test/feed","name":"Credentials"},
            {"id":"space","url":"https://host.test/a b","name":"Whitespace"},
            {"id":"bare","url":"https://localhost/feed","name":"No domain"},
            {"id":"no-name","url":"https://noname.test/feed"},
            {"id":"blank-name","url":"https://blank.test/feed","name":"  \n "},
            {"id":"","url":"https://noid.test/feed","name":"No id"},
            {"id":7,"url":"https://numid.test/feed","name":"Numeric id"},
            {"id":"typed","url":"https://typed.test/feed","name":["a list"]},
            {"id":"gate","url":"https://gate.test/feed","name":"Unreadable gate","locales":"en"},
            {"id":"good","url":"https://Good.test/feed/","name":" A  good\tfeed ","site":"ftp://x",
             "description":7,"locales":null,"unknown":{"deep":[true]}},
            {"id":"good","url":"https://other.test/feed","name":"Duplicate id"},
            {"id":"same","url":"http://www.good.test/feed","name":"Duplicate address"},
            {"id":"mixed","url":"https://mixed.test/feed","name":{"en":3,"fr":"Mixte"},
             "locales":[5,"en"]}
        ]}"#;
        let feeds = parse(list.as_bytes(), &tags(&["en-US"])).unwrap();
        assert_eq!(
            feeds.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
            ["good", "mixed"]
        );
        assert_eq!(feeds[0].title, "A good feed");
        assert_eq!(feeds[0].description, "");
        assert_eq!(feeds[0].site, None);
        assert_eq!(feeds[1].title, "Mixte");
    }

    #[test]
    fn only_a_document_that_is_not_one_is_refused() {
        let refused = |json: &[u8]| parse(json, &[]).unwrap_err();
        assert_eq!(refused(br#"{"version":2,"feeds":[]}"#), Refused::Version(2));
        assert_eq!(refused(br#"{"feeds":[]}"#), Refused::NotADocument);
        assert_eq!(
            refused(br#"{"version":"1","feeds":[]}"#),
            Refused::NotADocument
        );
        assert_eq!(
            refused(br#"{"version":1,"feeds":{}}"#),
            Refused::NotADocument
        );
        assert_eq!(refused(b"[]"), Refused::NotADocument);
        assert!(matches!(refused(b"<html>"), Refused::NotJson(_)));
        assert!(matches!(refused(b""), Refused::NotJson(_)));
        assert!(matches!(refused(b"\xff\xfe"), Refused::NotJson(_)));
        assert_eq!(refused(&vec![b' '; LIMIT + 1]), Refused::TooLarge);
        // A byte-order mark and an empty list are both fine.
        assert_eq!(
            parse(b"\xEF\xBB\xBF{\"version\":1,\"feeds\":[]}", &[]),
            Ok(Vec::new())
        );
    }

    #[test]
    fn long_text_and_long_lists_are_bounded() {
        let name = "n".repeat(MAX_NAME * 3);
        let feeds: Vec<String> = (0..MAX_FEEDS + 50)
            .map(|i| format!(r#"{{"id":"f{i}","url":"https://f{i}.test/feed","name":"{name}"}}"#))
            .collect();
        let list = format!(r#"{{"version":1,"feeds":[{}]}}"#, feeds.join(","));
        let parsed = parse(list.as_bytes(), &[]).unwrap();
        assert_eq!(parsed.len(), MAX_FEEDS);
        assert_eq!(parsed[0].title.chars().count(), MAX_NAME);
    }

    #[test]
    fn bundled_list_is_the_published_one() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let bundled = std::fs::read(root.join("resource/assets/recommended-feeds.json")).unwrap();
        let published = std::fs::read(root.join("website/public/recommended-feeds.json")).unwrap();
        assert!(
            bundled == published,
            "copy website/public/recommended-feeds.json to resource/assets/recommended-feeds.json"
        );
        // Every entry is one this build reads, whatever the reader's languages.
        let document: Value = serde_json::from_slice(&bundled).unwrap();
        let entries = document["feeds"].as_array().unwrap();
        let every_range: Vec<String> = entries
            .iter()
            .flat_map(|e| e["locales"].as_array().cloned().unwrap_or_default())
            .filter_map(|r| r.as_str().map(str::to_owned))
            .collect();
        let everyone = parse(&bundled, &every_range).unwrap();
        assert_eq!(everyone.len(), entries.len());
        assert!(everyone.iter().all(|f| !f.description.is_empty()));
        assert!(everyone.iter().all(|f| f.category.is_some()));
        // English alone is offered something to start with, and French is offered more.
        let english = parse(&bundled, &tags(&["en-US"])).unwrap();
        assert!(english.iter().any(|f| f.starter));
        let french = parse(&bundled, &tags(&["fr-FR", "en-US"])).unwrap();
        assert!(french.len() > english.len());
    }
}

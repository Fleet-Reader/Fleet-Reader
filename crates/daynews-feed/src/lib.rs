// SPDX-License-Identifier: Apache-2.0
//! Fetching and parsing syndication feeds, normalized to the shape the store keeps.
//!
//! [`parse`] is pure and offline-testable; [`fetch`] adds the network on top of it. Policy that
//! the app depends on (which field becomes the title, how an article's stable identity is
//! derived, which body wins) lives here rather than being spread through the UI.

pub mod discovery;
pub use day_part_http::HttpError;

/// A feed as we store it: channel metadata plus its current items.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ParsedFeed {
    pub title: Option<String>,
    pub site_url: Option<String>,
    /// Explicit self link, useful when a browser opens a downloaded feed document.
    pub source_url: Option<String>,
    pub description: Option<String>,
    pub icon_url: Option<String>,
    pub items: Vec<ParsedItem>,
}

/// One article. `guid` is the stable identity used to recognize an item across refreshes.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedItem {
    pub guid: String,
    pub title: Option<String>,
    pub url: Option<String>,
    pub author: Option<String>,
    /// Unix seconds. `None` when the feed omits a date (some do); the store falls back to
    /// first-seen time so ordering stays stable.
    pub published: Option<i64>,
    pub summary: Option<String>,
    /// The richest body the feed offered, as HTML.
    pub content_html: Option<String>,
}

impl ParsedItem {
    /// A name that is never empty. Microblog feeds (Mastodon, and anything else posting short
    /// updates) ship items with no `<title>` at all, only a body, so readers show the start of
    /// the content instead. Falls back further to the link, then the id.
    pub fn display_title(&self) -> String {
        if let Some(t) = self
            .title
            .as_ref()
            .map(|t| t.trim())
            .filter(|t| !t.is_empty())
        {
            return t.to_string();
        }
        let body = self
            .summary
            .as_deref()
            .or(self.content_html.as_deref())
            .unwrap_or("");
        let text = clean(body);
        if !text.is_empty() {
            return truncate_on_word(&text, 120);
        }
        self.url.clone().unwrap_or_else(|| self.guid.clone())
    }
}

/// Cut to at most `max` chars, preferring a word boundary, with an ellipsis when shortened.
fn truncate_on_word(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    let cut = match cut.rsplit_once(' ') {
        // Only honour the word boundary if it does not throw away most of the text.
        Some((head, _)) if head.chars().count() >= max / 2 => head,
        _ => cut.trim_end(),
    };
    format!("{}…", cut.trim_end_matches([',', '.', ';', ':', ' ']))
}

#[derive(Debug)]
pub enum FeedError {
    Http(HttpError),
    /// A non-2xx response; the feed may have moved or be gone.
    Status(u16),
    /// A busy origin asked us not to poll again until this Unix timestamp.
    RetryAfter {
        status: u16,
        until: i64,
    },
    /// The bytes did not parse as any syndication format we understand.
    Parse(String),
}

impl std::fmt::Display for FeedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FeedError::Http(e) => write!(f, "network error: {e:?}"),
            FeedError::Status(s) | FeedError::RetryAfter { status: s, .. } => {
                write!(f, "server returned HTTP {s}")
            }
            FeedError::Parse(e) => write!(f, "could not parse feed: {e}"),
        }
    }
}

impl std::error::Error for FeedError {}

/// Parse feed bytes. `base_url` is the feed's own URL, used to resolve relative links and as
/// the last resort for deriving an item id.
pub fn parse(bytes: &[u8], base_url: &str) -> Result<ParsedFeed, FeedError> {
    let parsed = feed_rs::parser::Builder::new()
        .base_uri(Some(base_url))
        // Feed HTML is untrusted and we hand it to a native web view, so strip scripts and
        // event handlers at the parse boundary rather than trusting the renderer.
        .sanitize_content(true)
        .build()
        .parse(bytes)
        .map_err(|e| FeedError::Parse(e.to_string()))?;
    Ok(normalize(parsed, base_url))
}

/// Fetch and parse. Sends a browser-ish `Accept` because some hosts serve HTML to unknown
/// clients, and follows the part's platform-native redirect handling.
pub async fn fetch(url: &str) -> Result<ParsedFeed, FeedError> {
    fetch_with_base(url, url).await
}

/// [`fetch`] with the parse base split from the fetch target, for a caller whose fetch URL
/// is not a usable base (the web build fetches bundled `asset:` feeds as relative same-origin
/// URLs, and URL resolution inside the parser needs an absolute base).
pub async fn fetch_with_base(fetch_url: &str, base_url: &str) -> Result<ParsedFeed, FeedError> {
    match fetch_conditional_with_base(
        fetch_url,
        base_url,
        &CacheValidators::default(),
        None,
        |_, _| {},
    )
    .await?
    {
        FeedUpdate::Modified(feed, _, _) => Ok(feed),
        FeedUpdate::NotModified(_) => Err(FeedError::Status(304)),
    }
}

pub use day_part_http::CacheValidators;

#[derive(Debug)]
pub enum FeedUpdate {
    Modified(ParsedFeed, CacheValidators, Option<String>),
    NotModified(CacheValidators),
}

/// Revalidate the last stored representation, preserving its validators on 304. Callers must
/// only persist new validators after the corresponding parsed articles are successfully stored.
pub async fn fetch_conditional(
    url: &str,
    validators: &CacheValidators,
) -> Result<FeedUpdate, FeedError> {
    fetch_conditional_with_base(url, url, validators, None, |_, _| {}).await
}

pub async fn fetch_conditional_with_progress(
    url: &str,
    validators: &CacheValidators,
    progress: impl FnMut(u64, Option<u64>),
) -> Result<FeedUpdate, FeedError> {
    fetch_conditional_with_base(url, url, validators, None, progress).await
}

/// Revalidate a persisted representation and skip parsing byte-identical 200 responses.
/// Store the returned fingerprint atomically with the articles and validators. Validators
/// from an unchanged body still replace the old values, including clearing absent headers.
pub async fn fetch_cached_with_progress(
    url: &str,
    validators: &CacheValidators,
    fingerprint: Option<&str>,
    progress: impl FnMut(u64, Option<u64>),
) -> Result<FeedUpdate, FeedError> {
    fetch_conditional_with_base(url, url, validators, fingerprint, progress).await
}

async fn fetch_conditional_with_base(
    fetch_url: &str,
    base_url: &str,
    validators: &CacheValidators,
    fingerprint: Option<&str>,
    progress: impl FnMut(u64, Option<u64>),
) -> Result<FeedUpdate, FeedError> {
    let req = day_part_http::Request::get(fetch_url)
        .header(
            "Accept",
            "application/atom+xml, application/rss+xml, application/xml;q=0.9, */*;q=0.8",
        )
        .header("User-Agent", USER_AGENT)
        .conditional(validators)
        .timeout(std::time::Duration::from_secs(25))
        .timeout_total(std::time::Duration::from_secs(30));
    // One shared native client reuses connections. Bound bytes even for chunked responses.
    let response =
        day_part_http::fetch_limited_with_progress_future(req, 16 * 1024 * 1024, progress)
            .await
            .map_err(FeedError::Http)?;
    if response.status != 200 {
        return process_response(response, base_url, validators, fingerprint);
    }
    let validators = validators.clone();
    let fingerprint = fingerprint.map(str::to_owned);
    let base = base_url.to_string();
    // Feed parsing and HTML sanitization must not stall the reader while several feeds finish.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (tx, rx) = day_async::oneshot();
        std::thread::Builder::new()
            .name("feed-parse".into())
            .spawn(move || {
                tx.send(process_response(
                    response,
                    &base,
                    &validators,
                    fingerprint.as_deref(),
                ))
            })
            .map_err(|e| FeedError::Http(HttpError::Io(e.to_string())))?;
        rx.await
            .map_err(|_| FeedError::Http(HttpError::Cancelled))?
    }
    #[cfg(target_arch = "wasm32")]
    {
        process_response(response, &base, &validators, fingerprint.as_deref())
    }
}

fn process_response(
    response: day_part_http::Response,
    base_url: &str,
    validators: &CacheValidators,
    fingerprint: Option<&str>,
) -> Result<FeedUpdate, FeedError> {
    if response.status == 304 {
        return if validators.is_empty() {
            Err(FeedError::Status(304))
        } else {
            Ok(FeedUpdate::NotModified(validators.updated(&response)))
        };
    }
    if matches!(response.status, 429 | 503) {
        let now = (daynews_time::now_epoch_ms() / 1000).min(i64::MAX as u64) as i64;
        if let Some(until) = response
            .header("retry-after")
            .and_then(|value| retry_after(value, now))
        {
            return Err(FeedError::RetryAfter {
                status: response.status,
                until,
            });
        }
    }
    if response.status != 200 {
        return Err(FeedError::Status(response.status));
    }
    let updated = validators.updated(&response);
    use sha2::{Digest, Sha256};
    let hash = format!("{:x}", Sha256::digest(&response.body));
    if fingerprint == Some(hash.as_str()) {
        return Ok(FeedUpdate::NotModified(updated));
    }
    // Keep the subscription URL as the identity base for ID-less entries across redirects.
    let base = base_url;
    Ok(FeedUpdate::Modified(
        parse(&response.body, base)?,
        updated,
        Some(hash),
    ))
}

fn retry_after(value: &str, now: i64) -> Option<i64> {
    let value = value.trim();
    if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
        return value
            .parse::<u64>()
            .ok()
            .map(|seconds| now.saturating_add(seconds.min(i64::MAX as u64) as i64));
    }
    httpdate::parse_http_date(value)
        .ok()
        .and_then(|date| date.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|elapsed| (elapsed.as_secs().min(i64::MAX as u64) as i64).max(now))
}

/// Identifies us to publishers; several block requests with no agent string.
pub const USER_AGENT: &str = concat!(
    "FleetReader/",
    env!("CARGO_PKG_VERSION"),
    " (+https://fleet-reader.github.io/Fleet-Reader)"
);

fn normalize(f: feed_rs::model::Feed, base_url: &str) -> ParsedFeed {
    // The human-facing site: `rel="alternate"` when present, otherwise any link that is not the
    // feed's own address (WordPress lists the feed itself first, which would send readers in a
    // circle when they click "open website").
    let site_url = f
        .links
        .iter()
        .find(|l| l.rel.as_deref() == Some("alternate") && l.href != base_url)
        .or_else(|| {
            f.links
                .iter()
                .find(|l| l.href != base_url && l.rel.as_deref() != Some("self"))
        })
        .map(|l| l.href.clone());
    ParsedFeed {
        title: f.title.map(|t| clean(&t.content)).filter(|s| !s.is_empty()),
        site_url,
        source_url: f
            .links
            .iter()
            .find(|l| l.rel.as_deref() == Some("self"))
            .map(|l| l.href.clone()),
        description: f
            .description
            .map(|t| clean(&t.content))
            .filter(|s| !s.is_empty()),
        icon_url: f.icon.or(f.logo).map(|i| i.uri),
        items: f
            .entries
            .into_iter()
            .map(|e| normalize_entry(e, base_url))
            .collect(),
    }
}

fn normalize_entry(e: feed_rs::model::Entry, base_url: &str) -> ParsedItem {
    let url = e
        .links
        .iter()
        .find(|l| l.rel.as_deref() == Some("alternate"))
        .or_else(|| e.links.first())
        .map(|l| l.href.clone());

    // Body preference mirrors what readers show: the full `content` when present, otherwise the
    // summary. Keeping both lets the list show a snippet while the reader shows the article.
    // RSS 2.0 lets `description` carry HTML, and many feeds put the whole article there with no
    // `content:encoded`; that HTML is the body, because flattened to text the reader ran every
    // paragraph together.
    let content_html = e
        .content
        .as_ref()
        .and_then(|c| c.body.as_deref().map(windows_1252_c1))
        .filter(|b| !b.trim().is_empty())
        .or_else(|| {
            e.summary
                .as_ref()
                .filter(|t| t.content_type.as_str() == "text/html" && has_markup(&t.content))
                .map(|t| windows_1252_c1(&t.content))
        });
    let summary = e
        .summary
        .as_ref()
        .map(|t| clean(&t.content))
        .filter(|s| !s.is_empty());

    ParsedItem {
        // Identity, most stable first: the feed's own id, then the article URL, then a hash of
        // title+date. Without this an item reappears as "new" whenever a publisher rewrites a
        // field, which is the classic feed-reader bug.
        guid: first_non_empty([
            Some(e.id.clone()),
            url.clone(),
            Some(fallback_id(&e, base_url)),
        ]),
        title: e.title.map(|t| clean(&t.content)).filter(|s| !s.is_empty()),
        url,
        author: e
            .authors
            .first()
            .map(|a| a.name.clone())
            .filter(|s| !s.is_empty()),
        published: e
            .published
            .or(e.updated)
            .map(|d| d.timestamp())
            .filter(|t| *t > 0),
        summary,
        content_html,
    }
}

fn first_non_empty<const N: usize>(candidates: [Option<String>; N]) -> String {
    candidates
        .into_iter()
        .flatten()
        .find(|s| !s.trim().is_empty())
        .unwrap_or_default()
}

/// A deterministic id for an entry with neither id nor link: hash what identity it does have.
fn fallback_id(e: &feed_rs::model::Entry, base_url: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |s: &str| {
        for b in s.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
    };
    eat(base_url);
    if let Some(t) = &e.title {
        eat(&t.content);
    }
    if let Some(d) = e.published.or(e.updated) {
        eat(&d.timestamp().to_string());
    } else {
        // Nothing identifying at all: fall back to now, so at least it is not confused with a
        // different item. It will look new once; there is no better answer.
        eat(&now_secs().to_string());
    }
    format!("news:{h:016x}")
}

fn now_secs() -> i64 {
    // `daynews-time` rather than `SystemTime::now()`, which aborts on wasm32; on web this is
    // the page's `Date.now()`.
    daynews_time::now_unix()
}

/// Collapse whitespace and strip tags from a text field. Feeds put HTML in `<title>` more often
/// than they should, and a title with markup in it looks broken in a list.
pub fn clean(s: &str) -> String {
    // Decode first: feeds routinely escape a whole HTML body into a text field, so the tags
    // only become visible after decoding (`&lt;p&gt;` → `<p>`).
    let decoded = decode_entities(s);
    let chars: Vec<char> = decoded.chars().map(c1_char).collect();
    let mut out = String::with_capacity(decoded.len());
    let mut i = 0;
    let mut last_space = true;
    // Whether the field turned out to hold escaped HTML; see the second decode below.
    let mut stripped_a_tag = false;
    while i < chars.len() {
        let c = chars[i];
        // A `<` only opens a tag when a name, a closing slash or a declaration follows; that
        // keeps prose like "a < b" intact.
        if c == '<'
            && chars
                .get(i + 1)
                .is_some_and(|n| n.is_ascii_alphabetic() || *n == '/' || *n == '!')
        {
            match chars[i..].iter().position(|c| *c == '>') {
                Some(end) => {
                    i += end + 1;
                    stripped_a_tag = true;
                    // A stripped tag is a word break, not a join.
                    if !last_space {
                        out.push(' ');
                        last_space = true;
                    }
                    continue;
                }
                None => break, // unterminated tag: drop the remainder
            }
        }
        if c.is_whitespace() {
            if !last_space {
                out.push(' ');
                last_space = true;
            }
        } else {
            out.push(c);
            last_space = false;
        }
        i += 1;
    }
    let out = out.trim();
    // Escaped HTML is escaped twice: once for the markup itself, and again for the entities
    // inside it, so a publisher's `&` arrives as `&amp;amp;`. Finding tags after the first
    // decode proves this field was escaped HTML, which is what makes a second pass safe; text
    // that merely mentions "&amp;" has no tags and is left exactly as written.
    if stripped_a_tag {
        decode_entities(out)
    } else {
        out.to_string()
    }
}

/// The Windows-1252 character a publisher means by a C1 control (U+0080–U+009F). A feed that
/// writes `&#149;` wants a bullet, not a control character: the number is the byte's position
/// in Windows-1252, and browsers read numeric references in that range the same way (the HTML
/// standard says so). The XML layer has already turned the reference into the raw control by
/// the time text reaches this crate, so the remap is on characters, not entities. Left alone,
/// the control draws as a box on Android and as nothing on Apple platforms. The five positions
/// Windows-1252 leaves undefined stay as they are, as in browsers.
fn c1_char(c: char) -> char {
    const WINDOWS_1252: [char; 32] = [
        '\u{20AC}', '\u{81}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{8D}',
        '\u{017D}', '\u{8F}', '\u{90}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}',
        '\u{2013}', '\u{2014}', '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}',
        '\u{9D}', '\u{017E}', '\u{0178}',
    ];
    match c as u32 {
        n @ 0x80..=0x9F => WINDOWS_1252[(n - 0x80) as usize],
        _ => c,
    }
}

/// [`c1_char`] over a whole body, which goes to the web view undecoded and so needs the same
/// fix applied to its characters.
fn windows_1252_c1(s: &str) -> String {
    s.chars().map(c1_char).collect()
}

/// Whether decoded text holds real markup: a `<` that opens a tag name or a closing tag, the same
/// test [`clean`] strips by, so prose like "2 < 7" does not count.
fn has_markup(s: &str) -> bool {
    s.char_indices().any(|(i, c)| {
        c == '<'
            && s[i + 1..]
                .chars()
                .next()
                .is_some_and(|n| n.is_ascii_alphabetic() || n == '/')
            && s[i..].contains('>')
    })
}

/// The named entities a reader meets in feed text, plus the numeric forms.
///
/// Publishers escape typographic punctuation constantly (`&rsquo;`, `&ldquo;`, `&mdash;`), and
/// summaries are shown as plain text in the timeline; nothing downstream will decode them, so
/// an undecoded entity is visible to the user as literal `&rsquo;`. Bodies are different: they
/// go to a web view, which decodes the full HTML set itself.
const NAMED_ENTITIES: &[(&str, char)] = &[
    ("amp", '&'),
    ("lt", '<'),
    ("gt", '>'),
    ("quot", '"'),
    ("apos", '\''),
    ("nbsp", ' '),
    ("ensp", ' '),
    ("emsp", ' '),
    ("thinsp", ' '),
    ("rsquo", '\u{2019}'),
    ("lsquo", '\u{2018}'),
    ("rdquo", '\u{201D}'),
    ("ldquo", '\u{201C}'),
    ("sbquo", '\u{201A}'),
    ("bdquo", '\u{201E}'),
    ("prime", '\u{2032}'),
    ("Prime", '\u{2033}'),
    ("mdash", '\u{2014}'),
    ("ndash", '\u{2013}'),
    ("minus", '\u{2212}'),
    ("hyphen", '\u{2010}'),
    ("hellip", '\u{2026}'),
    ("bull", '\u{2022}'),
    ("middot", '\u{00B7}'),
    ("copy", '\u{00A9}'),
    ("reg", '\u{00AE}'),
    ("trade", '\u{2122}'),
    ("deg", '\u{00B0}'),
    ("laquo", '\u{00AB}'),
    ("raquo", '\u{00BB}'),
    ("times", '\u{00D7}'),
    ("divide", '\u{00F7}'),
    ("frac12", '\u{00BD}'),
    ("frac14", '\u{00BC}'),
    ("euro", '\u{20AC}'),
    ("pound", '\u{00A3}'),
    ("yen", '\u{00A5}'),
    ("cent", '\u{00A2}'),
    ("sect", '\u{00A7}'),
    ("para", '\u{00B6}'),
    ("dagger", '\u{2020}'),
    ("Dagger", '\u{2021}'),
    ("permil", '\u{2030}'),
    ("larr", '\u{2190}'),
    ("rarr", '\u{2192}'),
    ("harr", '\u{2194}'),
    ("darr", '\u{2193}'),
    ("uarr", '\u{2191}'),
    ("ne", '\u{2260}'),
    ("le", '\u{2264}'),
    ("ge", '\u{2265}'),
];

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';').filter(|e| *e <= 9) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..end];
        let decoded = match ent {
            e if e.starts_with("#x") || e.starts_with("#X") => u32::from_str_radix(&e[2..], 16)
                .ok()
                .and_then(char::from_u32)
                .map(c1_char),
            e if e.starts_with('#') => e[1..].parse().ok().and_then(char::from_u32).map(c1_char),
            e => NAMED_ENTITIES
                .iter()
                .find(|(name, _)| *name == e)
                .map(|(_, c)| *c),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod refresh_tests {
    use super::*;
    use day_part_http::Response;

    #[test]
    fn unchanged_response_needs_a_cached_representation_and_never_parses() {
        let validators = CacheValidators {
            etag: Some("W/\"one\"".into()),
            last_modified: Some("Wed, 30 Sep 2026 10:00:00 GMT".into()),
        };
        let response = || Response::new(304, vec![], b"not a feed".to_vec());
        assert!(matches!(
            process_response(
                response(),
                "https://fixture.example/feed",
                &CacheValidators::default(),
                None
            ),
            Err(FeedError::Status(304))
        ));
        match process_response(
            response(),
            "https://fixture.example/feed",
            &validators,
            None,
        )
        .unwrap()
        {
            FeedUpdate::NotModified(next) => assert_eq!(next, validators),
            _ => panic!("304 must preserve the stored representation"),
        }
    }

    #[test]
    fn retry_after_accepts_seconds_and_dates_but_not_invalid_values() {
        let now = 1_000;
        assert_eq!(retry_after("120", now), Some(1_120));
        assert_eq!(retry_after("0", now), Some(now));
        assert_eq!(
            retry_after("Thu, 01 Jan 1970 00:20:00 GMT", now),
            Some(1_200)
        );
        assert_eq!(retry_after("Thu, 01 Jan 1970 00:00:00 GMT", now), Some(now));
        for value in ["-1", "+120", "1.5", "tomorrow", ""] {
            assert_eq!(retry_after(value, now), None);
        }
        let error = process_response(
            Response::new(429, vec![("Retry-After".into(), "120".into())], vec![]),
            "https://fixture.example/feed",
            &CacheValidators::default(),
            None,
        )
        .unwrap_err();
        assert!(matches!(error, FeedError::RetryAfter { status: 429, .. }));
    }

    #[test]
    fn identical_200_skips_import_but_rotates_and_clears_validators() {
        let url = "https://fixture.example/feed";
        let body =
            br#"{"version":"https://jsonfeed.org/version/1.1","title":"Fixture","items":[]}"#;
        let old = CacheValidators {
            etag: Some("W/\"old\"".into()),
            last_modified: Some("Wed, 30 Sep 2026 10:00:00 GMT".into()),
        };
        let first =
            process_response(Response::new(200, vec![], body.to_vec()), url, &old, None).unwrap();
        let FeedUpdate::Modified(_, _, Some(hash)) = first else {
            panic!("must import first body")
        };
        let response = Response::new(
            200,
            vec![
                ("ETag".into(), "W/\"new\"".into()),
                (
                    "Last-Modified".into(),
                    "Thu, 01 Oct 2026 10:00:00 GMT".into(),
                ),
            ],
            body.to_vec(),
        );
        let FeedUpdate::NotModified(next) =
            process_response(response, url, &old, Some(&hash)).unwrap()
        else {
            panic!("identical 200 must not parse or import")
        };
        assert_eq!(next.etag.as_deref(), Some("W/\"new\""));
        assert_eq!(
            next.last_modified.as_deref(),
            Some("Thu, 01 Oct 2026 10:00:00 GMT")
        );
        let request = day_part_http::Request::get(url).conditional(&next);
        assert!(
            request
                .headers()
                .iter()
                .any(|(key, value)| key == "If-None-Match" && value == "W/\"new\"")
        );
        let FeedUpdate::NotModified(cleared) = process_response(
            Response::new(200, vec![], body.to_vec()),
            url,
            &next,
            Some(&hash),
        )
        .unwrap() else {
            panic!("identical body must still update validators")
        };
        assert!(cleared.is_empty());
        // Content changes must import even when the origin incorrectly reuses its ETag.
        let changed = br#"{"version":"https://jsonfeed.org/version/1.1","title":"Changed fixture","items":[]}"#;
        assert!(matches!(
            process_response(
                Response::new(200, vec![], changed.to_vec()),
                url,
                &next,
                Some(&hash)
            )
            .unwrap(),
            FeedUpdate::Modified(_, _, Some(_))
        ));
    }

    #[test]
    fn only_a_parsed_success_can_replace_validators() {
        let previous = CacheValidators {
            etag: Some("\"old\"".into()),
            last_modified: None,
        };
        let json =
            br#"{"version":"https://jsonfeed.org/version/1.1","title":"Fixture","items":[]}"#;
        let response = Response::new(200, vec![], json.to_vec());
        match process_response(response, "https://fixture.example/feed", &previous, None).unwrap() {
            FeedUpdate::Modified(feed, next, _) => {
                assert_eq!(feed.title.as_deref(), Some("Fixture"));
                assert!(next.is_empty());
            }
            _ => panic!("200 must parse the new representation"),
        }
        assert!(
            process_response(
                Response::new(200, vec![], b"invalid".to_vec()),
                "https://fixture.example/feed",
                &previous,
                None
            )
            .is_err()
        );
        assert!(matches!(
            process_response(
                Response::new(503, vec![], vec![]),
                "https://fixture.example/feed",
                &previous,
                None
            ),
            Err(FeedError::Status(503))
        ));
    }
}

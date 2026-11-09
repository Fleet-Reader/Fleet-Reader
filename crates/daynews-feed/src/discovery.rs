// SPDX-License-Identifier: Apache-2.0
//! Feed discovery from a fetched representation. Pure candidate extraction is separately testable.
use crate::{CacheValidators, FeedError, FeedUpdate};
use day_part_http::{Request, Response};
use html5ever::{
    tendril::StrTendril,
    tokenizer::{
        BufferQueue, StartTag, Token, TokenSink, TokenSinkResult, Tokenizer, states::RawKind,
    },
};
use std::cell::RefCell;
use url::Url;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub url: String,
    pub title: Option<String>,
}
pub enum Discovery {
    Feed {
        url: String,
        update: Box<FeedUpdate>,
    },
    Candidates(Vec<Candidate>),
}
/// Only absolute HTTP(S) URLs without embedded credentials are subscription input.
pub fn web_url(value: &str) -> Option<Url> {
    Url::parse(value.trim()).ok().filter(|u| {
        matches!(u.scheme(), "http" | "https")
            && u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
    })
}
pub async fn discover(url: &str) -> Result<Discovery, FeedError> {
    let url = web_url(url).ok_or_else(|| FeedError::Parse("invalid subscription URL".into()))?;
    let response = day_part_http::fetch_limited_future(Request::get(url.as_str()).header("Accept", "application/atom+xml, application/rss+xml, application/feed+json, application/xml, text/xml, text/html;q=0.9, */*;q=0.1").timeout(std::time::Duration::from_secs(25)).timeout_total(std::time::Duration::from_secs(30)), 16 * 1024 * 1024).await.map_err(FeedError::Http)?;
    let requested = url.to_string();
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (tx, rx) = day_async::oneshot();
        std::thread::Builder::new()
            .name("feed-discovery".into())
            .spawn(move || {
                tx.send(discover_response(response, &requested));
            })
            .map_err(|e| FeedError::Parse(e.to_string()))?;
        rx.await
            .map_err(|_| FeedError::Parse("discovery worker stopped".into()))?
    }
    #[cfg(target_arch = "wasm32")]
    discover_response(response, &requested)
}
fn discover_response(response: Response, requested: &str) -> Result<Discovery, FeedError> {
    let final_url = if response.url.is_empty() {
        requested.to_owned()
    } else {
        response.url.clone()
    };
    // Feed parsing is authoritative, even for origins with an incorrect Content-Type.
    if !(200..300).contains(&response.status) {
        return Err(FeedError::Status(response.status));
    }
    if let Ok(parsed) = crate::parse(&response.body, &final_url) {
        use sha2::{Digest, Sha256};
        let hash = format!("{:x}", Sha256::digest(&response.body));
        let update = FeedUpdate::Modified(
            parsed,
            CacheValidators::default().updated(&response),
            Some(hash),
        );
        return Ok(Discovery::Feed {
            url: final_url,
            update: Box::new(update),
        });
    }
    Ok(Discovery::Candidates(candidates(&response, &final_url)))
}
fn feed_type(value: &str) -> bool {
    matches!(
        value
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "application/rss+xml"
            | "application/atom+xml"
            | "application/feed+json"
            | "application/rdf+xml"
    )
}
fn feed_path(value: &str) -> bool {
    let value = value
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .trim_end_matches('/')
        .to_ascii_lowercase();
    value.ends_with(".rss")
        || value.ends_with(".atom")
        || value.ends_with("/feed")
        || value.ends_with("/rss")
        || value.ends_with("/atom")
        || value.ends_with("feed.xml")
        || value.ends_with("rss.xml")
        || value.ends_with("atom.xml")
        || value.ends_with("feed.json")
}
#[derive(Default)]
struct Links {
    base: Option<String>,
    links: Vec<(String, Option<String>)>,
}
#[derive(Default)]
struct Sink(RefCell<Links>);
impl TokenSink for Sink {
    type Handle = ();
    fn process_token(&self, token: Token, _: u64) -> TokenSinkResult<()> {
        let Token::TagToken(tag) = token else {
            return TokenSinkResult::Continue;
        };
        if tag.kind != StartTag {
            return TokenSinkResult::Continue;
        }
        match tag.name.as_ref() {
            "script" => return TokenSinkResult::RawData(RawKind::ScriptData),
            "style" => return TokenSinkResult::RawData(RawKind::Rawtext),
            "title" | "textarea" => return TokenSinkResult::RawData(RawKind::Rcdata),
            _ => {}
        }
        let attr = |key: &str| {
            tag.attrs
                .iter()
                .find(|a| a.name.local.as_ref() == key)
                .map(|a| a.value.to_string())
                .unwrap_or_default()
        };
        let mut links = self.0.borrow_mut();
        let href = attr("href");
        match tag.name.as_ref() {
            "base" if links.base.is_none() && !href.is_empty() => links.base = Some(href),
            "link" | "a" if !href.is_empty() => {
                let rel = attr("rel").to_ascii_lowercase();
                if (feed_type(&attr("type"))
                    && (tag.name.as_ref() == "a"
                        || rel.split_ascii_whitespace().any(|r| r == "alternate"))
                    || tag.name.as_ref() == "a" && feed_path(&href))
                    && links.links.len() < 128
                {
                    links.links.push((href, nonempty(attr("title"))));
                }
            }
            "meta" => {
                let name = attr("name").to_ascii_lowercase();
                let property = attr("property").to_ascii_lowercase();
                if matches!(
                    name.as_str(),
                    "rss" | "atom" | "feed" | "rss-url" | "feed-url"
                ) || matches!(property.as_str(), "rss" | "atom" | "feed")
                {
                    let url = attr("content");
                    if !url.is_empty() && links.links.len() < 128 {
                        links.links.push((url, None));
                    }
                }
            }
            _ => {}
        }
        TokenSinkResult::Continue
    }
}
fn nonempty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}
/// Split RFC 8288 link-values without splitting commas in URLs or quoted titles.
fn link_values(value: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut angle = false;
    let mut escaped = false;
    for (i, c) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if quoted && c == '\\' {
            escaped = true;
            continue;
        }
        match c {
            '"' if !angle => quoted = !quoted,
            '<' if !quoted => angle = true,
            '>' if !quoted => angle = false,
            ',' if !quoted && !angle => {
                out.push(&value[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&value[start..]);
    out
}
pub fn candidates(response: &Response, page_url: &str) -> Vec<Candidate> {
    let Some(page) = web_url(page_url) else {
        return vec![];
    };
    let input = BufferQueue::default();
    input.push_back(StrTendril::from(
        String::from_utf8_lossy(&response.body).as_ref(),
    ));
    let tokenizer = Tokenizer::new(Sink::default(), Default::default());
    let _ = tokenizer.feed(&input);
    tokenizer.end();
    let mut links = tokenizer.sink.0.into_inner();
    let base = links
        .base
        .as_deref()
        .and_then(|b| page.join(b).ok())
        .filter(|u| web_url(u.as_str()).is_some())
        .unwrap_or(page.clone());
    // HTTP Link targets resolve against the response URL, not the document's <base>.
    let mut declared = Vec::new();
    for (_, header) in response
        .headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("link"))
    {
        for value in link_values(header) {
            let Some((target, params)) = value
                .trim()
                .strip_prefix('<')
                .and_then(|v| v.split_once('>'))
            else {
                continue;
            };
            let mut rel = String::new();
            let mut kind = String::new();
            let mut title = None;
            for param in params.split(';') {
                let Some((k, v)) = param.trim().split_once('=') else {
                    continue;
                };
                let v = v.trim().trim_matches('"');
                match k.trim().to_ascii_lowercase().as_str() {
                    "rel" => rel = v.to_ascii_lowercase(),
                    "type" => kind = v.to_owned(),
                    "title" => title = nonempty(v.to_owned()),
                    _ => {}
                }
            }
            if rel.split_ascii_whitespace().any(|r| r == "alternate")
                && feed_type(&kind)
                && let Ok(url) = page.join(target)
            {
                declared.push((url.to_string(), title));
            }
        }
    }
    declared.extend(
        links
            .links
            .drain(..)
            .filter_map(|(href, title)| base.join(&href).ok().map(|url| (url.to_string(), title))),
    );
    let mut result: Vec<Candidate> = Vec::new();
    for (url, title) in declared {
        let Some(mut url) = web_url(&url) else {
            continue;
        };
        url.set_fragment(None);
        let url = url.to_string();
        if let Some(existing) = result.iter_mut().find(|c| c.url == url) {
            if existing.title.is_none() {
                existing.title = title;
            }
        } else if result.len() < 64 {
            result.push(Candidate { url, title });
        }
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn combines_headers_metadata_and_html_with_base_and_deduplication() {
        let r = Response::new(200, vec![("Link".into(), "</news.rss>; rel=\"alternate\"; type=\"application/rss+xml\"; title=\"News, daily\"".into())], br#"<base href='/blog/'><link rel='alternate' type='application/atom+xml' href='atom.xml' title='Blog'><a href='/news.rss#top'>RSS</a><meta name='feed-url' content='feed.json'><link rel='alternate' type='text/html' href='other'><script>const x="<a href='/fake.rss'>"</script><a href='javascript:fake.rss'>bad</a>"#.to_vec());
        let feeds = candidates(&r, "https://fixture.example/home");
        assert_eq!(feeds.len(), 3);
        assert_eq!(feeds[0].url, "https://fixture.example/news.rss");
        assert_eq!(feeds[0].title.as_deref(), Some("News, daily"));
        assert_eq!(feeds[1].url, "https://fixture.example/blog/atom.xml");
        assert_eq!(feeds[2].url, "https://fixture.example/blog/feed.json");
    }
    #[test]
    fn direct_feed_preserves_response_validators_and_final_url() {
        let mut r=Response::new(200, vec![("ETag".into(),"\"v1\"".into())], br#"<rss version="2.0"><channel><title>Fixture</title><link>https://fixture.example/</link><description>Fixture</description></channel></rss>"#.to_vec());
        r.url = "https://fixture.example/real.rss".into();
        let Discovery::Feed { url, update } =
            discover_response(r, "https://fixture.example/redirect").unwrap()
        else {
            panic!("feed")
        };
        let FeedUpdate::Modified(_, validators, Some(_)) = *update else {
            panic!("modified feed")
        };
        assert_eq!(url, "https://fixture.example/real.rss");
        assert_eq!(validators.etag.as_deref(), Some("\"v1\""));
    }
    #[test]
    fn missing_candidates_and_bad_status_are_distinct() {
        assert!(
            matches!(discover_response(Response::new(200,vec![],b"<html>No feed</html>".to_vec()),"https://fixture.example"), Ok(Discovery::Candidates(c)) if c.is_empty())
        );
        assert!(matches!(
            discover_response(
                Response::new(404, vec![], vec![]),
                "https://fixture.example"
            ),
            Err(FeedError::Status(404))
        ));
        for url in [
            "javascript:alert(1)",
            "file:///tmp/feed",
            "https://user:pass@fixture.example/",
            "not a URL",
        ] {
            assert!(web_url(url).is_none());
        }
    }
}

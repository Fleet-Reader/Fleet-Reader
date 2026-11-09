// SPDX-License-Identifier: Apache-2.0
//! Publication metadata only: no page scripts, third-party favicon services, or credentials.
use html5ever::tokenizer::{
    BufferQueue, StartTag, Token, TokenSink, TokenSinkResult, Tokenizer, states::RawKind,
};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use url::Url;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub feed: String,
    pub home: Option<String>,
    pub icon: Option<String>,
}

pub fn web_url(value: &str) -> Option<Url> {
    Url::parse(value).ok().filter(|u| {
        matches!(u.scheme(), "http" | "https")
            && u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
    })
}

pub fn resolve(base: &Url, value: &str) -> Option<Url> {
    let mut url = web_url(base.join(value.trim()).ok()?.as_str())?;
    url.set_fragment(None);
    Some(url)
}

impl Source {
    pub fn declared_icon(&self) -> Option<Url> {
        resolve(&web_url(&self.feed)?, self.icon.as_deref()?)
    }

    pub fn pages(&self) -> Vec<Url> {
        let Some(feed) = web_url(&self.feed) else {
            return Vec::new();
        };
        let mut pages = Vec::new();
        if let Some(home) = self.home.as_deref().and_then(|h| resolve(&feed, h)) {
            pages.push(home);
        }
        let root = feed.join("/").expect("web URL has a base");
        if !pages.iter().any(|p| p.origin() == root.origin()) {
            pages.push(root);
        }
        pages
    }
}

#[derive(Default)]
struct Links {
    base: Option<String>,
    icons: Vec<(u32, String)>,
    manifest: Option<String>,
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
        let href = attr("href");
        let mut links = self.0.borrow_mut();
        if tag.name.as_ref() == "base" && links.base.is_none() && !href.is_empty() {
            links.base = Some(href);
        } else if tag.name.as_ref() == "link" && !href.is_empty() {
            let rel = attr("rel").to_ascii_lowercase();
            let rel: Vec<_> = rel.split_ascii_whitespace().collect();
            if rel.contains(&"manifest") && links.manifest.is_none() {
                links.manifest = Some(href.clone());
            }
            let touch =
                rel.contains(&"apple-touch-icon") || rel.contains(&"apple-touch-icon-precomposed");
            if (rel.contains(&"icon") || touch) && links.icons.len() < 32 {
                // Prefer small raster artwork over vector-only or monochrome mask icons.
                if attr("type").eq_ignore_ascii_case("image/svg+xml") {
                    return TokenSinkResult::Continue;
                }
                links.icons.push((
                    size_score(&attr("sizes")).saturating_add(if touch { 20 } else { 0 }),
                    href,
                ));
            }
        }
        TokenSinkResult::Continue
    }
}

fn size_score(sizes: &str) -> u32 {
    sizes
        .split_ascii_whitespace()
        .filter_map(|s| {
            let (w, h) = s.split_once(['x', 'X'])?;
            let (w, h) = (w.parse::<u32>().ok()?, h.parse::<u32>().ok()?);
            (w == h && w > 0).then_some(if w >= 64 { w - 64 } else { 512 - w })
        })
        .min()
        .unwrap_or(600)
}

pub struct PageIcons {
    pub icons: Vec<Url>,
    pub manifest: Option<Url>,
}

pub fn page_icons(html: &str, page: &Url) -> PageIcons {
    let input = BufferQueue::default();
    input.push_back(html.into());
    let tokenizer = Tokenizer::new(Sink::default(), Default::default());
    let _ = tokenizer.feed(&input);
    tokenizer.end();
    let mut links = tokenizer.sink.0.into_inner();
    let base = links
        .base
        .as_deref()
        .and_then(|s| resolve(page, s))
        .unwrap_or_else(|| page.clone());
    links.icons.sort_by_key(|(rank, _)| *rank);
    let mut icons = Vec::new();
    for (_, href) in links.icons {
        if let Some(url) = resolve(&base, &href)
            && !icons.contains(&url)
        {
            icons.push(url);
        }
    }
    icons.truncate(6);
    PageIcons {
        icons,
        manifest: links.manifest.and_then(|m| resolve(&base, &m)),
    }
}

pub fn manifest_icons(bytes: &[u8], manifest: &Url) -> Vec<Url> {
    #[derive(Deserialize)]
    struct Manifest {
        #[serde(default)]
        icons: Vec<ManifestIcon>,
    }
    #[derive(Deserialize)]
    struct ManifestIcon {
        src: String,
        #[serde(default)]
        sizes: String,
    }
    let Ok(mut value) = serde_json::from_slice::<Manifest>(bytes) else {
        return Vec::new();
    };
    value.icons.sort_by_key(|icon| size_score(&icon.sizes));
    value
        .icons
        .iter()
        .filter_map(|icon| resolve(manifest, &icon.src))
        .take(4)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_real_html_links_entities_base_and_ranks_raster_sizes() {
        let page = web_url("https://fixture.example/blog/story").unwrap();
        let links = page_icons(
            r#"<script>let fake = '<link rel="icon" href="bad.png">';</script><BASE href="/art/"><link href='small.png' rel='shortcut ICON' sizes='16x16'><link rel='apple-touch-icon' sizes='180x180' href='touch.png'><link rel='icon' sizes='64x64' href='best.png?a=1&amp;b=2'><link rel='icon' href='javascript:bad()'><link rel='manifest' href='../site.webmanifest'>"#,
            &page,
        );
        assert_eq!(
            links.icons[0].as_str(),
            "https://fixture.example/art/best.png?a=1&b=2"
        );
        assert_eq!(links.icons.len(), 3);
        assert_eq!(
            links.manifest.unwrap().as_str(),
            "https://fixture.example/site.webmanifest"
        );
    }
    #[test]
    fn resolves_home_and_manifest_paths_without_non_web_schemes() {
        let source = Source {
            feed: "https://feeds.fixture.example/rss".into(),
            home: Some("https://fixture.example/blog/".into()),
            icon: Some("/logo.png".into()),
        };
        assert_eq!(source.pages().len(), 2);
        assert_eq!(
            source.declared_icon().unwrap().as_str(),
            "https://feeds.fixture.example/logo.png"
        );
        let icons = manifest_icons(
            br#"{"icons":[{"src":"icons/a.png","sizes":"192x192"},{"src":"file:///bad"}]}"#,
            &web_url("https://fixture.example/app/site.json").unwrap(),
        );
        assert_eq!(icons[0].as_str(), "https://fixture.example/app/icons/a.png");
        assert_eq!(icons.len(), 1);
        assert!(web_url("https://user:password@fixture.example").is_none());
        assert!(web_url("asset:demo/feed.xml").is_none());
    }
}

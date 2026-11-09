// SPDX-License-Identifier: Apache-2.0
//! Replaceable full-article extraction. Providers return publication data, never UI strings.
//! The default fetches HTML with native networking and runs bundled Mozilla Readability in
//! an inert DOM in the reader's JS engine. It never navigates to or runs the publisher page.

use std::{future::Future, pin::Pin, time::Duration};

#[cfg(not(target_arch = "wasm32"))]
use day::prelude::*;
use day_piece_webview::JsHandle;
use serde::Deserialize;

const MAX_HTML_BYTES: usize = 5 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ExtractedArticle {
    pub content: String,
    pub title: Option<String>,
    pub byline: Option<String>,
    pub url: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ExtractionError {
    InvalidUrl,
    Network,
    TooLarge,
    NoContent,
    Unavailable,
    Timeout,
    CustomScript,
}

pub type ExtractionFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ExtractedArticle, ExtractionError>> + 'a>>;

/// An HTTP service or a local/offscreen engine can implement this without changing commands,
/// window state, rendering, or cancellation. Providers must return sanitized, inert HTML.
/// Dropping the future abandons results and cancels work supported by the underlying engine.
pub trait ArticleExtractor {
    fn extract<'a>(&'a self, url: &'a str) -> ExtractionFuture<'a>;
}

pub struct LocalReadability<'a> {
    pub engine: JsHandle,
    pub diagnostics: &'a crate::load_diagnostics::Load,
}

pub fn web_url(value: &str) -> Option<url::Url> {
    url::Url::parse(value).ok().filter(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
    })
}

impl ArticleExtractor for LocalReadability<'_> {
    fn extract<'a>(&'a self, url: &'a str) -> ExtractionFuture<'a> {
        Box::pin(async move {
            let url = web_url(url).ok_or(ExtractionError::InvalidUrl)?;
            // Follow each hop ourselves so cookie eligibility is recalculated for its URL;
            // credentials must never be forwarded blindly to a redirect destination.
            let client = day_part_http::Client::builder()
                .redirects(day_part_http::Redirects::Never)
                .cookies(day_part_http::Cookies::Off)
                .cache(day_part_http::Cache::Off)
                .build();
            let mut target = url.clone();
            let mut response = None;
            for redirect in 0..=10 {
                let cookies = self
                    .engine
                    .cookie_header(target.as_str())
                    .await
                    .unwrap_or_default();
                let mut request = day_part_http::Request::get(target.as_str())
                    .header("Accept", "text/html, application/xhtml+xml;q=0.9")
                    .header(
                        "User-Agent",
                        concat!("FleetReader/", env!("CARGO_PKG_VERSION")),
                    )
                    .timeout(Duration::from_secs(25));
                if !cookies.is_empty() {
                    request = request.header("Cookie", &cookies);
                }
                let received = client
                    .fetch_future(request)
                    .await
                    .map_err(|_| ExtractionError::Network)?;
                self.diagnostics.event(
                    "http_response",
                    serde_json::json!({
                        "status": received.status, "response_bytes": received.body.len(),
                        "redirect_count": redirect,
                    }),
                );
                for (_, cookie) in received
                    .headers
                    .iter()
                    .filter(|(key, _)| key.eq_ignore_ascii_case("set-cookie"))
                {
                    let _ = self.engine.set_cookie(target.as_str(), cookie).await;
                }
                if matches!(received.status, 301 | 302 | 303 | 307 | 308) {
                    let location = received
                        .headers
                        .iter()
                        .find(|(key, _)| key.eq_ignore_ascii_case("location"))
                        .map(|(_, value)| value);
                    if let Some(location) = location {
                        let next = target
                            .join(location)
                            .map_err(|_| ExtractionError::InvalidUrl)?;
                        target = web_url(next.as_str()).ok_or(ExtractionError::InvalidUrl)?;
                        continue;
                    }
                }
                response = Some(received);
                break;
            }
            let response = response.ok_or(ExtractionError::Network)?;
            let (html, base) = decode_page(response, url.as_str())?;
            self.diagnostics
                .event("decoded", serde_json::json!({"html_bytes": html.len()}));
            // Match the final page origin after redirects, never an unrelated feed URL.
            let custom = crate::reader_scripts::for_url(&base);
            let script = extraction_script(&html, &base, custom.as_deref(), self.engine).await?;
            self.diagnostics.event(
                "extracting",
                serde_json::json!({"script_bytes": script.len(), "custom_script": custom.is_some()}),
            );
            let reply = self.engine.eval(script).await.map_err(|_| {
                if custom.is_some() {
                    ExtractionError::CustomScript
                } else {
                    ExtractionError::Unavailable
                }
            })?;
            let article: Option<ExtractedArticle> =
                serde_json::from_str(&reply).map_err(|_| ExtractionError::NoContent)?;
            article
                .filter(|a| !a.content.trim().is_empty())
                .ok_or(ExtractionError::NoContent)
        })
    }
}

fn decode_page(
    response: day_part_http::Response,
    original_url: &str,
) -> Result<(String, String), ExtractionError> {
    if !(200..300).contains(&response.status) {
        return Err(ExtractionError::Network);
    }
    if response.body.len() > MAX_HTML_BYTES {
        return Err(ExtractionError::TooLarge);
    }
    let content_type = response
        .headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.as_str())
        .unwrap_or("text/html");
    let media_type = content_type.split(';').next().unwrap_or_default().trim();
    if !media_type.eq_ignore_ascii_case("text/html")
        && !media_type.eq_ignore_ascii_case("application/xhtml+xml")
    {
        return Err(ExtractionError::NoContent);
    }
    let charset = content_type.split(';').skip(1).find_map(|part| {
        let (key, value) = part.trim().split_once('=')?;
        key.eq_ignore_ascii_case("charset")
            .then(|| value.trim().trim_matches(['\'', '"']))
    });
    let encoding = charset
        .and_then(|name| encoding_rs::Encoding::for_label(name.as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    let (html, _, _) = encoding.decode(&response.body);
    let base = if response.url.is_empty() {
        original_url
    } else {
        &response.url
    };
    let base = web_url(base).ok_or(ExtractionError::InvalidUrl)?;
    Ok((html.into_owned(), base.into()))
}

async fn script_asset(name: day::AssetName, _engine: JsHandle) -> Result<String, ExtractionError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let asset = resource(name).ok_or(ExtractionError::Unavailable)?;
        String::from_utf8(asset.as_slice().to_vec()).map_err(|_| ExtractionError::Unavailable)
    }
    #[cfg(target_arch = "wasm32")]
    {
        // The browser serves generated assets as same-origin files, rather than
        // exposing the native synchronous resource opener. Resolve against the
        // application document, not the reader's resource-worker URL.
        let path = serde_json::to_string(&format!("assets/data/{name}")).unwrap();
        let reply = _engine
            .eval(format!("new URL({path}, parent.document.baseURI).href"))
            .await
            .map_err(|_| ExtractionError::Unavailable)?;
        let url: String = serde_json::from_str(&reply).map_err(|_| ExtractionError::Unavailable)?;
        let response = day_part_http::fetch_future(day_part_http::Request::get(url))
            .await
            .map_err(|_| ExtractionError::Unavailable)?;
        if response.status != 200 {
            return Err(ExtractionError::Unavailable);
        }
        String::from_utf8(response.body).map_err(|_| ExtractionError::Unavailable)
    }
}

async fn extraction_script(
    html: &str,
    url: &str,
    custom: Option<&str>,
    engine: JsHandle,
) -> Result<String, ExtractionError> {
    let readability = script_asset(crate::res::assets::reader::readability_js, engine).await?;
    let purify = script_asset(crate::res::assets::reader::purify_js, engine).await?;
    let extract = script_asset(crate::res::assets::reader::extract_js, engine).await?;
    // JSON quoting keeps publication text inert, including quotes and closing script tags.
    let input = serde_json::json!({ "html": html, "url": url });
    // Explicitly installed code is executable; publication text remains JSON data. A local
    // function scope keeps hook declarations out of the live reader's global namespace.
    let hook = custom.map_or_else(|| "undefined".to_owned(), |source| format!(
        "function(document, url, Readability) {{\n{source}\n;return extract(document, url, Readability);\n}}"
    ));
    Ok(format!(
        "(() => {{ {readability}\n{purify}\n{extract}\nreturn extractArticle({input}, {hook}); }})()"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_web_article_urls_without_embedded_credentials() {
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "https://u:p@example.org/",
        ] {
            assert!(web_url(url).is_none());
        }
        assert!(web_url("https://example.org/article?q=1#part").is_some());
    }

    #[test]
    fn decodes_publication_encoding_and_preserves_redirect_base() {
        let mut response = day_part_http::Response::new(
            200,
            vec![(
                "Content-Type".into(),
                "text/html; charset=windows-1252".into(),
            )],
            b"<p>caf\xe9</p>".to_vec(),
        );
        response.url = "https://example.org/final/article".into();
        assert_eq!(
            decode_page(response, "https://example.org/").unwrap(),
            (
                "<p>café</p>".into(),
                "https://example.org/final/article".into()
            )
        );
    }

    #[test]
    fn rejects_error_pages_non_html_and_oversized_responses() {
        assert_eq!(
            decode_page(
                day_part_http::Response::new(403, vec![], vec![]),
                "https://example.org/"
            ),
            Err(ExtractionError::Network)
        );
        assert_eq!(
            decode_page(
                day_part_http::Response::new(
                    200,
                    vec![("content-type".into(), "application/pdf".into())],
                    vec![]
                ),
                "https://example.org/"
            ),
            Err(ExtractionError::NoContent)
        );
        assert_eq!(
            decode_page(
                day_part_http::Response::new(200, vec![], vec![0; MAX_HTML_BYTES + 1]),
                "https://example.org/"
            ),
            Err(ExtractionError::TooLarge)
        );
    }
}

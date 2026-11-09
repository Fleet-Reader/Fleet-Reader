// SPDX-License-Identifier: Apache-2.0
//! Synthetic publication fixtures: active HTML must never reach the reader.

#[test]
fn strips_active_content_from_json_rss_and_atom() {
    let body = r#"<p onclick="alert(1)">Readable <em>article</em></p><script>alert(1)</script><a href="javascript:alert(1)">link</a><iframe src="https://fixture.example/embed"></iframe>"#;
    let json = format!(
        r#"{{"version":"https://jsonfeed.org/version/1.1","title":"Fixture","items":[{{"id":"one","content_html":"{}"}}]}}"#,
        body.replace('"', "\\\"")
    );
    let rss = format!(
        r#"<rss version="2.0"><channel><title>Fixture</title><link>https://fixture.example/</link><description>Fixture</description><item><guid>one</guid><description><![CDATA[{body}]]></description></item></channel></rss>"#
    );
    let atom = format!(
        r#"<feed xmlns="http://www.w3.org/2005/Atom"><title>Fixture</title><id>fixture</id><updated>2026-09-30T00:00:00Z</updated><entry><id>one</id><title>Fixture</title><updated>2026-09-30T00:00:00Z</updated><content type="html">{}</content></entry></feed>"#,
        body.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    );
    for source in [json, rss, atom] {
        let feed = daynews_feed::parse(source.as_bytes(), "https://fixture.example/feed").unwrap();
        let html = feed.items[0].content_html.as_deref().unwrap();
        assert!(html.contains("Readable <em>article</em>"), "{html}");
        for active in ["<script", "onclick", "javascript:", "<iframe"] {
            assert!(
                !html.contains(active),
                "{active} survived sanitization: {html}"
            );
        }
    }
}

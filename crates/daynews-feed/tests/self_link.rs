// SPDX-License-Identifier: Apache-2.0
//! Synthetic downloaded Atom document: self links retain the subscribable source address.
#[test]
fn downloaded_atom_retains_its_self_link_and_publication_site() {
    let bytes = br#"<feed xmlns="http://www.w3.org/2005/Atom"><id>https://fixture.example/</id><title>Fixture</title><updated>2026-10-05T12:00:00Z</updated><link rel="self" href="https://fixture.example/atom.xml"/><link rel="alternate" href="https://fixture.example/"/></feed>"#;
    let parsed = daynews_feed::parse(bytes, "file:///tmp/downloaded.atom").unwrap();
    assert_eq!(
        parsed.source_url.as_deref(),
        Some("https://fixture.example/atom.xml")
    );
    assert_eq!(parsed.site_url.as_deref(), Some("https://fixture.example/"));
}

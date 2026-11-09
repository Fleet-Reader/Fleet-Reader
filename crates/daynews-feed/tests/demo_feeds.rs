// SPDX-License-Identifier: Apache-2.0
//! The demo feeds the app bundles for its screenshots and walkthrough
//! (resource/assets/demo/README.md): one set per locale the app ships, the same feeds in each,
//! and every feed parsing into articles the timeline and the reader can show.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use daynews_feed::{ParsedFeed, parse};

/// The word the walkthrough searches the English set for (dayscript/walkthrough.yaml).
const SEARCH_TERM: &str = "moon";

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn demo_root() -> PathBuf {
    repo().join("resource/assets/demo")
}

fn entries(dir: &Path, want_dirs: bool) -> BTreeSet<String> {
    std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir() == want_dirs))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.'))
        .collect()
}

/// Every feed in one locale's set, parsed against the locale-free URL the app subscribes with.
fn parsed_set(locale: &str) -> Vec<(String, ParsedFeed)> {
    let dir = demo_root().join(locale);
    entries(&dir, false)
        .into_iter()
        .map(|file| {
            let bytes = std::fs::read(dir.join(&file)).expect("read a demo feed");
            let url = format!("asset:demo/{file}");
            let feed = parse(&bytes, &url).unwrap_or_else(|e| panic!("{locale}/{file}: {e}"));
            (file, feed)
        })
        .collect()
}

#[test]
fn every_app_locale_has_a_demo_set_and_no_set_lacks_a_locale() {
    let locales = entries(&repo().join("resource/locales"), true);
    let sets = entries(&demo_root(), true);
    assert!(sets.contains("en"), "English is the fallback set");
    assert_eq!(
        sets, locales,
        "resource/assets/demo/<locale>/ and resource/locales/<locale>/ name the same locales"
    );
}

#[test]
fn every_set_carries_the_same_feeds() {
    let en = entries(&demo_root().join("en"), false);
    assert_eq!(en.len(), 7, "seven feeds: {en:?}");
    for locale in entries(&demo_root(), true) {
        assert_eq!(
            entries(&demo_root().join(&locale), false),
            en,
            "{locale} carries the same files as en"
        );
    }
}

#[test]
fn demo_feeds_parse_into_complete_articles() {
    for locale in entries(&demo_root(), true) {
        for (file, feed) in parsed_set(&locale) {
            let name = format!("{locale}/{file}");
            assert!(feed.title.is_some(), "{name}: feed title");
            assert!(feed.site_url.is_some(), "{name}: site link");
            assert!(feed.items.len() >= 6, "{name}: at least six articles");
            let ids: BTreeSet<_> = feed.items.iter().map(|i| i.guid.as_str()).collect();
            assert_eq!(ids.len(), feed.items.len(), "{name}: unique article ids");
            for it in &feed.items {
                let title = it.display_title();
                assert!(!title.is_empty(), "{name}: every article is nameable");
                assert!(it.published.is_some(), "{name}: {title:?} carries its date");
                assert!(
                    it.url.as_deref().is_some_and(|u| u.starts_with("https://")),
                    "{name}: {title:?} links to further reading"
                );
                assert!(
                    it.summary.is_some() || it.content_html.is_some(),
                    "{name}: {title:?} has text"
                );
                for text in [it.title.as_deref(), it.summary.as_deref()]
                    .into_iter()
                    .flatten()
                {
                    for bad in ["&amp;", "&lt;", "&#", "<p>"] {
                        assert!(!text.contains(bad), "{name}: {bad} in {text:.80?}");
                    }
                }
            }
        }
    }
}

/// The screenshots are also the parser's showcase: titled and title-less items, full bodies and
/// summary-only feeds all appear in every set.
#[test]
fn every_set_covers_the_parser_shapes() {
    for locale in entries(&demo_root(), true) {
        let set = parsed_set(&locale);
        let items = || set.iter().flat_map(|(_, f)| &f.items);
        assert!(
            items().any(|i| i.title.is_none()),
            "{locale}: a microblog feed"
        );
        assert!(
            items().any(|i| i.content_html.is_some()),
            "{locale}: full article bodies"
        );
        assert!(
            set.iter()
                .any(|(_, f)| f.items.iter().all(|i| i.content_html.is_none())),
            "{locale}: a summary-only feed"
        );
    }
}

#[test]
fn the_walkthrough_search_matches_several_english_feeds() {
    let hits = parsed_set("en")
        .iter()
        .filter(|(_, f)| {
            f.items.iter().any(|i| {
                [
                    i.title.as_deref(),
                    i.summary.as_deref(),
                    i.content_html.as_deref(),
                ]
                .into_iter()
                .flatten()
                .any(|t| t.to_lowercase().contains(SEARCH_TERM))
            })
        })
        .count();
    assert!(hits >= 3, "{SEARCH_TERM:?} appears in {hits} feeds");
}

// SPDX-License-Identifier: Apache-2.0
use daynews_opml::{FeedRef, Opml, Outline, parse, write};

// Real-world OPML, vendored so these tests run on any host (tests/data/README.md records where
// each file came from and under what license).
const SUBSCRIPTIONS: &str = include_str!("data/mySubscriptions.opml");
const CATEGORIES: &str = include_str!("data/categories.opml");
const UNTITLED: &str = include_str!("data/untitled.opml");
// The sample list dayscript/import.yaml hands to the file picker, authored here.
const SAMPLE: &str = include_str!("data/daynews.opml");

/// A real subscription list: every subscription must survive import, entities and all.
#[test]
fn parses_a_real_subscription_list() {
    let doc = parse(SUBSCRIPTIONS).expect("parse");
    let feeds = doc.feeds();
    assert_eq!(feeds.len(), 13, "every subscription should be found");
    assert_eq!(doc.title.as_deref(), Some("mySubscriptions.opml"));
    // This list is flat; folders get their own fixture below.
    assert!(
        feeds.iter().all(|(path, _)| path.is_empty()),
        "no folders in this export"
    );
    // Attributes are decoded, not passed through raw: this title carries `&gt;`…
    let nyt = feeds
        .iter()
        .find(|(_, f)| f.xml_url.ends_with("nyt/Business.xml"))
        .expect("NYT feed");
    assert_eq!(nyt.1.title, "NYT > Business", "entities decoded");
    // …and this site URL carries `&amp;`, which must not survive as an entity.
    let yahoo = feeds
        .iter()
        .find(|(_, f)| f.title.starts_with("Yahoo!"))
        .expect("Yahoo feed");
    let site = yahoo.1.html_url.as_deref().expect("site url");
    assert!(
        site.contains("tmpl=index&cid=738"),
        "entities decoded: {site}"
    );
    assert!(!site.contains("&amp;"), "no double-encoding: {site}");
    // The file also carries `description`, `type`, `version` and `language`, which this parser
    // does not model; unknown attributes are ignored without dropping the subscription.
    assert!(
        feeds.iter().all(|(_, f)| f.xml_url.starts_with("http")),
        "urls absolute"
    );
    assert!(
        feeds.iter().all(|(_, f)| !f.display_title().is_empty()),
        "display_title must never be empty"
    );
}

/// Folders become the path reported beside each subscription.
#[test]
fn folders_become_paths() {
    let doc = parse(CATEGORIES).expect("parse");
    let found: Vec<_> = doc
        .feeds()
        .iter()
        .map(|(path, f)| (path.clone(), f.title.clone()))
        .collect();
    assert_eq!(
        found,
        vec![
            (vec!["My Category 1".to_string()], "Feed 1".to_string()),
            (vec!["My Category 1".to_string()], "Feed 2".to_string()),
            (vec!["My Category 2".to_string()], "Feed 3".to_string()),
        ]
    );
}

/// The sample list an import seeds from: two folders plus one top-level subscription, so one
/// file exercises both shapes, and its feeds are the live originals of the parser's captured
/// fixtures (crates/daynews-feed/tests/data).
#[test]
fn sample_list_mixes_folders_and_top_level_feeds() {
    let doc = parse(SAMPLE).expect("parse");
    assert_eq!(
        doc.title.as_deref(),
        Some("Fleet Reader sample subscriptions")
    );
    let found: Vec<_> = doc
        .feeds()
        .iter()
        .map(|(path, f)| (path.join("/"), f.title.clone()))
        .collect();
    assert_eq!(
        found,
        vec![
            ("Science".to_string(), "NASA".to_string()),
            ("Science".to_string(), "Quanta Magazine".to_string()),
            ("Science".to_string(), "ScienceDaily".to_string()),
            ("Rust".to_string(), "Rust Blog".to_string()),
            ("Rust".to_string(), "Rust Forum: Announcements".to_string()),
            ("Rust".to_string(), "Rust Language".to_string()),
            (
                String::new(),
                "Merriam-Webster's Word of the Day".to_string()
            ),
        ]
    );
    assert!(
        doc.feeds()
            .iter()
            .all(|(_, f)| f.xml_url.starts_with("https://") && f.html_url.is_some()),
        "every subscription names its feed and its site"
    );
}

/// A subscription a reader has never fetched records no name at all: a real state, not a
/// parse failure. The recorded title stays empty; the display fallback still names it.
#[test]
fn untitled_subscriptions_fall_back_to_the_host() {
    let doc = parse(UNTITLED).expect("parse");
    let feeds = doc.feeds();
    assert_eq!(feeds.len(), 2, "both subscriptions found");
    assert!(
        feeds.iter().all(|(_, f)| f.title.is_empty()),
        "the export really does contain untitled subscriptions"
    );
    assert!(
        feeds
            .iter()
            .all(|(_, f)| f.display_title() == "example.org"),
        "falls back to the host"
    );
}

/// Export then re-import must preserve the subscription set exactly, for every shape of
/// document, since the flat list, the folders, the untitled feeds and the mixed sample each
/// stress it differently.
#[test]
fn round_trips_through_export() {
    let fields = |doc: &Opml| -> Vec<(Vec<String>, String, String, Option<String>)> {
        doc.feeds()
            .iter()
            .map(|(path, f)| {
                (
                    path.clone(),
                    f.xml_url.clone(),
                    f.title.clone(),
                    f.html_url.clone(),
                )
            })
            .collect()
    };
    for (name, src) in [
        ("mySubscriptions.opml", SUBSCRIPTIONS),
        ("categories.opml", CATEGORIES),
        ("untitled.opml", UNTITLED),
        ("daynews.opml", SAMPLE),
    ] {
        let doc = parse(src).expect("parse");
        let out = write(&doc).expect("write");
        let back = parse(&out).expect("re-parse our own output");
        assert_eq!(
            fields(&doc),
            fields(&back),
            "{name}: round-trip must preserve every subscription, name, site URL and folder path"
        );
    }
}

/// Deeper nesting and an ampersand in the feed url: shapes the vendored files (one folder
/// level, entities only in titles and site urls) do not reach.
#[test]
fn nested_folders_round_trip() {
    let doc = Opml {
        title: Some("Test".into()),
        outlines: vec![
            Outline::Folder {
                title: "Tech".into(),
                children: vec![
                    Outline::Feed(FeedRef {
                        title: "A".into(),
                        xml_url: "https://a.example/f".into(),
                        html_url: Some("https://a.example/".into()),
                    }),
                    Outline::Folder {
                        title: "Rust".into(),
                        children: vec![Outline::Feed(FeedRef {
                            title: "B & C".into(),
                            xml_url: "https://b.example/f?x=1&y=2".into(),
                            html_url: None,
                        })],
                    },
                ],
            },
            Outline::Feed(FeedRef {
                title: "Top".into(),
                xml_url: "https://t.example/f".into(),
                html_url: None,
            }),
        ],
    };
    let back = parse(&write(&doc).expect("write")).expect("parse");
    assert_eq!(
        back.outlines, doc.outlines,
        "nesting, ampersands and titles preserved"
    );
    let paths: Vec<_> = back.feeds().iter().map(|(p, _)| p.clone()).collect();
    assert_eq!(
        paths,
        vec![
            vec!["Tech".to_string()],
            vec!["Tech".into(), "Rust".into()],
            vec![]
        ]
    );
}

/// What a mis-picked file does: a document with no `<body>` is refused outright, and one that
/// has a body but no outlines imports nothing rather than inventing subscriptions.
#[test]
fn rejects_non_opml() {
    assert!(
        parse("<rss><channel><title>x</title></channel></rss>").is_err(),
        "an RSS feed is not a subscription list"
    );
    let page = parse("<html><body>hi</body></html>").expect("html carries a <body>");
    assert!(page.feeds().is_empty(), "no subscriptions in a web page");
}

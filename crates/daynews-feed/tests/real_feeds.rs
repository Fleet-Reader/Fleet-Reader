// SPDX-License-Identifier: Apache-2.0
//! Parsing checks against feeds captured from the wire: plain RSS 2.0, WordPress,
//! Discourse, Atom and Mastodon variants. Fixtures are bytes, so these run offline and
//! deterministically.
use daynews_feed::parse;

struct Fixture {
    bytes: &'static [u8],
    url: &'static str,
    name: &'static str,
}

// Kept beside the tests, not in the app: the README in tests/data records where each came from.
// The app's screenshots and walkthrough read the demo feeds under resource/assets/demo/ instead
// (tests/demo_feeds.rs), so publishers' articles never ship inside the bundle.
const FIXTURES: &[Fixture] = &[
    Fixture {
        bytes: include_bytes!("data/merriam-webster.xml"),
        url: "https://www.merriam-webster.com/wotd/feed/rss2",
        name: "plain RSS 2.0",
    },
    Fixture {
        bytes: include_bytes!("data/sciencedaily.xml"),
        url: "https://www.sciencedaily.com/rss/all.xml",
        name: "plain RSS 2.0, summaries only",
    },
    Fixture {
        bytes: include_bytes!("data/nasa.xml"),
        url: "https://www.nasa.gov/feed/",
        name: "WordPress RSS + content:encoded",
    },
    Fixture {
        bytes: include_bytes!("data/quanta.xml"),
        url: "https://www.quantamagazine.org/feed/",
        name: "WordPress RSS + media thumbnails",
    },
    Fixture {
        bytes: include_bytes!("data/rust-forum.xml"),
        url: "https://users.rust-lang.org/c/announcements/6.rss",
        name: "Discourse RSS",
    },
    Fixture {
        bytes: include_bytes!("data/rust-blog.xml"),
        url: "https://blog.rust-lang.org/feed.xml",
        name: "Atom",
    },
    Fixture {
        bytes: include_bytes!("data/rust-mastodon.xml"),
        url: "https://social.rust-lang.org/@rust.rss",
        name: "Mastodon RSS",
    },
];

#[test]
fn every_real_feed_parses_with_usable_items() {
    for f in FIXTURES {
        let feed = parse(f.bytes, f.url).unwrap_or_else(|e| panic!("{}: {e}", f.name));
        assert!(feed.title.is_some(), "{}: feed title", f.name);
        assert!(!feed.items.is_empty(), "{}: items", f.name);
        for it in &feed.items {
            assert!(
                !it.guid.trim().is_empty(),
                "{}: every item needs a stable id",
                f.name
            );
        }
        // Microblog feeds (Mastodon) ship items with no <title> at all, so the requirement is
        // that every item can still be named and opened.
        for it in &feed.items {
            assert!(
                !it.display_title().is_empty(),
                "{}: item must be nameable",
                f.name
            );
        }
        let linked = feed.items.iter().filter(|i| i.url.is_some()).count();
        assert!(
            linked * 2 >= feed.items.len(),
            "{}: most items should link somewhere",
            f.name
        );
        println!(
            "{:34} {:>4} items, title={:?}, site={:?}",
            f.name,
            feed.items.len(),
            feed.title.as_deref().unwrap_or(""),
            feed.site_url.as_deref().unwrap_or("")
        );
    }
}

/// Item identity must be stable across re-parses, or every refresh resurrects read articles.
#[test]
fn item_ids_are_stable_and_unique() {
    for f in FIXTURES {
        let a = parse(f.bytes, f.url).expect("parse");
        let b = parse(f.bytes, f.url).expect("re-parse");
        let ids_a: Vec<_> = a.items.iter().map(|i| i.guid.clone()).collect();
        let ids_b: Vec<_> = b.items.iter().map(|i| i.guid.clone()).collect();
        assert_eq!(ids_a, ids_b, "{}: ids must be deterministic", f.name);
        let uniq: std::collections::HashSet<_> = ids_a.iter().collect();
        assert_eq!(
            uniq.len(),
            ids_a.len(),
            "{}: ids must be unique within a feed",
            f.name
        );
    }
}

/// Titles are plain text, with neither tags nor raw entities.
#[test]
fn titles_are_clean_text() {
    for f in FIXTURES {
        let feed = parse(f.bytes, f.url).expect("parse");
        for it in feed.items.iter().filter_map(|i| i.title.as_ref()) {
            assert!(
                !it.contains('<') && !it.contains('>'),
                "{}: tag in title: {it:?}",
                f.name
            );
            assert!(
                !it.contains("&amp;") && !it.contains("&#"),
                "{}: undecoded entity: {it:?}",
                f.name
            );
        }
    }
}

/// Garbage in must not panic; a publisher serving an HTML error page is routine.
#[test]
fn non_feed_input_errors_cleanly() {
    assert!(
        parse(
            b"<html><body>404 not found</body></html>",
            "https://x.example/f"
        )
        .is_err()
    );
    assert!(parse(b"", "https://x.example/f").is_err());
    assert!(parse(b"\x00\x01\x02 not xml", "https://x.example/f").is_err());
}

/// The Mastodon case explicitly: title-less items get a readable name from their body, with the
/// escaped markup resolved rather than shown.
#[test]
fn microblog_items_derive_a_title_from_content() {
    let f = FIXTURES
        .iter()
        .find(|f| f.name == "Mastodon RSS")
        .expect("the Mastodon fixture");
    let feed = parse(f.bytes, f.url).expect("parse");
    let first = feed.items.first().expect("an item");
    assert!(first.title.is_none(), "Mastodon items carry no <title>");
    let t = first.display_title();
    assert!(!t.is_empty() && t.len() <= 130, "derived title: {t:?}");
    assert!(
        !t.contains('<') && !t.contains("&lt;"),
        "markup must be resolved, got {t:?}"
    );
    println!("derived title: {t}");
}

/// Prose containing a bare `<` is not mistaken for markup.
#[test]
fn angle_brackets_in_prose_survive() {
    let xml = br#"<?xml version="1.0"?><rss version="2.0"><channel><title>T</title>
      <link>https://e.example/</link>
      <item><title>Why 2 &lt; 3 &amp; 4 &gt; 1</title><link>https://e.example/a</link>
      <guid>a</guid></item></channel></rss>"#;
    let feed = parse(xml, "https://e.example/f").expect("parse");
    assert_eq!(feed.items[0].title.as_deref(), Some("Why 2 < 3 & 4 > 1"));
}

/// Summaries are shown as plain text in the timeline, so typographic entities must be resolved;
/// publishers escape them constantly and nothing downstream would decode them.
#[test]
fn typographic_entities_are_decoded_in_text() {
    let xml = br#"<?xml version="1.0"?><rss version="2.0"><channel><title>T</title>
      <link>https://e.example/</link>
      <item><title>We&rsquo;ve shipped&hellip;</title><link>https://e.example/a</link><guid>a</guid>
      <description>&ldquo;privatized&rdquo; photos &mdash; and more &amp; more</description>
      </item></channel></rss>"#;
    let feed = parse(xml, "https://e.example/f").expect("parse");
    let it = &feed.items[0];
    assert_eq!(it.title.as_deref(), Some("We’ve shipped…"));
    assert_eq!(
        it.summary.as_deref(),
        Some("“privatized” photos — and more & more")
    );
    assert!(
        !it.summary.as_deref().unwrap().contains('&')
            || it.summary.as_deref().unwrap().contains("& more")
    );
}

/// Every summary from the real fixtures must be free of leftover named entities.
#[test]
fn real_feed_summaries_have_no_raw_entities() {
    for f in FIXTURES {
        let feed = parse(f.bytes, f.url).expect("parse");
        for s in feed.items.iter().filter_map(|i| i.summary.as_ref()) {
            for bad in [
                "&rsquo;", "&ldquo;", "&rdquo;", "&mdash;", "&amp;", "&#8217;",
            ] {
                assert!(
                    !s.contains(bad),
                    "{}: undecoded {bad} in summary: {s:.90}",
                    f.name
                );
            }
        }
    }
}

/// `&#149;` is a Windows-1252 bullet, not the C1 control U+0095 that number names in Unicode:
/// browsers read numeric references 128–159 through Windows-1252 (the HTML standard says so),
/// and Merriam-Webster separates its pronunciations exactly this way. The XML layer resolves
/// the reference before this crate sees it, so the raw control has to be remapped as a
/// character, in titles, summaries and bodies alike, or Android draws a box.
#[test]
fn c1_controls_decode_as_windows_1252() {
    let xml = "<?xml version=\"1.0\"?><rss version=\"2.0\"><channel><title>T</title>\
      <link>https://e.example/</link>\
      <item><title>a &#149; b &#151; c &#x92; d \u{96}</title><link>https://e.example/a</link>\
      <guid>a</guid><description>&lt;p&gt;x &amp;#149; y&lt;/p&gt;</description>\
      <content:encoded xmlns:content=\"http://purl.org/rss/1.0/modules/content/\">\
      &lt;p&gt;body &#149; here&lt;/p&gt;</content:encoded></item></channel></rss>";
    let feed = parse(xml.as_bytes(), "https://e.example/f").expect("parse");
    let it = &feed.items[0];
    assert_eq!(it.title.as_deref(), Some("a • b — c ’ d –"));
    assert_eq!(
        it.summary.as_deref(),
        Some("x • y"),
        "double-escaped reference"
    );
    assert_eq!(
        it.content_html.as_deref(),
        Some("<p>body • here</p>"),
        "the body is remapped too"
    );
    for f in FIXTURES {
        let feed = parse(f.bytes, f.url).expect("parse");
        for text in feed
            .items
            .iter()
            .flat_map(|i| [i.title.as_deref(), i.summary.as_deref()])
            .flatten()
        {
            assert!(
                !text.chars().any(|c| ('\u{80}'..='\u{9f}').contains(&c)),
                "{}: C1 control in {text:.80?}",
                f.name
            );
        }
    }
}

/// Documents the decoding layers, which are easy to get wrong: the XML parser resolves one
/// level of escaping before we ever see the text, and `clean` resolves what remains. A publisher
/// writing `&amp;amp;` has escaped twice and means a literal `&`, which is what a reader shows.
#[test]
fn double_escaped_text_resolves_to_one_ampersand() {
    let xml = br#"<?xml version="1.0"?><rss version="2.0"><channel><title>T</title>
      <link>https://e.example/</link>
      <item><title>Tips &amp;amp; tricks</title><link>https://e.example/a</link>
      <guid>a</guid></item></channel></rss>"#;
    let feed = parse(xml, "https://e.example/f").expect("parse");
    assert_eq!(feed.items[0].title.as_deref(), Some("Tips & tricks"));
}

/// RSS 2.0 lets `description` carry HTML, and plenty of feeds put the whole article there with no
/// `content:encoded`. That HTML is the article body: flattened to text, the reader ran every
/// paragraph together. A plain-text description stays a summary, and `content:encoded` still wins.
#[test]
fn an_html_description_is_the_body_when_nothing_richer_exists() {
    let xml = br#"<?xml version="1.0"?><rss version="2.0"><channel><title>T</title>
      <link>https://e.example/</link>
      <item><title>Escaped</title><link>https://e.example/a</link><guid>a</guid>
      <description>&lt;p&gt;One.&lt;/p&gt;&lt;p&gt;Two &amp;amp; three.&lt;/p&gt;</description></item>
      <item><title>CDATA</title><link>https://e.example/b</link><guid>b</guid>
      <description><![CDATA[<p>Four.</p><p>Five.</p>]]></description></item>
      <item><title>Plain</title><link>https://e.example/c</link><guid>c</guid>
      <description>Six is less than 2 &lt; 7 in no way.</description></item>
      <item><title>Both</title><link>https://e.example/d</link><guid>d</guid>
      <description>&lt;p&gt;Teaser.&lt;/p&gt;</description>
      <content:encoded xmlns:content="http://purl.org/rss/1.0/modules/content/">&lt;p&gt;Full.&lt;/p&gt;</content:encoded>
      </item></channel></rss>"#;
    let feed = parse(xml, "https://e.example/f").expect("parse");
    let [escaped, cdata, plain, both] = &feed.items[..] else {
        panic!("four items, got {}", feed.items.len());
    };
    assert_eq!(
        escaped.content_html.as_deref(),
        Some("<p>One.</p><p>Two &amp; three.</p>")
    );
    assert_eq!(escaped.summary.as_deref(), Some("One. Two & three."));
    assert_eq!(
        cdata.content_html.as_deref(),
        Some("<p>Four.</p><p>Five.</p>")
    );
    assert_eq!(
        plain.content_html, None,
        "prose with a bare < is not markup"
    );
    assert_eq!(both.content_html.as_deref(), Some("<p>Full.</p>"));
    assert_eq!(both.summary.as_deref(), Some("Teaser."));
}

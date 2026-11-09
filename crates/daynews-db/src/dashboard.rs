// SPDX-License-Identifier: Apache-2.0
//! Bounded dashboard projections: scan article metadata, never fault article bodies.
use crate::{Db, Feed, Scope, Value};
use day_persistence::DbError;
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Debug, PartialEq)]
pub struct PollingFeed {
    pub id: u64,
    pub title: String,
    pub interval: i64,
    pub next: Option<i64>,
    pub retry_after: Option<i64>,
    pub last_checked: Option<i64>,
    pub failed: bool,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dashboard {
    pub feed_id: Option<u64>,
    source_counts: BTreeMap<u64, u64>,
    pub title: String,
    pub description: Option<String>,
    pub site_url: Option<String>,
    pub feed_url: Option<String>,
    pub total: u64,
    pub unread: u64,
    pub starred: u64,
    pub words: u64,
    pub authors: usize,
    pub latest: Option<i64>,
    pub earliest: Option<i64>,
    pub ages: [u64; 5],
    pub hourly: [u64; 24],
    /// At most 366 day bins, regardless of library size.
    pub days: Vec<(i64, u64)>,
    pub sources: Vec<(u64, String, u64)>,
    pub feeds: Vec<PollingFeed>,
}

/// Words estimated from visible stored text; inline markup does not split a word.
pub fn word_count(html: &str) -> u32 {
    let mut text = String::new();
    let mut tag = String::new();
    let mut in_tag = false;
    let mut skip = false;
    for c in html.chars() {
        match c {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '>' if in_tag => {
                in_tag = false;
                let name = tag
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                match name.trim_end_matches('/') {
                    "script" | "style" => skip = true,
                    "/script" | "/style" => skip = false,
                    "p" | "/p" | "div" | "/div" | "br" | "li" | "/li" | "h1" | "/h1" | "h2"
                    | "/h2" => text.push(' '),
                    _ => {}
                }
            }
            _ if in_tag => tag.push(c),
            _ if !skip => text.push(c),
            _ => {}
        }
    }
    // Common non-breaking/spacing entities must be boundaries, not artificial words.
    let text = text
        .replace("&nbsp;", " ")
        .replace("&#160;", " ")
        .replace("&#xA0;", " ")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&mdash;", " ")
        .replace("&ndash;", " ");
    text.split_whitespace()
        .filter(|word| word.chars().any(char::is_alphanumeric))
        .count()
        .min(u32::MAX as usize) as u32
}

/// The articles of `scope` as a SQL condition on `articles a`, with its parameters.
fn scope_predicate(scope: Scope, now: i64) -> (&'static str, Vec<Value>) {
    match scope {
        Scope::All => ("1", vec![]),
        Scope::Unread => ("a.is_read = 0", vec![]),
        Scope::Starred => ("a.is_starred = 1", vec![]),
        Scope::Today => (
            "a.published_at >= ? AND a.published_at <= ?",
            vec![Value::Int(daynews_time::start_of_day(now)), Value::Int(now)],
        ),
        Scope::Feed(id) => ("a.feed = ?", vec![Value::Int(id as i64)]),
        Scope::Folder(id) => (
            "a.feed IN (SELECT id FROM feeds WHERE folder = ?)",
            vec![Value::Int(id as i64)],
        ),
        Scope::Tag(id) => (
            "a.id IN (SELECT article_id FROM article_tags WHERE tag_id = ?)",
            vec![Value::Int(id as i64)],
        ),
    }
}

/// One word of the dashboard's word cloud: as the articles spell it, and how often it occurs.
#[derive(Clone, Debug, PartialEq)]
pub struct CloudWord {
    pub word: String,
    pub count: u64,
}

/// Fold a word the way the search index does (`unicode61 remove_diacritics 2`): lowercase,
/// with combining accents dropped, so "École" and "ecole" are one term. The precomposed
/// letters of the Latin alphabets fold through their decomposition table below; a letter with
/// no table entry keeps its accent, as the index's does.
pub fn fold(word: &str) -> String {
    word.chars()
        .flat_map(char::to_lowercase)
        .filter(|c| !is_combining_mark(*c))
        .map(strip_accent)
        .collect()
}

fn is_combining_mark(c: char) -> bool {
    matches!(c, '\u{0300}'..='\u{036F}' | '\u{1AB0}'..='\u{1AFF}' | '\u{1DC0}'..='\u{1DFF}'
        | '\u{20D0}'..='\u{20FF}' | '\u{FE20}'..='\u{FE2F}')
}

/// The unaccented base of a precomposed Latin letter (Latin-1 Supplement and Latin
/// Extended-A, already lowercase), the letters feeds in Latin-alphabet languages use.
fn strip_accent(c: char) -> char {
    const FROM: &str =
        "àáâãäåāăąçćĉċčďđèéêëēĕėęěĝğġģĥħìíîïĩīĭįıĵķĺļľŀłñńņňòóôõöøōŏőŕŗřśŝşšţťŧùúûüũūŭůűųŵýÿŷźżž";
    const TO: &str =
        "aaaaaaaaacccccddeeeeeeeeegggghhiiiiiiiiijklllllnnnnoooooooooorrrsssstttuuuuuuuuuuwyyyzzz";
    FROM.chars()
        .position(|f| f == c)
        .and_then(|i| TO.chars().nth(i))
        .unwrap_or(c)
}

/// The vendored stopword lists (data/stopwords/README.md), folded like the index's terms.
fn stopwords() -> &'static [(&'static str, HashSet<String>)] {
    macro_rules! lists {
        ($($lang:literal),* $(,)?) => {
            [$(($lang, include_str!(concat!("../data/stopwords/", $lang, ".txt")))),*]
        };
    }
    static LISTS: std::sync::OnceLock<Vec<(&'static str, HashSet<String>)>> =
        std::sync::OnceLock::new();
    LISTS.get_or_init(|| {
        lists!(
            "af", "ar", "bg", "bn", "br", "ca", "cs", "da", "de", "el", "en", "eo", "es", "et",
            "eu", "fa", "fi", "fr", "ga", "gl", "gu", "ha", "he", "hi", "hr", "hu", "hy", "id",
            "it", "ku", "la", "lt", "lv", "mr", "ms", "nl", "no", "pl", "pt", "ro", "ru", "sk",
            "sl", "so", "st", "sv", "sw", "tl", "tr", "uk", "ur", "vi", "yo", "zu",
        )
        .into_iter()
        .map(|(lang, text)| (lang, text.lines().map(fold).collect()))
        .collect()
    })
}

/// Words left in summaries by HTML and feed boilerplate rather than by their writers.
const MARKUP: &[&str] = &[
    "href",
    "http",
    "https",
    "www",
    "com",
    "org",
    "net",
    "html",
    "htm",
    "img",
    "src",
    "alt",
    "div",
    "span",
    "class",
    "style",
    "width",
    "height",
    "nbsp",
    "amp",
    "quot",
    "apos",
    "rel",
    "noopener",
    "noreferrer",
    "target",
    "blank",
    "jpg",
    "jpeg",
    "png",
    "gif",
    "webp",
    "svg",
    "utm",
    "figure",
    "figcaption",
    "strong",
    "appeared",
];

/// The stopword languages of a cloud: the language whose list covers the most of `terms`,
/// weighted by count, and any other that covers at least a quarter as much (a library that
/// mixes two languages loses both sets of articles). `fallback` (an ISO 639-1 code) answers
/// when no list matches at all.
fn cloud_languages(terms: &[(String, u64)], fallback: &str) -> Vec<&'static HashSet<String>> {
    let scores: Vec<(u64, &HashSet<String>)> = stopwords()
        .iter()
        .map(|(_, list)| {
            let score = terms
                .iter()
                .filter(|(term, _)| list.contains(term))
                .map(|(_, n)| n)
                .sum();
            (score, list)
        })
        .collect();
    let best = scores.iter().map(|(score, _)| *score).max().unwrap_or(0);
    if best == 0 {
        return stopwords()
            .iter()
            .filter(|(lang, _)| *lang == fallback)
            .map(|(_, list)| list)
            .collect();
    }
    scores
        .into_iter()
        .filter(|(score, _)| *score * 4 >= best)
        .map(|(_, list)| list)
        .collect()
}

/// The fewest articles a feed needs before a term in most of them counts as its boilerplate.
const BOILERPLATE_FEED: u64 = 8;

/// Whether a term can stand in a word cloud: three characters or more, with a letter.
fn cloud_term(term: &str) -> bool {
    term.chars().count() >= 3 && term.chars().any(char::is_alphabetic) && !MARKUP.contains(&term)
}

/// The spelling a word cloud shows for one term, from how often each spelling occurs.
/// Headlines capitalize ordinary words, so a lowercase spelling seen at least a third as often
/// as the most common one wins; a name or an acronym ("Microsoft", "NASA") is rarely lowercase
/// and keeps its capitals.
fn spelling(forms: &BTreeMap<String, u64>) -> String {
    let Some((top, top_count)) = forms.iter().max_by_key(|(_, n)| **n) else {
        return String::new();
    };
    forms
        .iter()
        .filter(|(form, _)| form.chars().all(|c| !c.is_uppercase()))
        .max_by_key(|(_, n)| **n)
        .filter(|(_, n)| **n * 3 >= *top_count)
        .map_or(top, |(form, _)| form)
        .clone()
}

#[derive(Default)]
pub struct DashboardCache {
    key: Option<(u64, i64, Scope)>,
    articles: Dashboard,
}
impl Db {
    /// The `limit` most frequent words in the titles and summaries of `scope`'s articles, for
    /// the dashboard's word cloud, most frequent first.
    ///
    /// The counts come straight from the search index: an `fts5vocab` table over `articles_fts`
    /// lists every indexed term with the article it occurs in, so counting one feed's words
    /// reads the index rather than re-tokenizing its text. The index folds case and accents
    /// away, so each word is then shown in its most common original spelling, read from the
    /// scope's articles that contain it. Stopwords go by the languages the terms themselves
    /// are in; `fallback` (an ISO 639-1 code, the reader's language) answers for a scope too
    /// small to tell.
    pub fn word_cloud(
        &self,
        scope: Scope,
        now: i64,
        limit: usize,
        fallback: &str,
    ) -> Result<Vec<CloudWord>, DbError> {
        let (predicate, parameters) = scope_predicate(scope, now);
        // Counted in SQL, per feed and term first: a term in more than half of a feed's articles
        // (a feed with enough articles to tell) is that feed's boilerplate, "Comments URL" under
        // every link a news aggregator posts, not something its articles are about, so that
        // feed's occurrences of it do not count. Only the top terms leave the engine.
        let mut terms: Vec<(String, u64)> = Vec::new();
        let mut doubled = parameters.clone();
        doubled.extend(parameters.iter().cloned());
        self.container.try_with_connection(|conn| {
            conn.query(
                &format!(
                    "WITH per_feed AS (\
                       SELECT a.feed AS feed, v.term AS term, count(*) AS n, \
                              count(DISTINCT v.doc) AS articles \
                       FROM articles_fts_terms v JOIN articles a ON a.id = v.doc \
                       WHERE v.col IN ('title', 'summary') AND {predicate} \
                       GROUP BY a.feed, v.term), \
                     sizes AS (\
                       SELECT a.feed AS feed, count(*) AS size FROM articles a \
                       WHERE {predicate} GROUP BY a.feed) \
                     SELECT per_feed.term, sum(per_feed.n) FROM per_feed \
                     JOIN sizes ON sizes.feed = per_feed.feed \
                     WHERE NOT (sizes.size >= {BOILERPLATE_FEED} \
                                AND per_feed.articles * 2 > sizes.size) \
                     GROUP BY per_feed.term ORDER BY sum(per_feed.n) DESC, per_feed.term \
                     LIMIT 2000"
                ),
                &doubled,
                &mut |row| {
                    if let Ok(term) = row.get(0).as_text() {
                        terms.push((term.to_owned(), row.get(1).as_int().unwrap_or(0) as u64));
                    }
                },
            )
        })?;
        let languages = cloud_languages(&terms[..terms.len().min(400)], fallback);
        terms.retain(|(term, _)| {
            cloud_term(term) && !languages.iter().any(|list| list.contains(term))
        });
        terms.truncate(limit);
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        // Original spellings: every spelling of each kept term in the articles that contain one,
        // tokenized as the index tokenizes (letters, digits and combining marks).
        let wanted: HashSet<&str> = terms.iter().map(|(term, _)| term.as_str()).collect();
        let mut spellings: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
        let matcher = terms
            .iter()
            .map(|(term, _)| format!("\"{}\"", term.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" OR ");
        let mut spelled_parameters = parameters.clone();
        spelled_parameters.push(Value::Text(matcher));
        self.container.try_with_connection(|conn| {
            conn.query(
                &format!(
                    "SELECT a.title, a.summary FROM articles a WHERE {predicate} AND a.id IN \
                     (SELECT rowid FROM articles_fts WHERE articles_fts MATCH ?) LIMIT 5000"
                ),
                &spelled_parameters,
                &mut |row| {
                    for column in 0..2 {
                        let value = row.get(column);
                        let Ok(text) = value.as_text() else {
                            continue;
                        };
                        for token in text
                            .split(|c: char| !(c.is_alphanumeric() || is_combining_mark(c)))
                            .filter(|token| !token.is_empty())
                        {
                            let folded = fold(token);
                            if wanted.contains(folded.as_str()) {
                                *spellings
                                    .entry(folded)
                                    .or_default()
                                    .entry(token.to_owned())
                                    .or_default() += 1;
                            }
                        }
                    }
                },
            )
        })?;
        Ok(terms
            .into_iter()
            .map(|(term, count)| CloudWord {
                word: spellings.get(&term).map(spelling).unwrap_or(term),
                count,
            })
            .collect())
    }

    pub fn dashboard(&self, scope: Scope, now: i64) -> Result<Dashboard, DbError> {
        self.dashboard_cached(scope, now, &mut DashboardCache::default())
    }
    pub fn dashboard_cached(
        &self,
        scope: Scope,
        now: i64,
        cache: &mut DashboardCache,
    ) -> Result<Dashboard, DbError> {
        let (predicate, parameters) = scope_predicate(scope, now);
        let key = (
            self.container.cache::<crate::Article>().version(),
            daynews_time::start_of_day(now),
            scope,
        );
        let mut result = if cache.key == Some(key) {
            cache.articles.clone()
        } else {
            let mut result = Dashboard::default();
            let mut days = BTreeMap::<i64, u64>::new();
            let mut sources = BTreeMap::<u64, u64>::new();
            let mut authors = HashSet::new();
            let today = daynews_time::start_of_day(now);
            self.container.try_with_connection(|conn| conn.query(
            &format!("SELECT a.feed, a.published_at, a.word_count, a.is_read, a.is_starred, a.author FROM articles a WHERE {predicate}"),
            &parameters, &mut |row| {
                let feed = row.get(0).as_int().unwrap_or(0) as u64;
                let date = row.get(1).as_int().unwrap_or(0);
                let words = row.get(2).as_int().unwrap_or(0).max(0) as u64;
                result.total += 1;
                result.unread += u64::from(row.get(3).as_int().unwrap_or(0) == 0);
                result.starred += u64::from(row.get(4).as_int().unwrap_or(0) != 0);
                result.words += words;
                if let Ok(author) = row.get(5).as_text() && !author.trim().is_empty() { authors.insert(author.to_owned()); }
                let age = now.saturating_sub(date).max(0);
                result.ages[match age / 86400 { 0 => 0, 1..=6 => 1, 7..=29 => 2, 30..=89 => 3, _ => 4 }] += 1;
                *sources.entry(feed).or_default() += 1;
                if date > 0 && date <= now {
                    result.latest = Some(result.latest.unwrap_or(date).max(date));
                    result.earliest = Some(result.earliest.unwrap_or(date).min(date));
                    let local = date + i64::from(daynews_time::local_offset_seconds(date).unwrap_or(0));
                    if date >= today { result.hourly[local.rem_euclid(86400) as usize / 3600] += 1; }
                    if date >= now.saturating_sub(365 * 86400) {
                        *days.entry(local.div_euclid(86400) * 86400).or_default() += 1;
                    }
                }
            }))?;
            result.authors = authors.len();
            result.days = days.into_iter().collect();
            result.source_counts = sources;
            cache.key = Some(key);
            cache.articles = result.clone();
            result
        };
        let feeds = self
            .container
            .query::<Feed>()
            .sort(Feed::position().asc())
            .sort(Feed::title().asc())
            .live()
            .try_collect()?;
        for feed in feeds {
            let relevant = match scope {
                Scope::Feed(id) => id == feed.id,
                Scope::All | Scope::Unread | Scope::Today | Scope::Starred => true,
                _ => result.source_counts.contains_key(&feed.id),
            };
            if let Scope::Feed(id) = scope
                && id == feed.id
            {
                result.feed_id = Some(feed.id);
                result.title = feed.title.clone();
                result.description = feed.description.clone();
                result.site_url = feed.site_url.clone();
                result.feed_url = Some(feed.feed_url.clone());
            }
            if let Some(count) = result.source_counts.get(&feed.id) {
                result.sources.push((feed.id, feed.title.clone(), *count));
            }
            if relevant {
                result.feeds.push(PollingFeed {
                    id: feed.id,
                    title: feed.title,
                    interval: crate::polling::interval(feed.publication_history.as_deref(), now),
                    next: feed.next_poll_at,
                    retry_after: feed.retry_after,
                    last_checked: feed.last_fetched_at,
                    failed: feed.last_error.is_some(),
                });
            }
        }
        result.sources.sort_by(|a, b| {
            b.2.cmp(&a.2)
                .then_with(|| a.1.cmp(&b.1))
                .then_with(|| a.0.cmp(&b.0))
        });
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn conditional_metadata_changes_do_not_rescan_dashboard_articles() {
        use std::sync::{Arc, Mutex};
        let statements = Arc::new(Mutex::new(Vec::<String>::new()));
        let recorder = statements.clone();
        let db = Db::from_driver(
            day_persistence::Sqlite::memory()
                .trace_sql(move |sql| recorder.lock().unwrap().push(sql.to_owned())),
        )
        .unwrap();
        let url = "https://fixture.example/dashboard-cache";
        let feed = db.add_feed(url, "Fixture", None);
        db.upsert_articles(
            feed,
            url,
            &[crate::IncomingArticle {
                guid: "fixture-one".into(),
                content_html: Some("<p>A synthetic fixture.</p>".into()),
                published: Some(100),
                title: None,
                url: None,
                author: None,
                summary: None,
            }],
        );
        let mut cache = DashboardCache::default();
        let first = db
            .dashboard_cached(Scope::Feed(feed), 200, &mut cache)
            .unwrap();
        assert_eq!(first.total, 1);
        db.feed_checked(feed, None, None);
        statements.lock().unwrap().clear();
        let checked = db
            .dashboard_cached(Scope::Feed(feed), 200, &mut cache)
            .unwrap();
        assert_eq!(checked.words, first.words);
        assert!(
            !statements
                .lock()
                .unwrap()
                .iter()
                .any(|sql| sql.contains("FROM articles a WHERE")),
            "a successful conditional check must reuse article aggregates"
        );
        db.set_read(crate::article_id(url, "fixture-one"), true)
            .unwrap();
        statements.lock().unwrap().clear();
        let changed = db
            .dashboard_cached(Scope::Feed(feed), 200, &mut cache)
            .unwrap();
        assert_eq!(changed.unread, 0);
        assert!(
            statements
                .lock()
                .unwrap()
                .iter()
                .any(|sql| sql.contains("FROM articles a WHERE"))
        );
    }
    #[test]
    fn publisher_identity_survives_duplicate_titles_and_cached_scope_changes() {
        let db = Db::from_driver(day_persistence::Sqlite::memory()).unwrap();
        let mut ids = Vec::new();
        for url in ["https://fixture.example/one", "https://fixture.example/two"] {
            let id = db.add_feed(url, "Same fixture title", None);
            db.upsert_articles(
                id,
                url,
                &[crate::IncomingArticle {
                    guid: "fixture".into(),
                    published: Some(100),
                    content_html: None,
                    title: None,
                    url: None,
                    author: None,
                    summary: None,
                }],
            );
            ids.push(id);
        }
        let mut cache = DashboardCache::default();
        let aggregate = db.dashboard_cached(Scope::All, 200, &mut cache).unwrap();
        assert_eq!(aggregate.feed_id, None);
        assert_eq!(aggregate.sources.len(), 2);
        assert_ne!(aggregate.sources[0].0, aggregate.sources[1].0);
        for id in ids {
            let feed = db
                .dashboard_cached(Scope::Feed(id), 200, &mut cache)
                .unwrap();
            assert_eq!(feed.feed_id, Some(id));
            assert_eq!(feed.sources, vec![(id, "Same fixture title".into(), 1)]);
        }
    }
    #[test]
    fn visible_text_word_counts() {
        assert_eq!(
            word_count(
                "<p>Hello <b>beautiful</b> world.</p><p>Second&nbsp;paragraph.</p><script>ignored text</script>"
            ),
            5
        );
        assert_eq!(word_count("hel<b>lo</b> &amp; goodbye"), 2);
        assert_eq!(word_count(""), 0);
    }
}

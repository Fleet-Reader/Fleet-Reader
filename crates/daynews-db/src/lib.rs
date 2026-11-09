// SPDX-License-Identifier: Apache-2.0
//! The local article store, on day-persistence's lazy engine.
//!
//! The schema is relational and declared: folders own feeds (ordered, cascade), feeds own
//! articles (cascade), an article's body is its own row (so the timeline's window-faulting
//! never loads bodies), tags cross articles through a join table, and search runs through
//! generated FTS5 shadows over titles/authors/summaries and over bodies; one fetch reads
//! both via a relation-crossing match. Older stores backfill missing word counts once; otherwise
//! open does not materialize articles. The UI binds live queries and
//! the engine answers them.
//!
//! Identity is deterministic: a feed's id is a hash of its URL, an article's a hash of
//! (feed URL, guid), a folder's/tag's a hash of its name. Subscribing twice, re-importing an
//! OPML, or refetching a feed therefore cannot create duplicates (the id already exists),
//! and read state survives every refresh because an existing article is never touched.

pub mod dashboard;
mod polling;
pub mod sync;
#[cfg(all(feature = "experimental-webdav", not(target_arch = "wasm32")))]
pub mod webdav;

use day_model::{ModelId, Op};
use day_persistence::{
    CountQuery, DbError, Fetch, Many, Model, ModelContainer, One, Pred, Query, Sqlite, schema,
};
use day_reactive::Binding;

pub use day_persistence::Value;

// Every fts(…) below declares `tokenize = "unicode61 remove_diacritics 2"`: diacritics-
// insensitive search, so `ecole` finds `École`, which is what a reader's search field means. (An
// attribute takes only literals, so the string repeats rather than naming a const.)

// ---------------------------------------------------------------------------
// Models
// ---------------------------------------------------------------------------

#[derive(Model, Clone, Default, PartialEq, Debug)]
#[model(table = "folders")]
pub struct Folder {
    #[model(id)]
    pub id: u64,
    #[model(unique)]
    pub name: String,
    /// Sidebar order among folders (fractional keying, like every ordered surface).
    pub position: f64,
    /// How the sidebar orders this folder's feeds: a [`FeedSort::key`], empty for
    /// [`FeedSort::Priority`].
    pub sort: String,
    /// The schema still cascades, but [`Db::delete_folder`] releases the feeds first: deleting
    /// a collection leaves its feeds subscribed, outside any collection.
    #[model(relation(target = Feed, inverse = "folder", delete = "cascade", ordered = "position"))]
    pub feeds: Many<Feed>,
}

/// How a folder's feeds are ordered in the sidebar (Settings ▸ Feeds ▸ Sort Feeds).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FeedSort {
    /// The global feed order: refresh priority and the timeline's feed-group order.
    #[default]
    Priority,
    /// Newest article first.
    RecentlyUpdated,
    Name,
    /// Most unread articles first.
    UnreadCount,
    /// Most stored articles first.
    TotalCount,
    /// The folder's own order, set by dragging ([`Feed::collection_position`]).
    Manual,
}

impl FeedSort {
    pub const ALL: [FeedSort; 6] = [
        FeedSort::Priority,
        FeedSort::RecentlyUpdated,
        FeedSort::Name,
        FeedSort::UnreadCount,
        FeedSort::TotalCount,
        FeedSort::Manual,
    ];

    /// The stored spelling; Priority is the empty default.
    pub fn key(self) -> &'static str {
        match self {
            FeedSort::Priority => "",
            FeedSort::RecentlyUpdated => "updated",
            FeedSort::Name => "name",
            FeedSort::UnreadCount => "unread",
            FeedSort::TotalCount => "total",
            FeedSort::Manual => "manual",
        }
    }

    /// The sort a stored key names; anything unknown reads as Priority.
    pub fn from_key(key: &str) -> FeedSort {
        FeedSort::ALL
            .into_iter()
            .find(|sort| sort.key() == key)
            .unwrap_or_default()
    }
}

/// Per-feed article statistics for the sidebar's badges and sorts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeedStats {
    pub unread: i64,
    pub total: i64,
    /// The newest article's publication time.
    pub newest: Option<i64>,
}

/// Durable metadata for revalidating one successfully imported representation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FeedCache {
    pub needs_publication_sample: bool,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub body_hash: Option<String>,
    pub retry_after: Option<i64>,
}

#[derive(Model, Clone, Default, PartialEq, Debug)]
#[model(table = "feeds")]
pub struct Feed {
    #[model(id)]
    pub id: u64,
    #[model(unique)]
    pub feed_url: String,
    #[model(index)]
    pub title: String,
    pub custom_title: Option<String>,
    pub sync_account: Option<u64>,
    pub unsubscribed: bool,
    /// `None` is a top-level subscription (no folder).
    pub folder: Option<One<Folder>>,
    pub site_url: Option<String>,
    pub description: Option<String>,
    pub icon_url: Option<String>,
    pub last_fetched_at: Option<i64>,
    pub last_error: Option<String>,
    /// HTTP validators belong to the last successfully stored representation.
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    /// SHA-256 of the last successfully imported raw feed body.
    pub body_hash: Option<String>,
    /// Origin-supplied Retry-After deadline, surviving app restarts.
    pub retry_after: Option<i64>,
    /// Only explicit publication dates, independent of article retention and read state.
    pub publication_history: Option<String>,
    #[model(index)]
    pub next_poll_at: Option<i64>,
    pub poll_failures: u32,
    /// The global feed order: refresh priority and the timeline's feed-group order.
    pub position: f64,
    /// Order within the folder when it sorts [`FeedSort::Manual`]; ties fall back to
    /// `position`.
    pub collection_position: f64,
    #[model(relation(target = Article, inverse = "feed", delete = "cascade"))]
    pub articles: Many<Article>,
}

#[derive(Model, Clone, Default, PartialEq, Debug)]
#[model(
    table = "articles",
    index("feed", "published_at"),
    index("feed", "is_read"),
    index("is_read", "published_at"),
    fts(
        "title",
        "author",
        "summary",
        tokenize = "unicode61 remove_diacritics 2"
    )
)]
pub struct Article {
    /// Estimated words in stored article text, computed once rather than reading bodies for charts.
    #[model(index)]
    pub word_count: Option<u32>,
    #[model(id)]
    pub id: u64,
    pub guid: String,
    pub feed: One<Feed>,
    pub title: Option<String>,
    pub url: Option<String>,
    pub author: Option<String>,
    #[model(index)]
    pub published_at: i64,
    pub summary: Option<String>,
    #[model(index)]
    pub is_read: bool,
    #[model(index)]
    pub is_starred: bool,
    pub first_seen_at: i64,
    /// The body rides in its own row so a timeline window faults metadata only.
    #[model(relation(target = ArticleBody, inverse = "article", delete = "cascade"))]
    pub body: Many<ArticleBody>,
    /// User labels, a many-to-many; membership rows cascade with either side.
    #[model(relation(target = Tag, join = "article_tags"))]
    pub tags: Many<Tag>,
}

/// An article's HTML body: one row per article, keyed by the same id, faulted only when the
/// reader opens it, and searched through its own FTS shadow.
#[derive(Model, Clone, Default, PartialEq, Debug)]
#[model(
    table = "article_bodies",
    fts("content_html", tokenize = "unicode61 remove_diacritics 2")
)]
pub struct ArticleBody {
    #[model(id)]
    pub id: u64,
    pub article: One<Article>,
    pub content_html: String,
}

#[derive(Model, Clone, Default, PartialEq, Debug)]
#[model(table = "tags")]
pub struct Tag {
    #[model(id)]
    pub id: u64,
    #[model(unique)]
    pub name: String,
    #[model(relation(target = Article, join = "article_tags"))]
    pub articles: Many<Article>,
}

// ---------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------

/// FNV-1a over the parts, masked into the integer-key space (the top bit is day-model's
/// interned-handle floor) and steered off 0. Deterministic identity is the dedup mechanism:
/// the same URL or (feed, guid) always names the same row.
fn ident(parts: &[&str]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in parts {
        for b in p.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        h ^= 0x1f; // separator, so ("ab","c") and ("a","bc") differ
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    let h = h & (u64::MAX >> 1);
    if h == 0 { 1 } else { h }
}

pub fn feed_id(feed_url: &str) -> u64 {
    ident(&["feed", feed_url])
}

pub fn article_id(feed_url: &str, guid: &str) -> u64 {
    ident(&["article", feed_url, guid])
}

pub fn folder_id(name: &str) -> u64 {
    ident(&["folder", name])
}

pub fn tag_id(name: &str) -> u64 {
    ident(&["tag", name])
}

// ---------------------------------------------------------------------------
// Scopes and fetches
// ---------------------------------------------------------------------------

/// What the timeline is showing: the sidebar selection, in NetNewsWire's terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Every article from every feed.
    All,
    /// Unread only, across all feeds: NetNewsWire's "All Unread".
    Unread,
    /// Published since local midnight, read or not: NetNewsWire's "Today".
    Today,
    Starred,
    Feed(u64),
    Folder(u64),
    Tag(u64),
}

/// The instant local midnight happened, as unix seconds. Local rather than UTC because "Today"
/// is a claim about the reader's calendar, not the server's; `daynews-time` asks the host for
/// the offset in force, DST and all, on every target this app runs on.
pub fn start_of_today() -> i64 {
    daynews_time::start_of_day(now_unix())
}

/// The scope's predicate alone (without search or sort), which is what count badges share with
/// the timeline.
pub fn scope_pred(scope: Scope) -> Pred {
    let visible = Article::feed().any(Feed::unsubscribed().eq(false));
    visible
        & match scope {
            Scope::All => Pred::Always,
            Scope::Unread => Article::is_read().eq(false),
            Scope::Today => Article::published_at().ge(start_of_today()),
            Scope::Starred => Article::is_starred().eq(true),
            Scope::Feed(id) => Article::feed().is(id),
            Scope::Folder(id) => Article::feed().any(Feed::folder().is(id)),
            Scope::Tag(id) => Article::tags().any(Pred::IdIn(vec![id])),
        }
}

/// The timeline's fetch: the scope, the search text (through both full-text shadows, titles
/// and bodies), newest first, windowed.
pub fn timeline_fetch(scope: Scope, search: &str, limit: usize) -> Fetch {
    let mut f = Fetch::new()
        .filter(scope_pred(scope))
        .sort(Article::published_at().desc())
        .limit(limit);
    let search = search.trim();
    if !search.is_empty() {
        let q = fts_query(search);
        // A body shares its article's id, so the body index's matches are article ids too,
        // and the match is one id list SQLite builds once. Crossing the `body` relation instead
        // compiles to a subquery correlated per article, which made a common word ("Apple", as
        // a word-cloud tap searches for) five times slower than a rare one.
        f = f.filter(
            Article::fts().matches(q.clone())
                | Pred::Raw(
                    "articles.id IN (SELECT rowid FROM article_bodies_fts \
                     WHERE article_bodies_fts MATCH ?)"
                        .into(),
                    vec![Value::Text(q)],
                ),
        );
    }
    f
}

/// Turn user text into an FTS5 query. Everything is quoted as a phrase and the last token gets
/// a prefix `*`, so search feels live as you type. Quoting also means a stray `"` or `-` is
/// searched for rather than being read as FTS syntax and erroring.
pub fn fts_query(input: &str) -> String {
    let tokens: Vec<String> = input
        .split_whitespace()
        .map(|t| t.replace('"', ""))
        .filter(|t| !t.is_empty())
        .collect();
    if tokens.is_empty() {
        return "\"\"".into();
    }
    let last = tokens.len() - 1;
    tokens
        .iter()
        .enumerate()
        .map(|(i, t)| {
            if i == last {
                format!("\"{t}\"*")
            } else {
                format!("\"{t}\"")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------

/// An article as it arrives from a parsed feed, ready to store.
#[derive(Debug, Clone)]
pub struct IncomingArticle {
    pub guid: String,
    pub title: Option<String>,
    pub url: Option<String>,
    pub author: Option<String>,
    pub published: Option<i64>,
    pub summary: Option<String>,
    pub content_html: Option<String>,
}

/// The open store: a [`ModelContainer`] plus the reader's domain operations. The UI reads
/// through live queries ([`Db::timeline`], [`Db::count`], the caches); everything here writes
/// through the front door, so autosave, undo and every query hear it.
pub struct Db {
    pub container: ModelContainer,
}

/// Bound the rows materialized at once by a bulk write, so marking thousands read cannot
/// balloon the cache: each chunk faults, writes, and flushes before the next.
const BULK_CHUNK: usize = 2_000;

/// Explicit SQL diagnostics work in release and debug builds, independently of DAY_LOG.
/// Otherwise retain the normal debug-only trace logging, with no formatting cost when off.
fn traced(driver: Sqlite) -> Sqlite {
    if std::env::var_os("DAY_NEWS_LOG_SQL").is_some() {
        driver.trace_sql(|sql| {
            use std::io::Write;
            // A closed console must not interrupt a database operation.
            let _ = writeln!(std::io::stderr().lock(), "[Fleet-Reader SQL] {sql}");
        })
    } else if cfg!(debug_assertions) && log::log_enabled!(log::Level::Trace) {
        driver.trace_sql(|sql| log::trace!("sql: {sql}"))
    } else {
        driver
    }
}

impl Db {
    /// Open (creating if needed) the store at `path`.
    pub fn open(path: &std::path::Path) -> Result<Db, DbError> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        Self::from_driver(Sqlite::at(path))
    }

    /// An in-memory store, for tests.
    pub fn open_in_memory() -> Result<Db, DbError> {
        Self::from_driver(Sqlite::memory())
    }

    fn from_driver(driver: Sqlite) -> Result<Db, DbError> {
        let models = schema![
            Folder,
            Feed,
            Article,
            ArticleBody,
            Tag,
            sync::Account,
            sync::SubscriptionLink,
            sync::EntryLink,
            sync::Intent
        ];
        #[cfg(all(feature = "experimental-webdav", not(target_arch = "wasm32")))]
        let models = models.with::<day_sync::Bookmark>();
        let container = ModelContainer::open(traced(driver), models)?;
        let db = Db { container };
        db.backfill_word_counts()?;
        // The search index's vocabulary, for the dashboard's word cloud (`Db::word_cloud`): a
        // virtual table that stores nothing of its own, only lists the index's terms with the
        // articles they occur in. Created here, on the writable connection, because the
        // database worker answers reads on a read-only one.
        db.container.try_with_connection(|conn| {
            conn.execute(
                "CREATE VIRTUAL TABLE IF NOT EXISTS articles_fts_terms \
                 USING fts5vocab(articles_fts, instance)",
                &[],
            )
        })?;
        Ok(db)
    }

    /// Migration: only missing metrics read bodies, in bounded batches on the database worker.
    fn backfill_word_counts(&self) -> Result<(), DbError> {
        loop {
            let mut missing = Vec::new();
            self.container.try_with_connection(|conn| conn.query(
                "SELECT a.id, COALESCE(b.content_html, a.summary, '') FROM articles a LEFT JOIN article_bodies b ON b.id = a.id WHERE a.word_count IS NULL LIMIT 128", &[], &mut |row| {
                    if let (Ok(id), Ok(text)) = (row.get(0).as_int(), row.get(1).as_text()) {
                        missing.push((id as u64, dashboard::word_count(text)));
                    }
                }))?;
            if missing.is_empty() {
                break;
            }
            for (id, words) in missing {
                if let Some(article) = self.container.try_get::<Article>(id)? {
                    article.word_count().write(Some(words));
                }
            }
            self.container.save()?;
        }
        Ok(())
    }

    // ---- feeds & folders ------------------------------------------------------------------

    /// Subscribe, or answer the existing subscription: the id is the URL's hash, so
    /// subscribing twice cannot duplicate (and OPML re-imports are naturally idempotent).
    pub fn add_feed(&self, feed_url: &str, title: &str, folder: Option<u64>) -> u64 {
        let id = feed_id(feed_url);
        if let Some(feed) = self.container.get::<Feed>(id) {
            #[cfg(all(feature = "experimental-webdav", not(target_arch = "wasm32")))]
            if feed.sync_account().peek().is_none() && webdav::is_installed(&self.container) {
                feed.unsubscribed().write(false);
            }
            #[cfg(not(all(feature = "experimental-webdav", not(target_arch = "wasm32"))))]
            let _ = feed;
            return id;
        }
        let position = self.next_position::<Feed>(Feed::position());
        self.container.insert(Feed {
            id,
            feed_url: feed_url.to_string(),
            title: title.to_string(),
            folder: folder.map(One::to),
            position,
            ..Default::default()
        });
        id
    }

    /// Create a folder, or answer the one already named `name`. The id is the name's hash,
    /// salted when a renamed folder still holds that hash.
    pub fn add_folder(&self, name: &str) -> u64 {
        if let Some(id) = self.folder_named(name) {
            return id;
        }
        let mut id = folder_id(name);
        let mut salt = 0u32;
        let used = |id| {
            #[cfg(all(feature = "experimental-webdav", not(target_arch = "wasm32")))]
            if webdav::collection_id_used(&self.container, id) {
                return true;
            }
            self.container.get::<Folder>(id).is_some()
        };
        while used(id) {
            salt += 1;
            id = ident(&["folder", name, &salt.to_string()]);
        }
        let position = self.next_position::<Folder>(Folder::position());
        self.container.insert(Folder {
            id,
            name: name.to_string(),
            position,
            ..Default::default()
        });
        id
    }

    /// The folder called `name`, if any.
    pub fn folder_named(&self, name: &str) -> Option<u64> {
        self.container
            .query::<Folder>()
            .filter(Folder::name().eq(name.to_string()))
            .limit(1)
            .live()
            .first()
            .map(|id| id.handle())
    }

    /// Rename a folder. Refuses an empty name and one another folder already has.
    pub fn rename_folder(&self, id: u64, name: &str) -> Result<bool, DbError> {
        let name = name.trim();
        let Some(folder) = self.container.get::<Folder>(id) else {
            return Ok(false);
        };
        if name.is_empty() || self.folder_named(name).is_some_and(|other| other != id) {
            return Ok(false);
        }
        let old = folder.name().peek();
        if old != name {
            folder.name().write(name.to_string());
            self.sync_queue_folder_rename(&old, name)?;
        }
        Ok(true)
    }

    /// Delete a folder. Its feeds stay subscribed and leave the folder first, so the
    /// relation's cascade finds nothing to take with it.
    pub fn delete_folder(&self, id: u64) -> Result<(), DbError> {
        let Some(folder) = self.container.get::<Folder>(id) else {
            return Ok(());
        };
        let name = folder.name().peek();
        for feed in self.folder_feeds(Some(id))? {
            if let Some(row) = self.container.get::<Feed>(feed) {
                row.folder().write(None);
            }
        }
        self.container.delete::<Folder>(id)?;
        self.sync_queue_folder_delete(&name)?;
        Ok(())
    }

    /// Move a folder to `to` in the sidebar's folder order.
    pub fn move_folder(&self, id: u64, to: usize) -> Result<(), DbError> {
        let mut folders: Vec<u64> = self
            .container
            .query::<Folder>()
            .sort(Folder::position().asc())
            .sort(Folder::name().asc())
            .live()
            .try_collect()?
            .into_iter()
            .map(|folder| folder.id)
            .collect();
        let Some(from) = folders.iter().position(|folder| *folder == id) else {
            return Ok(());
        };
        folders.remove(from);
        folders.insert(to.min(folders.len()), id);
        for (index, folder) in folders.into_iter().enumerate() {
            if let Some(row) = self.container.get::<Folder>(folder)
                && row.position().peek() != index as f64
            {
                row.position().write(index as f64);
            }
        }
        Ok(())
    }

    /// Choose how the sidebar orders a folder's feeds.
    pub fn set_folder_sort(&self, id: u64, sort: FeedSort) {
        if let Some(folder) = self.container.get::<Folder>(id)
            && folder.sort().peek() != sort.key()
        {
            folder.sort().write(sort.key().to_string());
        }
    }

    /// Put a feed in a folder (`None`: in none), before `before` in that place's manual order
    /// or at its end. Within its own folder this is a manual reorder; the feeds in no folder
    /// keep a manual order of their own.
    pub fn set_feed_folder(
        &self,
        feed: u64,
        folder: Option<u64>,
        before: Option<u64>,
    ) -> Result<(), DbError> {
        let Some(row) = self.container.get::<Feed>(feed) else {
            return Ok(());
        };
        if folder.is_some_and(|id| self.container.get::<Folder>(id).is_none()) {
            return Ok(());
        }
        let current = row
            .folder()
            .peek()
            .and_then(|f| f.id())
            .map(|id| id.handle());
        if current != folder {
            row.folder().write(folder.map(One::to));
            let name = folder
                .and_then(|id| self.container.get::<Folder>(id))
                .map(|f| f.name().peek());
            self.sync_queue_feed_folder(feed, name.as_deref())?;
        }
        // Renumber the manual order with the feed in its new place.
        let mut order: Vec<u64> = self
            .folder_feeds(folder)?
            .into_iter()
            .filter(|id| *id != feed)
            .collect();
        let at = before
            .and_then(|b| order.iter().position(|id| *id == b))
            .unwrap_or(order.len());
        order.insert(at, feed);
        for (index, id) in order.into_iter().enumerate() {
            if let Some(row) = self.container.get::<Feed>(id)
                && row.collection_position().peek() != index as f64
            {
                row.collection_position().write(index as f64);
            }
        }
        Ok(())
    }

    /// A folder's feeds (`None`: the subscribed feeds in no folder), in its manual order.
    fn folder_feeds(&self, folder: Option<u64>) -> Result<Vec<u64>, DbError> {
        let query = self.container.query::<Feed>();
        let query = match folder {
            Some(folder) => query.filter(Feed::folder().is(folder)),
            None => query.filter(Feed::unsubscribed().eq(false)),
        };
        Ok(query
            .sort(Feed::collection_position().asc())
            .sort(Feed::position().asc())
            .live()
            .try_collect()?
            .into_iter()
            .filter(|feed| folder.is_some() || feed.folder.is_none())
            .map(|feed| feed.id)
            .collect())
    }

    /// Write folder positions in one go: the sidebar's collection order, with gaps where the
    /// app places something of its own (the feeds in no folder).
    pub fn set_folder_positions(&self, positions: &[(u64, f64)]) {
        for (id, position) in positions {
            if let Some(row) = self.container.get::<Folder>(*id)
                && row.position().peek() != *position
            {
                row.position().write(*position);
            }
        }
    }

    /// Unread, total and newest-article figures for every feed with articles.
    pub fn feed_stats(&self) -> Result<std::collections::HashMap<u64, FeedStats>, DbError> {
        let mut stats = std::collections::HashMap::new();
        self.container.try_with_connection(|conn| {
            conn.query(
                "SELECT feed, SUM(is_read = 0), COUNT(*), MAX(published_at) FROM articles GROUP BY feed",
                &[],
                &mut |row| {
                    if let Ok(id) = row.get(0).as_int() {
                        stats.insert(
                            id as u64,
                            FeedStats {
                                unread: row.get(1).as_int().unwrap_or(0),
                                total: row.get(2).as_int().unwrap_or(0),
                                newest: row.get(3).as_int().ok(),
                            },
                        );
                    }
                },
            )
        })?;
        Ok(stats)
    }

    /// The subscribed feeds already holding articles at any of `links`, with how many of them
    /// each holds, most first: how the subscription sheet recognizes a feed it is about to add
    /// twice under another address.
    pub fn feeds_with_links(&self, links: &[String]) -> Result<Vec<(u64, usize)>, DbError> {
        let links: Vec<Value> = links
            .iter()
            .filter(|l| !l.is_empty())
            .take(500)
            .map(|l| Value::Text(l.clone()))
            .collect();
        let mut found = Vec::new();
        if links.is_empty() {
            return Ok(found);
        }
        let marks = vec!["?"; links.len()].join(",");
        self.container.try_with_connection(|conn| {
            conn.query(
                &format!(
                    "SELECT a.feed, COUNT(DISTINCT a.url) FROM articles a JOIN feeds f ON f.id = a.feed \
                     WHERE f.unsubscribed = 0 AND a.url IN ({marks}) GROUP BY a.feed ORDER BY 2 DESC"
                ),
                &links,
                &mut |row| {
                    if let (Ok(feed), Ok(count)) = (row.get(0).as_int(), row.get(1).as_int()) {
                        found.push((feed as u64, count as usize));
                    }
                },
            )
        })?;
        Ok(found)
    }

    /// Create a tag (idempotent by name).
    pub fn add_tag(&self, name: &str) -> u64 {
        let id = tag_id(name);
        if self.container.get::<Tag>(id).is_none() {
            self.container.insert(Tag {
                id,
                name: name.to_string(),
                ..Default::default()
            });
        }
        id
    }

    /// One past the current largest `position`, so new rows land last.
    fn next_position<M: day_persistence::Model>(&self, col: day_persistence::Col<f64>) -> f64 {
        let last = self
            .container
            .query::<M>()
            .sort(col.desc())
            .limit(1)
            .live()
            .first();
        let Some(id) = last else { return 1.0 };
        if self.container.get::<M>(id).is_none() {
            return 1.0;
        }
        // Read the position through the cache row the fault just landed.
        let mut v = 0.0;
        self.container.cache::<M>().with_untracked(|k| {
            if let Some(m) = k.get(id.handle())
                && let Some(p) = day_model::ApplyField::read_field(m, col.field)
                && let Some(p) = p.downcast_ref::<f64>()
            {
                v = *p;
            }
        });
        v + 1.0
    }

    /// Record what a refresh learned about the channel itself.
    pub fn update_feed_metadata(
        &self,
        id: u64,
        title: Option<&str>,
        site_url: Option<&str>,
        description: Option<&str>,
        icon_url: Option<&str>,
    ) {
        let Some(feed) = self.container.get::<Feed>(id) else {
            return;
        };
        // Only overwrite the title when the feed supplied one, so a subscription
        // named by hand (or by its URL) is not blanked by a feed with an empty <title>.
        if feed.custom_title().peek().is_none()
            && let Some(t) = title.filter(|t| !t.trim().is_empty())
            && feed.title().peek() != t
        {
            feed.title().write(t.to_string());
        }
        let site_url = site_url.map(str::to_string);
        if feed.site_url().peek() != site_url {
            feed.site_url().write(site_url);
        }
        let description = description.map(str::to_string);
        if feed.description().peek() != description {
            feed.description().write(description);
        }
        let icon_url = icon_url.map(str::to_string);
        if feed.icon_url().peek() != icon_url {
            feed.icon_url().write(icon_url);
        }
        let checked = Some(now());
        if feed.last_fetched_at().peek() != checked {
            feed.last_fetched_at().write(checked);
        }
        if feed.last_error().peek().is_some() {
            feed.last_error().write(None);
        }
    }

    /// Record a failed refresh so the sidebar can show the feed as broken.
    pub fn set_feed_error(&self, id: u64, message: &str) {
        if let Some(feed) = self.container.get::<Feed>(id) {
            let failures = feed.poll_failures().peek().saturating_add(1);
            feed.poll_failures().write(failures);
            let delay = (1800_i64 * (1_i64 << failures.min(6))).min(86400);
            feed.next_poll_at().write(Some(
                now()
                    .saturating_add(delay)
                    .max(feed.retry_after().peek().unwrap_or(0)),
            ));
            let error = Some(message.to_string());
            if feed.last_error().peek() != error {
                feed.last_error().write(error);
            }
            let checked = Some(now());
            if feed.last_fetched_at().peek() != checked {
                feed.last_fetched_at().write(checked);
            }
        }
    }

    /// Finish a successful conditional check without rewriting article bodies or read state.
    pub fn feed_checked(&self, id: u64, etag: Option<String>, last_modified: Option<String>) {
        self.set_feed_retry_after(id, None);
        if let Some(feed) = self.container.get::<Feed>(id) {
            if feed.poll_failures().peek() != 0 {
                feed.poll_failures().write(0);
            }
            let next = now().saturating_add(polling::interval(
                feed.publication_history().peek().as_deref(),
                now(),
            ));
            if feed.next_poll_at().peek() != Some(next) {
                feed.next_poll_at().write(Some(next));
            }
            if feed.etag().peek() != etag {
                feed.etag().write(etag);
            }
            if feed.last_modified().peek() != last_modified {
                feed.last_modified().write(last_modified);
            }
            let checked = Some(now());
            if feed.last_fetched_at().peek() != checked {
                feed.last_fetched_at().write(checked);
            }
            if feed.last_error().peek().is_some() {
                feed.last_error().write(None);
            }
        }
    }

    pub fn feed_validators(&self, id: u64) -> Option<(Option<String>, Option<String>)> {
        self.container
            .get::<Feed>(id)
            .map(|feed| (feed.etag().peek(), feed.last_modified().peek()))
    }

    /// HTTP cache metadata is read together before a request.
    pub fn feed_cache(&self, id: u64) -> Option<FeedCache> {
        self.container.get::<Feed>(id).map(|feed| FeedCache {
            needs_publication_sample: feed.publication_history().peek().is_none(),
            etag: feed.etag().peek(),
            last_modified: feed.last_modified().peek(),
            body_hash: feed.body_hash().peek(),
            retry_after: feed.retry_after().peek(),
        })
    }

    pub fn set_feed_retry_after(&self, id: u64, until: Option<i64>) {
        if let Some(feed) = self.container.get::<Feed>(id)
            && feed.retry_after().peek() != until
        {
            feed.retry_after().write(until);
        }
    }

    pub fn set_feed_body_hash(&self, id: u64, hash: Option<String>) {
        if let Some(feed) = self.container.get::<Feed>(id)
            && feed.body_hash().peek() != hash
        {
            feed.body_hash().write(hash);
        }
    }

    pub fn record_publications(&self, id: u64, dates: impl IntoIterator<Item = i64>) {
        if let Some(feed) = self.container.get::<Feed>(id) {
            let previous = feed.publication_history().peek();
            let history = polling::publication_history(previous.as_deref(), dates, now());
            if previous.as_deref() != Some(&history) {
                feed.publication_history().write(Some(history));
            }
        }
    }

    /// Only due subscriptions; publication sampling happens on import, never on every tick.
    pub fn due_feeds(&self, at: i64) -> Result<Vec<(u64, String)>, day_persistence::DbError> {
        Ok(self
            .container
            .query::<Feed>()
            .filter(
                Feed::sync_account().is_null()
                    & Feed::unsubscribed().eq(false)
                    & (Feed::next_poll_at().is_null() | Feed::next_poll_at().le(Some(at))),
            )
            .sort(Feed::position().asc())
            .sort(Feed::title().asc())
            .live()
            .try_collect()?
            .into_iter()
            .filter(|feed| feed.retry_after.is_none_or(|until| until <= at))
            .map(|feed| (feed.id, feed.feed_url))
            .collect())
    }

    /// Move one subscription in the shared global priority order. Keep folders intact.
    pub fn move_feed(&self, id: u64, to: usize) -> Result<(), day_persistence::DbError> {
        let mut feeds = self
            .container
            .query::<Feed>()
            .sort(Feed::position().asc())
            .sort(Feed::title().asc())
            .live()
            .try_collect()?;
        let Some(from) = feeds.iter().position(|feed| feed.id == id) else {
            return Ok(());
        };
        let feed = feeds.remove(from);
        let to = to.min(feeds.len());
        feeds.insert(to, feed);
        for (index, feed) in feeds.into_iter().enumerate() {
            if feed.position != index as f64
                && let Some(row) = self.container.get::<Feed>(feed.id)
            {
                row.position().write(index as f64);
            }
        }
        Ok(())
    }

    pub fn rename_feed(&self, id: u64, title: &str) -> Result<(), DbError> {
        if let Some(feed) = self.container.get::<Feed>(id) {
            feed.title().write(title.to_string());
            feed.custom_title().write(Some(title.to_string()));
            self.sync_queue_rename(id, title)?;
        }
        Ok(())
    }

    /// Unsubscribe. Articles, bodies and tag memberships go with the feed (the cascade),
    /// and with an undo stack installed the whole subtree comes back as one unit.
    pub fn delete_feed(&self, id: u64) -> Result<(), DbError> {
        #[cfg(all(feature = "experimental-webdav", not(target_arch = "wasm32")))]
        if webdav::is_installed(&self.container)
            && let Some(feed) = self.container.get::<Feed>(id)
            && feed.sync_account().peek().is_none()
        {
            feed.unsubscribed().write(true);
            return Ok(());
        }
        if !self.sync_unsubscribe(id)? {
            self.container.delete::<Feed>(id)?;
        }
        Ok(())
    }

    // ---- articles -------------------------------------------------------------------------

    /// Store a refresh's items for one feed. Returns how many were new; an item already
    /// present is left completely alone, which is what preserves read state (ids are the
    /// (feed, guid) hash, so existence is one id-set query, no faulting).
    pub fn upsert_articles(&self, feed: u64, feed_url: &str, items: &[IncomingArticle]) -> usize {
        self.try_upsert_articles(feed, feed_url, items)
            .unwrap_or_default()
    }

    /// Checked import for asynchronous transactions. A failed existence query must abort
    /// the import rather than treating stored articles as new and overwriting read state.
    pub fn try_upsert_articles(
        &self,
        feed: u64,
        feed_url: &str,
        items: &[IncomingArticle],
    ) -> Result<usize, day_persistence::DbError> {
        if items.is_empty() {
            return Ok(0);
        }
        let seen = now();
        let ids: Vec<u64> = items
            .iter()
            .map(|i| article_id(feed_url, &i.guid))
            .collect();
        let mut existing: std::collections::HashSet<u64> = self
            .container
            .query::<Article>()
            .filter(Pred::IdIn(ids.clone()))
            .live()
            .try_ids()?
            .iter()
            .map(|i| i.handle())
            .collect();
        let mut added = 0usize;
        for (item, id) in items.iter().zip(&ids) {
            if !existing.insert(*id) {
                continue;
            }
            self.container.insert(Article {
                id: *id,
                guid: item.guid.clone(),
                feed: One::to(feed),
                title: item.title.clone(),
                url: item.url.clone(),
                author: item.author.clone(),
                // Undated items sort by when we first saw them, so they do not all pile up
                // at the epoch.
                published_at: item.published.unwrap_or(seen),
                summary: item.summary.clone(),
                first_seen_at: seen,
                word_count: Some(dashboard::word_count(
                    item.content_html
                        .as_deref()
                        .or(item.summary.as_deref())
                        .unwrap_or(""),
                )),
                ..Default::default()
            });
            if let Some(html) = item.content_html.clone().filter(|h| !h.is_empty()) {
                self.container.insert(ArticleBody {
                    id: *id,
                    article: One::to(*id),
                    content_html: html,
                });
            }
            added += 1;
        }
        Ok(added)
    }

    /// One article's body, faulted on open; `None` when the item shipped none.
    pub fn body(&self, article: u64) -> Option<String> {
        self.container
            .get::<ArticleBody>(article)
            .map(|b| b.content_html().peek())
    }

    pub fn set_read(&self, article: u64, read: bool) -> Result<(), DbError> {
        if let Some(a) = self.container.try_get::<Article>(article)?
            && a.is_read().peek() != read
        {
            a.is_read().write(read);
            self.sync_queue_read(article, read)?;
        }
        Ok(())
    }

    /// Mark cached articles published strictly before `cutoff` read. Use inside a worker
    /// write so reading choices and provider/sync intents commit atomically.
    pub fn set_read_before(&self, cutoff: i64) -> Result<usize, DbError> {
        let ids = self
            .container
            .query::<Article>()
            .filter(Article::published_at().lt(cutoff) & Article::is_read().eq(false))
            .live()
            .try_ids()?;
        for id in &ids {
            self.set_read(id.handle(), true)?;
        }
        Ok(ids.len())
    }

    pub fn set_starred(&self, article: u64, starred: bool) {
        if let Some(a) = self.container.get::<Article>(article) {
            a.is_starred().write(starred);
        }
    }

    /// Tag or untag one article: a join-row link, one INSERT or DELETE.
    pub fn set_tagged(&self, article: u64, tag: u64, on: bool) {
        let Some(a) = self.container.get::<Article>(article) else {
            return;
        };
        if on {
            a.tags().add(tag);
        } else {
            a.tags().remove(tag);
        }
    }

    /// Capture exactly the current scope; a later arriving article is not part of this edit.
    /// The worker commits article state and outbox together in its enclosing transaction.
    pub fn set_read_all(&self, scope: Scope, read: bool) -> Result<usize, DbError> {
        let ids = self
            .container
            .query::<Article>()
            .filter(scope_pred(scope) & Article::is_read().eq(!read))
            .live()
            .try_ids()?;
        for id in &ids {
            self.set_read(id.handle(), read)?;
        }
        Ok(ids.len())
    }

    /// Delete read, unstarred, untagged articles older than `days`: the retention pass.
    /// Starred and tagged articles are the user's; they stay whatever their age.
    /// Age is the publication date (arrival date for undated items), not time spent
    /// on this installation: importing history must not restart its retention clock.
    pub fn prune_older_than(&self, days: u32) -> usize {
        if days == 0 {
            return 0;
        }
        let cutoff = now() - i64::from(days) * 86_400;
        let doomed: Vec<ModelId<Article>> = self
            .container
            .query::<Article>()
            .filter(
                Article::is_read().eq(true)
                    & Article::is_starred().eq(false)
                    & Article::published_at().lt(cutoff)
                    & Article::tags().is_empty(),
            )
            .live()
            .ids();
        let n = doomed.len();
        let store = self.container.cache::<Article>();
        for chunk in doomed.chunks(BULK_CHUNK) {
            for id in chunk {
                let h = id.handle();
                store.restructure("prune", Op::Delete, h, |k| {
                    k.remove(h);
                });
            }
            let _ = self.container.save();
        }
        n
    }

    // ---- the live surface -----------------------------------------------------------------

    /// The timeline query for a fixed scope/search (the app's reactive form composes
    /// [`timeline_fetch`] with `query_fn` itself).
    pub fn timeline(&self, scope: Scope, search: &str, limit: usize) -> Query<Article> {
        self.container
            .query::<Article>()
            .filter(timeline_fetch(scope, search, limit).pred)
            .sort(Article::published_at().desc())
            .limit(limit)
            .live()
    }

    /// A live badge for a scope: `SELECT COUNT(*)`, no ids.
    pub fn count(&self, scope: Scope) -> CountQuery<Article> {
        self.container
            .query::<Article>()
            .filter(scope_pred(scope))
            .live_count()
    }

    /// A live unread badge for a scope.
    pub fn unread_count(&self, scope: Scope) -> CountQuery<Article> {
        self.container
            .query::<Article>()
            .filter(scope_pred(scope) & Article::is_read().eq(false))
            .live_count()
    }
}

fn now() -> i64 {
    now_unix()
}

/// The wall clock as unix seconds, everywhere the app runs. `daynews-time` rather than
/// `SystemTime::now()`, which aborts on wasm32; on web this is the page's `Date.now()`.
pub fn now_unix() -> i64 {
    daynews_time::now_unix()
}

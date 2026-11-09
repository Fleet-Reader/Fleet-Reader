// SPDX-License-Identifier: Apache-2.0
//! The app's view-model: opens the container, wires live queries, and publishes what the UI
//! renders as reactive signals, every one of them derived. There is no reload call anywhere:
//! a write lands in the store, the affected queries re-derive, the standing effects rebuild
//! exactly the signals whose sources moved, and the UI follows. Undo, a background refresh,
//! and a menu command all reach the screen through the same road.
//!
//! Single-threaded. day's reactive core is `!Send` and its executor (`day::task`)
//! polls futures on the UI thread, so the container lives here in a `RefCell` and every
//! mutation happens between awaits, without marshaling or locks. Network I/O is the only
//! off-thread part, and the HTTP part hands the response back on the main thread.

use std::cell::{OnceCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use day_core::Ambient;
use day_persistence::{CountQuery, Query};
use day_reactive::{Effect, Scope as RScope, Signal, batch, watch};
use daynews_db::{
    Article, ArticleFields, ArticleRelations, Db, FeedFields, FolderFields, IncomingArticle, Scope,
    TagFields, timeline_fetch,
};
use futures_util::{StreamExt, stream};

pub use daynews_db::{FeedSort, Scope as TimelineScope, article_id, feed_id, folder_id, tag_id};

/// How many articles the timeline shows at once. The query is windowed (`LIMIT`), so this caps
/// what the UI materializes, not what the store holds.
const TIMELINE_LIMIT: usize = 500;

/// How deep the undo history goes.
const UNDO_LEVELS: usize = 200;

/// The retention default: prune read, unstarred, untagged articles older than this many days.
/// `0` means keep everything; the app stores the user's choice in its preferences.
pub const DEFAULT_RETENTION_DAYS: u32 = 90;

/// A sidebar row: the feed plus the badge count the UI draws.
#[derive(Debug, Clone, PartialEq)]
pub struct FeedRow {
    pub id: u64,
    pub title: String,
    pub unread: i64,
    pub folder_id: Option<u64>,
    pub has_error: bool,
    pub site_url: Option<String>,
    pub feed_url: String,
    pub icon_url: Option<String>,
    /// When the feed was last checked, whether or not it had anything new.
    pub last_fetched: Option<i64>,
    /// Stored articles, read or not.
    pub total: i64,
    /// The newest article's publication time.
    pub newest: Option<i64>,
    /// Place in its collection's manual order.
    pub collection_position: f64,
}

/// A collection: the store's folder, with how the sidebar orders its feeds.
#[derive(Debug, Clone, PartialEq)]
pub struct FolderRow {
    pub id: u64,
    pub name: String,
    pub sort: FeedSort,
    /// Place in the sidebar's collection order.
    pub position: f64,
}

/// A sidebar tag row with its article count.
#[derive(Debug, Clone, PartialEq)]
pub struct TagRow {
    pub id: u64,
    pub name: String,
    pub count: i64,
}

/// A timeline row. Excludes the body: it lives in its own model
/// (`ArticleBody`) and faults in only when the reader opens the article.
#[derive(Debug, Clone, PartialEq)]
pub struct ArticleSummary {
    pub id: u64,
    pub feed_id: u64,
    pub feed_title: String,
    pub title: Option<String>,
    pub url: Option<String>,
    pub author: Option<String>,
    pub published_at: i64,
    pub summary: Option<String>,
    pub is_read: bool,
    pub is_starred: bool,
}

/// A full article, body included, for the reader pane.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredArticle {
    pub id: u64,
    pub feed_id: u64,
    pub feed_title: String,
    pub title: Option<String>,
    pub url: Option<String>,
    pub author: Option<String>,
    pub published_at: i64,
    pub summary: Option<String>,
    pub content_html: Option<String>,
    pub is_read: bool,
    pub is_starred: bool,
}

pub use daynews_opml as opml;

/// The app's data (docs/state.md): the subscription tree, the smart-feed badges, and the state
/// of a refresh. One database, one set of feeds, one unread count: a second window is another
/// view of the same reader, not a second reader.
#[derive(Clone, Copy)]
pub struct SheetsState {
    pub feeds: Signal<Vec<FeedRow>>,
    pub group_by_feed: Signal<bool>,
    pub folders: Signal<Vec<FolderRow>>,
    pub tags: Signal<Vec<TagRow>>,
    /// `Some((done, total))` while a refresh runs: the progress NetNewsWire shows in its bar.
    pub refresh_progress: Signal<Option<(usize, usize)>>,
    /// Feeds with an active request, shared by every window.
    pub updating_feeds: Signal<HashMap<u64, Option<f64>>>,
    pub total_unread: Signal<i64>,
    pub total_starred: Signal<i64>,
    pub total_today: Signal<i64>,
    pub total_articles: Signal<i64>,
    /// A short transient message ("Imported 145 feeds", "3 feeds failed").
    pub status: Signal<String>,
}

/// Everything one window is looking at (docs/state.md): its sidebar scope, its search text, the
/// timeline those two produce, and the article it has open.
///
/// Per-window because that is what a second window is for: one on Unread while another sits in
/// a folder, each with its own selection and its own reader. The timeline query behind it is
/// per-window too: `create` stands up one live query and the effects that publish from it, and
/// those effects hold it (the container keeps a live query weakly, so a dropped one goes quiet).
#[derive(Clone, Copy)]
pub struct NewsScene {
    pub scope: Signal<Scope>,
    pub articles: Signal<Vec<ArticleSummary>>,
    /// Unread count for the whole scope, including rows outside the timeline window.
    pub scope_unread: Signal<usize>,
    /// The open article's id, and its loaded body.
    pub selected: Signal<Option<u64>>,
    /// Two-way native timeline focus, requested by keyboard reading commands.
    pub timeline_focused: Signal<bool>,
    pub article: Signal<Option<StoredArticle>>,
    /// Whether the reader is showing: the selector's `detail_visible` binding. On a phone
    /// this is the push gate for the reader page, and the platform's back writes it false;
    /// wide layouts keep the reader pane on screen and ignore it.
    pub reader_open: Signal<bool>,
    pub search: Signal<String>,
    /// Articles marked read while the unread scope shows them: they stay visible (their dot
    /// clears in place) until the scope or search changes, which is NetNewsWire's rule, or a
    /// phone's list closes. The timeline fetch ORs these ids back into the unread predicate.
    sticky_read: Signal<Vec<u64>>,
}

/// The open store and the query handles derived from it, one per process, held for the app's
/// lifetime. Not app state (nothing here is a signal an app reads): a database connection, an
/// undo history, and the count queries whose badges feed `SheetsState`. `Ambient::app` owns the
/// signals; this owns the resources behind them.
struct Store {
    db: RefCell<Option<Db>>,
    undo: OnceCell<day_model::UndoStack>,
    /// Per-feed and per-tag unread badges, created on first sight and reused; each is one
    /// live `SELECT COUNT(*)` that re-runs only when a change touches its dependency set.
    feed_counts: RefCell<HashMap<u64, CountQuery<Article>>>,
    tag_counts: RefCell<HashMap<u64, CountQuery<Article>>>,
    totals: OnceCell<[CountQuery<Article>; 2]>,
    refresh_slots: [futures_util::lock::Mutex<()>; 4],
    scheduled_feeds: RefCell<HashSet<u64>>,
}

#[derive(Clone)]
struct StoreHandle(std::rc::Rc<Store>);

impl Ambient for StoreHandle {
    fn create() -> Self {
        StoreHandle(std::rc::Rc::new(Store {
            db: RefCell::new(None),
            undo: OnceCell::new(),
            feed_counts: RefCell::new(HashMap::new()),
            tag_counts: RefCell::new(HashMap::new()),
            totals: OnceCell::new(),
            refresh_slots: std::array::from_fn(|_| futures_util::lock::Mutex::new(())),
            scheduled_feeds: RefCell::new(HashSet::new()),
        }))
    }
}

/// The process's one store handle. App-scoped like everything else here (docs/state.md), so
/// there is no `thread_local!` left in this crate at all.
fn store() -> std::rc::Rc<Store> {
    StoreHandle::app().0
}

impl Ambient for SheetsState {
    /// Created on the reactive root scope by `Ambient::app`, which is what the detached scope
    /// this replaces existed for: it outlives any UI subtree.
    fn create() -> Self {
        SheetsState {
            feeds: Signal::new(Vec::new()),
            group_by_feed: Signal::new(true),
            folders: Signal::new(Vec::new()),
            tags: Signal::new(Vec::new()),
            refresh_progress: Signal::new(None),
            updating_feeds: Signal::new(HashMap::new()),
            total_unread: Signal::new(0),
            total_starred: Signal::new(0),
            total_today: Signal::new(0),
            total_articles: Signal::new(0),
            status: Signal::new(String::new()),
        }
    }
}

/// The app's data. One reader, one database, one set of badges.
pub fn state() -> SheetsState {
    SheetsState::app()
}

impl Ambient for NewsScene {
    /// One window's view. Stands up this window's timeline query and the effects that publish
    /// from it; the effects capture the query, which is what keeps it subscribed (the container
    /// holds a live query weakly).
    fn create() -> Self {
        let scene = NewsScene {
            scope: Signal::new(Scope::Unread),
            articles: Signal::new(Vec::new()),
            scope_unread: Signal::new(0),
            selected: Signal::new(None),
            timeline_focused: Signal::new(false),
            article: Signal::new(None),
            reader_open: Signal::new(false),
            search: Signal::new(String::new()),
            sticky_read: Signal::new(Vec::new()),
        };
        wire_scene(scene);
        scene
    }
}

/// The window whose view a call belongs to: the ambient one while a piece builds, the focused
/// window's when a command runs later from a handler that belongs to no scope (docs/state.md).
pub fn scene() -> NewsScene {
    try_scene().expect("no window is open, so there is no NewsScene to act on")
}

/// App menus are evaluated before the first window and after the last one closes.
pub fn try_scene() -> Option<NewsScene> {
    NewsScene::try_ambient().or_else(NewsScene::focused)
}

/// Stand up one window's timeline: the live query that follows its scope + search, and the
/// effects that publish rows and the open article from it.
fn wire_scene(sc: NewsScene) {
    // No store (open failed): the window still builds, empty.
    let Some(timeline) = with_db(|db| {
        db.container.query_fn::<Article>(move || {
            let mut fetch = timeline_fetch(sc.scope.get(), &sc.search.get(), TIMELINE_LIMIT);
            let sticky = sc.sticky_read.get();
            if sc.scope.get() == Scope::Unread && !sticky.is_empty() {
                // Re-admit the rows read under the cursor, so they clear in place instead of
                // vanishing (the fetch rebuilds from scratch, so this replaces the filter).
                fetch = timeline_fetch(Scope::All, &sc.search.get(), TIMELINE_LIMIT);
                fetch.pred = fetch.pred
                    & (daynews_db::scope_pred(Scope::Unread) | day_persistence::Pred::IdIn(sticky));
            }
            fetch
        })
    }) else {
        return;
    };

    if let Some(unread) = with_db(|db| {
        db.container.count_fn::<Article>(move || {
            day_persistence::Fetch::new()
                .filter(daynews_db::scope_pred(sc.scope.get()) & Article::is_read().eq(false))
        })
    }) {
        Effect::new(move || sc.scope_unread.set(unread.get()));
    }
    // The native search field binds directly to this window's query. Changing it starts
    // a fresh unread view, including when Next Unread clears a previous search.
    watch(
        move || sc.search.get(),
        move |_, _| sc.sticky_read.set(Vec::new()),
    );

    // Timeline rows: ids from the query, fields read tracked so an edit to a visible row
    // (a star, a read dot) rebuilds exactly this list. The closure holds `timeline`.
    Effect::new(move || {
        let mut rows = build_summaries(&timeline);
        if state().group_by_feed.get() {
            let order: HashMap<_, _> = state()
                .feeds
                .get()
                .iter()
                .enumerate()
                .map(|(i, f)| (f.id, i))
                .collect();
            rows.sort_by_key(|a| order.get(&a.feed_id).copied().unwrap_or(usize::MAX));
        }
        sc.articles.set(rows);
    });

    // The reader: whatever article is selected, kept current as its fields change (a
    // star from the toolbar repaints the open article without any hand patching).
    Effect::new(move || {
        let article = sc.selected.get().and_then(build_reader_article);
        sc.article.set(article);
    });

    // Closing the reader (the platform's back on a phone) drops the selection with it:
    // the row un-highlights, and reopening starts from the list.
    watch(
        move || sc.reader_open.get(),
        move |open, _| {
            if !open {
                sc.selected.set(None);
            }
        },
    );
}

/// Open the container and stand up the live pipeline. Call once at startup, before the first
/// build.
pub fn init() {
    match Db::open(&store_dir().join(crate::LIBRARY)) {
        Ok(db) => {
            store_with(|s| *s.db.borrow_mut() = Some(db));
            store_with(|s| {
                let _ = s
                    .undo
                    .set(with_db(|db| db.container.undo(UNDO_LEVELS)).expect("db just set"));
            });
            wire_app_state();
        }
        Err(e) => state()
            .status
            .set(format!("Could not open the article store: {e}")),
    }
}

/// Run `f` against the store. A closed store (open failed) makes this a no-op, so the UI keeps
/// working, empty, rather than panicking.
fn with_db<R>(f: impl FnOnce(&Db) -> R) -> Option<R> {
    store_with(|s| s.db.borrow().as_ref().map(f))
}

/// The container's undo history; `day::install_undo` wires it to the platform.
pub fn undo_stack() -> Option<day_model::UndoStack> {
    store_with(|s| s.undo.get().cloned())
}

// ---- the live pipeline ----------------------------------------------------------------------

/// Stand up the standing effects that derive every published signal from live queries.
/// Stand up the app-wide standing effects: the subscription tree and the smart-feed badges.
/// A window's own timeline is `wire_scene`, run once per window.
fn wire_app_state() {
    let st = state();
    // On the root scope: these outlive every window (the detached scope this replaces existed
    // for the same reason).
    let scope = RScope::root();

    // Totals: unread and starred are plain count queries; Today re-derives on scope of its
    // predicate (its midnight cutoff re-evaluates whenever the count query re-derives, and a
    // refresh nudges it across midnight).
    let (unread_total, starred_total) =
        with_db(|db| (db.unread_count(Scope::All), db.count(Scope::Starred))).expect("open");
    store_with(|s| {
        let _ = s.totals.set([unread_total.clone(), starred_total.clone()]);
    });
    let articles_total = with_db(|db| db.count(Scope::All)).expect("open");
    let today_total = with_db(|db| db.unread_count(Scope::Today)).expect("open");

    // The sidebar lists are standing queries too: the container holds a live query weakly, so
    // one created inside an effect run and dropped at its end takes the subscription with it
    // and the list goes quiet. These live for the session, like the timeline's.
    let (feeds_q, folders_q, tags_q) = with_db(|db| {
        (
            db.container
                .query::<daynews_db::Feed>()
                .sort(daynews_db::Feed::position().asc())
                .sort(daynews_db::Feed::title().asc())
                .live(),
            db.container
                .query::<daynews_db::Folder>()
                .sort(daynews_db::Folder::position().asc())
                .sort(daynews_db::Folder::name().asc())
                .live(),
            db.container
                .query::<daynews_db::Tag>()
                .sort(daynews_db::Tag::name().asc())
                .live(),
        )
    })
    .expect("open");

    scope.enter(|| {
        // Sidebar: feeds with their badges, folders, tags with theirs.
        Effect::new(move || {
            let rows = build_feed_rows(&feeds_q);
            st.feeds.set(rows);
        });
        Effect::new(move || {
            let rows = build_folder_rows(&folders_q);
            st.folders.set(rows);
        });
        Effect::new(move || {
            let rows = build_tag_rows(&tags_q);
            st.tags.set(rows);
        });

        // The smart-feed badges.
        Effect::new(move || st.total_unread.set(unread_total.get() as i64));
        Effect::new(move || st.total_starred.set(starred_total.get() as i64));
        Effect::new(move || st.total_today.set(today_total.get() as i64));
        Effect::new(move || st.total_articles.set(articles_total.get() as i64));
    });
}

fn build_summaries(q: &Query<Article>) -> Vec<ArticleSummary> {
    let Some(db) = store_with(|s| s.db.borrow().as_ref().map(|d| d.container.clone())) else {
        return Vec::new();
    };
    let ids = q.ids();
    let keys: Vec<u64> = ids.iter().map(|i| i.handle()).collect();
    let _ = db.ensure_resident::<Article>(&keys);
    let articles = db.cache::<Article>();
    // The window's feeds too, for the footer titles.
    let feed_keys: Vec<u64> = articles.with_untracked(|k| {
        keys.iter()
            .filter_map(|id| k.get(*id).and_then(|a| a.feed.id()))
            .map(|i| i.handle())
            .collect()
    });
    let _ = db.ensure_resident::<daynews_db::Feed>(&feed_keys);
    let feeds = db.cache::<daynews_db::Feed>();

    keys.iter()
        .filter_map(|id| {
            let a = articles.elem(*id);
            if !a.exists() {
                return None;
            }
            let feed_key = a
                .feed()
                .with(|f| f.and_then(|f| f.id()))
                .map(|i| i.handle());
            let feed_title = feed_key
                .map(|fk| {
                    feeds
                        .elem(fk)
                        .title()
                        .with(|t| t.cloned().unwrap_or_default())
                })
                .unwrap_or_default();
            Some(ArticleSummary {
                id: *id,
                feed_id: feed_key.unwrap_or(0),
                feed_title,
                title: a.title().with(|v| v.cloned().flatten()),
                url: a.url().with(|v| v.cloned().flatten()),
                author: a.author().with(|v| v.cloned().flatten()),
                published_at: a.published_at().with(|v| v.copied().unwrap_or(0)),
                summary: a.summary().with(|v| v.cloned().flatten()),
                is_read: a.is_read().with(|v| v.copied().unwrap_or(false)),
                is_starred: a.is_starred().with(|v| v.copied().unwrap_or(false)),
            })
        })
        .collect()
}

fn build_feed_rows(q: &Query<daynews_db::Feed>) -> Vec<FeedRow> {
    let Some(db) = store_with(|s| s.db.borrow().as_ref().map(|d| d.container.clone())) else {
        return Vec::new();
    };
    let ids = q.ids();
    let keys: Vec<u64> = ids.iter().map(|i| i.handle()).collect();
    let _ = db.ensure_resident::<daynews_db::Feed>(&keys);
    let feeds = db.cache::<daynews_db::Feed>();
    // Totals and dates for the sidebar's sorts; the unread badges below stay live queries.
    let stats = with_db(|d| d.feed_stats().unwrap_or_default()).unwrap_or_default();
    keys.iter()
        .filter_map(|id| {
            let f = feeds.elem(*id);
            if !f.exists() {
                return None;
            }
            let figures = stats.get(id).copied().unwrap_or_default();
            let unread = store_with(|s| {
                s.feed_counts
                    .borrow_mut()
                    .entry(*id)
                    .or_insert_with(|| with_db(|d| d.unread_count(Scope::Feed(*id))).expect("open"))
                    .get() as i64
            });
            Some(FeedRow {
                id: *id,
                title: f.title().with(|t| t.cloned().unwrap_or_default()),
                unread,
                folder_id: f
                    .folder()
                    .with(|v| v.copied().flatten())
                    .and_then(|o| o.id())
                    .map(|i| i.handle()),
                has_error: f.last_error().with(|v| v.cloned().flatten()).is_some(),
                site_url: f.site_url().with(|v| v.cloned().flatten()),
                feed_url: f.feed_url().with(|v| v.cloned().unwrap_or_default()),
                icon_url: f.icon_url().with(|v| v.cloned().flatten()),
                last_fetched: f.last_fetched_at().with(|v| v.copied().flatten()),
                total: figures.total,
                newest: figures.newest,
                collection_position: f.collection_position().with(|v| v.copied().unwrap_or(0.0)),
            })
        })
        .collect()
}

fn build_folder_rows(q: &Query<daynews_db::Folder>) -> Vec<FolderRow> {
    let Some(db) = store_with(|s| s.db.borrow().as_ref().map(|d| d.container.clone())) else {
        return Vec::new();
    };
    let keys: Vec<u64> = q.ids().iter().map(|i| i.handle()).collect();
    let _ = db.ensure_resident::<daynews_db::Folder>(&keys);
    let folders = db.cache::<daynews_db::Folder>();
    keys.iter()
        .filter_map(|id| {
            let f = folders.elem(*id);
            f.exists().then(|| FolderRow {
                id: *id,
                name: f.name().with(|n| n.cloned().unwrap_or_default()),
                sort: FeedSort::from_key(&f.sort().with(|s| s.cloned().unwrap_or_default())),
                position: f.position().with(|p| p.copied().unwrap_or(0.0)),
            })
        })
        .collect()
}

fn build_tag_rows(q: &Query<daynews_db::Tag>) -> Vec<TagRow> {
    let Some(db) = store_with(|s| s.db.borrow().as_ref().map(|d| d.container.clone())) else {
        return Vec::new();
    };
    let keys: Vec<u64> = q.ids().iter().map(|i| i.handle()).collect();
    let _ = db.ensure_resident::<daynews_db::Tag>(&keys);
    let tags = db.cache::<daynews_db::Tag>();
    keys.iter()
        .filter_map(|id| {
            let t = tags.elem(*id);
            if !t.exists() {
                return None;
            }
            let count = store_with(|s| {
                s.tag_counts
                    .borrow_mut()
                    .entry(*id)
                    .or_insert_with(|| with_db(|d| d.count(Scope::Tag(*id))).expect("open"))
                    .get() as i64
            });
            Some(TagRow {
                id: *id,
                name: t.name().with(|n| n.cloned().unwrap_or_default()),
                count,
            })
        })
        .collect()
}

/// Read-only page preparation, matching the native worker API.
pub async fn preview_article(id: u64) -> Result<Option<StoredArticle>, day_persistence::DbError> {
    Ok(build_reader_article(id))
}

fn build_reader_article(id: u64) -> Option<StoredArticle> {
    let a = with_db(|d| d.container.get::<Article>(id))??;
    let feed_key = a
        .feed()
        .with(|f| f.and_then(|f| f.id()))
        .map(|i| i.handle());
    let feed_title = feed_key
        .and_then(|fk| {
            with_db(|d| d.container.get::<daynews_db::Feed>(fk))
                .flatten()
                .map(|f| f.title().with(|t| t.cloned().unwrap_or_default()))
        })
        .unwrap_or_default();
    let content_html = with_db(|d| d.body(id)).flatten();
    Some(StoredArticle {
        id,
        feed_id: feed_key.unwrap_or(0),
        feed_title,
        title: a.title().with(|v| v.cloned().flatten()),
        url: a.url().with(|v| v.cloned().flatten()),
        author: a.author().with(|v| v.cloned().flatten()),
        published_at: a.published_at().with(|v| v.copied().unwrap_or(0)),
        summary: a.summary().with(|v| v.cloned().flatten()),
        content_html,
        is_read: a.is_read().with(|v| v.copied().unwrap_or(false)),
        is_starred: a.is_starred().with(|v| v.copied().unwrap_or(false)),
    })
}

// ---- selection ------------------------------------------------------------------------------

pub fn select_scope(new_scope: Scope) {
    let sc = scene();
    batch(|| {
        sc.scope.set(new_scope);
        sc.selected.set(None);
        sc.reader_open.set(false);
        sc.sticky_read.set(Vec::new());
    });
}

/// Lets the articles read in the unread scope drop out of it. A phone calls this when its
/// list closes, so returning to the same section starts from what is unread now: the scope
/// itself never changed, so `select_scope` alone would keep them.
pub fn release_sticky_read() {
    scene().sticky_read.set(Vec::new());
}
pub fn set_search(text: &str) {
    let sc = scene();
    sc.search.set(text.to_string());
}

/// Open an article. Reading it marks it read, like every reader does.
pub fn open_article(id: u64) {
    let sc = scene();
    let unread = with_db(|d| {
        d.container
            .get::<Article>(id)
            .map(|a| !a.is_read().with(|v| v.copied().unwrap_or(true)))
    })
    .flatten()
    .unwrap_or(false);
    batch(|| {
        if unread {
            set_read(id, true);
        }
        sc.selected.set(Some(id));
        sc.reader_open.set(true);
    });
}

/// Open the next unread article after the current one (NetNewsWire's ⌘/).
///
/// Searches the visible timeline first (so it follows whatever the sidebar and search box have
/// filtered to), wrapping to the top; if nothing there is unread, falls back to the global
/// unread list so the shortcut still advances from a fully-read view.
pub fn open_next_unread() -> bool {
    let sc = scene();
    let rows = sc.articles.get_untracked();
    let current = sc.selected.get_untracked();
    let start = current
        .and_then(|id| rows.iter().position(|r| r.id == id))
        .map(|i| i + 1)
        .unwrap_or(0);
    // After the cursor, then wrapping around to the rows before it.
    let next = rows[start.min(rows.len())..]
        .iter()
        .chain(rows[..start.min(rows.len())].iter())
        .find(|r| !r.is_read && Some(r.id) != current)
        .map(|r| r.id);
    if let Some(id) = next {
        open_article(id);
        return true;
    }
    // Match the timeline's newest-first order so the destination is inside its window.
    // Skip a deliberately unread current article instead of reopening it immediately.
    let anywhere = with_db(|d| {
        d.container
            .query::<Article>()
            .filter(
                daynews_db::scope_pred(Scope::Unread)
                    & !day_persistence::Pred::IdIn(current.into_iter().collect()),
            )
            .sort(Article::published_at().desc())
            .limit(1)
            .live()
            .first()
            .map(|i| i.handle())
    })
    .flatten();
    match anywhere {
        Some(id) => {
            batch(|| {
                select_scope(Scope::Unread);
                sc.search.set(String::new());
            });
            open_article(id);
            true
        }
        None => false,
    }
}

pub fn set_read(id: u64, read: bool) {
    // The sticky set belongs to the window that did the reading; only its unread timeline
    // should keep the row visible.
    let sc = scene();
    if read && sc.scope.get_untracked() == Scope::Unread {
        // Keep the row visible in the unread timeline until the scope changes.
        let mut sticky = sc.sticky_read.get_untracked();
        if !sticky.contains(&id) {
            sticky.push(id);
            sc.sticky_read.set(sticky);
        }
    }
    with_db(|d| d.set_read(id, read));
}

/// Flip one article's read flag. The row swipe, the toolbar toggle, and the menu item all
/// land here, so every affordance agrees on what "toggle" means.
pub fn toggle_read(id: u64) {
    let read = with_db(|d| {
        d.container
            .get::<Article>(id)
            .map(|a| a.is_read().with(|v| v.copied().unwrap_or(false)))
    })
    .flatten()
    .unwrap_or(false);
    set_read(id, !read);
}

pub fn set_starred(id: u64, starred: bool) {
    with_db(|d| d.set_starred(id, starred));
}

/// Toggle a named tag on an article, creating the tag on first use.
pub fn toggle_tag(article: u64, name: &str) {
    let name = name.trim();
    if name.is_empty() {
        return;
    }
    with_db(|d| {
        let tag = d.add_tag(name);
        let tagged = d
            .container
            .get::<Article>(article)
            .map(|a| a.tags().contains(tag))
            .unwrap_or(false);
        d.set_tagged(article, tag, !tagged);
    });
}

/// "Mark All as Read" for whatever the sidebar has selected.
pub fn mark_scope_read(read: bool) {
    let sc = scene();
    let in_scope = sc.scope.get_untracked();
    batch(|| {
        with_db(|d| d.set_read_all(in_scope, read));
        // Bulk completion clears rows retained by individual reading.
        sc.sticky_read.set(Vec::new());
    });
}

pub fn mark_feed_read(feed: u64, read: bool) {
    with_db(|d| d.set_read_all(Scope::Feed(feed), read));
}

// ---- retention ------------------------------------------------------------------------------

/// Prune old read articles (starred and tagged stay). `0` keeps everything. The app calls
/// this at startup and when the setting shortens.
pub async fn prune(days: u32) -> usize {
    if days == 0 {
        return 0;
    }
    with_db(|d| d.prune_older_than(days)).unwrap_or(0)
}

// ---- subscriptions --------------------------------------------------------------------------

/// Subscribe and immediately fetch, so the feed is named and populated without a manual refresh.
pub fn subscribe(url: &str) {
    let url = normalize_feed_url(url);
    if url.is_empty() {
        return;
    }
    let st = state();
    let Some(id) = with_db(|d| d.add_feed(&url, &fallback_title(&url), None)) else {
        st.status.set("Could not add the subscription.".into());
        return;
    };
    st.status
        .set(format!("Subscribed to {}", fallback_title(&url)));
    day_core::task(async move {
        // This independent subscription fetch must not clear a concurrent full refresh's
        // progress or allow a second full refresh to start.
        refresh_one(id, url).await;
    });
}

pub async fn subscribe_discovered(
    url: String,
    update: daynews_feed::FeedUpdate,
) -> Result<u64, String> {
    let id = with_db(|d| d.add_feed(&url, &fallback_title(&url), None))
        .ok_or_else(|| "subscription store unavailable".to_string())?;
    if !apply_update(id, url, Ok(update)) {
        return Err("subscription import failed".into());
    }
    Ok(id)
}
pub async fn subscribe_asset(url: String) -> Result<u64, String> {
    let parsed = preview_feed(&url).await.map_err(|e| e.to_string())?;
    subscribe_discovered(
        url,
        daynews_feed::FeedUpdate::Modified(parsed, Default::default(), None),
    )
    .await
}
pub fn unsubscribe(feed: u64) {
    with_db(|d| d.delete_feed(feed));
    // The window that removed the feed cannot stay scoped to it.
    let sc = scene();
    if sc.scope.get_untracked() == Scope::Feed(feed) {
        sc.scope.set(Scope::Unread);
    }
}

/// Create a collection (or find the one with this name), optionally moving `feed` into it.
pub fn create_folder(name: &str, feed: Option<u64>) {
    let name = name.trim();
    if name.is_empty() {
        return;
    }
    with_db(|d| {
        let id = d.add_folder(name);
        if let Some(feed) = feed {
            let _ = d.set_feed_folder(feed, Some(id), None);
        }
    });
}
/// Rename a collection; an empty or already-used name is refused.
pub fn rename_folder(folder: u64, name: &str) {
    with_db(|d| d.rename_folder(folder, name));
}
/// Delete a collection. Its feeds stay subscribed, outside any collection.
pub fn delete_folder(folder: u64) {
    with_db(|d| d.delete_folder(folder));
}
/// Write the sidebar's collection order: each collection's position.
pub fn set_folder_positions(positions: Vec<(u64, f64)>) {
    with_db(|d| d.set_folder_positions(&positions));
}
/// Choose how the sidebar orders a collection's feeds.
pub fn set_folder_sort(folder: u64, sort: FeedSort) {
    with_db(|d| d.set_folder_sort(folder, sort));
}
/// Put a feed in a collection (`None`: in none), before `before` in its manual order or last.
pub fn set_feed_folder(feed: u64, folder: Option<u64>, before: Option<u64>) {
    with_db(|d| d.set_feed_folder(feed, folder, before));
}

/// The subscribed feeds already holding articles at any of `links`, most first.
pub async fn feeds_with_links(links: Vec<String>) -> Vec<(u64, usize)> {
    with_db(|d| d.feeds_with_links(&links))
        .and_then(Result::ok)
        .unwrap_or_default()
}
pub fn rename_feed(feed: u64, title: &str) {
    with_db(|d| d.rename_feed(feed, title));
}

/// Refresh one subscription: a feed row's Refresh and Feed ▸ Refresh Feed. A full refresh
/// already under way fetches this feed too, so asking again while it runs does nothing.
pub fn refresh_feed(feed: u64) {
    let st = state();
    if st.refresh_progress.get_untracked().is_some() {
        return;
    }
    let Some((title, url)) = st
        .feeds
        .get_untracked()
        .iter()
        .find(|f| f.id == feed)
        .map(|f| (f.title.clone(), f.feed_url.clone()))
    else {
        return;
    };
    st.refresh_progress.set(Some((0, 1)));
    day_core::task(async move {
        let ok = refresh_one(feed, url).await;
        let st = state();
        st.refresh_progress.set(None);
        st.status.set(if ok {
            format!("Refreshed {title}")
        } else {
            format!("Could not refresh {title}")
        });
    });
}

pub fn refresh_due() {
    if state().refresh_progress.get_untracked().is_some() {
        return;
    }
    if let Some(Ok(feeds)) = with_db(|db| db.due_feeds(daynews_time::now_unix())) {
        start_refresh(feeds);
    }
}

pub fn move_feed(feed: u64, to: usize) {
    with_db(|db| {
        let _ = db.move_feed(feed, to);
    });
}

// ---- refreshing -----------------------------------------------------------------------------

/// Refresh up to four subscriptions concurrently. A slow server does not hold up every
/// other feed; the bounded stream keeps resource use predictable. Futures still poll on
/// the UI executor, where the store and reactive state belong.
pub fn refresh_all() {
    let st = state();
    if st.refresh_progress.get_untracked().is_some() {
        return; // already running
    }
    let feeds: Vec<(u64, String)> = st
        .feeds
        .get_untracked()
        .iter()
        .map(|f| (f.id, f.feed_url.clone()))
        .collect();
    start_refresh(feeds);
}

fn start_refresh(feeds: Vec<(u64, String)>) {
    let st = state();
    if feeds.is_empty() || st.refresh_progress.get_untracked().is_some() {
        return;
    }
    let total = feeds.len();
    st.refresh_progress.set(Some((0, total)));
    day_core::task(async move {
        let mut failed = 0usize;
        let mut pending = stream::iter(feeds)
            .map(|(id, url)| refresh_one(id, url))
            .buffer_unordered(4);
        let mut done = 0;
        while let Some(ok) = pending.next().await {
            failed += usize::from(!ok);
            done += 1;
            state().refresh_progress.set(Some((done, total)));
        }
        let st = state();
        st.refresh_progress.set(None);
        st.status.set(match failed {
            0 => format!("Refreshed {total} feeds"),
            n => format!("Refreshed {} of {total} feeds — {n} failed", total - n),
        });
    });
}

/// Fetch and store one feed. Returns whether it succeeded.
/// A feed's bytes, parsed: over HTTP normally, or from the app bundle for `asset:` URLs, the
/// deterministic, network-free source the walkthrough seeds from on every platform
/// (dayscript/seed-demo.yaml subscribes to the demo feeds bundled under
/// `resource/assets/demo/`). A missing asset reports as a 404 rather than a new error
/// arm; the subscription then shows the same failed-refresh state a dead feed does.
pub async fn preview_feed(url: &str) -> Result<daynews_feed::ParsedFeed, daynews_feed::FeedError> {
    if let Some(name) = url.strip_prefix("asset:") {
        let locale = day_localization::locale().get_untracked();
        for candidate in asset_candidates(name, &locale) {
            // wasm has no filesystem for the resource opener to read; the web dist serves the
            // same bundle over HTTP instead (`resource/assets/` staged under `assets/data/`,
            // day-cli web.rs), so the asset rides the ordinary fetch path as a same-origin URL.
            // Fetch by the relative dist URL, parse against the absolute `asset:` base, because
            // the parser's URL resolution rejects a relative base outright.
            #[cfg(target_arch = "wasm32")]
            match daynews_feed::fetch_with_base(&format!("assets/data/{candidate}"), url).await {
                Err(daynews_feed::FeedError::Status(404)) => continue,
                other => return other,
            }
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(res) = day_core::resource(day_core::AssetName::dynamic(candidate)) {
                return daynews_feed::parse(res.as_slice(), url);
            }
        }
        return Err(daynews_feed::FeedError::Status(404));
    }
    daynews_feed::fetch(url).await
}

/// The bundle paths an `asset:` name may live at, most specific first.
///
/// The demo feeds are written once per language (resource/assets/demo/README.md) and subscribed
/// to without one, as `demo/night-sky.xml`, so a subscription reads the set for the language the
/// app is running in: the exact locale, then its language alone, then English. Any other name is
/// one file, taken as written.
fn asset_candidates(name: &str, locale: &str) -> Vec<String> {
    let Some(file) = name.strip_prefix("demo/") else {
        return vec![name.to_string()];
    };
    let tag = locale.split("-u-").next().unwrap_or(locale);
    let lang = tag.split('-').next().unwrap_or(tag);
    let mut paths: Vec<String> = Vec::new();
    for dir in [tag, lang, "en"] {
        let path = format!("demo/{dir}/{file}");
        if !dir.is_empty() && !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
}

struct UpdatingFeed {
    store: std::rc::Rc<Store>,
    id: u64,
    active: Signal<HashMap<u64, Option<f64>>>,
}
impl Drop for UpdatingFeed {
    fn drop(&mut self) {
        self.store.scheduled_feeds.borrow_mut().remove(&self.id);
        self.active.update(|ids| {
            ids.remove(&self.id);
        });
    }
}

async fn refresh_one(id: u64, url: String) -> bool {
    let store = store();
    if !store.scheduled_feeds.borrow_mut().insert(id) {
        return true;
    }
    let active = state().updating_feeds;
    let _updating = UpdatingFeed {
        store: store.clone(),
        id,
        active,
    };
    // Subscription fetches and full/manual refreshes share the same four native exchanges.
    let (_slot, _, waiting) = futures_util::future::select_all(
        store.refresh_slots.iter().map(|slot| Box::pin(slot.lock())),
    )
    .await;
    drop(waiting);
    let Some(daynews_db::FeedCache {
        etag,
        last_modified,
        body_hash,
        retry_after,
        needs_publication_sample,
    }) = with_db(|d| d.feed_cache(id)).flatten()
    else {
        return true;
    };
    if retry_after.is_some_and(|until| {
        until > (daynews_time::now_epoch_ms() / 1000).min(i64::MAX as u64) as i64
    }) {
        return false;
    }
    active.update(|ids| {
        ids.insert(id, None);
    });
    let validators = if needs_publication_sample {
        daynews_feed::CacheValidators::default()
    } else {
        daynews_feed::CacheValidators {
            etag,
            last_modified,
        }
    };
    let result = if url.starts_with("asset:") {
        preview_feed(&url)
            .await
            .map(|feed| daynews_feed::FeedUpdate::Modified(feed, Default::default(), None))
    } else {
        let mut last = None;
        let mut last_update = 0;
        daynews_feed::fetch_cached_with_progress(
            &url,
            &validators,
            if needs_publication_sample {
                None
            } else {
                body_hash.as_deref()
            },
            |received, total| {
                let fraction = total
                    .filter(|n| *n > 0)
                    .map(|n| (received as f64 / n as f64).clamp(0.0, 1.0));
                let now = daynews_time::now_epoch_ms();
                if fraction != last
                    && (fraction.is_some() != last.is_some()
                        || fraction == Some(1.0)
                        || now.saturating_sub(last_update) >= 100)
                {
                    active.update(|values| {
                        values.insert(id, fraction);
                    });
                    last = fraction;
                    last_update = now;
                }
            },
        )
        .await
    };
    apply_update(id, url, result)
}
fn apply_update(
    id: u64,
    url: String,
    result: Result<daynews_feed::FeedUpdate, daynews_feed::FeedError>,
) -> bool {
    batch(|| match result {
        Ok(daynews_feed::FeedUpdate::NotModified(validators)) => {
            with_db(|d| d.feed_checked(id, validators.etag, validators.last_modified));
            true
        }
        Ok(daynews_feed::FeedUpdate::Modified(parsed, validators, body_hash)) => {
            let items: Vec<IncomingArticle> = parsed
                .items
                .into_iter()
                .map(|i| {
                    // Move bodies into the store rather than cloning every downloaded article
                    // on the UI thread. Title-less items still get their readable display title.
                    let title = Some(i.display_title());
                    IncomingArticle {
                        guid: i.guid,
                        title,
                        url: i.url,
                        author: i.author,
                        published: i.published,
                        summary: i.summary,
                        content_html: i.content_html,
                    }
                })
                .collect();
            with_db(|d| {
                // Keep cache metadata unchanged if the checked import fails.
                if d.feed_cache(id).is_none() {
                    return true;
                }
                if let Err(error) = d.try_upsert_articles(id, &url, &items) {
                    d.set_feed_error(id, &error.to_string());
                    return false;
                }
                d.record_publications(id, items.iter().filter_map(|item| item.published));
                d.update_feed_metadata(
                    id,
                    parsed.title.as_deref(),
                    parsed.site_url.as_deref(),
                    parsed.description.as_deref(),
                    parsed.icon_url.as_deref(),
                );
                d.set_feed_body_hash(id, body_hash);
                d.feed_checked(id, validators.etag, validators.last_modified);
                true
            })
            .unwrap_or(false)
        }
        Err(e) => {
            with_db(|d| {
                let until = match &e {
                    daynews_feed::FeedError::RetryAfter { until, .. } => Some(*until),
                    _ => None,
                };
                d.set_feed_retry_after(id, until);
                d.set_feed_error(id, &e.to_string());
            });
            false
        }
    })
}

// ---- OPML -----------------------------------------------------------------------------------

/// Import subscriptions, creating folders as needed. Returns (added, already present).
pub async fn import_opml(text: &str) -> std::result::Result<(usize, usize), String> {
    let doc = daynews_opml::parse(text).map_err(|e| e.to_string())?;
    let entries = doc.feeds();
    let (mut added, mut existing) = (0usize, 0usize);
    with_db(|d| {
        for (path, feed) in &entries {
            // Only the innermost folder becomes a folder; deeper nesting is rare and flattening
            // it matches what the sidebar can represent.
            let folder = path.last().map(|name| d.add_folder(name));
            let url = feed.xml_url.clone();
            if d.container.get::<daynews_db::Feed>(feed_id(&url)).is_some() {
                existing += 1;
                continue;
            }
            d.add_feed(&url, &feed.display_title(), folder);
            added += 1;
        }
    });
    state().status.set(match existing {
        0 => format!("Imported {added} feeds"),
        n => format!("Imported {added} feeds ({n} already subscribed)"),
    });
    Ok((added, existing))
}

/// Serialize the current subscriptions as OPML, grouped by folder.
pub fn export_opml() -> String {
    let st = state();
    let feeds = st.feeds.get_untracked();
    let folders = st.folders.get_untracked();
    let mut root: Vec<daynews_opml::Outline> = Vec::new();
    for folder in &folders {
        let children: Vec<daynews_opml::Outline> = feeds
            .iter()
            .filter(|f| f.folder_id == Some(folder.id))
            .map(to_outline)
            .collect();
        if !children.is_empty() {
            root.push(daynews_opml::Outline::Folder {
                title: folder.name.clone(),
                children,
            });
        }
    }
    root.extend(
        feeds
            .iter()
            .filter(|f| f.folder_id.is_none())
            .map(to_outline),
    );
    daynews_opml::write(&daynews_opml::Opml {
        title: Some("Fleet Reader Subscriptions".into()),
        outlines: root,
    })
    .unwrap_or_default()
}

fn to_outline(f: &FeedRow) -> daynews_opml::Outline {
    daynews_opml::Outline::Feed(daynews_opml::FeedRef {
        title: f.title.clone(),
        xml_url: f.feed_url.clone(),
        html_url: f.site_url.clone(),
    })
}

// ---- helpers --------------------------------------------------------------------------------

/// Accept what people paste: bare hosts get a scheme, and whitespace is trimmed.
pub fn normalize_feed_url(input: &str) -> String {
    let t = input.trim();
    // `asset:` is the bundled-fixture scheme (see `fetch_feed`): pass it through untouched.
    if t.starts_with("http://") || t.starts_with("https://") || t.starts_with("asset:") {
        t.to_string()
    } else if t.is_empty() {
        String::new()
    } else {
        format!("https://{t}")
    }
}

/// A provisional name for a brand-new subscription, replaced by the feed's title on first
/// refresh, the same placeholder NetNewsWire shows.
fn fallback_title(url: &str) -> String {
    // A bundled feed has no host: name it after its file, not the `asset:demo` prefix.
    if let Some(name) = url.strip_prefix("asset:") {
        let file = name.rsplit('/').next().unwrap_or(name);
        return file.split('.').next().unwrap_or(file).to_string();
    }
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    host.strip_prefix("www.").unwrap_or(host).to_string()
}

/// Where the library lives. The web's names an OPFS file, private to the origin, so it needs no
/// folder; elsewhere (this module's tests) the app's data directory.
#[cfg(target_arch = "wasm32")]
pub fn store_dir() -> PathBuf {
    PathBuf::new()
}

#[cfg(not(target_arch = "wasm32"))]
pub fn store_dir() -> PathBuf {
    day_spec::storage::data_dir().unwrap_or_else(std::env::temp_dir)
}

/// Where re-fetchable files go (feed icons). The web has no directory for them: its file reads
/// and writes fail, and the icons are fetched again.
#[cfg(target_arch = "wasm32")]
pub fn cache_dir() -> PathBuf {
    PathBuf::from("cache")
}

#[cfg(not(target_arch = "wasm32"))]
pub fn cache_dir() -> PathBuf {
    day_spec::storage::cache_dir().unwrap_or_else(std::env::temp_dir)
}

/// Run `f` against the process store, the shape the old `thread_local!` accessor had, so every
/// call site below reads the same.
fn store_with<R>(f: impl FnOnce(&Store) -> R) -> R {
    f(&store())
}

pub fn watch_dashboard(
    scope: Signal<Scope>,
    local_day: Signal<i64>,
    output: Signal<Option<daynews_db::dashboard::Dashboard>>,
) {
    let st = state();
    let sc = scene();
    let cache = RefCell::new(daynews_db::dashboard::DashboardCache::default());
    Effect::new(move || {
        let scope = scope.get();
        let _ = local_day.get();
        let _ = (
            st.feeds.get(),
            st.total_unread.get(),
            st.total_starred.get(),
            sc.articles.get(),
        );
        if let Some(Ok(snapshot)) = with_db(|db| {
            db.dashboard_cached(scope, daynews_time::now_unix(), &mut cache.borrow_mut())
        }) {
            output.set_if_changed(Some(snapshot));
        }
    });
}

/// How many words the dashboard's word cloud draws at most.
pub const CLOUD_WORDS: usize = 90;

/// The dashboard's word cloud for `scope` (`Db::word_cloud`), following the scope, the day and
/// the number of stored articles but not read marks.
pub fn watch_word_cloud(
    scope: Signal<Scope>,
    local_day: Signal<i64>,
    output: Signal<Option<Vec<daynews_db::dashboard::CloudWord>>>,
) {
    // Counted clouds by what they were counted for, so an overview built again reuses one.
    type Counted = Vec<(
        (Scope, i64, i64, String),
        Vec<daynews_db::dashboard::CloudWord>,
    )>;
    let counted: RefCell<Counted> = RefCell::default();
    let st = state();
    Effect::new(move || {
        let scope = scope.get();
        let fallback = day_localization::locale()
            .get()
            .split(['-', '_'])
            .next()
            .unwrap_or("en")
            .to_ascii_lowercase();
        let key = (scope, local_day.get(), st.total_articles.get(), fallback);
        if let Some(words) = counted
            .borrow()
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, w)| w.clone())
        {
            output.set_if_changed(Some(words));
            return;
        }
        if let Some(Ok(words)) =
            with_db(|db| db.word_cloud(scope, daynews_time::now_unix(), CLOUD_WORDS, &key.3))
        {
            let mut counted = counted.borrow_mut();
            counted.push((key, words.clone()));
            let excess = counted.len().saturating_sub(12);
            counted.drain(..excess);
            output.set_if_changed(Some(words));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::asset_candidates;

    // Synthetic local fixtures: exercise the real live queries without network or UI hosts.
    fn with_scene(test: impl FnOnce(super::NewsScene, &daynews_db::Db)) {
        use day_core::Ambient;
        let scope = day_reactive::Scope::child();
        scope.enter(|| {
            let db = daynews_db::Db::open_in_memory().unwrap();
            super::store_with(|s| *s.db.borrow_mut() = Some(db));
            let scene = super::NewsScene::create();
            scope.provide(scene);
            super::with_db(|db| test(scene, db)).unwrap();
        });
        scope.dispose();
    }

    fn seed(db: &daynews_db::Db, count: usize) -> Vec<u64> {
        let url = "https://fixture.example/feed";
        let feed = db.add_feed(url, "Fixture feed", None);
        let articles: Vec<_> = (0..count)
            .map(|i| daynews_db::IncomingArticle {
                guid: i.to_string(),
                title: Some(format!("Fixture {i}")),
                url: None,
                author: None,
                published: Some(i as i64),
                summary: Some("Fixture summary".into()),
                content_html: None,
            })
            .collect();
        db.upsert_articles(feed, url, &articles);
        (0..count)
            .map(|i| daynews_db::article_id(url, &i.to_string()))
            .collect()
    }

    #[test]
    fn reading_keeps_the_row_until_bulk_completion() {
        with_scene(|sc, db| {
            let ids = seed(db, 3);
            super::open_article(ids[2]);
            assert_eq!(sc.selected.get_untracked(), Some(ids[2]));
            assert_eq!(sc.articles.get_untracked().len(), 3);
            assert_eq!(sc.scope_unread.get_untracked(), 2);
            assert!(sc.article.get_untracked().unwrap().is_read);
            super::mark_scope_read(true);
            assert!(sc.articles.get_untracked().is_empty());
            assert_eq!(sc.scope_unread.get_untracked(), 0);
        });
    }

    #[test]
    fn next_unread_clears_search_and_lands_inside_a_large_timeline() {
        with_scene(|sc, db| {
            let ids = seed(db, super::TIMELINE_LIMIT + 5);
            super::select_scope(daynews_db::Scope::Starred);
            super::set_search("no matching fixture");
            assert!(sc.articles.get_untracked().is_empty());
            assert!(super::open_next_unread());
            assert_eq!(sc.scope.get_untracked(), daynews_db::Scope::Unread);
            assert_eq!(sc.search.get_untracked(), "");
            assert_eq!(sc.selected.get_untracked(), ids.last().copied());
            assert!(
                sc.articles
                    .get_untracked()
                    .iter()
                    .any(|a| Some(a.id) == sc.selected.get_untracked())
            );
            assert_eq!(sc.scope_unread.get_untracked(), super::TIMELINE_LIMIT + 4);
        });
    }

    #[test]
    fn next_unread_wraps_but_does_not_reopen_the_current_article() {
        with_scene(|sc, db| {
            let ids = seed(db, 3);
            super::select_scope(daynews_db::Scope::All);
            super::open_article(ids[0]);
            assert!(super::open_next_unread());
            assert_eq!(sc.selected.get_untracked(), Some(ids[2]));
            super::mark_scope_read(true);
            super::set_read(ids[2], false);
            assert!(!super::open_next_unread());
            // Model writes drain at the end of a native event turn.
            day_reactive::flush_sync();
            assert!(!sc.article.get_untracked().unwrap().is_read);
        });
    }

    #[test]
    fn demo_feeds_resolve_through_the_locale_then_english() {
        assert_eq!(
            asset_candidates("demo/night-sky.xml", "zh-CN"),
            [
                "demo/zh-CN/night-sky.xml",
                "demo/zh/night-sky.xml",
                "demo/en/night-sky.xml"
            ]
        );
        assert_eq!(
            asset_candidates("demo/atlas.xml", "fr"),
            ["demo/fr/atlas.xml", "demo/en/atlas.xml"]
        );
        assert_eq!(
            asset_candidates("demo/atlas.xml", "en"),
            ["demo/en/atlas.xml"]
        );
        // A Unicode extension picks a collation, not a language.
        assert_eq!(
            asset_candidates("demo/atlas.xml", "zh-u-co-stroke"),
            ["demo/zh/atlas.xml", "demo/en/atlas.xml"]
        );
        assert_eq!(
            asset_candidates("demo/atlas.xml", ""),
            ["demo/en/atlas.xml"]
        );
    }

    #[test]
    fn a_new_subscription_is_named_after_its_host_or_bundled_file() {
        assert_eq!(
            super::fallback_title("https://www.nasa.gov/feed/"),
            "nasa.gov"
        );
        assert_eq!(
            super::fallback_title("asset:demo/night-sky.xml"),
            "night-sky"
        );
    }

    #[test]
    fn other_assets_are_read_as_named() {
        assert_eq!(
            asset_candidates("feeds/custom.xml", "fr"),
            ["feeds/custom.xml"]
        );
    }
}

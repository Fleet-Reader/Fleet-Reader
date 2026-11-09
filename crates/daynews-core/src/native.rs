// SPDX-License-Identifier: Apache-2.0
//! UI signals stay on the UI thread; SQLite, migrations, FTS, materialization, and undo
//! live on one database worker. Committed, owned projections are the only bridge.
use day_core::Ambient;
use day_model::Source;
mod refresh;
#[cfg(feature = "feedbin")]
mod sync;
#[cfg(not(feature = "feedbin"))]
#[path = "native/local_sync.rs"]
mod sync;
mod sync_status;
pub use refresh::{
    refresh_all, refresh_due, refresh_feed, refresh_scheduled, refresh_sync_pending,
};
pub use sync_status::SyncStatus;
#[cfg(feature = "experimental-webdav")]
pub mod webdav;
pub use sync::*;

use day_persistence::{DatabaseWorker, DbError, ModelContainer, Pred};
use day_reactive::Binding;
use day_reactive::{Effect, Signal, batch, watch};
use daynews_db::{
    Article, ArticleFields, ArticleRelations, Db, FeedFields, IncomingArticle, Scope,
    timeline_fetch,
};
pub use daynews_db::{FeedSort, Scope as TimelineScope, article_id, feed_id, folder_id, tag_id};
use futures_util::{
    FutureExt,
    future::{LocalBoxFuture, Shared},
};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
};

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

type StatusFormatter = fn(StatusMessage) -> String;
type OpeningWorker = Shared<LocalBoxFuture<'static, Result<DatabaseWorker, DbError>>>;

struct Store {
    worker: RefCell<Option<DatabaseWorker>>,
    format_status: RefCell<Option<StatusFormatter>>,
    ready: Signal<bool>,
    opening: RefCell<Option<OpeningWorker>>,
    refresh_slots: [futures_util::lock::Mutex<()>; 4],
    scheduled_feeds: RefCell<HashSet<u64>>,
    can_undo: Signal<bool>,
    can_redo: Signal<bool>,
}
#[derive(Clone)]
struct StoreHandle(Rc<Store>);
impl Ambient for StoreHandle {
    fn create() -> Self {
        Self(Rc::new(Store {
            worker: RefCell::new(None),
            format_status: RefCell::new(None),
            opening: RefCell::new(None),
            ready: Signal::new(false),
            refresh_slots: std::array::from_fn(|_| futures_util::lock::Mutex::new(())),
            scheduled_feeds: RefCell::new(HashSet::new()),
            can_undo: Signal::new(false),
            can_redo: Signal::new(false),
        }))
    }
}
fn store() -> Rc<Store> {
    StoreHandle::app().0
}
fn worker() -> Option<DatabaseWorker> {
    store().worker.borrow().clone()
}
fn status(message: StatusMessage) -> String {
    store()
        .format_status
        .borrow()
        .map(|format| format(message))
        .unwrap_or_default()
}
pub fn shutdown() {
    if let Some(worker) = worker()
        && let Err(error) = worker.close_blocking()
    {
        report(error);
    }
}
async fn ready_worker() -> Result<DatabaseWorker, DbError> {
    if let Some(worker) = worker() {
        return Ok(worker);
    }
    let opening = store().opening.borrow().clone();
    match opening {
        Some(opening) => opening.await,
        None => Err(DbError::new(
            day_persistence::DbErrorKind::Closed,
            "article store not initialized",
        )),
    }
}
// Error diagnostics are data. The application supplies a localized presentation on the UI.
#[derive(Clone, Copy)]
pub struct StorageError(pub Signal<Option<String>>);
impl Ambient for StorageError {
    fn create() -> Self {
        Self(Signal::new(None))
    }
}
fn report(error: DbError) {
    StorageError::app().0.set(Some(error.to_string()));
}

impl Ambient for SheetsState {
    fn create() -> Self {
        Self {
            feeds: Signal::new(vec![]),
            group_by_feed: Signal::new(true),
            folders: Signal::new(vec![]),
            tags: Signal::new(vec![]),
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
pub fn state() -> SheetsState {
    SheetsState::app()
}
impl Ambient for NewsScene {
    fn create() -> Self {
        let sc = Self {
            scope: Signal::new(Scope::Unread),
            articles: Signal::new(vec![]),
            scope_unread: Signal::new(0),
            selected: Signal::new(None),
            timeline_focused: Signal::new(false),
            article: Signal::new(None),
            reader_open: Signal::new(false),
            search: Signal::new(String::new()),
            sticky_read: Signal::new(vec![]),
        };
        wire_scene(sc);
        sc
    }
}
pub fn scene() -> NewsScene {
    try_scene().expect("no window is open")
}
pub fn try_scene() -> Option<NewsScene> {
    NewsScene::try_ambient().or_else(NewsScene::focused)
}

/// Own a task for precisely one effect run. Changing scope/search/selection cancels the old
/// subscription before starting a new one; a late result cannot replace the new projection.
fn scoped_task(future: impl std::future::Future<Output = ()> + 'static) {
    let task = day_core::task(future);
    day_reactive::on_run_retrack(move || task.abort());
}
fn wire_scene(sc: NewsScene) {
    let st = store();
    watch(
        move || sc.search.get(),
        move |_, _| sc.sticky_read.set(vec![]),
    );
    Effect::new(move || {
        if !st.ready.get() {
            return;
        }
        let Some(worker) = worker() else {
            return;
        };
        let scope = sc.scope.get();
        let search = sc.search.get();
        let sticky = sc.sticky_read.get();
        let grouped = state().group_by_feed.get();
        scoped_task(async move {
            let projection = worker
                .observe(move |db| {
                    let (mut rows, count) = timeline_snapshot(db, scope, &search, &sticky)?;
                    if grouped {
                        let feeds = db
                            .query::<daynews_db::Feed>()
                            .sort(daynews_db::Feed::position().asc())
                            .sort(daynews_db::Feed::title().asc())
                            .live()
                            .try_collect()?;
                        let order: HashMap<_, _> =
                            feeds.iter().enumerate().map(|(i, f)| (f.id, i)).collect();
                        rows.sort_by_key(|a| order.get(&a.feed_id).copied().unwrap_or(usize::MAX));
                    }
                    Ok((rows, count))
                })
                .await;
            let mut projection = match projection {
                Ok(v) => v,
                Err(e) => {
                    report(e);
                    return;
                }
            };
            while let Some(result) = projection.next().await {
                match result {
                    Ok(snapshot) => batch(|| {
                        sc.articles.set_if_changed(snapshot.value.0);
                        sc.scope_unread.set_if_changed(snapshot.value.1);
                    }),
                    Err(e) => report(e),
                }
            }
        });
    });
    let st = store();
    Effect::new(move || {
        let ready = st.ready.get();
        let id = sc.selected.get();
        let (true, Some(id), Some(worker)) = (ready, id, worker()) else {
            sc.article.set_if_changed(None);
            return;
        };
        // Keep the displayed snapshot until its replacement arrives. Clearing it between
        // worker reads destroys the WebView and toolbar even though the reader stayed open.
        // The scoped task still cancels obsolete subscriptions on every selection change.
        scoped_task(async move {
            let mut projection = match worker.observe(move |db| reader_snapshot(db, id)).await {
                Ok(v) => v,
                Err(e) => {
                    report(e);
                    return;
                }
            };
            while let Some(result) = projection.next().await {
                match result {
                    Ok(v) => sc.article.set_if_changed(v.value),
                    Err(e) => report(e),
                }
            }
        });
    });
    watch(
        move || sc.reader_open.get(),
        move |open, _| {
            if !open {
                sc.selected.set(None);
            }
        },
    );
}

pub enum StatusMessage {
    RefreshedFeed(String, bool),
    RefreshedFeeds(usize, usize),
    RefreshedFeedsPendingSync(usize, usize),
    Imported(usize, usize),
    ExportTitle,
}
pub fn init(format_status: fn(StatusMessage) -> String) {
    let st = store();
    *st.format_status.borrow_mut() = Some(format_status);
    let path = store_dir().join(crate::LIBRARY);
    let can_undo = st.can_undo;
    let can_redo = st.can_redo;
    day_core::install_undo_bridge(
        can_undo,
        can_redo,
        Signal::new(String::new()),
        Signal::new(String::new()),
        |redo| {
            let Some(worker) = worker() else {
                return;
            };
            day_core::task(async move {
                if let Err(e) = worker.undo(redo).await {
                    report(e);
                }
                if let Err(e) = worker
                    .write(|c| {
                        Db {
                            container: c.clone(),
                        }
                        .sync_after_undo()
                    })
                    .await
                {
                    report(e);
                }
                update_undo(&worker).await;
                sync_request_soon();
            });
        },
    );
    let opening = async move {
        #[cfg(feature = "experimental-webdav")]
        let worker = webdav::open_worker(path).await?;
        #[cfg(not(feature = "experimental-webdav"))]
        let worker = DatabaseWorker::open(move || Db::open(&path).map(|db| db.container)).await?;
        worker.enable_undo(UNDO_LEVELS).await?;
        Ok(worker)
    }
    .boxed_local()
    .shared();
    *st.opening.borrow_mut() = Some(opening.clone());
    day_core::task(async move {
        let worker = match opening.await {
            Ok(worker) => worker,
            Err(error) => {
                report(error);
                return;
            }
        };
        *st.worker.borrow_mut() = Some(worker.clone());
        st.ready.set(true);
        let mut projection = match worker.observe(sidebar_snapshot).await {
            Ok(v) => v,
            Err(e) => {
                report(e);
                return;
            }
        };
        while let Some(result) = projection.next().await {
            match result {
                Ok(snapshot) => batch(|| {
                    let (feeds, folders, tags, unread, starred, today, articles) = snapshot.value;
                    let state = state();
                    state.feeds.set_if_changed(feeds);
                    state.folders.set_if_changed(folders);
                    state.tags.set_if_changed(tags);
                    state.total_unread.set_if_changed(unread);
                    state.total_starred.set_if_changed(starred);
                    state.total_today.set_if_changed(today);
                    state.total_articles.set_if_changed(articles);
                }),
                Err(e) => report(e),
            }
        }
    });
}
async fn update_undo(worker: &DatabaseWorker) {
    match worker.undo_status().await {
        Ok((undo, redo)) => {
            let st = store();
            batch(|| {
                st.can_undo.set(undo);
                st.can_redo.set(redo);
            });
        }
        Err(e) => report(e),
    }
}
fn edit(f: impl FnOnce(&Db) -> Result<(), DbError> + Send + 'static) {
    day_core::task(async move {
        let worker = match ready_worker().await {
            Ok(w) => w,
            Err(e) => {
                report(e);
                return;
            }
        };
        if let Err(e) = worker
            .write(move |container| {
                f(&Db {
                    container: container.clone(),
                })
            })
            .await
        {
            report(e);
        }
        update_undo(&worker).await;
        sync_request_soon();
    });
}

type SidebarSnapshot = (
    Vec<FeedRow>,
    Vec<FolderRow>,
    Vec<TagRow>,
    i64,
    i64,
    i64,
    i64,
);
fn sidebar_snapshot(db: &ModelContainer) -> Result<SidebarSnapshot, DbError> {
    use daynews_db::{Feed, Folder, Tag};
    let d = Db {
        container: db.clone(),
    };
    let stats = d.feed_stats()?;
    let feeds: Vec<FeedRow> = db
        .query::<Feed>()
        .filter(Feed::unsubscribed().eq(false))
        .sort(Feed::position().asc())
        .sort(Feed::title().asc())
        .live()
        .try_collect()?
        .into_iter()
        .map(|f| {
            let stats = stats.get(&f.id).copied().unwrap_or_default();
            FeedRow {
                id: f.id,
                title: f.title,
                unread: stats.unread,
                folder_id: f.folder.and_then(|f| f.id()).map(|id| id.handle()),
                has_error: f.last_error.is_some(),
                site_url: f.site_url,
                feed_url: f.feed_url,
                icon_url: f.icon_url,
                last_fetched: f.last_fetched_at,
                total: stats.total,
                newest: stats.newest,
                collection_position: f.collection_position,
            }
        })
        .collect();
    let folders = db
        .query::<Folder>()
        .sort(Folder::position().asc())
        .sort(Folder::name().asc())
        .live()
        .try_collect()?
        .into_iter()
        .map(|f| FolderRow {
            id: f.id,
            name: f.name,
            sort: FeedSort::from_key(&f.sort),
            position: f.position,
        })
        .collect();
    let tags = db
        .query::<Tag>()
        .sort(Tag::name().asc())
        .live()
        .try_collect()?
        .into_iter()
        .map(|t| {
            Ok(TagRow {
                id: t.id,
                name: t.name,
                count: d.count(Scope::Tag(t.id)).try_get()? as i64,
            })
        })
        .collect::<Result<_, DbError>>()?;
    // Only the feeds the sidebar lists: an unsubscribed feed keeps its articles, but they show
    // in no list, so they count toward no total either.
    let unread = feeds.iter().map(|f| f.unread).sum();
    Ok((
        feeds,
        folders,
        tags,
        unread,
        d.count(Scope::Starred).try_get()? as i64,
        d.unread_count(Scope::Today).try_get()? as i64,
        d.count(Scope::All).try_get()? as i64,
    ))
}
fn timeline_snapshot(
    db: &ModelContainer,
    scope: Scope,
    search: &str,
    sticky: &[u64],
) -> Result<(Vec<ArticleSummary>, usize), DbError> {
    let mut fetch = timeline_fetch(scope, search, TIMELINE_LIMIT);
    if scope == Scope::Unread && !sticky.is_empty() {
        fetch = timeline_fetch(Scope::All, search, TIMELINE_LIMIT);
        fetch.pred =
            fetch.pred & (daynews_db::scope_pred(Scope::Unread) | Pred::IdIn(sticky.to_vec()));
    }
    let rows = db
        .query::<Article>()
        .filter(fetch.pred)
        .sort(Article::published_at().desc())
        .limit(TIMELINE_LIMIT)
        .live()
        .try_collect()?;
    let mut feeds = HashMap::new();
    let mut result = Vec::with_capacity(rows.len());
    for a in rows {
        let feed_id = a.feed.id().map(|v| v.handle()).unwrap_or(0);
        if let std::collections::hash_map::Entry::Vacant(entry) = feeds.entry(feed_id) {
            let title = db
                .try_get::<daynews_db::Feed>(feed_id)?
                .and_then(|f| f.with_value_untracked(|f| f.map(|f| f.title.clone())))
                .unwrap_or_default();
            entry.insert(title);
        }
        result.push(ArticleSummary {
            id: a.id,
            feed_id,
            feed_title: feeds[&feed_id].clone(),
            title: a.title,
            url: a.url,
            author: a.author,
            published_at: a.published_at,
            summary: a.summary,
            is_read: a.is_read,
            is_starred: a.is_starred,
        });
    }
    let count = db
        .query::<Article>()
        .filter(daynews_db::scope_pred(scope) & Article::is_read().eq(false))
        .live_count()
        .try_get()?;
    Ok((result, count))
}
/// The subscribed feeds already holding articles at any of `links`, most first
/// (`Db::feeds_with_links`); empty when the library cannot be read.
pub async fn feeds_with_links(links: Vec<String>) -> Vec<(u64, usize)> {
    let Ok(worker) = ready_worker().await else {
        return Vec::new();
    };
    worker
        .read(move |db| {
            Db {
                container: db.clone(),
            }
            .feeds_with_links(&links)
        })
        .await
        .unwrap_or_default()
}

/// Read-only page preparation: never selects or marks an article read.
pub async fn preview_article(id: u64) -> Result<Option<StoredArticle>, DbError> {
    ready_worker()
        .await?
        .read(move |db| reader_snapshot(db, id))
        .await
}

fn reader_snapshot(db: &ModelContainer, id: u64) -> Result<Option<StoredArticle>, DbError> {
    let Some(a) = db
        .try_get::<Article>(id)?
        .and_then(|a| a.with_value_untracked(|a| a.cloned()))
    else {
        return Ok(None);
    };
    let feed_id = a.feed.id().map(|id| id.handle()).unwrap_or(0);
    let feed_title = db
        .try_get::<daynews_db::Feed>(feed_id)?
        .and_then(|f| f.with_value_untracked(|f| f.map(|f| f.title.clone())))
        .unwrap_or_default();
    let content_html = db
        .try_get::<daynews_db::ArticleBody>(id)?
        .and_then(|b| b.with_value_untracked(|b| b.map(|b| b.content_html.clone())));
    Ok(Some(StoredArticle {
        id,
        feed_id,
        feed_title,
        title: a.title,
        url: a.url,
        author: a.author,
        published_at: a.published_at,
        summary: a.summary,
        content_html,
        is_read: a.is_read,
        is_starred: a.is_starred,
    }))
}

pub fn select_scope(scope: Scope) {
    let sc = scene();
    batch(|| {
        sc.scope.set(scope);
        sc.selected.set(None);
        sc.reader_open.set(false);
        sc.sticky_read.set(vec![]);
    });
}
/// Lets the articles read in the unread scope drop out of it. A phone calls this when its
/// list closes, so returning to the same section starts from what is unread now: the scope
/// itself never changed, so `select_scope` alone would keep them.
pub fn release_sticky_read() {
    scene().sticky_read.set(vec![]);
}
pub fn set_search(text: &str) {
    scene().search.set(text.to_owned());
}
pub fn open_article(id: u64) {
    let sc = scene();
    batch(|| {
        set_read(id, true);
        sc.selected.set(Some(id));
        sc.reader_open.set(true);
    });
}
pub fn open_next_unread() -> bool {
    let sc = scene();
    let rows = sc.articles.get_untracked();
    let current = sc.selected.get_untracked();
    let start = current
        .and_then(|id| rows.iter().position(|r| r.id == id))
        .map(|i| i + 1)
        .unwrap_or(0);
    let next = rows[start.min(rows.len())..]
        .iter()
        .chain(rows[..start.min(rows.len())].iter())
        .find(|r| !r.is_read && Some(r.id) != current)
        .map(|r| r.id);
    if let Some(id) = next {
        open_article(id);
        return true;
    }
    let Some(worker) = worker() else {
        return false;
    };
    if state().total_unread.get_untracked() == 0 {
        return false;
    }
    let scope = sc.scope.get_untracked();
    let search = sc.search.get_untracked();
    let request = worker.read(move |db| {
        let q = db
            .query::<Article>()
            .filter(
                daynews_db::scope_pred(Scope::Unread) & !Pred::IdIn(current.into_iter().collect()),
            )
            .sort(Article::published_at().desc())
            .limit(1)
            .live();
        Ok(q.try_collect()?.first().map(|a| a.id))
    });
    // Capture the originating scene. An asynchronous completion must never target whatever
    // other window happens to become focused while the query runs.
    day_core::task(async move {
        match request.await {
            Ok(Some(id))
                if sc.selected.try_get() == Some(current)
                    && sc.scope.try_get() == Some(scope)
                    && sc.search.try_get().as_ref() == Some(&search) =>
            {
                batch(|| {
                    sc.scope.set(Scope::Unread);
                    sc.search.set(String::new());
                    sc.sticky_read.set(vec![id]);
                    sc.selected.set(Some(id));
                    sc.reader_open.set(true);
                });
                edit(move |db| {
                    db.set_read(id, true)?;
                    Ok(())
                });
            }
            Ok(_) => (),
            Err(e) => report(e),
        }
    });
    true
}
pub fn set_read(id: u64, read: bool) {
    let sc = scene();
    if read && sc.scope.get_untracked() == Scope::Unread {
        sc.sticky_read.update(|ids| {
            if !ids.contains(&id) {
                ids.push(id);
            }
        });
    }
    edit(move |db| {
        db.set_read(id, read)?;
        Ok(())
    });
}
pub fn toggle_read(id: u64) {
    let sc = scene();
    if sc.scope.get_untracked() == Scope::Unread {
        sc.sticky_read.update(|ids| {
            if !ids.contains(&id) {
                ids.push(id);
            }
        });
    }
    // Read-modify-write on the queue, never against a potentially stale UI snapshot.
    edit(move |db| {
        if let Some(a) = db.container.try_get::<Article>(id)? {
            db.set_read(id, !a.is_read().peek())?;
        }
        Ok(())
    });
}
pub fn set_starred(id: u64, starred: bool) {
    edit(move |db| {
        db.set_starred(id, starred);
        Ok(())
    });
}
pub fn toggle_tag(article: u64, name: &str) {
    let name = name.trim().to_owned();
    if name.is_empty() {
        return;
    }
    edit(move |db| {
        let tag = db.add_tag(&name);
        let tagged = db
            .container
            .try_get::<Article>(article)?
            .map(|a| a.tags().contains(tag))
            .unwrap_or(false);
        db.set_tagged(article, tag, !tagged);
        Ok(())
    });
}
pub fn mark_scope_read(read: bool) {
    let sc = scene();
    let scope = sc.scope.get_untracked();
    edit(move |db| {
        db.set_read_all(scope, read)?;
        Ok(())
    });
    sc.sticky_read.set(vec![]);
}
pub fn mark_feed_read(feed: u64, read: bool) {
    edit(move |db| {
        db.set_read_all(Scope::Feed(feed), read)?;
        Ok(())
    });
}
pub async fn prune(days: u32) -> usize {
    if days == 0 {
        return 0;
    }
    let worker = match ready_worker().await {
        Ok(w) => w,
        Err(e) => {
            report(e);
            return 0;
        }
    };
    match worker
        .write(move |db| {
            day_model::with_author("retention", || {
                Ok(Db {
                    container: db.clone(),
                }
                .prune_older_than(days))
            })
        })
        .await
    {
        Ok(n) => n,
        Err(e) => {
            report(e);
            0
        }
    }
}
pub fn subscribe(url: &str) {
    let url = normalize_feed_url(url);
    if url.is_empty() {
        return;
    }
    day_core::task(async move {
        let worker = match ready_worker().await {
            Ok(w) => w,
            Err(e) => {
                report(e);
                return;
            }
        };
        let copy = url.clone();
        match worker
            .write(move |db| {
                let db = Db {
                    container: db.clone(),
                };
                let id = db.add_feed(&copy, &fallback_title(&copy), None);
                db.sync_adopt_local(id)?;
                Ok(id)
            })
            .await
        {
            Ok(id) => {
                update_undo(&worker).await;
                refresh_one(id, url).await;
            }
            Err(e) => report(e),
        }
    });
}
/// Subscribe after discovery, retaining the original validators and body hash without refetching.
pub async fn subscribe_discovered(
    url: String,
    update: daynews_feed::FeedUpdate,
) -> Result<u64, String> {
    let worker = ready_worker().await.map_err(|e| e.to_string())?;
    let copy = url.clone();
    let id = worker
        .write(move |container| {
            let db = Db {
                container: container.clone(),
            };
            let id = db.add_feed(&copy, &fallback_title(&copy), None);
            db.sync_adopt_local(id)?;
            Ok(id)
        })
        .await
        .map_err(|e| e.to_string())?;
    if !apply_update(worker.clone(), id, url, Ok(update)).await {
        return Err("subscription import failed".into());
    }
    update_undo(&worker).await;
    sync_request_soon();
    Ok(id)
}
/// Fixture subscriptions use the same awaitable import, preserving bundled localization.
pub async fn subscribe_asset(url: String) -> Result<u64, String> {
    let parsed = preview_feed(&url).await.map_err(|e| e.to_string())?;
    subscribe_discovered(
        url,
        daynews_feed::FeedUpdate::Modified(parsed, Default::default(), None),
    )
    .await
}
pub fn unsubscribe(feed: u64) {
    edit(move |db| {
        db.delete_feed(feed)?;
        Ok(())
    });
    let sc = scene();
    if sc.scope.get_untracked() == Scope::Feed(feed) {
        select_scope(Scope::Unread);
    }
}
/// Create a collection (or find the one with this name), optionally moving `feed` into it.
pub fn create_folder(name: &str, feed: Option<u64>) {
    let name = name.trim().to_owned();
    if name.is_empty() {
        return;
    }
    edit(move |db| {
        let id = db.add_folder(&name);
        if let Some(feed) = feed {
            db.set_feed_folder(feed, Some(id), None)?;
        }
        Ok(())
    });
}
/// Rename a collection; an empty or already-used name is refused.
pub fn rename_folder(folder: u64, name: &str) {
    let name = name.to_owned();
    edit(move |db| db.rename_folder(folder, &name).map(|_| ()));
}
/// Delete a collection. Its feeds stay subscribed, outside any collection.
pub fn delete_folder(folder: u64) {
    edit(move |db| db.delete_folder(folder));
}
/// Write the sidebar's collection order: each collection's position.
pub fn set_folder_positions(positions: Vec<(u64, f64)>) {
    edit(move |db| {
        db.set_folder_positions(&positions);
        Ok(())
    });
}
/// Choose how the sidebar orders a collection's feeds.
pub fn set_folder_sort(folder: u64, sort: FeedSort) {
    edit(move |db| {
        db.set_folder_sort(folder, sort);
        Ok(())
    });
}
/// Put a feed in a collection (`None`: in none), before `before` in its manual order or last.
pub fn set_feed_folder(feed: u64, folder: Option<u64>, before: Option<u64>) {
    edit(move |db| db.set_feed_folder(feed, folder, before));
}
pub fn rename_feed(feed: u64, title: &str) {
    let title = title.to_owned();
    edit(move |db| {
        db.rename_feed(feed, &title)?;
        Ok(())
    });
}
pub fn move_feed(feed: u64, to: usize) {
    edit(move |db| db.move_feed(feed, to));
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

// Recheck both before issuing HTTP and when applying its response. Sync or the user can
// remove a subscription while a request waits for a slot or while it is in flight.
fn feed_is_current(db: &ModelContainer, id: u64, url: &str) -> Result<bool, DbError> {
    Ok(db.try_get::<daynews_db::Feed>(id)?.is_some_and(|feed| {
        !feed.unsubscribed().peek()
            && feed.sync_account().peek().is_none()
            && feed.feed_url().peek() == url
    }))
}

async fn refresh_one(id: u64, url: String) -> bool {
    if let Ok(w) = ready_worker().await {
        let managed = w
            .read(move |c| {
                Ok(c.try_get::<daynews_db::Feed>(id)?
                    .is_some_and(|f| f.sync_account().peek().is_some()))
            })
            .await;
        if matches!(managed, Ok(true)) {
            sync_auto_now();
            return true;
        }
    }
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
    let worker = match ready_worker().await {
        Ok(w) => w,
        Err(e) => {
            report(e);
            return false;
        }
    };
    let validators = worker
        .read({
            let url = url.clone();
            move |db| {
                if !feed_is_current(db, id, &url)? {
                    return Ok(None);
                }
                Ok(Db {
                    container: db.clone(),
                }
                .feed_cache(id))
            }
        })
        .await;
    let daynews_db::FeedCache {
        etag,
        last_modified,
        body_hash,
        retry_after,
        needs_publication_sample,
    } = match validators {
        Ok(Some(v)) => v,
        Ok(None) => return true,
        Err(e) => {
            report(e);
            return false;
        }
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
    apply_update(worker, id, url, result).await
}

async fn apply_update(
    worker: DatabaseWorker,
    id: u64,
    url: String,
    result: Result<daynews_feed::FeedUpdate, daynews_feed::FeedError>,
) -> bool {
    let applied = worker
        .write(move |container| {
            if !feed_is_current(container, id, &url)? {
                return Ok(true);
            }
            day_model::with_author("feed-refresh", || {
                let d = Db {
                    container: container.clone(),
                };
                Ok(match result {
                    Ok(daynews_feed::FeedUpdate::NotModified(validators)) => {
                        d.feed_checked(id, validators.etag, validators.last_modified);
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
                        {
                            d.update_feed_metadata(
                                id,
                                parsed.title.as_deref(),
                                parsed.site_url.as_deref(),
                                parsed.description.as_deref(),
                                parsed.icon_url.as_deref(),
                            );
                            // Unsubscribing while the request was in flight must not insert orphan articles.
                            if d.feed_cache(id).is_some() {
                                d.try_upsert_articles(id, &url, &items)?;
                                d.record_publications(
                                    id,
                                    items.iter().filter_map(|item| item.published),
                                );
                                d.set_feed_body_hash(id, body_hash);
                                d.feed_checked(id, validators.etag, validators.last_modified);
                            }
                        }
                        true
                    }
                    Err(e) => {
                        let until = match &e {
                            daynews_feed::FeedError::RetryAfter { until, .. } => Some(*until),
                            _ => None,
                        };
                        d.set_feed_retry_after(id, until);
                        d.set_feed_error(id, &e.to_string());
                        false
                    }
                })
            })
        })
        .await;
    match applied {
        Ok(ok) => ok,
        Err(e) => {
            report(e);
            false
        }
    }
}

/// Import subscriptions, creating folders as needed. Returns (added, already present).
pub async fn import_opml(text: &str) -> std::result::Result<(usize, usize), String> {
    let doc = daynews_opml::parse(text).map_err(|e| e.to_string())?;
    let entries: Vec<_> = doc
        .feeds()
        .into_iter()
        .map(|(path, feed)| (path, feed.clone()))
        .collect();
    let worker = ready_worker().await.map_err(|e| e.to_string())?;
    let (added, existing) = worker
        .write(move |container| {
            let d = Db {
                container: container.clone(),
            };
            let (mut added, mut existing) = (0usize, 0usize);
            for (path, feed) in &entries {
                // Only the innermost folder becomes a folder; deeper nesting is rare and flattening
                // it matches what the sidebar can represent.
                let folder = path.last().map(|name| d.add_folder(name));
                let url = feed.xml_url.clone();
                if d.container.get::<daynews_db::Feed>(feed_id(&url)).is_some() {
                    existing += 1;
                    continue;
                }
                let id = d.add_feed(&url, &feed.display_title(), folder);
                d.sync_adopt_local(id)?;
                added += 1;
            }
            Ok((added, existing))
        })
        .await
        .map_err(|e| e.to_string())?;
    update_undo(&worker).await;
    state()
        .status
        .set(status(StatusMessage::Imported(added, existing)));
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
        title: Some(status(StatusMessage::ExportTitle)),
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

/// The app's data directory, where the library lives: `day_spec::storage::data_dir()`, a folder
/// named for the app where its platform keeps app data, or `DAY_DATA_DIR`, which a test or a UI
/// validation run sets so it never touches a reader's subscriptions.
pub fn store_dir() -> PathBuf {
    day_spec::storage::data_dir().unwrap_or_else(std::env::temp_dir)
}

/// The app's cache directory, for what can be fetched again (feed icons):
/// `day_spec::storage::cache_dir()`.
pub fn cache_dir() -> PathBuf {
    day_spec::storage::cache_dir()
        .unwrap_or_else(|| std::env::temp_dir().join("fleet-reader-cache"))
}

/// Subscribe only while a dashboard is mounted; scope disposal cancels obsolete projections.
pub fn watch_dashboard(
    scope: Signal<Scope>,
    local_day: Signal<i64>,
    output: Signal<Option<daynews_db::dashboard::Dashboard>>,
) {
    let st = store();
    Effect::new(move || {
        let scope = scope.get();
        let _ = local_day.get();
        if !st.ready.get() {
            return;
        }
        let Some(worker) = worker() else {
            return;
        };
        scoped_task(async move {
            let cache = RefCell::new(daynews_db::dashboard::DashboardCache::default());
            let observed = worker
                .observe(move |container| {
                    Db {
                        container: container.clone(),
                    }
                    .dashboard_cached(
                        scope,
                        daynews_time::now_unix(),
                        &mut cache.borrow_mut(),
                    )
                })
                .await;
            let mut observed = match observed {
                Ok(value) => value,
                Err(error) => {
                    report(error);
                    return;
                }
            };
            while let Some(snapshot) = observed.next().await {
                match snapshot {
                    Ok(snapshot) => output.set_if_changed(Some(snapshot.value)),
                    Err(error) => report(error),
                }
            }
        });
    });
}

/// How many words the dashboard's word cloud draws at most.
pub const CLOUD_WORDS: usize = 90;

/// The reader's language, as the ISO 639-1 code the word cloud's stopwords fall back to.
fn reader_language() -> String {
    day_localization::locale()
        .get()
        .split(['-', '_'])
        .next()
        .unwrap_or("en")
        .to_ascii_lowercase()
}

/// Word clouds already counted, newest last, by what they were counted for: the scope, the
/// local day, the number of stored articles and the stopword fallback language. An overview
/// built again (a search changes the list's selection, the reader closes) shows its cloud at
/// once rather than queueing the count again in front of the list's own query.
type CloudKey = (Scope, i64, i64, String);
thread_local! {
    static CLOUDS: RefCell<Vec<(CloudKey, Vec<daynews_db::dashboard::CloudWord>)>> =
        const { RefCell::new(Vec::new()) };
}
const CLOUD_CACHE: usize = 12;

/// The dashboard's word cloud for `scope`, read off the search index on the worker
/// (`Db::word_cloud`). It follows the scope, the day and the number of stored articles, but not
/// read marks: a cloud is the gist of the articles, and recounting it on every article read
/// would be work no reader could see.
pub fn watch_word_cloud(
    scope: Signal<Scope>,
    local_day: Signal<i64>,
    output: Signal<Option<Vec<daynews_db::dashboard::CloudWord>>>,
) {
    let st = store();
    let totals = state();
    Effect::new(move || {
        let scope = scope.get();
        let key = (
            scope,
            local_day.get(),
            totals.total_articles.get(),
            reader_language(),
        );
        if let Some(words) = CLOUDS.with(|c| {
            c.borrow()
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, w)| w.clone())
        }) {
            output.set_if_changed(Some(words));
            return;
        }
        let fallback = key.3.clone();
        if !st.ready.get() {
            return;
        }
        let Some(worker) = worker() else {
            return;
        };
        scoped_task(async move {
            let words = worker
                .read(move |db| {
                    Db {
                        container: db.clone(),
                    }
                    .word_cloud(
                        scope,
                        daynews_time::now_unix(),
                        CLOUD_WORDS,
                        &fallback,
                    )
                })
                .await;
            match words {
                Ok(words) => {
                    CLOUDS.with(|c| {
                        let mut c = c.borrow_mut();
                        c.retain(|(k, _)| *k != key);
                        c.push((key, words.clone()));
                        let excess = c.len().saturating_sub(CLOUD_CACHE);
                        c.drain(..excess);
                    });
                    output.set_if_changed(Some(words));
                }
                Err(error) => report(error),
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        future::Future,
        sync::Arc,
        task::{Context, Poll, Wake, Waker},
        time::{Duration, Instant},
    };
    pub(super) fn wait<F: Future>(future: F) -> F::Output {
        struct Unpark(std::thread::Thread);
        impl Wake for Unpark {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
        let mut cx = Context::from_waker(&waker);
        let mut future = std::pin::pin!(future);
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
                return value;
            }
            assert!(Instant::now() < deadline, "article worker timed out");
            std::thread::park_timeout(Duration::from_millis(10));
        }
    }
    const URL: &str = "https://synthetic.example/feed";
    fn item(id: &str, title: &str, body: &str, published: i64) -> IncomingArticle {
        IncomingArticle {
            guid: id.into(),
            title: Some(title.into()),
            url: Some(format!("https://synthetic.example/{id}")),
            author: None,
            published: Some(published),
            summary: None,
            content_html: Some(format!("<p>{body}</p>")),
        }
    }
    fn seeded() -> DatabaseWorker {
        let worker = wait(DatabaseWorker::open(|| {
            Db::open_in_memory().map(|db| db.container)
        }))
        .unwrap();
        wait(worker.write(|container| {
            let db = Db {
                container: container.clone(),
            };
            let feed = db.add_feed(URL, "Synthetic publication", None);
            db.upsert_articles(
                feed,
                URL,
                &[
                    item("1", "Moon landing", "celestial body", 100),
                    item("2", "Other article", "uniqueparallax", 200),
                ],
            );
            Ok(())
        }))
        .unwrap();
        worker
    }
    #[test]
    fn quit_cancels_pending_feed_guards_before_closing_the_store() {
        const CHILD: &str = "DAY_NEWS_QUIT_CHILD";
        if std::env::var_os(CHILD).is_some() {
            std::thread::spawn(|| {
                assert!(!day_core::has_tree());
                let store = store();
                let worker = seeded();
                *store.worker.borrow_mut() = Some(worker);
                let active = state().updating_feeds;
                for id in 1..=4 {
                    store.scheduled_feeds.borrow_mut().insert(id);
                    active.update(|feeds| {
                        feeds.insert(id, None);
                    });
                    let guard = UpdatingFeed {
                        store: store.clone(),
                        id,
                        active,
                    };
                    day_core::task(std::future::poll_fn(move |_| {
                        let _ = &guard;
                        Poll::Pending
                    }));
                }
                day_core::on_lifecycle(day_spec::Lifecycle::WillTerminate, move || {
                    assert!(active.get_untracked().is_empty());
                    assert!(store.scheduled_feeds.borrow().is_empty());
                    shutdown();
                });
                day_core::dispatch_lifecycle(day_spec::Lifecycle::WillTerminate);
            })
            .join()
            .unwrap();
            return;
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "native::tests::quit_cancels_pending_feed_guards_before_closing_the_store",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    }
    #[test]
    fn worker_search_reads_title_and_body_indexes_and_handles_user_syntax() {
        let worker = seeded();
        for (term, expected) in [
            ("moon", vec![article_id(URL, "1")]),
            ("uniqueparallax", vec![article_id(URL, "2")]),
            ("doesnotexist", vec![]),
            ("\" AND (", vec![]),
        ] {
            let result =
                wait(worker.read(move |db| timeline_snapshot(db, Scope::All, term, &[]))).unwrap();
            assert_eq!(result.0.iter().map(|a| a.id).collect::<Vec<_>>(), expected);
        }
        worker.close_blocking().unwrap();
    }
    #[test]
    fn independent_search_subscriptions_keep_their_own_scopes() {
        let worker = seeded();
        let mut moon =
            wait(worker.observe(|db| timeline_snapshot(db, Scope::All, "moon", &[]))).unwrap();
        let mut body =
            wait(worker.observe(|db| timeline_snapshot(db, Scope::All, "uniqueparallax", &[])))
                .unwrap();
        assert_eq!(
            wait(moon.next()).unwrap().unwrap().value.0[0].id,
            article_id(URL, "1")
        );
        assert_eq!(
            wait(body.next()).unwrap().unwrap().value.0[0].id,
            article_id(URL, "2")
        );
        wait(worker.write(|db| {
            db.try_get::<Article>(article_id(URL, "1"))?
                .unwrap()
                .is_starred()
                .write(true);
            Ok(())
        }))
        .unwrap();
        assert!(wait(moon.next()).unwrap().unwrap().value.0[0].is_starred);
        worker.close_blocking().unwrap();
    }
    #[test]
    fn sticky_read_rows_and_sidebar_counts_reflect_committed_edits() {
        let worker = seeded();
        wait(worker.write(|db| {
            db.try_get::<Article>(article_id(URL, "1"))?
                .unwrap()
                .is_read()
                .write(true);
            Ok(())
        }))
        .unwrap();
        let (rows, count) = wait(
            worker.read(|db| timeline_snapshot(db, Scope::Unread, "", &[article_id(URL, "1")])),
        )
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(count, 1);
        let sidebar = wait(worker.read(sidebar_snapshot)).unwrap();
        assert_eq!(sidebar.0[0].unread, 1);
        assert_eq!(sidebar.3, 1);
        assert_eq!(sidebar.6, 2);
        let filtered = wait(
            worker.read(|db| timeline_snapshot(db, Scope::Unread, "moon", &[article_id(URL, "1")])),
        )
        .unwrap();
        assert_eq!(filtered.0.len(), 1);
        assert!(filtered.0[0].is_read);
        worker.close_blocking().unwrap();
    }
    #[test]
    fn concurrent_refresh_and_read_edits_preserve_read_state() {
        let worker = seeded();
        let importer = worker.clone();
        let refresh = std::thread::spawn(move || {
            for _ in 0..40 {
                wait(importer.write(|container| {
                    day_model::with_author("feed-refresh", || {
                        let db = Db {
                            container: container.clone(),
                        };
                        db.upsert_articles(
                            feed_id(URL),
                            URL,
                            &[item("1", "Moon landing", "updated body", 100)],
                        );
                        Ok(())
                    })
                }))
                .unwrap();
            }
        });
        wait(worker.write(|db| {
            db.try_get::<Article>(article_id(URL, "1"))?
                .unwrap()
                .is_read()
                .write(true);
            Ok(())
        }))
        .unwrap();
        refresh.join().unwrap();
        let article = wait(worker.read(|db| reader_snapshot(db, article_id(URL, "1"))))
            .unwrap()
            .unwrap();
        assert!(article.is_read);
        assert_eq!(
            article.content_html.as_deref(),
            Some("<p>celestial body</p>")
        );
        worker.close_blocking().unwrap();
    }
    #[test]
    fn preparing_a_page_does_not_select_or_mark_it_read() {
        let scope = day_reactive::Scope::child();
        scope.enter(|| {
            let worker = seeded();
            *store().worker.borrow_mut() = Some(worker.clone());
            let scene = NewsScene::create();
            day_reactive::Scope::current().provide(scene);
            let id = article_id(URL, "1");
            let preview = wait(preview_article(id)).unwrap().unwrap();
            assert!(!preview.is_read);
            assert_eq!(scene.selected.get_untracked(), None);
            assert!(!scene.reader_open.get_untracked());
            assert!(
                !wait(worker.read(move |db| reader_snapshot(db, id)))
                    .unwrap()
                    .unwrap()
                    .is_read
            );
            worker.close_blocking().unwrap();
        });
        scope.dispose();
    }

    #[test]
    fn deleted_article_clears_reader_projection_after_commit() {
        let worker = seeded();
        let mut reader =
            wait(worker.observe(|db| reader_snapshot(db, article_id(URL, "1")))).unwrap();
        assert!(wait(reader.next()).unwrap().unwrap().value.is_some());
        wait(worker.write(|db| {
            db.delete::<Article>(article_id(URL, "1"))?;
            Ok(())
        }))
        .unwrap();
        assert!(wait(reader.next()).unwrap().unwrap().value.is_none());
        worker.close_blocking().unwrap();
    }
    #[test]
    fn rapid_search_cancellation_keeps_latest_subscription() {
        let worker = seeded();
        for i in 0..100 {
            let term = format!("obsolete{i}");
            drop(
                wait(worker.observe(move |db| timeline_snapshot(db, Scope::All, &term, &[])))
                    .unwrap(),
            );
        }
        let mut current =
            wait(worker.observe(|db| timeline_snapshot(db, Scope::All, "uniqueparallax", &[])))
                .unwrap();
        let result = wait(current.next()).unwrap().unwrap().value;
        assert_eq!(result.0.len(), 1);
        assert_eq!(result.0[0].id, article_id(URL, "2"));
        worker.close_blocking().unwrap();
    }
}

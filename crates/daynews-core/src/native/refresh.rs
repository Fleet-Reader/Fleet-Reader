//! Publisher refresh follows a bounded library sync. This coordinator runs on the UI thread;
//! feed selection always reads the worker after sync, never an observer's stale projection.
use super::*;
use futures_util::{StreamExt, stream};

#[derive(Clone, Copy)]
enum Selection {
    All,
    Due,
    One(u64),
    #[cfg(feature = "experimental-webdav")]
    Received,
}

thread_local! {
    static SYNC_PENDING: Signal<bool> = Signal::new_in(day_reactive::Scope::root(), false);
    static RECEIVED: RefCell<HashSet<u64>> = RefCell::new(HashSet::new());
}

/// Tracked: content may be fresh while the library's shared state is still pending.
pub fn refresh_sync_pending() -> bool {
    SYNC_PENDING.with(|pending| pending.get())
}

#[cfg(feature = "experimental-webdav")]
pub(super) fn set_sync_pending(pending: bool) {
    SYNC_PENDING.with(|signal| signal.set_if_changed(pending));
}

pub fn refresh_all() {
    start(Selection::All, true);
}

/// Fixed-interval refresh, respecting automatic sync backoff.
pub fn refresh_scheduled() {
    start(Selection::All, false);
}

pub fn refresh_due() {
    start(Selection::Due, false);
}

pub fn refresh_feed(id: u64) {
    start(Selection::One(id), true);
}

/// New or reactivated subscriptions received by any DAV pass, including one that completed
/// after the refresh's timeout. Never starts another sync pass.
#[cfg(feature = "experimental-webdav")]
pub(super) fn received_subscriptions(ids: impl IntoIterator<Item = u64>) {
    RECEIVED.with(|pending| pending.borrow_mut().extend(ids));
    if RECEIVED.with(|pending| !pending.borrow().is_empty()) {
        start(Selection::Received, false);
    }
}

struct RefreshGuard;
impl Drop for RefreshGuard {
    fn drop(&mut self) {
        state().refresh_progress.set(None);
    }
}

fn start(selection: Selection, manual: bool) {
    if state().refresh_progress.get_untracked().is_some() {
        return;
    }
    // Reserve before spawning so concurrent timer/gesture/activation requests coalesce.
    state().refresh_progress.set(Some((0, 0)));
    day_core::task(async move {
        let _guard = RefreshGuard;
        if let Err(error) = run(selection, manual).await {
            report(error);
        }
    });
}

pub(super) async fn active_feeds(worker: &DatabaseWorker) -> Result<Vec<(u64, String)>, DbError> {
    worker
        .read(|c| {
            Ok(c.query::<daynews_db::Feed>()
                .filter(
                    daynews_db::Feed::sync_account().is_null()
                        & daynews_db::Feed::unsubscribed().eq(false),
                )
                .sort(daynews_db::Feed::position().asc())
                .sort(daynews_db::Feed::title().asc())
                .live()
                .try_collect()?
                .into_iter()
                .map(|f| (f.id, f.feed_url))
                .collect())
        })
        .await
}

async fn select_feeds(
    worker: &DatabaseWorker,
    selection: Selection,
) -> Result<Vec<(u64, String)>, DbError> {
    match selection {
        Selection::Due => {
            worker
                .read(|c| {
                    Db {
                        container: c.clone(),
                    }
                    .due_feeds(daynews_time::now_unix())
                })
                .await
        }
        _ => Ok(active_feeds(worker)
            .await?
            .into_iter()
            .filter(|(id, _)| match selection {
                Selection::One(wanted) => *id == wanted,
                #[cfg(feature = "experimental-webdav")]
                Selection::Received => false,
                _ => true,
            })
            .collect()),
    }
}

/// The synchronization barrier is injected to exercise the production ordering with real
/// databases and controlled network completion in tests.
async fn prepare(
    worker: &DatabaseWorker,
    selection: Selection,
    sync: impl std::future::Future<Output = bool>,
) -> Result<(bool, Vec<(u64, String)>), DbError> {
    let synced = sync.await;
    Ok((synced, select_feeds(worker, selection).await?))
}

async fn before_refresh(manual: bool) -> bool {
    #[cfg(all(feature = "feedbin", feature = "experimental-webdav"))]
    if manual {
        sync_now();
    } else {
        sync_auto_now();
    }

    #[cfg(feature = "experimental-webdav")]
    {
        webdav::before_refresh(manual).await
    }
    #[cfg(not(feature = "experimental-webdav"))]
    {
        if manual {
            sync_now();
        }
        true
    }
}

async fn run(selection: Selection, manual: bool) -> Result<(), DbError> {
    let worker = ready_worker().await?;
    // The minute timer must not provoke a DAV round trip when no publishers are due.
    if matches!(selection, Selection::Due)
        && select_feeds(&worker, selection).await?.is_empty()
        && RECEIVED.with(|pending| pending.borrow().is_empty())
    {
        return Ok(());
    }
    let (synced, feeds) = prepare(&worker, selection, async {
        match selection {
            #[cfg(feature = "experimental-webdav")]
            Selection::Received => true,
            _ => before_refresh(manual).await,
        }
    })
    .await?;
    let (done, failed) = fetch_batches(&worker, feeds, refresh_one).await?;
    state().status.set(status(if synced {
        StatusMessage::RefreshedFeeds(done, failed)
    } else {
        StatusMessage::RefreshedFeedsPendingSync(done, failed)
    }));
    Ok(())
}

async fn fetch_batches<F: std::future::Future<Output = bool>>(
    worker: &DatabaseWorker,
    mut feeds: Vec<(u64, String)>,
    mut fetch: impl FnMut(u64, String) -> F,
) -> Result<(usize, usize), DbError> {
    let mut handled = HashSet::new();
    let mut done = 0;
    let mut failed = 0;
    loop {
        // Drain only after a successful database read. New subscriptions arriving while
        // publishers are in flight join the next batch, without repeating completed feeds.
        if RECEIVED.with(|pending| !pending.borrow().is_empty()) {
            let active = active_feeds(worker).await?;
            let received = RECEIVED.with(|pending| std::mem::take(&mut *pending.borrow_mut()));
            feeds.extend(active.into_iter().filter(|(id, _)| received.contains(id)));
        }
        feeds.retain(|(id, _)| handled.insert(*id));
        if feeds.is_empty() {
            break;
        }
        let total = done + feeds.len();
        state().refresh_progress.set(Some((done, total)));
        let mut pending = stream::iter(std::mem::take(&mut feeds))
            .map(|(id, url)| fetch(id, url))
            .buffer_unordered(4);
        while let Some(ok) = pending.next().await {
            failed += usize::from(!ok);
            done += 1;
            state().refresh_progress.set(Some((done, total)));
        }
    }
    Ok((done, failed))
}

#[cfg(test)]
mod tests;

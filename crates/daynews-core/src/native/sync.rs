// SPDX-License-Identifier: Apache-2.0
//! One account session per application, independent of window lifetimes.
use super::*;
use daynews_db::sync::AccountFields;
use daynews_sync::progress::{Phase, Progress};
use daynews_sync::{
    engine::{self, Storage},
    *,
};
use futures_util::future::{AbortHandle, Abortable};
use std::{
    cell::Cell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

struct SyncRun {
    abort: AbortHandle,
    cancelled: Arc<AtomicBool>,
}
struct Session {
    run: RefCell<Option<SyncRun>>,
    status: Signal<SyncStatus>,
    provider: RefCell<Option<Rc<dyn Provider>>>,
    candidate: RefCell<Option<Credentials>>,
    generation: Cell<u64>,
    account: Cell<Option<u64>>,
    installed: Cell<bool>,
    next_attempt: Cell<i64>,
    failures: Cell<u32>,
    retention_days: Cell<u32>,
}
thread_local! {static SESSION:Session=Session {
    run:RefCell::new(None),
    status:Signal::new_in(day_reactive::Scope::root(),SyncStatus::default()),
    provider:RefCell::new(None),candidate:RefCell::new(None),generation:Cell::new(0),account:Cell::new(None),
    installed:Cell::new(false),next_attempt:Cell::new(0),failures:Cell::new(0),
    retention_days:Cell::new(DEFAULT_RETENTION_DAYS),
};}
pub fn sync_status() -> Signal<SyncStatus> {
    SESSION.with(|s| s.status)
}
fn generation() -> u64 {
    SESSION.with(|s| s.generation.get())
}
fn change_session() -> u64 {
    SESSION.with(|s| {
        if let Some(run) = s.run.borrow_mut().take() {
            run.cancelled.store(true, Ordering::SeqCst);
            run.abort.abort();
        }
        let g = s.generation.get() + 1;
        s.generation.set(g);
        g
    })
}
fn active(g: u64) -> bool {
    generation() == g
}
fn fixture() -> bool {
    std::env::var("DAY_NEWS_SYNC_TEST").ok().as_deref() == Some("1")
        && std::env::var("DAY_NEWS_SYNC_TEST_SECURE_STORE")
            .ok()
            .as_deref()
            != Some("1")
}
fn credential_key(account: u64) -> String {
    format!("account:{account}")
}

async fn load_secret(account: u64) -> Result<Option<String>> {
    if fixture() {
        return Ok(Some("fixture-password".into()));
    }
    let key = credential_key(account);
    #[cfg(any(target_os = "android", target_env = "ohos"))]
    let result = daynews_credentials::load(&key).await;
    #[cfg(not(any(target_os = "android", target_env = "ohos")))]
    let result = day_async::spawn_blocking(move || daynews_credentials::load(&key))
        .await
        .map_err(|_| Error::Storage)?;
    result.map_err(|_| Error::Storage)
}
async fn save_secret(account: u64, password: String) -> Result<()> {
    if fixture() {
        return Ok(());
    }
    let key = credential_key(account);
    #[cfg(any(target_os = "android", target_env = "ohos"))]
    let result = daynews_credentials::save(&key, &password);
    #[cfg(not(any(target_os = "android", target_env = "ohos")))]
    let result = day_async::spawn_blocking(move || daynews_credentials::save(&key, &password))
        .await
        .map_err(|_| Error::Storage)?;
    result.map_err(|_| Error::Storage)
}
async fn remove_secret(account: u64) -> Result<()> {
    if fixture() {
        return Ok(());
    }
    let key = credential_key(account);
    #[cfg(any(target_os = "android", target_env = "ohos"))]
    let result = daynews_credentials::remove(&key);
    #[cfg(not(any(target_os = "android", target_env = "ohos")))]
    let result = day_async::spawn_blocking(move || daynews_credentials::remove(&key))
        .await
        .map_err(|_| Error::Storage)?;
    result.map_err(|_| Error::Storage)
}

pub fn sync_install() {
    #[cfg(feature = "experimental-webdav")]
    super::webdav::install();
    if SESSION.with(|s| s.installed.replace(true)) {
        return;
    }
    day_core::task(async {
        let g = generation();
        let result = async {
            let w = ready_worker().await.map_err(|_| Error::Storage)?;
            let accounts = w
                .read(|c| {
                    Db {
                        container: c.clone(),
                    }
                    .sync_accounts()
                })
                .await
                .map_err(|_| Error::Storage)?;
            if let Some(a) = accounts.into_iter().find(|a| a.connected) {
                if !active(g) {
                    return Ok(());
                }
                SESSION.with(|s| s.account.set(Some(a.id)));
                sync_status().set(SyncStatus {
                    username: a.username.clone(),
                    connected: true,
                    last_sync: a.last_sync,
                    ..Default::default()
                });
                let password = load_secret(a.id).await?.ok_or(Error::Authentication)?;
                if !active(g) {
                    return Ok(());
                }
                // Provider construction is centralized; adding another provider belongs here.
                let provider = make_provider(
                    &a.provider,
                    Credentials {
                        username: a.username.clone(),
                        password,
                    },
                )?;
                SESSION.with(|s| {
                    s.account.set(Some(a.id));
                    *s.provider.borrow_mut() = Some(provider);
                });
                sync_status().set(SyncStatus {
                    username: a.username,
                    connected: true,
                    last_sync: a.last_sync,
                    ..Default::default()
                });
                sync_now();
            }
            Ok::<_, Error>(())
        }
        .await;
        if active(g)
            && let Err(e) = result
        {
            sync_status().update(|s| s.error = Some(e));
        }
        loop {
            day_core::sleep(5_000).await;
            let st = sync_status().get_untracked();
            if st.connected
                && !st.paused
                && !st.busy
                && st.error != Some(Error::Authentication)
                && SESSION.with(|s| s.next_attempt.get() <= daynews_time::now_unix())
            {
                sync_now();
            }
        }
    });
}
fn make_provider(kind: &str, credentials: Credentials) -> Result<Rc<dyn Provider>> {
    match kind {
        "feedbin" => Ok(Rc::new(daynews_sync::feedbin::Feedbin::new(credentials))),
        _ => Err(Error::Unsupported),
    }
}

pub fn sync_sign_in(username: String, password: String) {
    if sync_status().get_untracked().busy {
        return;
    }
    let g = change_session();
    sync_status().update(|s| {
        s.busy = true;
        s.error = None;
        s.preview = false;
        s.progress = None;
        s.cancellable = false;
        s.remaining_seconds = None;
    });
    day_core::task(async move {
        let credentials = Credentials {
            username: username.trim().to_string(),
            password,
        };
        let result = async {
            let provider = make_provider("feedbin", credentials.clone())?;
            provider.authenticate().await?;
            let changes = provider.subscriptions().await?;
            let count = match changes {
                Changes::Snapshot(rows) => rows.len(),
                _ => return Err(Error::InvalidResponse),
            };
            let w = ready_worker().await.map_err(|_| Error::Storage)?;
            let local_feeds = w
                .read(|c| {
                    Ok(c.query::<daynews_db::Feed>()
                        .filter(
                            daynews_db::Feed::sync_account().is_null()
                                & daynews_db::Feed::unsubscribed().eq(false),
                        )
                        .live()
                        .try_collect()?
                        .into_iter()
                        .filter(|f| {
                            f.feed_url.starts_with("https://") || f.feed_url.starts_with("http://")
                        })
                        .count())
                })
                .await
                .map_err(|_| Error::Storage)?;
            if !active(g) {
                return Ok(());
            }
            SESSION.with(|s| {
                *s.provider.borrow_mut() = Some(provider);
                *s.candidate.borrow_mut() = Some(credentials.clone());
            });
            sync_status().set(SyncStatus {
                username: credentials.username,
                preview: true,
                remote_feeds: count,
                local_feeds,
                ..Default::default()
            });
            Ok::<_, Error>(())
        }
        .await;
        if active(g) {
            sync_status().update(|s| {
                s.busy = false;
                if let Err(e) = result {
                    s.error = Some(e);
                }
            });
        }
    });
}
pub fn sync_connect(upload_local: bool) {
    if sync_status().get_untracked().busy {
        return;
    }
    let Some(credentials) = SESSION.with(|s| s.candidate.borrow().clone()) else {
        return;
    };
    let g = generation();
    sync_status().update(|s| {
        s.busy = true;
        s.error = None;
    });
    day_core::task(async move {
        let result = async {
            let account = daynews_db::sync::account_id("feedbin", &credentials.username);
            save_secret(account, credentials.password).await?;
            if !active(g) {
                return Ok(());
            }
            let w = ready_worker().await.map_err(|_| Error::Storage)?;
            w.write(move |c| {
                day_model::with_author("sync", || {
                    let db = Db {
                        container: c.clone(),
                    };
                    db.sync_connect("feedbin", &credentials.username)?;
                    if upload_local {
                        for feed in c
                            .query::<daynews_db::Feed>()
                            .filter(
                                daynews_db::Feed::sync_account().is_null()
                                    & daynews_db::Feed::unsubscribed().eq(false),
                            )
                            .live()
                            .try_collect()?
                        {
                            if feed.feed_url.starts_with("https://")
                                || feed.feed_url.starts_with("http://")
                            {
                                db.sync_adopt_local(feed.id)?;
                            }
                        }
                    }
                    Ok(())
                })
            })
            .await
            .map_err(|_| Error::Storage)?;
            if !active(g) {
                return Ok(());
            }
            SESSION.with(|s| {
                s.account.set(Some(account));
                s.candidate.borrow_mut().take();
                s.next_attempt.set(0);
            });
            sync_status().update(|s| {
                s.connected = true;
                s.preview = false;
            });
            Ok::<_, Error>(())
        }
        .await;
        if active(g) {
            sync_status().update(|s| {
                s.busy = false;
                if let Err(e) = result {
                    s.error = Some(e);
                }
            });
            if sync_status().get_untracked().connected {
                sync_now();
            }
        }
    });
}
pub fn sync_cancel_login() {
    change_session();
    SESSION.with(|s| {
        s.candidate.borrow_mut().take();
        s.provider.borrow_mut().take();
    });
    sync_status().set(SyncStatus::default());
}
pub fn sync_disconnect() {
    let g = change_session();
    let account = SESSION.with(|s| {
        s.provider.borrow_mut().take();
        s.candidate.borrow_mut().take();
        s.account.take()
    });
    sync_status().set(SyncStatus {
        busy: account.is_some(),
        ..Default::default()
    });
    if let Some(account) = account {
        day_core::task(async move {
            let result = async {
                let w = ready_worker().await.map_err(|_| Error::Storage)?;
                w.write(move |c| {
                    day_model::with_author("sync", || {
                        Db {
                            container: c.clone(),
                        }
                        .sync_disconnect(account);
                        Ok(())
                    })
                })
                .await
                .map_err(|_| Error::Storage)?;
                remove_secret(account).await
            }
            .await;
            if active(g) {
                sync_status().update(|s| {
                    s.busy = false;
                    s.error = result.err();
                });
            }
        });
    }
}

pub fn sync_request_soon() {
    #[cfg(feature = "experimental-webdav")]
    super::webdav::request_soon();
    if sync_status().get_untracked().error.is_some() {
        return;
    }
    SESSION.with(|s| {
        s.next_attempt.set(daynews_time::now_unix() + 2);
    });
}
/// Automatic triggers respect a user's cancellation for the remainder of this session.
pub fn sync_auto_now() {
    if !sync_status().get_untracked().paused {
        sync_now();
    }
}
pub fn sync_cancel() {
    if !sync_status().get_untracked().cancellable {
        return;
    }
    change_session();
    sync_status().update(|s| {
        s.busy = false;
        s.cancellable = false;
        s.paused = true;
        s.remaining_seconds = None;
        s.error = None;
    });
    log::info!("sync cancelled; automatic synchronization paused");
}

/// Called only after the app's destructive confirmation. Keep the credential and the
/// connected account, but remove every local library record and pending remote mutation.
pub fn sync_reset_local() {
    let status = sync_status().get_untracked();
    if status.busy
        || !status.connected
        || state().refresh_progress.get_untracked().is_some()
        || !state().updating_feeds.get_untracked().is_empty()
    {
        return;
    }
    let Some(account) = SESSION.with(|s| s.account.get()) else {
        return;
    };
    if SESSION.with(|s| s.provider.borrow().is_none()) {
        sync_status().update(|s| s.error = Some(Error::Authentication));
        return;
    }
    let g = change_session();
    sync_status().update(|s| {
        s.busy = true;
        s.cancellable = false;
        s.error = None;
        s.paused = true;
        s.progress = Some(Progress {
            phase: Phase::Resetting,
            ..Default::default()
        });
        s.elapsed_seconds = 0;
        s.remaining_seconds = None;
    });
    day_core::task(async move {
        let result = async {
            let worker = ready_worker().await.map_err(|_| Error::Storage)?;
            worker
                .write_resetting_undo(move |c| {
                    Db {
                        container: c.clone(),
                    }
                    .sync_reset_local(account)
                })
                .await
                .map_err(|_| Error::Storage)?;
            update_undo(&worker).await;
            Ok::<_, Error>(())
        }
        .await;
        if !active(g) {
            return;
        }
        let succeeded = result.is_ok();
        sync_status().update(|s| {
            s.busy = false;
            s.error = result.err();
            s.progress = None;
            if succeeded {
                s.pending = 0;
                s.last_sync = None;
            }
        });
        if succeeded {
            log::info!("sync local library reset; downloading authoritative remote state");
            sync_now();
        }
    });
}

/// The app's persisted Keep Articles preference; read once at the next sync boundary.
pub fn set_sync_retention_days(days: u32) {
    SESSION.with(|s| s.retention_days.set(days));
}

fn history_since(days: u32, now: i64) -> Option<i64> {
    // Round down to UTC midnight: at most one day of grace, with a stable query/ETag for
    // repeated syncs that day. Zero is the existing Keep Forever setting.
    (days != 0).then(|| now.div_euclid(86_400) * 86_400 - i64::from(days) * 86_400)
}

pub fn sync_now() {
    let status = sync_status().get_untracked();
    if status.busy || !status.connected {
        return;
    }
    let Some((provider, account)) =
        SESSION.with(|s| Some((s.provider.borrow().clone()?, s.account.get()?)))
    else {
        return;
    };
    let g = change_session();
    let (abort, registration) = AbortHandle::new_pair();
    let cancelled = Arc::new(AtomicBool::new(false));
    SESSION.with(|s| {
        *s.run.borrow_mut() = Some(SyncRun {
            abort,
            cancelled: cancelled.clone(),
        })
    });
    sync_status().update(|s| {
        s.busy = true;
        s.cancellable = true;
        s.paused = false;
        s.error = None;
        s.progress = Some(Progress::default());
        s.elapsed_seconds = 0;
        s.remaining_seconds = None;
    });
    let started = Instant::now();
    let estimate_deadline = Rc::new(Cell::new(None::<Instant>));
    let ticker_deadline = estimate_deadline.clone();
    day_core::task(async move {
        while active(g) && sync_status().get_untracked().cancellable {
            day_core::sleep(1000).await;
            if !active(g) || !sync_status().get_untracked().cancellable {
                break;
            }
            sync_status().update(|s| {
                s.elapsed_seconds = started.elapsed().as_secs();
                s.remaining_seconds = ticker_deadline
                    .get()
                    .and_then(|at| at.checked_duration_since(Instant::now()))
                    .map(|d| d.as_secs().max(1));
            });
        }
    });
    let days = SESSION.with(|s| s.retention_days.get());
    let since = history_since(days, daynews_time::now_unix());
    log::info!(
        "sync history retention_days={days} since={since:?}; unread recovery is age-independent"
    );
    day_core::task(async move {
        let phase = Cell::new(Phase::Preparing);
        let phase_started = Cell::new(started);
        let operation = async {
            let worker = ready_worker().await.map_err(|_| Error::Storage)?;
            let store = WorkerStorage {
                worker: worker.clone(),
                account,
                generation: g,
                cancelled,
            };
            let report = engine::synchronize_with_history(provider.as_ref(), &store, since, |p| {
                if !active(g) { return Err(Error::Cancelled); }
                if phase.replace(p.phase) != p.phase { phase_started.set(Instant::now()); }
                let remaining = p.remaining(phase_started.get().elapsed());
                estimate_deadline.set(remaining.map(|d| Instant::now() + d));
                // Publication titles are untrusted: escape newlines/control characters so each
                // event remains one log line. Never log credentials, URLs, or article bodies.
                log::info!("sync phase={:?} completed={} total={:?} articles={} feeds={} feed={:?} elapsed_ms={} remaining_seconds={:?}",
                    p.phase, p.completed, p.total, p.articles, p.subscriptions, p.feed,
                    started.elapsed().as_millis(), remaining.map(|d| d.as_secs()));
                sync_status().update(|s| {
                    s.progress = Some(p);
                    s.elapsed_seconds = started.elapsed().as_secs();
                    s.remaining_seconds = remaining.map(|d| d.as_secs().max(1));
                });
                Ok(())
            }).await?;
            let (pending, last) = worker
                .read(move |c| {
                    let db = Db {
                        container: c.clone(),
                    };
                    Ok((
                        db.sync_pending_count(account),
                        c.get::<daynews_db::sync::Account>(account)
                            .and_then(|a| a.last_sync().peek()),
                    ))
                })
                .await
                .map_err(|_| Error::Storage)?;
            if active(g) {
                sync_status().update(|s| {
                    s.pending = pending;
                    s.last_sync = last;
                });
            }
            log::info!(
                "sync provider={} downloaded={} imported={} acknowledged={} pending={} elapsed_ms={}",
                provider.kind(),
                report.entries,
                report.imported,
                report.acknowledged,
                pending,
                started.elapsed().as_millis()
            );
            report.pending_error.map_or(Ok(()), Err)
        };
        let result = Abortable::new(operation, registration)
            .await
            .unwrap_or(Err(Error::Cancelled));
        if !active(g) {
            return;
        }
        SESSION.with(|s| {
            s.run.borrow_mut().take();
            let delay = match &result {
                Ok(()) => {
                    s.failures.set(0);
                    if sync_status().get_untracked().pending > 0 {
                        2
                    } else {
                        60
                    }
                }
                Err(Error::Retry { after_seconds }) => {
                    (*after_seconds).clamp(1, i64::MAX as u64 / 2) as i64
                }
                Err(Error::Authentication) => 3600,
                Err(_) => {
                    let n = (s.failures.get() + 1).min(7);
                    s.failures.set(n);
                    (5_i64 << n).min(600)
                }
            };
            s.next_attempt.set(daynews_time::now_unix() + delay);
        });
        sync_status().update(|s| {
            s.busy = false;
            s.cancellable = false;
            s.remaining_seconds = None;
            s.elapsed_seconds = started.elapsed().as_secs();
            s.error = result.err();
        });
    });
}
pub fn sync_reauthenticate() {
    change_session();
    SESSION.with(|s| {
        s.provider.borrow_mut().take();
        s.candidate.borrow_mut().take();
    });
    sync_status().update(|s| {
        s.connected = false;
        s.busy = false;
        s.preview = false;
    });
}
pub fn sync_active_account() -> Option<u64> {
    SESSION.with(|s| s.account.get())
}

struct WorkerStorage {
    worker: DatabaseWorker,
    account: u64,
    generation: u64,
    cancelled: Arc<AtomicBool>,
}
impl WorkerStorage {
    async fn write<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Db, u64) -> std::result::Result<T, DbError> + Send + 'static,
    ) -> Result<T> {
        if !active(self.generation) {
            return Err(Error::Cancelled);
        }
        let account = self.account;
        let cancelled = self.cancelled.clone();
        self.worker
            .write(move |c| {
                if cancelled.load(Ordering::SeqCst) {
                    return Err(DbError::driver("sync cancelled"));
                }
                day_model::with_author("sync", || {
                    f(
                        &Db {
                            container: c.clone(),
                        },
                        account,
                    )
                })
            })
            .await
            .map_err(|_| Error::Storage)
    }
}
impl Storage for WorkerStorage {
    fn checkpoint(&self) -> Call<'_, Option<String>> {
        async {
            self.write(|db, a| {
                Ok(db
                    .container
                    .try_get::<daynews_db::sync::Account>(a)?
                    .and_then(|a| a.checkpoint().peek()))
            })
            .await
        }
        .boxed_local()
    }

    fn begin_entries(&self) -> Call<'_, bool> {
        async { self.write(|db, a| db.sync_begin_entries(a)).await }.boxed_local()
    }
    fn pending(&self) -> Call<'_, Vec<Operation>> {
        async { self.write(|db, a| db.sync_pending(a)).await }.boxed_local()
    }
    fn outcomes(&self, o: Vec<Outcome>) -> Call<'_, ()> {
        async move { self.write(move |db, a| db.sync_outcomes(a, o)).await }.boxed_local()
    }
    fn subscriptions(&self, c: Changes<Subscription>) -> Call<'_, ()> {
        async move {
            self.write(move |db, a| db.sync_apply_subscriptions(a, c))
                .await
        }
        .boxed_local()
    }
    fn taggings(&self, t: Vec<Tagging>) -> Call<'_, ()> {
        async move { self.write(move |db, a| db.sync_apply_taggings(a, t)).await }.boxed_local()
    }
    fn missing(&self, ids: Vec<String>) -> Call<'_, Vec<String>> {
        async move { self.write(move |db, a| Ok(db.sync_missing(a, ids))).await }.boxed_local()
    }
    fn entries(&self, e: Vec<Entry>, s: ReadChanges) -> Call<'_, daynews_sync::engine::Imported> {
        async move {
            self.write(move |db, a| db.sync_import_entries(a, e, &s))
                .await
        }
        .boxed_local()
    }
    fn reading(&self, s: ReadChanges) -> Call<'_, ()> {
        async move { self.write(move |db, a| db.sync_apply_read(a, s)).await }.boxed_local()
    }
    fn finished(&self, checkpoint: Option<String>) -> Call<'_, ()> {
        async move {
            self.write(move |db, a| {
                if let Some(a) = db.container.get::<daynews_db::sync::Account>(a) {
                    a.last_sync().write(Some(daynews_time::now_unix()));
                    a.checkpoint().write(checkpoint);
                    a.importing().write(false);
                }
                Ok(())
            })
            .await
        }
        .boxed_local()
    }
}

#[cfg(test)]
mod retention_tests {
    use super::history_since;

    #[test]
    fn cutoff_tracks_retention_and_remains_stable_during_the_day() {
        let midnight = 1_792_368_000; // Synthetic 2026-10-19 UTC.
        assert_eq!(history_since(90, midnight + 43_200), Some(1_784_592_000));
        assert_eq!(
            history_since(90, midnight),
            history_since(90, midnight + 86_399)
        );
        assert_eq!(history_since(30, midnight), Some(midnight - 30 * 86_400));
        assert_eq!(history_since(0, midnight), None);
    }
}

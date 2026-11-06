// SPDX-License-Identifier: Apache-2.0
//! Experimental DAV session. Configuration and device identity are public; the WebDAV password stays in native credentials.
use super::*;
use daynews_db::webdav::{
    self as adapter,
    day_sync::{Identity, Replica, Space},
    day_sync_webdav::{
        WebDavStore,
        webdav::{Authentication, Client, Endpoint, http},
    },
};
use serde::{Deserialize, Serialize};
use std::{cell::Cell, sync::Arc};
use zeroize::Zeroizing;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Disabled,
    Working,
    Ready,
    Restart,
    Failed,
    Attention,
    ReadOnly,
    Paused,
    DisconnectRestart,
}
#[derive(Clone, Default)]
pub struct Status {
    pub phase: Phase,
    pub endpoint: String,
    pub username: String,
    pub pending: u64,
    pub last_sync: Option<i64>,
    pub space_id: String,
    pub reqwest: bool,
}
#[derive(Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(default)]
    identity: Option<Identity>,
    endpoint: String,
    username: String,
    reqwest: bool,
    seed: bool,
    // Persist the credential account separately from the movable iOS container path.
    #[serde(default)]
    credential_key: Option<String>,
    #[serde(default)]
    paused: bool,
    #[serde(default)]
    disconnect_backup: Option<String>,
}
impl Config {
    fn key(&self, directory: &std::path::Path) -> String {
        self.credential_key
            .clone()
            .unwrap_or_else(|| format!("webdav:{}", directory.display()))
    }
}
// Deliberately no Debug. This serialization is confined to the native secure store.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Secret {
    password: Zeroizing<String>,
}
thread_local! {
 static STATUS:Signal<Status>=Signal::new_in(day_reactive::Scope::root(),Status::default());
 static REPLICA:RefCell<Option<Replica>>=const{RefCell::new(None)};
 static CLIENT:RefCell<Option<WebDavStore>>=const{RefCell::new(None)};
 static INSTALLED:Cell<bool>=const{Cell::new(false)};
 static NEXT:Cell<i64>=const{Cell::new(0)};
 static FAILURES:Cell<u32>=const{Cell::new(0)};
 static IN_FLIGHT:RefCell<Option<futures_util::future::WeakShared<LocalBoxFuture<'static, bool>>>>=const{RefCell::new(None)};
 static REQUEST:Cell<u64>=const{Cell::new(0)};
 static BACKOFF:Cell<i64>=const{Cell::new(0)};
}
pub fn status() -> Signal<Status> {
    STATUS.with(|s| *s)
}
fn config_path() -> PathBuf {
    store_dir().join("experimental-webdav.json")
}
fn key() -> Result<String, DbError> {
    let config: Config =
        serde_json::from_slice(&std::fs::read(config_path()).map_err(|_| failure())?)
            .map_err(|_| failure())?;
    Ok(config.key(&store_dir()))
}
fn failure() -> DbError {
    DbError::driver("news.webdav.setup_or_sync_failed")
}
fn endpoint(url: &str) -> Result<Endpoint, DbError> {
    Endpoint::new(url)
        .or_else(|_| Endpoint::loopback(url))
        .map_err(|_| failure())
}
fn make_client(
    url: &str,
    username: String,
    password: String,
    reqwest: bool,
) -> Result<WebDavStore, DbError> {
    let session = http::Session::new();
    session.set_provider(if reqwest {
        http::Provider::Reqwest
    } else {
        http::Provider::Native
    });
    Ok(WebDavStore::new(
        Client::builder(endpoint(url)?)
            .session(session)
            .authentication(Authentication::Basic { username, password })
            .build(),
    ))
}
async fn secret() -> Result<Secret, DbError> {
    let key = key()?;
    #[cfg(any(target_os = "android", target_env = "ohos"))]
    let result = daynews_credentials::load(&key).await;
    #[cfg(not(any(target_os = "android", target_env = "ohos")))]
    let result = day_async::spawn_blocking(move || daynews_credentials::load(&key))
        .await
        .map_err(|_| failure())?;
    let encoded = Zeroizing::new(result.map_err(|_| failure())?.ok_or_else(failure)?);
    serde_json::from_str(&encoded).map_err(|_| failure())
}
async fn save_secret(secret: Secret) -> Result<(), DbError> {
    save_secret_at(secret, key()?).await
}
async fn save_secret_at(secret: Secret, key: String) -> Result<(), DbError> {
    let value = Zeroizing::new(serde_json::to_string(&secret).map_err(|_| failure())?);
    #[cfg(any(target_os = "android", target_env = "ohos"))]
    let result = daynews_credentials::save(&key, &value);
    #[cfg(not(any(target_os = "android", target_env = "ohos")))]
    let result = day_async::spawn_blocking(move || daynews_credentials::save(&key, &value))
        .await
        .map_err(|_| failure())?;
    result.map_err(|_| failure())
}
fn save_config(c: &Config) -> Result<(), DbError> {
    let path = config_path();
    std::fs::create_dir_all(store_dir()).map_err(|_| failure())?;
    let tmp = path.with_extension("pending");
    std::fs::write(&tmp, serde_json::to_vec(c).map_err(|_| failure())?).map_err(|_| failure())?;
    std::fs::File::open(&tmp)
        .and_then(|f| f.sync_all())
        .map_err(|_| failure())?;
    std::fs::rename(tmp, &path).map_err(|_| failure())?;
    sync_directory(&store_dir())
}
pub(super) async fn open_worker(path: PathBuf) -> Result<DatabaseWorker, DbError> {
    let configured = config_path().exists();
    let result = open_worker_inner(path.clone()).await;
    if configured && result.is_err() {
        status().update(|s| s.phase = Phase::ReadOnly);
    }
    if configured && result.is_err() && path.is_file() {
        return DatabaseWorker::open(move || {
            let db = Db::open(&path)?;
            db.container
                .install_transaction_participant(adapter::RecoveryReadOnly)?;
            Ok(db.container)
        })
        .await;
    }
    result
}
async fn open_worker_inner(path: PathBuf) -> Result<DatabaseWorker, DbError> {
    if !config_path().exists() {
        let worker = DatabaseWorker::open(move || {
            let db = Db::open(&path)?;
            if adapter::is_installed(&db.container) {
                db.container
                    .install_transaction_participant(adapter::RecoveryReadOnly)?;
            }
            Ok(db.container)
        })
        .await?;
        if worker.read(|db| Ok(adapter::is_installed(db))).await? {
            status().update(|s| s.phase = Phase::ReadOnly);
        }
        return Ok(worker);
    }
    let mut config: Config =
        serde_json::from_slice(&std::fs::read(config_path()).map_err(|_| failure())?)
            .map_err(|_| failure())?;
    if let Some(name) = &config.disconnect_backup {
        // Filename is generated locally; never let a modified configuration select another path.
        if !name.starts_with("webdav-disconnect-")
            || !name.ends_with(".db")
            || name.contains(['/', '\\'])
        {
            return Err(failure());
        }
        let backup = store_dir().join(name);
        let backup_source = path.clone();
        let worker = DatabaseWorker::open(move || {
            let db = Db::open(&backup_source)?;
            if !backup.exists() {
                let pending = backup.with_extension("pending");
                if pending.exists() {
                    std::fs::remove_file(&pending).map_err(|_| failure())?;
                }
                db.container.backup_to(&pending)?;
                std::fs::File::open(&pending)
                    .and_then(|f| f.sync_all())
                    .map_err(|_| failure())?;
                std::fs::rename(&pending, &backup).map_err(|_| failure())?;
                sync_directory(backup.parent().ok_or_else(failure)?)?;
            }
            Ok(db.container)
        })
        .await?;
        worker.close().await?;
        let account = config.key(&store_dir());
        if !account.is_empty() {
            remove_secret(account).await?;
        }
        let worker = DatabaseWorker::open(move || {
            let db = Db::open(&path)?;
            adapter::detach(&db.container)?;
            Ok(db.container)
        })
        .await?;
        if std::fs::remove_file(config_path()).is_err() {
            worker.close().await?;
            return Err(failure());
        }
        status().set(Status::default());
        return Ok(worker);
    }
    // Public account details must remain available when a credential is missing or
    // expired, so the recovery UI can repair the existing enrollment.
    status().update(|s| {
        s.endpoint = config.endpoint.clone();
        s.username = config.username.clone();
        s.reqwest = config.reqwest;
        s.space_id = config
            .identity
            .as_ref()
            .map(|i| i.space().to_owned())
            .unwrap_or_default();
    });
    let secret = secret().await?;
    // Preserve the existing account name before any future container relocation. An already
    // moved legacy container whose secret cannot be found still fails explicitly.
    if config.credential_key.is_none() {
        config.credential_key = Some(config.key(&store_dir()));
        save_config(&config)?;
    }
    let identity = Arc::new(config.identity.clone().ok_or_else(failure)?);
    identity.validate().map_err(|_| failure())?;
    let client = make_client(
        &config.endpoint,
        config.username,
        secret.password.to_string(),
        config.reqwest,
    )?;
    let replica = adapter::open(&path, identity.clone(), Arc::new(|_| Ok(())))
        .await
        .map_err(|_| failure())?;
    if config.seed {
        adapter::seed(&replica).await.map_err(|_| failure())?;
    }
    let worker = replica.worker();
    REPLICA.with(|r| *r.borrow_mut() = Some(replica));
    CLIENT.with(|r| *r.borrow_mut() = Some(client));
    status().update(|s| {
        s.phase = if config.paused {
            Phase::Paused
        } else {
            Phase::Ready
        };
        s.space_id = identity.space().to_owned();
    });
    Ok(worker)
}
/// Setup changes require restart so the participant is installed before exposing any writers.
/// Existing configurations cannot be overwritten by this action.
pub fn update_credentials(username: String, password: String, reqwest: bool) {
    let password = Zeroizing::new(password);
    if matches!(
        status().get_untracked().phase,
        Phase::Working | Phase::DisconnectRestart
    ) {
        return;
    }
    status().update(|s| s.phase = Phase::Working);
    day_core::task(async move {
        let result = async {
            let mut config: Config =
                serde_json::from_slice(&std::fs::read(config_path()).map_err(|_| failure())?)
                    .map_err(|_| failure())?;
            let client = make_client(
                &config.endpoint,
                username.clone(),
                password.to_string(),
                reqwest,
            )?;
            client
                .client()
                .propfind(
                    &adapter::day_sync_webdav::webdav::ResourcePath::root(),
                    adapter::day_sync_webdav::webdav::Depth::Zero,
                )
                .await
                .map_err(|_| failure())?;
            config.username = username;
            config.reqwest = reqwest;
            save_secret(Secret { password }).await?;
            save_config(&config)
        }
        .await;
        status().update(|s| {
            s.phase = if result.is_ok() {
                Phase::Restart
            } else {
                Phase::Failed
            }
        });
    });
}
pub fn prepare(
    url: String,
    username: String,
    password: String,
    reqwest: bool,
    space_id: Option<String>,
) {
    let password = Zeroizing::new(password);
    if status().get_untracked().phase == Phase::Working {
        return;
    }
    status().update(|s| s.phase = Phase::Working);
    day_core::task(async move {
        let result = async {
            if config_path().exists() {
                return Err(failure());
            }
            let client = make_client(&url, username.clone(), password.to_string(), reqwest)?;
            client
                .client()
                .propfind(
                    &adapter::day_sync_webdav::webdav::ResourcePath::root(),
                    adapter::day_sync_webdav::webdav::Depth::Zero,
                )
                .await
                .map_err(|_| failure())?;
            let seed = space_id.is_none();
            let identity = match space_id {
                Some(id) => {
                    let space = Space::fetch(&client, id.trim())
                        .await
                        .map_err(|_| failure())?;
                    if space.profile != adapter::PROFILE {
                        return Err(failure());
                    }
                    Identity::join(space).map_err(|_| failure())?
                }
                None => Identity::create_space_for(adapter::PROFILE).map_err(|_| failure())?,
            };
            let account = format!("webdav:device:{}", identity.actor());
            let config = Config {
                identity: Some(identity.clone()),
                endpoint: url,
                username,
                reqwest,
                seed,
                credential_key: Some(account.clone()),
                paused: false,
                disconnect_backup: None,
            };
            save_secret_at(Secret { password }, account).await?;
            save_config(&config)?;
            status().update(|s| s.space_id = identity.space().to_owned());
            Ok::<_, DbError>(())
        }
        .await;
        status().update(|s| {
            s.phase = if result.is_ok() {
                Phase::Restart
            } else {
                Phase::Failed
            }
        });
    });
}
pub fn request_soon() {
    REQUEST.with(|n| n.set(n.get().wrapping_add(1)));
    NEXT.with(|n| n.set((daynews_time::now_unix() + 3).max(BACKOFF.with(Cell::get))));
}
pub fn sync_now() {
    let _ = start_sync(true);
}

/// Join the current pass instead of opening a competing session. The detached task owns
/// another clone, so timing out a refresh does not cancel in-flight sync or its follow-up.
pub(super) async fn before_refresh(manual: bool) -> bool {
    let complete = match start_sync(manual) {
        Some(pass) => wait_for_pass(pass, day_core::sleep(10_000)).await,
        None => status().get_untracked().phase == Phase::Disabled,
    };
    refresh::set_sync_pending(!complete);
    complete
}

async fn wait_for_pass(
    pass: impl std::future::Future<Output = bool>,
    timeout: impl std::future::Future<Output = ()>,
) -> bool {
    futures_util::pin_mut!(pass, timeout);
    match futures_util::future::select(pass, timeout).await {
        futures_util::future::Either::Left((complete, _)) => complete,
        futures_util::future::Either::Right(_) => false,
    }
}

fn start_sync(manual: bool) -> Option<Shared<LocalBoxFuture<'static, bool>>> {
    if let Some(pass) =
        IN_FLIGHT.with(|slot| slot.borrow().as_ref().and_then(|pass| pass.upgrade()))
    {
        return Some(pass);
    }
    if !manual && BACKOFF.with(|n| n.get() > daynews_time::now_unix()) {
        return None;
    }
    if matches!(
        status().get_untracked().phase,
        Phase::Working
            | Phase::Restart
            | Phase::Paused
            | Phase::ReadOnly
            | Phase::DisconnectRestart
    ) {
        return None;
    }
    let r = REPLICA.with(|r| r.borrow().clone());
    let c = CLIENT.with(|r| r.borrow().clone());
    let (Some(r), Some(c)) = (r, c) else {
        return None;
    };
    status().update(|s| s.phase = Phase::Working);
    let generation = REQUEST.with(Cell::get);
    let pass = async move {
        let before = refresh::active_feeds(&r.worker()).await;
        let result = r.sync(&c).await;
        let after = refresh::active_feeds(&r.worker()).await;
        let deferred = result.as_ref().is_ok_and(|report| {
            report.downloads_deferred || report.uploads_deferred || report.pending_deletions > 0
        });
        let complete = result.as_ref().is_ok_and(|report| {
            !report.downloads_deferred
                && !report.uploads_deferred
                && report.waiting == 0
                && report.held == 0
        });
        let failures = FAILURES.with(|n| {
            let count = if result.is_ok() {
                0
            } else {
                n.get().saturating_add(1)
            };
            n.set(count);
            count
        });
        let delay = retry_delay(result.as_ref().err(), failures);
        let snapshot = r.snapshot().await;
        status().update(|s| {
            if let Ok(v) = snapshot {
                s.pending = v.pending_uploads;
            }
            s.phase = if result.is_ok() {
                s.last_sync = Some(daynews_time::now_unix());
                Phase::Ready
            } else if delay.is_none() {
                Phase::Attention
            } else {
                Phase::Failed
            };
        });
        let delay = follow_up_delay(delay, result.is_ok(), generation, deferred);
        let next = delay
            .map(|seconds| daynews_time::now_unix().saturating_add(seconds))
            .unwrap_or(i64::MAX);
        BACKOFF.with(|n| n.set(if result.is_ok() { 0 } else { next }));
        NEXT.with(|n| n.set(next));
        IN_FLIGHT.with(|slot| slot.borrow_mut().take());
        refresh::set_sync_pending(!complete);
        if let (Ok(before), Ok(after)) = (before, after) {
            let existing: HashSet<_> = before.into_iter().map(|(id, _)| id).collect();
            refresh::received_subscriptions(
                after
                    .into_iter()
                    .map(|(id, _)| id)
                    .filter(|id| !existing.contains(id)),
            );
        }
        complete
    }
    .boxed_local()
    .shared();
    IN_FLIGHT.with(|slot| *slot.borrow_mut() = pass.downgrade());
    let driver = pass.clone();
    day_core::task(async move {
        driver.await;
    });
    Some(pass)
}
fn follow_up_delay(
    delay: Option<i64>,
    success: bool,
    generation: u64,
    deferred: bool,
) -> Option<i64> {
    if success && (deferred || REQUEST.with(Cell::get) != generation) {
        Some(3)
    } else {
        delay
    }
}

pub(super) fn install() {
    if INSTALLED.with(|i| i.replace(true)) {
        return;
    }
    day_core::task(async {
        let _ = ready_worker().await;
        loop {
            if NEXT.with(|n| n.get() <= daynews_time::now_unix()) {
                let _ = start_sync(false);
            }
            day_core::sleep(5_000).await;
        }
    });
}

// Retry-After is a lower bound. Permanent/security failures require explicit user retry;
// request_soon cannot defeat server backoff. A manual Sync Now remains available.
fn retry_delay(error: Option<&adapter::day_sync::Error>, failures: u32) -> Option<i64> {
    use adapter::day_sync::{Error as E, StoreErrorKind as K};
    let Some(error) = error else { return Some(60) };
    let backoff = (15_i64.saturating_mul(1_i64 << failures.min(6))).min(900);
    match error {
        E::WrongSpace
        | E::UnsupportedFormat
        | E::Retired
        | E::Fork
        | E::WrongEndpoint
        | E::RemoteHistoryMissing
        | E::InvalidWire
        | E::Limit => None,
        E::Database(e)
            if !matches!(
                e.kind,
                day_persistence::DbErrorKind::Busy | day_persistence::DbErrorKind::Cancelled
            ) =>
        {
            None
        }
        E::Store(s) => match s.kind {
            K::Unauthorized | K::InsufficientStorage | K::Limit | K::Invalid => None,
            K::Forbidden if failures >= 3 => None,
            _ => Some(s.retry_after.map_or(backoff, |wait| {
                backoff.max(wait.as_secs().min(i64::MAX as u64) as i64)
            })),
        },
        E::Integrity | E::RemoteObjectsRejected { .. } if failures >= 3 => None,
        _ => Some(backoff),
    }
}

async fn remove_secret(account: String) -> Result<(), DbError> {
    #[cfg(any(target_os = "android", target_env = "ohos"))]
    let result = daynews_credentials::remove(&account);
    #[cfg(not(any(target_os = "android", target_env = "ohos")))]
    let result = day_async::spawn_blocking(move || daynews_credentials::remove(&account))
        .await
        .map_err(|_| failure())?;
    result.map_err(|_| failure())
}

pub fn pause(paused: bool) {
    if matches!(
        status().get_untracked().phase,
        Phase::Working | Phase::ReadOnly | Phase::DisconnectRestart
    ) {
        return;
    }
    let result = (|| {
        let mut config: Config =
            serde_json::from_slice(&std::fs::read(config_path()).map_err(|_| failure())?)
                .map_err(|_| failure())?;
        config.paused = paused;
        save_config(&config)
    })();
    status().update(|s| {
        s.phase = if result.is_err() {
            Phase::Attention
        } else if paused {
            Phase::Paused
        } else {
            Phase::Ready
        }
    });
    if !paused {
        NEXT.with(|n| n.set(0));
    }
}

/// Persist intent first. Startup completes backup, key removal and transactional detach in
/// that order; a crash retries the same backup name. Local data and archived unsent records stay.
pub fn disconnect() {
    if matches!(
        status().get_untracked().phase,
        Phase::Working | Phase::DisconnectRestart
    ) {
        return;
    }
    let result = (|| {
        let bytes = std::fs::read(config_path()).ok();
        let mut config = disconnect_config(bytes.as_deref());
        if config.disconnect_backup.is_none() {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| failure())?
                .as_nanos();
            config.disconnect_backup = Some(format!("webdav-disconnect-{stamp}.db"));
        }
        save_config(&config)
    })();
    status().update(|s| {
        s.phase = if result.is_ok() {
            Phase::DisconnectRestart
        } else {
            Phase::Attention
        }
    });
}

// A missing or corrupt public configuration must not trap a readable library forever.
// An unknown credential account is deliberately left alone; never guess which key to remove.
fn disconnect_config(bytes: Option<&[u8]>) -> Config {
    bytes
        .and_then(|b| serde_json::from_slice(b).ok())
        .unwrap_or(Config {
            identity: None,
            endpoint: String::new(),
            username: String::new(),
            reqwest: false,
            seed: false,
            credential_key: Some(String::new()),
            paused: true,
            disconnect_backup: None,
        })
}

fn sync_directory(path: &std::path::Path) -> Result<(), DbError> {
    #[cfg(unix)]
    std::fs::File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|_| failure())?;
    #[cfg(not(unix))]
    let _ = path; // Windows has no portable directory fsync through std::fs.
    Ok(())
}

#[cfg(test)]
mod retry_tests {
    use super::*;
    #[test]
    fn paused_sync_is_visible_until_refresh_can_proceed_without_pending_state() {
        IN_FLIGHT.with(|slot| slot.borrow_mut().take());
        status().update(|s| s.phase = Phase::Paused);
        crate::native::tests::wait(async {
            assert!(!before_refresh(true).await);
            assert!(refresh::refresh_sync_pending());
            status().update(|s| s.phase = Phase::Disabled);
            assert!(before_refresh(false).await);
            assert!(!refresh::refresh_sync_pending());
        });
    }

    #[test]
    fn refresh_and_manual_requests_join_one_existing_pass() {
        let executions = Rc::new(Cell::new(0));
        let count = executions.clone();
        let driver = async move {
            count.set(count.get() + 1);
            true
        }
        .boxed_local()
        .shared();
        IN_FLIGHT.with(|slot| *slot.borrow_mut() = driver.downgrade());
        // Joining must also work while the phase is Working and retry backoff is set.
        status().update(|s| s.phase = Phase::Working);
        BACKOFF.with(|n| n.set(i64::MAX));
        let manual = start_sync(true).unwrap();
        let automatic = start_sync(false).unwrap();
        crate::native::tests::wait(async {
            assert!(manual.await);
            assert!(automatic.await);
            assert!(driver.await);
        });
        assert_eq!(executions.get(), 1);
        IN_FLIGHT.with(|slot| slot.borrow_mut().take());
        BACKOFF.with(|n| n.set(0));
    }

    #[test]
    fn timeout_releases_refresh_without_cancelling_sync_and_late_completion_is_shared() {
        use std::future::{pending, poll_fn, ready};
        use std::task::Poll;
        let finish = Rc::new(Cell::new(false));
        let gate = finish.clone();
        let driver = poll_fn(move |_| {
            if gate.get() {
                Poll::Ready(true)
            } else {
                Poll::Pending
            }
        })
        .boxed_local()
        .shared();
        crate::native::tests::wait(async {
            assert!(!wait_for_pass(driver.clone(), ready(())).await);
            assert!(driver.peek().is_none());
            finish.set(true);
            assert!(wait_for_pass(driver.clone(), pending()).await);
            assert!(driver.await);
            // Failed sync returns immediately, without waiting for the time budget.
            assert!(!wait_for_pass(ready(false), pending()).await);
        });
    }

    #[test]
    fn request_during_sync_is_preserved_without_overriding_failure_backoff() {
        REQUEST.with(|n| n.set(7));
        let generation = REQUEST.with(Cell::get);
        BACKOFF.with(|n| n.set(i64::MAX));
        request_soon();
        assert_ne!(REQUEST.with(Cell::get), generation);
        assert_eq!(NEXT.with(Cell::get), i64::MAX);
        assert_eq!(follow_up_delay(Some(60), true, generation, false), Some(3));
        assert_eq!(
            follow_up_delay(Some(120), false, generation, false),
            Some(120)
        );
        assert_eq!(follow_up_delay(None, false, generation, false), None);
        let current = REQUEST.with(Cell::get);
        assert_eq!(follow_up_delay(Some(60), true, current, false), Some(60));
        assert_eq!(follow_up_delay(Some(60), true, current, true), Some(3));
        BACKOFF.with(|n| n.set(0));
    }

    #[test]
    fn public_identity_is_configuration_and_secure_entry_is_only_server_password() {
        let id = Identity::create_space_for(adapter::PROFILE).unwrap();
        let config = Config {
            identity: Some(id.clone()),
            endpoint: "https://fixture.invalid/".into(),
            username: "fixture".into(),
            reqwest: false,
            seed: true,
            credential_key: Some("fixture-account".into()),
            paused: false,
            disconnect_backup: None,
        };
        let encoded = serde_json::to_vec(&config).unwrap();
        let restored: Config = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(restored.identity, Some(id));
        let secret = Secret {
            password: Zeroizing::new("synthetic-server-password".into()),
        };
        let encoded_secret = serde_json::to_value(&secret).unwrap();
        assert_eq!(encoded_secret.as_object().unwrap().len(), 1);
        assert!(
            serde_json::from_str::<Secret>(
                r#"{"password":"synthetic","identity":"old-private-key","pending":null}"#
            )
            .is_err()
        );
        assert!(
            !String::from_utf8(encoded)
                .unwrap()
                .contains("synthetic-server-password")
        );
    }
    #[test]
    fn orphaned_configuration_can_detach_without_guessing_a_credential() {
        for bytes in [None, Some(b"invalid configuration".as_slice())] {
            let config = disconnect_config(bytes);
            assert!(config.key(std::path::Path::new("synthetic")).is_empty());
            assert!(config.paused);
        }
        let bytes = br#"{"endpoint":"https://fixture.invalid/","username":"synthetic","reqwest":false,"seed":false,"credential_key":"known-account"}"#;
        assert_eq!(
            disconnect_config(Some(bytes)).credential_key.as_deref(),
            Some("known-account")
        );
    }
    #[test]
    fn credential_account_survives_serialized_container_relocation() {
        let mut config: Config = serde_json::from_str(r#"{"endpoint":"https://fixture.invalid/dav/","username":"synthetic","reqwest":false,"seed":false}"#).unwrap();
        let old = std::path::Path::new("/container/old-id/Library/Application Support/demo");
        let new = std::path::Path::new("/container/new-id/Library/Application Support/demo");
        let legacy_account = config.key(old);
        config.credential_key = Some(legacy_account.clone());
        let restored: Config =
            serde_json::from_slice(&serde_json::to_vec(&config).unwrap()).unwrap();
        assert_eq!(restored.key(new), legacy_account);
        config.credential_key = Some("webdav:device:synthetic-independent-id".into());
        let restored: Config =
            serde_json::from_slice(&serde_json::to_vec(&config).unwrap()).unwrap();
        assert_eq!(restored.key(old), restored.key(new));
        assert_ne!(restored.key(new), legacy_account);
    }
    #[test]
    fn backoff_respects_server_and_pauses_credentials_or_history_loss() {
        use adapter::day_sync::{Error as E, StoreError, StoreErrorKind as K};
        let throttle = E::Store(
            StoreError::new(K::Unavailable).retry_after(Some(std::time::Duration::from_secs(120))),
        );
        assert_eq!(retry_delay(Some(&throttle), 1), Some(120));
        assert_eq!(retry_delay(Some(&E::Busy), 100), Some(900));
        assert_eq!(retry_delay(Some(&E::RemoteHistoryMissing), 1), None);
        assert_eq!(
            retry_delay(Some(&E::Store(StoreError::new(K::Unauthorized))), 1),
            None
        );
        assert_eq!(
            retry_delay(
                Some(&E::Database(DbError::driver("sync.retention_limit"))),
                1
            ),
            None
        );
        assert_eq!(retry_delay(None, 0), Some(60));
    }
}

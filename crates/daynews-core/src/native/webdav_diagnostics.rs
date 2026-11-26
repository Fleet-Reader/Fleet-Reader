// SPDX-License-Identifier: GPL-2.0-or-later
//! Bounded developer diagnostics, never UI text. No URLs, credentials, object names,
//! article content, SQL or platform error strings are serialized.
use daynews_db::webdav::{day_sync as sync, day_sync_webdav::webdav as dav};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    io::{Read, Write},
    path::PathBuf,
    sync::Mutex,
    time::Instant,
};
use sync::{Created, Store, StoreError, StoreFuture};

const ATTEMPTS: usize = 16;
const OPERATIONS: usize = 16;
const MAX_BYTES: u64 = 512 * 1024;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Failure {
    code: String,
    database_kind: Option<String>,
    store: Option<String>,
    dav: Option<String>,
    http_status: Option<u16>,
    transport: Option<String>,
    write_outcome: Option<String>,
    retry_after_seconds: Option<u64>,
}
impl Failure {
    fn sync(error: &sync::Error) -> Self {
        if let sync::Error::Store(error) = error {
            Self::store(error)
        } else {
            Self {
                code: error.to_string(),
                database_kind: match error {
                    sync::Error::Database(error) => Some(format!("{:?}", error.kind)),
                    _ => None,
                },
                ..Self::default()
            }
        }
    }
    fn store(error: &StoreError) -> Self {
        use std::error::Error as _;
        let mut failure = Self {
            code: "sync.store".into(),
            store: Some(error.to_string()),
            retry_after_seconds: error.retry_after.map(|d| d.as_secs()),
            ..Self::default()
        };
        if let Some(error) = error.source().and_then(|s| s.downcast_ref::<dav::Error>()) {
            failure.dav = Some(error.to_string());
            match error {
                dav::Error::Status {
                    status, outcome, ..
                }
                | dav::Error::Redirect { status, outcome } => {
                    failure.http_status = Some(*status);
                    failure.write_outcome = Some(format!("{outcome:?}"));
                }
                dav::Error::Transport { source, outcome } => {
                    use dav::http::HttpError as H;
                    failure.transport = Some(
                        match source {
                            H::BadUrl(_) => "bad_url",
                            H::Timeout => "timeout",
                            H::Dns => "dns",
                            H::Connect => "connect",
                            H::NetworkRestricted(dav::http::NetworkRestriction::Expensive) => {
                                "network.expensive"
                            }
                            H::NetworkRestricted(dav::http::NetworkRestriction::Constrained) => {
                                "network.constrained"
                            }
                            H::NetworkRestricted(dav::http::NetworkRestriction::Cellular) => {
                                "network.cellular"
                            }
                            H::Tls(_) => "tls",
                            H::Io(_) => "io",
                            H::Cancelled => "cancelled",
                            H::Unsupported => "unsupported",
                            H::TooManyRedirects => "too_many_redirects",
                            H::Status(status) => {
                                failure.http_status = Some(*status);
                                "status"
                            }
                            H::BodyTooLarge { .. } => "body_too_large",
                            _ => "other",
                        }
                        .into(),
                    );
                    failure.write_outcome = Some(format!("{outcome:?}"));
                }
                dav::Error::Deferred(state) => {
                    failure.transport = Some(format!("deferred_{state:?}"))
                }
                _ => {}
            }
        }
        failure
    }
}

pub(super) fn sync_detail(error: &sync::Error) -> String {
    let detail = Failure::sync(error);
    if let Some(status) = detail.http_status {
        return format!("{} · HTTP {status}", detail.code);
    }
    detail
        .transport
        .or(detail.dav)
        .or(detail.store)
        .unwrap_or(detail.code)
}

#[derive(Clone, Serialize, Deserialize)]
struct Operation {
    method: String,
    elapsed_ms: u64,
    error: Option<Failure>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct Trace {
    counts: BTreeMap<String, u64>,
    recent: VecDeque<Operation>,
}

/// Observes completed operations without changing ordering, results or cancellation.
pub(super) struct RecordingStore<'a> {
    inner: &'a dyn Store,
    trace: Mutex<Trace>,
}
fn milliseconds(start: Instant) -> u64 {
    start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}
impl<'a> RecordingStore<'a> {
    pub(super) fn new(inner: &'a dyn Store) -> Self {
        Self {
            inner,
            trace: Mutex::new(Trace::default()),
        }
    }
    async fn record<T>(&self, method: &str, work: StoreFuture<'_, T>) -> Result<T, StoreError> {
        let start = Instant::now();
        let result = work.await;
        let mut trace = self
            .trace
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *trace.counts.entry(method.into()).or_default() += 1;
        if trace.recent.len() == OPERATIONS {
            trace.recent.pop_front();
        }
        trace.recent.push_back(Operation {
            method: method.into(),
            elapsed_ms: milliseconds(start),
            error: result.as_ref().err().map(Failure::store),
        });
        result
    }
}
impl Store for RecordingStore<'_> {
    fn endpoint(&self) -> String {
        self.inner.endpoint()
    }
    fn ensure_directory<'a>(&'a self, d: &'a [String]) -> StoreFuture<'a, ()> {
        Box::pin(self.record("ensure_directory", self.inner.ensure_directory(d)))
    }
    fn list<'a>(&'a self, d: &'a [String]) -> StoreFuture<'a, Vec<String>> {
        Box::pin(self.record("list", self.inner.list(d)))
    }
    fn get<'a>(&'a self, d: &'a [String], n: &'a str) -> StoreFuture<'a, Option<Vec<u8>>> {
        Box::pin(self.record("get", self.inner.get(d, n)))
    }
    fn put<'a>(&'a self, d: &'a [String], n: &'a str, b: Vec<u8>) -> StoreFuture<'a, ()> {
        Box::pin(self.record("put", self.inner.put(d, n, b)))
    }
    fn create<'a>(&'a self, d: &'a [String], n: &'a str, b: Vec<u8>) -> StoreFuture<'a, Created> {
        Box::pin(self.record("create", self.inner.create(d, n, b)))
    }
    fn delete<'a>(&'a self, d: &'a [String], n: &'a str) -> StoreFuture<'a, bool> {
        Box::pin(self.record("delete", self.inner.delete(d, n)))
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Attempt {
    #[serde(default)]
    app_version: String,
    #[serde(default)]
    platform: String,
    started_at: i64,
    finished_at: Option<i64>,
    elapsed_ms: u64,
    manual: bool,
    provider: String,
    outcome: String,
    error: Option<Failure>,
    next_retry_at: Option<i64>,
    report: Option<serde_json::Value>,
    frontier: BTreeMap<String, u64>,
    pending_uploads: Option<u64>,
    trace: Trace,
}
#[derive(Default, Serialize, Deserialize)]
struct History {
    attempts: VecDeque<Attempt>,
    // Successful polling must not erase the last failure before someone can inspect it.
    last_failure: Option<Attempt>,
}
pub(super) struct Journal {
    path: PathBuf,
    history: History,
    start: Instant,
}
impl Journal {
    fn start(path: PathBuf, manual: bool, reqwest: bool) -> std::io::Result<Self> {
        let mut history: History = match std::fs::File::open(&path) {
            Ok(file) => {
                let mut bytes = Vec::new();
                file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
                if bytes.len() as u64 <= MAX_BYTES {
                    serde_json::from_slice(&bytes).unwrap_or_default()
                } else {
                    History::default()
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => History::default(),
            Err(e) => return Err(e),
        };
        for attempt in &mut history.attempts {
            if attempt.outcome == "running" {
                // Could be process death or an unwritten completion; do not invent an error.
                attempt.outcome = "completion_not_recorded".into();
            }
        }
        while history.attempts.len() >= ATTEMPTS {
            history.attempts.pop_front();
        }
        history.attempts.push_back(Attempt {
            app_version: env!("CARGO_PKG_VERSION").into(),
            platform: std::env::consts::OS.into(),
            started_at: daynews_time::now_unix(),
            finished_at: None,
            elapsed_ms: 0,
            manual,
            provider: if reqwest { "reqwest" } else { "native" }.into(),
            outcome: "running".into(),
            error: None,
            next_retry_at: None,
            report: None,
            frontier: BTreeMap::new(),
            pending_uploads: None,
            trace: Trace::default(),
        });
        let journal = Self {
            path,
            history,
            start: Instant::now(),
        };
        journal.save()?;
        Ok(journal)
    }
    fn save(&self) -> std::io::Result<()> {
        let bytes = serde_json::to_vec(&self.history)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(std::io::Error::other("sync diagnostic size limit"));
        }
        let temporary = self.path.with_extension("pending");
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(temporary, &self.path)
    }
    pub(super) async fn begin(path: PathBuf, manual: bool, reqwest: bool) -> Option<Self> {
        match day_async::spawn_blocking(move || Self::start(path, manual, reqwest)).await {
            Ok(Ok(journal)) => Some(journal),
            _ => {
                log::warn!("sync diagnostics could not record attempt start");
                None
            }
        }
    }
    pub(super) async fn finish(
        mut self,
        result: &Result<sync::SyncReport, sync::Error>,
        snapshot: Option<&sync::Snapshot>,
        next_retry_at: Option<i64>,
        store: &RecordingStore<'_>,
    ) {
        let attempt = self
            .history
            .attempts
            .back_mut()
            .expect("journal starts with an attempt");
        attempt.finished_at = Some(daynews_time::now_unix());
        attempt.elapsed_ms = milliseconds(self.start);
        attempt.outcome = if result.is_ok() {
            "succeeded"
        } else {
            "failed"
        }
        .into();
        attempt.error = result.as_ref().err().map(Failure::sync);
        attempt.next_retry_at = next_retry_at;
        attempt.trace = store
            .trace
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(snapshot) = snapshot {
            attempt.frontier.clone_from(&snapshot.frontier);
            attempt.pending_uploads = Some(snapshot.pending_uploads);
        }
        if let Ok(r) = result {
            attempt.report = Some(serde_json::json!({
                "uploaded": r.uploaded, "downloaded": r.downloaded, "applied": r.applied,
                "snapshots_verified": r.snapshots_verified, "snapshots_published": r.snapshots_published,
                "waiting": r.waiting, "held": r.held, "required_revision": r.required_revision,
                "downloads_deferred": r.downloads_deferred, "uploads_deferred": r.uploads_deferred,
                "pending_deletions": r.pending_deletions, "repaired_history": r.repaired_history,
                "compaction_capacity_limited": r.compaction_capacity_limited,
            }));
        }
        if attempt.error.is_some() {
            self.history.last_failure = Some(attempt.clone());
        }
        if !matches!(
            day_async::spawn_blocking(move || self.save()).await,
            Ok(Ok(()))
        ) {
            log::warn!("sync diagnostics could not record attempt completion");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostics_keep_categories_without_platform_secrets() {
        let error = StoreError::with_source(
            sync::StoreErrorKind::Unavailable,
            dav::Error::Transport {
                source: dav::http::HttpError::Io(
                    "synthetic-secret https://private.invalid/article".into(),
                ),
                outcome: dav::WriteOutcome::MayHaveApplied,
            },
        );
        let encoded = serde_json::to_string(&Failure::store(&error)).unwrap();
        assert!(encoded.contains("MayHaveApplied") && encoded.contains("\"io\""));
        assert!(!encoded.contains("synthetic-secret") && !encoded.contains("private.invalid"));
        let error = StoreError::with_source(
            sync::StoreErrorKind::Unavailable,
            dav::Error::Status {
                status: 429,
                retry_after: None,
                outcome: dav::WriteOutcome::NotSent,
            },
        )
        .retry_after(Some(std::time::Duration::from_secs(120)));
        let failure = Failure::store(&error);
        assert_eq!(failure.http_status, Some(429));
        assert_eq!(failure.retry_after_seconds, Some(120));
    }
    #[test]
    fn bounded_history_survives_restart_and_keeps_last_failure() {
        crate::native::tests::wait(async {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("diagnostics.json");
            let inner = sync::MemoryStore::new();
            let store = RecordingStore::new(&inner);
            Journal::start(path.clone(), true, false)
                .unwrap()
                .finish(&Err(sync::Error::Integrity), None, None, &store)
                .await;
            for _ in 0..ATTEMPTS + 3 {
                Journal::start(path.clone(), false, false)
                    .unwrap()
                    .finish(&Ok(sync::SyncReport::default()), None, Some(123), &store)
                    .await;
            }
            drop(Journal::start(path.clone(), true, true).unwrap());
            let reopened = Journal::start(path.clone(), true, false).unwrap();
            assert_eq!(reopened.history.attempts.len(), ATTEMPTS);
            assert_eq!(
                reopened.history.attempts[ATTEMPTS - 2].outcome,
                "completion_not_recorded"
            );
            assert_eq!(
                reopened.history.last_failure.unwrap().error.unwrap().code,
                "sync.integrity"
            );
            assert!(std::fs::metadata(path).unwrap().len() <= MAX_BYTES);
        });
    }
    #[test]
    fn store_observation_preserves_results_and_bounds_trace() {
        crate::native::tests::wait(async {
            let inner = sync::MemoryStore::new();
            let store = RecordingStore::new(&inner);
            let directory = vec!["synthetic-private-directory".into()];
            store.ensure_directory(&directory).await.unwrap();
            assert_eq!(
                store
                    .create(&directory, "synthetic-private-name", vec![1, 2])
                    .await
                    .unwrap(),
                Created::Created
            );
            for _ in 0..OPERATIONS + 1 {
                assert_eq!(
                    store
                        .get(&directory, "synthetic-private-name")
                        .await
                        .unwrap(),
                    Some(vec![1, 2])
                );
            }
            let trace = store.trace.lock().unwrap();
            assert_eq!(trace.recent.len(), OPERATIONS);
            assert_eq!(trace.counts["get"], OPERATIONS as u64 + 1);
            assert!(
                !serde_json::to_string(&*trace)
                    .unwrap()
                    .contains("synthetic-private")
            );
        });
    }
}

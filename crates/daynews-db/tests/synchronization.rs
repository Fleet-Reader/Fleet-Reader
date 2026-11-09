// SPDX-License-Identifier: Apache-2.0
use daynews_db::{
    Article, ArticleFields, Db,
    sync::{Account, EntryLink},
};
use daynews_sync::{
    engine::{Storage, synchronize},
    fixture::Server,
    *,
};
use futures_util::FutureExt;
use std::{
    future::Future,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
};
struct Thread(std::thread::Thread);
impl Wake for Thread {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}
fn wait<T>(future: impl Future<Output = T>) -> T {
    let w = Waker::from(Arc::new(Thread(std::thread::current())));
    let mut f = std::pin::pin!(future);
    loop {
        match f.as_mut().poll(&mut Context::from_waker(&w)) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::park(),
        }
    }
}
struct Replica {
    db: Db,
    account: u64,
}
impl Replica {
    fn new() -> Self {
        Self::from_db(Db::open_in_memory().unwrap())
    }
    fn from_db(db: Db) -> Self {
        let account = db
            .sync_connect("feedbin", "fixture@example.invalid")
            .unwrap();
        Self { db, account }
    }
    fn article(&self, remote: &str) -> u64 {
        self.db
            .container
            .query::<EntryLink>()
            .live()
            .try_collect()
            .unwrap()
            .into_iter()
            .find(|e| e.remote == remote)
            .unwrap()
            .article
    }
    fn read(&self, remote: &str) -> bool {
        use day_reactive::Binding;
        self.db
            .container
            .get::<Article>(self.article(remote))
            .unwrap()
            .is_read()
            .peek()
    }
    fn set_read(&self, remote: &str, read: bool) {
        self.db.set_read(self.article(remote), read).unwrap();
        self.db.container.save().unwrap();
    }
    fn saved<T>(&self, result: std::result::Result<T, day_persistence::DbError>) -> Result<T> {
        let v = result.map_err(|_| Error::Storage)?;
        self.db.container.save().map_err(|_| Error::Storage)?;
        Ok(v)
    }
}
impl Storage for Replica {
    fn checkpoint(&self) -> Call<'_, Option<String>> {
        async {
            use day_reactive::Binding;
            use daynews_db::sync::AccountFields;
            Ok(self
                .db
                .container
                .get::<Account>(self.account)
                .and_then(|a| a.checkpoint().peek()))
        }
        .boxed_local()
    }

    fn begin_entries(&self) -> Call<'_, bool> {
        async { self.saved(self.db.sync_begin_entries(self.account)) }.boxed_local()
    }
    fn pending(&self) -> Call<'_, Vec<Operation>> {
        async { self.saved(self.db.sync_pending(self.account)) }.boxed_local()
    }
    fn outcomes(&self, o: Vec<Outcome>) -> Call<'_, ()> {
        async move { self.saved(self.db.sync_outcomes(self.account, o)) }.boxed_local()
    }
    fn subscriptions(&self, c: Changes<Subscription>) -> Call<'_, ()> {
        async move { self.saved(self.db.sync_apply_subscriptions(self.account, c)) }.boxed_local()
    }
    fn taggings(&self, t: Vec<Tagging>) -> Call<'_, ()> {
        async move { self.saved(self.db.sync_apply_taggings(self.account, t)) }.boxed_local()
    }
    fn missing(&self, ids: Vec<String>) -> Call<'_, Vec<String>> {
        async move { Ok(self.db.sync_missing(self.account, ids)) }.boxed_local()
    }
    fn entries(&self, e: Vec<Entry>, s: ReadChanges) -> Call<'_, daynews_sync::engine::Imported> {
        async move { self.saved(self.db.sync_import_entries(self.account, e, &s)) }.boxed_local()
    }
    fn reading(&self, s: ReadChanges) -> Call<'_, ()> {
        async move { self.saved(self.db.sync_apply_read(self.account, s)) }.boxed_local()
    }
    fn finished(&self, checkpoint: Option<String>) -> Call<'_, ()> {
        async move {
            use day_reactive::Binding;
            use daynews_db::sync::AccountFields;
            let a = self.db.container.get::<Account>(self.account).unwrap();
            a.last_sync().write(Some(daynews_time::now_unix()));
            a.checkpoint().write(checkpoint);
            a.importing().write(false);
            self.saved(Ok(()))
        }
        .boxed_local()
    }
}

#[test]
fn two_installations_read_unread_and_restart_through_real_http_interface() {
    let server = Server::new();
    let provider = server.provider();
    let a = Replica::new();
    let b = Replica::new();
    wait(provider.authenticate()).unwrap();
    wait(synchronize(&provider, &a)).unwrap();
    wait(synchronize(&provider, &b)).unwrap();
    assert!(!a.read("1001"));
    assert!(!b.read("1001"));
    a.set_read("1001", true);
    assert_eq!(a.db.sync_pending_count(a.account), 1);
    wait(synchronize(&provider, &a)).unwrap();
    wait(synchronize(&provider, &b)).unwrap();
    assert!(b.read("1001"));
    assert!(!server.unread().contains(&1001));
    b.set_read("1001", false);
    wait(synchronize(&provider, &b)).unwrap();
    wait(synchronize(&provider, &a)).unwrap();
    assert!(!a.read("1001"));
    assert_eq!(a.db.sync_pending_count(a.account), 0);
    assert!(
        server
            .simulation
            .requests()
            .iter()
            .any(|r| r.method == "POST" && r.url.ends_with("unread_entries/delete.json"))
    );
    assert!(
        server
            .simulation
            .requests()
            .iter()
            .any(|r| r.url.contains("page=2"))
    );
}

#[test]
fn unchanged_read_snapshot_does_not_dirty_library_rows() {
    let server = Server::new();
    let p = server.provider();
    let a = Replica::new();
    wait(synchronize(&p, &a)).unwrap();
    let entries = a.db.container.cache::<EntryLink>();
    let articles = a.db.container.cache::<Article>();
    let before = (entries.version(), articles.version());
    a.db.sync_apply_read(a.account, wait(p.read_state()).unwrap())
        .unwrap();
    assert_eq!(before, (entries.version(), articles.version()));
}

#[test]
fn acknowledgement_cannot_clear_newer_local_intent() {
    let server = Server::new();
    let p = server.provider();
    let a = Replica::new();
    wait(synchronize(&p, &a)).unwrap();
    a.set_read("1001", true);
    let sent = a.db.sync_pending(a.account).unwrap();
    a.set_read("1001", false);
    let outcomes = wait(p.apply(sent)).unwrap();
    a.db.sync_outcomes(a.account, outcomes).unwrap();
    assert_eq!(a.db.sync_pending_count(a.account), 1);
    a.db.sync_apply_read(a.account, ReadChanges::UnreadSnapshot(vec![]))
        .unwrap();
    assert!(!a.read("1001"));
    wait(synchronize(&p, &a)).unwrap();
    assert!(server.unread().contains(&1001));
}

#[test]
fn partial_acknowledgement_retains_failure_and_downloads_continue() {
    let server = Server::new();
    let p = server.provider();
    let a = Replica::new();
    wait(synchronize(&p, &a)).unwrap();
    a.set_read("1001", true);
    a.set_read("1002", true);
    server.reject(1001);
    let report = wait(synchronize(&p, &a)).unwrap();
    assert_eq!(report.acknowledged, 1);
    assert_eq!(report.pending_error, Some(Error::Rejected));
    assert_eq!(a.db.sync_pending_count(a.account), 1);
    assert!(a.read("1001"));
    assert!(server.unread().contains(&1001));
    assert!(!server.unread().contains(&1002));
}

#[test]
fn failed_subscription_does_not_replay_history_after_restart() {
    use day_reactive::Binding;
    use daynews_db::sync::AccountFields;
    let path = std::env::temp_dir().join(format!(
        "daynews-sync-upload-failure-{}-{}.db",
        std::process::id(),
        daynews_time::now_epoch_ms()
    ));
    let server = Server::new();
    {
        let a = Replica::from_db(Db::open(&path).unwrap());
        a.db.sync_queue_subscribe(a.account, "https://fixture.invalid/rejected")
            .unwrap();
        let report = wait(synchronize(&server.provider(), &a)).unwrap();
        assert_eq!(report.pending_error, Some(Error::Rejected));
        assert_eq!((report.entries, report.imported), (2, 2));
        assert_eq!(a.db.sync_pending_count(a.account), 1);
        let account = a.db.container.get::<Account>(a.account).unwrap();
        assert!(!account.importing().peek());
        assert!(account.last_sync().peek().is_some());
        assert!(wait(a.checkpoint()).unwrap().is_some());
    }
    let before = server.simulation.requests().len();
    {
        let a = Replica::from_db(Db::open(&path).unwrap());
        // The same rejected upload is retried, while reading state still comes through.
        server.set_read(1001, true);
        let report = wait(synchronize(&server.provider(), &a)).unwrap();
        assert_eq!(report.pending_error, Some(Error::Rejected));
        assert_eq!((report.entries, report.imported), (0, 0));
        assert!(a.read("1001"));
        assert_eq!(a.db.sync_pending_count(a.account), 1);
    }
    let requests = server.simulation.requests();
    let article_requests: Vec<_> = requests[before..]
        .iter()
        .filter(|r| r.url.contains("/entries.json"))
        .collect();
    assert_eq!(
        article_requests.len(),
        1,
        "no second history page or body re-download"
    );
    // The fixture only returns an empty page for a matching If-None-Match (304).
    assert!(article_requests[0].url.contains("page=1&"));
    let _ = std::fs::remove_file(path);
}

#[test]
fn known_boundary_page_does_not_rewrite_records_or_count_as_new() {
    use day_reactive::Binding;
    use daynews_db::sync::AccountFields;
    use daynews_sync::engine::synchronize_with_progress;
    let server = Server::new();
    let a = Replica::new();
    wait(synchronize(&server.provider(), &a)).unwrap();
    // Simulate a changed first-page validator with no new entries on that page.
    let account = a.db.container.get::<Account>(a.account).unwrap();
    let mut checkpoint: serde_json::Value =
        serde_json::from_str(&account.checkpoint().peek().unwrap()).unwrap();
    checkpoint["entries_etag"] = serde_json::Value::Null;
    account.checkpoint().write(Some(checkpoint.to_string()));
    a.db.container.save().unwrap();
    let links = a.db.container.cache::<EntryLink>();
    let articles = a.db.container.cache::<Article>();
    let versions = (links.version(), articles.version());
    let events = std::cell::RefCell::new(Vec::new());
    let report = wait(synchronize_with_progress(&server.provider(), &a, |p| {
        events.borrow_mut().push(p);
        Ok(())
    }))
    .unwrap();
    assert_eq!((report.entries, report.imported), (1, 0));
    assert_eq!(versions, (links.version(), articles.version()));
    assert!(events.borrow().iter().all(|p| p.articles == 0));
}

#[test]
fn offline_intent_survives_database_reopen() {
    let path = std::env::temp_dir().join(format!(
        "daynews-sync-{}-{}.db",
        std::process::id(),
        daynews_time::now_epoch_ms()
    ));
    let server = Server::new();
    let p = server.provider();
    {
        let a = Replica::from_db(Db::open(&path).unwrap());
        wait(synchronize(&p, &a)).unwrap();
        a.set_read("1001", true);
    }
    {
        let b = Replica::from_db(Db::open(&path).unwrap());
        assert_eq!(b.db.sync_pending_count(b.account), 1);
        wait(synchronize(&p, &b)).unwrap();
        assert!(!server.unread().contains(&1001));
    }
    let _ = std::fs::remove_file(path);
}

#[test]
fn removal_does_not_resurrect_from_an_offline_replica() {
    use day_reactive::Binding;
    let server = Server::new();
    let p = server.provider();
    let a = Replica::new();
    let b = Replica::new();
    wait(synchronize(&p, &a)).unwrap();
    wait(synchronize(&p, &b)).unwrap();
    let feed =
        a.db.container
            .query::<daynews_db::Feed>()
            .live()
            .try_collect()
            .unwrap()[0]
            .id;
    a.db.delete_feed(feed).unwrap();
    wait(synchronize(&p, &a)).unwrap();
    wait(synchronize(&p, &b)).unwrap();
    assert!(matches!(wait(p.subscriptions()).unwrap(),Changes::Snapshot(v) if v.is_empty()));
    use daynews_db::FeedFields;
    assert!(
        b.db.container
            .get::<daynews_db::Feed>(feed)
            .unwrap()
            .unsubscribed()
            .peek()
    );
    assert!(b.db.container.get::<Account>(b.account).is_some());
}

#[test]
fn undo_before_and_after_acknowledgement_queues_the_restored_state() {
    use day_reactive::Binding;
    let server = Server::new();
    let p = server.provider();
    let a = Replica::new();
    wait(synchronize(&p, &a)).unwrap();
    a.set_read("1001", true);
    // The undo stack restores model fields; reconciliation must replace the pending read.
    a.db.container
        .get::<Article>(a.article("1001"))
        .unwrap()
        .is_read()
        .write(false);
    a.db.sync_after_undo().unwrap();
    wait(synchronize(&p, &a)).unwrap();
    assert!(server.unread().contains(&1001));
    a.set_read("1001", true);
    wait(synchronize(&p, &a)).unwrap();
    a.db.container
        .get::<Article>(a.article("1001"))
        .unwrap()
        .is_read()
        .write(false);
    a.db.sync_after_undo().unwrap();
    wait(synchronize(&p, &a)).unwrap();
    assert!(server.unread().contains(&1001));
}

#[test]
fn delete_and_rename_before_subscription_ack_are_not_lost() {
    let server = Server::new();
    let p = server.provider();
    let a = Replica::new();
    let feed =
        a.db.add_feed("https://fixture.invalid/new.xml", "New", None);
    a.db.sync_adopt_local(feed).unwrap();
    a.db.rename_feed(feed, "Renamed while offline").unwrap();
    wait(synchronize(&p, &a)).unwrap();
    wait(synchronize(&p, &a)).unwrap();
    assert!(
        matches!(wait(p.subscriptions()).unwrap(),Changes::Snapshot(v) if v.iter().any(|s|s.title=="Renamed while offline"))
    );
    let other =
        a.db.add_feed("https://fixture.invalid/deleted.xml", "Deleted", None);
    a.db.sync_adopt_local(other).unwrap();
    a.db.delete_feed(other).unwrap();
    wait(synchronize(&p, &a)).unwrap();
    wait(synchronize(&p, &a)).unwrap();
    assert!(
        matches!(wait(p.subscriptions()).unwrap(),Changes::Snapshot(v) if !v.iter().any(|s|s.url.ends_with("deleted.xml")))
    );
}

#[test]
fn provider_neutral_ids_and_deltas_do_not_remove_unmentioned_subscriptions() {
    let a = Replica::new();
    let sub = Subscription {
        id: "opaque:subscription/a".into(),
        feed_id: "urn:feed:x".into(),
        url: "https://fixture.invalid/other.xml".into(),
        title: "Other protocol".into(),
        site_url: None,
    };
    a.db.sync_apply_subscriptions(a.account, Changes::Snapshot(vec![sub.clone()]))
        .unwrap();
    a.db.sync_import_entries(
        a.account,
        vec![Entry {
            id: "opaque:entry/a".into(),
            feed_id: sub.feed_id.clone(),
            title: None,
            url: None,
            author: None,
            content: None,
            summary: None,
            published: 0,
        }],
        &ReadChanges::Delta(vec![]),
    )
    .unwrap();
    a.db.sync_apply_read(
        a.account,
        ReadChanges::Delta(vec![ReadState {
            id: "opaque:entry/a".into(),
            read: true,
        }]),
    )
    .unwrap();
    assert!(a.read("opaque:entry/a"));
    a.db.sync_apply_subscriptions(
        a.account,
        Changes::Delta {
            upserts: vec![],
            removed: vec![],
        },
    )
    .unwrap();
    use day_reactive::Binding;
    use daynews_db::FeedFields;
    let feed = daynews_db::feed_id(&sub.url);
    assert!(
        !a.db
            .container
            .get::<daynews_db::Feed>(feed)
            .unwrap()
            .unsubscribed()
            .peek()
    );
}

#[test]
fn failed_worker_transaction_rolls_back_article_and_outbox_together() {
    use day_persistence::{DatabaseWorker, DbError};
    use day_reactive::Binding;
    let w = wait(DatabaseWorker::open(|| {
        Db::open_in_memory().map(|d| d.container)
    }))
    .unwrap();
    let article = wait(w.write(|c| {
        let db = Db {
            container: c.clone(),
        };
        let a = db.sync_connect("fixture", "transaction@example.invalid")?;
        db.sync_apply_subscriptions(
            a,
            Changes::Snapshot(vec![Subscription {
                id: "sub".into(),
                feed_id: "feed".into(),
                url: "https://fixture.invalid/transaction".into(),
                title: "Fixture".into(),
                site_url: None,
            }]),
        )?;
        db.sync_import_entries(
            a,
            vec![Entry {
                id: "entry".into(),
                feed_id: "feed".into(),
                title: None,
                url: None,
                author: None,
                content: None,
                summary: None,
                published: 0,
            }],
            &ReadChanges::UnreadSnapshot(vec!["entry".into()]),
        )?;
        Ok(c.query::<EntryLink>().live().try_collect()?[0].article)
    }))
    .unwrap();
    let failed: std::result::Result<(), _> = wait(w.write(move |c| {
        Db {
            container: c.clone(),
        }
        .set_read(article, true)?;
        Err(DbError::driver("synthetic transaction failure"))
    }));
    assert!(failed.is_err());
    wait(w.read(move |c| {
        assert!(!c.try_get::<Article>(article)?.unwrap().is_read().peek());
        assert_eq!(c.table_count::<daynews_db::sync::Intent>()?, 0);
        Ok(())
    }))
    .unwrap();
    wait(w.close()).unwrap();
}

#[test]
fn real_undo_stack_preserves_acknowledgements_and_reconciles_unsubscribe() {
    use day_reactive::Binding;
    use daynews_db::FeedFields;
    let server = Server::new();
    let p = server.provider();
    let a = Replica::new();
    wait(synchronize(&p, &a)).unwrap();
    let stack = a.db.container.undo(20);
    a.set_read("1001", true);
    day_reactive::flush_sync();
    day_model::with_author("sync", || wait(synchronize(&p, &a))).unwrap();
    assert_eq!(a.db.sync_pending_count(a.account), 0);
    assert!(stack.undo());
    a.db.sync_after_undo().unwrap();
    day_model::with_author("sync", || wait(synchronize(&p, &a))).unwrap();
    assert!(server.unread().contains(&1001));
    let feed = daynews_db::feed_id("https://fixture.invalid/feed.xml");
    a.db.delete_feed(feed).unwrap();
    day_reactive::flush_sync();
    day_model::with_author("sync", || wait(synchronize(&p, &a))).unwrap();
    assert!(stack.undo());
    a.db.sync_after_undo().unwrap();
    day_model::with_author("sync", || wait(synchronize(&p, &a))).unwrap();
    assert!(
        !a.db
            .container
            .get::<daynews_db::Feed>(feed)
            .unwrap()
            .unsubscribed()
            .peek()
    );
    assert!(matches!(wait(p.subscriptions()).unwrap(),Changes::Snapshot(v) if !v.is_empty()));
}

/// A deliberately non-Feedbin protocol with string IDs, read deltas and opaque checkpoints.
struct DeltaProvider {
    restored: std::cell::RefCell<Option<String>>,
    fail: std::cell::Cell<bool>,
}
impl Provider for DeltaProvider {
    fn kind(&self) -> &'static str {
        "fixture-delta"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            subscriptions: true,
            read_state: true,
            entries: true,
        }
    }
    fn restore_checkpoint(&self, checkpoint: Option<String>) -> Result<()> {
        *self.restored.borrow_mut() = checkpoint;
        Ok(())
    }
    fn checkpoint(&self) -> Option<String> {
        Some("opaque/version:2".into())
    }
    fn authenticate(&self) -> Call<'_, ()> {
        async { Ok(()) }.boxed_local()
    }
    fn subscriptions(&self) -> Call<'_, Changes<Subscription>> {
        async {
            if self.restored.borrow().is_some() {
                return Ok(Changes::Unchanged);
            }
            Ok(Changes::Snapshot(vec![Subscription {
                id: "subscription:other".into(),
                feed_id: "feed:other".into(),
                url: "https://fixture.invalid/delta".into(),
                title: "Delta fixture".into(),
                site_url: None,
            }]))
        }
        .boxed_local()
    }
    fn entries(&self, _cursor: Option<String>) -> Call<'_, Page<Entry>> {
        async {
            if self.fail.replace(false) {
                return Err(Error::Network);
            }
            Ok(Page {
                total_pages: None,
                values: if self.restored.borrow().is_some() {
                    vec![]
                } else {
                    vec![Entry {
                        id: "entry:other".into(),
                        feed_id: "feed:other".into(),
                        title: None,
                        url: None,
                        author: None,
                        content: None,
                        summary: None,
                        published: 0,
                    }]
                },
                next: None,
            })
        }
        .boxed_local()
    }
    fn entries_by_id(&self, _ids: Vec<String>) -> Call<'_, Vec<Entry>> {
        async { Ok(vec![]) }.boxed_local()
    }
    fn read_state(&self) -> Call<'_, ReadChanges> {
        async {
            Ok(ReadChanges::Delta(vec![ReadState {
                id: "entry:other".into(),
                read: true,
            }]))
        }
        .boxed_local()
    }
    fn apply(&self, operations: Vec<Operation>) -> Call<'_, Vec<Outcome>> {
        async move {
            Ok(operations
                .into_iter()
                .map(|o| Outcome::Applied {
                    revision: o.revision,
                    subscription: None,
                })
                .collect())
        }
        .boxed_local()
    }
}
#[test]
fn alternate_protocol_checkpoints_advance_only_after_complete_import() {
    let a = Replica::new();
    let p = DeltaProvider {
        restored: Default::default(),
        fail: std::cell::Cell::new(true),
    };
    assert!(matches!(wait(synchronize(&p, &a)), Err(Error::Network)));
    assert_eq!(wait(a.checkpoint()).unwrap(), None);
    wait(synchronize(&p, &a)).unwrap();
    assert!(a.read("entry:other"));
    assert_eq!(
        wait(a.checkpoint()).unwrap().as_deref(),
        Some("opaque/version:2")
    );
    wait(synchronize(&p, &a)).unwrap();
    assert_eq!(p.restored.borrow().as_deref(), Some("opaque/version:2"));
    // A provider without collections never empties local folders.
    let feed = daynews_db::feed_id("https://fixture.invalid/delta");
    a.file(feed, Some("Local collection"));
    wait(synchronize(&p, &a)).unwrap();
    assert_eq!(a.folder(feed).as_deref(), Some("Local collection"));
}

#[test]
fn committed_entry_validator_skips_bodies_but_new_subscriptions_invalidate_it() {
    let server = Server::new();
    let p = server.provider();
    let a = Replica::new();
    assert_eq!(wait(synchronize(&p, &a)).unwrap().entries, 2);
    assert_eq!(wait(synchronize(&p, &a)).unwrap().entries, 0);
    server.set_read(1001, true);
    assert_eq!(wait(synchronize(&p, &a)).unwrap().entries, 0);
    assert!(a.read("1001"));
    let f =
        a.db.add_feed("https://fixture.invalid/another", "Another", None);
    a.db.sync_adopt_local(f).unwrap();
    assert_eq!(wait(synchronize(&p, &a)).unwrap().entries, 2);
}

#[test]
fn cancelled_import_reports_progress_and_resumes_without_advancing_checkpoint() {
    use daynews_sync::{engine::synchronize_with_progress, progress::Phase};
    let server = Server::new();
    let p = server.provider();
    let a = Replica::new();
    let events = std::cell::RefCell::new(Vec::new());
    let result = wait(synchronize_with_progress(&p, &a, |progress| {
        let cancel = progress.phase == Phase::Articles && progress.completed == 1;
        events.borrow_mut().push(progress);
        if cancel {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }));
    assert!(matches!(result, Err(Error::Cancelled)));
    assert_eq!(wait(a.checkpoint()).unwrap(), None);
    assert_eq!(a.db.container.query::<Article>().live().ids().len(), 1);
    let events = events.into_inner();
    let downloaded = events.last().unwrap();
    assert_eq!(downloaded.total, Some(2));
    assert_eq!(downloaded.articles, 1);
    assert_eq!(downloaded.feed.as_deref(), Some("Sync fixture"));
    assert!(!events.iter().any(|p| p.phase == Phase::Complete));
    wait(synchronize(&p, &a)).unwrap();
    assert_eq!(a.db.container.query::<Article>().live().ids().len(), 2);
    assert!(wait(a.checkpoint()).unwrap().is_some());
}

#[test]
fn reset_discards_pending_uploads_and_reimports_remote_state_without_remote_deletions() {
    use day_reactive::Binding;
    use daynews_db::{
        ArticleBody, Feed, Folder, Tag,
        sync::{Intent, SubscriptionLink},
    };
    let server = Server::new();
    let p = server.provider();
    let a = Replica::new();
    wait(synchronize(&p, &a)).unwrap();
    a.set_read("1001", true); // deliberately never uploaded
    a.db.container
        .get::<Article>(a.article("1001"))
        .unwrap()
        .is_starred()
        .write(true);
    a.db.add_folder("Synthetic local folder");
    a.db.add_feed(
        "https://fixture.invalid/local-only",
        "Synthetic local feed",
        None,
    );
    a.db.container.insert(Tag {
        id: 123,
        name: "Synthetic local tag".into(),
        ..Default::default()
    });
    a.db.container.save().unwrap();
    let request_count = server.simulation.requests().len();
    a.saved(a.db.sync_reset_local(a.account)).unwrap();
    macro_rules! empty {
        ($model:ty) => {
            assert!(a.db.container.query::<$model>().live().ids().is_empty());
        };
    }
    empty!(Article);
    empty!(ArticleBody);
    empty!(Feed);
    empty!(Folder);
    empty!(Tag);
    empty!(Intent);
    empty!(SubscriptionLink);
    empty!(EntryLink);
    assert_eq!(wait(a.checkpoint()).unwrap(), None);
    assert_eq!(a.db.sync_accounts().unwrap().len(), 1);
    assert!(a.db.sync_accounts().unwrap()[0].connected);
    assert!(server.unread().contains(&1001));
    wait(synchronize(&p, &a)).unwrap();
    assert_eq!(a.db.container.query::<Article>().live().ids().len(), 2);
    assert!(!a.read("1001"));
    assert_eq!(a.db.sync_pending_count(a.account), 0);
    assert!(
        server.simulation.requests()[request_count..]
            .iter()
            .all(|r| r.method == "GET")
    );
}

#[test]
fn interrupted_incremental_scan_does_not_stop_at_a_previously_imported_first_page() {
    use day_reactive::Binding;
    use daynews_db::sync::AccountFields;
    let server = Server::new();
    let p = server.provider();
    let a = Replica::new();
    wait(synchronize(&p, &a)).unwrap();
    // Model an interrupted later scan: its first page is cached, but an older read
    // article has not been received. It cannot be rescued by the missing-unread pass.
    server.set_read(1002, true);
    a.db.container.delete::<Article>(a.article("1002")).unwrap();
    let account = a.db.container.get::<Account>(a.account).unwrap();
    account.importing().write(true);
    account.checkpoint().write(None);
    a.db.container.save().unwrap();
    wait(synchronize(&p, &a)).unwrap();
    assert_eq!(a.db.container.query::<Article>().live().ids().len(), 2);
    assert!(a.read("1002"));
    assert!(!account.importing().peek());
}

// Synthetic Feedbin wire records: one recent read article, an old unread article and an
// old read article. Fixture transport deliberately pages one item at a time.
fn historical_server() -> Server {
    let server = Server::new();
    server.set_read(1001, true);
    server.replace_entries(vec![
        historical_entry(1001, "2026-10-18T12:00:00Z"),
        historical_entry(1002, "2026-01-02T12:00:00Z"),
        historical_entry(1003, "2026-01-01T12:00:00Z"),
    ]);
    server
}
fn historical_entry(id: u64, date: &str) -> serde_json::Value {
    serde_json::json!({"id":id,"feed_id":11,"title":format!("History fixture {id}"),
        "url":format!("https://fixture.invalid/history/{id}"),"content":"<p>Synthetic history.</p>",
        "summary":"Synthetic history", "published":date,"created_at":date})
}
const HISTORY_CUTOFF: i64 = 1_784_592_000; // 2026-07-21 UTC

#[test]
fn retention_limits_network_history_but_recovers_old_unread_in_bulk() {
    use daynews_sync::engine::synchronize_with_history;
    let server = historical_server();
    let a = Replica::new();
    let p = server.provider();
    let report = wait(synchronize_with_history(
        &p,
        &a,
        Some(HISTORY_CUTOFF),
        |_| Ok(()),
    ))
    .unwrap();
    assert_eq!(report.entries, 2);
    assert_eq!(report.imported, 2);
    assert!(a.read("1001"));
    assert!(!a.read("1002"));
    assert_eq!(wait(a.missing(vec!["1003".into()])).unwrap(), vec!["1003"]);
    let requests = server.simulation.requests();
    let history: Vec<_> = requests
        .iter()
        .filter(|r| r.url.contains("entries.json?page="))
        .collect();
    assert_eq!(history.len(), 1); // Never requests the historical read body or page 2.
    assert!(history[0].url.contains("since=2026-07-21T00%3A00%3A00Z"));
    assert!(
        requests
            .iter()
            .any(|r| r.url.ends_with("entries.json?ids=1002"))
    );
    let repeat = wait(synchronize_with_history(
        &p,
        &a,
        Some(HISTORY_CUTOFF),
        |_| Ok(()),
    ))
    .unwrap();
    assert_eq!(repeat.entries, 0); // Same window can reuse its HTTP validator.
}

#[test]
fn widening_retention_backfills_past_known_pages_and_cancelled_backfills_resume() {
    use daynews_sync::{engine::synchronize_with_history, progress::Phase};
    let server = historical_server();
    let a = Replica::new();
    let p = server.provider();
    wait(synchronize_with_history(
        &p,
        &a,
        Some(HISTORY_CUTOFF),
        |_| Ok(()),
    ))
    .unwrap();
    let checkpoint = wait(a.checkpoint()).unwrap();
    let result = wait(synchronize_with_history(&p, &a, None, |progress| {
        if progress.phase == Phase::Articles && progress.completed == 1 {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }));
    assert!(matches!(result, Err(Error::Cancelled)));
    assert_eq!(wait(a.checkpoint()).unwrap(), checkpoint);
    let report = wait(synchronize_with_history(&p, &a, None, |_| Ok(()))).unwrap();
    assert_eq!(report.entries, 3); // First two are known; third must still download.
    assert_eq!(report.imported, 1);
    assert!(a.read("1003"));
    assert_eq!(a.db.container.query::<Article>().live().ids().len(), 3);
}

#[test]
fn incremental_history_uses_server_watermark_with_launch_and_daily_lookback() {
    use daynews_sync::engine::synchronize_with_history;
    let server = historical_server();
    let a = Replica::new();
    let p = server.provider();
    server.set_response_date(1_792_368_000); // 2026-10-19 UTC
    wait(synchronize_with_history(
        &p,
        &a,
        Some(HISTORY_CUTOFF),
        |_| Ok(()),
    ))
    .unwrap();
    server.set_response_date(1_792_368_060);
    let lookback = wait(synchronize_with_history(
        &p,
        &a,
        Some(HISTORY_CUTOFF),
        |_| Ok(()),
    ))
    .unwrap();
    assert_eq!(lookback.entries, 1);
    let latest = || {
        server
            .simulation
            .requests()
            .into_iter()
            .rev()
            .find(|r| r.url.contains("entries.json?page="))
            .unwrap()
            .url
    };
    assert!(latest().contains("since=2026-10-18T00%3A00%3A00Z"));
    let incremental = wait(synchronize_with_history(
        &p,
        &a,
        Some(HISTORY_CUTOFF),
        |_| Ok(()),
    ))
    .unwrap();
    assert_eq!(incremental.entries, 0);
    assert!(latest().contains("since=2026-10-19T00%3A01%3A00Z"));
    // A fresh provider models relaunch: the durable server watermark survives, and the
    // first refresh deliberately looks back a day, as NetNewsWire does.
    wait(synchronize_with_history(
        &server.provider(),
        &a,
        Some(HISTORY_CUTOFF),
        |_| Ok(()),
    ))
    .unwrap();
    assert!(latest().contains("since=2026-10-18T00%3A01%3A00Z"));
    server.set_response_date(1_792_454_460); // Next day; move the successful server watermark.
    wait(synchronize_with_history(
        &p,
        &a,
        Some(HISTORY_CUTOFF),
        |_| Ok(()),
    ))
    .unwrap();
    wait(synchronize_with_history(
        &p,
        &a,
        Some(HISTORY_CUTOFF),
        |_| Ok(()),
    ))
    .unwrap();
    assert!(latest().contains("since=2026-10-19T00%3A01%3A00Z"));
}

#[test]
fn lookback_updates_content_without_erasing_local_read_star_or_tags() {
    use day_reactive::Binding;
    use daynews_db::{ArticleBody, ArticleBodyFields, ArticleRelations};
    use daynews_sync::engine::synchronize_with_history;
    let server = historical_server();
    let a = Replica::new();
    let p = server.provider();
    server.set_response_date(1_792_368_000);
    wait(synchronize_with_history(
        &p,
        &a,
        Some(HISTORY_CUTOFF),
        |_| Ok(()),
    ))
    .unwrap();
    let id = a.article("1001");
    let article = a.db.container.get::<Article>(id).unwrap();
    let arrival = article.first_seen_at().peek();
    a.db.set_starred(id, true);
    let tag = a.db.add_tag("Synthetic retained tag");
    a.db.set_tagged(id, tag, true);
    a.db.container.save().unwrap();
    let mut changed = historical_entry(1001, "2026-10-18T12:00:00Z");
    changed["content"] = serde_json::json!("<p>Corrected synthetic content.</p>");
    changed["title"] = serde_json::json!("Corrected fixture");
    server.replace_entries(vec![
        changed,
        historical_entry(1002, "2026-01-02T12:00:00Z"),
    ]);
    wait(synchronize_with_history(
        &p,
        &a,
        Some(HISTORY_CUTOFF),
        |_| Ok(()),
    ))
    .unwrap();
    assert_eq!(article.title().peek().as_deref(), Some("Corrected fixture"));
    assert_eq!(article.first_seen_at().peek(), arrival);
    assert!(article.is_read().peek());
    assert!(article.is_starred().peek());
    assert!(article.tags().contains(tag));
    assert_eq!(
        a.db.container
            .get::<ArticleBody>(id)
            .unwrap()
            .content_html()
            .peek(),
        "<p>Corrected synthetic content.</p>"
    );
}

#[test]
fn newly_subscribed_feed_backfills_within_retention_behind_known_recent_pages() {
    use daynews_sync::engine::synchronize_with_history;
    let server = historical_server();
    let a = Replica::new();
    let p = server.provider();
    server.set_response_date(1_792_368_000);
    wait(synchronize_with_history(
        &p,
        &a,
        Some(HISTORY_CUTOFF),
        |_| Ok(()),
    ))
    .unwrap();
    let outcomes = wait(p.apply(vec![Operation {
        revision: 99,
        mutation: Mutation::Subscribe {
            url: "https://fixture.invalid/new-history".into(),
        },
    }]))
    .unwrap();
    let feed_id = match &outcomes[0] {
        Outcome::Applied {
            subscription: Some(sub),
            ..
        } => sub.feed_id.parse::<u64>().unwrap(),
        _ => panic!("synthetic subscription was not accepted"),
    };
    let mut added = historical_entry(1004, "2026-08-01T12:00:00Z");
    added["feed_id"] = serde_json::json!(feed_id);
    server.replace_entries(vec![
        historical_entry(1001, "2026-10-18T12:00:00Z"),
        added,
        historical_entry(1002, "2026-01-02T12:00:00Z"),
    ]);
    let report = wait(synchronize_with_history(
        &p,
        &a,
        Some(HISTORY_CUTOFF),
        |_| Ok(()),
    ))
    .unwrap();
    assert_eq!(report.entries, 2);
    assert_eq!(report.imported, 1);
    assert!(wait(a.missing(vec!["1004".into()])).unwrap().is_empty());
}

const FIXTURE_FEED: &str = "https://fixture.invalid/feed.xml";
const FIXTURE_REMOTE_FEED: u64 = 11;
impl Replica {
    /// The fixture subscription's local folder, by name.
    fn folder(&self, feed: u64) -> Option<String> {
        use day_reactive::Binding;
        use daynews_db::{FeedFields, Folder, FolderFields};
        let folder = self
            .db
            .container
            .get::<daynews_db::Feed>(feed)
            .unwrap()
            .folder()
            .peek()?
            .id()?;
        Some(
            self.db
                .container
                .get::<Folder>(folder.handle())
                .unwrap()
                .name()
                .peek(),
        )
    }
    fn file(&self, feed: u64, folder: Option<&str>) {
        let folder = folder.map(|name| self.db.add_folder(name));
        self.db.set_feed_folder(feed, folder, None).unwrap();
        self.db.container.save().unwrap();
    }
    fn posts(server: &Server, path: &str) -> u64 {
        server
            .simulation
            .request_count("POST", &format!("https://api.feedbin.com/v2/{path}"))
    }
}

#[test]
fn folder_moves_upload_as_taggings_replacing_others_and_coalesce() {
    let server = Server::new();
    let p = server.provider();
    let a = Replica::new();
    wait(synchronize(&p, &a)).unwrap();
    let feed = daynews_db::feed_id(FIXTURE_FEED);
    // Another client filed the feed twice; Fleet Reader keeps one collection per feed.
    server.tag(FIXTURE_REMOTE_FEED, "Elsewhere");
    server.tag(FIXTURE_REMOTE_FEED, "Twice");
    a.file(feed, Some("Tech"));
    a.file(feed, Some("News"));
    assert_eq!(
        a.db.sync_pending_count(a.account),
        1,
        "the later move replaces"
    );
    wait(synchronize(&p, &a)).unwrap();
    assert_eq!(
        server.taggings(),
        [(FIXTURE_REMOTE_FEED, "News".to_string())]
    );
    assert_eq!(
        Replica::posts(&server, "taggings.json"),
        1,
        "only the last move"
    );
    assert_eq!(a.folder(feed).as_deref(), Some("News"));
    assert_eq!(a.db.sync_pending_count(a.account), 0);
    a.file(feed, None);
    wait(synchronize(&p, &a)).unwrap();
    assert!(server.taggings().is_empty(), "no collection removes them");
    assert_eq!(a.folder(feed), None);
}

#[test]
fn folder_rename_and_delete_upload_as_tag_edits_keeping_feeds_subscribed() {
    use day_reactive::Binding;
    use daynews_db::FeedFields;
    let server = Server::new();
    let p = server.provider();
    let a = Replica::new();
    wait(synchronize(&p, &a)).unwrap();
    let feed = daynews_db::feed_id(FIXTURE_FEED);
    a.file(feed, Some("News"));
    wait(synchronize(&p, &a)).unwrap();
    let news = a.db.folder_named("News").unwrap();
    assert!(a.db.rename_folder(news, "World").unwrap());
    a.db.container.save().unwrap();
    wait(synchronize(&p, &a)).unwrap();
    assert_eq!(Replica::posts(&server, "tags.json"), 1);
    assert_eq!(
        server.taggings(),
        [(FIXTURE_REMOTE_FEED, "World".to_string())]
    );
    assert_eq!(a.folder(feed).as_deref(), Some("World"));
    assert_eq!(
        a.db.folder_named("News"),
        None,
        "the old name is not recreated"
    );
    a.db.delete_folder(news).unwrap();
    a.db.container.save().unwrap();
    wait(synchronize(&p, &a)).unwrap();
    assert!(server.taggings().is_empty());
    assert!(matches!(wait(p.subscriptions()).unwrap(), Changes::Snapshot(v) if v.len() == 1));
    let row = a.db.container.get::<daynews_db::Feed>(feed).unwrap();
    assert!(!row.unsubscribed().peek());
    assert_eq!(a.folder(feed), None);
    assert_eq!(a.db.folder_named("World"), None);
    // A collection no subscription uses on the service needs no request at all.
    let local = a.db.add_folder("Local only");
    let before = server.simulation.requests().len();
    assert!(a.db.rename_folder(local, "Still local").unwrap());
    a.db.delete_folder(local).unwrap();
    a.db.container.save().unwrap();
    wait(synchronize(&p, &a)).unwrap();
    assert_eq!(a.db.sync_pending_count(a.account), 0);
    assert!(
        server.simulation.requests()[before..]
            .iter()
            .all(|r| r.method == "GET")
    );
}

#[test]
fn downloaded_taggings_create_folders_and_file_linked_feeds_only() {
    let server = Server::new();
    let p = server.provider();
    let a = Replica::new();
    let b = Replica::new();
    let feed = daynews_db::feed_id(FIXTURE_FEED);
    let local =
        a.db.add_feed("https://fixture.invalid/local-only", "Local", None);
    a.file(local, Some("Mine"));
    server.tag(FIXTURE_REMOTE_FEED, "Zeta");
    server.tag(FIXTURE_REMOTE_FEED, "alpha");
    wait(synchronize(&p, &a)).unwrap();
    assert_eq!(
        a.folder(feed).as_deref(),
        Some("alpha"),
        "several taggings: the alphabetically first"
    );
    assert_eq!(a.db.folder_named("Zeta"), None, "no feed is filed there");
    assert_eq!(
        a.db.sync_pending_count(a.account),
        0,
        "a download is no edit"
    );
    // Another installation's move arrives through the service.
    wait(synchronize(&p, &b)).unwrap();
    b.file(feed, Some("From B"));
    wait(synchronize(&p, &b)).unwrap();
    wait(synchronize(&p, &a)).unwrap();
    assert_eq!(a.folder(feed).as_deref(), Some("From B"));
    // Untagged on the service: out of every folder, which stays behind for the user.
    server.untag(FIXTURE_REMOTE_FEED);
    wait(synchronize(&p, &a)).unwrap();
    assert_eq!(a.folder(feed), None);
    assert!(a.db.folder_named("From B").is_some());
    assert!(a.db.folder_named("alpha").is_some());
    assert_eq!(
        a.folder(local).as_deref(),
        Some("Mine"),
        "unlinked feeds keep theirs"
    );
}

#[test]
fn pending_folder_edits_win_over_a_stale_tagging_download() {
    let server = Server::new();
    let p = server.provider();
    let a = Replica::new();
    let feed = daynews_db::feed_id(FIXTURE_FEED);
    server.tag(FIXTURE_REMOTE_FEED, "Theirs");
    wait(synchronize(&p, &a)).unwrap();
    assert_eq!(a.folder(feed).as_deref(), Some("Theirs"));
    a.file(feed, Some("Mine"));
    let stale = || wait(p.taggings()).unwrap().unwrap();
    a.db.sync_apply_taggings(a.account, stale()).unwrap();
    assert_eq!(a.folder(feed).as_deref(), Some("Mine"), "queued move wins");
    wait(synchronize(&p, &a)).unwrap();
    assert_eq!(
        server.taggings(),
        [(FIXTURE_REMOTE_FEED, "Mine".to_string())]
    );
    // A rename not yet uploaded maps the service's old name onto the new one.
    let mine = a.db.folder_named("Mine").unwrap();
    assert!(a.db.rename_folder(mine, "Renamed").unwrap());
    a.db.sync_apply_taggings(a.account, stale()).unwrap();
    assert_eq!(a.folder(feed).as_deref(), Some("Renamed"));
    assert_eq!(a.db.folder_named("Mine"), None);
    // So does a deletion: the service's tag no longer files the feed anywhere.
    a.db.delete_folder(mine).unwrap();
    a.db.sync_apply_taggings(a.account, stale()).unwrap();
    assert_eq!(a.folder(feed), None);
    assert_eq!(a.db.folder_named("Mine"), None);
    a.db.container.save().unwrap();
    wait(synchronize(&p, &a)).unwrap();
    assert!(server.taggings().is_empty());
    assert_eq!(a.db.sync_pending_count(a.account), 0);
}

#[test]
fn move_before_subscription_ack_and_into_a_renamed_folder_is_not_lost() {
    let server = Server::new();
    let p = server.provider();
    let a = Replica::new();
    let feed =
        a.db.add_feed("https://fixture.invalid/new.xml", "New", None);
    a.db.sync_adopt_local(feed).unwrap();
    a.file(feed, Some("Before"));
    // Queued before the subscription exists remotely: nothing to tag yet.
    assert_eq!(a.db.sync_pending_count(a.account), 1);
    wait(synchronize(&p, &a)).unwrap();
    wait(synchronize(&p, &a)).unwrap();
    let remote = match wait(p.subscriptions()).unwrap() {
        Changes::Snapshot(v) => v
            .into_iter()
            .find(|s| s.url.ends_with("new.xml"))
            .unwrap()
            .feed_id
            .parse::<u64>()
            .unwrap(),
        _ => unreachable!(),
    };
    assert_eq!(server.taggings(), [(remote, "Before".to_string())]);
    assert_eq!(a.folder(feed).as_deref(), Some("Before"));
    // A move into a folder renamed before either upload lands under the new name.
    let fixture = daynews_db::feed_id(FIXTURE_FEED);
    a.file(fixture, Some("Draft"));
    let draft = a.db.folder_named("Draft").unwrap();
    assert!(a.db.rename_folder(draft, "Final").unwrap());
    a.db.container.save().unwrap();
    wait(synchronize(&p, &a)).unwrap();
    assert!(
        server
            .taggings()
            .contains(&(FIXTURE_REMOTE_FEED, "Final".to_string()))
    );
    assert!(!server.taggings().iter().any(|(_, n)| n == "Draft"));
    assert_eq!(a.folder(fixture).as_deref(), Some("Final"));
}

#[test]
fn undoing_a_pending_move_queues_the_restored_folder() {
    let server = Server::new();
    let p = server.provider();
    let a = Replica::new();
    wait(synchronize(&p, &a)).unwrap();
    let feed = daynews_db::feed_id(FIXTURE_FEED);
    a.file(feed, Some("Kept"));
    wait(synchronize(&p, &a)).unwrap();
    let stack = a.db.container.undo(20);
    a.file(feed, Some("Undone"));
    day_reactive::flush_sync();
    assert!(stack.undo());
    a.db.sync_after_undo().unwrap();
    a.db.container.save().unwrap();
    assert_eq!(a.folder(feed).as_deref(), Some("Kept"));
    day_model::with_author("sync", || wait(synchronize(&p, &a))).unwrap();
    assert_eq!(
        server.taggings(),
        [(FIXTURE_REMOTE_FEED, "Kept".to_string())]
    );
    assert_eq!(a.folder(feed).as_deref(), Some("Kept"));
}

#[test]
fn journal_rows_from_an_older_build_still_decode() {
    use daynews_db::sync::Intent;
    let a = Replica::new();
    for (id, json) in [
        (1, r#"{"SetRead":{"id":"1001","read":true}}"#),
        (
            2,
            r#"{"Subscribe":{"url":"https://fixture.invalid/old.xml"}}"#,
        ),
        (3, r#"{"Unsubscribe":{"subscription":"101"}}"#),
        (
            4,
            r#"{"Rename":{"subscription":"101","title":"Older build"}}"#,
        ),
    ] {
        a.db.container.insert(Intent {
            id,
            account: a.account,
            revision: id,
            mutation: json.into(),
            blocked: false,
        });
    }
    let pending = a.db.sync_pending(a.account).unwrap();
    assert_eq!(pending.len(), 4);
    assert_eq!(
        pending[3].mutation,
        Mutation::Rename {
            subscription: "101".into(),
            title: "Older build".into(),
        }
    );
}

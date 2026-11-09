// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::native::tests::wait;
use daynews_db::Feed;

async fn worker() -> DatabaseWorker {
    DatabaseWorker::open(|| Db::open_in_memory().map(|db| db.container))
        .await
        .unwrap()
}

#[test]
fn selection_waits_for_sync_and_rechecks_removals_even_for_single_feed() {
    wait(async {
        let worker = worker().await;
        let old = worker
            .write(|c| {
                Ok(Db {
                    container: c.clone(),
                }
                .add_feed("https://old.invalid/feed", "Synthetic old", None))
            })
            .await
            .unwrap();
        let (_, feeds) = prepare(&worker, Selection::All, async {
            worker
                .write(move |c| {
                    let db = Db {
                        container: c.clone(),
                    };
                    db.delete_feed(old)?;
                    db.add_feed("https://new.invalid/feed", "Synthetic new", None);
                    Ok(())
                })
                .await
                .unwrap();
            true
        })
        .await
        .unwrap();
        assert_eq!(feeds.len(), 1);
        assert_eq!(feeds[0].1, "https://new.invalid/feed");
        assert!(
            prepare(&worker, Selection::One(old), async { true })
                .await
                .unwrap()
                .1
                .is_empty()
        );
        worker.close().await.unwrap();
    });
}

#[test]
fn failure_keeps_local_feeds_and_due_selection_respects_deadlines_and_retry_after() {
    wait(async {
        let worker = worker().await;
        worker
            .write(|c| {
                let db = Db {
                    container: c.clone(),
                };
                let due = db.add_feed("https://due.invalid/feed", "Synthetic due", None);
                let later = db.add_feed("https://later.invalid/feed", "Synthetic later", None);
                let retry = db.add_feed("https://retry.invalid/feed", "Synthetic retry", None);
                let removed =
                    db.add_feed("https://removed.invalid/feed", "Synthetic removed", None);
                c.get::<Feed>(later)
                    .unwrap()
                    .next_poll_at()
                    .write(Some(daynews_time::now_unix() + 3600));
                c.get::<Feed>(retry)
                    .unwrap()
                    .retry_after()
                    .write(Some(daynews_time::now_unix() + 3600));
                c.get::<Feed>(removed).unwrap().unsubscribed().write(true);
                assert_ne!(due, later);
                Ok(())
            })
            .await
            .unwrap();
        let (synced, all) = prepare(&worker, Selection::All, async { false })
            .await
            .unwrap();
        assert!(!synced);
        assert_eq!(all.len(), 3);
        let (synced, due) = prepare(&worker, Selection::Due, async { false })
            .await
            .unwrap();
        assert!(!synced);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].1, "https://due.invalid/feed");
        worker.close().await.unwrap();
    });
}

#[cfg(feature = "experimental-webdav")]
#[test]
fn replica_sync_precedes_feed_selection_and_retained_choices_apply_on_article_arrival() {
    use daynews_db::webdav::{
        self as adapter,
        day_sync::{Identity, MemoryStore},
    };
    use std::sync::Arc;
    wait(async {
        let dir = tempfile::tempdir().unwrap();
        let identity = Arc::new(Identity::create_space_for(adapter::PROFILE).unwrap());
        let peer = Arc::new(Identity::join(identity.space.clone()).unwrap());
        let owner = adapter::open(dir.path().join("owner.db"), identity, Arc::new(|_| Ok(())))
            .await
            .unwrap();
        let client = adapter::open(dir.path().join("client.db"), peer, Arc::new(|_| Ok(())))
            .await
            .unwrap();
        let remote = MemoryStore::new();
        owner
            .worker()
            .write(|c| {
                let db = Db {
                    container: c.clone(),
                };
                let feed = db.add_feed("https://synthetic.invalid/feed", "Synthetic feed", None);
                db.upsert_articles(feed, "https://synthetic.invalid/feed", &[article()]);
                Ok(())
            })
            .await
            .unwrap();
        // Reading is a later user transaction, as it is after a publisher import in the app.
        owner
            .worker()
            .write(|c| {
                let db = Db {
                    container: c.clone(),
                };
                db.set_read(
                    article_id("https://synthetic.invalid/feed", "synthetic-guid"),
                    true,
                )?;
                db.set_starred(
                    article_id("https://synthetic.invalid/feed", "synthetic-guid"),
                    true,
                );
                Ok(())
            })
            .await
            .unwrap();
        owner.sync(&remote).await.unwrap();
        assert!(active_feeds(&client.worker()).await.unwrap().is_empty());
        let (synced, feeds) = prepare(&client.worker(), Selection::All, async {
            client.sync(&remote).await.unwrap();
            true
        })
        .await
        .unwrap();
        assert!(synced);
        assert_eq!(feeds.len(), 1);
        let (id, url) = feeds[0].clone();
        client
            .worker()
            .write(move |c| {
                let db = Db {
                    container: c.clone(),
                };
                assert!(
                    c.get::<Article>(article_id(&url, "synthetic-guid"))
                        .is_none()
                );
                // Production feed import enters through the same DB transaction and arrival hooks.
                db.upsert_articles(id, &url, &[article()]);
                Ok(())
            })
            .await
            .unwrap();
        client
            .worker()
            .read(|c| {
                let a = c
                    .get::<Article>(article_id(
                        "https://synthetic.invalid/feed",
                        "synthetic-guid",
                    ))
                    .unwrap();
                assert!(a.is_read().peek());
                assert!(a.is_starred().peek());
                Ok(())
            })
            .await
            .unwrap();
        // A subsequent remote removal must affect the very next refresh selection.
        owner
            .worker()
            .write(|c| {
                Db {
                    container: c.clone(),
                }
                .delete_feed(feed_id("https://synthetic.invalid/feed"))
            })
            .await
            .unwrap();
        owner.sync(&remote).await.unwrap();
        let (_, feeds) = prepare(&client.worker(), Selection::All, async {
            client.sync(&remote).await.unwrap();
            true
        })
        .await
        .unwrap();
        assert!(feeds.is_empty());
        owner.worker().close().await.unwrap();
        client.worker().close().await.unwrap();
    });
}

#[cfg(feature = "experimental-webdav")]
fn article() -> IncomingArticle {
    IncomingArticle {
        guid: "synthetic-guid".into(),
        title: Some("Synthetic article".into()),
        url: None,
        author: None,
        published: Some(100),
        summary: None,
        content_html: None,
    }
}

#[test]
fn batches_bound_concurrency_deduplicate_and_drain_late_subscriptions_after_failures() {
    use std::cell::Cell;
    wait(async {
        let worker = worker().await;
        worker
            .write(|c| {
                for i in 0..9 {
                    Db {
                        container: c.clone(),
                    }
                    .add_feed(
                        &format!("https://fixture.invalid/{i}"),
                        "Synthetic feed",
                        None,
                    );
                }
                Ok(())
            })
            .await
            .unwrap();
        let feeds = active_feeds(&worker).await.unwrap();
        let first = feeds[0].0;
        let active = Rc::new(Cell::new(0));
        let peak = Rc::new(Cell::new(0));
        let seen = Rc::new(RefCell::new(HashSet::new()));
        state().refresh_progress.set(Some((0, 0)));
        let guard = RefreshGuard;
        let (done, failed) = fetch_batches(&worker, feeds, |id, _| {
            let worker = worker.clone();
            let active = active.clone();
            let peak = peak.clone();
            let seen = seen.clone();
            async move {
                assert!(seen.borrow_mut().insert(id), "a feed was fetched twice");
                active.set(active.get() + 1);
                peak.set(peak.get().max(active.get()));
                let mut yielded = false;
                std::future::poll_fn(move |cx| {
                    if std::mem::replace(&mut yielded, true) {
                        std::task::Poll::Ready(())
                    } else {
                        cx.waker().wake_by_ref();
                        std::task::Poll::Pending
                    }
                })
                .await;
                if id == first {
                    let late = worker
                        .write(|c| {
                            Ok(Db {
                                container: c.clone(),
                            }
                            .add_feed(
                                "https://fixture.invalid/late",
                                "Synthetic late",
                                None,
                            ))
                        })
                        .await
                        .unwrap();
                    // The same mailbox is filled by the sync completion callback. Include
                    // a duplicate and an obsolete id to exercise both filters.
                    RECEIVED.with(|pending| pending.borrow_mut().extend([first, late, u64::MAX]));
                }
                active.set(active.get() - 1);
                id != first // One publisher failing must not strand subsequent batches.
            }
        })
        .await
        .unwrap();
        assert_eq!((done, failed), (10, 1));
        assert_eq!(seen.borrow().len(), 10);
        assert_eq!(peak.get(), 4);
        assert_eq!(active.get(), 0);
        assert!(RECEIVED.with(|pending| pending.borrow().is_empty()));
        drop(guard);
        assert!(state().refresh_progress.get_untracked().is_none());
        worker.close().await.unwrap();
    });
}

#[test]
fn responses_for_removed_or_changed_subscriptions_are_discarded() {
    wait(async {
        for removed in [false, true] {
            let worker = worker().await;
            let id = worker
                .write(move |c| {
                    let id = Db {
                        container: c.clone(),
                    }
                    .add_feed(
                        "https://fixture.invalid/old",
                        "Synthetic original",
                        None,
                    );
                    let feed = c.get::<Feed>(id).unwrap();
                    if removed {
                        feed.unsubscribed().write(true);
                    } else {
                        feed.feed_url().write("https://fixture.invalid/new".into());
                    }
                    Ok(id)
                })
                .await
                .unwrap();
            let parsed = daynews_feed::parse(br#"<rss version="2.0"><channel><title>Synthetic stale response</title><link>https://fixture.invalid/</link><description>Fixture</description><item><guid>stale</guid><title>Stale item</title></item></channel></rss>"#, "https://fixture.invalid/old").unwrap();
            for result in [
                Ok(daynews_feed::FeedUpdate::Modified(
                    parsed,
                    Default::default(),
                    None,
                )),
                Ok(daynews_feed::FeedUpdate::NotModified(Default::default())),
                Err(daynews_feed::FeedError::Status(503)),
            ] {
                assert!(
                    apply_update(
                        worker.clone(),
                        id,
                        "https://fixture.invalid/old".into(),
                        result
                    )
                    .await
                );
            }
            worker
                .read(move |c| {
                    let feed = c.get::<Feed>(id).unwrap();
                    assert_eq!(feed.title().peek(), "Synthetic original");
                    assert!(feed.last_fetched_at().peek().is_none());
                    assert!(c.query::<Article>().live().try_collect()?.is_empty());
                    Ok(())
                })
                .await
                .unwrap();
            worker.close().await.unwrap();
        }
    });
}

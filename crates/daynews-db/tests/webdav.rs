#![cfg(all(feature = "experimental-webdav", not(target_arch = "wasm32")))]
use day_persistence::Value;
use day_reactive::Binding;
use daynews_db::{
    Db, FeedFields, IncomingArticle, Scope,
    webdav::{
        self, State,
        day_sync::{self, Identity, Replica, StoreErrorKind},
        day_sync_webdav::{WebDavStore, webdav::*},
    },
};
use futures_lite::future::block_on;
use futures_util::future::join_all;
use std::sync::Arc;
async fn edit(
    r: &Replica,
    f: impl FnOnce(&Db) -> Result<(), day_persistence::DbError> + Send + 'static,
) {
    r.worker()
        .write(move |c| {
            f(&Db {
                container: c.clone(),
            })
        })
        .await
        .unwrap();
}
async fn converge(rs: &[Replica], client: &WebDavStore) {
    for attempt in 0..12 {
        let results = join_all(rs.iter().map(|r| r.sync(client))).await;
        let mut retry = false;
        for result in results {
            if let Err(error) = result {
                match error {
                    day_sync::Error::Store(e)
                        if matches!(
                            e.kind,
                            StoreErrorKind::Forbidden
                                | StoreErrorKind::NotFound
                                | StoreErrorKind::Unavailable
                        ) =>
                    {
                        retry = true;
                        eprintln!("transient DAV contention; attempt={attempt}");
                    }
                    day_sync::Error::Store(e)
                        if matches!(
                            e.kind,
                            StoreErrorKind::Unauthorized | StoreErrorKind::InsufficientStorage
                        ) && std::env::var_os("DAY_NEWS_FAULT_TEST").is_some() =>
                    {
                        // Explicit operator retry after the proxy restores credentials/quota;
                        // production scheduling pauses these errors until user intervention.
                        retry = true;
                        eprintln!("synthetic credential/quota restoration; attempt={attempt}");
                    }
                    day_sync::Error::Store(_)
                    | day_sync::Error::Integrity
                    | day_sync::Error::RemoteObjectsRejected { .. }
                        if std::env::var_os("DAY_NEWS_FAULT_TEST").is_some() =>
                    {
                        retry = true;
                        eprintln!("injected transfer failure; attempt={attempt}");
                    }
                    other => panic!("unexpected sync error: {other:?}"),
                }
            }
        }
        if !retry && attempt >= 3 {
            let expected = rs[0].snapshot().await.unwrap();
            let mut settled =
                expected.pending_uploads == 0 && expected.waiting == 0 && !expected.poisoned;
            for r in rs.iter().skip(1) {
                let s = r.snapshot().await.unwrap();
                settled &= s.bookmarks == expected.bookmarks
                    && s.candidates == expected.candidates
                    && s.frontier == expected.frontier
                    && s.pending_uploads == 0
                    && s.waiting == 0
                    && !s.poisoned;
            }
            if settled {
                return;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(150 * (attempt + 1)));
    }
    let expected = rs[0].snapshot().await.unwrap();
    for r in rs {
        let s = r.snapshot().await.unwrap();
        assert!(
            s.bookmarks == expected.bookmarks,
            "projected state did not converge (private rows omitted)"
        );
        assert!(
            s.candidates == expected.candidates,
            "merge candidates did not converge (private rows omitted)"
        );
        assert_eq!(s.frontier, expected.frontier);
        assert_eq!(s.pending_uploads, 0);
        assert_eq!(s.waiting, 0);
        assert!(!s.poisoned);
    }
}
async fn exchange(rs: &[Replica]) {
    for from in rs {
        for to in rs {
            if from.actor() == to.actor() {
                continue;
            }
            for c in from.member_objects().await.unwrap() {
                to.receive_member(c).await.unwrap();
            }
            for object in from.local_objects().await.unwrap() {
                to.receive(object).await.unwrap();
            }
        }
    }
    for r in rs {
        r.apply_ready().await.unwrap();
    }
}
async fn replicas(dir: &std::path::Path, count: usize, source: Option<&str>) -> Vec<Replica> {
    replicas_with_keys(dir, count, source).await.0
}
async fn replicas_with_keys(
    dir: &std::path::Path,
    count: usize,
    source: Option<&str>,
) -> (Vec<Replica>, Vec<Arc<Identity>>) {
    let mut keys = vec![];
    let owner = Arc::new(Identity::create_space_for(webdav::PROFILE).unwrap());
    let mut result = vec![];
    for i in 0..count {
        let path = dir.join(format!("client-{i}.db"));
        if let Some(source) = source {
            std::fs::copy(source, &path).unwrap();
            let c = rusqlite::Connection::open(&path).unwrap();
            // Isolated test copies only. Never contact Feedbin or alter the source database.
            c.execute_batch("UPDATE feeds SET sync_account=NULL; DELETE FROM sync_intents; DELETE FROM sync_entries; DELETE FROM sync_subscriptions; DELETE FROM sync_accounts;").unwrap();
        }
        let credentials = if i == 0 {
            owner.clone()
        } else {
            Arc::new(Identity::join(owner.space.clone()).unwrap())
        };
        keys.push(credentials.clone());
        result.push(
            webdav::open(&path, credentials, Arc::new(|_| Ok(())))
                .await
                .unwrap(),
        );
    }
    exchange(&result).await;
    (result, keys)
}
fn incoming(guid: &str) -> IncomingArticle {
    IncomingArticle {
        guid: guid.into(),
        title: Some("Synthetic feed item".into()),
        url: None,
        author: None,
        published: Some(1_700_000_000),
        summary: None,
        content_html: Some("<p>Synthetic article body retained on unsubscribe.</p>".into()),
    }
}
#[test]
fn three_clients_preserve_fields_retention_and_unsubscribe() {
    block_on(async {
        let d = tempfile::tempdir().unwrap();
        let rs = replicas(d.path(), 3, None).await;
        for r in &rs {
            edit(r, |db| {
                db.add_feed("https://fixture.invalid/rss", "Synthetic feed", None);
                db.try_upsert_articles(
                    daynews_db::feed_id("https://fixture.invalid/rss"),
                    "https://fixture.invalid/rss",
                    &[incoming("a")],
                )?;
                Ok(())
            })
            .await;
        }
        exchange(&rs).await;
        let id = daynews_db::article_id("https://fixture.invalid/rss", "a");
        edit(&rs[0], move |d| {
            d.set_read(id, true)?;
            Ok(())
        })
        .await;
        edit(&rs[1], move |d| {
            d.set_starred(id, true);
            Ok(())
        })
        .await;
        exchange(&rs).await;
        for r in &rs {
            r.worker()
                .read(move |c| {
                    let a = c.try_get::<daynews_db::Article>(id)?.unwrap();
                    use daynews_db::ArticleFields;
                    assert!(a.is_read().peek());
                    assert!(a.is_starred().peek());
                    Ok(())
                })
                .await
                .unwrap();
        }
        // Eviction does not author a deletion; a later refetch recovers retained state.
        edit(&rs[2], move |d| {
            d.container.delete::<daynews_db::Article>(id)
        })
        .await;
        edit(&rs[2], |d| {
            d.try_upsert_articles(
                daynews_db::feed_id("https://fixture.invalid/rss"),
                "https://fixture.invalid/rss",
                &[incoming("a")],
            )?;
            Ok(())
        })
        .await;
        rs[2]
            .worker()
            .read(move |c| {
                use daynews_db::ArticleFields;
                let a = c.try_get::<daynews_db::Article>(id)?.unwrap();
                assert!(a.is_read().peek());
                assert!(a.is_starred().peek());
                Ok(())
            })
            .await
            .unwrap();
        for r in &rs {
            edit(r, move |d| {
                let tag = d.add_tag("Synthetic local annotation");
                d.set_tagged(id, tag, true);
                Ok(())
            })
            .await;
        }
        edit(&rs[0], |d| {
            d.delete_feed(daynews_db::feed_id("https://fixture.invalid/rss"))
        })
        .await;
        edit(&rs[1], move |d| {
            d.set_starred(id, false);
            Ok(())
        })
        .await;
        exchange(&rs).await;
        for r in &rs {
            r.worker()
                .read(|c| {
                    use daynews_db::FeedFields;
                    assert_eq!(c.table_count::<daynews_db::Feed>()?, 1);
                    assert!(
                        c.get::<daynews_db::Feed>(daynews_db::feed_id(
                            "https://fixture.invalid/rss"
                        ))
                        .unwrap()
                        .unsubscribed()
                        .peek()
                    );
                    assert_eq!(c.table_count::<daynews_db::Article>()?, 1);
                    assert_eq!(c.table_count::<daynews_db::ArticleBody>()?, 1);
                    c.try_with_connection(|sql| {
                        sql.query("SELECT count(*) FROM article_tags", &[], &mut |r| {
                            assert_eq!(r.get(0), Value::Int(1))
                        })
                    })?;
                    Ok(())
                })
                .await
                .unwrap();
        }
        // A causally later resubscription can restore the feed. Re-fetching content on each
        // client recovers retained flags without resurrecting stale competing choices.
        edit(&rs[0], |d| {
            d.add_feed(
                "https://fixture.invalid/rss",
                "Synthetic resubscription",
                None,
            );
            Ok(())
        })
        .await;
        exchange(&rs).await;
        for r in &rs {
            edit(r, |d| {
                d.try_upsert_articles(
                    daynews_db::feed_id("https://fixture.invalid/rss"),
                    "https://fixture.invalid/rss",
                    &[incoming("a"), incoming("new-publication")],
                )?;
                Ok(())
            })
            .await;
            r.worker()
                .read(move |c| {
                    use daynews_db::ArticleFields;
                    let a = c.try_get::<daynews_db::Article>(id)?.unwrap();
                    assert!(a.is_read().peek());
                    assert!(!a.is_starred().peek());
                    Ok(())
                })
                .await
                .unwrap();
        }
        exchange(&rs).await;
        for r in rs {
            r.close().await.unwrap();
        }
    });
}
#[test]
fn conflicts_use_causality_and_application_policy() {
    block_on(async {
        let d = tempfile::tempdir().unwrap();
        let rs = replicas(d.path(), 3, None).await;
        for kind in ["read", "star", "subscription"] {
            for (i, r) in rs.iter().enumerate() {
                let s = State {
                    kind: kind.into(),
                    feed: "https://fixture.invalid/conflict".into(),
                    guid: if kind == "subscription" { "" } else { "item" }.into(),
                    value: i == 0,
                    label: String::new(),
                };
                r.commit(vec![s.change().unwrap()]).await.unwrap();
            }
            exchange(&rs).await;
            for r in &rs {
                let s = r.snapshot().await.unwrap();
                let value = s
                    .bookmarks
                    .iter()
                    .find(|v| serde_json::from_str::<State>(&v.title).unwrap().kind == kind)
                    .unwrap();
                assert_eq!(
                    serde_json::from_str::<State>(&value.title).unwrap().value,
                    kind == "star"
                );
            }
        }
        // A later causally-informed choice overrides the concurrent preference.
        let s = State {
            kind: "star".into(),
            feed: "https://fixture.invalid/conflict".into(),
            guid: "item".into(),
            value: false,
            label: String::new(),
        };
        rs[2].commit(vec![s.change().unwrap()]).await.unwrap();
        exchange(&rs).await;
        for r in &rs {
            let s = r.snapshot().await.unwrap();
            assert!(
                !s.bookmarks
                    .iter()
                    .map(|b| serde_json::from_str::<State>(&b.title).unwrap())
                    .find(|s| s.kind == "star")
                    .unwrap()
                    .value
            );
        }
        for r in rs {
            r.close().await.unwrap();
        }
    });
}
#[test]
#[ignore = "requires private SQLite backup and isolated DAV endpoint; no live library writes"]
fn personal_three_client_live_shakeout() {
    block_on(async {
        let source = std::env::var("DAY_NEWS_TEST_DB").unwrap();
        let url = std::env::var("DAV_FIXTURE_URL").unwrap();
        let session = http::Session::new();
        session.set_provider(
            if std::env::var("DAV_PROVIDER").as_deref() == Ok("reqwest") {
                http::Provider::Reqwest
            } else {
                http::Provider::Native
            },
        );
        let client = WebDavStore::new(
            Client::builder(Endpoint::loopback(&url).unwrap())
                .session(session)
                .authentication(Authentication::Basic {
                    username: std::env::var("DAV_FIXTURE_USER")
                        .unwrap_or_else(|_| "day_fixture".into()),
                    password: std::env::var("DAV_FIXTURE_PASSWORD").unwrap(),
                })
                .build(),
        );
        let d = tempfile::tempdir().unwrap();
        let (mut rs, keys) = replicas_with_keys(d.path(), 3, Some(&source)).await;
        let start = std::time::Instant::now();
        webdav::seed(&rs[0]).await.unwrap();
        let before = rs[0].snapshot().await.unwrap();
        for state in [
            NetworkState::Offline,
            NetworkState::Constrained,
            NetworkState::Expensive,
        ] {
            client.client().set_network_state(state);
            assert!(rs[0].sync(&client).await.is_err());
            assert_eq!(rs[0].snapshot().await.unwrap(), before);
        }
        client.client().set_network_state(NetworkState::Available);
        converge(&rs, &client).await;
        let rounds = std::env::var("DAY_NEWS_TEST_ROUNDS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8);
        for round in 0..rounds {
            let jobs = rs.iter().enumerate().map(|(i, r)| {
                edit(r, move |db| {
                    db.set_read_all(Scope::All, (round + i) % 2 == 0)?;
                    let ids = db
                        .container
                        .query::<daynews_db::Article>()
                        .limit(100)
                        .live()
                        .try_ids()?;
                    for id in ids {
                        db.set_starred(id.handle(), (round + i) % 3 == 0);
                    }
                    let url = format!("https://fixture.invalid/round-{round}/client-{i}");
                    db.add_feed(&url, "Synthetic concurrent subscription", None);
                    db.try_upsert_articles(
                        daynews_db::feed_id(&url),
                        &url,
                        &[incoming("first"), incoming("second")],
                    )?;
                    if round > 0 {
                        let old = format!("https://fixture.invalid/round-{}/client-{i}", round - 1);
                        db.delete_feed(daynews_db::feed_id(&old))?;
                    }
                    Ok(())
                })
            });
            join_all(jobs).await;
            converge(&rs, &client).await;
            // Compare actual projected article state as well as encrypted CRDT state.
            let mut expected = None;
            for r in &rs {
                let state=r.worker().read(|c|c.try_with_connection(|conn|{let mut rows=vec![];conn.query("SELECT f.feed_url,a.guid,a.is_read,a.is_starred FROM articles a JOIN feeds f ON f.id=a.feed WHERE f.unsubscribed=0 AND f.feed_url NOT LIKE 'https://fixture.invalid/%' ORDER BY f.feed_url,a.guid",&[],&mut|r|rows.push((0..r.len()).map(|i|r.get(i)).collect::<Vec<Value>>()))?;Ok(rows)})).await.unwrap();
                if let Some(e) = &expected {
                    assert!(&state == e, "article state differs (private rows omitted)");
                } else {
                    expected = Some(state);
                }
            }
            println!(
                "round={round} elapsed_s={} transactions={}",
                start.elapsed().as_secs(),
                rs[0].snapshot().await.unwrap().transactions
            );
        }
        // Restart with the same actor, then enroll a fourth fresh database after history exists.
        rs[2].close().await.unwrap();
        rs[2] = webdav::open(
            d.path().join("client-2.db"),
            keys[2].clone(),
            Arc::new(|_| Ok(())),
        )
        .await
        .unwrap();
        let key = Arc::new(Identity::join(rs[0].space_descriptor()).unwrap());
        rs.push(
            webdav::open(d.path().join("late-joiner.db"), key, Arc::new(|_| Ok(())))
                .await
                .unwrap(),
        );
        converge(&rs, &client).await;
        println!("restart_and_late_join clients=4 passed");
        let count = rs[0].snapshot().await.unwrap().transactions;
        let space = rs[0].space().to_owned();
        for r in rs {
            r.close().await.unwrap();
        }
        client
            .client()
            .delete(
                &ResourcePath::from_segments([format!("day-sync-v4-{space}")], true).unwrap(),
                Condition::Unconditional,
            )
            .await
            .unwrap();
        println!(
            "RESULT clients=4 rounds={rounds} transactions={count} elapsed_s={}",
            start.elapsed().as_secs()
        );
    });
}

#[test]
fn bulk_journal_failure_rolls_back_models_and_every_chunk() {
    block_on(async {
        use std::sync::atomic::{AtomicBool, Ordering};
        let d = tempfile::tempdir().unwrap();
        let armed = Arc::new(AtomicBool::new(false));
        let flag = armed.clone();
        let r = webdav::open(
            d.path().join("bulk.db"),
            Arc::new(Identity::create_space_for(webdav::PROFILE).unwrap()),
            Arc::new(move |p| {
                if p == day_sync::Checkpoint::AfterJournal && flag.load(Ordering::SeqCst) {
                    Err(day_persistence::DbError::driver("synthetic journal fault"))
                } else {
                    Ok(())
                }
            }),
        )
        .await
        .unwrap();
        edit(&r, |db| {
            let f = db.add_feed("https://fixture.invalid/bulk", "Synthetic bulk feed", None);
            let articles = (0..600)
                .map(|i| incoming(&format!("item-{i}")))
                .collect::<Vec<_>>();
            db.try_upsert_articles(f, "https://fixture.invalid/bulk", &articles)?;
            Ok(())
        })
        .await;
        let before = r.snapshot().await.unwrap();
        armed.store(true, Ordering::SeqCst);
        let result = r
            .worker()
            .write(|c| {
                Db {
                    container: c.clone(),
                }
                .set_read_all(Scope::All, true)?;
                Ok(())
            })
            .await;
        assert!(result.is_err());
        assert_eq!(r.snapshot().await.unwrap(), before);
        r.worker()
            .read(|c| {
                assert_eq!(
                    c.query::<daynews_db::Article>()
                        .filter(daynews_db::Article::is_read().eq(true))
                        .live()
                        .try_ids()?
                        .len(),
                    0
                );
                Ok(())
            })
            .await
            .unwrap();
        armed.store(false, Ordering::SeqCst);
        edit(&r, |d| {
            d.set_read_all(Scope::All, true)?;
            Ok(())
        })
        .await;
        assert_eq!(
            r.snapshot().await.unwrap().transactions,
            before.transactions + 5
        );
        r.close().await.unwrap();
    });
}

#[test]
fn rejects_foreign_profile_and_does_not_capture_feedbin_changes() {
    block_on(async {
        let d = tempfile::tempdir().unwrap();
        let (rs, keys) = replicas_with_keys(d.path(), 2, None).await;
        edit(&rs[0], |db| {
            let a = db.sync_connect("feedbin", "synthetic-account")?;
            let f = db.add_feed(
                "https://fixture.invalid/provider",
                "Synthetic provider feed",
                None,
            );
            db.container.save()?;
            db.container.try_with_connection(|c| {
                c.execute(
                    "UPDATE feeds SET sync_account=? WHERE id=?",
                    &[Value::Int(a as i64), Value::Int(f as i64)],
                )?;
                Ok(())
            })?;
            db.try_upsert_articles(f, "https://fixture.invalid/provider", &[incoming("one")])?;
            Ok(())
        })
        .await;
        let before = rs[0].snapshot().await.unwrap().transactions;
        assert_eq!(
            before, 0,
            "Feedbin import must not publish a DAV subscription"
        );
        edit(&rs[0], |d| {
            let id = daynews_db::article_id("https://fixture.invalid/provider", "one");
            d.set_read(id, true)?;
            d.set_starred(id, true);
            Ok(())
        })
        .await;
        assert_eq!(rs[0].snapshot().await.unwrap().transactions, before);
        let mut c = State {
            kind: "star".into(),
            feed: "https://fixture.invalid/provider".into(),
            guid: "one".into(),
            value: true,
            label: String::new(),
        }
        .change()
        .unwrap();
        c.value.as_mut().unwrap().url = "foreign-profile".into();
        assert!(rs[1].commit(vec![c]).await.is_err());
        assert_eq!(rs[1].snapshot().await.unwrap().transactions, 0);
        // The space profile is bound before the app adapter can open the database.
        assert!(
            Replica::open(d.path().join("foreign.db"), keys[0].clone())
                .await
                .is_err()
        );
        let foreign = Replica::open(
            d.path().join("other-space.db"),
            Arc::new(Identity::create_space().unwrap()),
        )
        .await
        .unwrap();
        foreign
            .commit(vec![day_sync::Change {
                id: "11111111-1111-4111-8111-111111111111".into(),
                value: Some(day_sync::BookmarkValue {
                    url: "fixture".into(),
                    title: "foreign".into(),
                }),
            }])
            .await
            .unwrap();
        assert!(
            rs[1]
                .receive(foreign.local_objects().await.unwrap().remove(0))
                .await
                .is_err()
        );
        assert_eq!(rs[1].snapshot().await.unwrap().transactions, 0);
        foreign.close().await.unwrap();
        for r in rs {
            r.close().await.unwrap();
        }
    });
}

#[test]
fn populated_application_releases_writer_lease_before_close_returns() {
    block_on(async {
        let d = tempfile::tempdir().unwrap();
        let key = Arc::new(Identity::create_space_for(webdav::PROFILE).unwrap());
        for _ in 0..8 {
            let r = webdav::open(
                d.path().join("restart.db"),
                key.clone(),
                Arc::new(|_| Ok(())),
            )
            .await
            .unwrap();
            edit(&r, |db| {
                db.add_feed("https://fixture.invalid/restart", "Synthetic restart", None);
                db.try_upsert_articles(
                    daynews_db::feed_id("https://fixture.invalid/restart"),
                    "https://fixture.invalid/restart",
                    &[incoming("item")],
                )?;
                Ok(())
            })
            .await;
            r.snapshot().await.unwrap();
            r.close().await.unwrap();
            // Keep the closed handle alive: close completion, not Drop, releases the lease.
            let reopened = webdav::open(
                d.path().join("restart.db"),
                key.clone(),
                Arc::new(|_| Ok(())),
            )
            .await
            .unwrap();
            reopened.close().await.unwrap();
        }
    });
}

#[test]
#[ignore = "explicit capacity experiment; seals 4095 real authenticated records"]
fn history_limit_rolls_back_entire_bulk_edit_at_the_boundary() {
    block_on(async {
        let d = tempfile::tempdir().unwrap();
        let r = webdav::open(
            d.path().join("limit.db"),
            Arc::new(Identity::create_space_for(webdav::PROFILE).unwrap()),
            Arc::new(|_| Ok(())),
        )
        .await
        .unwrap();
        edit(&r, |db| {
            let f = db.add_feed("https://fixture.invalid/limit", "Synthetic capacity", None);
            db.try_upsert_articles(
                f,
                "https://fixture.invalid/limit",
                &(0..600)
                    .map(|i| incoming(&format!("item-{i}")))
                    .collect::<Vec<_>>(),
            )?;
            Ok(())
        })
        .await;
        let state = State {
            kind: "star".into(),
            feed: "https://fixture.invalid/absent".into(),
            guid: "absent".into(),
            value: true,
            label: String::new(),
        };
        for _ in 0..4094 {
            r.commit(vec![state.change().unwrap()]).await.unwrap();
        }
        let before = r.snapshot().await.unwrap();
        assert_eq!(before.transactions, 4095);
        let error = r
            .worker()
            .write(|c| {
                Db {
                    container: c.clone(),
                }
                .set_read_all(Scope::All, true)
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("retention_limit"));
        assert_eq!(r.snapshot().await.unwrap(), before);
        r.worker()
            .read(|c| {
                assert_eq!(
                    c.query::<daynews_db::Article>()
                        .filter(daynews_db::Article::is_read().eq(true))
                        .live()
                        .try_ids()?
                        .len(),
                    0
                );
                Ok(())
            })
            .await
            .unwrap();
        println!(
            "CAPACITY verified 4095 retained records; rejected multi-record edit rolled back every model and journal chunk"
        );
        r.close().await.unwrap();
    });
}

#[test]
fn offline_application_commands_create_real_concurrent_register_conflicts() {
    block_on(async {
        let d = tempfile::tempdir().unwrap();
        let rs = replicas(d.path(), 3, None).await;
        for r in &rs {
            edit(r, |db| {
                let f = db.add_feed(
                    "https://fixture.invalid/offline",
                    "Synthetic offline feed",
                    None,
                );
                db.try_upsert_articles(f, "https://fixture.invalid/offline", &[incoming("item")])?;
                Ok(())
            })
            .await;
        }
        exchange(&rs).await;
        let id = daynews_db::article_id("https://fixture.invalid/offline", "item");
        // Two separate offline transactions on B express a real false choice, avoiding
        // the application's intentional no-op when reasserting an unchanged false value.
        edit(&rs[0], move |db| {
            db.set_read(id, true)?;
            db.set_starred(id, true);
            Ok(())
        })
        .await;
        edit(&rs[1], move |db| {
            db.set_read(id, true)?;
            db.set_starred(id, true);
            Ok(())
        })
        .await;
        edit(&rs[1], move |db| {
            db.set_read(id, false)?;
            db.set_starred(id, false);
            Ok(())
        })
        .await;
        exchange(&rs).await;
        for r in &rs {
            r.worker()
                .read(move |c| {
                    use daynews_db::ArticleFields;
                    let a = c.try_get::<daynews_db::Article>(id)?.unwrap();
                    assert!(!a.is_read().peek());
                    assert!(a.is_starred().peek());
                    Ok(())
                })
                .await
                .unwrap();
        }
        edit(&rs[2], move |db| {
            db.set_read(id, true)?;
            db.set_starred(id, false);
            Ok(())
        })
        .await;
        exchange(&rs).await;
        for r in rs {
            r.worker()
                .read(move |c| {
                    use daynews_db::ArticleFields;
                    let a = c.try_get::<daynews_db::Article>(id)?.unwrap();
                    assert!(a.is_read().peek());
                    assert!(!a.is_starred().peek());
                    Ok(())
                })
                .await
                .unwrap();
            r.close().await.unwrap();
        }
    });
}

#[test]
#[ignore = "multi-epoch stress against isolated DAV using a private metadata-only template"]
fn personal_lifecycle_stress() {
    block_on(async {
        use day_sync::lifecycle::{Package, RecoveryPolicy};
        use daynews_db::ArticleFields;
        let source = std::env::var("DAY_NEWS_TEST_DB").unwrap();
        let clients: usize = std::env::var("LIFECYCLE_CLIENTS")
            .unwrap_or("6".into())
            .parse()
            .unwrap();
        let epochs: usize = std::env::var("LIFECYCLE_EPOCHS")
            .unwrap_or("5".into())
            .parse()
            .unwrap();
        let writes: usize = std::env::var("LIFECYCLE_WRITES")
            .unwrap_or("200".into())
            .parse()
            .unwrap();
        let session = http::Session::new();
        session.set_provider(
            if std::env::var("DAV_PROVIDER").as_deref() == Ok("reqwest") {
                http::Provider::Reqwest
            } else {
                http::Provider::Native
            },
        );
        let client = WebDavStore::new(
            Client::builder(
                Endpoint::loopback(&std::env::var("DAV_FIXTURE_URL").unwrap()).unwrap(),
            )
            .session(session)
            .authentication(Authentication::Basic {
                username: std::env::var("DAV_FIXTURE_USER")
                    .unwrap_or_else(|_| "day_fixture".into()),
                password: std::env::var("DAV_FIXTURE_PASSWORD").unwrap(),
            })
            .build(),
        );
        let d = tempfile::tempdir().unwrap();
        let (mut rs, _) = replicas_with_keys(d.path(), clients, Some(&source)).await;
        webdav::seed(&rs[0]).await.unwrap();
        converge(&rs, &client).await;
        let started = std::time::Instant::now();
        let mut total_records = 0;
        let mut namespace_cleanup = vec![];
        for epoch in 0..epochs {
            let before = rs[0].snapshot().await.unwrap().transactions;
            let work = rs.iter().enumerate().map(|(actor, r)| async move {
                let ids = r
                    .worker()
                    .read(|db| {
                        db.query::<daynews_db::Article>()
                            .limit(2000)
                            .live()
                            .try_ids()
                            .map(|ids| ids.into_iter().map(|i| i.handle()).collect::<Vec<_>>())
                    })
                    .await
                    .unwrap();
                if std::env::var_os("LIFECYCLE_BATCH").is_some() {
                    edit(r, move |db| {
                        for step in 0..writes {
                            let id = ids[(actor * 101 + step * 37) % ids.len()];
                            let article = db.container.try_get::<daynews_db::Article>(id)?.unwrap();
                            let read = !article.is_read().peek();
                            let star = !article.is_starred().peek();
                            db.set_read(id, read)?;
                            if step % 3 == 0 {
                                db.set_starred(id, star);
                            }
                        }
                        Ok(())
                    })
                    .await;
                } else {
                    for step in 0..writes {
                        let id = ids[(actor * 101 + step * 37) % ids.len()];
                        edit(r, move |db| {
                            let article = db.container.try_get::<daynews_db::Article>(id)?.unwrap();
                            let read = !article.is_read().peek();
                            let star = !article.is_starred().peek();
                            db.set_read(id, read)?;
                            if step % 3 == 0 {
                                db.set_starred(id, star);
                            }
                            Ok(())
                        })
                        .await;
                    }
                }
                edit(r, move |db| {
                    let url = format!("https://fixture.invalid/lifecycle-{epoch}/client-{actor}");
                    db.add_feed(&url, "Synthetic lifecycle feed", None);
                    db.try_upsert_articles(
                        daynews_db::feed_id(&url),
                        &url,
                        &[incoming("synthetic-lifecycle-article")],
                    )?;
                    if epoch > 0 {
                        db.delete_feed(daynews_db::feed_id(&format!(
                            "https://fixture.invalid/lifecycle-{}/client-{actor}",
                            epoch - 1
                        )))?;
                    }
                    Ok(())
                })
                .await;
            });
            join_all(work).await;
            converge(&rs, &client).await;
            let projection = lifecycle_projection(&rs[0]).await;
            for r in &rs {
                assert!(
                    lifecycle_projection(r).await == projection,
                    "app projection mismatch; private rows omitted"
                );
            }
            let old = rs[0].snapshot().await.unwrap();
            total_records += old.transactions - before;
            let freeze = uuid_for_fixture(epoch);
            for r in &rs {
                r.freeze(&freeze).await.unwrap();
            }
            converge(&rs, &client).await;
            let mut receipts = vec![];
            for r in &rs {
                receipts.push(r.attest().await.unwrap());
            }
            let next_dir = d.path().join(format!("epoch-{epoch}"));
            std::fs::create_dir(&next_dir).unwrap();
            let target_path = next_dir.join("owner.db");
            std::fs::copy(&source, &target_path).unwrap();
            let new_key = Arc::new(Identity::create_space_for(webdav::PROFILE).unwrap());
            let target = webdav::open(&target_path, new_key.clone(), Arc::new(|_| Ok(())))
                .await
                .unwrap();
            // Public identity persisted beside this isolated staging database.
            lifecycle_durable_file(
                &next_dir.join("staging-identity"),
                &new_key.to_bytes().unwrap(),
            );
            let checkpoint_start = std::time::Instant::now();
            let package = rs[0]
                .prepare_checkpoint(&target, receipts, vec![])
                .await
                .unwrap();
            let pin = package.pin().unwrap();
            let backup_path = next_dir.join("checkpoint.bin");
            lifecycle_durable_file(&backup_path, &package.export().unwrap());
            let readback = std::fs::read(&backup_path).unwrap();
            let permit = Package::verify_backup(&readback, &pin).unwrap();
            let mut published = false;
            for _ in 0..30 {
                if package.publish(&client, &pin).await.is_ok() {
                    published = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            assert!(published, "checkpoint publication retries exhausted");
            let fetched = Package::fetch(&client, &pin).await.unwrap();
            let mut next = vec![target];
            for i in 1..clients {
                let key = Arc::new(Identity::join(fetched.destination(&pin).unwrap()).unwrap());
                let path = next_dir.join(format!("client-{i}.db"));
                std::fs::copy(&source, &path).unwrap();
                let join = webdav::open(path, key, Arc::new(|_| Ok(()))).await.unwrap();
                join.install_checkpoint(&fetched, &pin).await.unwrap();
                assert!(
                    join.snapshot().await.unwrap().bookmarks == old.bookmarks,
                    "checkpoint state mismatch; private values omitted"
                );
                assert!(
                    lifecycle_projection(&join).await == projection,
                    "bootstrap app projection mismatch; private rows omitted"
                );
                next.push(join);
            }
            converge(&next, &client).await;
            let mut collected = None;
            for _ in 0..30 {
                match rs[0]
                    .collect_history(&client, &package, &pin, &permit)
                    .await
                {
                    Ok(n) => {
                        collected = Some(n);
                        break;
                    }
                    Err(_) => std::thread::sleep(std::time::Duration::from_millis(200)),
                }
            }
            assert!(collected.is_some(), "GC retries exhausted");
            assert!(
                rs[clients - 1]
                    .commit(vec![
                        State {
                            kind: "read".into(),
                            feed: "https://fixture.invalid/retired".into(),
                            guid: "stale".into(),
                            value: true,
                            label: String::new()
                        }
                        .change()
                        .unwrap()
                    ])
                    .await
                    .is_err()
            );
            // Recover after simulated total loss to another NEW root; never reopen an old actor.
            let recovered = Package::from_bytes(&readback, &pin)
                .unwrap()
                .recover(&pin, RecoveryPolicy::AcceptLossAfterCheckpoint)
                .unwrap();
            let recovered_path = next_dir.join("recovered.db");
            std::fs::copy(&source, &recovered_path).unwrap();
            let restored = webdav::open(
                recovered_path,
                recovered.identity.clone(),
                Arc::new(|_| Ok(())),
            )
            .await
            .unwrap();
            recovered.seed(&restored).await.unwrap();
            assert!(
                restored.snapshot().await.unwrap().bookmarks == old.bookmarks,
                "recovery state mismatch; private values omitted"
            );
            assert!(
                lifecycle_projection(&restored).await == projection,
                "recovered app projection mismatch; private rows omitted"
            );
            recovered.seed(&restored).await.unwrap();
            restored.close().await.unwrap();
            println!(
                "LIFECYCLE epoch={epoch} clients={clients} old_records={} checkpoint_records={} checkpoint_bytes={} cumulative_new_records={total_records} cutover_ms={} elapsed_s={}",
                old.transactions,
                next[0].snapshot().await.unwrap().transactions,
                package.byte_count(),
                checkpoint_start.elapsed().as_millis(),
                started.elapsed().as_secs()
            );
            namespace_cleanup.push(format!("day-sync-v4-{}", rs[0].space()));
            namespace_cleanup.push(format!("day-handoff-v4-{}", rs[0].space()));
            for r in rs {
                r.close().await.unwrap();
            }
            rs = next;
        }
        namespace_cleanup.push(format!("day-sync-v4-{}", rs[0].space()));
        for r in rs {
            r.close().await.unwrap();
        }
        for root in namespace_cleanup {
            client
                .client()
                .delete(
                    &ResourcePath::from_segments([root], true).unwrap(),
                    Condition::Unconditional,
                )
                .await
                .unwrap();
        }
        println!(
            "LIFECYCLE_RESULT clients={clients} epochs={epochs} writes_per_client={writes} cumulative_new_records={total_records} elapsed_s={}",
            started.elapsed().as_secs()
        );
    });
}
fn uuid_for_fixture(epoch: usize) -> String {
    format!("aaaaaaaa-aaaa-4aaa-8aaa-{epoch:012}")
}

async fn lifecycle_projection(r: &Replica) -> (Vec<Vec<Value>>, Vec<Vec<Value>>) {
    r.worker().read(|db|db.try_with_connection(|c|{
        let mut articles=vec![];let mut feeds=vec![];
        c.query("SELECT f.feed_url,a.guid,a.is_read,a.is_starred FROM articles a JOIN feeds f ON f.id=a.feed WHERE f.unsubscribed=0 AND f.feed_url NOT LIKE 'https://fixture.invalid/%' ORDER BY f.feed_url,a.guid",&[],&mut|r|articles.push((0..r.len()).map(|i|r.get(i)).collect()))?;
        c.query("SELECT feed_url FROM feeds WHERE unsubscribed=0 ORDER BY feed_url",&[],&mut|r|feeds.push((0..r.len()).map(|i|r.get(i)).collect()))?;
        Ok((articles,feeds))
    })).await.unwrap()
}
fn lifecycle_durable_file(path: &std::path::Path, bytes: &[u8]) {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
    #[cfg(unix)]
    std::fs::File::open(path.parent().unwrap())
        .unwrap()
        .sync_all()
        .unwrap();
}

#[test]
fn recovery_mode_blocks_all_write_origins_and_detach_preserves_backed_up_library() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.db");
        let backup = dir.path().join("backup.db");
        let credentials = Arc::new(Identity::create_space_for(webdav::PROFILE).unwrap());
        let replica = webdav::open(&path, credentials, Arc::new(|_| Ok(())))
            .await
            .unwrap();
        edit(&replica, |db| {
            let feed = db.add_feed(
                "https://fixture.invalid/rss",
                "Synthetic retained feed",
                None,
            );
            db.try_upsert_articles(feed, "https://fixture.invalid/rss", &[incoming("retained")])?;
            let tag = db.add_tag("Synthetic retained tag");
            db.set_tagged(
                daynews_db::article_id("https://fixture.invalid/rss", "retained"),
                tag,
                true,
            );
            Ok(())
        })
        .await;
        assert!(replica.snapshot().await.unwrap().pending_uploads > 0);
        replica.close().await.unwrap();
        let source = path.clone();
        let worker = day_persistence::DatabaseWorker::open(move || {
            let db = Db::open(&source)?;
            db.container
                .install_transaction_participant(webdav::RecoveryReadOnly)?;
            Ok(db.container)
        })
        .await
        .unwrap();
        assert_eq!(
            worker
                .read(|db| db.table_count::<daynews_db::Article>())
                .await
                .unwrap(),
            1
        );
        for origin in [
            day_persistence::WriteOrigin::Local,
            day_persistence::WriteOrigin::RemoteImport,
            day_persistence::WriteOrigin::Maintenance,
        ] {
            let error = worker
                .write_with_origin(origin, |db| {
                    db.delete::<daynews_db::Article>(daynews_db::article_id(
                        "https://fixture.invalid/rss",
                        "retained",
                    ))
                })
                .await
                .unwrap_err();
            assert_eq!(error.kind, day_persistence::DbErrorKind::Deny);
        }
        worker.close().await.unwrap();
        let db = Db::open(&path).unwrap();
        db.container.backup_to(&backup).unwrap();
        webdav::detach(&db.container).unwrap();
        webdav::detach(&db.container).unwrap(); // Restart after cleanup, before config deletion.
        assert!(!webdav::is_installed(&db.container));
        db.container.try_with_connection(|c| {
            c.query("SELECT count(*) FROM sqlite_master WHERE substr(name,1,10)='_day_sync_' OR substr(name,1,10)='_news_dav_'", &[], &mut |r| assert_eq!(r.get(0), Value::Int(0)))
        }).unwrap();
        assert_eq!(db.container.table_count::<daynews_db::Feed>().unwrap(), 1);
        assert_eq!(
            db.container.table_count::<daynews_db::Article>().unwrap(),
            1
        );
        assert_eq!(
            db.container
                .table_count::<daynews_db::ArticleBody>()
                .unwrap(),
            1
        );
        db.container
            .try_with_connection(|c| {
                c.query("SELECT count(*) FROM article_tags", &[], &mut |r| {
                    assert_eq!(r.get(0), Value::Int(1))
                })
            })
            .unwrap();
        let archived = rusqlite::Connection::open(&backup).unwrap();
        let pending: i64 = archived
            .query_row(
                "SELECT count(*) FROM _day_sync_transactions WHERE local=1 AND published=0",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            pending > 0,
            "disconnect must retain the unsent journal in its backup"
        );
        assert!(db.container.integrity_check().unwrap().is_empty());
        drop(db);
        let fresh = webdav::open(
            &path,
            Arc::new(Identity::create_space_for(webdav::PROFILE).unwrap()),
            Arc::new(|_| Ok(())),
        )
        .await
        .unwrap();
        webdav::seed(&fresh).await.unwrap();
        assert_eq!(fresh.snapshot().await.unwrap().bookmarks.len(), 1);
        fresh.close().await.unwrap();
    });
}

async fn layout_rows(r: &Replica) -> Vec<Vec<Value>> {
    r.worker().read(|db|db.try_with_connection(|c| {
        let mut out=vec![];
        for sql in ["SELECT id,name,position,sort FROM folders ORDER BY id", "SELECT feed_url,folder,collection_position,position FROM feeds WHERE sync_account IS NULL ORDER BY feed_url"] {
            c.query(sql,&[],&mut|row|out.push((0..row.len()).map(|i|row.get(i)).collect()))?;
        }
        Ok(out)
    })).await.unwrap()
}
fn layout_state(kind: &str, feed: impl ToString, value: bool, label: impl ToString) -> State {
    State {
        kind: kind.into(),
        feed: feed.to_string(),
        guid: String::new(),
        value,
        label: label.to_string(),
    }
}

#[test]
fn collections_converge_rename_order_membership_delete_and_recreate() {
    block_on(async {
        let d = tempfile::tempdir().unwrap();
        let (mut rs, keys) = replicas_with_keys(d.path(), 3, None).await;
        let a = daynews_db::folder_id("Synthetic A");
        let b = daynews_db::folder_id("Synthetic B");
        let f = daynews_db::feed_id("https://fixture.invalid/collections");
        edit(&rs[0], move |db| {
            assert_eq!(db.add_folder("Synthetic A"), a);
            db.add_folder("Synthetic B");
            db.add_feed("https://fixture.invalid/collections", "Synthetic", Some(a));
            db.try_upsert_articles(
                f,
                "https://fixture.invalid/collections",
                &[incoming("retained")],
            )?;
            db.set_folder_sort(a, daynews_db::FeedSort::Manual);
            Ok(())
        })
        .await;
        exchange(&rs).await;
        assert_eq!(layout_rows(&rs[0]).await, layout_rows(&rs[1]).await);
        // Independent concurrent edits must both survive: name and ordering are separate registers.
        edit(&rs[0], move |db| {
            db.rename_folder(a, "Synthetic Renamed")?;
            Ok(())
        })
        .await;
        edit(&rs[1], move |db| db.move_folder(a, 1)).await;
        edit(&rs[2], move |db| db.set_feed_folder(f, Some(b), None)).await;
        exchange(&rs).await;
        for r in &rs {
            assert_eq!(layout_rows(&rs[0]).await, layout_rows(r).await);
        }
        let values = layout_rows(&rs[0]).await;
        assert!(values.iter().any(
            |r| r.get(1) == Some(&Value::Text("Synthetic Renamed".into()))
                && r.get(2) == Some(&Value::Real(1.0))
        ));
        // Move into a collection concurrently with deleting it. The feed survives, ungrouped.
        edit(&rs[0], move |db| db.delete_folder(a)).await;
        edit(&rs[1], move |db| db.set_feed_folder(f, Some(a), None)).await;
        exchange(&rs).await;
        for (index, r) in rs.iter().enumerate() {
            assert_eq!(layout_rows(&rs[0]).await, layout_rows(r).await);
            r.worker()
                .read(move |c| {
                    let db = Db {
                        container: c.clone(),
                    };
                    assert!(c.get::<daynews_db::Folder>(a).is_none());
                    assert!(
                        c.get::<daynews_db::Feed>(f)
                            .unwrap()
                            .folder()
                            .peek()
                            .is_none()
                    );
                    assert_eq!(
                        db.count(Scope::Feed(f)).get(),
                        if index == 0 { 1 } else { 0 },
                        "deleting a collection must preserve cached articles"
                    );
                    Ok(())
                })
                .await
                .unwrap();
        }
        edit(&rs[0], move |db| {
            let new = db.add_folder("Synthetic A");
            assert_ne!(new, a);
            Ok(())
        })
        .await;
        exchange(&rs).await;
        let before = rs[0].snapshot().await.unwrap().transactions;
        edit(&rs[0], move |db| {
            db.set_folder_sort(b, daynews_db::FeedSort::Priority);
            Ok(())
        })
        .await;
        assert_eq!(
            before,
            rs[0].snapshot().await.unwrap().transactions,
            "assigning the same sort is a no-op"
        );
        rs[2].close().await.unwrap();
        rs[2] = webdav::open(
            d.path().join("client-2.db"),
            keys[2].clone(),
            Arc::new(|_| Ok(())),
        )
        .await
        .unwrap();
        exchange(&rs).await;
        assert_eq!(layout_rows(&rs[0]).await, layout_rows(&rs[2]).await);
        let late = webdav::open(
            d.path().join("late.db"),
            Arc::new(Identity::join(keys[0].space.clone()).unwrap()),
            Arc::new(|_| Ok(())),
        )
        .await
        .unwrap();
        rs.push(late);
        exchange(&rs).await;
        assert_eq!(layout_rows(&rs[0]).await, layout_rows(&rs[3]).await);
    });
}

#[test]
fn concurrent_duplicate_names_and_name_swaps_are_deterministic() {
    block_on(async {
        let d = tempfile::tempdir().unwrap();
        let rs = replicas(d.path(), 3, None).await;
        let a = daynews_db::folder_id("Synthetic A");
        let b = daynews_db::folder_id("Synthetic B");
        edit(&rs[0], |db| {
            db.add_folder("Synthetic A");
            db.add_folder("Synthetic B");
            Ok(())
        })
        .await;
        exchange(&rs).await;
        edit(&rs[0], move |db| {
            db.rename_folder(a, "Synthetic Shared")?;
            Ok(())
        })
        .await;
        edit(&rs[1], move |db| {
            db.rename_folder(b, "Synthetic Shared")?;
            Ok(())
        })
        .await;
        exchange(&rs).await;
        let expected = layout_rows(&rs[0]).await;
        assert_eq!(expected.len(), 2);
        assert_ne!(expected[0][1], expected[1][1]);
        for r in &rs {
            assert_eq!(expected, layout_rows(r).await);
        }
        // Explicit simultaneous name swap exercises UNIQUE(name) in both arrival orders.
        rs[0]
            .commit(vec![
                layout_state("collection", a, true, "Synthetic B")
                    .change()
                    .unwrap(),
                layout_state("collection", b, true, "Synthetic A")
                    .change()
                    .unwrap(),
            ])
            .await
            .unwrap();
        exchange(&rs).await;
        for r in &rs {
            assert_eq!(layout_rows(&rs[0]).await, layout_rows(r).await);
        }
        assert!(
            layout_rows(&rs[0])
                .await
                .iter()
                .any(|r| r[0] == Value::Int(a as i64) && r[1] == Value::Text("Synthetic B".into()))
        );
    });
}

#[test]
fn retained_membership_handles_arbitrary_dependency_order_and_deleted_feeds() {
    block_on(async {
        let d = tempfile::tempdir().unwrap();
        let rs = replicas(d.path(), 2, None).await;
        let folder = daynews_db::folder_id("Synthetic Late");
        let url = "https://fixture.invalid/late-layout";
        rs[0]
            .commit(vec![
                layout_state("membership", url, false, folder)
                    .change()
                    .unwrap(),
                layout_state("feed_order", url, false, "42.0")
                    .change()
                    .unwrap(),
            ])
            .await
            .unwrap();
        exchange(&rs).await;
        rs[0]
            .commit(vec![
                layout_state("collection", folder, true, "Synthetic Late")
                    .change()
                    .unwrap(),
            ])
            .await
            .unwrap();
        exchange(&rs).await;
        rs[0]
            .commit(vec![
                layout_state("subscription", url, true, "Synthetic")
                    .change()
                    .unwrap(),
            ])
            .await
            .unwrap();
        exchange(&rs).await;
        for r in &rs {
            r.worker()
                .read(move |c| {
                    let f = c.get::<daynews_db::Feed>(daynews_db::feed_id(url)).unwrap();
                    assert_eq!(f.folder().peek().unwrap().id().unwrap().handle(), folder);
                    assert_eq!(f.collection_position().peek(), 42.0);
                    Ok(())
                })
                .await
                .unwrap();
        }
        // Deleting a collection never cascades through Feedbin-owned subscriptions either.
        edit(&rs[0], move |db| {
            let account = db.sync_connect("feedbin", "synthetic-account")?;
            let f = db.add_feed(
                "https://fixture.invalid/feedbin-layout",
                "Synthetic managed feed",
                Some(folder),
            );
            db.container
                .get::<daynews_db::Feed>(f)
                .unwrap()
                .sync_account()
                .write(Some(account));
            Ok(())
        })
        .await;
        rs[1]
            .commit(vec![
                layout_state("collection", folder, false, "Synthetic Late")
                    .change()
                    .unwrap(),
            ])
            .await
            .unwrap();
        exchange(&rs).await;
        rs[0]
            .worker()
            .read(|c| {
                let f = c
                    .get::<daynews_db::Feed>(daynews_db::feed_id(
                        "https://fixture.invalid/feedbin-layout",
                    ))
                    .unwrap();
                assert!(f.folder().peek().is_none());
                Ok(())
            })
            .await
            .unwrap();
    });
}

#[test]
fn existing_collections_seed_once_and_empty_joiner_does_not_clear_them() {
    block_on(async {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("owner.db");
        let db = Db::open(&path).unwrap();
        let folder = db.add_folder("Synthetic Imported");
        db.add_folder("Synthetic Empty");
        db.add_feed(
            "https://fixture.invalid/import-layout",
            "Synthetic",
            Some(folder),
        );
        db.container.save().unwrap();
        drop(db);
        let key = Arc::new(Identity::create_space_for(webdav::PROFILE).unwrap());
        let owner = webdav::open(&path, key.clone(), Arc::new(|_| Ok(())))
            .await
            .unwrap();
        webdav::seed(&owner).await.unwrap();
        let join = webdav::open(
            d.path().join("join.db"),
            Arc::new(Identity::join(key.space.clone()).unwrap()),
            Arc::new(|_| Ok(())),
        )
        .await
        .unwrap();
        assert_eq!(join.snapshot().await.unwrap().transactions, 0);
        let rs = [owner, join];
        exchange(&rs).await;
        assert_eq!(layout_rows(&rs[0]).await, layout_rows(&rs[1]).await);
        let before = rs[0].snapshot().await.unwrap().transactions;
        rs[0].close().await.unwrap();
        let reopened = webdav::open(&path, key, Arc::new(|_| Ok(())))
            .await
            .unwrap();
        assert_eq!(before, reopened.snapshot().await.unwrap().transactions);
    });
}

#[test]
fn collection_validation_rejects_malformed_and_unbounded_registers() {
    for s in [
        layout_state("collection", "0", true, "Synthetic"),
        layout_state("collection", "01", true, "Synthetic"),
        layout_state("collection", "1", true, " "),
        layout_state("collection_order", "1", false, "NaN"),
        layout_state("collection_order", "1", false, "1000000000001.0"),
        layout_state("collection_sort", "1", false, "unsupported"),
        layout_state("membership", "https://fixture.invalid/rss", false, "-1"),
        layout_state("feed_order", "https://fixture.invalid/rss", true, "1.0"),
    ] {
        assert!(s.change().is_err());
    }
}

/// Models a deployed revision-one reader: it rejects new record types if asked to validate
/// them. The engine must retain those records without calling the older adapter on them.
struct RevisionOne;
impl day_sync::Application for RevisionOne {
    fn profile(&self) -> &'static str {
        webdav::PROFILE
    }
    fn install(
        &self,
        c: &mut dyn day_persistence::SqliteConnection,
    ) -> Result<(), day_persistence::DbError> {
        day_sync::Application::install(&webdav::NewsApplication, c)
    }
    fn capture(
        &self,
        c: &mut dyn day_persistence::SqliteConnection,
        changes: &[day_persistence::RowChange],
    ) -> Result<Vec<day_sync::Change>, day_persistence::DbError> {
        day_sync::Application::capture(&webdav::NewsApplication, c, changes)
    }
    fn validate(&self, change: &day_sync::Change) -> Result<(), day_persistence::DbError> {
        let state: State = serde_json::from_str(&change.value.as_ref().unwrap().title).unwrap();
        if !matches!(state.kind.as_str(), "subscription" | "read" | "star") {
            return Err(day_persistence::DbError::driver(
                "synthetic old reader cannot decode collection",
            ));
        }
        day_sync::Application::validate(&webdav::NewsApplication, change)
    }
    fn resolve(
        &self,
        values: &[Option<day_sync::BookmarkValue>],
    ) -> Result<usize, day_persistence::DbError> {
        day_sync::Application::resolve(&webdav::NewsApplication, values)
    }
    fn project(
        &self,
        c: &mut dyn day_persistence::SqliteConnection,
        id: &str,
        value: Option<&day_sync::BookmarkValue>,
    ) -> Result<(), day_persistence::DbError> {
        day_sync::Application::project(&webdav::NewsApplication, c, id, value)
    }
}

#[test]
fn old_reader_holds_collections_and_upgrade_releases_them_after_compaction() {
    block_on(async {
        let d = tempfile::tempdir().unwrap();
        let key = Arc::new(Identity::create_space_for(webdav::PROFILE).unwrap());
        let old_key = Arc::new(Identity::join(key.space.clone()).unwrap());
        let old_path = d.path().join("old.db");
        let owner = webdav::open(d.path().join("owner.db"), key.clone(), Arc::new(|_| Ok(())))
            .await
            .unwrap();
        let old = Replica::open_application(
            &old_path,
            old_key.clone(),
            Arc::new(|_| Ok(())),
            Some(Arc::new(RevisionOne)),
            |p| Db::open(&p).map(|db| db.container),
        )
        .await
        .unwrap();
        edit(&owner, |db| {
            let f = db.add_folder("Synthetic Versioned");
            db.add_feed("https://fixture.invalid/versioned", "Synthetic", Some(f));
            Ok(())
        })
        .await;
        let store = day_sync::MemoryStore::new();
        owner.sync(&store).await.unwrap();
        old.sync(&store).await.unwrap();
        owner.sync(&store).await.unwrap();
        let snap = old.snapshot().await.unwrap();
        assert!(snap.held > 0);
        assert_eq!(snap.required_revision, 2);
        assert_eq!(
            old.worker()
                .read(|c| Ok(c.query::<daynews_db::Folder>().live().try_collect()?.len()))
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            old.worker()
                .read(|c| Ok(c.query::<daynews_db::Feed>().live().try_collect()?.len()))
                .await
                .unwrap(),
            1
        );
        // Old clients can still publish ordinary reading choices while newer records are held.
        let read = State {
            kind: "read".into(),
            feed: "https://fixture.invalid/versioned".into(),
            guid: "synthetic-evicted".into(),
            value: true,
            label: String::new(),
        };
        old.commit(vec![read.change().unwrap()]).await.unwrap();
        old.sync(&store).await.unwrap();
        owner.sync(&store).await.unwrap();
        owner.compact(&store).await.unwrap();
        old.sync(&store).await.unwrap();
        old.close().await.unwrap();
        let upgraded = webdav::open(&old_path, old_key, Arc::new(|_| Ok(())))
            .await
            .unwrap();
        assert_eq!(upgraded.snapshot().await.unwrap().held, 0);
        assert_eq!(layout_rows(&owner).await, layout_rows(&upgraded).await);
        let fresh = webdav::open(
            d.path().join("fresh.db"),
            Arc::new(Identity::join(key.space.clone()).unwrap()),
            Arc::new(|_| Ok(())),
        )
        .await
        .unwrap();
        fresh.sync(&store).await.unwrap();
        assert_eq!(layout_rows(&owner).await, layout_rows(&fresh).await);
        assert_eq!(
            owner.snapshot().await.unwrap().bookmarks,
            fresh.snapshot().await.unwrap().bookmarks
        );
    });
}

#[test]
fn collection_journal_fault_rolls_back_models_archive_and_pending_choices() {
    block_on(async {
        use std::sync::atomic::{AtomicBool, Ordering};
        let d = tempfile::tempdir().unwrap();
        let armed = Arc::new(AtomicBool::new(false));
        let flag = armed.clone();
        let r = webdav::open(
            d.path().join("fault.db"),
            Arc::new(Identity::create_space_for(webdav::PROFILE).unwrap()),
            Arc::new(move |p| {
                if p == day_sync::Checkpoint::AfterJournal && flag.load(Ordering::SeqCst) {
                    Err(day_persistence::DbError::driver(
                        "synthetic collection journal failure",
                    ))
                } else {
                    Ok(())
                }
            }),
        )
        .await
        .unwrap();
        edit(&r, |db| {
            let f = db.add_folder("Synthetic Before");
            db.add_feed("https://fixture.invalid/rollback", "Synthetic", Some(f));
            Ok(())
        })
        .await;
        let before = layout_rows(&r).await;
        let snapshot = r.snapshot().await.unwrap();
        armed.store(true, Ordering::SeqCst);
        assert!(
            r.worker()
                .write(|c| {
                    let db = Db {
                        container: c.clone(),
                    };
                    db.rename_folder(daynews_db::folder_id("Synthetic Before"), "Synthetic After")?;
                    db.set_feed_folder(
                        daynews_db::feed_id("https://fixture.invalid/rollback"),
                        None,
                        None,
                    )?;
                    Ok(())
                })
                .await
                .is_err()
        );
        assert_eq!(before, layout_rows(&r).await);
        assert_eq!(snapshot, r.snapshot().await.unwrap());
        armed.store(false, Ordering::SeqCst);
        edit(&r, |db| {
            db.rename_folder(
                daynews_db::folder_id("Synthetic Before"),
                "Synthetic Retried",
            )?;
            Ok(())
        })
        .await;
        assert!(
            layout_rows(&r)
                .await
                .iter()
                .any(|row| row.get(1) == Some(&Value::Text("Synthetic Retried".into())))
        );
    });
}

#[test]
#[ignore = "requires a private consistent backup; no network or source writes"]
fn personal_collection_upgrade_preserves_existing_projection() {
    block_on(async {
        let source = std::env::var("DAY_NEWS_COLLECTION_BACKUP").unwrap();
        let source = std::path::Path::new(&source);
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("upgrade.db");
        std::fs::copy(source.join("library.db"), &path).unwrap();
        let identity: serde_json::Value = serde_json::from_slice(
            &std::fs::read(source.join("experimental-webdav.json")).unwrap(),
        )
        .unwrap();
        let identity: Identity = serde_json::from_value(identity["identity"].clone()).unwrap();
        let c = rusqlite::Connection::open(&path).unwrap();
        let before: Vec<(i64, String, f64, String)> = c
            .prepare("SELECT id,name,position,sort FROM folders ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let members: Vec<(i64, Option<i64>, f64, f64)> = c
            .prepare("SELECT id,folder,collection_position,position FROM feeds ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        drop(c);
        let r = webdav::open(&path, Arc::new(identity), Arc::new(|_| Ok(())))
            .await
            .unwrap();
        assert!(r.snapshot().await.unwrap().pending_uploads > 0);
        r.close().await.unwrap();
        let c = rusqlite::Connection::open(&path).unwrap();
        let after: Vec<(i64, String, f64, String)> = c
            .prepare("SELECT id,name,position,sort FROM folders ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let after_members: Vec<(i64, Option<i64>, f64, f64)> = c
            .prepare("SELECT id,folder,collection_position,position FROM feeds ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(before == after, "private collection projection changed");
        assert!(members == after_members, "private membership/order changed");
        assert_eq!(
            c.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
    });
}

#[test]
fn marking_old_articles_respects_strict_cutoff_and_syncs_read_choices() {
    block_on(async {
        let d = tempfile::tempdir().unwrap();
        let rs = replicas(d.path(), 2, None).await;
        for r in &rs {
            edit(r, |db| {
                let id = db.add_feed("https://fixture.invalid/cutoff", "Synthetic cutoff", None);
                let items = [
                    ("older", 99),
                    ("already-read", 98),
                    ("at-cutoff", 100),
                    ("newer", 101),
                ]
                .map(|(guid, time)| IncomingArticle {
                    published: Some(time),
                    ..incoming(guid)
                });
                db.try_upsert_articles(id, "https://fixture.invalid/cutoff", &items)?;
                db.set_read(
                    daynews_db::article_id("https://fixture.invalid/cutoff", "already-read"),
                    true,
                )?;
                db.set_starred(
                    daynews_db::article_id("https://fixture.invalid/cutoff", "older"),
                    true,
                );
                Ok(())
            })
            .await;
        }
        exchange(&rs).await;
        assert_eq!(
            rs[0]
                .worker()
                .write(|c| Db {
                    container: c.clone()
                }
                .set_read_before(100))
                .await
                .unwrap(),
            1
        );
        exchange(&rs).await;
        for r in &rs {
            r.worker()
                .read(|c| {
                    use daynews_db::ArticleFields;
                    for (guid, read) in [
                        ("older", true),
                        ("already-read", true),
                        ("at-cutoff", false),
                        ("newer", false),
                    ] {
                        let row = c
                            .get::<daynews_db::Article>(daynews_db::article_id(
                                "https://fixture.invalid/cutoff",
                                guid,
                            ))
                            .unwrap();
                        assert_eq!(row.is_read().peek(), read);
                    }
                    assert!(
                        c.get::<daynews_db::Article>(daynews_db::article_id(
                            "https://fixture.invalid/cutoff",
                            "older"
                        ))
                        .unwrap()
                        .is_starred()
                        .peek()
                    );
                    Ok(())
                })
                .await
                .unwrap();
        }
        let before = rs[0].snapshot().await.unwrap().transactions;
        assert_eq!(
            rs[0]
                .worker()
                .write(|c| Db {
                    container: c.clone()
                }
                .set_read_before(100))
                .await
                .unwrap(),
            0
        );
        assert_eq!(before, rs[0].snapshot().await.unwrap().transactions);
    });
}

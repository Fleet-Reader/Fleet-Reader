// SPDX-License-Identifier: Apache-2.0
use day_part_http::{
    Client,
    simulation::{Reply, Simulation},
};
use daynews_sync::{feedbin::Feedbin, fixture::Server, *};
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
fn wait<T>(f: impl Future<Output = T>) -> T {
    let w = Waker::from(Arc::new(Thread(std::thread::current())));
    let mut f = std::pin::pin!(f);
    loop {
        match f.as_mut().poll(&mut Context::from_waker(&w)) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::park(),
        }
    }
}
fn provider(sim: Simulation) -> Feedbin {
    Feedbin::with_client(
        Credentials {
            username: "synthetic".into(),
            password: "synthetic".into(),
        },
        Client::builder()
            .transport(sim)
            .redirects(day_part_http::Redirects::Never)
            .build(),
    )
}
#[test]
fn unauthorized_credentials_and_rate_limit_are_classified_without_response_details() {
    let s = Server::new();
    assert_eq!(
        wait(provider(s.simulation).authenticate()),
        Err(Error::Authentication)
    );
    let sim = Simulation::new();
    sim.handle(
        |_, _| true,
        |_| Ok(Reply::new(429, b"private response content".to_vec()).header("Retry-After", "123")),
    );
    let error = wait(provider(sim).authenticate()).unwrap_err();
    assert_eq!(error, Error::Retry { after_seconds: 123 });
    assert!(!format!("{error}").contains("private"));
}
#[test]
fn redirect_does_not_forward_credentials_to_another_origin() {
    let sim = Simulation::new();
    sim.handle(
        |_, u| u.starts_with("https://api.feedbin.com/"),
        |_| Ok(Reply::new(302, vec![]).header("Location", "https://other.invalid/")),
    );
    assert_eq!(
        wait(provider(sim.clone()).authenticate()),
        Err(Error::Authentication)
    );
    assert_eq!(sim.requests().len(), 1);
}
#[test]
fn malformed_unread_snapshot_is_never_interpreted_as_everything_read() {
    let sim = Simulation::new();
    sim.handle(|_, _| true, |_| Ok(Reply::new(200, b"[1,".to_vec())));
    assert!(matches!(
        wait(provider(sim).read_state()),
        Err(Error::InvalidResponse)
    ));
}
#[test]
fn bulk_read_upload_is_batched_and_acknowledged_per_id() {
    let s = Server::new();
    let p = s.provider();
    let operations = (1..=2001)
        .map(|id| Operation {
            revision: id,
            mutation: Mutation::SetRead(ReadState {
                id: id.to_string(),
                read: true,
            }),
        })
        .collect();
    let outcomes = wait(p.apply(operations)).unwrap();
    assert_eq!(outcomes.len(), 2001);
    assert!(
        outcomes
            .iter()
            .all(|o| matches!(o, Outcome::Applied { .. }))
    );
    assert_eq!(
        s.simulation.request_count(
            "POST",
            "https://api.feedbin.com/v2/unread_entries/delete.json"
        ),
        3
    );
}
#[test]
fn cursor_is_not_a_url_or_an_injection_surface() {
    let s = Server::new();
    let p = s.provider();
    assert!(matches!(
        wait(p.entries(Some("https://other.invalid".into()))),
        Err(Error::InvalidResponse)
    ));
    assert!(s.simulation.requests().is_empty());
}

#[test]
fn state_requests_bypass_platform_caches_after_a_mutation() {
    let sim = Simulation::new();
    sim.handle(
        |_, _| true,
        |request| {
            assert!(matches!(request.cache, day_part_http::CachePolicy::Reload));
            assert!(
                request
                    .headers
                    .iter()
                    .any(|(k, v)| k.eq_ignore_ascii_case("cache-control") && v == "no-cache")
            );
            Ok(Reply::new(200, b"[]".to_vec()))
        },
    );
    assert!(matches!(wait(provider(sim).subscriptions()),Ok(Changes::Snapshot(v)) if v.is_empty()));
}

#[test]
fn article_progress_uses_last_page_metadata_without_following_foreign_urls() {
    let sim = Simulation::new();
    sim.handle(|_, _| true, |_| Ok(Reply::new(200, b"[]".to_vec())
        .header("Link", "<https://other.invalid/?page=102>; rel=\"last\", <https://other.invalid/?page=2>; rel=\"next\"")));
    let page = wait(provider(sim.clone()).entries(None)).unwrap();
    assert_eq!(page.total_pages, Some(102));
    assert_eq!(page.next.as_deref(), Some("2"));
    assert_eq!(sim.requests().len(), 1);
    assert!(
        sim.requests()[0]
            .url
            .starts_with("https://api.feedbin.com/")
    );
}

#[test]
fn article_progress_stays_indeterminate_without_a_valid_last_page() {
    let sim = Simulation::new();
    sim.handle(|_, _| true, |_| Ok(Reply::new(200, b"[]".to_vec())
        .header("Link", "<https://api.feedbin.com/v2/entries.json?page=oops>; rel=\"last\", <https://api.feedbin.com/v2/entries.json?page=2>; rel=\"next\"")));
    assert_eq!(wait(provider(sim).entries(None)).unwrap().total_pages, None);
}

#[test]
fn aborting_during_a_delayed_http_response_does_not_wait_for_the_network_timeout() {
    use day_part_http::simulation::{Conditions, ManualClock};
    use futures_util::future::{AbortHandle, Abortable};
    let clock = ManualClock::default();
    let sim = Simulation::with_clock(Arc::new(clock.clone()));
    sim.set_conditions(Conditions {
        latency: std::time::Duration::from_secs(30),
        ..Default::default()
    });
    sim.handle(|_, _| true, |_| Ok(Reply::new(200, vec![])));
    let p = provider(sim.clone());
    let (abort, registration) = AbortHandle::new_pair();
    let mut future = Box::pin(Abortable::new(p.authenticate(), registration));
    let waker = Waker::from(Arc::new(Thread(std::thread::current())));
    assert!(
        future
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending()
    );
    abort.abort();
    assert!(matches!(
        future.as_mut().poll(&mut Context::from_waker(&waker)),
        Poll::Ready(Err(_))
    ));
    drop(future);
    clock.advance(std::time::Duration::from_secs(60));
    assert_eq!(sim.requests().len(), 1);
}

fn op(revision: u64, mutation: Mutation) -> Operation {
    Operation { revision, mutation }
}
#[test]
fn folder_writes_follow_the_tagging_api() {
    let s = Server::new();
    let p = s.provider();
    s.tag(11, "A");
    s.tag(11, "B");
    s.tag(12, "B");
    let outcomes = wait(p.apply(vec![op(
        1,
        Mutation::SetFolder {
            feed: "11".into(),
            folder: Some("C".into()),
        },
    )]))
    .unwrap();
    assert!(matches!(
        outcomes[..],
        [Outcome::Applied { revision: 1, .. }]
    ));
    assert_eq!(s.taggings(), [(12, "B".into()), (11, "C".into())]);
    // Already filed there: only the other taggings go, with no new POST.
    s.tag(11, "D");
    let posts = s
        .simulation
        .request_count("POST", "https://api.feedbin.com/v2/taggings.json");
    wait(p.apply(vec![op(
        2,
        Mutation::SetFolder {
            feed: "11".into(),
            folder: Some("C".into()),
        },
    )]))
    .unwrap();
    assert_eq!(
        s.simulation
            .request_count("POST", "https://api.feedbin.com/v2/taggings.json"),
        posts
    );
    assert_eq!(s.taggings(), [(12, "B".into()), (11, "C".into())]);
    let outcomes = wait(p.apply(vec![
        op(
            3,
            Mutation::RenameFolder {
                old: "B".into(),
                new: "E".into(),
            },
        ),
        op(
            4,
            Mutation::SetFolder {
                feed: "11".into(),
                folder: None,
            },
        ),
    ]))
    .unwrap();
    assert!(
        outcomes
            .iter()
            .all(|o| matches!(o, Outcome::Applied { .. }))
    );
    assert_eq!(s.taggings(), [(12, "E".into())]);
    wait(p.apply(vec![op(5, Mutation::DeleteFolder { name: "E".into() })])).unwrap();
    assert!(s.taggings().is_empty());
    assert!(matches!(wait(p.subscriptions()), Ok(Changes::Snapshot(v)) if v.len() == 1));
    let deletes = s
        .simulation
        .requests()
        .iter()
        .filter(|r| r.method == "DELETE" && r.url == "https://api.feedbin.com/v2/tags.json")
        .count();
    assert_eq!(deletes, 1);
}
#[test]
fn folder_edits_on_tags_the_service_lacks_send_no_writes() {
    let s = Server::new();
    let p = s.provider();
    let outcomes = wait(p.apply(vec![
        op(
            1,
            Mutation::RenameFolder {
                old: "Local".into(),
                new: "Other".into(),
            },
        ),
        op(
            2,
            Mutation::DeleteFolder {
                name: "Other".into(),
            },
        ),
        op(
            3,
            Mutation::SetFolder {
                feed: "11".into(),
                folder: None,
            },
        ),
    ]))
    .unwrap();
    assert!(
        outcomes
            .iter()
            .all(|o| matches!(o, Outcome::Applied { .. }))
    );
    assert!(s.simulation.requests().iter().all(|r| r.method == "GET"));
    // One listing serves the whole batch while nothing changes it.
    assert_eq!(s.simulation.requests().len(), 1);
}
#[test]
fn non_numeric_tagging_feed_is_refused_before_any_request() {
    let s = Server::new();
    let p = s.provider();
    s.tag(11, "A");
    let outcomes = wait(p.apply(vec![op(
        1,
        Mutation::SetFolder {
            feed: "../11".into(),
            folder: None,
        },
    )]))
    .unwrap();
    assert!(matches!(
        outcomes[..],
        [Outcome::Failed {
            error: Error::InvalidResponse,
            ..
        }]
    ));
    assert_eq!(s.taggings(), [(11, "A".into())]);
    assert!(s.simulation.requests().is_empty());
}
#[test]
fn taggings_download_as_feed_and_name_pairs() {
    let s = Server::new();
    s.tag(11, "News");
    let taggings = wait(s.provider().taggings()).unwrap().unwrap();
    assert_eq!(
        taggings,
        [Tagging {
            feed_id: "11".into(),
            name: "News".into(),
        }]
    );
}
#[test]
fn journal_mutations_from_an_older_build_still_decode() {
    for json in [
        r#"{"SetRead":{"id":"1","read":false}}"#,
        r#"{"Subscribe":{"url":"https://fixture.invalid/a"}}"#,
        r#"{"Unsubscribe":{"subscription":"7"}}"#,
        r#"{"Rename":{"subscription":"7","title":"T"}}"#,
    ] {
        let mutation: Mutation = serde_json::from_str(json).unwrap();
        assert_eq!(serde_json::to_string(&mutation).unwrap(), json);
    }
}

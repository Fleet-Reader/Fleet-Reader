// SPDX-License-Identifier: Apache-2.0
//! One isolated integration-test process exercises global interception of the ordinary fetch API.
use day_part_http::{
    Session,
    simulation::{Conditions, Reply, Simulation},
};
use daynews_feed::{CacheValidators, FeedUpdate, fetch_cached_with_progress};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
    time::Duration,
};
fn wait<T>(future: impl std::future::Future<Output = T>) -> T {
    struct Unpark(std::thread::Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::park_timeout(Duration::from_secs(5)),
        }
    }
}
#[test]
fn conditional_headers_parser_fingerprint_and_network_failures_use_normal_pipeline() {
    let simulation = Simulation::new();
    let count = Arc::new(AtomicUsize::new(0));
    let calls = count.clone();
    simulation.route("GET","https://fixture.example/feed",move|request|{
  let round=calls.fetch_add(1,Ordering::SeqCst);
  if round>0{assert!(request.headers.iter().any(|(k,v)|k.eq_ignore_ascii_case("if-none-match")&&v=="\"one\""));}
  if round==2{return Ok(Reply::new(304,vec![]).header("ETag","\"one\""));}
  Ok(Reply::new(200,br#"{"version":"https://jsonfeed.org/version/1.1","title":"Synthetic feed","items":[{"id":"one","title":"Synthetic article","content_text":"Fixture"}]}"#.to_vec()).header("ETag","\"one\""))
 });
    let session = Session::global();
    session.set_simulation(Some(simulation.clone()));
    let mut validators = CacheValidators::default();
    let mut fingerprint = None;
    for round in 0..3 {
        let result = wait(fetch_cached_with_progress(
            "https://fixture.example/feed",
            &validators,
            fingerprint.as_deref(),
            |_, _| {},
        ))
        .unwrap();
        match result {
            FeedUpdate::Modified(_, v, f) => {
                assert_eq!(round, 0);
                validators = v;
                fingerprint = f;
            }
            FeedUpdate::NotModified(v) => {
                assert_ne!(round, 0);
                validators = v;
            }
        }
    }
    simulation.set_conditions(Conditions {
        failure_rate: 1.0,
        seed: 42,
        ..Default::default()
    });
    assert!(
        wait(fetch_cached_with_progress(
            "https://fixture.example/feed",
            &validators,
            fingerprint.as_deref(),
            |_, _| {}
        ))
        .is_err()
    );
    assert_eq!(count.load(Ordering::SeqCst), 3);
    assert_eq!(
        simulation.request_count("GET", "https://fixture.example/feed"),
        4
    );
    assert_eq!(session.statistics().failed, 1);
    assert!(session.statistics().downloaded_bytes > 0);
    session.set_simulation(None);
}

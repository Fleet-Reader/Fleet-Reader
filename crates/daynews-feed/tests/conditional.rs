// SPDX-License-Identifier: Apache-2.0
//! Synthetic loopback HTTP fixtures: no publisher traffic.
#![cfg(any(target_os = "macos", target_os = "linux"))]
use daynews_feed::{CacheValidators, FeedUpdate, fetch_cached_with_progress};
use std::{
    future::Future,
    io::{Read, Write},
    net::TcpListener,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
    time::Duration,
};

fn wait<T>(future: impl Future<Output = T>) -> T {
    struct Unpark(std::thread::Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::park_timeout(Duration::from_secs(35)),
        }
    }
}

#[test]
fn rotating_validators_on_identical_bodies_reach_the_next_request() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/feed", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let body = br#"{"version":"https://jsonfeed.org/version/1.1","title":"Synthetic fixture","items":[]}"#;
        for round in 0..4 {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut chunk = [0; 1024];
            while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                let read = stream.read(&mut chunk).unwrap();
                assert!(read > 0);
                request.extend_from_slice(&chunk[..read]);
            }
            let request = String::from_utf8(request).unwrap().to_ascii_lowercase();
            if round == 0 {
                assert!(!request.contains("if-none-match:"));
            } else {
                let tag = if round == 1 {
                    "one"
                } else if round == 2 {
                    "two"
                } else {
                    "three"
                };
                assert!(
                    request.contains(&format!("if-none-match: w/\"{tag}\"")),
                    "{request}"
                );
                assert!(
                    request.contains("if-modified-since: wed, 30 sep 2026 10:00:00 gmt"),
                    "{request}"
                );
            }
            let (status, tag, data) = match round {
                0 => ("200 OK", "one", body.as_slice()),
                1 => ("200 OK", "two", body.as_slice()),
                2 => ("304 Not Modified", "three", &[][..]),
                _ => ("304 Not Modified", "three", &[][..]),
            };
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nETag: W/\"{tag}\"\r\nLast-Modified: Wed, 30 Sep 2026 10:00:00 GMT\r\nConnection: close\r\n\r\n",
                data.len()
            );
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(data).unwrap();
        }
    });
    let mut validators = CacheValidators::default();
    let mut hash = None;
    for round in 0..4 {
        match wait(fetch_cached_with_progress(
            &url,
            &validators,
            hash.as_deref(),
            |_, _| {},
        ))
        .unwrap()
        {
            FeedUpdate::Modified(_, next, fingerprint) => {
                assert_eq!(round, 0, "identical responses must not import again");
                validators = next;
                hash = fingerprint;
            }
            FeedUpdate::NotModified(next) => {
                assert_ne!(round, 0);
                validators = next;
            }
        }
    }
    server.join().unwrap();
}

// SPDX-License-Identifier: Apache-2.0
//! Opt-in live trial test. Credentials are read from an owner-only external JSON file;
//! never pass them in command arguments or write them into fixtures.
use daynews_sync::{Changes, Credentials, Mutation, Operation, Provider, ReadChanges, ReadState};
use std::{
    future::Future,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
    time::Instant,
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
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("credential file path required")?;
    let secret: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let credentials = Credentials {
        username: secret["username"].as_str().ok_or("username")?.into(),
        password: secret["password"].as_str().ok_or("password")?.into(),
    };
    let provider = daynews_sync::feedbin::Feedbin::new(credentials);
    wait(async {
        let start = Instant::now();
        provider.authenticate().await?;
        println!(
            "Authentication succeeded ({} ms)",
            start.elapsed().as_millis()
        );
        if let Changes::Snapshot(rows) = provider.subscriptions().await? {
            println!("Subscriptions: {}", rows.len());
            if rows.is_empty() {
                let result = provider
                    .apply(vec![Operation {
                        revision: 1,
                        mutation: Mutation::Subscribe {
                            url: "https://blog.rust-lang.org/feed.xml".into(),
                        },
                    }])
                    .await?;
                println!("Sample subscription: {result:?}");
            }
        }
        let page = provider.entries(None).await?;
        println!(
            "First entry page: {} entries; more={}",
            page.values.len(),
            page.next.is_some()
        );
        let ReadChanges::UnreadSnapshot(unread) = provider.read_state().await? else {
            return Err(daynews_sync::Error::InvalidResponse);
        };
        println!("Unread entries: {}", unread.len());
        if let Some(entry) = page.values.first() {
            let original_read = !unread.contains(&entry.id);
            for (revision, read) in [
                (2, !original_read),
                (3, !original_read),
                (4, original_read),
                (5, original_read),
            ] {
                let results = provider
                    .apply(vec![Operation {
                        revision,
                        mutation: Mutation::SetRead(ReadState {
                            id: entry.id.clone(),
                            read,
                        }),
                    }])
                    .await?;
                if !matches!(results.first(), Some(daynews_sync::Outcome::Applied { .. })) {
                    return Err(daynews_sync::Error::Rejected);
                }
                let ReadChanges::UnreadSnapshot(ids) = provider.read_state().await? else {
                    return Err(daynews_sync::Error::InvalidResponse);
                };
                if ids.contains(&entry.id) == read {
                    return Err(daynews_sync::Error::InvalidResponse);
                }
                println!("Verified entry {} read={read}", entry.id);
            }
        }
        let conditional = provider.entries(None).await?;
        println!(
            "Conditional entry check: {} bodies downloaded",
            conditional.values.len()
        );
        let Changes::Snapshot(before) = provider.subscriptions().await? else {
            return Err(daynews_sync::Error::InvalidResponse);
        };
        if let Some(subscription) = before.first() {
            let changed = provider
                .apply(vec![Operation {
                    revision: 6,
                    mutation: Mutation::Rename {
                        subscription: subscription.id.clone(),
                        title: "Fleet Reader live verification".into(),
                    },
                }])
                .await;
            let observed = provider.subscriptions().await;
            // Restore even if observing the temporary title failed.
            let restored = provider
                .apply(vec![Operation {
                    revision: 7,
                    mutation: Mutation::Rename {
                        subscription: subscription.id.clone(),
                        title: subscription.title.clone(),
                    },
                }])
                .await?;
            if !matches!(
                changed?.first(),
                Some(daynews_sync::Outcome::Applied { .. })
            ) || !matches!(
                restored.first(),
                Some(daynews_sync::Outcome::Applied { .. })
            ) || !matches!(observed?,Changes::Snapshot(rows) if rows.iter().any(|s|s.id==subscription.id && s.title=="Fleet Reader live verification"))
            {
                return Err(daynews_sync::Error::Rejected);
            }
            println!("Verified subscription rename and restored original title");
        }
        let added = provider
            .apply(vec![Operation {
                revision: 8,
                mutation: Mutation::Subscribe {
                    url: "https://www.swift.org/atom.xml".into(),
                },
            }])
            .await?;
        let Some(daynews_sync::Outcome::Applied {
            subscription: Some(temporary),
            ..
        }) = added.first()
        else {
            return Err(daynews_sync::Error::Rejected);
        };
        // A 302 can identify a subscription already owned by the account. Never delete it.
        if !before.iter().any(|s| s.id == temporary.id) {
            let removed = provider
                .apply(vec![Operation {
                    revision: 9,
                    mutation: Mutation::Unsubscribe {
                        subscription: temporary.id.clone(),
                    },
                }])
                .await?;
            if !matches!(removed.first(), Some(daynews_sync::Outcome::Applied { .. })) {
                return Err(daynews_sync::Error::Rejected);
            }
            if !matches!(provider.subscriptions().await?,Changes::Snapshot(rows) if !rows.iter().any(|s|s.id==temporary.id))
            {
                return Err(daynews_sync::Error::InvalidResponse);
            }
            println!("Verified temporary subscription creation and removal");
        }
        println!(
            "Live provider validation finished ({} ms)",
            start.elapsed().as_millis()
        );
        Ok::<_, daynews_sync::Error>(())
    })?;
    Ok(())
}

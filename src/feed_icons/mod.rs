// SPDX-License-Identifier: Apache-2.0
//! App-wide discovery queue. Four downloads at a time; native disk/decoding work stays off UI.
mod cache;
mod discovery;
use day::prelude::*;
use discovery::{Source, web_url};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
    rc::Rc,
    time::Duration,
};

struct State {
    paths: Signal<HashMap<u64, String>>,
    sources: RefCell<HashMap<u64, Source>>,
    queue: RefCell<VecDeque<(u64, Source)>>,
    active: Cell<usize>,
    pending: RefCell<HashMap<u64, (Source, String)>>,
    flush_scheduled: Cell<bool>,
    client: day_part_http::Client,
}
#[derive(Clone)]
struct Icons(Rc<State>);
impl Ambient for Icons {
    fn create() -> Self {
        Self(Rc::new(State {
            paths: Signal::new(HashMap::new()),
            sources: RefCell::new(HashMap::new()),
            queue: RefCell::new(VecDeque::new()),
            active: Cell::new(0),
            pending: RefCell::new(HashMap::new()),
            flush_scheduled: Cell::new(false),
            client: day_part_http::Client::builder()
                .cookies(day_part_http::Cookies::Off)
                .cache(day_part_http::Cache::Off)
                .timeout_total(Duration::from_secs(8))
                .build(),
        }))
    }
}

pub fn paths() -> Signal<HashMap<u64, String>> {
    Icons::app().0.paths
}

pub fn init() {
    // Browser builds have no local file cache to present through native sidebar image paths.
    if cfg!(target_arch = "wasm32") {
        return;
    }
    let icons = Icons::app();
    let feeds = daynews_core::state().feeds;
    Effect::new(move || {
        let feeds = feeds.get();
        let mut known = icons.0.sources.borrow_mut();
        known.retain(|id, _| feeds.iter().any(|f| f.id == *id));
        let mut added = Vec::new();
        for feed in &feeds {
            if web_url(&feed.feed_url).is_none()
                || (feed.last_fetched.is_none()
                    && feed.site_url.is_none()
                    && feed.icon_url.is_none())
            {
                continue;
            }
            let source = Source {
                feed: feed.feed_url.clone(),
                home: feed.site_url.clone(),
                icon: feed.icon_url.clone(),
            };
            if known.get(&feed.id) != Some(&source) {
                known.insert(feed.id, source.clone());
                added.push((feed.id, source));
            }
        }
        drop(known);
        if !added.is_empty() {
            let state = icons.0.clone();
            // Read every cached thumbnail before starting discovery: one slow site must not
            // delay the other sites' already downloaded icons at launch.
            day::task(async move {
                let entries = work(move || {
                    let root = cache::root();
                    let now = daynews_time::now_unix();
                    added
                        .into_iter()
                        .map(|(id, source)| {
                            let cached = cache::read(&root, &source, now);
                            (id, source, cached)
                        })
                        .collect::<Vec<_>>()
                })
                .await
                .unwrap_or_default();
                for (id, source, cached) in entries {
                    if state.sources.borrow().get(&id) != Some(&source) {
                        continue;
                    }
                    publish(state.clone(), id, &source, cached.path);
                    if !cached.fresh {
                        state.queue.borrow_mut().push_back((id, source));
                    }
                }
                schedule(state);
            });
        }
    });
}

fn publish(state: Rc<State>, id: u64, source: &Source, path: Option<String>) {
    if state.sources.borrow().get(&id) != Some(source) {
        return;
    }
    let Some(path) = path else { return };
    state
        .pending
        .borrow_mut()
        .insert(id, (source.clone(), path));
    if !state.flush_scheduled.replace(true) {
        day::task(async move {
            day::sleep(120).await;
            let pending = std::mem::take(&mut *state.pending.borrow_mut());
            let mut paths = state.paths.get_untracked();
            paths.extend(pending.into_iter().filter_map(|(id, (source, path))| {
                (state.sources.borrow().get(&id) == Some(&source)).then_some((id, path))
            }));
            paths.retain(|id, _| state.sources.borrow().contains_key(id));
            if paths != state.paths.get_untracked() {
                state.paths.set(paths);
            }
            state.flush_scheduled.set(false);
        });
    }
}

fn schedule(state: Rc<State>) {
    while state.active.get() < 4 {
        let Some((id, source)) = state.queue.borrow_mut().pop_front() else {
            break;
        };
        if state.sources.borrow().get(&id) != Some(&source) {
            continue;
        }
        state.active.set(state.active.get() + 1);
        let state = state.clone();
        day::task(async move {
            let root = cache::root();
            let now = daynews_time::now_unix();
            let png = match futures_util::future::select(
                Box::pin(discover(&state.client, &source)),
                Box::pin(day::sleep(25_000)),
            )
            .await
            {
                futures_util::future::Either::Left((png, _)) => png,
                futures_util::future::Either::Right(_) => None,
            };
            let s = source.clone();
            let path = work(move || cache::write(&root, &s, png, now))
                .await
                .flatten();
            publish(state.clone(), id, &source, path);
            state.active.set(state.active.get() - 1);
            schedule(state);
        });
    }
}

// Four queue workers bound the thread count too. Cancellation/disposal drops results safely.
async fn work<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (tx, rx) = day_async::oneshot();
        std::thread::Builder::new()
            .name("feed-icon".into())
            .spawn(move || tx.send(f()))
            .ok()?;
        rx.await.ok()
    }
    #[cfg(target_arch = "wasm32")]
    {
        Some(f())
    }
}

async fn fetch(
    client: &day_part_http::Client,
    url: &url::Url,
    max: usize,
) -> Option<(url::Url, Vec<u8>)> {
    let response = client
        .fetch_limited_future(
            day_part_http::Request::get(url.as_str())
                .header(
                    "User-Agent",
                    concat!("FleetReader/", env!("CARGO_PKG_VERSION")),
                )
                .timeout(Duration::from_secs(8)),
            max,
        )
        .await
        .ok()?;
    if !(200..300).contains(&response.status) {
        return None;
    }
    let final_url = web_url(&response.url).unwrap_or_else(|| url.clone());
    Some((final_url, response.body))
}

async fn icon(
    client: &day_part_http::Client,
    url: url::Url,
    attempted: &mut Vec<url::Url>,
) -> Option<Vec<u8>> {
    if attempted.contains(&url) || attempted.len() >= 12 {
        return None;
    }
    attempted.push(url.clone());
    let (_, bytes) = fetch(client, &url, 2 * 1024 * 1024).await?;
    work(move || cache::thumbnail(&bytes)).await.flatten()
}

async fn discover(client: &day_part_http::Client, source: &Source) -> Option<Vec<u8>> {
    let mut attempted = Vec::new();
    if let Some(url) = source.declared_icon()
        && let Some(png) = icon(client, url, &mut attempted).await
    {
        return Some(png);
    }
    for page in source.pages() {
        let mut base = page.clone();
        if let Some((final_url, bytes)) = fetch(client, &page, 1024 * 1024).await {
            base = final_url.clone();
            let links =
                work(move || discovery::page_icons(&String::from_utf8_lossy(&bytes), &final_url))
                    .await?;
            for url in links.icons {
                if let Some(png) = icon(client, url, &mut attempted).await {
                    return Some(png);
                }
            }
            if let Some(manifest) = links.manifest
                && let Some((url, bytes)) = fetch(client, &manifest, 128 * 1024).await
            {
                for url in discovery::manifest_icons(&bytes, &url) {
                    if let Some(png) = icon(client, url, &mut attempted).await {
                        return Some(png);
                    }
                }
            }
        }
        for path in ["/favicon.ico", "/apple-touch-icon.png"] {
            if let Some(url) = discovery::resolve(&base, path)
                && let Some(png) = icon(client, url, &mut attempted).await
            {
                return Some(png);
            }
        }
    }
    None
}

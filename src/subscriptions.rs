//! Managing subscriptions: add by URL, import/export OPML, and per-feed actions.

use day::prelude::*;

fn subscription_url(input: &str) -> Option<String> {
    let input = input.trim();
    // A bare hostname with a port can parse as an opaque URL scheme. Preserve that input
    // convenience while rejecting actual non-web protocols and embedded credentials.
    let bare_port = input.split_once(':').is_some_and(|(_, rest)| {
        rest.split('/')
            .next()
            .is_some_and(|port| port.parse::<u16>().is_ok())
    });
    if url::Url::parse(input).is_ok()
        && daynews_feed::discovery::web_url(input).is_none()
        && !bare_port
    {
        return None;
    }
    daynews_feed::discovery::web_url(&daynews_core::normalize_feed_url(input))
        .map(|u| u.to_string())
}

/// Window-owned subscription sheet shared by File > New Feed and URL activation.
#[derive(Clone, Copy)]
pub struct SubscriptionSheet {
    open: Signal<Option<String>>,
}
impl Ambient for SubscriptionSheet {
    fn create() -> Self {
        Self {
            open: Signal::new(None),
        }
    }
}
pub fn begin_new_feed() {
    if daynews_core::try_scene().is_none() {
        return;
    }
    let Some(sheet) = SubscriptionSheet::try_ambient().or_else(SubscriptionSheet::focused) else {
        return;
    };
    let clipboard = day::clipboard::read(&["text/plain", "text/uri-list"]);
    day::task(async move {
        // A clipboard suggestion is optional. A slow provider or permission prompt must
        // not keep the subscription form closed, and late data must not replace typed input.
        let initial = clipboard_feed_url(clipboard, day::sleep(200)).await;
        sheet.open.set(Some(initial));
    });
}

async fn clipboard_feed_url(
    clipboard: day::clipboard::ClipboardFuture<Option<day::clipboard::Representation>>,
    deadline: impl std::future::Future<Output = ()>,
) -> String {
    use futures_util::future::{Either, select};
    futures_util::pin_mut!(deadline);
    let result = match select(clipboard, deadline).await {
        Either::Left((result, _)) => result.ok().flatten(),
        Either::Right(_) => None,
    };
    result
        .and_then(|r| String::from_utf8(r.bytes.as_ref().clone()).ok())
        .and_then(|s| {
            incoming_url(&s)
                .or_else(|| daynews_feed::discovery::web_url(s.trim()).map(|u| u.to_string()))
        })
        .unwrap_or_default()
}
fn is_feed_scheme(input: &str) -> bool {
    input.split_once(':').is_some_and(|(scheme, _)| {
        ["feed", "feeds", "rss", "atom", "web+feed"]
            .iter()
            .any(|s| s.eq_ignore_ascii_case(scheme))
    })
}
fn incoming_url(input: &str) -> Option<String> {
    let (scheme, rest) = input.split_once(':')?;
    if !["feed", "feeds", "rss", "atom", "web+feed"]
        .iter()
        .any(|s| scheme.eq_ignore_ascii_case(s))
    {
        return None;
    }
    let candidate = if rest.starts_with("//") {
        format!(
            "{}:{rest}",
            if scheme.eq_ignore_ascii_case("feeds") {
                "https"
            } else {
                "http"
            }
        )
    } else {
        rest.to_owned()
    };
    subscription_url(&candidate)
}
async fn verify_url(input: &str) -> Result<(String, daynews_feed::FeedUpdate), ()> {
    if input.starts_with("asset:") {
        let feed = daynews_core::preview_feed(input).await.map_err(|_| ())?;
        return Ok((
            input.into(),
            daynews_feed::FeedUpdate::Modified(feed, Default::default(), None),
        ));
    }
    let normalized = incoming_url(input)
        .or_else(|| subscription_url(input))
        .ok_or(())?;
    let discovered = daynews_feed::discovery::discover(&normalized)
        .await
        .map_err(|_| ())?;
    match discovered {
        daynews_feed::discovery::Discovery::Feed { url, update } => Ok((url, *update)),
        daynews_feed::discovery::Discovery::Candidates(options) => {
            let url = if options.len() == 1 {
                options[0].url.clone()
            } else {
                if options.is_empty() {
                    return Err(());
                }
                let mut dialog = Alert::new(crate::res::str::subscribe_choose())
                    .message(crate::res::str::subscribe_choose_note());
                for option in options {
                    let label = option
                        .title
                        .as_deref()
                        .map(|title| {
                            crate::res::str::subscribe_candidate(
                                crate::res::str::SubscribeCandidate {
                                    title,
                                    url: option.url.as_str(),
                                },
                            )
                            .format()
                        })
                        .unwrap_or_else(|| option.url.clone());
                    dialog = dialog.button(label, option.url);
                }
                dialog
                    .cancel(crate::res::str::cancel_action())
                    .await
                    .ok_or(())?
            };
            match daynews_feed::discovery::discover(&url)
                .await
                .map_err(|_| ())?
            {
                daynews_feed::discovery::Discovery::Feed { url, update } => Ok((url, *update)),
                _ => Err(()),
            }
        }
    }
}
pub fn subscription_sheet() -> impl Piece {
    let sheet = SubscriptionSheet::ambient();
    day::on_open_url(move |url| {
        if let Some(url) = incoming_url(url)
            .or_else(|| daynews_feed::discovery::web_url(url).map(|u| u.to_string()))
        {
            sheet.open.set(Some(url));
            true
        } else if is_feed_scheme(url) {
            sheet.open.set(Some(url.to_owned()));
            true
        } else {
            false
        }
    });
    day::on_open_files(move |files| {
        if let Some(locator) = files.into_iter().next() {
            day::task(async move {
                // Downloaded feeds usually omit their source URL. Discover from their site;
                // temporary file paths must never become refreshable subscriptions.
                let site = FileUrl::new(locator.clone())
                    .read_limited(16 * 1024 * 1024)
                    .await
                    .ok()
                    .and_then(|bytes| daynews_feed::parse(&bytes, &locator).ok())
                    .and_then(|feed| {
                        feed.source_url
                            .and_then(|s| daynews_feed::discovery::web_url(&s))
                            .or_else(|| {
                                feed.site_url
                                    .and_then(|s| daynews_feed::discovery::web_url(&s))
                            })
                            .map(|u| u.to_string())
                    });
                sheet.open.set(Some(site.unwrap_or_default()));
            });
        }
    });
    cover(sheet.open, move |initial: &String| {
        let sheet_scope = day::reactive::Scope::current();
        let address = Signal::new(initial.clone());
        let busy = Signal::new(false);
        // Only Verify's fetch says it is checking; Subscribe's question has its own dialog.
        let verifying = Signal::new(false);
        let status = Signal::new(String::new());
        let articles = Signal::new(Vec::<daynews_feed::ParsedItem>::new());
        let title = Signal::new(String::new());
        let verified_input = Signal::new(None::<String>);
        let verified = std::rc::Rc::new(std::cell::RefCell::new(
            None::<(String, daynews_feed::FeedUpdate)>,
        ));
        let verify_result = verified.clone();
        let subscribe_result = verified.clone();
        column((
            // The sheet's actions sit beside its title, where a phone's keyboard can't cover them.
            row((
                button(crate::res::str::cancel_action())
                    .action(move || sheet.open.set(None))
                    .id(day::id!(feed_cancel)),
                spacer().grow(Dimension::Width),
                label(crate::res::str::new_feed_title())
                    .font(Font::Headline)
                    .id(day::id!(subscription_title)),
                spacer().grow(Dimension::Width),
                button(crate::res::str::subscribe_action())
                    .enabled(move || {
                        !busy.get() && verified_input.get().as_deref() == Some(address.get().trim())
                    })
                    .action(move || {
                        let Some((url, update)) = subscribe_result.borrow_mut().take() else {
                            return;
                        };
                        busy.set(true);
                        let unchosen = subscribe_result.clone();
                        day::task(async move {
                            match already_subscribed(&url, &update).await {
                                Proceed::Subscribe => {}
                                Proceed::Show(feed) => {
                                    select_subscribed_feed(feed).await;
                                    if sheet_scope.is_alive() {
                                        busy.set(false);
                                    }
                                    sheet.open.set(None);
                                    return;
                                }
                                // The preview stays, ready for Subscribe again.
                                Proceed::Cancel => {
                                    *unchosen.borrow_mut() = Some((url, update));
                                    busy.set(false);
                                    return;
                                }
                            }
                            match daynews_core::subscribe_discovered(url, update).await {
                                Ok(feed) => {
                                    // Keep this task's scope alive until the feed projection
                                    // settles. Dismissing the cover first disposes its child
                                    // observer on DOM and can leave the old scope selected.
                                    select_subscribed_feed(feed).await;
                                    if sheet_scope.is_alive() {
                                        busy.set(false);
                                    }
                                    sheet.open.set(None);
                                    return;
                                }
                                Err(_) => {
                                    verified_input.set(None);
                                    status.set(crate::res::str::subscribe_failed().format());
                                }
                            }
                            busy.set(false);
                        });
                    })
                    .id(day::id!(feed_subscribe)),
            ))
            .spacing(Space::S),
            label(crate::res::str::subscribe_preview_note()).font(Font::Footnote),
            row((
                text_field(address)
                    .input_purpose(InputPurpose::Url)
                    .placeholder(crate::res::str::subscribe_placeholder())
                    .id(day::id!(feed_url))
                    .grow(Dimension::Width),
                button(crate::res::str::subscribe_verify())
                    .enabled(move || !busy.get() && !address.get().trim().is_empty())
                    .action(move || {
                        busy.set(true);
                        verifying.set(true);
                        status.set(String::new());
                        articles.set(Vec::new());
                        verified_input.set(None);
                        verify_result.borrow_mut().take();
                        let input = address.get_untracked().trim().to_owned();
                        let result = verify_result.clone();
                        let task = day::task(async move {
                            match verify_url(&input).await {
                                Ok((url, update)) if address.get_untracked().trim() == input => {
                                    if let daynews_feed::FeedUpdate::Modified(feed, _, _) = &update
                                    {
                                        title
                                            .set(feed.title.clone().unwrap_or_else(|| url.clone()));
                                        articles.set(feed.items.clone());
                                    }
                                    *result.borrow_mut() = Some((url, update));
                                    verified_input.set(Some(input));
                                    status.set(
                                        crate::res::str::subscribe_preview_ready(
                                            articles.with_untracked(|a| a.len()) as f64,
                                        )
                                        .format(),
                                    );
                                }
                                Ok(_) => (),
                                Err(()) => status.set(crate::res::str::subscribe_failed().format()),
                            }
                            busy.set(false);
                            verifying.set(false);
                        });
                        sheet_scope.on_cleanup(move || task.abort());
                    })
                    .id(day::id!(feed_verify)),
            ))
            .spacing(Space::S),
            when(
                move || verifying.get(),
                || label(crate::res::str::subscribe_verifying()).id(day::id!(feed_verifying)),
            ),
            label(move || status.get())
                .font(Font::Footnote)
                .id(day::id!(feed_verification_status)),
            label(move || title.get())
                .font(Font::Headline)
                .id(day::id!(feed_preview_title)),
            list(items(move || articles.get(), |a| a.guid.clone()), |slot| {
                column((
                    label(move || slot.with(|a| a.display_title()))
                        .font(Font::Headline)
                        .max_lines(2),
                    label(move || {
                        slot.with(|a| {
                            a.summary
                                .as_deref()
                                .map(daynews_feed::clean)
                                .unwrap_or_default()
                        })
                    })
                    .font(Font::Footnote)
                    .max_lines(2),
                ))
                .align(HAlign::Leading)
                .padding(Space::S)
            })
            .row_height(RowHeight::Uniform(100.0))
            .id(day::id!(feed_preview))
            .grow(Dimension::Both),
        ))
        .spacing(Space::M)
        .align(HAlign::Leading)
        .padding(Space::L)
        .grow(Dimension::Both)
        .id(day::id!(subscription_sheet))
    })
    .sheet()
    .unrouted()
}

/// What to do with a verified feed that may already be in the library.
enum Proceed {
    Subscribe,
    Show(u64),
    Cancel,
}

/// The guard against subscribing to one feed twice. A feed at an address already subscribed
/// (`http` or `https`, with or without `www.` and a trailing slash) is the same feed: the choice
/// is to show it. A feed whose stories mostly arrive already through another subscription is
/// the same feed under a second address (a site's two links to its own feed, or one feed
/// through a sync service and directly): subscribing again asks first. Several feeds of one
/// site, each with stories of its own, pass without a question.
async fn already_subscribed(url: &str, update: &daynews_feed::FeedUpdate) -> Proceed {
    let titled = |feed: u64| {
        daynews_core::state()
            .feeds
            .with_untracked(|feeds| feeds.iter().find(|f| f.id == feed).map(|f| f.title.clone()))
    };
    let key = address_key(url);
    let same_address = daynews_core::state().feeds.with_untracked(|feeds| {
        feeds
            .iter()
            .find(|f| address_key(&f.feed_url) == key)
            .map(|f| (f.id, f.title.clone()))
    });
    if let Some((feed, title)) = same_address {
        return match Alert::new(crate::res::str::subscribe_duplicate_title())
            .message(crate::res::str::subscribe_duplicate(title.as_str()))
            .button(crate::res::str::subscribe_show_existing(), feed)
            .cancel(crate::res::str::cancel_action())
            .await
        {
            Some(feed) => Proceed::Show(feed),
            None => Proceed::Cancel,
        };
    }
    let links: Vec<String> = match update {
        daynews_feed::FeedUpdate::Modified(feed, _, _) => {
            feed.items.iter().filter_map(|i| i.url.clone()).collect()
        }
        _ => Vec::new(),
    };
    let distinct = links.iter().collect::<std::collections::HashSet<_>>().len();
    let Some((feed, shared)) = daynews_core::feeds_with_links(links).await.first().copied() else {
        return Proceed::Subscribe;
    };
    let Some(title) = titled(feed) else {
        return Proceed::Subscribe;
    };
    if distinct == 0 || shared * 2 < distinct {
        return Proceed::Subscribe;
    }
    match Alert::new(crate::res::str::subscribe_same_feed_title())
        .message(crate::res::str::subscribe_same_feed(title.as_str()))
        .button(crate::res::str::subscribe_anyway(), None)
        .button(crate::res::str::subscribe_show_existing(), Some(feed))
        .cancel(crate::res::str::cancel_action())
        .await
    {
        Some(None) => Proceed::Subscribe,
        Some(Some(feed)) => Proceed::Show(feed),
        None => Proceed::Cancel,
    }
}

/// A feed address with what does not tell two feeds apart taken off: the scheme, a leading
/// `www.`, the host's case and a trailing slash.
fn address_key(url: &str) -> String {
    let url = url.trim();
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    format!("{host}/{}", path.trim_end_matches('/'))
}

/// Wait for the committed feed projection and its native sidebar bindings to settle.
/// This also makes an empty source visible when the unread-only filter was active.
async fn select_subscribed_feed(feed: u64) {
    let st = daynews_core::state();
    let scene = daynews_core::scene();
    scene.search.set(String::new());
    let (tx, rx) = day_async::oneshot();
    let sender = std::cell::RefCell::new(Some(tx));
    let ready = day::reactive::Scope::child();
    ready.enter(|| {
        day::reactive::batch(|| {
            day::reactive::Effect::new(move || {
                if let Some(unread) = st
                    .feeds
                    .with(|feeds| feeds.iter().find(|f| f.id == feed).map(|f| f.unread))
                {
                    if unread == 0
                        && crate::feed_list::FeedList::app()
                            .unread_only
                            .get_untracked()
                    {
                        crate::feed_list::toggle();
                    }
                    if let Some(tx) = sender.borrow_mut().take() {
                        day::reactive::at_turn_end(move || tx.send(()));
                    }
                }
            })
        })
    });
    let visible = rx.await.is_ok();
    ready.dispose();
    if visible {
        navigate(&format!("feed:{feed}"));
        daynews_core::select_scope(daynews_db::Scope::Feed(feed));
    }
}

/// Import: pick an `.opml` file and merge its subscriptions in.
pub fn import_opml() {
    day::task(async {
        let Some(url) = open_file()
            .filter(crate::res::str::opml_filter().format(), &["opml", "xml"])
            .await
        else {
            return;
        };
        match url.read() {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes).to_string();
                if let Err(e) = daynews_core::import_opml(&text).await {
                    daynews_core::state()
                        .status
                        .set(crate::res::str::opml_import_failed(e.as_str()).format());
                } else {
                    // Newly imported feeds have no articles yet; fetch them straight away so the
                    // app is useful immediately after an import.
                    daynews_core::refresh_all();
                }
            }
            Err(e) => daynews_core::state()
                .status
                .set(crate::res::str::opml_read_failed(e.to_string()).format()),
        }
    });
}

/// Export: write every subscription out as OPML.
pub fn export_opml() {
    day::task(async {
        let text = daynews_core::export_opml();
        // `save_file` takes the bytes up front: the picker writes them itself, which is the only
        // shape that works on the sandboxed platforms (the app never gets a writable path).
        let saved = save_file(text.into_bytes())
            .suggested_name("Day-News-Subscriptions.opml")
            .filter(crate::res::str::opml_filter().format(), &["opml"])
            .await;
        daynews_core::state().status.set(match saved {
            Some(_) => crate::res::str::opml_exported().format(),
            None => String::new(),
        });
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn feed_addresses_compare_without_scheme_www_case_or_trailing_slash() {
        use super::address_key;
        assert_eq!(
            address_key("https://www.TheRegister.com/headlines.atom"),
            address_key("http://theregister.com/headlines.atom/")
        );
        assert_ne!(
            address_key("https://www.theregister.com/headlines.atom"),
            address_key("https://www.theregister.com/?lab_viewport=rss")
        );
        assert_eq!(
            address_key("asset:demo/kitchen-science.xml"),
            address_key("asset:demo/kitchen-science.xml")
        );
    }
    #[test]
    fn clipboard_suggestion_does_not_wait_past_the_form_deadline() {
        use futures_util::FutureExt;
        use std::future::{pending, ready};

        let slow = super::clipboard_feed_url(Box::pin(pending()), ready(()));
        assert_eq!(slow.now_or_never(), Some(String::new()));

        let fixture = day::clipboard::Representation::new(
            "text/plain",
            b"feed:https://fixture.example/rss".to_vec(),
        );
        let available = super::clipboard_feed_url(Box::pin(ready(Ok(Some(fixture)))), pending());
        assert_eq!(
            available.now_or_never().as_deref(),
            Some("https://fixture.example/rss")
        );
    }

    #[test]
    fn incoming_feed_schemes_preserve_url_data() {
        for (input, expected) in [
            (
                "feed:https://fixture.example/rss?q=a%26b",
                "https://fixture.example/rss?q=a%26b",
            ),
            ("feed://fixture.example/rss", "http://fixture.example/rss"),
            (
                "feeds://fixture.example/atom",
                "https://fixture.example/atom",
            ),
            (
                "ATOM:https://fixture.example/feed",
                "https://fixture.example/feed",
            ),
            (
                "web+feed:https://fixture.example/feed",
                "https://fixture.example/feed",
            ),
        ] {
            assert_eq!(super::incoming_url(input).as_deref(), Some(expected));
        }
        for bad in [
            "feed:javascript:alert(1)",
            "feed:https://user:pass@fixture.example/",
            "unknown:https://fixture.example/",
        ] {
            assert!(super::incoming_url(bad).is_none());
        }
    }

    #[test]
    fn subscription_input_accepts_bare_hosts_and_rejects_non_web_urls() {
        for input in [
            "https://fixture.example/feed",
            "fixture.example/feed",
            "localhost:28763/feed",
            "http://127.0.0.1:28763/feed",
        ] {
            assert!(super::subscription_url(input).is_some(), "{input}");
        }
        for input in [
            "file:///tmp/feed",
            "file:/tmp/feed",
            "ftp://fixture.example/feed",
            "javascript:alert(1)",
            "https://user:pass@fixture.example/feed",
            "not a URL",
            "",
        ] {
            assert!(super::subscription_url(input).is_none(), "{input}");
        }
    }
}

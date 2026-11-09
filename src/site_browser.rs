//! Dedicated site profiles shared by the interactive browser and inert reader extraction.
use crate::res;
use day::prelude::*;
use day_piece_webview::{JsHandle, WebProfile, web_view};
/// Tracked read of the Incognito Mode setting (Settings ▸ Browser).
pub fn incognito() -> bool {
    crate::settings::INCOGNITO.get()
}
pub fn site_url(feed: u64) -> Option<String> {
    daynews_core::state()
        .feeds
        .with(|rows| {
            rows.iter()
                .find(|row| row.id == feed)
                .map(|row| row.site_url.clone().unwrap_or_else(|| row.feed_url.clone()))
        })
        .filter(|url| crate::extraction::web_url(url).is_some())
}
/// The web profile a feed's pages load in: one per feed site, persistent unless Incognito Mode
/// is on. The in-app browser and the reader (which reads its cookies for Load Full Article)
/// share it, so signing in to the site in one serves the other.
pub fn profile(feed: u64) -> WebProfile {
    let name = site_url(feed)
        .and_then(|url| url::Url::parse(&url).ok())
        .map(|url| url.origin().ascii_serialization())
        .unwrap_or_else(|| format!("feed:{feed}"));
    profile_named(&name)
}

/// The profile for a link with no feed behind it: its own site's.
fn profile_for_url(url: &str) -> WebProfile {
    let name = url::Url::parse(url)
        .map(|url| url.origin().ascii_serialization())
        .unwrap_or_else(|_| "links".into());
    profile_named(&name)
}

fn profile_named(name: &str) -> WebProfile {
    if day_piece_webview::profile_support() != Support::Native {
        return if incognito() {
            WebProfile::private("default")
        } else {
            WebProfile::default()
        };
    }
    if incognito() {
        WebProfile::private(format!("news:{name}"))
    } else {
        WebProfile::persistent(format!("news:{name}"))
    }
}
/// Manual article deselection returns to its publisher rather than the smart-feed scope.
pub fn show_article_feed_dashboard() {
    let scene = daynews_core::scene();
    let Some(selected) = scene.selected.get_untracked() else {
        // Scope changes already clear selection. Do not redirect those navigation events.
        return;
    };
    let feed = scene
        .article
        .with_untracked(|article| {
            article
                .as_ref()
                .filter(|a| a.id == selected)
                .map(|a| a.feed_id)
        })
        .or_else(|| {
            scene.articles.with_untracked(|articles| {
                articles
                    .iter()
                    .find(|a| a.id == selected)
                    .map(|a| a.feed_id)
            })
        });
    let Some(feed) = feed else { return };
    show_feed_dashboard(feed);
}
/// Open a sidebar publisher's overview, regardless of the current article or scope.
pub fn show_feed_dashboard(feed: u64) {
    let scene = daynews_core::scene();
    batch(|| {
        if crate::feed_list::FeedList::app()
            .unread_only
            .get_untracked()
            && daynews_core::state()
                .feeds
                .with_untracked(|feeds| feeds.iter().any(|f| f.id == feed && f.unread == 0))
        {
            crate::feed_list::toggle();
        }
        daynews_core::select_scope(daynews_db::Scope::Feed(feed));
        navigate(&format!("feed:{feed}"));
        scene.article.set(None);
        day::reactive::at_turn_end(move || scene.reader_open.set(true));
    });
}
#[derive(Clone, Copy)]
pub struct FeedSelection {
    pub section: Signal<Option<String>>,
    pub scene: daynews_core::NewsScene,
}
impl Binding<Option<String>> for FeedSelection {
    fn read(&self) -> Option<String> {
        self.section.get()
    }
    fn peek(&self) -> Option<String> {
        self.section.get_untracked()
    }
    fn write(&self, key: Option<String>) {
        if self.section.get_untracked() == key
            && key.as_deref().and_then(crate::scope_for_key).is_some()
        {
            batch(|| {
                self.scene.selected.set(None);
                self.scene.article.set(None);
                self.scene
                    .reader_open
                    .set(day_core::capability(day_spec::Cap::NavContentList) != Support::Emulated);
            });
        }
        self.section.set(key);
    }
}
/// The window's in-app browser: a page, the feed whose site profile it loads in (the one the
/// reader's Load Full Article reads cookies from), and what it shows while open.
#[derive(Clone, Copy)]
pub(crate) struct Browser {
    /// The page it opens on; `None` while closed.
    open: Signal<Option<String>>,
    /// The feed the page belongs to, when there is one.
    feed: Signal<Option<u64>>,
}
impl Ambient for Browser {
    fn create() -> Self {
        let state = Self {
            open: Signal::new(None),
            feed: Signal::new(None),
        };
        watch(incognito, move |private, _| {
            if *private {
                state.open.set(None);
            }
        });
        // Closing it, by Done or by swiping the sheet away, is the moment a sign-in can have
        // happened: every reader of the window drops a load that failed or came back short, and
        // the one on screen loads again with the session (`ReaderView::retry_after_browsing`).
        watch(
            move || state.open.get().is_some(),
            move |open, previous| {
                if !*open && previous == Some(&true) {
                    SESSIONS.with(|s| s.update(|n| *n += 1));
                }
            },
        );
        state
    }
}
thread_local! {
    /// Counts the in-app browser's closings: each may have changed a site's session.
    static SESSIONS: Signal<u64> = Signal::new(0);
}

/// Tracked: changes whenever the in-app browser closes, after which a site's cookies may differ.
pub fn sessions() -> u64 {
    SESSIONS.with(|s| s.get())
}

/// The Feed menu follows the focused window's sidebar selection.
pub fn selected_feed() -> Option<u64> {
    match daynews_core::try_scene()?.scope.get() {
        daynews_db::Scope::Feed(feed) => Some(feed),
        _ => None,
    }
}
/// Whether pages can open in the app: a native web view, outside Incognito Mode.
pub fn can_browse() -> bool {
    !incognito() && day_piece_webview::support() == Support::Native
}
pub fn can_preview(feed: u64) -> bool {
    can_browse() && site_url(feed).is_some()
}
/// Shared entry point for the dashboard, app menu and the context-clicked feed.
pub fn preview(feed: u64) {
    let Some(browser) = Browser::try_ambient().or_else(Browser::focused) else {
        return;
    };
    open_preview(browser, feed);
}
/// Row and button callbacks retain their owning window even if focus changes meanwhile.
pub fn preview_action(feed: u64) -> impl Fn() {
    let browser = Browser::ambient();
    move || open_preview(browser, feed)
}
fn open_preview(browser: Browser, feed: u64) {
    if can_preview(feed)
        && let Some(url) = site_url(feed)
    {
        browser.show(url, Some(feed));
    }
}
impl Browser {
    fn show(self, url: String, feed: Option<u64>) {
        self.feed.set(feed);
        self.open.set(Some(url));
    }
}
/// Open a page in the in-app browser whatever Settings ▸ Browser says about links: the reader
/// asks for it to sign in to a site. Without an in-app browser it goes out like any link.
pub fn browse(url: &str, feed: Option<u64>) {
    match Browser::try_ambient().or_else(Browser::focused) {
        Some(browser) if can_browse() && crate::extraction::web_url(url).is_some() => {
            browser.show(url.to_owned(), feed);
        }
        _ => crate::settings::open_link(url),
    }
}
/// Open a link the way Settings ▸ Browser says: in the in-app browser, under `feed`'s site
/// profile so a sign-in there reaches the reader, or in the external browser. Links the
/// in-app browser cannot show (another scheme, Incognito Mode, no native web view) always go
/// out.
pub fn open_link(url: &str, feed: Option<u64>) {
    let browser = Browser::try_ambient().or_else(Browser::focused);
    match browser {
        Some(browser)
            if crate::settings::BROWSER.get_untracked().is_empty()
                && can_browse()
                && crate::extraction::web_url(url).is_some() =>
        {
            browser.show(url.to_owned(), feed);
        }
        _ => crate::settings::open_link(url),
    }
}
pub fn site_controls(feed: u64) -> impl Piece {
    let clearing = Signal::new(false);
    let clear = Signal::new(false);
    let started = Signal::new(false);
    let status = Signal::new(String::new());
    let engine = JsHandle::new();
    let url = Signal::new("about:blank".to_owned());
    column((
        row((
            button(res::str::site_preview())
                .action(preview_action(feed))
                .enabled(move || !clearing.get() && can_preview(feed))
                .id(day::id!(dashboard_site_preview)),
            button(res::str::site_forget())
                .action(move || {
                    clearing.set(true);
                    started.set(false);
                    status.set(String::new());
                    clear.set(true);
                })
                .enabled(move || {
                    !incognito()
                        && !clearing.get()
                        && day_piece_webview::clear_data_support() == Support::Native
                        && day_piece_webview::profile_support() == Support::Native
                })
                .id(day::id!(dashboard_site_forget)),
        ))
        .spacing(Space::S),
        label(move || status.get())
            .font(Font::Caption)
            .id(day::id!(dashboard_site_status)),
        when(
            move || clear.get(),
            move || {
                web_view(url)
                    .profile(profile(feed))
                    .js(engine)
                    .on_load(move || {
                        if started.get_untracked() {
                            return;
                        }
                        started.set(true);
                        day::task(async move {
                            let result = futures_util::future::select(
                                Box::pin(engine.clear_data()),
                                Box::pin(day::sleep(15_000)),
                            )
                            .await;
                            let succeeded =
                                matches!(result, futures_util::future::Either::Left((Ok(_), _)));
                            status.set(if succeeded {
                                res::str::site_data_cleared().format()
                            } else {
                                res::str::site_data_failed().format()
                            });
                            clear.set(false);
                            clearing.set(false);
                        });
                    })
                    .id(day::id!(site_data_web))
                    .frame(1.0, 1.0)
            },
        ),
    ))
    .spacing(Space::XS)
}
/// The in-app browser, NetNewsWire's: Done, the page's title and site, and Open in Browser
/// across the top; Back and Forward along the bottom. Escape closes it on a desktop, where it is
/// a large sheet. Not a full browser: there is no address to type, only the page a link opened
/// and where its own links lead.
pub fn browser_cover() -> impl Piece {
    // Created in the window's ambient scope; dashboard controls find this same instance.
    let browser = Browser::ambient();
    cover(browser.open, move |start: &String| {
        let feed = browser.feed.get_untracked();
        let web_profile = match feed {
            Some(feed) => profile(feed),
            None => profile_for_url(start),
        };
        let address = Signal::new(start.clone());
        let page = Signal::new(day_piece_webview::NavigationState {
            url: start.clone(),
            ..Default::default()
        });
        let js = JsHandle::new();
        let (back, forward) = (Trigger::new(), Trigger::new());
        // The engine reports navigation state on request, not as it changes; ask while the
        // sheet is up, which is what moves the title and the history buttons.
        let poll = day::task(async move {
            loop {
                if let Ok(state) = js.navigation_state().await {
                    page.set_if_changed(state);
                }
                day::sleep(400).await;
            }
        });
        day::reactive::Scope::current().on_cleanup(move || poll.abort());
        let host = move || {
            let url = page.with(|p| p.url.clone());
            url::Url::parse(&url)
                .ok()
                .and_then(|u| {
                    u.host_str()
                        .map(|h| h.trim_start_matches("www.").to_owned())
                })
                .unwrap_or(url)
        };
        let current = move || {
            let url = page.with(|p| p.url.clone());
            if crate::extraction::web_url(&url).is_some() {
                url
            } else {
                address.get_untracked()
            }
        };
        let icon_button = |label: fn() -> day::LocalizedText, symbol: Symbol| {
            button(label()).icon(symbol).icon_only().plain()
        };
        column((
            row((
                button(res::str::site_browser_done())
                    .plain()
                    .action(move || browser.open.set(None))
                    .id(day::id!(site_browser_close)),
                column((
                    label(move || {
                        let title = page.with(|p| p.title.trim().to_owned());
                        if title.is_empty() { host() } else { title }
                    })
                    .font(Font::Headline)
                    .single_line()
                    .id(day::id!(site_browser_title)),
                    // The site under the page's title; a page with none shows its site once.
                    when(
                        move || page.with(|p| !p.title.trim().is_empty()),
                        move || {
                            label(host)
                                .font(Font::Caption)
                                .color(|| crate::theme::palette().text_muted)
                                .single_line()
                                .id(day::id!(site_browser_host))
                        },
                    ),
                ))
                .align(HAlign::Center)
                .grow(Dimension::Width),
                button(res::str::site_browser_open_external())
                    .image(res::vectors::open_browser)
                    .icon_only()
                    .plain()
                    .action(move || crate::settings::open_link(&current()))
                    .id(day::id!(site_browser_external)),
            ))
            .spacing(Space::S)
            .align(VAlign::Center)
            .padding(Space::S),
            web_view(address)
                .profile(web_profile)
                .js(js)
                .back(back)
                .forward(forward)
                .on_external_link(|_| day_piece_webview::LinkPolicy::InView)
                .id(day::id!(site_browser_web))
                .grow(Dimension::Both),
            row((
                icon_button(res::str::site_browser_back, Symbol::Back)
                    .enabled(move || page.with(|p| p.can_go_back))
                    .action(move || back.notify())
                    .id(day::id!(site_browser_back)),
                icon_button(res::str::site_browser_forward, Symbol::Forward)
                    .enabled(move || page.with(|p| p.can_go_forward))
                    .action(move || forward.notify())
                    .id(day::id!(site_browser_forward)),
                spacer().grow(Dimension::Width),
            ))
            .spacing(Space::M)
            .align(VAlign::Center)
            .padding(Space::S),
        ))
        .id(day::id!(site_browser_sheet))
        .grow(Dimension::Both)
    })
    // A large sheet on a desktop, the page needing more room than a form; the composed tiers
    // dim the window around it, and a phone shows the full-window cover.
    .large_sheet()
    .background(|_| crate::theme::palette().bg)
    .unrouted()
}

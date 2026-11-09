// SPDX-License-Identifier: Apache-2.0
//! Fleet Reader: a feed reader built on [Day](https://daybrite.dev), modeled on NetNewsWire.
//!
//! Three panes on a desktop (feed sidebar, timeline, article) and a push-navigation stack on a
//! phone, from one `root()`. Everything the UI shows is a reactive signal published by
//! `daynews-core`; the crates underneath own feed parsing, OPML and the SQLite store.

use day::prelude::*;

#[cfg(not(target_arch = "wasm32"))]
mod accounts;
mod commands;
mod dashboard;
mod extraction;
mod feed_icons;
mod feed_list;
mod format;
mod http_fixtures;
mod load_diagnostics;
mod menus;
mod reader;
mod reader_options;
mod reader_pager;
mod reader_scripts;
mod reader_styles;
mod reader_view;
mod refresh_schedule;
mod settings;
mod site_browser;
mod subscriptions;
#[cfg(all(
    test,
    any(target_os = "macos", target_os = "linux", target_os = "windows")
))]
mod test_prefs;
mod theme;
mod timeline;
mod toolbar;
#[cfg(all(feature = "experimental-webdav", not(target_arch = "wasm32")))]
mod webdav_accounts;

use daynews_db::Scope;

// The mobile / embedded entry point. Expands to the export each platform's shell binds against,
// and to nothing at all on a plain cargo desktop build, where src/main.rs is the entry instead.
// Both entries hand `launch` the same description, so they open the same window.
day::day_start!(options: window(), root);

/// The window every entry point opens: `src/main.rs` on the desktop, the platform shells
/// through the macro above.
///
/// `launch` installs the catalog itself, after the OS's languages have reached day-localization and
/// before the first localized string is read; installing it here, or in `root`, would resolve
/// against an empty hint list and open an English window on a French device. The same ordering
/// is what lets the title come from the catalog (docs/localization.md).
pub fn window() -> day::WindowOptions {
    // Three panes need room. At 960 the timeline and the article both end up too narrow to
    // read comfortably; this is close to NetNewsWire's own default.
    res::window_options()
        .size(1440.0, 900.0)
        .min_size(720.0, 480.0)
}

// Typed constants for the files under `resource/`, generated at build time by `day-build`.
day::resources!();

/// The sidebar's selection, as a route key. Smart feeds come first (NetNewsWire's "All Unread"
/// and "Starred"), then one entry per subscription, then the management page.
fn scope_for_key(key: &str) -> Option<Scope> {
    match key {
        "today" => Some(Scope::Today),
        "unread" => Some(Scope::Unread),
        "all" => Some(Scope::All),
        "starred" => Some(Scope::Starred),
        k => {
            if let Some(id) = k
                .strip_prefix("feed:")
                .and_then(|id| id.parse::<u64>().ok())
            {
                Some(Scope::Feed(id))
            } else {
                k.strip_prefix("tag:")
                    .and_then(|id| id.parse::<u64>().ok())
                    .map(Scope::Tag)
            }
        }
    }
}

/// The feed a sidebar key names.
fn feed_of_key(key: &str) -> Option<u64> {
    key.strip_prefix("feed:")?.parse().ok()
}

/// A sidebar count, blank when there is nothing unread; an empty badge draws nothing.
fn count(n: i64) -> String {
    if n > 0 {
        day::format_decimal(n as f64, 0)
    } else {
        String::new()
    }
}

pub fn root() -> impl Piece {
    http_fixtures::install();
    // Start opening the store before building the views. Native database work runs on its
    // owning worker; committed snapshots populate the UI as they become available.
    #[cfg(target_arch = "wasm32")]
    daynews_core::init();
    #[cfg(not(target_arch = "wasm32"))]
    {
        daynews_core::init(|message| {
            use daynews_core::StatusMessage;
            match message {
                StatusMessage::RefreshedFeed(title, true) => {
                    res::str::status_feed_refreshed(title).format()
                }
                StatusMessage::RefreshedFeed(title, false) => {
                    res::str::status_feed_failed(title).format()
                }
                StatusMessage::RefreshedFeeds(total, failed) => {
                    res::str::status_feeds_refreshed(res::str::StatusFeedsRefreshed {
                        done: (total - failed) as i64,
                        failed: failed as i64,
                        total: total as i64,
                    })
                    .format()
                }
                StatusMessage::RefreshedFeedsPendingSync(total, failed) => {
                    res::str::status_feeds_sync_pending(res::str::StatusFeedsSyncPending {
                        done: (total - failed) as i64,
                        failed: failed as i64,
                        total: total as i64,
                    })
                    .format()
                }
                StatusMessage::Imported(added, existing) => {
                    res::str::status_imported(res::str::StatusImported {
                        added: added as i64,
                        existing: existing as i64,
                    })
                    .format()
                }
                StatusMessage::ExportTitle => res::str::subscriptions_export_title().format(),
            }
        });
        day::on_lifecycle(Lifecycle::WillTerminate, daynews_core::shutdown);
    }
    #[cfg(not(target_arch = "wasm32"))]
    Effect::new(|| {
        if let Some(error) = daynews_core::StorageError::app().0.get() {
            daynews_core::state()
                .status
                .set(res::str::storage_error(error).format());
        }
    });
    // Settings and About (https://daybrite.dev/docs/reference/settings): appearance and language from
    // Day's catalog, the app's own groups, the name and version from Day.toml. A desktop gets
    // the App menu's Settings… and About items and their windows; the web uses a modal surface
    // and phones get the Settings sidebar section in `shell_body`. Also applies grouping and retention
    // and starts the refresh timer, so it follows the store being opened above.
    let prefs = settings::install();
    day::on_lifecycle(Lifecycle::WillTerminate, refresh_schedule::stop);
    feed_icons::init();
    #[cfg(not(target_arch = "wasm32"))]
    {
        daynews_core::sync_install();
        day::on_lifecycle(Lifecycle::DidBecomeActive, refresh_schedule::activated);
    }
    // The undo history rides the container's change log; the platform bridge gives it ⌘Z,
    // the Edit menu, and the mobile gestures.
    #[cfg(target_arch = "wasm32")]
    if let Some(stack) = daynews_core::undo_stack() {
        day::install_undo(&stack);
    }
    // Every window shows the same store (the reader is the app, not the window), so a new
    // window is just another shell. Registered once; each window builds its own signals.
    {
        let prefs = prefs.clone();
        day::register_new_window(move || build_shell(prefs.clone()));
    }
    // Menu availability needs the registered window's scene. Install on the next UI
    // turn, after the shell has mounted; commands also tolerate the last window closing.
    day::task(async {
        menus::install();
    });
    build_shell(prefs)
}

/// The sidebar row a window opens on: NetNewsWire's top smart feed.
const OPENING_SECTION: &str = "today";

/// One window's contents. Called again for each File ▸ New Window.
fn build_shell(prefs: day_piece_settings::Prefs) -> impl Piece {
    // Each window owns its scope, search and selection; the store and badges stay shared.
    daynews_core::NewsScene::scoped(move |sc| {
        reader_view::ReaderView::scoped(move |_| {
            site_browser::Browser::scoped(move |_| {
                subscriptions::SubscriptionSheet::scoped(move |_| shell_body(sc, prefs))
            })
        })
    })
}

fn shell_body(sc: daynews_core::NewsScene, prefs: day_piece_settings::Prefs) -> impl Piece {
    let st = daynews_core::state();
    // Sidebar rows and toolbar items can be re-derived outside this window's reactive
    // scope. Resolve their scene and register their commands with this window, so closing
    // a transient sheet cannot unregister the actions still installed on its sidebar.
    let menu_scope = day::reactive::Scope::current();
    let section: Signal<Option<String>> = Signal::new(Some(OPENING_SECTION.into()));
    bind_sidebar_scope(section, sc);
    // Whether a phone shows an article list rather than the feeds: choosing a section pushes
    // one, Back clears the selection, and an open reader sits on top of one. A phone opens on
    // the list of the section the window starts with (docs/navigation.md), so that counts too.
    let list_shown = Signal::new(section.get_untracked().is_some());
    watch(
        move || section.get(),
        move |now, _| list_shown.set(now.is_some()),
    );
    watch(
        move || sc.reader_open.get(),
        move |open, _| {
            if *open {
                list_shown.set(true);
            }
        },
    );

    let navigation = nav(site_browser::FeedSelection { section, scene: sc })
        .style(NavStyle::Sidebar)
        .retain_selection_when(move |key: &Option<String>| {
            matches!(key.as_deref().and_then(scope_for_key), Some(Scope::Feed(id))
                if st.feeds.with_untracked(|feeds| feeds.iter().any(|feed| feed.id == id)))
        })
        .title(res::str::app_title())
        .refreshable(commands::refreshing, commands::refresh_now)
        // Feeds drag between collections, and within the sections whose order the user sets
        // (feed_list::drag); a drop joins the collection it lands in.
        .regroup_items(
            |key: &Option<String>| match key.as_deref().and_then(feed_of_key) {
                Some(feed) => feed_list::drag(feed),
                None => NavDrag::Fixed,
            },
            |drop: NavDrop<Option<String>>| {
                if let Some(feed) = drop.moved.as_deref().and_then(feed_of_key) {
                    let before = drop.before.flatten().as_deref().and_then(feed_of_key);
                    feed_list::dropped(feed, drop.section.as_deref(), before);
                }
            },
        )
        // Refresh and Mark All as Read act on the scope this list has chosen, so they ride this
        // host's chrome (docs/toolbars.md): the sidebar column on a desktop, the root
        // list's bar when it collapses. The article's commands are on the reader.
        .toolbar(move || menu_scope.enter(|| toolbar::feed_items(list_shown)))
        .icon_progress(move || {
            st.updating_feeds
                .get()
                .into_iter()
                .map(|(id, progress)| (Some(format!("feed:{id}")), progress))
                .collect()
        })
        // Search moved off the toolbar when day replaced `toolbar_search` with `.searchable()`:
        // the nav owns the field now, and the toolkit puts it in the window toolbar on
        // desktop and inline above the list on a phone. The signal is still the shared one the
        // timeline filters on.
        .searchable(
            Search::new(toolbar::search())
                .reveal(SearchReveal::OnPull)
                .prompt(res::str::search_placeholder()),
        )
        // The article list is the content-list pane (docs/navigation.md): its own column
        // between the sidebar and the reader where the toolkit has one (a real `contentList`
        // split item on macOS, the supplementary column on iPadOS), the pushed middle layer
        // on a phone, and composed beside the reader elsewhere. Full-page sections keep the
        // whole detail area.
        .content_list(
            ContentList::new(timeline::timeline_pane)
                .width(400.0)
                .only_for(|k: &Option<String>| !matches!(k.as_deref(), Some("settings")))
                .detail_visible(sc.reader_open),
        )
        // Smart feeds: Today, All Unread and Starred, in NetNewsWire's order, under their own
        // header. Counts are real badges: right-aligned and de-emphasized by the toolkit.
        // Per-feed identity colors (docs/vectors.md): the sun, the dot, the star and the
        // stack each wear their own tint, NetNewsWire-style.
        .section(
            res::str::nav_smart_feeds(),
            (
                item("today".to_string(), res::str::nav_today())
                    .icon(res::images::sidebar_today)
                    .icon_tint(Color::hex(0xFF9F0A))
                    .badge(move || count(st.total_today.get()))
                    .page(reader_dest),
                item("unread".to_string(), res::str::nav_all_unread())
                    .icon(res::images::sidebar_unread)
                    .icon_tint(Color::hex(0x0A84FF))
                    .badge(move || count(st.total_unread.get()))
                    .page(reader_dest),
                item("starred".to_string(), res::str::nav_starred())
                    .icon(res::images::sidebar_starred)
                    .icon_tint(Color::hex(0xE8940A))
                    .badge(move || count(st.total_starred.get()))
                    .page(reader_dest),
                item("all".to_string(), res::str::nav_all_articles())
                    .icon(res::images::sidebar_all)
                    .icon_tint(Color::hex(0x5E5CE6))
                    .badge(move || count(st.total_articles.get()))
                    .page(reader_dest),
            ),
        )
        // One section per collection, then the feeds in no collection, re-derived whenever the
        // feeds, their counts or the collections change. Each section heads its first row.
        .items(nav_items(
            move || {
                let unread_only = feed_list::FeedList::app().unread_only.get();
                let icons = feed_icons::paths().get();
                feed_list::current_groups()
                    .into_iter()
                    .flat_map(|group| {
                        let header = (group.section_id(), group.title());
                        group
                            .feeds
                            .into_iter()
                            .filter(|feed| feed_list::visible(feed.unread, unread_only))
                            .enumerate()
                            .map(|(i, feed)| {
                                let icon = icons.get(&feed.id).cloned();
                                (feed, icon, (i == 0).then(|| header.clone()))
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>()
            },
            move |(f, icon, header): &(
                daynews_core::FeedRow,
                Option<String>,
                Option<(String, String)>,
            )| {
                let name = if f.has_error {
                    format!("⚠ {}", f.title)
                } else {
                    f.title.clone()
                };
                let row = item(format!("feed:{}", f.id), name);
                let row = if let Some(path) = icon {
                    row.icon(path.clone())
                } else {
                    row.icon(res::images::sidebar_feed)
                        .icon_tint(Color::hex(0x30B0C7))
                };
                let row = match header {
                    Some((id, title)) => row.section_id(id.clone(), title.clone()),
                    None => row,
                };
                menu_scope.enter(|| {
                    row.badge(count(f.unread))
                        .context_menu(menus::feed_context_menu(f.id, f.unread, f.folder_id))
                })
            },
        ))
        // User tags, with their article counts; selecting one scopes the timeline to it.
        .section(
            res::str::nav_tags_section(),
            nav_items(
                move || st.tags.get(),
                |t: &daynews_core::TagRow| {
                    item(format!("tag:{}", t.id), t.name.clone())
                        .icon(res::images::sidebar_starred)
                        .icon_tint(Color::hex(0xE8940A))
                        .badge(count(t.count))
                },
            ),
        )
        // Settings rides the sidebar where no app menu can open the surface (phones):
        // `nav_item` is `None` elsewhere, including the web (https://daybrite.dev/docs/reference/settings).
        .items(
            (!crate::settings::TOOLBAR_SETTINGS)
                .then(|| prefs.nav_item(Some("settings".to_string())))
                .flatten(),
        )
        .destination(move |_: &Option<String>| reader_dest());
    #[cfg(not(target_arch = "wasm32"))]
    let navigation = navigation.footer(|| {
        when(
            || {
                crate::settings::TOOLBAR_SETTINGS
                    && day::size_class().is_none_or(|size| size.width == WidthClass::Compact)
                    && !toolbar::status_in_bottom_bar()
            },
            || accounts::sync_status_footer(day::id!(feeds_sync_status_footer)),
        )
    });
    column((
        navigation.id(day::id!(nav)).grow(Dimension::Both),
        toolbar::status_bar(),
        site_browser::browser_cover(),
        subscriptions::subscription_sheet(),
    ))
    .grow(Dimension::Both)
}

/// The reader as a destination. The timeline is no longer in here; it is the nav's
/// content-list pane, its own column beside this on the desktops and the pushed middle layer
/// on a phone (docs/navigation.md). Every phone's own back returns from it to the list: the
/// iOS chevron, and on Android the app bar's arrow and the system back, which close the
/// reader through the nav's `detail_visible` binding. (Android once added a Back button row
/// here as well, which put two back controls on one screen.)
fn reader_dest() -> impl Piece {
    reader::reader_pane().grow(Dimension::Both)
}

fn bind_sidebar_scope(section: Signal<Option<String>>, sc: daynews_core::NewsScene) {
    // Reactions may drain after an event leaves this window's scope. Resolve scene-based
    // commands in their owner's context, even when Settings or another window has focus.
    let owner = day::reactive::Scope::current();
    // `watch` fires on change, so apply the opening scope before subscribing.
    if let Some(scope) = section.get_untracked().as_deref().and_then(scope_for_key) {
        daynews_core::select_scope(scope);
    }
    watch(
        move || section.get(),
        move |key, _| {
            if key.is_none() {
                // A phone's Back closed the list. Picking the same section again keeps the
                // scope, so the read articles held in it are released here instead.
                owner.enter(daynews_core::release_sticky_read);
            } else if let Some(scope) = key.as_deref().and_then(scope_for_key)
                && sc.scope.get_untracked() != scope
            {
                owner.enter(|| daynews_core::select_scope(scope));
            }
        },
    );
}

#[cfg(test)]
mod navigation_tests {
    use super::*;

    #[test]
    fn navigation_counts_follow_locale_changes() {
        res::locales::install();
        let previous = day::locale().get_untracked();
        let feeds = daynews_core::state().feeds;
        let original = feeds.get_untracked();
        feeds.set(
            (0..12_345)
                .map(|id| daynews_core::FeedRow {
                    id,
                    title: "Synthetic feed".into(),
                    unread: if id < 1_234 { 99 } else { 0 },
                    folder_id: None,
                    has_error: false,
                    site_url: None,
                    feed_url: String::new(),
                    icon_url: None,
                    last_fetched: Some(0),
                    total: 0,
                    newest: None,
                    collection_position: 0.0,
                })
                .collect(),
        );
        let title = || {
            feed_list::current_groups()
                .last()
                .map(|group| group.title().replace(['\u{2068}', '\u{2069}'], ""))
                .unwrap_or_default()
        };
        for (locale, expected) in [
            ("en-US", "72,811"),
            ("fr-FR", "72\u{202f}811"),
            ("de-DE", "72.811"),
        ] {
            day::locale().set(locale.to_string());
            assert_eq!(count(72_811), expected);
            assert_eq!(count(0), "");
            let expected_count = format!("({})", day::format_decimal(12_345.0, 0));
            assert!(title().contains(&expected_count), "{:?}", title());
        }
        feeds.set(Vec::new());
        assert!(title().contains("(0)"), "{:?}", title());
        feeds.set(original);
        day::locale().set(previous);
    }

    #[test]
    fn sidebar_selection_keeps_its_scene_when_reactions_run_elsewhere() {
        let owner = day::reactive::Scope::child();
        let (scene, section) = owner.enter(|| {
            let scene = daynews_core::NewsScene::create();
            owner.provide(scene);
            let section = Signal::new(Some(OPENING_SECTION.to_string()));
            bind_sidebar_scope(section, scene);
            (scene, section)
        });
        assert_eq!(scene.scope.get_untracked(), Scope::Today);
        scene.selected.set(Some(42)); // Synthetic selection; no database or network.
        scene.reader_open.set(true);
        let other_owner = day::reactive::Scope::child();
        let other = other_owner.enter(|| {
            let other = daynews_core::NewsScene::create();
            other_owner.provide(other);
            section.set(Some("feed:7".into()));
            day::reactive::flush_sync();
            other
        });
        assert_eq!(scene.scope.get_untracked(), Scope::Feed(7));
        assert_eq!(other.scope.get_untracked(), Scope::Unread);
        assert_eq!(scene.selected.get_untracked(), None);
        assert!(!scene.reader_open.get_untracked());

        // Settings is a full-page section. Returning from it must also work with no
        // ambient scene and no focused native window, as in the browser walkthrough.
        assert!(daynews_core::try_scene().is_none());
        section.set(Some("settings".into()));
        day::reactive::flush_sync();
        assert_eq!(scene.scope.get_untracked(), Scope::Feed(7));
        section.set(Some("feed:9".into()));
        day::reactive::flush_sync();
        assert_eq!(scene.scope.get_untracked(), Scope::Feed(9));
        other_owner.dispose();
        owner.dispose();
    }
}

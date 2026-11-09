// SPDX-License-Identifier: Apache-2.0
//! Reader operations shared by native chrome and content fallbacks.
use crate::res;
use day::prelude::*;

pub(crate) fn preview_site() -> Command {
    Command::new(day::id!(preview_site), res::str::site_preview(), || {
        if let Some(feed) = crate::site_browser::selected_feed() {
            crate::site_browser::preview(feed);
        }
    })
    .enabled(|| crate::site_browser::selected_feed().is_some_and(crate::site_browser::can_preview))
}

pub(crate) fn refreshing() -> bool {
    let refreshing = daynews_core::state().refresh_progress.get().is_some();
    #[cfg(not(target_arch = "wasm32"))]
    let refreshing = refreshing || daynews_core::sync_status().get().busy;
    refreshing
}

pub(crate) fn refresh_now() {
    daynews_core::refresh_all();
}

pub(crate) fn refresh() -> Command {
    Command::new(day::id!(refresh), res::str::refresh_action(), refresh_now)
        .icon(Symbol::Refresh)
        .shortcut(Shortcut::primary("r"))
        .enabled(|| !refreshing())
}

pub(crate) fn mark_all_read() -> Command {
    Command::new(day::id!(mark_all_read), res::str::mark_all_read(), || {
        if daynews_core::try_scene().is_some() {
            daynews_core::mark_scope_read(true);
        }
    })
    .icon(Symbol::Check)
    .shortcut(Shortcut::primary("k"))
    .enabled(|| daynews_core::try_scene().is_some_and(|sc| sc.scope_unread.get() > 0))
}

/// Shared by the menu and native toolbars, including before any article is open.
pub(crate) fn next_unread() -> Command {
    Command::new(day::id!(next_unread), res::str::menu_next_unread(), || {
        let Some(sc) = daynews_core::try_scene() else {
            return;
        };
        let before = sc.scope.get_untracked();
        if daynews_core::open_next_unread() {
            if before != sc.scope.get_untracked() {
                navigate(crate::menus::ROUTE_UNREAD);
            }
            if cfg!(feature = "appkit") {
                sc.timeline_focused.set(true);
            }
        }
    })
    .image(res::vectors::next_unread)
    .shortcut(Shortcut::primary("/"))
    .enabled(|| {
        daynews_core::try_scene().is_some_and(|sc| {
            let current_unread = sc.article.with(|a| a.as_ref().is_some_and(|a| !a.is_read));
            daynews_core::state().total_unread.get() > i64::from(current_unread)
        })
    })
}

pub(crate) fn open_in_browser() -> Command {
    Command::new(
        day::id!(open_in_browser),
        res::str::menu_open_in_browser(),
        || {
            if let Some(url) = displayed_link() {
                crate::site_browser::open_link(&url, displayed_feed());
            }
        },
    )
    .image(res::vectors::open_browser)
    .shortcut(Shortcut::plain("Return"))
    .enabled(|| displayed_link().is_some())
}

/// Traverse the visible ordering, including search results, without wrapping or jumping
/// to another feed. A stale selection must not silently jump to an unrelated article.
fn adjacent_id(
    mut ids: impl DoubleEndedIterator<Item = u64>,
    selected: Option<u64>,
    forward: bool,
) -> Option<u64> {
    let Some(selected) = selected else {
        return if forward { ids.next() } else { ids.next_back() };
    };
    let mut previous = None;
    while let Some(id) = ids.next() {
        if id == selected {
            return if forward { ids.next() } else { previous };
        }
        previous = Some(id);
    }
    None
}

fn adjacent_article(forward: bool) -> Option<u64> {
    let sc = daynews_core::try_scene()?;
    let selected = sc.selected.get();
    sc.articles
        .with(|rows| adjacent_id(rows.iter().map(|a| a.id), selected, forward))
}

/// Shared by keyboard/menu navigation and mobile reader swipes.
pub(crate) fn open_adjacent_article(forward: bool) {
    if let Some(id) = adjacent_article(forward) {
        daynews_core::open_article(id);
        if cfg!(feature = "appkit") {
            daynews_core::scene().timeline_focused.set(true);
        }
    }
}

pub(crate) fn navigate_article(forward: bool) -> Command {
    Command::new(
        if forward {
            day::id!(next_article)
        } else {
            day::id!(previous_article)
        },
        if forward {
            res::str::menu_next_article()
        } else {
            res::str::menu_previous_article()
        },
        move || open_adjacent_article(forward),
    )
    .icon(if forward { Symbol::Down } else { Symbol::Up })
    .shortcut(Shortcut::primary(if forward { "]" } else { "[" }))
    .enabled(move || adjacent_article(forward).is_some())
}

pub(crate) fn copy_article_link() -> Command {
    Command::new(
        day::id!(copy_article_link),
        res::str::menu_copy_article_link(),
        || {
            if let Some(url) = displayed_link() {
                copy_link(&url);
            }
        },
    )
    .icon(Symbol::Copy)
    .shortcut(Shortcut::primary("c").shift())
    .enabled(|| displayed_link().is_some())
}

/// The open article's feed, whose site profile its page opens in.
fn displayed_feed() -> Option<u64> {
    daynews_core::try_scene()?
        .article
        .with_untracked(|a| a.as_ref().map(|a| a.feed_id))
}

fn displayed_link() -> Option<String> {
    daynews_core::try_scene()?
        .article
        .with(|a| a.as_ref().and_then(|a| a.url.clone()))
        .filter(|url| !url.trim().is_empty())
}

pub(crate) fn copy_link(url: &str) {
    // Start in the user gesture: browsers need its clipboard permission context.
    let write = day::clipboard::write(day::clipboard::Content(vec![
        day::clipboard::Representation::new("text/plain", url.as_bytes()),
    ]));
    day::task(async move {
        if write.await.is_err() {
            alert(res::str::copy_link_failed())
                .button(res::str::dismiss_alert(), ())
                .present()
                .await;
        }
    });
}

pub(crate) fn dashboard() -> Command {
    Command::new(day::id!(feed_overview), res::str::dashboard_show(), || {
        let scene = daynews_core::scene();
        if scene.selected.get_untracked().is_some() {
            crate::site_browser::show_article_feed_dashboard();
        } else {
            batch(|| {
                scene.article.set(None);
                scene.reader_open.set(true);
            });
        }
    })
    .icon(Symbol::Info)
}

pub(crate) fn group_by_feeds() -> Command {
    Command::new(
        day::id!(group_by_feeds),
        res::str::group_by_feeds(),
        crate::feed_list::toggle_grouping,
    )
    .icon(Symbol::Filter)
    .checked(|| daynews_core::state().group_by_feed.get())
}

pub(crate) fn unread_feeds_only() -> Command {
    Command::new(
        day::id!(unread_feeds_only),
        res::str::show_unread_feeds_only(),
        crate::feed_list::toggle,
    )
    .image(res::vectors::unread_feeds_only)
    .checked(|| crate::feed_list::FeedList::app().unread_only.get())
}

pub(crate) fn full_content_shortcut() -> Shortcut {
    Shortcut::primary("r").shift()
}

pub(crate) fn reader_view() -> Command {
    Command::new(day::id!(reader_view), res::str::menu_reader_view(), || {
        if let (Some(view), Some(scene)) = (
            crate::reader_view::ReaderView::current(),
            daynews_core::try_scene(),
        ) {
            view.toggle(scene);
        }
    })
    .icon(Symbol::Document)
    .shortcut(full_content_shortcut())
    .checked(|| crate::reader_view::ReaderView::current().is_some_and(|view| view.active.get()))
    .enabled(|| {
        crate::reader_view::ReaderView::current().is_some_and(|view| {
            (view.ready.get() || view.loading.get())
                && day_piece_webview::eval_support() == Support::Native
                && daynews_core::try_scene().is_some_and(|scene| {
                    scene.article.with(|a| {
                        a.as_ref()
                            .filter(|a| scene.selected.get() == Some(a.id))
                            .and_then(|a| a.url.as_deref())
                            .and_then(crate::extraction::web_url)
                            .is_some()
                    })
                })
        })
    })
}

/// Find searches the indexed articles through the native toolbar field.
pub(crate) fn find() -> Command {
    Command::new(
        day::id!(find_articles),
        res::str::menu_find_articles(),
        day::focus_search,
    )
    .shortcut(Shortcut::primary("f"))
    .enabled(|| daynews_core::try_scene().is_some())
}

#[cfg(test)]
mod tests {
    #[test]
    fn article_commands_are_disabled_before_a_window_exists() {
        assert!(!super::open_in_browser().is_enabled());
        assert!(!super::mark_all_read().is_enabled());
        assert!(!super::next_unread().is_enabled());
        assert!(!super::reader_view().is_enabled());
        assert!(!super::navigate_article(false).is_enabled());
        assert!(!super::navigate_article(true).is_enabled());
        assert!(!super::copy_article_link().is_enabled());
    }

    #[test]
    fn article_navigation_follows_visible_order_without_wrapping() {
        let ids = [80, 3, 42]; // Synthetic IDs deliberately differ from sort order.
        assert_eq!(super::adjacent_id(ids.into_iter(), None, true), Some(80));
        assert_eq!(super::adjacent_id(ids.into_iter(), None, false), Some(42));
        assert_eq!(super::adjacent_id(ids.into_iter(), Some(3), true), Some(42));
        assert_eq!(
            super::adjacent_id(ids.into_iter(), Some(3), false),
            Some(80)
        );
        assert_eq!(super::adjacent_id(ids.into_iter(), Some(80), false), None);
        assert_eq!(super::adjacent_id(ids.into_iter(), Some(42), true), None);
    }

    #[test]
    fn article_navigation_handles_empty_single_and_filtered_out_selections() {
        for forward in [false, true] {
            assert_eq!(super::adjacent_id([].into_iter(), None, forward), None);
            assert_eq!(super::adjacent_id([3].into_iter(), Some(3), forward), None);
            assert_eq!(
                super::adjacent_id([3, 4].into_iter(), Some(2), forward),
                None
            );
        }
    }
}

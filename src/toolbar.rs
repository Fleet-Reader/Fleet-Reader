// SPDX-License-Identifier: Apache-2.0
//! The window's toolbar items, its status bar, and the search signal they share
//! (docs/toolbars.md).
//!
//! Feed commands stay with navigation; article commands stay with the reader. Next Unread
//! is available before selection and moves into the article toolbar once reading starts.

use crate::res;
use day::prelude::*;

/// Search has one source of truth, shared by native chrome and the timeline query.
pub fn search() -> Signal<String> {
    daynews_core::scene().search
}

/// Whether this toolkit puts commands in a window bar. Where it does not, the reader's commands
/// still appear (a contribution always lands on some chrome), but the timeline carries the
/// search field itself rather than handing it to a window toolbar.
pub fn available() -> bool {
    capability(Cap::Toolbar) != Support::Unsupported
}

/// The feed list's commands: they act on the scope the sidebar has chosen, which is what the
/// user is looking at while the list is in front of them. `list_shown` says whether a phone
/// shows an article list rather than the feeds page.
pub fn feed_items(list_shown: Signal<bool>) -> Vec<ToolbarEntry> {
    let sc = daynews_core::scene();
    // A phone's narrow reader bar prioritizes reading actions. Sidebar commands
    // remain one native Back away on the timeline.
    let compact = day::size_class().is_none_or(|size| size.width == WidthClass::Compact);
    if crate::settings::TOOLBAR_SETTINGS && compact && sc.reader_open.get() {
        return Vec::new();
    }
    let mut items = Vec::new();
    if phone_bottom_bar() {
        // The filter and New Feed ride the bottom bar on a phone (`bottom_bar`).
        items.push(refresh_item());
        items.extend(bottom_bar(list_shown.get()));
    } else if cfg!(feature = "appkit") {
        // Only Refresh and the filter over a Mac's sidebar; the rest sit over the list
        // (`list_items`). AppKit folds the toolbar's search field into a magnifier whenever
        // the sidebar's part of the toolbar holds more than two items beside its toggle,
        // however much room the rest of the toolbar has, and Mail keeps its sidebar part
        // as light for the same reason.
        items.extend(list_controls());
        return items;
    } else {
        items.push(new_feed_item().placement(ToolbarPlacement::Secondary));
        if !wide_uikit() {
            items.extend(list_controls());
        }
    }
    if sc.article.with(|a| a.is_none()) {
        if !crate::settings::TOOLBAR_SETTINGS {
            items.push(
                crate::commands::dashboard()
                    .toolbar_item()
                    .placement(ToolbarPlacement::Secondary),
            );
        }
        items.push(crate::commands::next_unread().toolbar_item());
    }
    if crate::settings::TOOLBAR_SETTINGS {
        items.push(settings_item());
    }
    items
}

fn new_feed_item() -> ToolbarEntry {
    toolbar_button(day::id!(new_feed), res::str::menu_new_feed())
        .icon(Symbol::Add)
        .action(crate::subscriptions::begin_new_feed)
}

fn settings_item() -> ToolbarEntry {
    toolbar_button(day::id!(settings), res::str::nav_settings())
        .icon(Symbol::Settings)
        .action(|| {
            day_piece_settings::open();
        })
        .placement(ToolbarPlacement::Primary)
}

/// The open article's commands. Declared on the reader, so they arrive with the article and
/// leave with it, and each one reads the article it was built for rather than a mirror of it.
pub fn article_items(article: daynews_core::StoredArticle) -> Vec<ToolbarEntry> {
    let (id, starred, read) = (article.id, article.is_starred, article.is_read);
    let star_state = Signal::new(starred);
    let read_state = Signal::new(read);
    let read_label = if read {
        res::str::mark_unread()
    } else {
        res::str::mark_read()
    };
    // Like NetNewsWire, iOS expresses read status in the icon, not UIKit's
    // selected bar-button background. The reader rebuilds these items when
    // the stored article changes, including changes received through sync.
    let read_item = if cfg!(feature = "uikit") {
        let button = toolbar_button(day::id!(read), read_label);
        let button = if read {
            button.icon(Symbol::Circle)
        } else {
            button.image(res::vectors::article_unread)
        };
        button.action(move || daynews_core::toggle_read(id))
    } else {
        toolbar_toggle(day::id!(read), read_label, read_state)
            .icon(Symbol::CircleFilled)
            .action(move || daynews_core::set_read(id, read_state.get_untracked()))
    }
    .placement(ToolbarPlacement::Primary);
    let mut items = vec![
        crate::commands::navigate_article(false).toolbar_item(),
        crate::commands::navigate_article(true).toolbar_item(),
        crate::commands::next_unread().toolbar_item(),
        toolbar_toggle(
            day::id!(star),
            if starred {
                res::str::unstar()
            } else {
                res::str::star()
            },
            star_state,
        )
        .icon(Symbol::Star)
        .action(move || daynews_core::set_starred(id, star_state.get_untracked())),
        read_item,
        crate::commands::open_in_browser().toolbar_item(),
        crate::commands::reader_view().toolbar_item(),
        crate::commands::dashboard()
            .toolbar_item()
            .placement(ToolbarPlacement::Secondary),
        crate::commands::copy_article_link()
            .toolbar_item()
            .placement(ToolbarPlacement::Secondary),
    ];
    if crate::settings::TOOLBAR_SETTINGS {
        items.push(settings_item());
    }
    items
}

fn wide_uikit() -> bool {
    cfg!(feature = "uikit")
        && day::size_class().is_some_and(|size| size.width != WidthClass::Compact)
}

/// A phone, iPhone or Android: the app's status rides a bottom bar there (`bottom_bar`).
fn phone_bottom_bar() -> bool {
    let compact = day::size_class().is_none_or(|size| size.width == WidthClass::Compact);
    (cfg!(feature = "uikit") || cfg!(feature = "material")) && compact
}

fn refresh_item() -> ToolbarEntry {
    crate::commands::refresh()
        .toolbar_item()
        .placement(ToolbarPlacement::Navigation)
}

fn list_controls() -> Vec<ToolbarEntry> {
    vec![
        refresh_item(),
        crate::commands::unread_feeds_only()
            .toolbar_item()
            .placement(ToolbarPlacement::Navigation),
    ]
}

/// On iPad the list owns Refresh and Filter; a phone puts the filter on its bottom bar.
pub fn list_items() -> Vec<ToolbarEntry> {
    if wide_uikit() {
        list_controls()
    } else if cfg!(feature = "appkit") {
        // Over the list on a Mac, where the sidebar's part of the toolbar has no room to spare
        // (`feed_items`).
        let mut items = Vec::new();
        if daynews_core::scene().article.with(|a| a.is_none()) {
            items.push(crate::commands::next_unread().toolbar_item());
            items.push(
                crate::commands::dashboard()
                    .toolbar_item()
                    .placement(ToolbarPlacement::Secondary),
            );
        }
        items.push(new_feed_item().placement(ToolbarPlacement::Secondary));
        items.push(crate::commands::group_by_feeds().toolbar_item());
        items
    } else {
        Vec::new()
    }
}

/// Whether the app's status has a bar of its own, the phone's bottom bar or a desktop window's
/// status bar, so the lists leave out the refresh row and the sync panel they show elsewhere.
/// A desktop window whose status bar is hidden shows neither: hiding it hides the status.
pub fn status_in_bottom_bar() -> bool {
    phone_bottom_bar() || !crate::settings::TOOLBAR_SETTINGS
}

/// A desktop window's status bar, across the bottom of the whole window and shown until View ▸
/// Hide Status Bar: what is unread and when the library was last brought up to date, and
/// while a sync or a refresh runs, its progress instead.
pub fn status_bar() -> impl Piece {
    when(
        || !crate::settings::TOOLBAR_SETTINGS && crate::settings::STATUS_BAR.get(),
        || {
            let st = daynews_core::state();
            let muted = move || crate::theme::palette().text_muted;
            let refreshing = move || !sync_busy() && st.refresh_progress.get().is_some();
            let idle = move || !sync_busy() && st.refresh_progress.get().is_none();
            column((
                divider(),
                // One line of text on the leading side, and the progress and its controls on the
                // trailing side, at one fixed height: a refresh or a sync starting must not move
                // the window's content.
                row((
                    when(idle, move || {
                        label(move || {
                            let feeds = st.feeds.with(|f| f.len());
                            format!(
                                "{} · {}",
                                res::str::unread_count(st.total_unread.get() as f64).format(),
                                res::str::status_bar_feeds(feeds as f64).format()
                            )
                        })
                        .font(Font::Caption)
                        .single_line()
                        .color(muted)
                        .id(day::id!(status_bar_summary))
                    })
                    .otherwise(move || {
                        label(move || sync_activity().or_else(refresh_text).unwrap_or_default())
                            .font(Font::Caption)
                            .single_line()
                            .color(muted)
                            .id(day::id!(status_bar_activity))
                    }),
                    spacer().grow(Dimension::Width),
                    when(sync_busy, sync_controls),
                    when(refreshing, move || {
                        progress(move || {
                            st.refresh_progress
                                .get()
                                .map_or(0.0, |(done, total)| done as f64 / total.max(1) as f64)
                        })
                        .width(PROGRESS_WIDTH)
                        .id(day::id!(status_bar_progress))
                    }),
                    when(idle, move || {
                        label(move || {
                            pending_sync_text()
                                .or_else(|| {
                                    last_updated()
                                        .map(|at| crate::format::updated_status(at, clock().get()))
                                })
                                .unwrap_or_default()
                        })
                        .font(Font::Caption)
                        .single_line()
                        .color(muted)
                        .id(day::id!(status_bar_updated))
                    }),
                ))
                .spacing(Space::S)
                .align(VAlign::Center)
                .padding(Insets::symmetric(12.0, 0.0))
                .height(STATUS_BAR_HEIGHT)
                .grow(Dimension::Width),
            ))
            .background(move || crate::theme::palette().bg_alt)
            .id(day::id!(status_bar))
            .grow(Dimension::Width)
        },
    )
}

/// The status bar's height, the same idle or busy.
const STATUS_BAR_HEIGHT: f64 = 24.0;
const PROGRESS_WIDTH: f64 = 140.0;

/// A sync's timing, progress and the way to stop it, at the trailing end of the status bar.
#[cfg(not(target_arch = "wasm32"))]
fn sync_controls() -> impl Piece {
    let status = daynews_core::sync_status();
    let fraction = move || status.get().progress.as_ref().and_then(|p| p.fraction());
    row((
        label(move || crate::accounts::progress_timing(&status.get()))
            .font(Font::Caption)
            .single_line()
            .color(move || crate::theme::palette().text_muted),
        when(
            move || fraction().is_some(),
            move || {
                progress(move || fraction().unwrap_or(0.0))
                    .width(PROGRESS_WIDTH)
                    .id(day::id!(status_bar_progress))
            },
        )
        .otherwise(|| spinner().frame(14.0, 14.0)),
        button(res::str::sync_cancel())
            .image(res::vectors::cancel)
            .icon_only()
            .plain()
            .action(daynews_core::sync_cancel)
            .enabled(move || status.get().cancellable)
            .id(day::id!(status_cancel_sync)),
    ))
    .spacing(Space::S)
    .align(VAlign::Center)
}
#[cfg(target_arch = "wasm32")]
fn sync_controls() -> impl Piece {
    spacer()
}

#[cfg(not(target_arch = "wasm32"))]
fn sync_busy() -> bool {
    let status = daynews_core::sync_status().get();
    status.busy && status.progress.is_some()
}
#[cfg(target_arch = "wasm32")]
fn sync_busy() -> bool {
    false
}

fn pending_sync_text() -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    if daynews_core::refresh_sync_pending() {
        return Some(res::str::library_sync_pending().format());
    }
    None
}

fn refresh_text() -> Option<String> {
    daynews_core::state()
        .refresh_progress
        .get()
        .map(|(done, total)| {
            if total == 0 {
                return res::str::refresh_preparing().format();
            }
            res::str::refresh_progress(res::str::RefreshProgress {
                done: done as f64,
                total: total as f64,
            })
            .format()
        })
}

/// Up to date as of the newer of a feed's own check and the account's last sync.
fn last_updated() -> Option<i64> {
    daynews_core::state()
        .feeds
        .with(|feeds| feeds.iter().filter_map(|f| f.last_fetched).max())
        .max(last_sync())
}

/// A phone's one status bar, Mail's (docs/toolbars.md "Placement"): the unread filter at the
/// leading edge, the app's status in the middle, and at the trailing edge Cancel while a sync
/// can be stopped, otherwise Mark All as Read over the article list or New Feed over the feeds.
/// Declared on the navigation, so the feeds page and the article list share it; the reader has
/// its own commands and no bottom bar.
fn bottom_bar(list_showing: bool) -> Vec<ToolbarEntry> {
    // Four round icons drawn on one grid (resource/vectors), so the bar's buttons match in size
    // and weight whichever pair is showing.
    let trailing = if sync_cancellable() {
        // The round ✕, not the word: the status beside it needs the width.
        toolbar_button(day::id!(status_cancel_sync), res::str::sync_cancel())
            .image(res::vectors::cancel_round)
            .action(cancel_sync)
    } else if list_showing {
        crate::commands::mark_all_read()
            .toolbar_item()
            .image(res::vectors::mark_all_read)
    } else {
        new_feed_item().image(res::vectors::new_feed_round)
    };
    // NetNewsWire's filter: the icon fills in while the list is filtered, rather than UIKit's
    // selected background, which reads as a pressed square on a bar of plain icons.
    let filtering = crate::feed_list::FeedList::app().unread_only.get();
    let filter = toolbar_button(
        day::id!(unread_feeds_only),
        if filtering {
            res::str::show_all_feeds()
        } else {
            res::str::show_unread_feeds_only()
        },
    )
    .image(if filtering {
        res::vectors::feed_filter_on
    } else {
        res::vectors::feed_filter
    })
    .action(crate::feed_list::toggle);
    vec![
        filter.placement(ToolbarPlacement::Bottom),
        // Read here, not in a label closure: this builder is what reruns when they change.
        toolbar_label(day::id!(list_status), status(list_showing, clock().get()))
            .placement(ToolbarPlacement::Bottom),
        trailing.placement(ToolbarPlacement::Bottom),
    ]
}

/// The status, one line over the other: what the app is doing (a sync's step, a refresh's
/// progress) or when the library was last brought up to date, then what is unread, in the
/// list when one is showing and in every feed otherwise.
fn status(list_showing: bool, now: i64) -> String {
    let st = daynews_core::state();
    let unread = if list_showing {
        daynews_core::scene().scope_unread.get() as i64
    } else {
        st.total_unread.get()
    };
    let unread = res::str::unread_count(unread as f64).format();
    let first = sync_activity()
        .or_else(refresh_text)
        .or_else(pending_sync_text)
        .or_else(|| last_updated().map(|at| crate::format::updated_status(at, now)));
    match first {
        Some(first) => format!("{first}\n{unread}"),
        None => unread,
    }
}

/// The running sync's step, as the Accounts settings describe it.
#[cfg(not(target_arch = "wasm32"))]
fn sync_activity() -> Option<String> {
    let status = daynews_core::sync_status().get();
    let progress = status.progress.filter(|_| status.busy)?;
    Some(crate::accounts::progress_text(&progress))
}
#[cfg(target_arch = "wasm32")]
fn sync_activity() -> Option<String> {
    None
}

#[cfg(not(target_arch = "wasm32"))]
fn last_sync() -> Option<i64> {
    daynews_core::sync_status().get().last_sync
}
#[cfg(target_arch = "wasm32")]
fn last_sync() -> Option<i64> {
    None
}

#[cfg(not(target_arch = "wasm32"))]
fn sync_cancellable() -> bool {
    let status = daynews_core::sync_status().get();
    status.busy && status.cancellable
}
#[cfg(target_arch = "wasm32")]
fn sync_cancellable() -> bool {
    false
}

fn cancel_sync() {
    #[cfg(not(target_arch = "wasm32"))]
    daynews_core::sync_cancel();
}

/// The minute, for "Updated 5 minutes ago": it ages with the clock, not only with the data.
fn clock() -> Signal<i64> {
    thread_local! {
        static CLOCK: std::cell::OnceCell<Signal<i64>> = const { std::cell::OnceCell::new() };
    }
    CLOCK.with(|slot| {
        *slot.get_or_init(|| {
            let root = day::reactive::Scope::root();
            let clock = Signal::new_in(root, daynews_time::now_unix() / 60 * 60);
            root.enter(|| {
                day::task(async move {
                    loop {
                        day::sleep(15_000).await;
                        clock.set_if_changed(daynews_time::now_unix() / 60 * 60);
                    }
                })
            });
            clock
        })
    })
}

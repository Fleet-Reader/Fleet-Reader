// SPDX-License-Identifier: Apache-2.0
//! Settings and About (https://daybrite.dev/docs/reference/settings): the typed, persisted preferences
//! the rest of the app reads, and the declaration of the groups Day's shared surface presents.
//! The surface itself is Day's: the Settings window and the App menu's Settings…/About items on
//! a desktop, the Settings section's pages behind the sidebar item on a phone and the web, and
//! the About window or header card. Every value is a [`Pref`]: one key in the platform store,
//! one signal, read from anywhere with `.get()` and written by its row or by a command.

use crate::res;
use day::prelude::*;
use day_piece_settings::prelude::*;

/// Mobile Settings is reached from the toolbar and presented by Day as a native cover.
pub const TOOLBAR_SETTINGS: bool = cfg!(any(
    feature = "uikit",
    feature = "material",
    feature = "arkui"
));

/// The project site the About window links to.
const WEBSITE: &str = "https://fleet-reader.github.io/Fleet-Reader";
/// The repository the About window links to.
const SOURCE: &str = "https://github.com/Fleet-Reader/Fleet-Reader";

// ---------------------------------------------------------------------------------------------
// Articles
// ---------------------------------------------------------------------------------------------

/// Whether the timeline shows a heading per feed (the Feed menu toggles it too).
pub static GROUP_BY_FEED: Pref<bool> = Pref::new("news.articles.group_by_feed", || true);
/// How the overview's word cloud sets its words: `"2d"`, across and up (the default), or
/// `"1d"`, across only.
pub static WORD_CLOUD_LAYOUT: Pref<String> =
    Pref::new("news.dashboard.word_cloud_layout", || WORD_CLOUD_2D.into());
pub const WORD_CLOUD_2D: &str = "2d";
pub const WORD_CLOUD_1D: &str = "1d";
/// The word cloud's typeface: one of [`WORD_CLOUD_FONTS`] by family name, or empty for the
/// system face.
pub static WORD_CLOUD_FONT: Pref<String> = Pref::new("news.dashboard.word_cloud_font", String::new);
/// The display faces bundled for the word cloud (resource/fonts/licenses/README.md).
pub const WORD_CLOUD_FONTS: [day::FontFamily; 9] = [
    res::fonts::abril_fatface,
    res::fonts::amatic_sc,
    res::fonts::bungee,
    res::fonts::chewy,
    res::fonts::lobster,
    res::fonts::pacifico,
    res::fonts::permanent_marker,
    res::fonts::righteous,
    res::fonts::special_elite,
];
/// Whether a desktop window shows its status bar along the bottom (View ▸ Show Status Bar).
pub static STATUS_BAR: Pref<bool> = Pref::new("news.window.status_bar", || true);
/// Show synchronization beneath the article list and the compact mobile feeds list.
#[cfg(not(target_arch = "wasm32"))]
pub static SHOW_SYNC_STATUS: Pref<bool> = Pref::new("news.articles.show_sync_status", || true);
/// The article list's text size, in percent of the platform's body size.
pub static LIST_SCALE: Pref<f64> = Pref::new("news.list.scale", || DEFAULT_LIST_SCALE);
pub const DEFAULT_LIST_SCALE: f64 = 120.0;
pub const MIN_LIST_SCALE: f64 = 70.0;
pub const MAX_LIST_SCALE: f64 = 250.0;
/// Preview lines under each row's title, none to five.
pub static PREVIEW_LINES: Pref<u32> =
    Pref::new("news.list.preview_lines", || DEFAULT_PREVIEW_LINES);
pub const DEFAULT_PREVIEW_LINES: u32 = 2;
pub const MAX_PREVIEW_LINES: u32 = 5;

// ---------------------------------------------------------------------------------------------
// Reader
// ---------------------------------------------------------------------------------------------

/// The reader's text size, in percent; Article ▸ Increase/Decrease Text Size step it by ten.
pub static READER_SCALE: Pref<f64> =
    Pref::new("news.reader.scale", || crate::reader_styles::DEFAULT_SCALE);
/// The reader's font family; empty for the platform's system font.
pub static READER_FONT: Pref<String> = Pref::new("news.reader.font", String::new);
/// The reader's page color as `#rrggbb`; empty follows the appearance.
pub static READER_BACKGROUND: Pref<String> = Pref::new("news.reader.background", String::new);
/// The reader's text color as `#rrggbb`; empty follows the appearance.
pub static READER_TEXT: Pref<String> = Pref::new("news.reader.text", String::new);
/// Full content loads under the feed preview (on) or replaces it (off). Per-feed overrides
/// live in `reader_options`.
pub static READER_INLINE: Pref<bool> = Pref::new("news.reader.inline", || true);
/// Full content loads on its own when the load bar scrolls into view.
pub static READER_AUTO: Pref<bool> = Pref::new("news.reader.auto", || false);

// ---------------------------------------------------------------------------------------------
// Browser
// ---------------------------------------------------------------------------------------------

/// Where web links open: empty for the embedded browser (the default, which shares each site's
/// sign-in with the reader's Load Full Article), [`SYSTEM_BROWSER`] for the system default, or
/// an application id. Without an embedded browser, empty means the system default.
pub static BROWSER: Pref<String> = Pref::new("news.browser", String::new);
/// The [`BROWSER`] value for the system's default browser.
pub const SYSTEM_BROWSER: &str = "system";
/// Private browser storage for site previews and full-content loads.
pub static INCOGNITO: Pref<bool> = Pref::new("news.browser.incognito", || false);

// ---------------------------------------------------------------------------------------------
// Feeds
// ---------------------------------------------------------------------------------------------

/// The default collection, where feeds in no collection live: its name, empty for the
/// localized "Feeds".
pub static DEFAULT_COLLECTION_NAME: Pref<String> =
    Pref::new("news.collections.default.name", String::new);
/// Its place among the collections' positions; by default after every collection.
pub static DEFAULT_COLLECTION_POSITION: Pref<f64> =
    Pref::new("news.collections.default.position", || {
        DEFAULT_COLLECTION_LAST
    });
pub const DEFAULT_COLLECTION_LAST: f64 = 1e9;
/// How it sorts its feeds: a `FeedSort` key, empty for Priority.
pub static DEFAULT_COLLECTION_SORT: Pref<String> =
    Pref::new("news.collections.default.sort", String::new);

/// Days a read article stays before the launch-time prune removes it; 0 keeps everything.
pub static RETENTION_DAYS: Pref<u32> =
    Pref::new("retention.days", || daynews_core::DEFAULT_RETENTION_DAYS);
/// The retention choices, in days; `0` keeps everything.
const RETENTION_CHOICES: [u32; 5] = [30, 90, 180, 365, 0];

/// Declare and install the surface, then wire the prefs that drive state outside the UI: the
/// timeline's grouping flag, the retention prune and the refresh timer. Returns the handle the
/// navigation uses for its Settings section on the platforms without a menu bar.
pub fn install() -> Prefs {
    // Articles the last prune removed, reported under the retention row.
    let pruned = Signal::global(0usize);
    let prefs = Preferences::new()
        .about(
            About::from_manifest()
                .icon(res::images::app_logo)
                .link(res::str::day_about_website(), WEBSITE)
                .link(res::str::day_about_source(), SOURCE),
        )
        .group(general_group(res::locales::ALL))
        .group(articles_group())
        .group(reader_group())
        .group(browser_group())
        .group(feeds_group(pruned));
    #[cfg(not(target_arch = "wasm32"))]
    let prefs = prefs.group(
        Group::new("accounts", res::str::settings_accounts())
            .icon(res::vectors::manage_accounts)
            .custom(crate::accounts::page),
    );
    let prefs = prefs.install();
    // These outlive any window, like the prefs' own persistence watches.
    day::reactive::Scope::root().enter(|| {
        // daynews-core publishes the grouping flag the timeline reads; it has no view of the
        // store, so the pref feeds it.
        let st = daynews_core::state();
        st.group_by_feed.set(GROUP_BY_FEED.get_untracked());
        GROUP_BY_FEED.watch(move |on| st.group_by_feed.set(*on));
        #[cfg(not(target_arch = "wasm32"))]
        daynews_core::set_sync_retention_days(RETENTION_DAYS.get_untracked());
        // Retention: prune at launch per the stored window, and again on every change so the
        // choice visibly acts.
        day::task(async {
            daynews_core::prune(RETENTION_DAYS.get_untracked()).await;
        });
        RETENTION_DAYS.watch(move |days| {
            let days = *days;
            #[cfg(not(target_arch = "wasm32"))]
            daynews_core::set_sync_retention_days(days);
            day::task(async move {
                pruned.set(daynews_core::prune(days).await);
            });
        });
        crate::refresh_schedule::install();
    });
    prefs
}

fn articles_group() -> Group {
    let lines = (0..=MAX_PREVIEW_LINES).map(|n| {
        let label = if n == 0 {
            res::str::settings_preview_none()
        } else {
            res::str::settings_preview_lines_count(f64::from(n))
        };
        (n, label)
    });
    let group = Group::new("articles", res::str::settings_articles())
        .icon(res::vectors::dashboard_all)
        .row(
            rows::toggle(&GROUP_BY_FEED, res::str::group_by_feeds())
                .id(day::id!(group_by_feeds_picker)),
        )
        .row(
            rows::slider(
                &LIST_SCALE,
                res::str::settings_list_size(),
                MIN_LIST_SCALE..=MAX_LIST_SCALE,
            )
            .step(10.0)
            .caption(|| res::str::settings_list_percent(LIST_SCALE.get()).format())
            .id(day::id!(list_size_slider)),
        )
        .row(
            rows::choice(&PREVIEW_LINES, res::str::settings_preview_lines(), lines)
                .id(day::id!(preview_lines_picker)),
        )
        .row(
            rows::choice(
                &WORD_CLOUD_LAYOUT,
                res::str::settings_word_cloud_layout(),
                [
                    (
                        WORD_CLOUD_2D.to_owned(),
                        res::str::settings_word_cloud_2d().format(),
                    ),
                    (
                        WORD_CLOUD_1D.to_owned(),
                        res::str::settings_word_cloud_1d().format(),
                    ),
                ],
            )
            .id(day::id!(word_cloud_layout_picker)),
        )
        .row(
            rows::choice(
                &WORD_CLOUD_FONT,
                res::str::settings_word_cloud_font(),
                std::iter::once((
                    String::new(),
                    res::str::settings_word_cloud_system_font().format(),
                ))
                .chain(
                    WORD_CLOUD_FONTS
                        .iter()
                        .map(|family| (family.as_str().to_owned(), family.as_str().to_owned())),
                ),
            )
            .id(day::id!(word_cloud_font_picker)),
        );
    // A desktop window reports synchronization in its status bar; the phones and tablets
    // show it beneath their lists.
    if !TOOLBAR_SETTINGS {
        return group.row(
            rows::toggle(&STATUS_BAR, res::str::settings_show_status_bar())
                .id(day::id!(show_status_bar)),
        );
    }
    #[cfg(not(target_arch = "wasm32"))]
    let group = group.row(
        rows::toggle(&SHOW_SYNC_STATUS, res::str::settings_show_sync_status())
            .id(day::id!(show_sync_status)),
    );
    group
}

fn reader_group() -> Group {
    use crate::reader_styles::{self as styles, ReaderColor};
    let (mut families, _recommended) = styles::font_choices(
        day::font_families()
            .iter()
            .map(|f| f.family.clone())
            .collect(),
    );
    // A persisted family can disappear after uninstalling a font. Keep it selectable and let
    // CSS use its system fallback until the user picks another family or resets styles.
    let stored = READER_FONT.get_untracked();
    if !stored.is_empty() && !families.contains(&stored) {
        families.push(stored);
    }
    let fonts: Vec<(String, String)> = families
        .into_iter()
        .map(|family| {
            let label = if family.is_empty() {
                res::str::settings_reader_default_font().format()
            } else {
                family.clone()
            };
            (family, label)
        })
        .collect();
    // Incognito disables the full-content defaults, as it does the per-feed overrides.
    let editable = Signal::global(!INCOGNITO.get_untracked());
    day::reactive::Scope::root().enter(|| INCOGNITO.watch(move |private| editable.set(!private)));
    Group::new("reader", res::str::settings_reader())
        .icon(Symbol::Document)
        .section(
            Section::new()
                .row(
                    rows::slider(
                        &READER_SCALE,
                        res::str::settings_reader_size(),
                        styles::MIN_SCALE..=styles::MAX_SCALE,
                    )
                    .step(10.0)
                    .caption(|| res::str::settings_reader_percent(READER_SCALE.get()).format())
                    .id(day::id!(reader_size_slider)),
                )
                .row(
                    rows::choice(&READER_FONT, res::str::settings_reader_font(), fonts)
                        .id(day::id!(reader_font_picker)),
                )
                // Notes go in footers: a long caption widens the title column and pushes the
                // control past the panel's edge.
                .footer(res::str::settings_reader_font_note()),
        )
        .section(
            Section::new()
                .row(color_row(
                    res::str::settings_reader_background,
                    ReaderColor::Background,
                    day::id!(reader_background_color),
                ))
                .row(color_row(
                    res::str::settings_reader_text,
                    ReaderColor::Text,
                    day::id!(reader_text_color),
                )),
        )
        .section(
            Section::new()
                .row(
                    rows::action(res::str::settings_reader_reset(), styles::reset)
                        .id(day::id!(reader_reset_styles)),
                )
                .footer(res::str::settings_reader_reset_note()),
        )
        .section(
            Section::new()
                .row(
                    rows::toggle(&READER_INLINE, res::str::reader_inline_mode())
                        .enabled(editable)
                        .id(day::id!(inline_reader_picker)),
                )
                .footer(res::str::reader_inline_mode_note()),
        )
        .section(
            Section::new()
                .row(
                    rows::toggle(&READER_AUTO, res::str::reader_auto_mode())
                        .enabled(editable)
                        .id(day::id!(auto_reader_picker)),
                )
                .footer(res::str::reader_auto_mode_note()),
        )
        // The app script and the site-script manager, with the editor sheet they open.
        .section(Section::new().row(rows::custom(crate::reader_scripts::settings_controls)))
}

/// A color well row: the title leading, the platform's color picker trailing. The crate has no
/// color row yet, so this composes one from the colorpicker piece over a hex `Pref<String>`.
fn color_row(
    title: fn() -> day::LocalizedText,
    color: crate::reader_styles::ReaderColor,
    key: day::ElementId,
) -> Row {
    rows::custom(move || {
        row((
            label(title()),
            spacer(),
            day::colorpicker::color_picker(color)
                .title(title())
                .id(key.clone()),
        ))
        .spacing(Space::M)
        .align(VAlign::Center)
        .grow(Dimension::Width)
    })
}

fn browser_group() -> Group {
    // The embedded browser comes first where there is one. Discovery runs once per launch;
    // macOS lists the other browsers, the other platforms offer only the system's.
    let embedded = day_piece_webview::support() == Support::Native;
    let mut options = Vec::new();
    if embedded {
        options.push((
            String::new(),
            res::str::settings_browser_embedded().format(),
        ));
    }
    let system = if embedded { SYSTEM_BROWSER } else { "" }.to_owned();
    let found = day::application_handlers(&day::HandlerQuery::Scheme("https".into()));
    match &found {
        Ok(handlers) => {
            let label = match &handlers.default {
                Some(app) => res::str::settings_browser_default(app.name.as_str()).format(),
                None => res::str::settings_browser_system().format(),
            };
            options.push((system, label));
            options.extend(
                handlers
                    .applications
                    .iter()
                    .map(|app| (app.id.clone(), app.name.clone())),
            );
        }
        Err(_) => options.push((system, res::str::settings_browser_system().format())),
    }
    let links = if options.len() > 1 {
        Section::new()
            .row(
                rows::choice(&BROWSER, res::str::settings_browser_label(), options)
                    .id(day::id!(browser_picker)),
            )
            .footer(if embedded {
                res::str::settings_browser_embedded_note()
            } else {
                res::str::settings_browser_note()
            })
    } else {
        Section::new()
            .row(rows::info(
                res::str::settings_browser_label(),
                res::str::settings_browser_system(),
            ))
            .footer(res::str::settings_browser_unavailable())
    };
    let mut privacy = Section::new().row(
        rows::toggle(&INCOGNITO, res::str::incognito_mode()).id(day::id!(incognito_mode_picker)),
    );
    if day_piece_webview::private_browsing_support() == Support::Unsupported {
        privacy = privacy.footer(res::str::site_private_unsupported());
    }
    Group::new("browser", res::str::settings_browser())
        .icon(res::vectors::open_browser)
        .section(links)
        .section(privacy)
}

fn feeds_group(pruned: Signal<usize>) -> Group {
    use crate::refresh_schedule::{REFRESH, Refresh};
    let retention = RETENTION_CHOICES.map(|days| {
        let label = match days {
            30 => res::str::retention_30(),
            90 => res::str::retention_90(),
            180 => res::str::retention_180(),
            365 => res::str::retention_365(),
            _ => res::str::retention_forever(),
        };
        (days, label)
    });
    Group::new("feeds", res::str::settings_feeds())
        .icon(Symbol::Refresh)
        .section(
            Section::new()
                .title(res::str::settings_collections())
                .row(rows::custom(crate::feed_list::collections_editor))
                .footer(res::str::settings_collections_note()),
        )
        .section(
            Section::new()
                .row(
                    rows::choice(
                        &REFRESH,
                        res::str::settings_refresh_feeds(),
                        [
                            (Refresh::Automatic, res::str::settings_refresh_automatic()),
                            (Refresh::Manual, res::str::settings_refresh_manual()),
                            (Refresh::Minutes30, res::str::settings_refresh_30()),
                            (Refresh::Hour, res::str::settings_refresh_60()),
                            (Refresh::Hours2, res::str::settings_refresh_120()),
                            (Refresh::Hours4, res::str::settings_refresh_240()),
                            (Refresh::Hours8, res::str::settings_refresh_480()),
                        ],
                    )
                    .id(day::id!(refresh_interval_picker)),
                )
                .footer(res::str::settings_refresh_automatic_note()),
        )
        .section(
            Section::new()
                .row(
                    rows::choice(
                        &RETENTION_DAYS,
                        res::str::settings_retention_label(),
                        retention,
                    )
                    .id(day::id!(retention_picker)),
                )
                .footer(move || {
                    let note = res::str::settings_retention_note().format();
                    match pruned.get() {
                        0 => note,
                        n => format!("{note} {}", res::str::retention_pruned(n as f64).format()),
                    }
                }),
        )
        .section(
            Section::new()
                .row(
                    rows::action(res::str::opml_import(), crate::subscriptions::import_opml)
                        .id(day::id!(opml_import)),
                )
                .row(
                    rows::action(res::str::opml_export(), crate::subscriptions::export_opml)
                        .id(day::id!(opml_export)),
                )
                // The import and export outcomes land on the app-wide status line.
                .footer(move || daynews_core::state().status.get()),
        )
}

/// Open a link in the chosen browser, or the system's. Browser overrides apply only to web
/// links; mail, PDF/file and other schemes retain their own system associations.
pub fn open_link(url: &str) {
    let web_link = url.split_once(':').is_some_and(|(scheme, _)| {
        scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")
    });
    let chosen = BROWSER.get_untracked();
    if web_link && !chosen.is_empty() && chosen != SYSTEM_BROWSER {
        let url = url.to_owned();
        day::task(async move {
            if day::open_url_with_application(&url, &chosen).await.is_err() {
                // Removed/moved applications must never strand a link inside the reader.
                open_url(&url);
            }
        });
    } else {
        open_url(url);
    }
}

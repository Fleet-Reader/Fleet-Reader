//! A viewport-sized native dashboard. Chart data is a worker-owned metadata projection;
//! only the visible countdown changes each second, never the database or the chart marks.
use crate::{res, theme::palette};
use day::prelude::*;
use day_core::{Boundary, Flex, Layout, LayoutOps};
use day_piece_charts::{
    Coordinate, LegendPosition, Mark, area, bar, chart, line, sector, time, value,
};
use day_spec::{Affine, Point, Proposal, Rect};
use daynews_db::{Scope, dashboard::Dashboard};
use std::rc::Rc;

type Data = Signal<Option<Dashboard>>;
/// A card's title line; its color follows the live palette, so it stays a closure.
const HEADLINE: TextPreset = TextPreset::new(Font::Headline).single_line();
/// A card's one-line caption; the muted color follows the live palette.
const CAPTION: TextPreset = TextPreset::new(Font::Caption).single_line();
fn read<T: Default>(data: Data, f: impl FnOnce(&Dashboard) -> T) -> T {
    data.with(|snapshot| snapshot.as_ref().map(f).unwrap_or_default())
}
fn feed_page_url(data: Data) -> Option<String> {
    read(data, |d| {
        d.site_url
            .clone()
            .filter(|url| !url.trim().is_empty())
            .or_else(|| d.feed_url.clone().filter(|url| !url.trim().is_empty()))
    })
}
fn title(scope: Scope, data: Data) -> String {
    match scope {
        Scope::Feed(_) => read(data, |d| d.title.clone()),
        Scope::Today => res::str::nav_today().format(),
        Scope::Unread => res::str::nav_all_unread().format(),
        Scope::Starred => res::str::nav_starred().format(),
        Scope::All => res::str::nav_all_articles().format(),
        Scope::Folder(id) => daynews_core::state().folders.with(|f| {
            f.iter()
                .find(|f| f.id == id)
                .map(|f| f.name.clone())
                .unwrap_or_default()
        }),
        Scope::Tag(id) => daynews_core::state().tags.with(|f| {
            f.iter()
                .find(|f| f.id == id)
                .map(|f| f.name.clone())
                .unwrap_or_default()
        }),
    }
}
fn accent(scope: Scope) -> Color {
    match scope {
        Scope::Starred => palette().star,
        Scope::Today => Color::hex(if day::dark_mode() { 0x65D7BF } else { 0x168A76 }),
        Scope::Unread => Color::hex(if day::dark_mode() { 0xB49BFF } else { 0x7751C8 }),
        _ => palette().accent,
    }
}
fn card<P: Piece>(content: P) -> impl Piece {
    // Keep decoration behind the card's controls. A canvas overlay covers native hit
    // testing even when it paints only a thin border.
    content
        .padding(Space::M)
        .background(move || palette().bg_alt)
        .corner_radius(13.0)
        .padding(1.0)
        .background(move || palette().rule.with_alpha(0.65))
        .corner_radius(14.0)
}

fn metric(
    label_text: impl Fn() -> String + 'static,
    amount: impl Fn() -> String + 'static,
    scope: Signal<Scope>,
) -> AnyPiece {
    column((
        label(amount)
            .font(Font::Title2)
            .weight(FontWeight::Bold)
            .color(move || accent(scope.get()))
            .single_line(),
        label(label_text)
            .preset(&CAPTION)
            .color(move || palette().text_muted),
    ))
    .spacing(3.0)
    .align(HAlign::Leading)
    .grow_w()
    .any()
}
fn age_labels() -> [String; 5] {
    [
        res::str::dashboard_age_today().format(),
        res::str::dashboard_age_week().format(),
        res::str::dashboard_age_month().format(),
        res::str::dashboard_age_quarter().format(),
        res::str::dashboard_age_old().format(),
    ]
}
fn count_axis(config: &mut day_piece_charts::ChartConfig, max: u64) {
    let max = max.max(1);
    config.y_scale.domain = Some(day_piece_charts::Interval::new(0.0, max as f64));
    config.y_axis.format = Some(Rc::new(|v| {
        res::str::dashboard_number(v.as_continuous().unwrap_or(0.0)).format()
    }));
    config.y_axis.values = Some(if max <= 1 {
        vec![0.0, 1.0]
    } else {
        vec![0.0, max.div_ceil(2) as f64, max as f64]
    });
}
fn activity(data: Data, scope: Scope) -> Vec<Mark> {
    let count = res::str::dashboard_chart_count().format();
    if scope == Scope::Unread {
        return age_labels()
            .into_iter()
            .zip(read(data, |d| d.ages))
            .map(|(age, n)| {
                bar(value("", age), value(&count, n))
                    .foreground(accent(scope))
                    .corner_radius(5.0)
            })
            .collect();
    }
    if scope == Scope::Today {
        return read(data, |d| d.hourly)
            .into_iter()
            .enumerate()
            .map(|(hour, n)| {
                bar(
                    value("", res::str::dashboard_chart_hour(hour as f64).format()),
                    value(&count, n),
                )
                .foreground(accent(scope))
                .corner_radius(3.0)
            })
            .collect();
    }
    let now = daynews_time::now_unix();
    let local_now = now + i64::from(daynews_time::local_offset_seconds(now).unwrap_or(0));
    let earliest = read(data, |d| d.days.first().map(|d| d.0)).unwrap_or(local_now);
    let span = ((local_now - earliest).div_euclid(86400) + 1).clamp(28, 366);
    let width = (span + 13) / 14;
    let end = local_now.div_euclid(86400) * 86400;
    let start = end - (width * 14 - 1) * 86400;
    let mut bins = [0_u64; 14];
    data.with(|snapshot| {
        if let Some(snapshot) = snapshot {
            for &(date, n) in &snapshot.days {
                let index = (date - start).div_euclid(width * 86400);
                if (0..14).contains(&index) {
                    bins[index as usize] += n;
                }
            }
        }
    });
    bins.into_iter()
        .enumerate()
        .flat_map(|(i, n)| {
            let date = (start + (i as i64 * width + width / 2) * 86400) as f64;
            [
                area(time("", date), value(&count, n))
                    .foreground(accent(scope))
                    .opacity(0.18),
                line(time("", date), value(&count, n))
                    .foreground(accent(scope))
                    .line_width(2.8),
            ]
        })
        .collect()
}
fn activity_card(data: Data, scope: Signal<Scope>) -> AnyPiece {
    let selection = Signal::new(None);
    card(
        column((
            label(move || match scope.get() {
                Scope::Today => res::str::dashboard_activity_today().format(),
                Scope::Unread => res::str::dashboard_activity_unread().format(),
                Scope::Starred => res::str::dashboard_activity_starred().format(),
                _ => res::str::dashboard_activity().format(),
            })
            .preset(&HEADLINE)
            .color(move || palette().text),
            label(move || match scope.get() {
                Scope::Today => res::str::dashboard_today_note().format(),
                Scope::Unread => res::str::dashboard_unread_note().format(),
                _ => {
                    let now = daynews_time::now_unix();
                    let now = now + i64::from(daynews_time::local_offset_seconds(now).unwrap_or(0));
                    let start =
                        read(data, |d| d.days.first().map(|d| d.0)).unwrap_or(now - 27 * 86400);
                    res::str::dashboard_activity_note(res::str::DashboardActivityNote {
                        end: now as f64,
                        start: start as f64,
                    })
                    .format()
                }
            })
            .preset(&CAPTION)
            .color(move || palette().text_muted),
            chart(move || activity(data, scope.get()))
                .animated()
                .animate_appearance()
                .legend(LegendPosition::Hidden)
                .label_size(10.0)
                .configure(move |config| {
                    let max = activity(data, scope.get())
                        .iter()
                        .filter_map(|mark| mark.y.as_ref().and_then(|y| y.datum.as_continuous()))
                        .fold(0.0_f64, f64::max) as u64;
                    count_axis(config, max);
                    config.x_axis.desired_count = 4;
                    config.x_axis.format = if matches!(scope.get(), Scope::Today | Scope::Unread) {
                        None
                    } else {
                        Some(Rc::new(|v| {
                            res::str::dashboard_chart_date(v.as_continuous().unwrap_or(0.0))
                                .format()
                        }))
                    };
                })
                .interact(
                    day_piece_charts::Inspect::new(selection)
                        .snap(day_piece_charts::Snap::NearestX)
                        .guides(day_piece_charts::Guides::RULE),
                )
                .id(day::id!(dashboard_activity_chart))
                .grow(),
        ))
        .spacing(5.0)
        .align(HAlign::Leading)
        .grow(),
    )
    .any()
}
fn sources(data: Data) -> Vec<(u64, String, u64)> {
    let mut sources = read(data, |d| d.sources.clone());
    if sources.len() > 4 {
        let other = sources.drain(4..).map(|s| s.2).sum();
        sources.push((0, res::str::dashboard_other().format(), other));
    }
    sources
}
fn sources_card(data: Data) -> AnyPiece {
    let highlighted =
        day_piece_charts::PointSelection::new().project(day_piece_charts::Projection::Series);
    let links = day_piece_charts::Links::new().on_open(|target| {
        if let Some(id) = target
            .strip_prefix("feed:")
            .and_then(|id| id.parse::<u64>().ok())
        {
            if crate::feed_list::FeedList::app()
                .unread_only
                .get_untracked()
            {
                crate::feed_list::toggle();
            }
            // Changing scopes closes the compact reader. A publisher link opens the
            // target overview, so restore the detail layer after navigation settles.
            let scene = daynews_core::scene();
            batch(|| {
                daynews_core::select_scope(Scope::Feed(id));
                navigate(target);
                day::reactive::at_turn_end(move || scene.reader_open.set(true));
            });
        }
    });
    card(
        column((
            label(res::str::dashboard_sources())
                .preset(&HEADLINE)
                .color(move || palette().text),
            label(res::str::dashboard_sources_note())
                .preset(&CAPTION)
                .color(move || palette().text_muted),
            PublisherPieces(vec![
                chart(move || {
                    sources(data)
                        .into_iter()
                        .map(|(id, _, n)| {
                            let mark = sector(value("", n))
                                .by_series(value("", id.to_string()))
                                .angular_inset(2.0);
                            if id == 0 {
                                mark
                            } else {
                                mark.link(format!("feed:{id}"))
                            }
                        })
                        .collect()
                })
                .coordinate(Coordinate::donut(0.68))
                .animated()
                .animate_appearance()
                .legend(LegendPosition::Hidden)
                .condition(
                    highlighted.predicate(),
                    day_piece_charts::Visual::default(),
                    day_piece_charts::Visual::opacity(0.22),
                )
                .interact(highlighted.on(day_piece_charts::EventSource::Hover))
                .interact(links.clone())
                .id(day::id!(dashboard_publisher_chart))
                .any(),
                day_piece_charts::legend(move || {
                    sources(data)
                        .into_iter()
                        .enumerate()
                        .map(|(i, (id, name, n))| {
                            let entry = day_piece_charts::LegendEntry::new(
                                id.to_string(),
                                name,
                                day_piece_charts::categorical(i),
                            )
                            .detail(res::str::dashboard_number(n as f64).format());
                            if id == 0 {
                                entry
                            } else {
                                entry.link(format!("feed:{id}"))
                            }
                        })
                        .collect()
                })
                .interact(highlighted.on(day_piece_charts::EventSource::Hover))
                .entry_id(|key| day::id!(dashboard_publisher, key))
                .links(links)
                .any(),
            ])
            .grow(),
        ))
        .spacing(3.0)
        .align(HAlign::Leading)
        .grow(),
    )
    .any()
}
fn cadence(seconds: i64) -> String {
    if seconds >= 86400 {
        res::str::dashboard_daily().format()
    } else if seconds < 3600 {
        res::str::dashboard_every_minutes((seconds / 60) as f64).format()
    } else {
        res::str::dashboard_every_hours(seconds as f64 / 3600.0).format()
    }
}
fn next_check(data: Data) -> Option<i64> {
    let (mode, fixed) = crate::refresh_schedule::schedule();
    if mode == 0 {
        return None;
    }
    read(data, |d| {
        d.feeds
            .iter()
            .filter_map(|feed| {
                let deadline = if crate::refresh_schedule::automatic(mode) {
                    feed.next.or(feed
                        .last_checked
                        .map(|last| last.saturating_add(feed.interval)))
                } else {
                    fixed
                };
                deadline.map(|deadline| deadline.max(feed.retry_after.unwrap_or(0)))
            })
            .min()
    })
}
/// The refresh schedule: the cadence, then the next check and its countdown.
fn schedule(data: Data, clock: Signal<i64>) -> AnyPiece {
    column((
        row((
            label(res::str::dashboard_automatic())
                .font(Font::Caption2)
                .weight(FontWeight::Bold)
                .color(move || palette().accent),
            spacer(),
            label(move || {
                let intervals = read(data, |d| {
                    d.feeds.iter().map(|f| f.interval).collect::<Vec<_>>()
                });
                match (intervals.iter().min(), intervals.iter().max()) {
                    (Some(a), Some(b)) if a != b => {
                        res::str::dashboard_interval_range(res::str::DashboardIntervalRange {
                            fast: cadence(*a),
                            slow: cadence(*b),
                        })
                        .format()
                    }
                    (Some(a), _) => cadence(*a),
                    _ => res::str::dashboard_no_schedule().format(),
                }
            })
            .font(Font::Footnote)
            .weight(FontWeight::Semibold)
            .color(move || palette().text)
            .single_line(),
        ))
        .align(VAlign::Center)
        .grow_w(),
        row((
            label(move || match next_check(data) {
                Some(next) => {
                    let local =
                        next + i64::from(daynews_time::local_offset_seconds(next).unwrap_or(0));
                    res::str::dashboard_next(local as f64).format()
                }
                None if crate::refresh_schedule::schedule().0 == 0 => {
                    res::str::dashboard_manual().format()
                }
                _ => res::str::dashboard_no_schedule().format(),
            })
            .preset(&CAPTION)
            .color(move || palette().text_muted)
            .grow_w(),
            label(move || {
                let now = clock.get();
                if daynews_core::state().updating_feeds.with(|active| {
                    read(data, |d| {
                        d.feeds.iter().any(|feed| active.contains_key(&feed.id))
                    })
                }) {
                    return res::str::dashboard_refreshing().format();
                }
                let Some(next) = next_check(data) else {
                    return String::new();
                };
                let remaining = next.saturating_sub(now).max(0);
                if remaining == 0 {
                    return res::str::dashboard_due().format();
                }
                res::str::dashboard_countdown(res::str::DashboardCountdown {
                    hours: (remaining / 3600) as f64,
                    minutes: ((remaining / 60) % 60) as f64,
                    seconds: (remaining % 60) as f64,
                })
                .format()
            })
            .font(Font::Footnote)
            .weight(FontWeight::Semibold)
            .color(move || palette().accent)
            .tabular()
            .id(day::id!(dashboard_refresh_countdown)),
        ))
        .spacing(Space::S)
        .align(VAlign::Center)
        .grow_w(),
    ))
    .spacing(5.0)
    .align(HAlign::Leading)
    .grow_w()
    .any()
}
/// Why the schedule is what it is: backing off, a fixed interval, or the automatic policy.
fn policy(data: Data) -> String {
    if read(data, |d| {
        d.feeds.iter().any(|f| f.failed || f.retry_after.is_some())
    }) {
        res::str::dashboard_backoff().format()
    } else if !crate::refresh_schedule::automatic(crate::refresh_schedule::schedule().0)
        && crate::refresh_schedule::schedule().0 != 0
    {
        res::str::dashboard_fixed().format()
    } else {
        res::str::dashboard_policy().format()
    }
}
/// The library's refresh forecast along the bottom of every overview but a feed's, whose
/// schedule sits in its settings card instead.
fn forecast(data: Data, clock: Signal<i64>) -> AnyPiece {
    card(
        column((
            schedule(data, clock),
            label(move || policy(data))
                .font(Font::Caption2)
                .color(move || palette().text_muted)
                .single_line()
                .grow_w(),
        ))
        .spacing(5.0)
        .align(HAlign::Leading)
        .grow_w(),
    )
    .any()
}

pub fn dashboard() -> impl Piece {
    let scope = daynews_core::scene().scope;
    let data: Data = Signal::new(None);
    let now = daynews_time::now_unix();
    let local_day = Signal::new(daynews_time::start_of_day(now));
    daynews_core::watch_dashboard(scope, local_day, data);
    let clock = Signal::new(now);
    let timer = day::task(async move {
        loop {
            day::sleep(1000).await;
            let now = daynews_time::now_unix();
            clock.set(now);
            local_day.set_if_changed(daynews_time::start_of_day(now));
        }
    });
    day::reactive::Scope::current().on_cleanup(move || timer.abort());
    let hero_text = column((
        label(move || match scope.get() {
            Scope::Today => res::str::dashboard_today_eyebrow().format(),
            Scope::Unread => res::str::dashboard_unread_eyebrow().format(),
            Scope::Starred => res::str::dashboard_starred_eyebrow().format(),
            Scope::Feed(_) => res::str::dashboard_overview().format(),
            _ => res::str::dashboard_library_eyebrow().format(),
        })
        .font(Font::Caption2)
        .weight(FontWeight::Bold)
        .color(move || accent(scope.get()))
        .single_line(),
        label(move || title(scope.get(), data))
            .font(Font::Title)
            .weight(FontWeight::Bold)
            .color(move || palette().text)
            .single_line()
            .id(day::id!(dashboard_title)),
        label(move || match scope.get() {
            Scope::Today => res::str::dashboard_today_intro().format(),
            Scope::Unread => res::str::dashboard_unread_intro().format(),
            Scope::Starred => res::str::dashboard_starred_intro().format(),
            Scope::Feed(_) => read(data, |d| d.description.clone())
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| res::str::dashboard_feed_intro().format()),
            _ => res::str::dashboard_library_intro().format(),
        })
        .font(Font::Footnote)
        // The compact hero must also fit its eyebrow, title and actionable site URL.
        // A second description line can consume the URL's entire height on ArkUI.
        .single_line()
        .color(move || palette().text_muted),
        when(
            move || matches!(scope.get(), Scope::Feed(_)) && feed_page_url(data).is_some(),
            move || {
                label(move || feed_page_url(data).unwrap_or_default())
                    .font(Font::Caption2)
                    .single_line()
                    .color(move || accent(scope.get()))
                    .on_tap(move || {
                        if let Some(url) = feed_page_url(data) {
                            let feed = match scope.get_untracked() {
                                Scope::Feed(feed) => Some(feed),
                                _ => None,
                            };
                            crate::site_browser::open_link(&url, feed);
                        }
                    })
                    .a11y(|b| b.role(Role::Button))
                    .id(day::id!(dashboard_feed_link))
            },
        )
        .otherwise(move || {
            label(move || {
                if data.get().is_none() {
                    return res::str::dashboard_loading().format();
                }
                if read(data, |d| d.total) == 0 {
                    return res::str::dashboard_empty_note().format();
                }
                res::str::dashboard_authors(read(data, |d| d.authors) as f64).format()
            })
            .font(Font::Caption2)
            .single_line()
            .color(move || accent(scope.get()))
        }),
    ))
    .spacing(5.0)
    .align(HAlign::Leading)
    .grow_w()
    .any();
    let hero = row((
        keyed(
            move || {
                let downloaded = match scope.get() {
                    Scope::Feed(id) => {
                        crate::feed_icons::paths().with(|paths| paths.get(&id).cloned())
                    }
                    _ => None,
                };
                Some((scope.get(), downloaded))
            },
            |(scope, path)| {
                let scope = *scope;
                if let Some(path) = path {
                    image(path.clone()).decorative().frame(42.0, 42.0).any()
                } else {
                    vector(match scope {
                        Scope::Today => res::vectors::dashboard_today,
                        Scope::Unread => res::vectors::dashboard_unread,
                        Scope::Starred => res::vectors::dashboard_starred,
                        Scope::Feed(_) => res::vectors::dashboard_feed,
                        _ => res::vectors::dashboard_all,
                    })
                    .tint(move || accent(scope))
                    .decorative()
                    .frame(42.0, 42.0)
                    .any()
                }
            },
        )
        .padding(Space::M)
        .background(move || accent(scope.get()).with_alpha(0.1))
        .corner_radius(18.0),
        hero_text,
    ))
    .spacing(14.0)
    .align(VAlign::Center)
    .grow_w()
    .any();
    let metrics = row((
        metric(
            || res::str::dashboard_articles().format(),
            move || res::str::dashboard_number(read(data, |d| d.total) as f64).format(),
            scope,
        ),
        metric(
            || res::str::dashboard_unread().format(),
            move || res::str::dashboard_number(read(data, |d| d.unread) as f64).format(),
            scope,
        ),
        metric(
            || res::str::dashboard_reading().format(),
            move || {
                res::str::dashboard_minutes(read(data, |d| d.words.div_ceil(240)) as f64).format()
            },
            scope,
        ),
        metric(
            move || {
                if matches!(scope.get(), Scope::Feed(_)) {
                    res::str::dashboard_contributors().format()
                } else {
                    res::str::dashboard_publishers().format()
                }
            },
            move || {
                res::str::dashboard_number(read(data, |d| {
                    if matches!(scope.get(), Scope::Feed(_)) {
                        d.authors
                    } else {
                        d.sources.len()
                    }
                }) as f64)
                .format()
            },
            scope,
        ),
    ))
    .spacing(10.0)
    .align(VAlign::Center)
    .grow_w()
    .any();
    let words = Signal::new(None);
    daynews_core::watch_word_cloud(scope, local_day, words);
    // The regions share the viewport, and scroll once it is too short for the charts to draw
    // (`min_height`): a feed's settings take a short window's room first, and its word cloud
    // the rest.
    scroll(
        DashboardPieces(
            scope,
            vec![
                hero,
                // A feed's own settings, under its name: the first thing to change on its overview.
                keyed(
                    move || match scope.get() {
                        Scope::Feed(id) => Some((id, daynews_core::state().folders.with(Vec::len))),
                        _ => None,
                    },
                    move |&(feed, _)| feed_settings(feed, data, scope, clock),
                )
                .any(),
                metrics,
                when(
                    move || !matches!(scope.get(), Scope::Feed(_)),
                    move || activity_card(data, scope),
                )
                .any(),
                words_card(words, scope),
                when(
                    move || !matches!(scope.get(), Scope::Feed(_)),
                    move || sources_card(data),
                )
                .any(),
                when(
                    move || !matches!(scope.get(), Scope::Feed(_)),
                    move || forecast(data, clock),
                )
                .any(),
            ],
        )
        .padding(18.0),
    )
    .background(move || palette().bg)
    .id(day::id!(feed_dashboard))
    .grow()
}

/// A feed's settings, in one aligned form under its name: two columns of labeled controls where
/// the window is wide enough, one on a phone. Its refresh schedule and its actions (Refresh Now,
/// Unsubscribe) close the same card.
fn feed_settings(feed: u64, data: Data, scope: Signal<Scope>, clock: Signal<i64>) -> AnyPiece {
    let name = || {
        labeled(
            res::str::feed_name_label(),
            button(res::str::rename_feed())
                .action(move || crate::feed_list::rename_feed(feed))
                .id(day::id!(dashboard_rename_feed)),
        )
    };
    let collection = || {
        labeled(
            res::str::collection_menu(),
            crate::feed_list::collection_picker(feed),
        )
    };
    let inline = || {
        labeled(
            res::str::reader_inline_mode(),
            crate::reader_options::inline_toggle(feed),
        )
    };
    let auto = || {
        labeled(
            res::str::reader_auto_mode(),
            crate::reader_options::auto_toggle(feed),
        )
    };
    let script = || {
        labeled(
            res::str::reader_script_label(),
            crate::reader_scripts::feed_controls(feed),
        )
    };
    let site = || {
        labeled(
            res::str::site_label(),
            crate::site_browser::site_controls(feed),
        )
    };
    let compact = day::size_class().is_none_or(|size| size.width == WidthClass::Compact);
    let rows = if compact {
        column((name(), collection(), inline(), auto(), script(), site()))
            .spacing(4.0)
            .any()
    } else {
        grid((
            // The buttons and the picker down the first column, the switches beside them.
            grid_row((name(), inline())),
            grid_row((collection(), auto())),
            grid_row((script(),)),
            // The site's two buttons need the width of both columns.
            grid_row((site().grid_span(2),)),
        ))
        .row_spacing(4.0)
        .column_spacing(28.0)
        .any()
    };
    let actions = row((
        button(res::str::dashboard_refresh_now())
            .action(move || daynews_core::refresh_feed(feed))
            .enabled(move || {
                daynews_core::state().refresh_progress.get().is_none()
                    && scope.get() == Scope::Feed(feed)
            })
            .id(day::id!(dashboard_refresh_now)),
        button(res::str::unsubscribe_feed())
            .action(move || unsubscribe(feed))
            .id(day::id!(dashboard_unsubscribe)),
    ))
    .spacing(Space::S);
    card(
        column((
            form((rows,)).grow_w(),
            schedule(data, clock),
            label(move || policy(data))
                .font(Font::Caption2)
                .color(move || palette().text_muted)
                .grow_w(),
            actions,
        ))
        .spacing(Space::S)
        .align(HAlign::Leading)
        .grow_w(),
    )
    .any()
}

/// Confirm, then unsubscribe from `feed`; the overview moves on to All Unread.
fn unsubscribe(feed: u64) {
    let name = daynews_core::state().feeds.with_untracked(|feeds| {
        feeds
            .iter()
            .find(|f| f.id == feed)
            .map(|f| f.title.clone())
            .unwrap_or_default()
    });
    day::task(async move {
        let confirmed = confirm(res::str::unsubscribe_feed_title(name.as_str()))
            .message(res::str::unsubscribe_feed_message())
            .confirm_label(res::str::unsubscribe())
            .destructive()
            .present()
            .await;
        if confirmed {
            daynews_core::unsubscribe(feed);
        }
    });
}

/// The words of the scope's titles and summaries, most frequent drawn largest. Choosing one
/// searches the selected feed (or smart feed) for it.
fn words_card(
    words: Signal<Option<Vec<daynews_db::dashboard::CloudWord>>>,
    scope: Signal<Scope>,
) -> AnyPiece {
    let hovered: Signal<Option<usize>> = Signal::new(None);
    // The placement for the last words and size drawn: a hover redraws the canvas, and the
    // spiral search is the expensive part.
    let placed: Rc<std::cell::RefCell<Option<CloudKey>>> = Rc::default();
    let at = {
        let placed = placed.clone();
        move |point: Point| {
            placed.borrow().as_ref().and_then(|(_, _, _, words)| {
                words
                    .iter()
                    .find(|w| contains(&w.frame, point))
                    .map(|w| w.index)
            })
        }
    };
    let tap = at.clone();
    card(
        column((
            label(res::str::dashboard_words_title())
                .preset(&HEADLINE)
                .color(move || palette().text),
            label(res::str::dashboard_words_note())
                .preset(&CAPTION)
                .color(move || palette().text_muted),
            when(
                move || words.with(|w| w.as_ref().is_some_and(|w| w.is_empty())),
                move || {
                    label(res::str::dashboard_words_empty())
                        .font(Font::Footnote)
                        .color(move || palette().text_muted)
                },
            ),
            canvas(move |d, size| {
                let Some(list) = words.get() else {
                    return;
                };
                let style = CloudStyle {
                    across_and_up: crate::settings::WORD_CLOUD_LAYOUT.get()
                        != crate::settings::WORD_CLOUD_1D,
                    family: Some(crate::settings::WORD_CLOUD_FONT.get())
                        .filter(|family| !family.is_empty()),
                };
                let mut cache = placed.borrow_mut();
                if cache
                    .as_ref()
                    .is_none_or(|(w, s, st, _)| (w, *s, st) != (&list, size, &style))
                {
                    let measure = |text: &str, size: f64, weight: FontWeight| {
                        day::measure_text(text, size, &style.font(weight))
                    };
                    let layout = cloud_layout(&list, size, style.across_and_up, &measure);
                    *cache = Some((list.clone(), size, style.clone(), layout));
                }
                let Some((_, _, _, layout)) = cache.as_ref() else {
                    return;
                };
                let accent = accent(scope.get());
                let hover = hovered.get();
                for word in layout {
                    let text = &list[word.index].word;
                    let lit = hover == Some(word.index);
                    if lit {
                        d.fill(
                            Shape::RoundedRect(word.frame.inset(-3.0), 5.0),
                            accent.with_alpha(0.14),
                        );
                    }
                    let text_style = TextStyle {
                        size: word.size,
                        color: if lit {
                            accent
                        } else {
                            cloud_color(word.index, text, word.weight, accent)
                        },
                        font: style.font(word.font_weight()),
                        ..Default::default()
                    };
                    if word.vertical {
                        // Reading upward from the box's bottom-leading corner: a quarter turn
                        // back, so the word's run along x goes up the screen.
                        let frame = word.frame;
                        d.transformed(
                            Affine::rotate(-std::f64::consts::FRAC_PI_2)
                                .then(Affine::translate(frame.min_x(), frame.max_y())),
                            |d| d.text(text, Point::new(0.0, 0.0), text_style),
                        );
                    } else {
                        d.text(text, word.frame.origin, text_style);
                    }
                }
            })
            .id(day::id!(dashboard_word_cloud))
            .on_tap_at(move |point| {
                if let Some(index) = tap(point)
                    && let Some(word) =
                        words.with_untracked(|w| w.as_ref().and_then(|w| w.get(index)).cloned())
                {
                    search_for(&word.word);
                }
            })
            .on_hover(move |point| hovered.set_if_changed(point.and_then(&at)))
            .cursor(move || {
                if hovered.get().is_some() {
                    Cursor::Pointer
                } else {
                    Cursor::Default
                }
            })
            .a11y(|a| a.label(res::str::dashboard_words_title()))
            .grow(),
        ))
        .spacing(4.0)
        .align(HAlign::Leading)
        .grow(),
    )
    .any()
}

/// Search the selected feed for `word`: the toolbar's search field shows it, and the list
/// narrows to the articles that contain it. A phone goes back to that list to show them.
fn search_for(word: &str) {
    let scene = daynews_core::scene();
    scene.search.set(word.to_owned());
    if day::size_class().is_none_or(|size| size.width == WidthClass::Compact) {
        scene.reader_open.set(false);
    }
}

/// The words, the size and the style last drawn, and the placement drawn from them.
type CloudKey = (
    Vec<daynews_db::dashboard::CloudWord>,
    Size,
    CloudStyle,
    Vec<PlacedWord>,
);

/// How the reader has the word cloud set (Settings ▸ Articles): words across and up, or across
/// only, and in which face.
#[derive(Clone, Debug, PartialEq)]
struct CloudStyle {
    across_and_up: bool,
    /// A bundled display family, or `None` for the system face.
    family: Option<String>,
}
impl CloudStyle {
    /// The face for a word of `weight`. The bundled display faces ship one weight each, and a
    /// synthesized bold smears them, so only the system face varies its weight with the size.
    fn font(&self, weight: FontWeight) -> CanvasFont {
        CanvasFont {
            family: self.family.clone(),
            weight: self.family.is_none().then_some(weight),
            ..Default::default()
        }
    }
}

/// One word placed in the cloud: which word, its box, its type size, and its share of the
/// cloud's range from least to most frequent (`0.0..=1.0`).
#[derive(Clone, Debug, PartialEq)]
struct PlacedWord {
    index: usize,
    /// The word's box on screen: its line box, or that box turned upright for a word set
    /// vertically.
    frame: Rect,
    size: f64,
    weight: f64,
    vertical: bool,
}
impl PlacedWord {
    fn font_weight(&self) -> FontWeight {
        cloud_weight(self.weight)
    }
}

fn cloud_weight(weight: f64) -> FontWeight {
    if weight > 0.6 {
        FontWeight::Bold
    } else if weight > 0.25 {
        FontWeight::Semibold
    } else {
        FontWeight::Medium
    }
}

fn contains(frame: &Rect, point: Point) -> bool {
    point.x >= frame.min_x()
        && point.x <= frame.max_x()
        && point.y >= frame.min_y()
        && point.y <= frame.max_y()
}

/// A word's color: the scope's accent for the few largest, then a small set of hues that sit
/// well together on the dashboard's cards, chosen by the word itself so a word keeps its color
/// from one visit to the next. Smaller words are a little lighter, so the large ones lead.
fn cloud_color(index: usize, word: &str, weight: f64, accent: Color) -> Color {
    let hues: [u32; 5] = if day::dark_mode() {
        [0x5EC4D6, 0xB49BFF, 0xF59E62, 0x7BCB8A, 0xF472B6]
    } else {
        [0x1F7A8C, 0x6D4AC4, 0xC2410C, 0x2F7D46, 0xB0236A]
    };
    let base = if index < 3 {
        accent
    } else {
        Color::hex(hues[(word_hash(word) % hues.len() as u64) as usize])
    };
    base.with_alpha(0.7 + 0.3 * weight)
}

fn word_hash(word: &str) -> u64 {
    // FNV-1a: stable across runs and platforms, unlike the standard library's hasher.
    word.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Lay `words` (most frequent first) out in `area` the way Wordle and d3-cloud do. Each word,
/// largest first, starts at the middle and walks out along an Archimedean spiral, stretched to
/// the area's shape, until its box overlaps no word already placed; its starting angle comes
/// from the word, so the cloud is organic but the same every time. `across_and_up` sets about
/// two words in five upright (the largest always reads across); at each step of the spiral a
/// word tries its own direction and then the other, which packs the cloud tighter than either
/// direction alone. Type size follows the square
/// root of a word's count between the least and most frequent, so the middle of the range
/// stays legible. When the larger words cannot all find room, the whole cloud shrinks and is
/// placed again; the smallest words are left out last.
fn cloud_layout(
    words: &[daynews_db::dashboard::CloudWord],
    area: Size,
    across_and_up: bool,
    measure: &dyn Fn(&str, f64, FontWeight) -> TextMetrics,
) -> Vec<PlacedWord> {
    const MIN_SIZE: f64 = 10.0;
    const PADDING: f64 = 2.0;
    if words.is_empty() || area.width < 40.0 || area.height < 24.0 {
        return Vec::new();
    }
    let (low, high) = words.iter().fold((u64::MAX, 0), |(lo, hi), w| {
        (lo.min(w.count), hi.max(w.count))
    });
    let range = (high - low).max(1) as f64;
    let weights: Vec<f64> = words
        .iter()
        .map(|w| ((w.count - low) as f64 / range).sqrt())
        .collect();
    // The words that must fit before the cloud is allowed to drop any: the larger ones.
    let required = words.len().min(24).max(words.len() * 2 / 3);
    // A feed's cloud can fill a tall page, so the largest word starts from the area's size,
    // not a fixed cap; the passes below shrink it until the larger words all fit.
    let mut largest = (area.height * 0.24)
        .min(area.width * 0.14)
        .clamp(18.0, 120.0);
    let mut best = Vec::new();
    for _ in 0..10 {
        let smallest = MIN_SIZE.min(largest * 0.4);
        let mut placed: Vec<PlacedWord> = Vec::new();
        let mut complete = true;
        for (index, word) in words.iter().enumerate() {
            let weight = weights[index];
            let size = smallest + (largest - smallest) * weight;
            let metrics = measure(&word.word, size, cloud_weight(weight));
            let box_size = Size::new(metrics.width + PADDING, metrics.height + PADDING);
            let seed = word_hash(&word.word);
            let upright = across_and_up && index > 0 && seed % 5 < 2;
            match spiral_place(&placed, box_size, area, seed, upright, across_and_up) {
                Some((frame, vertical)) => placed.push(PlacedWord {
                    index,
                    frame: frame.inset(PADDING / 2.0),
                    size,
                    weight,
                    vertical,
                }),
                None if index < required => {
                    complete = false;
                    break;
                }
                None => {}
            }
        }
        if complete {
            return placed;
        }
        if placed.len() > best.len() {
            best = placed;
        }
        largest *= 0.86;
    }
    best
}

/// The first box along the spiral from `area`'s middle that stays inside `area` and overlaps
/// nothing in `placed`: `size` across, or turned upright when `upright`, and when `either`, the
/// other way too where the preferred way does not fit. Answers the box and whether it is upright.
fn spiral_place(
    placed: &[PlacedWord],
    size: Size,
    area: Size,
    seed: u64,
    upright: bool,
    either: bool,
) -> Option<(Rect, bool)> {
    let center = Point::new(area.width / 2.0, area.height / 2.0);
    let stretch = (area.width / area.height).clamp(0.5, 4.0);
    let start = (seed % 360) as f64 * std::f64::consts::PI / 180.0;
    let limit = (area.width.hypot(area.height)) / 2.0;
    // The box most recently in the way is the likeliest to be in the way again (Wordle's
    // cache), so it is tried first.
    let mut last_hit: Option<usize> = None;
    let mut theta: f64 = 0.0;
    loop {
        let radius = 2.0 * theta;
        if radius > limit {
            return None;
        }
        let angle = theta + start;
        let at = Point::new(
            center.x + radius * angle.cos() * stretch.sqrt(),
            center.y + radius * angle.sin() / stretch.sqrt(),
        );
        // Step about two points along the curve, whatever the radius.
        theta += (2.0 / radius.max(1.0)).min(0.5);
        let ways: &[bool] = match (upright, either) {
            (true, true) => &[true, false],
            (false, true) => &[false, true],
            (true, false) => &[true],
            (false, false) => &[false],
        };
        for &vertical in ways {
            let (w, h) = if vertical {
                (size.height, size.width)
            } else {
                (size.width, size.height)
            };
            let candidate = Rect::new(at.x - w / 2.0, at.y - h / 2.0, w, h);
            if candidate.min_x() < 0.0
                || candidate.min_y() < 0.0
                || candidate.max_x() > area.width
                || candidate.max_y() > area.height
            {
                continue;
            }
            if let Some(i) = last_hit
                && placed[i].frame.intersects(&candidate)
            {
                continue;
            }
            match placed.iter().position(|p| p.frame.intersects(&candidate)) {
                Some(i) => last_hit = Some(i),
                None => return Some((candidate, vertical)),
            }
        }
    }
}

/// Measures and places the seven dashboard regions inside the exact offered viewport, never
/// shorter than `min_height`: the scroll around it offers no height and then places it at the
/// taller of that and the viewport. No reactive state writes occur in layout. Charts share
/// flexible space.
struct DashboardPieces(Signal<Scope>, Vec<AnyPiece>);
impl Piece for DashboardPieces {
    fn build(self, cx: &mut BuildCx) -> day::RNode {
        let node = cx.native(
            day_spec::kinds::CONTAINER,
            &day_spec::props::ContainerProps {
                clips: true,
                ..Default::default()
            },
            Rc::new(DashboardLayout { scope: self.0 }),
            Flex::default(),
            Boundary::No,
        );
        cx.under(node, |cx| {
            for child in self.1 {
                child.build(cx);
            }
        });
        node
    }
}
// Reserve chart space independently of the legend's intrinsic height. In short
// windows a vertical stack otherwise lets its five legend rows consume the donut.
struct PublisherPieces(Vec<AnyPiece>);
impl Piece for PublisherPieces {
    fn build(self, cx: &mut BuildCx) -> day::RNode {
        let node = cx.native(
            day_spec::kinds::CONTAINER,
            &day_spec::props::ContainerProps {
                clips: true,
                ..Default::default()
            },
            Rc::new(PublisherLayout),
            Flex::default(),
            Boundary::No,
        );
        cx.under(node, |cx| {
            for child in self.0 {
                child.build(cx);
            }
        });
        node
    }
}
struct PublisherLayout;
fn publisher_regions(size: Size) -> [Rect; 2] {
    let gap = 8.0_f64.min(size.width / 4.0);
    let plot = (size.width - gap) * 0.38;
    [
        Rect::new(0.0, 0.0, plot, size.height),
        Rect::new(plot + gap, 0.0, size.width - plot - gap, size.height),
    ]
}
impl Layout for PublisherLayout {
    fn measure(&self, cx: &mut dyn LayoutOps, children: &[day::RNode], p: Proposal) -> Size {
        let size = Size::new(p.width.unwrap_or(240.0), p.height.unwrap_or(150.0));
        for (&child, rect) in children.iter().zip(publisher_regions(size)) {
            cx.measure_child(child, Proposal::exact(rect.size));
        }
        size
    }
    fn place(&self, cx: &mut dyn LayoutOps, children: &[day::RNode], bounds: Rect) {
        for (&child, rect) in children.iter().zip(publisher_regions(bounds.size)) {
            cx.place_child(child, rect);
        }
    }
}
struct DashboardLayout {
    scope: Signal<Scope>,
}
/// The dashboard's seven regions, top to bottom: the hero, a feed's settings (`settings` tall,
/// zero elsewhere), the metrics, the activity chart, the word cloud and, beside it, the publisher
/// mix, then the refresh forecast along the bottom. A `feed`'s overview has no activity chart,
/// publisher mix or forecast (its schedule is in its settings), so its word cloud takes the rest.
fn regions(size: Size, settings: f64, feed: bool) -> [Rect; 7] {
    let w = size.width.max(0.0);
    let h = size.height.max(0.0);
    let compact = w < 480.0;
    let gap = 10.0_f64.min(h / 50.0).min(w / 4.0);
    // In smaller windows, reserve space for the plots before the decorative header and
    // footer. Their old combined 74% allocation left only enough room for chart titles and
    // padding, so the flexible chart nodes collapsed to zero height.
    let hero = (if compact { 100.0_f64 } else { 112.0_f64 }).min(h * 0.15);
    let metrics = 62.0_f64.min(h * 0.08);
    // A feed's settings keep their whole height; the dashboard scrolls (`min_height`) before
    // they are cut short, so only a viewport too short for anything else squeezes them.
    let settings = settings
        .min((h - hero - metrics - 3.0 * gap).max(0.0))
        .max(0.0);
    let settings_gap = if settings > 0.0 { gap } else { 0.0 };
    let (footer, footer_gaps) = if feed {
        (0.0, 0.0)
    } else {
        (104.0_f64.min(h * 0.2), 2.0 * gap)
    };
    let plot =
        (h - hero - settings - settings_gap - metrics - footer - footer_gaps - 2.0 * gap).max(0.0);
    let y = hero + settings + settings_gap + metrics + 2.0 * gap;
    // The activity chart keeps enough height to draw its line even in a short window; the word
    // cloud gets the rest.
    let (activity_h, activity_gap) = if feed {
        (0.0, 0.0)
    } else {
        ((plot * 0.4).max((plot * 0.5).min(130.0)), gap)
    };
    let lower = (plot - activity_h - activity_gap).max(0.0);
    let lower_y = y + activity_h + activity_gap;
    // The cloud takes the larger share beside the publishers: words need width.
    let (cloud_w, side_w) = if !feed {
        let cloud = ((w - gap) * 0.6).max(0.0);
        (cloud, (w - gap - cloud).max(0.0))
    } else {
        (w, 0.0)
    };
    [
        Rect::new(0.0, 0.0, w, hero),
        Rect::new(0.0, hero + settings_gap, w, settings),
        Rect::new(0.0, hero + settings + settings_gap + gap, w, metrics),
        Rect::new(0.0, y, w, activity_h),
        Rect::new(0.0, lower_y, cloud_w, lower),
        Rect::new((w - side_w).max(0.0), lower_y, side_w, lower),
        Rect::new(0.0, h - footer, w, footer),
    ]
}
/// The word cloud's least height, its card's heading included, before the dashboard scrolls.
const MIN_CLOUD: f64 = 180.0;

/// The least height at which the regions give the word cloud `MIN_CLOUD` (and so the activity
/// chart its own floor). The plots' share grows with the height, so halving finds it.
fn min_height(width: f64, settings: f64, feed: bool) -> f64 {
    let cloud = |h: f64| regions(Size::new(width, h), settings, feed)[4].size.height;
    let (mut low, mut high) = (0.0_f64, 4000.0_f64);
    if cloud(high) < MIN_CLOUD {
        return high;
    }
    while high - low > 1.0 {
        let mid = (low + high) / 2.0;
        if cloud(mid) < MIN_CLOUD {
            low = mid;
        } else {
            high = mid;
        }
    }
    high.ceil()
}
impl DashboardLayout {
    /// The settings form's own height at `width` (nothing builds there off a feed's overview),
    /// and whether this is a feed's overview.
    fn measure_sections(
        &self,
        cx: &mut dyn LayoutOps,
        children: &[day::RNode],
        width: f64,
    ) -> (f64, bool) {
        let settings = children.get(1).map_or(0.0, |&child| {
            cx.measure_child(
                child,
                Proposal {
                    width: Some(width),
                    height: None,
                },
            )
            .height
        });
        (
            settings,
            matches!(self.scope.get_untracked(), Scope::Feed(_)),
        )
    }
}
impl Layout for DashboardLayout {
    fn measure(&self, cx: &mut dyn LayoutOps, children: &[day::RNode], p: Proposal) -> Size {
        let width = p.width.unwrap_or(680.0);
        let (settings, feed) = self.measure_sections(cx, children, width);
        let size = Size::new(
            width,
            p.height
                .unwrap_or_else(|| min_height(width, settings, feed)),
        );
        for (&child, rect) in children.iter().zip(regions(size, settings, feed)) {
            cx.measure_child(child, Proposal::exact(rect.size));
        }
        size
    }
    fn place(&self, cx: &mut dyn LayoutOps, children: &[day::RNode], bounds: Rect) {
        let (settings, feed) = self.measure_sections(cx, children, bounds.size.width);
        for (&child, rect) in children.iter().zip(regions(bounds.size, settings, feed)) {
            cx.place_child(child, rect);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(n: usize) -> Vec<daynews_db::dashboard::CloudWord> {
        (0..n)
            .map(|i| daynews_db::dashboard::CloudWord {
                word: format!("word{i}{}", "x".repeat(i % 7)),
                count: (n - i) as u64 * 3,
            })
            .collect()
    }

    /// A stand-in for the toolkit's measurement: Day's own approximation.
    fn measure(text: &str, size: f64, _: FontWeight) -> TextMetrics {
        TextMetrics::approximate(text, size)
    }

    #[test]
    fn word_cloud_words_stay_inside_and_never_overlap() {
        for (area, across_and_up) in [
            (Size::new(560.0, 220.0), true),
            (Size::new(560.0, 220.0), false),
            (Size::new(300.0, 120.0), true),
        ] {
            let placed = cloud_layout(&words(60), area, across_and_up, &measure);
            assert!(placed.len() >= 20, "{} words placed", placed.len());
            for (i, a) in placed.iter().enumerate() {
                assert!(a.frame.min_x() >= -0.001 && a.frame.min_y() >= -0.001);
                assert!(a.frame.max_x() <= area.width + 0.001);
                assert!(a.frame.max_y() <= area.height + 0.001);
                for b in &placed[i + 1..] {
                    assert!(!a.frame.intersects(&b.frame), "{a:?} overlaps {b:?}");
                }
            }
            // The most frequent word is placed, reading across.
            assert!(placed.iter().any(|w| w.index == 0 && !w.vertical));
            assert_eq!(
                placed.iter().any(|w| w.vertical),
                across_and_up,
                "upright words only in the two-dimensional layout"
            );
        }
    }
    #[test]
    fn dashboard_scrolls_before_the_charts_collapse() {
        for (width, settings, feed) in [
            (390.0, 0.0, false),
            (420.0, 200.0, true),
            (420.0, 420.0, true),
            (900.0, 120.0, true),
            (900.0, 0.0, false),
        ] {
            let h = min_height(width, settings, feed);
            let boxes = regions(Size::new(width, h), settings, feed);
            assert!(boxes[4].size.height >= MIN_CLOUD, "cloud {:?}", boxes[4]);
            if feed {
                assert_eq!(boxes[1].size.height, settings, "settings cut short");
            } else {
                assert!(boxes[3].size.height >= 130.0, "activity {:?}", boxes[3]);
            }
            let shorter = regions(Size::new(width, h - 2.0), settings, feed);
            assert!(
                shorter[4].size.height < MIN_CLOUD,
                "{h} is not the least height"
            );
        }
    }
    #[test]
    fn dashboard_regions_fit_small_and_large_viewports_without_overlap() {
        for (width, height, settings, feed) in [0.0, 280.0, 390.0, 640.0, 1000.0]
            .into_iter()
            .flat_map(|w| [0.0, 300.0, 550.0, 800.0, 1200.0].map(move |h| (w, h)))
            .flat_map(|(w, h)| [(w, h, 0.0, false), (w, h, 96.0, true), (w, h, 400.0, true)])
        {
            {
                let boxes = regions(Size::new(width, height), settings, feed);
                for rect in &boxes {
                    assert!(rect.origin.x >= 0.0 && rect.origin.y >= 0.0);
                    assert!(rect.size.width >= 0.0 && rect.size.height >= 0.0);
                    assert!(rect.origin.x + rect.size.width <= width + 0.0001);
                    assert!(rect.origin.y + rect.size.height <= height + 0.0001);
                }
                for a in 0..boxes.len() {
                    for b in a + 1..boxes.len() {
                        let x = (boxes[a].origin.x + boxes[a].size.width)
                            .min(boxes[b].origin.x + boxes[b].size.width)
                            - boxes[a].origin.x.max(boxes[b].origin.x);
                        let y = (boxes[a].origin.y + boxes[a].size.height)
                            .min(boxes[b].origin.y + boxes[b].size.height)
                            - boxes[a].origin.y.max(boxes[b].origin.y);
                        assert!(x <= 0.0001 || y <= 0.0001, "overlapping dashboard regions");
                    }
                }
            }
        }
    }
    #[test]
    fn a_feeds_word_cloud_fills_the_page_below_its_settings() {
        let size = Size::new(700.0, 900.0);
        let boxes = regions(size, 260.0, true);
        for empty in [3, 5, 6] {
            assert_eq!(boxes[empty].size.height * boxes[empty].size.width, 0.0);
        }
        assert_eq!(boxes[4].size.width, size.width);
        assert_eq!(boxes[4].max_y(), size.height);
        assert!(boxes[4].min_y() > boxes[2].max_y());
    }
}

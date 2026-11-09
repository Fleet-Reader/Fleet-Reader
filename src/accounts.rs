//! Native account setup; translated presentation stays on the UI thread.
use crate::res;
use day::prelude::*;

#[cfg(not(feature = "feedbin"))]
pub fn page() -> impl Piece {
    #[cfg(all(feature = "experimental-webdav", not(target_arch = "wasm32")))]
    {
        crate::webdav_accounts::page().any()
    }
    #[cfg(not(all(feature = "experimental-webdav", not(target_arch = "wasm32"))))]
    {
        label(res::str::accounts_sync_unavailable()).any()
    }
}

#[cfg(feature = "feedbin")]
pub fn page() -> impl Piece {
    #[cfg(all(feature = "experimental-webdav", not(target_arch = "wasm32")))]
    {
        let provider = Signal::new(usize::from(
            !daynews_core::webdav::status()
                .get_untracked()
                .endpoint
                .is_empty(),
        ));
        column((
            picker(
                provider,
                [
                    (0, res::str::sync_feedbin()),
                    (1, res::str::accounts_webdav()),
                ],
            )
            .segmented()
            .id(day::id!(accounts_provider))
            .a11y(|a| a.label(res::str::accounts_provider()))
            .grow(Dimension::Width),
            // The settings host owns scrolling on every target. An inner bounded
            // scroll clips this long form and competes with the phone's page scroll.
            keyed(
                move || Some(provider.get()),
                |selected| {
                    if *selected == 0 {
                        feedbin_page().any()
                    } else {
                        crate::webdav_accounts::page().any()
                    }
                },
            )
            .grow(Dimension::Width),
        ))
        .spacing(Space::M)
        .grow(Dimension::Width)
        .any()
    }
    #[cfg(not(all(feature = "experimental-webdav", not(target_arch = "wasm32"))))]
    feedbin_page().any()
}

#[cfg(feature = "feedbin")]
fn feedbin_page() -> impl Piece {
    let status = daynews_core::sync_status();
    let username = Signal::new(status.get_untracked().username);
    let password = Signal::new(String::new());
    let merge = Signal::new(false);
    column((
        label(res::str::sync_feedbin()).font(Font::Title2),
        label(res::str::sync_description()).font(Font::Body),
        sync_progress(),
        when(
            move || !status.get().connected && !status.get().preview,
            move || {
                column((
                    label(res::str::sync_username()),
                    text_field(username)
                        .input_purpose(InputPurpose::Email)
                        .placeholder(res::str::sync_username())
                        .id(day::id!(sync_username))
                        .grow(Dimension::Width),
                    label(res::str::sync_password()),
                    secure_field(password)
                        .placeholder(res::str::sync_password())
                        .id(day::id!(sync_password))
                        .grow(Dimension::Width),
                    button(res::str::sync_sign_in())
                        .enabled(move || {
                            !status.get().busy
                                && !username.get().trim().is_empty()
                                && !password.get().is_empty()
                        })
                        .action(move || {
                            let secret = password.get_untracked();
                            password.set(String::new());
                            daynews_core::sync_sign_in(username.get_untracked(), secret);
                        })
                        .id(day::id!(sync_sign_in)),
                ))
                .spacing(Space::S)
                .grow(Dimension::Width)
            },
        ),
        when(
            move || status.get().preview,
            move || {
                column((
                    label(move || {
                        res::str::sync_preview(res::str::SyncPreview {
                            local: status.get().local_feeds as f64,
                            remote: status.get().remote_feeds as f64,
                        })
                        .format()
                    })
                    .id(day::id!(sync_preview)),
                    label(res::str::sync_read_state_import()).font(Font::Footnote),
                    row((
                        label(res::str::sync_merge()).grow(Dimension::Width),
                        toggle(merge).id(day::id!(sync_merge)),
                    ))
                    .spacing(Space::S),
                    row((
                        button(res::str::sync_connect())
                            .enabled(move || !status.get().busy)
                            .action(move || daynews_core::sync_connect(merge.get_untracked()))
                            .id(day::id!(sync_connect)),
                        button(res::str::sync_cancel())
                            .enabled(move || !status.get().busy)
                            .action(daynews_core::sync_cancel_login)
                            .id(day::id!(sync_cancel)),
                    ))
                    .spacing(Space::M),
                ))
                .spacing(Space::M)
            },
        ),
        when(
            move || status.get().connected,
            move || {
                column((
                    label(move || res::str::sync_account(status.get().username).format())
                        .id(day::id!(sync_account)),
                    label(move || res::str::sync_pending(status.get().pending as f64).format())
                        .id(day::id!(sync_pending)),
                    label(move || match status.get().last_sync {
                        Some(at) => res::str::sync_last(crate::format::full_date(at)).format(),
                        None => res::str::sync_never().format(),
                    })
                    .id(day::id!(sync_last)),
                    row((
                        button(res::str::sync_now())
                            .enabled(move || !status.get().busy)
                            .action(daynews_core::sync_now)
                            .id(day::id!(sync_now)),
                        button(res::str::sync_sign_in())
                            .enabled(move || !status.get().busy)
                            .action(daynews_core::sync_reauthenticate)
                            .id(day::id!(sync_reauthenticate)),
                        button(res::str::sync_disconnect())
                            .enabled(move || !status.get().busy)
                            .action(daynews_core::sync_disconnect)
                            .id(day::id!(sync_disconnect)),
                    ))
                    .spacing(Space::M),
                    label(res::str::sync_disconnect_note()).font(Font::Footnote),
                    button(res::str::sync_reset())
                        .enabled(move || {
                            !status.get().busy
                                && daynews_core::state().refresh_progress.get().is_none()
                                && daynews_core::state().updating_feeds.get().is_empty()
                        })
                        .action(|| {
                            // All translations are formatted on the UI thread before awaiting.
                            let dialog = alert(res::str::sync_reset_title())
                                .message(res::str::sync_reset_message())
                                .destructive(res::str::sync_reset_confirm(), ())
                                .cancel(res::str::sync_cancel());
                            day::task(async move {
                                if dialog.present().await.is_some() {
                                    daynews_core::sync_reset_local();
                                }
                            });
                        })
                        .id(day::id!(sync_reset)),
                ))
                .spacing(10.0)
            },
        ),
        label(move || {
            let s = status.get();
            if let Some(error) = s.error {
                match error {
                    daynews_sync::Error::Authentication => res::str::sync_auth_error().format(),
                    daynews_sync::Error::Storage => res::str::sync_storage_error().format(),
                    daynews_sync::Error::Network | daynews_sync::Error::Retry { .. } => {
                        res::str::sync_network_error().format()
                    }
                    _ => res::str::sync_failed().format(),
                }
            } else if s.paused && !s.busy {
                res::str::sync_paused().format()
            } else if s.busy {
                res::str::sync_working().format()
            } else if s.connected && s.pending > 0 {
                res::str::sync_waiting().format()
            } else if s.connected {
                res::str::sync_ready().format()
            } else {
                String::new()
            }
        })
        .id(day::id!(sync_status)),
    ))
    .spacing(14.0)
    .align(HAlign::Leading)
    .grow(Dimension::Width)
}

/// Shared status footer for the article column and compact mobile feeds page.
pub fn sync_status_footer(id: day::ElementId) -> impl Piece {
    when(
        || {
            let status = daynews_core::sync_status().get();
            crate::settings::SHOW_SYNC_STATUS.get() && status.busy && status.progress.is_some()
        },
        move || {
            column((
                divider(),
                sync_progress().padding(Insets::symmetric(12.0, 4.0)),
            ))
            .id(id.clone())
            .background(move || crate::theme::palette().bg_alt)
            .grow(Dimension::Width)
        },
    )
}

/// Shared by Accounts, the article list and the compact mobile feeds page.
pub fn sync_progress() -> impl Piece {
    let status = daynews_core::sync_status();
    when(
        move || status.get().busy && status.get().progress.is_some(),
        move || {
            // Keep one stable layout for every phase: the indicator, activity, feed and timing
            // lines never appear/disappear as the server supplies different progress data.
            column((
                row((
                    when(
                        move || {
                            status
                                .get()
                                .progress
                                .as_ref()
                                .and_then(|p| p.fraction())
                                .is_some()
                        },
                        move || {
                            progress(move || {
                                status
                                    .get()
                                    .progress
                                    .as_ref()
                                    .and_then(|p| p.fraction())
                                    .unwrap_or(0.0)
                            })
                            .id(day::id!(sync_progress_bar))
                            .grow(Dimension::Width)
                        },
                    ),
                    when(
                        move || {
                            status
                                .get()
                                .progress
                                .as_ref()
                                .and_then(|p| p.fraction())
                                .is_none()
                        },
                        spinner,
                    ),
                ))
                .height(20.0)
                .grow(Dimension::Width),
                row((
                    column((
                        label(move || progress_text(&status.get().progress.unwrap_or_default()))
                            .font(Font::Caption)
                            .single_line()
                            .color(move || crate::theme::palette().text)
                            .id(day::id!(sync_progress_activity))
                            .grow(Dimension::Width),
                        label(move || {
                            status
                                .get()
                                .progress
                                .and_then(|p| p.feed)
                                .map(|feed| res::str::sync_progress_feed(feed).format())
                                .unwrap_or_default()
                        })
                        .font(Font::Caption)
                        .single_line()
                        .color(move || crate::theme::palette().text_muted)
                        // Reserve a line even before the first feed arrives, using the
                        // caption's actual font metrics (including accessibility scaling).
                        .reserving(res::str::sync_progress_feed(String::new()))
                        .id(day::id!(sync_progress_feed))
                        .grow(Dimension::Width),
                        label(move || progress_timing(&status.get()))
                            .font(Font::Caption)
                            .single_line()
                            .color(move || crate::theme::palette().text_muted)
                            .id(day::id!(sync_progress_time))
                            .grow(Dimension::Width),
                    ))
                    .spacing(2.0)
                    .align(HAlign::Leading)
                    .grow(Dimension::Width),
                    button(res::str::sync_cancel())
                        .action(daynews_core::sync_cancel)
                        .enabled(move || status.get().cancellable)
                        .id(day::id!(sync_cancel_transfer)),
                ))
                .spacing(Space::S)
                .align(VAlign::Center)
                .grow(Dimension::Width),
            ))
            .spacing(2.0)
            .align(HAlign::Leading)
            .grow(Dimension::Width)
        },
    )
}
/// How long the running sync has taken, and how much longer it should.
pub(crate) fn progress_timing(s: &daynews_core::SyncStatus) -> String {
    let duration = |seconds: u64| {
        res::str::sync_progress_duration(res::str::SyncProgressDuration {
            minutes: (seconds / 60) as f64,
            seconds: (seconds % 60) as f64,
        })
        .format()
    };
    match s.remaining_seconds {
        Some(seconds) => res::str::sync_progress_eta(res::str::SyncProgressEta {
            elapsed: duration(s.elapsed_seconds),
            remaining: duration(seconds),
        })
        .format(),
        None => res::str::sync_progress_estimating(duration(s.elapsed_seconds)).format(),
    }
}

pub(crate) fn progress_text(p: &daynews_sync::progress::Progress) -> String {
    use daynews_sync::progress::Phase;
    match p.phase {
        Phase::Preparing => res::str::sync_progress_preparing().format(),
        Phase::Uploading => res::str::sync_progress_upload(res::str::SyncProgressUpload {
            done: p.completed as f64,
            total: p.total.unwrap_or(0) as f64,
        })
        .format(),
        Phase::Subscriptions => {
            res::str::sync_progress_subscriptions(p.subscriptions as f64).format()
        }
        Phase::ReadState => res::str::sync_progress_read().format(),
        Phase::Articles => match p.total {
            Some(total) => res::str::sync_progress_articles(res::str::SyncProgressArticles {
                articles: p.articles as f64,
                page: p.completed as f64,
                total: total as f64,
            })
            .format(),
            None => {
                res::str::sync_progress_articles_unknown(res::str::SyncProgressArticlesUnknown {
                    articles: p.articles as f64,
                    pages: p.completed as f64,
                })
                .format()
            }
        },
        Phase::MissingArticles => res::str::sync_progress_missing(res::str::SyncProgressMissing {
            done: p.completed as f64,
            total: p.total.unwrap_or(0) as f64,
        })
        .format(),
        Phase::Finalizing => res::str::sync_progress_finalizing().format(),
        Phase::Complete => res::str::sync_progress_complete().format(),
        Phase::Resetting => res::str::sync_progress_resetting().format(),
    }
}

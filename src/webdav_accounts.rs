// SPDX-License-Identifier: GPL-2.0-or-later
//! WebDAV account setup. Credentials stay in the native secure store.
use crate::res;
use day::prelude::*;
use daynews_core::webdav::{self, Phase};

pub fn page() -> impl Piece {
    let status = webdav::status();
    let current = status.get_untracked();
    let reqwest = Signal::new(current.reqwest);
    let space_id = Signal::new(current.space_id);
    let advanced = Signal::new(false);
    let configured = move || !status.get().space_id.is_empty();
    let busy = move || {
        matches!(
            status.get().phase,
            Phase::Working | Phase::Restart | Phase::DisconnectRestart
        )
    };
    column((
        row((
            label(res::str::dav_title())
                .font(Font::Title3)
                .grow(Dimension::Width),
            button(res::str::dav_information())
                .icon(Symbol::Info)
                .icon_only()
                .plain()
                .action(|| {
                    let dialog = alert(res::str::dav_title())
                        .message(res::str::dav_description())
                        .button(res::str::collection_ok(), ());
                    day::task(async move {
                        dialog.present().await;
                    });
                })
                .id(day::id!(dav_information))
                .frame(44.0, 44.0),
        ))
        .grow(Dimension::Width),
        day_piece_webdav_settings::account_form(
            day_piece_webdav_settings::AccountFields {
                endpoint: current.endpoint,
                saved_folder: current.folder,
                default_folder: "Fleet-Reader",
                username: current.username,
            },
            configured,
            busy,
            day_piece_webdav_settings::Labels {
                endpoint: res::str::dav_endpoint(),
                folder: res::str::dav_app_folder(),
                invalid_address: res::str::dav_invalid_address(),
                username: res::str::dav_username(),
                password: res::str::dav_password(),
                connect: res::str::dav_connect(),
                update: res::str::dav_update_credentials(),
            },
            move |url, folder, username, password| {
                if configured() {
                    webdav::update_credentials(username, password, reqwest.get_untracked());
                } else {
                    let space = space_id.get_untracked().trim().to_owned();
                    webdav::prepare(
                        url,
                        folder,
                        username,
                        password,
                        reqwest.get_untracked(),
                        (!space.is_empty()).then_some(space),
                    );
                }
            },
        ),
        label(move || match status.get().phase {
            Phase::Disabled => res::str::dav_disabled().format(),
            Phase::Working => res::str::sync_working().format(),
            Phase::Ready => res::str::sync_ready().format(),
            Phase::Restart => res::str::dav_restart().format(),
            Phase::Failed => res::str::dav_failed().format(),
            Phase::Attention => res::str::dav_attention().format(),
            Phase::ReadOnly => res::str::dav_read_only().format(),
            Phase::Paused => res::str::dav_paused().format(),
            Phase::DisconnectRestart => res::str::dav_disconnect_restart().format(),
        })
        .font(Font::Footnote)
        .id(day::id!(dav_status))
        .grow(Dimension::Width),
        day_piece_webdav_settings::failure_details(res::str::dav_more_info(), move || {
            let s = status.get();
            if !matches!(s.phase, Phase::Failed | Phase::Attention) {
                return None;
            }
            s.detail
                .map(|detail| res::str::dav_diagnostic(detail).format())
        }),
        when(configured, move || {
            button(res::str::sync_now())
                .bordered()
                .enabled(move || {
                    matches!(
                        status.get().phase,
                        Phase::Ready | Phase::Failed | Phase::Attention
                    )
                })
                .action(webdav::sync_now)
                .id(day::id!(dav_sync_now))
                .grow(Dimension::Width)
        }),
        button(res::str::dav_advanced())
            .plain()
            .icon(move || {
                if advanced.get() {
                    Symbol::Down
                } else {
                    Symbol::Forward
                }
            })
            .action(move || advanced.set(!advanced.get_untracked()))
            .id(day::id!(dav_advanced))
            .grow(Dimension::Width),
        when(
            move || advanced.get(),
            move || {
                column((
                    form((section((
                        labeled(
                            res::str::dav_space_id(),
                            text_field(space_id)
                                .read_only(configured)
                                .id(day::id!(dav_space_id))
                                .grow(Dimension::Width),
                        ),
                        labeled(
                            res::str::dav_reqwest(),
                            toggle(reqwest).id(day::id!(dav_reqwest)),
                        ),
                    ))
                    .footer(res::str::dav_advanced_help()),))
                    .grow(Dimension::Width),
                    when(configured, move || {
                        column((
                            button(res::str::dav_copy_space())
                                .bordered()
                                .action(move || {
                                    let _ =
                                        day::clipboard::set_text(&status.get_untracked().space_id);
                                })
                                .id(day::id!(dav_copy_space))
                                .grow(Dimension::Width),
                            button(move || {
                                if status.get().phase == Phase::Paused {
                                    res::str::dav_resume().format()
                                } else {
                                    res::str::dav_pause().format()
                                }
                            })
                            .bordered()
                            .enabled(move || {
                                matches!(
                                    status.get().phase,
                                    Phase::Ready | Phase::Failed | Phase::Attention | Phase::Paused
                                )
                            })
                            .action(move || {
                                webdav::pause(status.get_untracked().phase != Phase::Paused)
                            })
                            .id(day::id!(dav_pause))
                            .grow(Dimension::Width),
                            label(move || {
                                res::str::sync_pending(status.get().pending as f64).format()
                            })
                            .font(Font::Footnote)
                            .id(day::id!(dav_pending)),
                            button(res::str::dav_disconnect())
                                .bordered()
                                .enabled(move || !busy())
                                .action(|| {
                                    let dialog = alert(res::str::dav_disconnect())
                                        .message(res::str::dav_disconnect_message())
                                        .destructive(res::str::dav_disconnect_confirm(), ())
                                        .cancel(res::str::sync_cancel());
                                    day::task(async move {
                                        if dialog.present().await.is_some() {
                                            webdav::disconnect();
                                        }
                                    });
                                })
                                .id(day::id!(dav_disconnect))
                                .grow(Dimension::Width),
                        ))
                        .spacing(Space::M)
                        .align(HAlign::Leading)
                        .grow(Dimension::Width)
                    }),
                ))
                .spacing(Space::M)
                .align(HAlign::Leading)
                .grow(Dimension::Width)
            },
        ),
    ))
    .spacing(Space::M)
    .align(HAlign::Leading)
    .grow(Dimension::Width)
}

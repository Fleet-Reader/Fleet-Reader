// SPDX-License-Identifier: GPL-2.0-or-later
//! Commands for the link or image under an article's context menu.
use crate::{reader_view::ReaderView, res, site_browser};
use day::prelude::*;
use day_piece_webview::{
    DownloadEvent, DownloadRequest, DownloadResponder, WebMenuItem, WebMenuTarget, WebView,
};
use daynews_core::NewsScene;

#[derive(Clone, Copy)]
enum Destination {
    External,
    Window,
    Cover,
}

impl Destination {
    fn current() -> Self {
        if !site_browser::can_browse() || !crate::settings::BROWSER.get_untracked().is_empty() {
            Self::External
        } else if day_core::capability(day_spec::Cap::MultiWindow) == Support::Native {
            Self::Window
        } else {
            Self::Cover
        }
    }
}

pub(crate) fn article_menu(web: WebView, scene: NewsScene, view: ReaderView) -> WebView {
    let owner = site_browser::Browser::ambient();
    let destination = Destination::current();
    let (link, image) = match destination {
        Destination::External => (
            res::str::reader_menu_link_browser(),
            res::str::reader_menu_image_browser(),
        ),
        Destination::Window => (
            res::str::reader_menu_link_window(),
            res::str::reader_menu_image_window(),
        ),
        Destination::Cover => (res::str::reader_menu_link(), res::str::reader_menu_image()),
    };
    let entries = [
        WebMenuItem::new("open-link", link.format(), WebMenuTarget::Link).browser_on_web(),
        WebMenuItem::new("open-image", image.format(), WebMenuTarget::Image).browser_on_web(),
        WebMenuItem::new(
            "download-image",
            res::str::reader_menu_download_image().format(),
            WebMenuTarget::DownloadImage,
        ),
    ];
    web.web_context_menu(entries, move |action| {
        let Some((article, feed)) = scene
            .article
            .with_untracked(|a| a.as_ref().map(|a| (a.id, a.feed_id)))
        else {
            return;
        };
        if !view.visible.get_untracked()
            || scene.selected.get_untracked() != Some(article)
            || source_article(&action.document) != Some(article)
            || crate::extraction::web_url(&action.url).is_none()
        {
            return;
        }
        let url = action.url.clone();
        match destination {
            Destination::External => crate::settings::open_link(&url),
            Destination::Cover => owner.show(url, Some(feed)),
            Destination::Window => {
                let profile = site_browser::profile(feed);
                day::open_window(crate::window(), move || {
                    day_piece_webview::web_view(Signal::new(url))
                        .profile(profile)
                        .on_external_link(|_| day_piece_webview::LinkPolicy::InView)
                        .grow(Dimension::Both)
                });
            }
        }
    })
    .on_download(save_image)
}

fn source_article(document: &str) -> Option<u64> {
    url::Url::parse(document)
        .ok()?
        .query_pairs()
        .find(|(key, _)| key == "article")?
        .1
        .parse()
        .ok()
}

fn save_image(request: DownloadRequest, reply: DownloadResponder) {
    let name = request.suggested_name;
    toast(res::str::reader_menu_download_started(name.clone()));
    #[cfg(target_os = "ios")]
    stage_image_for_export(request.id, name, reply);
    #[cfg(not(target_os = "ios"))]
    {
        let display_name = name.clone();
        reply.save_to_downloads(&name, move |event| match event {
            DownloadEvent::Finished { .. } => {
                toast(res::str::reader_menu_download_saved(display_name.clone()))
            }
            DownloadEvent::Failed(_) => toast(res::str::reader_menu_download_failed()),
            DownloadEvent::Progress { .. } => {}
        });
    }
}

#[cfg(target_os = "ios")]
fn stage_image_for_export(id: u64, name: String, reply: DownloadResponder) {
    use day_spec::present::{PresentResult, PresentSpec};

    let directory = std::env::temp_dir().join(format!("fleet-image-{}-{id}", std::process::id()));
    if std::fs::create_dir(&directory).is_err() {
        toast(res::str::reader_menu_download_failed());
        reply.cancel();
        return;
    }
    let file = directory.join(&name);
    reply.save_to(file.clone(), move |event| match event {
        DownloadEvent::Finished { .. } => {
            let directory = directory.clone();
            let file = file.clone();
            let name = name.clone();
            let export = PresentSpec::SaveFile {
                title: res::str::reader_menu_download_image().format(),
                suggested_name: name.clone(),
                src_path: file.to_string_lossy().into_owned(),
                filters: Vec::new(),
            };
            day::task(async move {
                let result = day_core::present(export).await;
                let _ = std::fs::remove_file(file);
                let _ = std::fs::remove_dir(directory);
                if matches!(result, PresentResult::Files(ref files) if !files.is_empty()) {
                    toast(res::str::reader_menu_download_saved(name));
                }
            });
        }
        DownloadEvent::Failed(_) => {
            let _ = std::fs::remove_file(&file);
            let _ = std::fs::remove_dir(&directory);
            toast(res::str::reader_menu_download_failed());
        }
        DownloadEvent::Progress { .. } => {}
    });
}

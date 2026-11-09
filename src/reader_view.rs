// SPDX-License-Identifier: Apache-2.0
//! Per-window Reader View state. RSS content stays intact; extracted content is a temporary
//! overlay for the selected article. Selection changes and window disposal cancel extraction.

use crate::extraction::{ArticleExtractor, ExtractedArticle, ExtractionError, LocalReadability};
use crate::load_diagnostics::{ArticleLoad, Load};
use day::prelude::*;
use day_piece_webview::JsHandle;
use std::{cell::Cell, rc::Rc};

#[derive(Clone, Copy)]
pub(crate) struct ReaderVisibility(pub Signal<bool>);

#[derive(Clone, Copy)]
pub struct ReaderView {
    /// The window's mobile pager publishes its current page here for toolbar commands.
    pub shown: Signal<Option<ReaderView>>,
    pub visible: Signal<bool>,
    pub engine: JsHandle,
    pub ready: Signal<bool>,
    pub active: Signal<bool>,
    pub loading: Signal<bool>,
    pub error: Signal<Option<ExtractionError>>,
    pub extracted: Signal<Option<(u64, ExtractedArticle)>>,
    /// The extracted article looks like a teaser (`looks_short`): the reader offers the page in
    /// the in-app browser, where a sign-in can unlock it.
    pub short: Signal<bool>,
    task: Signal<Rc<Cell<Option<day::TaskHandle>>>>,
    auto_article: Signal<Option<u64>>,
    pub article_load: Signal<ArticleLoad>,
}

impl Ambient for ReaderView {
    fn create() -> Self {
        let pending = Rc::new(Cell::new(None::<day::TaskHandle>));
        let article_load = ArticleLoad::default();
        let state = Self {
            shown: Signal::new(None),
            visible: day_core::environment::<ReaderVisibility>()
                .map_or_else(|| Signal::new(true), |v| v.0),
            engine: JsHandle::new(),
            ready: Signal::new(false),
            active: Signal::new(false),
            loading: Signal::new(false),
            error: Signal::new(None),
            extracted: Signal::new(None),
            short: Signal::new(false),
            task: Signal::new(pending.clone()),
            auto_article: Signal::new(None),
            article_load: Signal::new(article_load.clone()),
        };
        let scene = daynews_core::scene();
        // The displayed snapshot stays visible while the next database read is pending.
        // Cancel extraction as soon as the selection changes, before that snapshot arrives.
        article_load.select(scene.selected.get_untracked());
        watch(
            move || scene.selected.get(),
            move |id, previous| {
                state.cancel();
                if previous != Some(id) {
                    state.article_load.get_untracked().select(*id);
                }
            },
        );
        watch(
            move || {
                scene
                    .article
                    .with(|a| a.as_ref().map(|a| (a.id, a.url.clone())))
            },
            move |identity, previous| {
                // watch runs when any field of the article snapshot changes, even if
                // this projection is unchanged. Read/star edits must keep extraction,
                // readiness, the live document, and its scroll position intact.
                if previous == Some(identity) {
                    return;
                }
                if let Some((id, _)) = identity {
                    state.article_load.get_untracked().event(*id, "snapshot_ready", serde_json::json!({
                        "content_bytes": scene.article.with_untracked(|a| a.as_ref().and_then(|a| a.content_html.as_ref()).map_or(0, String::len)),
                    }));
                }
                state.cancel();
                state.auto_article.set(None);
                state.active.set(false);
                state.error.set(None);
                state.extracted.set(None);
                state.short.set(false);
                state.ready.set(false);
            },
        );
        watch(
            move || {
                (
                    state.ready.get(),
                    state.visible.get(),
                    scene.article.with(|a| {
                        a.as_ref()
                            .map(|a| (a.id, crate::reader_options::options(Some(a.feed_id))))
                    }),
                )
            },
            move |(ready, visible, article), _| {
                let Some((id, options)) = article else {
                    return;
                };
                if !options.auto {
                    state.auto_article.set(None);
                }
                if *ready && day_piece_webview::eval_support() == Support::Native {
                    let enabled =
                        *visible && options.auto && state.auto_article.get_untracked() != Some(*id);
                    let task = day::task(async move {
                        let _ = state.engine.eval(format!(r#"(() => {{
                            window.readerAutoObserver?.disconnect();
                            const button = document.getElementById('reader-load-full');
                            if (!{enabled} || !button) return;
                            window.readerAutoObserver = new IntersectionObserver(entries => {{
                                if (entries.some(entry => entry.isIntersecting && entry.intersectionRect.height > 0 && entry.intersectionRect.width > 0)) {{
                                    window.readerAutoObserver.disconnect();
                                    document.getElementById('reader-auto-link')?.click();
                                }}
                            }});
                            window.readerAutoObserver.observe(button);
                        }})()"#)).await;
                    });
                    day::reactive::Scope::current().on_cleanup(move || task.abort());
                }
            },
        );
        watch(
            move || state.visible.get(),
            move |visible, _| {
                if !visible {
                    state.cancel();
                }
            },
        );
        watch(crate::site_browser::incognito, move |_, _| {
            state.cancel();
            state.extracted.set(None);
            state.short.set(false);
            state.active.set(false);
            state.auto_article.set(None);
            state.ready.set(false);
        });
        // A sign-in in the in-app browser: this reader drops a failed or short load, and loads
        // again at once if it is the one being read; a pager's other pages load when shown.
        watch(crate::site_browser::sessions, move |_, previous| {
            if previous.is_some() {
                state.retry_after_browsing(scene);
            }
        });
        watch(crate::reader_scripts::revision, move |_, previous| {
            if previous.is_some() {
                state.cancel();
                state.extracted.set(None);
                state.short.set(false);
                state.active.set(false);
                state.error.set(None);
                // An explicit next load must use the new script. Do not automatically
                // restart network work merely because another window saved a preference.
                state.auto_article.set(scene.selected.get_untracked());
            }
        });
        // Signals are already disposed when scope cleanup runs. Keep only the cancellation
        // cell alive here; never read or write window signals during disposal.
        day::reactive::Scope::current().on_cleanup(move || {
            article_load.select(None);
            if let Some(task) = pending.take() {
                task.abort();
            }
        });
        state
    }
}

impl ReaderView {
    pub fn current() -> Option<Self> {
        let view = Self::try_ambient().or_else(Self::focused)?;
        Some(view.shown.get().unwrap_or(view))
    }

    fn cancel(self) {
        if let Some(task) = self.task.get_untracked().take() {
            task.abort();
        }
        self.loading.set(false);
    }

    /// Only a visible load bar may initiate automatic network work.
    pub fn auto_load(self, scene: daynews_core::NewsScene) {
        let Some(article) = scene.article.get_untracked() else {
            return;
        };
        if self.visible.get_untracked()
            && self.ready.get_untracked()
            && scene.selected.get_untracked() == Some(article.id)
            && crate::reader_options::options(Some(article.feed_id)).auto
            && self.auto_article.get_untracked() != Some(article.id)
            && !self.active.get_untracked()
            && !self.loading.get_untracked()
            && self.error.get_untracked().is_none()
        {
            self.auto_article.set(Some(article.id));
            self.toggle_with_trigger(scene, "automatic");
        }
    }

    /// Opening enables this feed's auto-load override. Hiding only disables it when
    /// auto-loading is also off in the application defaults.
    pub fn toggle_from_bar(self, scene: daynews_core::NewsScene) {
        let Some(article) = scene.article.get_untracked() else {
            return;
        };
        if self.loading.get_untracked() {
            return;
        }
        let auto = !self.active.get_untracked();
        day::reactive::batch(|| {
            self.toggle(scene);
            if auto || !crate::reader_options::options(None).auto {
                crate::reader_options::OptionBinding {
                    feed: article.feed_id,
                    auto: true,
                }
                .write(auto);
            }
        });
    }

    pub fn toggle(self, scene: daynews_core::NewsScene) {
        self.toggle_with_trigger(scene, "explicit");
    }

    fn toggle_with_trigger(self, scene: daynews_core::NewsScene, trigger: &'static str) {
        if self.loading.get_untracked() {
            self.cancel();
            return;
        }
        if self.active.get_untracked() {
            self.active.set(false);
            return;
        }
        let Some(article) = scene.article.get_untracked() else {
            return;
        };
        let load = Load::start("reader_view", article.id, trigger);
        if self
            .extracted
            .with_untracked(|a| a.as_ref().is_some_and(|(id, _)| *id == article.id))
        {
            load.finish("cache_hit", serde_json::json!({
                "content_bytes": self.extracted.with_untracked(|a| a.as_ref().map_or(0, |(_, a)| a.content.len())),
            }));
            self.active.set(true);
            return;
        }
        let Some(url) = article
            .url
            .filter(|url| crate::extraction::web_url(url).is_some())
        else {
            load.finish("unavailable", serde_json::json!({"reason": "invalid_url"}));
            return;
        };
        self.error.set(None);
        self.loading.set(true);
        let task = day::task(async move {
            let provider = LocalReadability {
                engine: self.engine,
                diagnostics: &load,
            };
            let result = match futures_util::future::select(
                provider.extract(&url),
                Box::pin(day::sleep(30_000)),
            )
            .await
            {
                futures_util::future::Either::Left((result, _)) => result,
                futures_util::future::Either::Right(_) => Err(ExtractionError::Timeout),
            };
            // A delayed response must never replace another article or another window's body.
            if scene.selected.get_untracked() != Some(article.id)
                || !scene.article.with_untracked(|a| {
                    a.as_ref()
                        .is_some_and(|a| a.id == article.id && a.url.as_deref() == Some(&url))
                })
            {
                load.finish(
                    "discarded",
                    serde_json::json!({"reason": "selection_changed"}),
                );
                return;
            }
            match &result {
                Ok(content) => load.finish(
                    "extracted",
                    serde_json::json!({"content_bytes": content.content.len()}),
                ),
                Err(error) => {
                    load.finish("failed", serde_json::json!({"error": format!("{error:?}")}))
                }
            }
            day::reactive::batch(|| {
                self.loading.set(false);
                match result {
                    Ok(content) => {
                        self.short.set(looks_short(
                            &content.content,
                            article
                                .content_html
                                .as_deref()
                                .or(article.summary.as_deref()),
                        ));
                        self.extracted.set(Some((article.id, content)));
                        self.active.set(true);
                    }
                    Err(error) => self.error.set(Some(error)),
                }
            });
        });
        self.task.get_untracked().set(Some(task));
    }

    /// The in-app browser closed: a sign-in may have happened there, in the profile this
    /// reader reads cookies from. A full load that failed or came back short is dropped, and
    /// loads again here if this reader is the visible one; anything else is left as it is.
    fn retry_after_browsing(self, scene: daynews_core::NewsScene) {
        let Some(article) = scene.article.get_untracked() else {
            return;
        };
        let failed = self.error.get_untracked().is_some()
            || (self.short.get_untracked()
                && self
                    .extracted
                    .with_untracked(|e| e.as_ref().is_some_and(|(id, _)| *id == article.id)));
        if !failed || self.loading.get_untracked() {
            return;
        }
        day::reactive::batch(|| {
            self.extracted.set(None);
            self.short.set(false);
            self.error.set(None);
            self.active.set(false);
        });
        if self.visible.get_untracked() {
            self.toggle_with_trigger(scene, "after_browser");
        }
    }

    pub fn article(
        self,
        original: Option<daynews_core::StoredArticle>,
    ) -> Option<daynews_core::StoredArticle> {
        let mut article = original?;
        if self.active.get()
            && let Some((id, extracted)) = self.extracted.get()
            && id == article.id
        {
            article.content_html = Some(extracted.content);
            article.url = Some(extracted.url);
            if extracted
                .title
                .as_ref()
                .is_some_and(|s| !s.trim().is_empty())
            {
                article.title = extracted.title;
            }
            if extracted
                .byline
                .as_ref()
                .is_some_and(|s| !s.trim().is_empty())
            {
                article.author = extracted.byline;
            }
        }
        Some(article)
    }

    /// Insert sanitized full content below the RSS preview in this same document.
    pub fn show_inline_content(self, scene: daynews_core::NewsScene) {
        let Some(article) = scene.article.get_untracked() else {
            return;
        };
        if !crate::reader_options::options(Some(article.feed_id)).inline {
            return;
        }
        let active = self.active.get_untracked();
        let html = self
            .extracted
            .get_untracked()
            .filter(|(id, _)| *id == article.id)
            .map(|(_, content)| content.content)
            .unwrap_or_default();
        let html = serde_json::to_string(&html).unwrap();
        let task = day::task(async move {
            let load = Load::start(
                "reader_inline",
                article.id,
                if active { "expand" } else { "collapse" },
            );
            let result = self.engine.eval(format!(r#"(() => {{
                const slot = document.getElementById('reader-full-slot');
                if (!slot) return 'missing_slot';
                const active = {active}, x = scrollX, y = scrollY;
                if (active) {{
                    window.readerAutoObserver?.disconnect();
                    const content = {html};
                    if (slot.readerContent !== content) {{
                        slot.innerHTML = content; slot.readerContent = content; slot.dataset.loaded = 'true';
                    }}
                }}
                if (slot.dataset.expanded === String(active)) return 'unchanged';
                slot.dataset.expanded = String(active);
                const opacity = slot.hidden ? 0 : Number(getComputedStyle(slot).opacity);
                slot.readerTransition?.cancel();
                if (!slot.animate || matchMedia('(prefers-reduced-motion: reduce)').matches) {{
                    slot.hidden = !active; window.scrollTo(x, y); return 'updated';
                }}
                if (!active && slot.hidden) return 'unchanged';
                slot.hidden = false;
                const animation = slot.animate(active
                    ? [{{opacity, transform: 'translateY(12px)'}}, {{opacity: 1, transform: 'translateY(0)'}}]
                    : [{{opacity}}, {{opacity: 0}}],
                    {{duration: active ? 320 : 180, easing: 'cubic-bezier(.2,.7,.2,1)', fill: 'forwards'}});
                slot.readerTransition = animation;
                animation.onfinish = () => {{
                    if (slot.readerTransition !== animation) return;
                    slot.hidden = !active;
                    animation.cancel(); slot.readerTransition = null;
                }};
                window.scrollTo(x, y);
                return 'updated';
            }})()"#)).await;
            match result {
                Ok(status) => {
                    let status = serde_json::from_str::<String>(&status).unwrap_or(status);
                    let outcome = match status.as_str() {
                        "updated" => "dom_updated",
                        "unchanged" => "unchanged",
                        _ => "failed",
                    };
                    load.finish(
                        outcome,
                        serde_json::json!({"status": status, "script_content_bytes": html.len()}),
                    );
                }
                Err(_) => load.finish(
                    "failed",
                    serde_json::json!({"error": "evaluation_unavailable"}),
                ),
            }
        });
        // Evaluation is keyed to the current web document and a replaced document drops it.
        day::reactive::Scope::current().on_cleanup(move || task.abort());
    }

    pub fn status(self) -> String {
        use crate::res;
        if self.loading.get() {
            res::str::reader_view_loading().format()
        } else if let Some(error) = self.error.get() {
            match error {
                ExtractionError::Network => res::str::reader_view_network_error().format(),
                ExtractionError::TooLarge => res::str::reader_view_too_large().format(),
                ExtractionError::NoContent => res::str::reader_view_no_content().format(),
                ExtractionError::Timeout => res::str::reader_view_timeout().format(),
                ExtractionError::CustomScript => res::str::reader_script_failed().format(),
                _ => res::str::reader_view_unavailable().format(),
            }
        } else {
            String::new()
        }
    }
}

/// Whether extracted content reads like a teaser rather than the article: little text, or
/// hardly more than the feed already showed. A paywall or sign-in page usually extracts as one.
fn looks_short(content_html: &str, preview: Option<&str>) -> bool {
    let full = text_length(content_html);
    full < 600 || full < preview.map_or(0, text_length) + 200
}

/// The visible text's length in characters: tags dropped, runs of whitespace counted once.
fn text_length(html: &str) -> usize {
    let (mut length, mut in_tag, mut space) = (0, false, true);
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if in_tag => {}
            c if c.is_whitespace() => {
                if !space {
                    length += 1;
                    space = true;
                }
            }
            _ => {
                length += 1;
                space = false;
            }
        }
    }
    length
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn teasers_read_as_short_and_articles_do_not() {
        let summary = Some("A one-line synthetic summary.");
        assert!(looks_short("<p>Subscribe to keep reading.</p>", summary));
        let full = format!(
            "<p>{}</p>",
            "Synthetic sentence about careful work. ".repeat(40)
        );
        assert!(!looks_short(&full, summary));
        // Hardly longer than what the feed already carried is a teaser too.
        assert!(looks_short(&full, Some(full.as_str())));
        assert_eq!(text_length("<p>a  b</p>\n<p>c</p>"), 5);
    }

    fn fixture(id: u64) -> daynews_core::StoredArticle {
        daynews_core::StoredArticle {
            id,
            feed_id: 1,
            feed_title: "Fixture publication".into(),
            title: Some("RSS title".into()),
            url: Some(format!("https://fixture.example/{id}")),
            author: None,
            published_at: 0,
            summary: None,
            content_html: Some("<p>RSS excerpt</p>".into()),
            is_read: true,
            is_starred: false,
        }
    }

    fn scene() -> (daynews_core::NewsScene, ReaderView) {
        let scene = daynews_core::NewsScene::create();
        day::reactive::Scope::current().provide(scene);
        let view = ReaderView::create();
        scene.article.set(Some(fixture(1)));
        day::reactive::flush_sync();
        (scene, view)
    }

    #[test]
    fn overlay_toggles_without_mutating_the_feed_and_clears_on_selection() {
        let scope = day::reactive::Scope::child();
        scope.enter(|| {
            let (scene, view) = scene();
            view.extracted.set(Some((
                1,
                ExtractedArticle {
                    content: "<p>Full fixture article</p>".into(),
                    title: Some("Full title".into()),
                    byline: Some("Fixture author".into()),
                    url: "https://fixture.example/final".into(),
                },
            )));
            view.toggle(scene);
            assert_eq!(
                view.article(scene.article.get()).unwrap().title.as_deref(),
                Some("Full title")
            );
            assert_eq!(
                scene.article.get().unwrap().title.as_deref(),
                Some("RSS title")
            );
            view.toggle(scene);
            assert_eq!(
                view.article(scene.article.get())
                    .unwrap()
                    .content_html
                    .as_deref(),
                Some("<p>RSS excerpt</p>")
            );
            view.toggle(scene);
            scene.article.set(Some(fixture(2)));
            day::reactive::flush_sync();
            assert!(!view.active.get());
            assert!(view.extracted.get().is_none());
        });
        scope.dispose();
    }

    #[test]
    fn paging_visibility_preserves_extracted_content_and_only_cancels_pending_work() {
        let scope = day::reactive::Scope::child();
        scope.enter(|| {
            let (scene, view) = scene();
            view.extracted.set(Some((
                1,
                ExtractedArticle {
                    content: "<p>Prepared full article</p>".into(),
                    title: None,
                    byline: None,
                    url: "https://fixture.example/1".into(),
                },
            )));
            view.active.set(true);
            view.ready.set(true);
            view.visible.set(false);
            day::reactive::flush_sync();
            assert!(view.active.get_untracked());
            assert!(view.ready.get_untracked());
            assert!(view.extracted.get_untracked().is_some());
            view.auto_load(scene);
            assert!(!view.loading.get_untracked());
            view.visible.set(true);
            day::reactive::flush_sync();
            assert_eq!(
                view.article(scene.article.get())
                    .unwrap()
                    .content_html
                    .as_deref(),
                Some("<p>Prepared full article</p>")
            );
        });
        scope.dispose();
    }

    #[test]
    fn window_disposal_and_selection_cancel_pending_work() {
        struct Pending(Rc<Cell<usize>>);
        impl Drop for Pending {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        let dropped = Rc::new(Cell::new(0));
        let scope = day::reactive::Scope::child();
        scope.enter(|| {
            let (scene, view) = scene();
            for iteration in 0..2 {
                let pending = Pending(dropped.clone());
                let task = day::task(async move {
                    let _pending = pending;
                    std::future::pending::<()>().await;
                });
                view.task.get_untracked().set(Some(task));
                view.loading.set(true);
                if iteration == 0 {
                    // The next article is still loading: its predecessor remains displayed.
                    scene.selected.set(Some(2));
                    day::reactive::flush_sync();
                    assert!(task.is_finished());
                    assert!(!view.loading.get());
                    assert_eq!(dropped.get(), 1);
                }
            }
        });
        scope.dispose();
        assert_eq!(
            dropped.get(),
            2,
            "cleanup cancels without accessing disposed signals"
        );
    }
}

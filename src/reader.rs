//! The article pane: a native web view showing the feed's own HTML, wrapped in a readable
//! document of our own so text is legible and matches the app's appearance.

use crate::format::full_date;
use crate::theme::palette;
use day::prelude::*;
use day_piece_webview::{ResourceProvider, ResourceResponse, web_view_resources};
use daynews_core::StoredArticle;

fn document(a: &StoredArticle, control_above: bool) -> String {
    let mut document = document_with_style(
        a,
        day::dark_mode(),
        &day::reactive::untrack(crate::reader_styles::style),
    );
    if a.url
        .as_deref()
        .and_then(crate::extraction::web_url)
        .is_some()
    {
        let control = format!(
            r#"<div id="reader-load-control"><button type="button" id="reader-load-full" aria-busy="false" aria-expanded="false" aria-controls="reader-full-slot" onclick="document.getElementById('reader-load-link').click()"><span class="reader-load-label">{}</span><span class="reader-spinner" aria-hidden="true"></span><svg class="reader-download" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="m6 5 6 6 6-6M6 12l6 6 6-6"/></svg></button><button type="button" id="reader-load-error" hidden aria-haspopup="dialog" aria-label="{}" onclick="document.getElementById('reader-error-link').click()"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 3 2 21h20L12 3Z M12 9v5 M12 17v.1"/></svg></button><span id="reader-error-status" role="status" class="reader-sr-only"></span><a hidden id="reader-error-link" href="day-news-reader://error/{}"></a><a hidden id="reader-load-link" href="day-news-reader://load/{}"></a><a hidden id="reader-auto-link" href="day-news-reader://auto/{}"></a></div><div id="reader-browse" hidden><span id="reader-browse-note"></span><a id="reader-browse-link" href="day-news-reader://browse/{}">{}</a></div>"#,
            escape(&crate::res::str::reader_load_full().format()),
            escape(&crate::res::str::reader_view_error_details().format()),
            a.id,
            a.id,
            a.id,
            a.id,
            escape(&crate::res::str::reader_browse().format()),
        );
        document = if control_above {
            document.replace(
                "<div id=\"reader-feed-content\">",
                &format!("{control}<div id=\"reader-feed-content\">"),
            )
        } else {
            document.replace(
                "<div id=\"reader-full-slot\"></div>",
                &format!("{control}<div id=\"reader-full-slot\"></div>"),
            )
        };
    }

    document
}

#[cfg(test)]
fn document_in_appearance(a: &StoredArticle, dark: bool) -> String {
    document_with_style(a, dark, &crate::reader_styles::ReaderStyle::default())
}

fn document_with_style(
    a: &StoredArticle,
    dark: bool,
    style: &crate::reader_styles::ReaderStyle,
) -> String {
    let p = crate::theme::palette_for(dark);
    let empty_body = format!(
        "<p><em>{}</em></p>",
        escape(&crate::res::str::reader_no_content().format())
    );
    // Parser summaries are plain text; only content_html is publication markup.
    let summary = a.summary.as_deref().map(escape);
    let body = a
        .content_html
        .as_deref()
        .filter(|body| !body.trim().is_empty())
        .or_else(|| summary.as_deref().filter(|text| !text.trim().is_empty()))
        .unwrap_or(&empty_body);
    let title = a.title.as_deref().filter(|title| !title.trim().is_empty());
    let title = match title {
        Some(title) => escape(title),
        None => escape(&crate::res::str::untitled().format()),
    };
    // Relative images and links in feed HTML resolve against the publication, not our
    // application resource address. Only web URLs are suitable document bases.
    let base = a
        .url
        .as_deref()
        .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
        .map(|url| format!(r#"<base href="{}">"#, escape(url)))
        .unwrap_or_default();
    let byline = match &a.author {
        Some(author) => format!(r#" <span class="by">{}</span>"#, escape(author)),
        None => String::new(),
    };
    let when = escape(&full_date(a.published_at));
    let link = a
        .url
        .as_deref()
        .map(|u| {
            format!(
                r#"<a class="src" href="{}">{}</a>"#,
                escape(u),
                escape(&a.feed_title)
            )
        })
        .unwrap_or_else(|| escape(&a.feed_title));

    // A self-contained document with no external assets: the reader must render the same
    // offline, and pulling remote CSS would leak the reader's activity to third parties.
    format!(
        r#"<!doctype html>
<html><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
{base}
<style>
  :root {{ color-scheme: {scheme}; }}
  html {{ -webkit-text-size-adjust: 100%; font-size: 17px; }}
  html, body {{ margin: 0; padding: 0; background: {bg}; color: {fg}; }}
  body {{
    font: 17px/1.65 -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto,
          "Helvetica Neue", system-ui, sans-serif;
    padding: 28px 32px 72px; max-width: 40em; margin: 0 auto;
    overflow-wrap: break-word; word-break: break-word;
  }}
  /* Masthead: source and byline, a rule, then the headline — the order a reader's eye wants,
     and the one NetNewsWire uses. */
  .src {{ color: {accent}; text-decoration: none; font-weight: 600; font-size: 0.87em; }}
  .by {{ color: {muted}; font-size: 0.87em; }}
  .mast {{ margin: 0 0 12px; }}
  .rule {{ border: none; border-top: 1px solid {rule}; margin: 0 0 16px; }}
  h1.t {{ font-size: 1.85em; line-height: 1.2; letter-spacing: -0.012em; margin: 0 0 8px;
          font-weight: 700; }}
  .when {{ color: {muted}; font-size: 0.74em; letter-spacing: 0.06em; text-transform: uppercase;
           margin: 0 0 22px; }}
  a, #reader-load-full {{ color: {accent}; }}
  #reader-load-control {{ position: relative; margin: 28px 0 0; overflow-anchor: none; }}
  #reader-load-error {{ position: absolute; right: 10px; top: 50%; transform: translateY(-50%);
    width: 36px; height: 36px; padding: 7px; border: 0; border-radius: 4px;
    color: inherit; background: transparent; cursor: pointer; }}
  #reader-load-error svg {{ display: block; width: 22px; height: 22px; }}
  #reader-load-error:hover {{ background: color-mix(in srgb, currentColor 10%, transparent); }}
  #reader-load-error:focus-visible {{ outline: 2px solid currentColor; outline-offset: 2px; }}
  #reader-load-control[data-error="true"] .reader-download {{ visibility: hidden; }}
  .reader-sr-only {{ position: absolute; width: 1px; height: 1px; padding: 0; overflow: hidden;
    clip-path: inset(50%); white-space: nowrap; }}
  #reader-full-slot {{ overflow-anchor: none; }}
  /* Offered when Load Full Article failed or came back short: the page in the in-app browser,
     where signing in reaches the next load. */
  #reader-browse {{ display: flex; flex-wrap: wrap; align-items: baseline; gap: 4px 12px;
    margin: 10px 2px 0; font-size: .85em; color: {muted}; }}
  #reader-browse[hidden] {{ display: none; }}
  #reader-browse-note {{ flex: 1 1 16em; }}
  #reader-browse-link {{ font-weight: 600; text-decoration: none; white-space: nowrap; }}
  #reader-load-full {{ width: 100%; display: flex; align-items: center; gap: 12px;
    padding: 15px 16px; border: 1px solid {rule}; border-radius: 6px;
    background: linear-gradient(color-mix(in srgb, {alt} 80%, {bg}), {alt});
    box-shadow: inset 0 1px 0 color-mix(in srgb, {fg} 8%, transparent), 0 1px 2px #0001;
    font: inherit; cursor: pointer; }}
  .reader-load-label {{ flex: 1; text-align: left; font-size: .85em; font-weight: 600;
    letter-spacing: .015em; text-shadow: 0 1px 0 color-mix(in srgb, {bg} 75%, transparent); }}
  .reader-download {{ width: 21px; height: 21px; flex-shrink: 0; }}
  #reader-load-full[aria-expanded="true"] .reader-download {{ transform: rotate(180deg); }}
  .reader-spinner {{ width: 15px; height: 15px; flex-shrink: 0; border: 2px solid currentColor;
    border-right-color: transparent; border-radius: 50%; display: none; }}
  #reader-load-full[aria-busy="true"] .reader-spinner {{ display: block; animation: reader-spin .85s linear infinite; }}
  #reader-load-full[aria-busy="true"] .reader-download {{ display: none; }}
  @keyframes reader-spin {{ to {{ transform: rotate(360deg); }} }}
  @media (prefers-reduced-motion: reduce) {{ .reader-spinner {{ animation-duration: 2s !important; }} }}
  #reader-load-full:hover:not(:disabled) {{ background: color-mix(in srgb, currentColor 8%, transparent); }}
  #reader-load-full:focus-visible {{ outline: 2px solid currentColor; outline-offset: 3px; }}
  #reader-load-full:disabled {{ opacity: .55; cursor: progress; }}
  #reader-link-tooltip {{ position: fixed; z-index: 2147483647; pointer-events: none;
    max-width: min(680px, calc(100vw - 16px)); padding: 7px 10px;
    background: {alt}; color: {fg}; border: 1px solid {rule}; border-radius: 6px;
    box-shadow: 0 3px 12px #0003; font: 12px/1.4 system-ui, sans-serif;
    overflow-wrap: anywhere; }}
  /* Feed HTML is arbitrary: keep media inside the pane rather than forcing a sideways scroll. */
  img, video, iframe, table {{ max-width: 100%; }}
  img, video {{ height: auto; }}
  table {{ display: block; overflow-x: auto; }}
  iframe {{ border: 0; }}
  figure {{ margin: 1em 0; }}
  pre {{ background: {alt}; padding: 12px; overflow-x: auto; border-radius: 8px; }}
  code {{ font-size: 0.9em; }}
  blockquote {{ margin: 1em 0; padding-left: 1em; border-left: 3px solid {rule}; color: {muted}; }}
  hr {{ border: none; border-top: 1px solid {rule}; }}
  @supports (font: -apple-system-body) {{
    html {{ font: -apple-system-body; }}
  }}
  @media (max-width: 480px) {{
    body {{ padding: 20px 20px 48px; }}
    h1.t {{ font-size: 1.65em; }}
  }}
</style>
<style id="reader-display">{display_style}</style></head>
<body>
<p class="mast">{link}{byline}</p>
<hr class="rule">
<h1 class="t" id="reader-title">{title}</h1>
<p class="when">{when}</p>
<div id="reader-feed-content">{body}</div>
<div id="reader-full-slot"></div>
<div id="reader-link-tooltip" role="tooltip" hidden></div>
<script>
(() => {{
  const tooltip = document.getElementById('reader-link-tooltip');
  const hide = () => {{ tooltip.hidden = true; tooltip.textContent = ''; }};
  document.addEventListener('pointermove', event => {{
    if (event.pointerType && event.pointerType !== 'mouse' && event.pointerType !== 'pen') {{ hide(); return; }}
    const link = event.target.closest && event.target.closest('a[href]');
    if (!link) {{ hide(); return; }}
    tooltip.textContent = link.href;
    tooltip.hidden = false;
    const box = tooltip.getBoundingClientRect();
    const x = Math.max(8, Math.min(event.clientX + 12, innerWidth - box.width - 8));
    let y = event.clientY + 18;
    if (y + box.height > innerHeight - 8) y = Math.max(8, event.clientY - box.height - 10);
    tooltip.style.left = x + 'px'; tooltip.style.top = y + 'px';
  }});
  document.addEventListener('pointerleave', hide);
  document.addEventListener('scroll', hide, true);
  document.addEventListener('click', hide, true);
  document.addEventListener('keydown', hide);
  window.addEventListener('blur', hide);
}})();
</script>
</body></html>"#,
        display_style = crate::reader_styles::stylesheet(style, dark),
        scheme = if dark { "dark" } else { "light" },
        bg = css(p.bg),
        fg = css(p.text),
        muted = css(p.text_muted),
        accent = css(p.accent),
        alt = css(p.bg_alt),
        rule = css(p.rule),
    )
}

fn css(c: Color) -> String {
    format!(
        "#{:02x}{:02x}{:02x}",
        (c.r * 255.0).round() as u8,
        (c.g * 255.0).round() as u8,
        (c.b * 255.0).round() as u8
    )
}

/// Escape for HTML text and quoted attributes.
pub(crate) fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// The open article, rendered the way this backend can.
///
/// Where there is a web engine the generated document goes to the web view. A backend
/// without one gets a composed text reader instead of a placeholder leaf.
fn reader_body(
    provider: ResourceProvider,
    page: Signal<String>,
    go: Trigger,
    reload: Trigger,
    view: crate::reader_view::ReaderView,
    document_load: std::rc::Rc<std::cell::RefCell<Option<crate::load_diagnostics::Load>>>,
) -> AnyPiece {
    if day_piece_webview::support() == Support::Unsupported
        || (crate::site_browser::incognito()
            && day_piece_webview::private_browsing_support() == Support::Unsupported)
    {
        article_text().any()
    } else {
        let scene = daynews_core::scene();
        let js = view.engine;
        watch(
            || (crate::reader_styles::style(), day::dark_mode()),
            move |_, _| crate::reader_styles::apply_to(js),
        );
        watch(
            move || {
                (
                    view.ready.get(),
                    view.loading.get(),
                    view.active.get(),
                    view.error.get(),
                    view.short.get(),
                    day::locale().get(),
                )
            },
            move |(ready, loading, active, error, short, _), _| {
                if !ready {
                    return;
                }
                let text = if *loading {
                    crate::res::str::reader_view_loading().format()
                } else if *active {
                    crate::res::str::reader_hide_full().format()
                } else {
                    crate::res::str::reader_load_full().format()
                };
                let text = serde_json::to_string(&text).unwrap();
                // Format localized UI text before entering asynchronous web evaluation.
                let failed = error.is_some();
                let message =
                    serde_json::to_string(&if failed { view.status() } else { String::new() })
                        .unwrap();
                // The way out of a paywall or a page extraction cannot read: open it here, where
                // a sign-in lands in the profile the next load reads cookies from.
                let browse = !*loading && (failed || *short) && crate::site_browser::can_browse();
                let note = serde_json::to_string(&if failed {
                    crate::res::str::reader_browse_failed().format()
                } else {
                    crate::res::str::reader_browse_short().format()
                })
                .unwrap();
                let reason = if failed { "'failed'" } else { "'short'" };
                let script = format!(
                    "(() => {{ const button = document.getElementById('reader-load-full'); if (button) {{ button.disabled = {loading}; button.setAttribute('aria-busy', String({loading})); button.setAttribute('aria-expanded', String({active})); button.querySelector('.reader-load-label').textContent = {text}; }} const control = document.getElementById('reader-load-control'); const warning = document.getElementById('reader-load-error'); if (control && warning) {{ control.dataset.error = String({failed}); warning.hidden = !{failed}; warning.title = {message}; document.getElementById('reader-error-status').textContent = {message}; }} const browse = document.getElementById('reader-browse'); if (browse) {{ browse.hidden = !{browse}; browse.dataset.reason = {reason}; document.getElementById('reader-browse-note').textContent = {note}; }} }})()"
                );
                day::task(async move {
                    let _ = js.eval(script).await;
                });
            },
        );
        let profile = scene
            .article
            .with_untracked(|a| a.as_ref().map(|a| crate::site_browser::profile(a.feed_id)))
            .unwrap_or_default();
        web_view_resources(provider, "reader.html")
            .url_binding(page)
            .profile(profile)
            .js(js)
            .on_load(move || {
                // Resource reloads can retain the previous URL. Verify the rendered
                // document's generation, rather than attributing that URL to an article.
                let pending = document_load.clone();
                let article_load = view.article_load.get_untracked();
                let selected = scene.selected;
                let url = page.get_untracked();
                day::task(async move {
                    let observed = if day_piece_webview::eval_support() == Support::Native {
                        js.eval("document.body.dataset.dayNewsLoadId")
                            .await
                            .ok()
                            .and_then(|value| serde_json::from_str::<String>(&value).ok())
                            .and_then(|value| value.parse::<u64>().ok())
                    } else {
                        // Backends without evaluation can only validate the article URL.
                        let id = url::Url::parse(&url).ok().and_then(|url| {
                            url.query_pairs()
                                .find(|(key, _)| key == "article")
                                .and_then(|(_, id)| id.parse::<u64>().ok())
                        });
                        pending
                            .borrow()
                            .as_ref()
                            .filter(|load| Some(load.article()) == id)
                            .map(|load| load.id())
                    };
                    let mut pending = pending.borrow_mut();
                    if pending.as_ref().is_some_and(|load| {
                        Some(load.id()) == observed
                            && selected.try_get() == Some(Some(load.article()))
                    }) {
                        let load = pending.take().unwrap();
                        article_load.finish(load.article(), "loaded");
                        load.finish("loaded", serde_json::json!({}));
                    }
                });
                crate::reader_styles::apply_to(js);
                view.ready.set(true);
                view.show_inline_content(scene);
            })
            .go(go)
            .reload(reload)
            .on_external_link(move |url| {
                if !view.visible.get_untracked() {
                    return day_piece_webview::LinkPolicy::Ignore;
                }
                if let Some(id) = url
                    .strip_prefix("day-news-reader://error/")
                    .and_then(|id| id.parse::<u64>().ok())
                {
                    if scene.selected.get_untracked() == Some(id)
                        && view.error.get_untracked().is_some()
                    {
                        let message = view.status();
                        let title = crate::res::str::reader_view_failed().format();
                        let dismiss = crate::res::str::dismiss_alert().format();
                        day::task(async move {
                            alert(title)
                                .message(message)
                                .button(dismiss, ())
                                .present()
                                .await;
                        });
                    }
                    return day_piece_webview::LinkPolicy::Ignore;
                }
                if let Some(id) = url
                    .strip_prefix("day-news-reader://auto/")
                    .and_then(|id| id.parse::<u64>().ok())
                {
                    if scene.selected.get_untracked() == Some(id) {
                        view.auto_load(scene);
                    }
                    return day_piece_webview::LinkPolicy::Ignore;
                }
                if let Some(id) = url
                    .strip_prefix("day-news-reader://load/")
                    .and_then(|id| id.parse::<u64>().ok())
                {
                    if scene.selected.get_untracked() == Some(id) && !view.loading.get_untracked() {
                        view.toggle_from_bar(scene);
                    }
                    return day_piece_webview::LinkPolicy::Ignore;
                }
                let article = scene.article.with_untracked(|a| {
                    a.as_ref()
                        .map(|a| (a.id, a.feed_id, a.url.clone().unwrap_or_default()))
                });
                if let Some(id) = url
                    .strip_prefix("day-news-reader://browse/")
                    .and_then(|id| id.parse::<u64>().ok())
                {
                    // Asked for by the reader to sign in, so it opens here whatever Settings ▸
                    // Browser says about other links.
                    if let Some((article, feed, page)) = article
                        && article == id
                    {
                        crate::site_browser::browse(&page, Some(feed));
                    }
                    return day_piece_webview::LinkPolicy::Ignore;
                }
                // Every other link, the masthead's site name included, opens the way links do,
                // under the article's site profile.
                crate::site_browser::open_link(url, article.map(|(_, feed, _)| feed));
                day_piece_webview::LinkPolicy::Ignore
            })
            .id_of({
                let slot = day_core::environment::<crate::reader_pager::PageSlot>();
                move || {
                    if view.visible.get() {
                        day::id!(reader_web)
                    } else {
                        slot.map_or(day::id!(reader_web), |slot| slot.web_id())
                    }
                }
            })
            .grow()
            .any()
    }
}

fn article_text() -> impl Piece {
    let st = daynews_core::scene();
    let view = crate::reader_view::ReaderView::ambient();
    // One label per block, keyed by article and position, so a new article rebuilds the column
    // and a heading keeps its weight instead of reading as one more paragraph.
    let body = move || {
        let Some(a) = view.article(st.article.get()) else {
            return Vec::new();
        };
        let html = a
            .content_html
            .as_deref()
            .or(a.summary.as_deref())
            .unwrap_or_default();
        let mut blocks = crate::format::blocks(html);
        if blocks.is_empty() {
            blocks.push(crate::format::Block {
                text: crate::res::str::reader_no_content().format(),
                heading: false,
            });
        }
        blocks
            .into_iter()
            .enumerate()
            .map(|(i, b)| (format!("{}-{i}", a.id), b))
            .collect::<Vec<_>>()
    };
    let block = |b: crate::format::Block| {
        if b.heading {
            label(b.text)
                .font(Font::Title3)
                .weight(FontWeight::Bold)
                .color(move || palette().text)
                .any()
        } else {
            label(b.text).color(move || palette().text).any()
        }
    };
    scroll(
        column((
            label(move || st.article.get().map(|a| a.feed_title).unwrap_or_default())
                .font(Font::Footnote)
                .color(move || palette().accent),
            label(move || {
                st.article
                    .get()
                    .and_then(|a| a.title)
                    .unwrap_or_else(|| crate::res::str::untitled().format())
            })
            .font(Font::Title2)
            .bold()
            .color(move || palette().text),
            label(move || {
                st.article
                    .get()
                    .map(|a| full_date(a.published_at))
                    .unwrap_or_default()
            })
            .font(Font::Caption)
            .color(move || palette().text_muted),
            each(
                items(body, |(key, _): &(String, crate::format::Block)| {
                    key.clone()
                }),
                move |slot| block(slot.get().1),
            ),
        ))
        .spacing(10.0)
        .align(HAlign::Leading)
        .padding(Space::XL)
        .grow_w(),
    )
    .id(day::id!(reader_text))
    .grow()
}

/// The reader pane. Empty state until an article is open.
///
/// The article's commands are declared here (docs/toolbars.md): next-unread, star and mark-read
/// all act on what this pane is showing, so they ride its chrome and leave when it does. The
/// window's bar cannot see the open article, which is why they used to need mirror signals.
pub fn reader_pane() -> AnyPiece {
    if cfg!(any(
        feature = "uikit",
        feature = "material",
        feature = "arkui"
    )) {
        let scene = daynews_core::scene();
        column((
            when(
                move || scene.selected.get().is_none(),
                crate::dashboard::dashboard,
            ),
            when(
                move || scene.selected.get().is_some(),
                crate::reader_pager::reader_pager,
            ),
        ))
        .grow()
        .any()
    } else {
        reader_document(true).any()
    }
}

pub(crate) fn reader_document(chrome: bool) -> impl Piece {
    let st = daynews_core::scene();
    let view = crate::reader_view::ReaderView::ambient();
    // The resource provider serves UI-thread-rendered HTML through every backend's
    // app resource channel, whose link policy also handles the inline load action.
    // Each window owns its document; native IO callbacks never touch UI signals.
    let content = std::sync::Arc::new(std::sync::RwLock::new(Vec::<u8>::new()));
    let shared = content.clone();
    let provider = ResourceProvider::new(move |request| {
        if request.path == "reader.html" {
            ResourceResponse::new(
                "text/html; charset=utf-8",
                shared.read().unwrap_or_else(|e| e.into_inner()).clone(),
            )
        } else {
            ResourceResponse::not_found()
        }
    });
    let document_url = provider.url("reader.html");
    let page = Signal::new(document_url.clone());
    let go = Trigger::new();
    let reload = Trigger::new();
    let rendered = std::rc::Rc::new(std::cell::RefCell::new(None::<String>));
    let document_load = std::rc::Rc::new(std::cell::RefCell::new(
        None::<crate::load_diagnostics::Load>,
    ));
    let pending_document = document_load.clone();
    let cleanup_document = document_load.clone();
    day::reactive::Scope::current().on_cleanup(move || {
        cleanup_document.borrow_mut().take();
    });
    // Re-render only for article/locale changes. Typography and colors update the live
    // document through its dedicated stylesheet, preserving reading position.
    bind(
        move || {
            (
                st.article
                    .with(|a| a.as_ref().map(|a| (a.id, a.url.clone()))),
                view.active.get(),
                view.extracted.get(),
                day::locale().get(),
                crate::site_browser::incognito(),
                st.article.with(|a| {
                    a.as_ref()
                        .map(|a| crate::reader_options::options(Some(a.feed_id)))
                }),
            )
        },
        move |_| {
            let original = st.article.get_untracked();
            // A different article is a navigation, not a reload of the previous document:
            // web engines restore the old scroll offset when reloading the same URL.
            let next_page = original
                .as_ref()
                .map(|a| format!("{document_url}?article={}", a.id))
                .unwrap_or_else(|| document_url.clone());
            let key = original.as_ref().map(|a| {
                format!(
                    "{}:{:?}:{:?}:{:?}:{}",
                    a.id,
                    a.url,
                    day::locale().get_untracked(),
                    crate::reader_options::options(Some(a.feed_id)).inline,
                    crate::site_browser::incognito()
                )
            });
            let inline_live = original
                .as_ref()
                .is_some_and(|a| crate::reader_options::options(Some(a.feed_id)).inline);
            if inline_live && *rendered.borrow() == key && view.ready.get_untracked() {
                view.show_inline_content(st);
                return;
            }
            *rendered.borrow_mut() = key;
            view.ready.set(false);
            let inline = original
                .as_ref()
                .is_some_and(|a| crate::reader_options::options(Some(a.feed_id)).inline);
            let article = if inline {
                original
            } else {
                view.article(original)
            };
            let load = article.as_ref().map(|a| {
                crate::load_diagnostics::Load::start(
                    "reader_document",
                    a.id,
                    if !inline && view.active.get_untracked() {
                        "reader_view"
                    } else {
                        "feed_content"
                    },
                )
            });
            let mut doc = article
                .as_ref()
                .map(|a| document(a, !inline && view.active.get_untracked()))
                .unwrap_or_default();
            if let Some(load) = &load {
                doc = doc.replacen(
                    "<body>",
                    &format!("<body data-day-news-load-id=\"{}\">", load.id()),
                    1,
                );
                load.event(
                    "html_ready",
                    serde_json::json!({"document_bytes": doc.len()}),
                );
                view.article_load.get_untracked().event(
                    load.article(),
                    "html_ready",
                    serde_json::json!({"document_bytes": doc.len()}),
                );
            }
            *pending_document.borrow_mut() = load;
            *content.write().unwrap_or_else(|e| e.into_inner()) = doc.into_bytes();
            if (day_piece_webview::support() == Support::Unsupported
                || (crate::site_browser::incognito()
                    && day_piece_webview::private_browsing_support() == Support::Unsupported))
                && let Some(load) = pending_document.borrow_mut().take()
            {
                view.article_load
                    .get_untracked()
                    .finish(load.article(), "text_prepared");
                load.finish("text_prepared", serde_json::json!({}));
            }
            if page.get_untracked() != next_page {
                page.set(next_page);
                go.notify();
            } else {
                reload.notify();
            }
        },
    );

    column((
        when(
            move || st.article.get().is_none(),
            move || {
                if chrome {
                    crate::dashboard::dashboard().any()
                } else {
                    let load = day_core::environment::<crate::reader_pager::PageSlot>()
                        .unwrap()
                        .1;
                    column((
                        when(
                            move || load.get() == crate::reader_pager::PageLoad::Loading,
                            spinner,
                        ),
                        label(move || match load.get() {
                            crate::reader_pager::PageLoad::Loading => {
                                crate::res::str::reader_page_loading().format()
                            }
                            crate::reader_pager::PageLoad::Unavailable => {
                                crate::res::str::reader_page_unavailable().format()
                            }
                            crate::reader_pager::PageLoad::Failed => {
                                crate::res::str::reader_page_failed().format()
                            }
                        }),
                    ))
                    .spacing(Space::M)
                    .grow()
                    .any()
                }
            },
        ),
        when(
            move || st.article.get().is_some(),
            move || {
                let provider = provider.clone();
                let document_load = document_load.clone();
                keyed(
                    move || {
                        st.article.with(|a| {
                            a.as_ref()
                                .map(|a| crate::site_browser::profile(a.feed_id).storage_key())
                        })
                    },
                    move |_| {
                        reader_body(
                            provider.clone(),
                            page,
                            go,
                            reload,
                            view,
                            document_load.clone(),
                        )
                    },
                )
            },
        ),
    ))
    .background(move || palette().bg)
    .grow()
    // The commands follow the open article: a new one re-derives the list with its own starred
    // and read state, and no article at all contributes nothing, so the bar carries them only
    // while there is something to act on (https://daybrite.dev/docs/reference/toolbars).
    .toolbar(move || match st.article.get().filter(|_| chrome) {
        Some(a) => crate::toolbar::article_items(a),
        None => Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn document_escapes_metadata_and_resolves_publication_links() {
        // Synthetic publication data, not app-owned UI strings.
        let article = daynews_core::StoredArticle {
            id: 1,
            feed_id: 1,
            feed_title: "Fixture & source".into(),
            title: Some("<Fixture title>".into()),
            url: Some("https://fixture.example/posts/story?x=1&y=2".into()),
            author: Some("<Fixture author>".into()),
            published_at: 0,
            summary: None,
            content_html: Some(r#"<p><img src="../image.png"></p>"#.into()),
            is_read: false,
            is_starred: false,
        };
        let html = super::document_in_appearance(&article, false);
        assert!(html.contains("&lt;Fixture title&gt;"));
        assert!(html.contains("&lt;Fixture author&gt;"));
        assert!(html.contains(r#"<base href="https://fixture.example/posts/story?x=1&amp;y=2">"#));
        assert!(html.contains(r#"<img src="../image.png">"#));
    }
}

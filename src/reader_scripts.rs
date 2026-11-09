// SPDX-License-Identifier: Apache-2.0
//! User-authored extraction hooks. Preferences store copies, never paths to mutable files.
use crate::res;
use day::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const KEY: &str = "news.reader.scripts.v1";
const MAX_SCRIPT_BYTES: usize = 256 * 1024;
// Executable API example, not interface copy or a bundled resource name.
const TEMPLATE: &str = "function extract(document, url, Readability) {\n  return new Readability(document, { maxElemsToParse: 50000 }).parse();\n}\n";

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Scripts {
    global: Option<String>,
    sites: BTreeMap<String, String>,
}
impl Scripts {
    fn select(&self, url: &str) -> Option<&str> {
        origin(url)
            .and_then(|origin| self.sites.get(&origin))
            .or(self.global.as_ref())
            .map(String::as_str)
    }
    fn get(&self, site: Option<&str>) -> Option<&str> {
        match site {
            Some(site) => self.sites.get(site),
            None => self.global.as_ref(),
        }
        .map(String::as_str)
    }
    fn put(&mut self, site: Option<&str>, source: Option<String>) {
        match (site, source) {
            (Some(site), Some(source)) => {
                self.sites.insert(site.to_owned(), source);
            }
            (Some(site), None) => {
                self.sites.remove(site);
            }
            (None, source) => self.global = source,
        }
    }
}
thread_local! {
    static REVISION: std::cell::OnceCell<Signal<u64>> = const { std::cell::OnceCell::new() };
}
pub fn revision() -> u64 {
    revision_signal().get()
}
fn revision_signal() -> Signal<u64> {
    REVISION.with(|slot| *slot.get_or_init(|| Signal::new_in(day::reactive::Scope::root(), 0)))
}
pub fn origin(url: &str) -> Option<String> {
    crate::extraction::web_url(url.trim()).map(|url| url.origin().ascii_serialization())
}
fn read() -> Scripts {
    day::prefs::get(KEY)
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}
pub fn for_url(url: &str) -> Option<String> {
    read().select(url).map(str::to_owned)
}
fn save(site: Option<&str>, source: Option<String>) {
    let mut scripts = read();
    scripts.put(site, source);
    day::prefs::set(KEY, &serde_json::to_string(&scripts).unwrap());
    revision_signal().update(|value| *value = value.wrapping_add(1));
}
fn valid_source(source: &str) -> bool {
    !source.trim().is_empty() && source.len() <= MAX_SCRIPT_BYTES && !source.contains('\0')
}

/// Settings ▸ Reader: the app script and the site-script manager, with their editor sheet.
pub fn settings_controls() -> impl Piece {
    controls(None)
}
/// A feed dashboard's Site Reader Script… button, with its editor sheet.
pub fn feed_controls(feed: u64) -> impl Piece {
    controls(Some(feed))
}
fn controls(feed: Option<u64>) -> impl Piece {
    // Empty string is the app default. Other values open an origin-specific editor.
    let open = Signal::new(None::<String>);
    column((
        row((
            button(if feed.is_some() {
                res::str::reader_script_edit()
            } else {
                res::str::reader_script_app()
            })
            .action(move || {
                open.set(Some(match feed {
                    Some(feed) => {
                        crate::site_browser::site_url(feed).unwrap_or_else(|| "https://".into())
                    }
                    None => String::new(),
                }));
            })
            .id(if feed.is_some() {
                day::id!(feed_reader_script)
            } else {
                day::id!(app_reader_script)
            }),
            when(
                move || feed.is_none(),
                move || {
                    button(res::str::reader_script_sites())
                        .action(move || open.set(Some("https://".into())))
                        .id(day::id!(site_reader_scripts))
                },
            ),
        ))
        .spacing(Space::S),
        cover(open, move |target: &String| editor(open, target))
            .sheet()
            .background(|_| crate::theme::palette().bg)
            .unrouted(),
    ))
    .spacing(Space::XS)
}

fn editor(open: Signal<Option<String>>, target: &str) -> impl Piece + use<> {
    let site = !target.is_empty();
    let address = Signal::new(origin(target).unwrap_or_else(|| target.to_owned()));
    let source = Signal::new(if site && origin(target).is_none() {
        String::new()
    } else {
        read()
            .get(origin(target).as_deref())
            .unwrap_or_default()
            .to_owned()
    });
    let status = Signal::new(String::new());
    let installed: Vec<String> = read().sites.into_keys().collect();
    // The picker binds the position: 0 is "new site", then the installed origins in order.
    let mut choices = vec![res::str::reader_script_new_site().format()];
    choices.extend(installed.iter().cloned());
    let choices: Vec<(usize, String)> = choices.into_iter().enumerate().collect();
    let selected = Signal::new(
        installed
            .iter()
            .position(|s| s == &address.get_untracked())
            .map_or(0, |i| i + 1),
    );
    watch(
        move || selected.get(),
        move |index, previous| {
            if previous.is_some() {
                address.set(
                    index
                        .checked_sub(1)
                        .and_then(|i| installed.get(i))
                        .cloned()
                        .unwrap_or_else(|| "https://".into()),
                );
            }
        },
    );
    watch(
        move || if site { origin(&address.get()) } else { None },
        move |origin, previous| {
            if previous == Some(origin) {
                return;
            }
            source.set(if site && origin.is_none() {
                String::new()
            } else {
                read().get(origin.as_deref()).unwrap_or_default().to_owned()
            });
            status.set(String::new());
        },
    );
    let destination = move || {
        if site {
            origin(&address.get_untracked())
        } else {
            None
        }
    };
    let content = column((
        label(if site {
            res::str::reader_script_site()
        } else {
            res::str::reader_script_app()
        })
        .font(Font::Headline),
        when(
            move || site,
            move || {
                column((
                    picker(selected, choices.clone()).id(day::id!(reader_script_sites)),
                    labeled(
                        res::str::reader_script_address(),
                        text_field(address)
                            .id(day::id!(reader_script_address))
                            .grow(Dimension::Width),
                    ),
                    label(move || {
                        origin(&address.get())
                            .unwrap_or_else(|| res::str::reader_script_invalid_site().format())
                    })
                    .font(Font::Caption)
                    .id(day::id!(reader_script_origin)),
                ))
                .spacing(Space::XS)
            },
        ),
        label(res::str::reader_script_help())
            .font(Font::Footnote)
            .grow(Dimension::Width),
        label(res::str::reader_script_trust())
            .font(Font::Caption)
            .grow(Dimension::Width),
        text_area(source)
            .spellcheck(false)
            .min_lines(8)
            .max_lines(14)
            .id(day::id!(reader_script_source))
            .grow(Dimension::Both),
        row((
            button(res::str::reader_script_template())
                .action(move || source.set(TEMPLATE.to_owned()))
                .id(day::id!(reader_script_template)),
            button(res::str::reader_script_import())
                .action(move || {
                    let task = day::task(async move {
                        let Some(file) = open_file()
                            .filter(res::str::reader_script_filter().format(), &["js"])
                            .await
                        else {
                            return;
                        };
                        match file
                            .read()
                            .ok()
                            .and_then(|bytes| String::from_utf8(bytes).ok())
                            .filter(|text| valid_source(text))
                        {
                            Some(text) => {
                                source.set(text);
                                status.set(String::new());
                            }
                            None => status.set(res::str::reader_script_import_failed().format()),
                        }
                    });
                    day::reactive::Scope::current().on_cleanup(move || task.abort());
                })
                .id(day::id!(reader_script_import)),
            button(res::str::reader_script_export())
                .action(move || {
                    let bytes = source.get_untracked().into_bytes();
                    day::task(async move {
                        save_file(bytes)
                            .suggested_name("Fleet-Reader-Script.js")
                            .filter(res::str::reader_script_filter().format(), &["js"])
                            .await;
                    });
                })
                .enabled(move || !source.get().trim().is_empty())
                .id(day::id!(reader_script_export)),
        ))
        .spacing(Space::S),
        label(move || status.get())
            .font(Font::Caption)
            .id(day::id!(reader_script_status))
            .grow(Dimension::Width),
        row((
            button(res::str::reader_script_remove())
                .action(move || {
                    let origin = destination();
                    if site && origin.is_none() {
                        return;
                    }
                    save(origin.as_deref(), None);
                    source.set(String::new());
                    status.set(res::str::reader_script_removed().format());
                })
                .enabled(move || !site || origin(&address.get()).is_some())
                .id(day::id!(reader_script_remove)),
            spacer().grow(Dimension::Width),
            button(res::str::site_browser_close())
                .action(move || open.set(None))
                .id(day::id!(reader_script_close)),
            button(res::str::reader_script_save())
                .action(move || {
                    let origin = destination();
                    if site && origin.is_none() {
                        status.set(res::str::reader_script_invalid_site().format());
                        return;
                    }
                    let text = source.get_untracked();
                    if !valid_source(&text) {
                        status.set(res::str::reader_script_invalid_source().format());
                        return;
                    }
                    save(origin.as_deref(), Some(text));
                    status.set(res::str::reader_script_saved().format());
                })
                .id(day::id!(reader_script_save)),
        ))
        .spacing(Space::S),
    ))
    .spacing(Space::S)
    .padding(Space::L)
    .background(|| crate::theme::palette().bg)
    .grow(Dimension::Both)
    .id(day::id!(reader_script_editor));
    if cfg!(any(feature = "appkit", feature = "uikit")) {
        content.any()
    } else {
        zstack((content.max_width(640.0),))
            .grow(Dimension::Both)
            .any()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scripts_round_trip_and_sites_override_the_app_default() {
        let mut scripts = Scripts::default();
        scripts.put(None, Some("global fixture".into()));
        let site = origin("https://EXAMPLE.com:443/path?q=1#fragment").unwrap();
        assert_eq!(site, "https://example.com");
        scripts.put(Some(&site), Some("site fixture".into()));
        let restored: Scripts =
            serde_json::from_str(&serde_json::to_string(&scripts).unwrap()).unwrap();
        assert_eq!(
            restored.select("https://example.com/another"),
            Some("site fixture")
        );
        for url in [
            "http://example.com",
            "https://example.com:8443",
            "https://sub.example.com",
            "https://example.com.evil.test",
        ] {
            assert_eq!(restored.select(url), Some("global fixture"));
        }
        scripts.put(Some(&site), None);
        assert_eq!(scripts.select(&site), Some("global fixture"));
        scripts.put(None, None);
        assert_eq!(scripts.select(&site), None);
        assert!(origin("https://user:password@example.com").is_none());
        assert!(origin("file:///tmp/fixture").is_none());
    }
    #[test]
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    fn installed_scripts_survive_process_restart_and_removal() {
        const CHILD: &str = "DAY_NEWS_TEST_READER_SCRIPTS";
        const SITE: &str = "https://reader-script-fixture.example";
        if let Ok(mode) = std::env::var(CHILD) {
            match mode.as_str() {
                "install" => {
                    save(None, Some("global fixture".into()));
                    save(Some(SITE), Some("site fixture".into()));
                }
                "read" => {
                    assert_eq!(for_url(SITE).as_deref(), Some("site fixture"));
                    assert_eq!(
                        for_url("https://other-fixture.example").as_deref(),
                        Some("global fixture")
                    );
                }
                "remove" => {
                    save(Some(SITE), None);
                    save(None, None);
                }
                "empty" => assert!(for_url(SITE).is_none()),
                _ => panic!("unknown fixture mode"),
            }
            return;
        }
        let preferences = crate::test_prefs::PreferenceTest::new(KEY);
        for mode in ["install", "read", "remove", "empty"] {
            let output = preferences
                .child(
                    "reader_scripts::tests::installed_scripts_survive_process_restart_and_removal",
                )
                .env(CHILD, mode)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{mode}: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
    #[test]
    fn import_rejects_empty_binary_and_oversized_scripts() {
        assert!(valid_source(TEMPLATE));
        assert!(!valid_source(" \n"));
        assert!(!valid_source("function extract(){}\0"));
        assert!(!valid_source(&"x".repeat(MAX_SCRIPT_BYTES + 1)));
    }
}

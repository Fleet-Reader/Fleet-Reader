// SPDX-License-Identifier: Apache-2.0
//! Small display helpers: relative dates and text snippets, the way a reader shows them.

/// Compact, localized ages for recent articles and a local calendar date for older ones.
pub fn relative_time(unix_secs: i64) -> String {
    let age = daynews_db::now_unix().saturating_sub(unix_secs);
    match age {
        a if a < 60 => crate::res::str::time_now().format(),
        a if a < 3_600 => crate::res::str::time_minutes((a / 60) as f64).format(),
        a if a < 86_400 => crate::res::str::time_hours((a / 3_600) as f64).format(),
        a if a < 7 * 86_400 => crate::res::str::time_days((a / 86_400) as f64).format(),
        _ => crate::res::str::timeline_date(local_time(unix_secs) as f64).format(),
    }
}

/// Fluent formats numeric instants as UTC civil time. Apply the host's offset at the
/// publication instant first, including daylight saving, to show the reader's local time.
fn local_time(unix_secs: i64) -> i64 {
    unix_secs.saturating_add(i64::from(
        daynews_time::local_offset_seconds(unix_secs).unwrap_or(0),
    ))
}

/// When the library was last brought up to date, for the list's status line: "Updated just
/// now", minutes for the first hour, then the time of day, then the date.
pub fn updated_status(unix_secs: i64, now: i64) -> String {
    let age = now.saturating_sub(unix_secs).max(0);
    if age < 60 {
        crate::res::str::list_status_updated_now().format()
    } else if age < 3_600 {
        crate::res::str::list_status_updated_minutes((age / 60) as f64).format()
    } else if daynews_time::start_of_day(unix_secs) == daynews_time::start_of_day(now) {
        crate::res::str::list_status_updated_time(local_time(unix_secs) as f64).format()
    } else {
        crate::res::str::list_status_updated_date(local_time(unix_secs) as f64).format()
    }
}

pub fn full_date(unix_secs: i64) -> String {
    crate::res::str::article_date(local_time(unix_secs) as f64).format()
}

/// The article's text as paragraphs, for the reader on a backend with no web engine to render
/// its HTML (macos-gtk; see `reader::article_text`). Block-level tags break paragraphs, inline
/// markup drops out, `script`/`style` bodies are skipped, and the entities feed HTML uses are
/// decoded. Text inside `h1`–`h6` comes back marked as a heading, so the reader can
/// set it apart instead of showing it as one more paragraph.
pub fn blocks(html: &str) -> Vec<Block> {
    const BLOCKS: [&str; 14] = [
        "p",
        "div",
        "br",
        "li",
        "tr",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "blockquote",
        "pre",
        "figcaption",
    ];
    fn flush(cur: &mut String, heading: bool, out: &mut Vec<Block>) {
        let text = decode_entities(cur.trim());
        if !text.is_empty() {
            out.push(Block { text, heading });
        }
        cur.clear();
    }

    let mut out = Vec::new();
    let mut cur = String::new();
    let mut tag = String::new();
    let mut in_tag = false;
    // Whether the text being gathered sits inside an `h1`–`h6`.
    let mut heading = false;
    // `script`/`style` text is markup's, not the reader's.
    let mut skipping = 0usize;
    for c in html.chars() {
        match c {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '>' if in_tag => {
                in_tag = false;
                let closing = tag.starts_with('/');
                let name: String = tag
                    .trim_start_matches('/')
                    .chars()
                    .take_while(char::is_ascii_alphanumeric)
                    .flat_map(char::to_lowercase)
                    .collect();
                match name.as_str() {
                    "script" | "style" => {
                        skipping = if closing {
                            skipping.saturating_sub(1)
                        } else {
                            skipping + 1
                        }
                    }
                    n if BLOCKS.contains(&n) => {
                        flush(&mut cur, heading, &mut out);
                        // `h1`–`h6` are the only two-letter `h` names in BLOCKS.
                        heading = !closing && n.len() == 2 && n.starts_with('h');
                    }
                    _ => {}
                }
            }
            _ if in_tag => tag.push(c),
            _ if skipping > 0 => {}
            c if c.is_whitespace() => {
                if !cur.is_empty() && !cur.ends_with(' ') {
                    cur.push(' ');
                }
            }
            c => cur.push(c),
        }
    }
    flush(&mut cur, heading, &mut out);
    out
}

/// One block of an article's text, as [`blocks`] splits it.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub text: String,
    /// Set for text inside `h1`–`h6`.
    pub heading: bool,
}

/// The entities feed HTML uses, named and numeric. Anything else stays as written:
/// showing `&frac34;` beats swallowing the text around it.
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        // A real entity is short; an unescaped `&` in prose is not the start of one.
        match tail.find(';').filter(|end| *end <= 10) {
            Some(end) => {
                let name = &tail[1..end];
                let decoded = match name {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    "nbsp" => Some(' '),
                    "hellip" => Some('…'),
                    "mdash" => Some('—'),
                    "ndash" => Some('–'),
                    "lsquo" => Some('\u{2018}'),
                    "rsquo" => Some('\u{2019}'),
                    "ldquo" => Some('\u{201c}'),
                    "rdquo" => Some('\u{201d}'),
                    n => n
                        .strip_prefix("#x")
                        .or_else(|| n.strip_prefix("#X"))
                        .and_then(|h| u32::from_str_radix(h, 16).ok())
                        .or_else(|| n.strip_prefix('#').and_then(|d| d.parse::<u32>().ok()))
                        .and_then(char::from_u32),
                };
                match decoded {
                    Some(c) => out.push(c),
                    None => out.push_str(&tail[..=end]),
                }
                rest = &tail[end + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// One line of plain text for the timeline's preview, from a summary that may be HTML.
pub fn snippet(s: &str, max: usize) -> String {
    let mut out = String::with_capacity(max);
    let mut in_tag = false;
    let mut last_space = true;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if in_tag => {}
            c if c.is_whitespace() => {
                if !last_space {
                    out.push(' ');
                    last_space = true;
                }
            }
            c => {
                out.push(c);
                last_space = false;
            }
        }
        if out.chars().count() >= max {
            out.push('…');
            break;
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The blocks' text alone, for the cases that are not about headings.
    fn texts(html: &str) -> Vec<String> {
        blocks(html).into_iter().map(|b| b.text).collect()
    }

    #[test]
    fn blocks_break_on_block_tags_and_drop_inline_markup() {
        let html = "<p>First <b>bold</b> line.</p><p>Second line.</p>";
        assert_eq!(texts(html), ["First bold line.", "Second line."]);
    }

    #[test]
    fn blocks_skip_script_and_style_bodies() {
        let html = "<style>p { color: red }</style><p>Visible.</p><script>var x = 1;</script>";
        assert_eq!(texts(html), ["Visible."]);
    }

    #[test]
    fn blocks_decode_the_entities_feeds_use() {
        // `&frac34;` is not in the table, and text around an unknown entity must survive it.
        let html = "<p>Tom &amp; Jerry &mdash; &#8220;quoted&#8221; &#x2019;s &frac34;</p>";
        assert_eq!(
            texts(html),
            ["Tom & Jerry \u{2014} \u{201c}quoted\u{201d} \u{2019}s &frac34;"]
        );
    }

    #[test]
    fn blocks_collapse_whitespace_and_drop_empty_blocks() {
        let html = "<div>\n  spaced   out\n</div><p></p><p>  </p><li>Item</li>";
        assert_eq!(texts(html), ["spaced out", "Item"]);
    }

    #[test]
    fn bare_text_is_one_block() {
        assert_eq!(texts("Just text"), ["Just text"]);
        assert!(blocks("   ").is_empty());
        assert!(blocks("").is_empty());
    }

    #[test]
    fn heading_text_is_marked_and_what_follows_is_not() {
        let html = "<p>Intro.</p><h3>A <em>compass</em> that keeps time</h3><p>Body.</p><hr>After";
        let heading = |text: &str, heading| Block {
            text: text.into(),
            heading,
        };
        assert_eq!(
            blocks(html),
            [
                heading("Intro.", false),
                heading("A compass that keeps time", true),
                heading("Body.", false),
                heading("After", false),
            ]
        );
    }

    #[test]
    fn an_unterminated_ampersand_is_left_alone() {
        assert_eq!(decode_entities("Fish & chips"), "Fish & chips");
    }
}

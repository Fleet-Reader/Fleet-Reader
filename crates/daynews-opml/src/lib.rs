// SPDX-License-Identifier: Apache-2.0
//! OPML subscription lists: the interchange format every feed reader speaks.
//!
//! Import and export are lossy in one direction only: we read every `<outline>`
//! attribute we understand and ignore the rest, but we never drop a subscription. Folders nest
//! (NetNewsWire writes one level; the format allows arbitrary depth, so we handle depth).

use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer};
use std::io::Cursor;

/// A subscription list: the `<head><title>` plus the top-level outline forest.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Opml {
    pub title: Option<String>,
    pub outlines: Vec<Outline>,
}

/// One `<outline>`: either a folder (no `xmlUrl`) or a subscription.
#[derive(Debug, Clone, PartialEq)]
pub enum Outline {
    Folder {
        title: String,
        children: Vec<Outline>,
    },
    Feed(FeedRef),
}

/// A subscription. `xml_url` is the feed itself; `html_url` is the human-facing site.
#[derive(Debug, Clone, PartialEq)]
pub struct FeedRef {
    /// As written in the file. Legitimately empty for a subscription that has never been
    /// fetched: a reader records the URL when you subscribe and only learns the name from the
    /// feed's `<title>` on first refresh. Real exports contain these; use
    /// [`display_title`](Self::display_title) for anything user-facing.
    pub title: String,
    pub xml_url: String,
    pub html_url: Option<String>,
}

impl FeedRef {
    /// A name that is never empty: the recorded title, else the feed URL's host (what
    /// NetNewsWire shows for a subscription it has not fetched yet), else the raw URL.
    pub fn display_title(&self) -> String {
        let t = self.title.trim();
        if !t.is_empty() {
            return t.to_string();
        }
        host_of(&self.xml_url).unwrap_or_else(|| self.xml_url.clone())
    }
}

/// The host of an absolute URL, without pulling in a URL parser for one field.
fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.rsplit_once('@').map(|(_, h)| h).unwrap_or(host);
    let host = host.split_once(':').map(|(h, _)| h).unwrap_or(host);
    let host = host.strip_prefix("www.").unwrap_or(host);
    (!host.is_empty()).then(|| host.to_string())
}

#[derive(Debug)]
pub enum OpmlError {
    Xml(String),
    /// No `<opml>`/`<body>` element; almost certainly not an OPML file.
    NotOpml,
}

impl std::fmt::Display for OpmlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpmlError::Xml(e) => write!(f, "malformed OPML: {e}"),
            OpmlError::NotOpml => write!(f, "not an OPML document (no <body> of outlines)"),
        }
    }
}

impl std::error::Error for OpmlError {}

impl Opml {
    /// Every subscription in the document, depth-first, paired with its folder path.
    pub fn feeds(&self) -> Vec<(Vec<String>, &FeedRef)> {
        let mut out = Vec::new();
        collect(&self.outlines, &mut Vec::new(), &mut out);
        out
    }
}

fn collect<'a>(
    outlines: &'a [Outline],
    path: &mut Vec<String>,
    out: &mut Vec<(Vec<String>, &'a FeedRef)>,
) {
    for o in outlines {
        match o {
            Outline::Feed(f) => out.push((path.clone(), f)),
            Outline::Folder { title, children } => {
                path.push(title.clone());
                collect(children, path, out);
                path.pop();
            }
        }
    }
}

/// Parse an OPML document. Unknown attributes and elements are ignored, and an `<outline>`
/// carrying an `xmlUrl` is a subscription however it is otherwise labeled; real exports
/// disagree about `type="rss"` and about which of `text`/`title` carries the name.
pub fn parse(src: &str) -> Result<Opml, OpmlError> {
    let mut reader = Reader::from_str(src);
    reader.config_mut().trim_text(true);
    reader.config_mut().check_end_names = false;

    let mut doc = Opml::default();
    // The outline stack: each level collects children until its `</outline>` arrives.
    let mut stack: Vec<(String, Vec<Outline>)> = Vec::new();
    let mut saw_body = false;
    let mut in_head_title = false;

    loop {
        match reader.read_event() {
            Err(e) => return Err(OpmlError::Xml(e.to_string())),
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => match e.local_name().as_ref() {
                b"body" => saw_body = true,
                b"title" => in_head_title = true,
                b"outline" => {
                    let o = read_outline(&e)?;
                    match o {
                        // A folder: push a frame; its children accumulate until `</outline>`.
                        Outline::Folder { title, .. } => stack.push((title, Vec::new())),
                        // A subscription written with a start tag (it may still nest, but its
                        // own identity is settled): keep collecting into a frame so any
                        // children are not lost, then flatten on close.
                        Outline::Feed(f) => {
                            push(&mut stack, &mut doc, Outline::Feed(f));
                            stack.push((String::new(), Vec::new()));
                        }
                    }
                }
                _ => {}
            },
            Ok(Event::Empty(e)) => {
                if e.local_name().as_ref() == b"outline" {
                    let o = read_outline(&e)?;
                    push(&mut stack, &mut doc, o);
                }
            }
            Ok(Event::End(e)) => match e.local_name().as_ref() {
                b"title" => in_head_title = false,
                b"outline" => {
                    if let Some((title, children)) = stack.pop() {
                        // An empty title marks the placeholder frame pushed for a feed that
                        // was written with a start tag; its children (if any) belong outside.
                        if title.is_empty() {
                            for c in children {
                                push(&mut stack, &mut doc, c);
                            }
                        } else {
                            push(&mut stack, &mut doc, Outline::Folder { title, children });
                        }
                    }
                }
                _ => {}
            },
            Ok(Event::Text(t)) if in_head_title && doc.title.is_none() => {
                let s = t.unescape().unwrap_or_default().trim().to_string();
                if !s.is_empty() {
                    doc.title = Some(s);
                }
            }
            _ => {}
        }
    }

    if !saw_body {
        return Err(OpmlError::NotOpml);
    }
    Ok(doc)
}

/// Append `o` to the innermost open folder, or to the document root.
fn push(stack: &mut [(String, Vec<Outline>)], doc: &mut Opml, o: Outline) {
    match stack.last_mut() {
        Some((_, children)) => children.push(o),
        None => doc.outlines.push(o),
    }
}

fn read_outline(e: &BytesStart<'_>) -> Result<Outline, OpmlError> {
    let (mut text, mut title, mut xml_url, mut html_url) = (None, None, None, None);
    for a in e.attributes().flatten() {
        let v = a
            .decode_and_unescape_value(quick_xml::Decoder {})
            .map(|c| c.into_owned())
            .unwrap_or_default();
        match a.key.local_name().as_ref() {
            b"text" => text = Some(v),
            b"title" => title = Some(v),
            b"xmlUrl" => xml_url = Some(v),
            b"htmlUrl" => html_url = Some(v),
            _ => {}
        }
    }
    // `title` is the documented name attribute but `text` is the one every reader actually
    // writes; accept either, preferring whichever is non-empty.
    let name = title
        .filter(|s| !s.trim().is_empty())
        .or(text)
        .unwrap_or_default();
    match xml_url.filter(|u| !u.trim().is_empty()) {
        Some(xml_url) => Ok(Outline::Feed(FeedRef {
            title: name,
            xml_url,
            html_url: html_url.filter(|u| !u.trim().is_empty()),
        })),
        None => Ok(Outline::Folder {
            title: name,
            children: Vec::new(),
        }),
    }
}

/// Serialize to OPML 1.1, the dialect NetNewsWire, Feedly and Reeder all import.
pub fn write(doc: &Opml) -> Result<String, OpmlError> {
    let mut w = Writer::new_with_indent(Cursor::new(Vec::new()), b'\t', 1);
    let map = |e: quick_xml::Error| OpmlError::Xml(e.to_string());
    let map_io = |e: std::io::Error| OpmlError::Xml(e.to_string());

    w.write_event(Event::Decl(quick_xml::events::BytesDecl::new(
        "1.0",
        Some("UTF-8"),
        None,
    )))
    .map_err(map_io)?;
    let mut opml = BytesStart::new("opml");
    opml.push_attribute(("version", "1.1"));
    w.write_event(Event::Start(opml)).map_err(map_io)?;

    w.write_event(Event::Start(BytesStart::new("head")))
        .map_err(map_io)?;
    if let Some(t) = &doc.title {
        w.write_event(Event::Start(BytesStart::new("title")))
            .map_err(map_io)?;
        w.write_event(Event::Text(BytesText::new(t)))
            .map_err(map_io)?;
        w.write_event(Event::End(BytesEnd::new("title")))
            .map_err(map_io)?;
    }
    w.write_event(Event::End(BytesEnd::new("head")))
        .map_err(map_io)?;

    w.write_event(Event::Start(BytesStart::new("body")))
        .map_err(map_io)?;
    for o in &doc.outlines {
        write_outline(&mut w, o).map_err(map)?;
    }
    w.write_event(Event::End(BytesEnd::new("body")))
        .map_err(map_io)?;
    w.write_event(Event::End(BytesEnd::new("opml")))
        .map_err(map_io)?;

    String::from_utf8(w.into_inner().into_inner())
        .map_err(|e| OpmlError::Xml(format!("non-UTF-8 output: {e}")))
}

fn write_outline(w: &mut Writer<Cursor<Vec<u8>>>, o: &Outline) -> Result<(), quick_xml::Error> {
    match o {
        Outline::Feed(f) => {
            let mut e = BytesStart::new("outline");
            // `text` and `title` both written: readers disagree about which they honour.
            e.push_attribute(("text", f.title.as_str()));
            e.push_attribute(("title", f.title.as_str()));
            e.push_attribute(("type", "rss"));
            e.push_attribute(("xmlUrl", f.xml_url.as_str()));
            if let Some(h) = &f.html_url {
                e.push_attribute(("htmlUrl", h.as_str()));
            }
            w.write_event(Event::Empty(e))
                .map_err(|e| quick_xml::Error::Io(std::sync::Arc::new(e)))?;
        }
        Outline::Folder { title, children } => {
            let mut e = BytesStart::new("outline");
            e.push_attribute(("text", title.as_str()));
            e.push_attribute(("title", title.as_str()));
            w.write_event(Event::Start(e))
                .map_err(|e| quick_xml::Error::Io(std::sync::Arc::new(e)))?;
            for c in children {
                write_outline(w, c)?;
            }
            w.write_event(Event::End(BytesEnd::new("outline")))
                .map_err(|e| quick_xml::Error::Io(std::sync::Arc::new(e)))?;
        }
    }
    Ok(())
}

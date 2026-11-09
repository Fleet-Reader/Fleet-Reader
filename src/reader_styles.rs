// SPDX-License-Identifier: Apache-2.0
//! App-wide reader presentation: the stylesheet built from the reader prefs in `settings`, the
//! color bindings the settings rows edit, and the reset every "Reset Styles" goes through.
use crate::settings::{
    LIST_SCALE, PREVIEW_LINES, READER_BACKGROUND, READER_FONT, READER_SCALE, READER_TEXT,
};
use day::prelude::*;
use day_piece_settings::prelude::*;

pub const DEFAULT_SCALE: f64 = 140.0;
pub const MIN_SCALE: f64 = 70.0;
pub const MAX_SCALE: f64 = 250.0;

/// The four reader prefs resolved into one value the document and the stylesheet take.
#[derive(Clone, Debug, PartialEq)]
pub struct ReaderStyle {
    pub scale: f64,
    pub family: String,
    pub background: Option<Color>,
    pub text: Option<Color>,
}

impl Default for ReaderStyle {
    fn default() -> Self {
        Self {
            scale: DEFAULT_SCALE,
            family: String::new(),
            background: None,
            text: None,
        }
    }
}

fn scale(value: f64) -> f64 {
    if value.is_finite() {
        ((value / 10.0).round() * 10.0).clamp(MIN_SCALE, MAX_SCALE)
    } else {
        DEFAULT_SCALE
    }
}

/// A stored `#rrggbb`, opaque; anything else (the empty default included) is "follow the
/// appearance".
fn parse_color(hex: &str) -> Option<Color> {
    Color::parse(hex).map(|mut c| {
        c.a = 1.0;
        c
    })
}

/// Tracked read of the reader prefs as one style; a document or a stylesheet watch rebuilds
/// when any of the four changes.
pub fn style() -> ReaderStyle {
    let family = READER_FONT.get();
    ReaderStyle {
        scale: scale(READER_SCALE.get()),
        // A store entry nobody could have typed is ignored rather than injected into CSS.
        family: if family.len() <= 512 {
            family
        } else {
            String::new()
        },
        background: parse_color(&READER_BACKGROUND.get()),
        text: parse_color(&READER_TEXT.get()),
    }
}

/// Article ▸ Increase/Decrease Text Size: one step of the settings slider.
pub fn adjust_size(delta: f64) {
    READER_SCALE.set(scale(READER_SCALE.get_untracked() + delta));
}

/// Reset Styles: the reader's prefs, the article list's, and the shared appearance back to
/// System. Each reset removes its key from the store.
pub fn reset() {
    batch(|| {
        READER_SCALE.reset();
        READER_FONT.reset();
        READER_BACKGROUND.reset();
        READER_TEXT.reset();
        LIST_SCALE.reset();
        PREVIEW_LINES.reset();
        APPEARANCE.reset();
    });
}

/// The two color wells, bound to their hex prefs: reading falls back to the appearance's
/// palette, writing stores the opaque color.
#[derive(Clone, Copy)]
pub enum ReaderColor {
    Background,
    Text,
}

impl ReaderColor {
    fn pref(self) -> &'static Pref<String> {
        match self {
            Self::Background => &READER_BACKGROUND,
            Self::Text => &READER_TEXT,
        }
    }
}

impl Binding<Color> for ReaderColor {
    fn read(&self) -> Color {
        let palette = crate::theme::palette();
        parse_color(&self.pref().get()).unwrap_or(match self {
            Self::Background => palette.bg,
            Self::Text => palette.text,
        })
    }
    fn peek(&self) -> Color {
        day::reactive::untrack(|| self.read())
    }
    fn write(&self, mut color: Color) {
        color.a = 1.0;
        self.pref().set(css_color(color));
    }
}

/// The font menu's entries: the system default, the recommended reading faces that are
/// installed, then the whole catalog. Returns the choices and the index where the catalog
/// starts. Repeated recommended entries are intentional: the lower section is complete.
pub fn font_choices(mut available: Vec<String>) -> (Vec<String>, usize) {
    available.sort_by_key(|s| s.to_lowercase());
    available.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    let mut choices = vec![String::new()]; // platform default
    for recommended in [
        "Georgia",
        "Palatino",
        "Charter",
        "Iowan Old Style",
        "Helvetica Neue",
    ] {
        if let Some(family) = available
            .iter()
            .find(|f| f.eq_ignore_ascii_case(recommended))
        {
            choices.push(family.clone());
        }
    }
    let boundary = choices.len();
    choices.extend(available);
    (choices, boundary)
}

pub fn css_color(c: Color) -> String {
    format!(
        "#{:02x}{:02x}{:02x}",
        (c.r * 255.0).round() as u8,
        (c.g * 255.0).round() as u8,
        (c.b * 255.0).round() as u8
    )
}

fn css_family(family: &str) -> String {
    // CSS string escaping also prevents a stored/font-catalog name from ending a style tag.
    let mut escaped = String::new();
    for c in family.chars() {
        if c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '_' {
            escaped.push(c);
        } else {
            escaped.push_str(&format!("\\{:x} ", c as u32));
        }
    }
    format!("\"{escaped}\", -apple-system, system-ui, sans-serif")
}

pub fn stylesheet(style: &ReaderStyle, dark: bool) -> String {
    let p = crate::theme::palette_for(dark);
    let background = style.background.unwrap_or(p.bg);
    let text = style.text.unwrap_or(p.text);
    let family = if style.family.is_empty() {
        "-apple-system, BlinkMacSystemFont, system-ui, sans-serif".into()
    } else {
        css_family(&style.family)
    };
    // Blend supporting colors into the chosen background so custom paper/ink stays coherent.
    let blend = |amount: f64| {
        Color::rgb(
            text.r * amount + background.r * (1.0 - amount),
            text.g * amount + background.g * (1.0 - amount),
            text.b * amount + background.b * (1.0 - amount),
        )
    };
    format!(
        ":root {{ color-scheme: {scheme}; }}\na, .src, #reader-load-full {{ color: {accent}; }}\nhtml, body {{ background: {bg}; color: {fg}; }}\nbody {{ font-family: {family}; font-size: calc(1rem * {factor}); }}\n.by, .when, blockquote {{ color: {muted}; }}\n.rule, hr, blockquote {{ border-color: {rule}; }}\n#reader-load-full {{ border-color: {rule}; background: linear-gradient(color-mix(in srgb, {alt} 80%, {bg}), {alt}); }} .reader-load-label {{ text-shadow: 0 1px 0 color-mix(in srgb, {bg} 75%, transparent); }}\npre, #reader-link-tooltip {{ background: {alt}; }} #reader-link-tooltip {{ color: {fg}; border-color: {rule}; }}",
        scheme = if dark { "dark" } else { "light" },
        accent = css_color(p.accent),
        bg = css_color(background),
        fg = css_color(text),
        factor = scale(style.scale) / 100.0,
        muted = css_color(if style.text.is_some() || style.background.is_some() {
            blend(0.65)
        } else {
            p.text_muted
        }),
        rule = css_color(if style.text.is_some() || style.background.is_some() {
            blend(0.18)
        } else {
            p.rule
        }),
        alt = css_color(if style.text.is_some() || style.background.is_some() {
            blend(0.06)
        } else {
            p.bg_alt
        })
    )
}

pub fn apply_to(js: day_piece_webview::JsHandle) {
    let css = stylesheet(&day::reactive::untrack(style), day::dark_mode());
    let encoded = serde_json::to_string(&css).expect("CSS string serializes");
    day::task(async move {
        // Replace a dedicated style node, leaving document state, selection, and scroll intact.
        let _ = js.eval(format!("(() => {{ const style = document.getElementById('reader-display'); if (style) style.textContent = {encoded}; }})()" )).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scale_is_bounded_and_invalid_persistence_is_safe() {
        assert_eq!(scale(f64::NAN), DEFAULT_SCALE);
        assert_eq!(scale(f64::INFINITY), DEFAULT_SCALE);
        assert_eq!(scale(0.0), MIN_SCALE);
        assert_eq!(scale(999.0), MAX_SCALE);
        assert_eq!(scale(117.0), 120.0);
    }
    #[test]
    fn recommendations_precede_the_complete_sorted_catalog() {
        let (fonts, separator) = font_choices(vec![
            "Zulu Fixture".into(),
            "Georgia".into(),
            "Alpha Fixture".into(),
            "Georgia".into(),
        ]);
        assert_eq!(&fonts[..separator], &["", "Georgia"]);
        assert_eq!(
            &fonts[separator..],
            &["Alpha Fixture", "Georgia", "Zulu Fixture"]
        );
    }
    #[test]
    fn styles_escape_font_names_and_restore_automatic_colors() {
        let s = ReaderStyle {
            family: "Fixture\";</style><script>".into(),
            scale: 150.0,
            background: Some(Color::hex(0xffeedd)),
            text: Some(Color::hex(0x112233)),
        };
        let css = stylesheet(&s, false);
        assert!(!css.contains("</style>"));
        assert!(css.contains("#ffeedd"));
        assert!(css.contains("#112233"));
        assert!(css.contains("1rem * 1.5"));
        assert!(
            stylesheet(&ReaderStyle::default(), true)
                .contains(&css_color(crate::theme::palette_for(true).bg))
        );
    }
    #[test]
    fn stored_colors_are_opaque_hex_or_nothing() {
        assert_eq!(parse_color(""), None);
        assert_eq!(parse_color("not a color"), None);
        let c = parse_color("#fff2d5").expect("hex parses");
        assert_eq!(c.a, 1.0);
        assert_eq!(css_color(c), "#fff2d5");
    }
}

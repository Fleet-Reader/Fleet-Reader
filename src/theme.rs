// SPDX-License-Identifier: Apache-2.0
//! Colors for light and dark, so the reader looks native on every platform.

use day::prelude::*;

pub struct Palette {
    pub bg: Color,
    pub bg_alt: Color,
    pub text: Color,
    pub text_muted: Color,
    pub accent: Color,
    pub rule: Color,
    pub unread_dot: Color,
    /// The star affordances: the row's glyph and the leading swipe action's fill. Warm, so
    /// starring reads apart from the blue read/unread actions.
    pub star: Color,
}

const LIGHT: Palette = Palette {
    bg: Color::hex(0xFFFFFF),
    bg_alt: Color::hex(0xF5F5F7),
    text: Color::hex(0x1C1C1E),
    text_muted: Color::hex(0x74747A),
    accent: Color::hex(0x2F6FDE),
    rule: Color::hex(0xE2E2E6),
    unread_dot: Color::hex(0x2F6FDE),
    star: Color::hex(0xE8940A),
};

const DARK: Palette = Palette {
    bg: Color::hex(0x1B1D1F),
    bg_alt: Color::hex(0x232629),
    text: Color::hex(0xF2F2F4),
    text_muted: Color::hex(0x9A9AA1),
    accent: Color::hex(0x4C8DFF),
    rule: Color::hex(0x33363A),
    unread_dot: Color::hex(0x4C8DFF),
    star: Color::hex(0xF0A62E),
};

/// The palette's colors as inks, the light palette's color in the light appearance and the dark
/// palette's in the dark one: a view drawn in one follows the system theme with no rebuild.
pub const BG: Ink = Ink::adaptive(LIGHT.bg, DARK.bg);
pub const BG_ALT: Ink = Ink::adaptive(LIGHT.bg_alt, DARK.bg_alt);
pub const TEXT: Ink = Ink::adaptive(LIGHT.text, DARK.text);
pub const TEXT_MUTED: Ink = Ink::adaptive(LIGHT.text_muted, DARK.text_muted);
pub const ACCENT: Ink = Ink::adaptive(LIGHT.accent, DARK.accent);
pub const RULE: Ink = Ink::adaptive(LIGHT.rule, DARK.rule);
pub const UNREAD_DOT: Ink = Ink::adaptive(LIGHT.unread_dot, DARK.unread_dot);
pub const STAR: Ink = Ink::adaptive(LIGHT.star, DARK.star);

/// The palette of one appearance, for content that carries its colors as values (the reader's
/// HTML and its stylesheet).
pub(crate) fn palette_for(dark: bool) -> &'static Palette {
    if dark { &DARK } else { &LIGHT }
}

// SPDX-License-Identifier: Apache-2.0
//! Colors, resolved per light/dark so the reader looks native on every platform.

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

/// Tracked read of the platform appearance, so color closures recolor live when the system
/// theme flips.
pub fn palette() -> &'static Palette {
    palette_for(day::dark_mode())
}

pub(crate) fn palette_for(dark: bool) -> &'static Palette {
    if dark { &DARK } else { &LIGHT }
}

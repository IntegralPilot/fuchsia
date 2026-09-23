// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use vt100::Screen;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SizeInfo {
    pub width: f32,
    pub height: f32,
    pub cell_width: f32,
    pub cell_height: f32,
    pub padding_x: f32,
    pub padding_y: f32,
    pub dpr: f32,
}

pub enum Scroll {
    Lines(isize),
    PageUp,
    PageDown,
    Bottom,
    Top,
}

impl Scroll {
    pub fn scroll_screen(&self, screen: &mut Screen) {
        let scrollback = screen.scrollback_len();
        let (visible_lines, _) = screen.size();
        let visible_lines = visible_lines.into();
        let history = scrollback.saturating_add(visible_lines);

        let new_scroll = match self {
            Scroll::Lines(lines) => scrollback.saturating_add_signed(*lines),
            Scroll::PageUp => scrollback.saturating_add(visible_lines),
            Scroll::PageDown => scrollback.saturating_sub(visible_lines),
            Scroll::Top => history,
            Scroll::Bottom => 0,
        };

        let clamped = new_scroll.max(0).min(history);
        screen.set_scrollback(clamped);
    }
}

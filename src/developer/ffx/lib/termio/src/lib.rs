// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::sync::LazyLock;

#[derive(Copy, Clone, Debug)]
pub struct Colors {
    pub red: &'static str,
    pub green: &'static str,
    pub yellow: &'static str,
    pub bold: &'static str,
    pub reset: &'static str,
}

static COLORS: LazyLock<Colors> = LazyLock::new(|| Colors::get());

impl Colors {
    /// Returns the cached Colors based on the environment at startup.
    pub fn current() -> Self {
        *COLORS
    }

    /// Internal function to construct Colors, public for testing.
    #[cfg(test)]
    pub(crate) fn get_for_term(term: Option<String>, is_terminal: bool) -> Self {
        Self::new_with_enabled(is_color_enabled_inner(term, is_terminal))
    }

    fn get() -> Self {
        Self::new_with_enabled(is_color_enabled())
    }

    fn new_with_enabled(enabled: bool) -> Self {
        if enabled {
            Colors {
                red: "\x1b[31m",
                green: "\x1b[32m",
                yellow: "\x1b[33m",
                bold: "\x1b[1m",
                reset: "\x1b[0m",
            }
        } else {
            Colors { red: "", green: "", yellow: "", bold: "", reset: "" }
        }
    }

    pub fn disabled() -> Self {
        Self::new_with_enabled(false)
    }
}

fn is_color_enabled_inner(term: Option<String>, is_terminal: bool) -> bool {
    if !is_terminal {
        return false;
    }
    match term {
        Some(val) => !val.is_empty() && val != "dumb",
        None => false,
    }
}

pub fn is_color_enabled() -> bool {
    use std::io::IsTerminal;
    is_color_enabled_inner(std::env::var("TERM").ok(), std::io::stdout().is_terminal())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_colors() {
        let colors = Colors::get_for_term(Some("xterm-256color".to_string()), true);
        assert_eq!(colors.red, "\x1b[31m");
        assert_eq!(colors.green, "\x1b[32m");
        assert_eq!(colors.yellow, "\x1b[33m");
        assert_eq!(colors.bold, "\x1b[1m");
        assert_eq!(colors.reset, "\x1b[0m");

        let colors = Colors::get_for_term(Some("xterm-256color".to_string()), false);
        assert_eq!(colors.red, "");
        assert_eq!(colors.green, "");
        assert_eq!(colors.yellow, "");
        assert_eq!(colors.bold, "");
        assert_eq!(colors.reset, "");

        let colors = Colors::get_for_term(Some("dumb".to_string()), true);
        assert_eq!(colors.red, "");
        assert_eq!(colors.green, "");
        assert_eq!(colors.yellow, "");
        assert_eq!(colors.bold, "");
        assert_eq!(colors.reset, "");

        let colors = Colors::get_for_term(None, true);
        assert_eq!(colors.red, "");
        assert_eq!(colors.green, "");
        assert_eq!(colors.yellow, "");
        assert_eq!(colors.bold, "");
        assert_eq!(colors.reset, "");
    }
}

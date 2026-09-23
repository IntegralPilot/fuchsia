// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use vte::{Params, Parser};

/// Get the length of a string when printed to the terminal (ignoring control
/// sequences).
///
/// This just strips sequences out. It doesn't account for how cursor movements
/// might change string length.
pub fn printed_length(s: &str) -> usize {
    struct VteCounter(usize);

    impl vte::Perform for VteCounter {
        fn print(&mut self, c: char) {
            self.0 += unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        }
        fn execute(&mut self, _: u8) {}
        fn hook(&mut self, _: &Params, _: &[u8], _: bool, _: char) {}
        fn put(&mut self, _: u8) {}
        fn unhook(&mut self) {}
        fn osc_dispatch(&mut self, _: &[&[u8]], _: bool) {}
        fn csi_dispatch(&mut self, _: &Params, _: &[u8], _: bool, _: char) {}
        fn esc_dispatch(&mut self, _: &[u8], _: bool, _: u8) {}
    }

    let mut counter = VteCounter(0);
    let mut parser = Parser::new();
    parser.advance(&mut counter, &s.as_bytes());

    counter.0
}

/// Sanitizes a string for terminal display by escaping ANSI escape sequences and control characters.
///
/// Newlines (`\n`) are preserved so that multiline formatting continues to work.
pub fn sanitize_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '\n' {
            out.push('\n');
        } else if c.is_control() {
            for esc in c.escape_default() {
                out.push(esc);
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_sanitize_str_plain() {
        assert_eq!(sanitize_str("hello world"), "hello world");
        assert_eq!(sanitize_str("hello \"world\" \\ test"), "hello \"world\" \\ test");
        assert_eq!(sanitize_str("unicode: ➤ 日本語"), "unicode: ➤ 日本語");
    }

    #[test]
    fn test_sanitize_str_preserves_newline() {
        assert_eq!(sanitize_str("line1\nline2\nline3"), "line1\nline2\nline3");
    }

    #[test]
    fn test_sanitize_str_escapes_ansi_and_control() {
        assert_eq!(
            sanitize_str("\x1b[31mred text\x1b[0m\r\n"),
            "\\u{1b}[31mred text\\u{1b}[0m\\r\n"
        );
        assert_eq!(sanitize_str("\x1b[6n"), "\\u{1b}[6n");
        assert_eq!(
            sanitize_str("\x00\x07\x08\x0b\x0c\x1b\x7f"),
            "\\u{0}\\u{7}\\u{8}\\u{b}\\u{c}\\u{1b}\\u{7f}"
        );
        assert_eq!(sanitize_str("\u{009b}2J"), "\\u{9b}2J");
    }
}

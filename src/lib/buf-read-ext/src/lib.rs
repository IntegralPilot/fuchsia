// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::io::{self, BufRead};

/// Extension trait for `std::io::BufRead`.
pub trait BufReadExt: BufRead {
    /// Returns a lending iterator over the lines of this reader.
    ///
    /// Unlike `std::io::BufRead::lines`, this iterator reuses a single `String` buffer
    /// and yields lines as `&str` references bound to the lifetime of the iterator's `next` call.
    /// This avoids allocating a new string for every line.
    fn lending_lines(&mut self) -> LendingLines<'_, Self>
    where
        Self: Sized,
    {
        LendingLines { reader: self, buffer: String::new() }
    }
}

impl<R: BufRead> BufReadExt for R {}

/// A lending iterator over the lines of a `BufRead` reader.
///
/// See [`BufReadExt::lending_lines`] for more details.
pub struct LendingLines<'a, R: BufRead> {
    reader: &'a mut R,
    buffer: String,
}

impl<'a, R: BufRead> LendingLines<'a, R> {
    /// Returns the next line from the reader, or `None` if the end of the file has been reached.
    /// The returned string slice is valid until the next call to `next`.
    pub fn next(&mut self) -> Option<io::Result<&str>> {
        self.buffer.clear();
        match self.reader.read_line(&mut self.buffer) {
            Ok(0) => None,
            Ok(_) => {
                let mut len = self.buffer.len();
                if len > 0 && self.buffer.as_bytes()[len - 1] == b'\n' {
                    len -= 1;
                    if len > 0 && self.buffer.as_bytes()[len - 1] == b'\r' {
                        len -= 1;
                    }
                }
                Some(Ok(&self.buffer[..len]))
            }
            Err(e) => Some(Err(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_lending_lines_empty() {
        let data = "";
        let mut cursor = Cursor::new(data);
        let mut lines = cursor.lending_lines();

        assert!(lines.next().is_none());
    }

    #[test]
    fn test_lending_lines() {
        let data = "line1\nline2\r\nline3";
        let mut cursor = Cursor::new(data);
        let mut lines = cursor.lending_lines();

        assert_eq!(lines.next().unwrap().unwrap(), "line1");
        assert_eq!(lines.next().unwrap().unwrap(), "line2");
        assert_eq!(lines.next().unwrap().unwrap(), "line3");
        assert!(lines.next().is_none());
    }
}

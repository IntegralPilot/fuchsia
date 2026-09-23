// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bstr::BString;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistoryDir {
    Prev,
    Next,
}

/// Manages input history for the line editor.
#[derive(Debug, Clone)]
pub struct History {
    entries: Vec<BString>,
    max_len: usize,
}

impl History {
    /// Creates a new history buffer with the specified maximum length.
    pub fn new(max_len: usize) -> Self {
        Self { entries: Vec::new(), max_len }
    }

    /// Adds a new line to history, returning `true` if added (deduplicating adjacent
    /// identical entries).
    pub fn add(&mut self, line: impl Into<BString>) -> bool {
        if self.max_len == 0 {
            return false;
        }
        let line_bstring = line.into();
        if let Some(last) = self.entries.last() {
            if last == &line_bstring {
                return false;
            }
        }
        if self.entries.len() >= self.max_len {
            self.entries.remove(0);
        }
        self.entries.push(line_bstring);
        true
    }

    /// Returns a slice of all stored history entries.
    pub fn entries(&self) -> &[BString] {
        &self.entries
    }

    /// Returns the number of stored history entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` if the history buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Clears all entries from the history buffer.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Sets the maximum number of history entries allowed, evicting oldest entries if needed.
    pub fn set_max_len(&mut self, max_len: usize) {
        self.max_len = max_len;
        while self.entries.len() > self.max_len {
            self.entries.remove(0);
        }
    }

    /// Returns the maximum number of history entries allowed.
    pub fn max_len(&self) -> usize {
        self.max_len
    }

    /// Returns an entry at the given index, if present.
    pub fn get(&self, index: usize) -> Option<&BString> {
        self.entries.get(index)
    }
}

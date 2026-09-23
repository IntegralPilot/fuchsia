// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![warn(missing_docs)]

//! The `TaskCommand` type and associated functions.

use flyweights::FlyByteStr;
use fuchsia_rcu::RcuDroppable;
use std::ops::Range;

/// The command for a task.
///
/// Linux task commands are limited to 15 bytes, but Fuchsia allows longer names in places. It's
/// useful to store longer names diagnostics and debugging information.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct TaskCommand {
    name: FlyByteStr,
    linux_name_range: Option<Range<usize>>,
}

// SAFETY: TaskCommand contains FlyByteStr (interned string without drop side effects) and Option<Range<usize>>.
unsafe impl RcuDroppable for TaskCommand {}

impl TaskCommand {
    /// Create a new `TaskCommand` from a byte slice. The byte slice is truncated at the first null
    /// byte if any.
    pub fn new(name: &[u8]) -> Self {
        let name = if let Some(idx) = memchr::memchr(b'\0', name) { &name[..idx] } else { name };
        Self { name: FlyByteStr::new(name), linux_name_range: None }
    }

    /// Create a new `TaskCommand` from a path. The basename of the path is used as the name.
    pub fn from_path_bytes(path: &[u8]) -> Self {
        let basename =
            if let Some(idx) = memchr::memrchr(b'/', path) { &path[idx + 1..] } else { path };
        Self::new(basename)
    }

    /// Returns the name truncated to 15 bytes.
    pub fn comm_name(&self) -> &[u8] {
        let bytes = self.linux_name_bytes();
        &bytes[..std::cmp::min(bytes.len(), 15)]
    }

    /// Returns the name as a 16-byte array, null-terminated if shorter than 16 bytes,
    /// as expected by `prctl(PR_GET_NAME)`.
    pub fn prctl_name(&self) -> [u8; 16] {
        let mut name = [0u8; 16];
        let comm = self.comm_name();
        name[..comm.len()].copy_from_slice(comm);
        name
    }

    /// Returns the entire name as a byte slice.
    pub fn as_bytes(&self) -> &[u8] {
        self.name.as_bytes()
    }

    /// Returns the Linux name as a byte slice, without truncation.
    fn linux_name_bytes(&self) -> &[u8] {
        if let Some(range) = &self.linux_name_range {
            &self.name.as_bytes()[range.clone()]
        } else {
            self.name.as_bytes()
        }
    }

    /// Tries to embed `other` as the Linux name within this command.
    /// Returns a new `TaskCommand` if `other` is a substring of this command.
    pub fn try_embed(&self, other: &TaskCommand) -> Option<Self> {
        use bstr::ByteSlice;
        self.name.as_bytes().find(other.linux_name_bytes()).map(|offset| Self {
            name: self.name.clone(),
            linux_name_range: Some(offset..offset + other.linux_name_bytes().len()),
        })
    }
}

impl Default for TaskCommand {
    fn default() -> Self {
        Self::new(b"")
    }
}

impl std::fmt::Debug for TaskCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.name.fmt(f)
    }
}

impl std::fmt::Display for TaskCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.name.fmt(f)
    }
}

impl PartialOrd for TaskCommand {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TaskCommand {
    /// This comparison ignores the linux rendering of the name and provides a total ordering
    /// based on the full name.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.name.cmp(&other.name)
    }
}

impl Into<FlyByteStr> for TaskCommand {
    fn into(self) -> FlyByteStr {
        self.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new() {
        assert_eq!(TaskCommand::new(b"foo").as_bytes(), b"foo");
        assert_eq!(TaskCommand::new(b"foo\0bar").as_bytes(), b"foo");
    }

    #[test]
    fn test_from_path_bytes() {
        assert_eq!(TaskCommand::from_path_bytes(b"/foo/bar").as_bytes(), b"bar");
        assert_eq!(TaskCommand::from_path_bytes(b"bar").as_bytes(), b"bar");
        assert_eq!(TaskCommand::from_path_bytes(b"/bar").as_bytes(), b"bar");
    }

    #[test]
    fn test_comm_name() {
        assert_eq!(TaskCommand::new(b"short").comm_name(), b"short");
        assert_eq!(TaskCommand::new(b"0123456789abcdef").comm_name(), b"0123456789abcde");
        assert_eq!(TaskCommand::new(b"0123456789abcdefg").comm_name(), b"0123456789abcde");
    }

    #[test]
    fn test_prctl_name() {
        assert_eq!(TaskCommand::new(b"short").prctl_name(), *b"short\0\0\0\0\0\0\0\0\0\0\0");
        assert_eq!(TaskCommand::new(b"0123456789abcdef").prctl_name(), *b"0123456789abcde\0");
        assert_eq!(TaskCommand::new(b"0123456789abcdefg").prctl_name(), *b"0123456789abcde\0");
    }

    #[test]
    fn test_prctl_name_16_bytes() {
        let name = b"0123456789abcdef"; // 16 bytes
        assert_eq!(TaskCommand::new(name).prctl_name(), *b"0123456789abcde\0");
        assert_eq!(TaskCommand::new(name).comm_name(), b"0123456789abcde"); // 15 bytes
    }

    #[test]
    fn test_debug() {
        assert_eq!(format!("{:?}", TaskCommand::new(b"foo")), "\"foo\"");
    }

    #[test]
    fn test_display() {
        assert_eq!(TaskCommand::new(b"foo").to_string(), "foo");
    }

    #[test]
    fn test_sniffing() {
        let argv0 = TaskCommand::new(b"/path/to/binary");
        let short = TaskCommand::new(b"binary");
        let embedded = argv0.try_embed(&short).expect("should embed");
        assert_eq!(embedded.as_bytes(), b"/path/to/binary");
        assert_eq!(embedded.comm_name(), b"binary");

        let other = TaskCommand::new(b"other");
        assert!(argv0.try_embed(&other).is_none());
    }

    #[test]
    fn test_comm_name_sniffed() {
        let long_argv0 = TaskCommand::new(b"/path/to/short_name_with_suffix");
        let short_name = TaskCommand::new(b"short_name");
        let embedded = long_argv0.try_embed(&short_name).expect("should embed");
        // comm_name should be "short_name" (len 10), not truncated version of full path
        assert_eq!(embedded.comm_name(), b"short_name");
    }
}

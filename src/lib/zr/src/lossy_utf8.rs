// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// Wraps a binary string that may contain non-UTF-8 bytes for formatting.
/// Invalid bytes are formatted as `\u{FFFFD}`.
pub fn from_utf8_lossy(bytes: &[u8]) -> LossyUtf8<'_> {
    LossyUtf8(bytes)
}

pub struct LossyUtf8<'a>(&'a [u8]);

impl core::fmt::Display for LossyUtf8<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut bytes = self.0;
        while !bytes.is_empty() {
            match core::str::from_utf8(bytes) {
                Ok(s) => {
                    f.write_str(s)?;
                    break;
                }
                Err(err) => {
                    let (valid, rest) = bytes.split_at(err.valid_up_to());
                    if !valid.is_empty() {
                        // SAFETY: `valid` was verified by `from_utf8`.
                        let s = unsafe { core::str::from_utf8_unchecked(valid) };
                        f.write_str(s)?;
                    }
                    f.write_str("\u{FFFD}")?;
                    match err.error_len() {
                        Some(len) => bytes = &rest[len..],
                        None => break,
                    }
                }
            }
        }
        Ok(())
    }
}

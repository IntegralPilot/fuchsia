// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::structures::Header;
use zx_status::Status;

/// Utility for working with the table of null-terminated strings after each SMBIOS structure.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct StringTable<'a> {
    data: &'a [u8],
}

impl<'a> StringTable<'a> {
    /// Creates an empty `StringTable`.
    pub const fn empty() -> Self {
        Self { data: &[] }
    }

    /// Initializes a `StringTable` from a structure header and the buffer containing the
    /// structure data (both header and trailing strings) bounded by `max_struct_len`.
    pub fn init(hdr: &Header, max_struct_len: usize, raw_data: &'a [u8]) -> Result<Self, Status> {
        let hdr_len = hdr.length as usize;
        if hdr_len > max_struct_len || hdr_len > raw_data.len() {
            return Err(Status::IO_DATA_INTEGRITY);
        }

        let available_len = core::cmp::min(max_struct_len, raw_data.len()) - hdr_len;
        // Make sure the table is big enough to include the two trailing NULs
        if available_len < 2 {
            return Err(Status::IO_DATA_INTEGRITY);
        }

        let string_bytes = &raw_data[hdr_len..hdr_len + available_len];

        // Check if the string table is empty
        if string_bytes[0] == 0 && string_bytes[1] == 0 {
            return Ok(Self { data: &string_bytes[..2] });
        }

        let start_idx = if string_bytes[0] == 0 { 1 } else { 0 };
        let mut i = start_idx;

        while i < available_len {
            let remaining = &string_bytes[i..];
            let len = remaining.iter().position(|&b| b == 0).unwrap_or(remaining.len());

            if len == 0 {
                let table_len = i + 1; // Include the trailing null
                return Ok(Self { data: &string_bytes[..table_len] });
            }

            // strnlen returns the length not including the NUL. Note that if
            // no NUL was found, it returns remaining.len(), which will exceed
            // available_len when incremented by len + 1.
            i += len + 1;
        }

        Err(Status::IO_DATA_INTEGRITY)
    }

    /// Returns the length of the `StringTable` in bytes, including terminating null bytes.
    pub fn length(&self) -> usize {
        self.data.len()
    }

    /// Returns true if the string table contains no string entries.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty() || (self.data.len() == 2 && self.data[0] == 0 && self.data[1] == 0)
    }

    /// Retrieves the string at 1-based index `idx`.
    ///
    /// If `idx == 0`, returns `Ok("<null>")` indicating an unassigned string field.
    /// Returns `Err(Status::NOT_FOUND)` if `idx` exceeds the available strings in the table.
    /// Returns `Err(Status::IO_DATA_INTEGRITY)` on corrupt data or invalid UTF-8.
    pub fn get_string(&self, mut idx: usize) -> Result<&str, Status> {
        if idx == 0 {
            return Ok("<null>");
        }

        let mut i = 0;
        while i < self.data.len() {
            let remaining = &self.data[i..];
            let len = remaining.iter().position(|&b| b == 0).unwrap_or(remaining.len());

            if len == 0 {
                if i != 0 {
                    return Err(Status::NOT_FOUND);
                }

                if self.data.len() - i < 2 {
                    return Err(Status::IO_DATA_INTEGRITY);
                }
                if self.data[i + 1] == 0 {
                    return Err(Status::NOT_FOUND);
                }
            }

            if idx == 1 {
                let str_bytes = &self.data[i..i + len];
                return core::str::from_utf8(str_bytes).map_err(|_| Status::IO_DATA_INTEGRITY);
            }

            idx -= 1;
            i += len + 1;
        }

        Err(Status::NOT_FOUND)
    }

    /// Convenience version of `get_string` that returns `"<missing string>"` on error,
    /// matching C++ `GetString(size_t idx)`.
    pub fn get_string_or_default(&self, idx: usize) -> &str {
        self.get_string(idx).unwrap_or("<missing string>")
    }

    /// Dumps the string table contents to the provided writer.
    pub fn dump<W: core::fmt::Write>(&self, writer: &mut W) -> core::fmt::Result {
        let mut i = 1;
        while let Ok(str_val) = self.get_string(i) {
            writeln!(writer, "  str {}: {}", i, str_val)?;
            i += 1;
        }
        Ok(())
    }
}

// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use delivery_blob::compression::{ChunkedArchiveError, DataBuffer};
use std::ops::Range;
use storage_ptr_slice::MutPtrByteSlice;

/// Trait for servicing a page request.
pub trait PageRequest: DataBuffer {
    /// Prepares the target buffer for reading `read_range`.
    ///
    /// Must be called at most once prior to accessing the buffer slice or committing data.
    fn prepare(&mut self, read_range: Range<u64>) -> Result<(), ChunkedArchiveError>;
}

/// A no-op [`PageRequest`] for intermediate mapper sessions that do not run a kernel pager thread.
pub struct NullPageRequest;

impl DataBuffer for NullPageRequest {
    fn range(&self) -> Range<u64> {
        0..0
    }

    fn mut_ptr_slice(&mut self) -> MutPtrByteSlice<'_> {
        MutPtrByteSlice::from(&mut [][..])
    }

    fn commit(&mut self, _size: usize) -> Result<(), ChunkedArchiveError> {
        Ok(())
    }
}

impl PageRequest for NullPageRequest {
    fn prepare(&mut self, _read_range: Range<u64>) -> Result<(), ChunkedArchiveError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_null_page_request() {
        let mut req = NullPageRequest;
        assert_eq!(req.range(), 0..0);
        assert!(req.prepare(0..4096).is_ok());
        assert_eq!(req.mut_ptr_slice().len(), 0);
        assert!(req.commit(0).is_ok());
    }
}

// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::att::attribute::Attribute;
use crate::att::bearer::MAX_ATTRIBUTE_SIZE;
use crate::att::pdu::ErrorCode;
use core::{cmp, fmt};
use sapphire_collections::vec::StdVec;
use sapphire_common::{PeerId, Uuid};
use sapphire_sync::mutex::Mutex;
use sapphire_sync::mutex::raw::SingleThreadMutex;

// TODO(https://fxbug.dev/524267879): Replace the temporary Vec with BufferModel defined in sapphire-buffer
//
// The vector storing the attribute value.
//
// sapphire_collections::vec::StdVec is used here because this mock is strictly for testing.
// A standard growable heap-allocated vector is sufficient.
pub type AttributeValueVec = StdVec<u8>;

/// A mock implementation of the `Attribute` trait for unit and integration testing.
///
/// Unlike production attributes, `MockAttribute` maintains its state in an owned
/// heap-allocated vector. This allows mock databases (such as `MockDb`) to
/// be populated dynamically in tests without requiring static lifetimes (`&'static [u8]`).
pub struct MockAttribute {
    /// The Type UUID of the attribute.
    uuid: Uuid,
    /// The owned byte value of the attribute, capped at 512 bytes.
    value: Mutex<SingleThreadMutex, AttributeValueVec>,
    /// The ending handle if this attribute is grouped.
    group_end_handle: Option<u16>,
    /// Optional error code returned by write transactions.
    write_error: Mutex<SingleThreadMutex, Option<ErrorCode>>,
}

impl fmt::Debug for MockAttribute {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MockAttribute")
            .field("uuid", &self.uuid)
            .field("value", &&self.value.lock()[..])
            .field("group_end_handle", &self.group_end_handle)
            .finish()
    }
}

impl MockAttribute {
    /// A constructor helper to create a standard, non-grouped `MockAttribute` with the
    /// given UUID and initial value.
    ///
    /// Copies up to `MAX_ATTRIBUTE_SIZE` (512) bytes from `initial_value` into the owned
    /// vector. Any bytes exceeding the limit are truncated.
    pub fn new(uuid: Uuid, initial_value: &[u8]) -> Self {
        let mut value = AttributeValueVec::new();
        let len = cmp::min(initial_value.len(), MAX_ATTRIBUTE_SIZE);
        for &byte in &initial_value[..len] {
            value.try_push(byte).unwrap();
        }
        Self {
            uuid,
            value: Mutex::new(value),
            group_end_handle: None,
            write_error: Mutex::new(None),
        }
    }

    /// A constructor helper to create a grouped `MockAttribute` with the given UUID,
    /// value, and group ending handle.
    pub fn new_grouped(uuid: Uuid, initial_value: &[u8], group_end_handle: u16) -> Self {
        let mut value = AttributeValueVec::new();
        let len = cmp::min(initial_value.len(), MAX_ATTRIBUTE_SIZE);
        for &byte in &initial_value[..len] {
            value.try_push(byte).unwrap();
        }
        Self {
            uuid,
            value: Mutex::new(value),
            group_end_handle: Some(group_end_handle),
            write_error: Mutex::new(None),
        }
    }

    /// Simulates a write failure by setting a specific error code.
    pub fn set_write_error(&self, error: ErrorCode) {
        *self.write_error.lock() = Some(error);
    }
}

impl Attribute for MockAttribute {
    /// Returns the Type UUID of this attribute.
    fn uuid(&self) -> &Uuid {
        &self.uuid
    }

    /// Returns the ending handle if this attribute is grouped.
    fn group_end_handle(&self) -> Option<u16> {
        self.group_end_handle
    }

    /// Reads a chunk of the attribute value starting at `offset` into `buf`.
    ///
    /// Capped by `buf.len()` and the remaining length of the value.
    /// Returns `Err(ErrorCode::INVALID_OFFSET)` if `offset` is out of bounds of the value.
    async fn read_chunk(
        &self,
        _peer_id: PeerId,
        offset: u16,
        buf: &mut [u8],
    ) -> Result<usize, ErrorCode> {
        let offset = offset as usize;
        let value = self.value.lock();
        if offset > value.len() {
            return Err(ErrorCode::INVALID_OFFSET);
        }
        let len = cmp::min(buf.len(), value.len() - offset);
        buf[..len].copy_from_slice(&value[offset..offset + len]);
        Ok(len)
    }

    /// Mock writing to the attribute.
    async fn write_chunk(
        &self,
        _peer_id: PeerId,
        offset: u16,
        data: &[u8],
    ) -> Result<(), ErrorCode> {
        if let Some(err) = *self.write_error.lock() {
            return Err(err);
        }
        let offset = offset as usize;
        let mut value = self.value.lock();
        if offset > value.len() {
            return Err(ErrorCode::INVALID_OFFSET);
        }
        if offset + data.len() > MAX_ATTRIBUTE_SIZE {
            return Err(ErrorCode::INVALID_ATTRIBUTE_VALUE_LENGTH);
        }
        if offset == 0 {
            value.clear();
        }
        value.truncate(offset);
        value.try_extend(data).map_err(|_| ErrorCode::INSUFFICIENT_RESOURCES)?;
        Ok(())
    }
}

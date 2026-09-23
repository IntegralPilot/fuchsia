// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module implements a zero-copy/optimized byte representation for keys in Fxfs.
//!
//! Its goal is to facilitate lexicographically comparable key serialization, permitting optimal
//! byte representations when performing operations across persistent layer structures in LSM trees.

use crate::serialized_types::varint::{self, Buffer};
use anyhow::{Error, anyhow, ensure};
use std::cmp;

/// Maximum length of a single key serialization chunk payload.
pub const MAX_CHUNK_LEN: usize = 255;

/// Evaluates comparison ordering between two serialized keys located at the beginning of `a` and
/// `b`.
///
/// Both byte slices must start with chunked key data. If the serialized key is <= 254 bytes,
/// it is prefixed by a 1-byte length. If 255 bytes or longer (>= 255 bytes), it is serialized
/// as a sequence of 255-byte chunks. Any trailing bytes after each serialized key are ignored.
#[inline(always)]
pub fn compare_keys(a: &[u8], b: &[u8]) -> Result<cmp::Ordering, Error> {
    if !a.is_empty() && !b.is_empty() {
        let len_a = a[0] as usize;
        let len_b = b[0] as usize;
        if a.len() > len_a && b.len() > len_b {
            let order = a[1..1 + len_a].cmp(&b[1..1 + len_b]);
            if order.is_ne() || len_a != MAX_CHUNK_LEN {
                return Ok(order);
            }
            return compare_keys_slow(&a[1 + MAX_CHUNK_LEN..], &b[1 + MAX_CHUNK_LEN..]);
        }
    }
    compare_keys_slow(a, b)
}

/// Fallback path for `compare_keys` when either key spans multiple chunks (chunk length == 255)
/// or buffers are malformed.
#[cold]
#[inline(never)]
fn compare_keys_slow(mut a: &[u8], mut b: &[u8]) -> Result<cmp::Ordering, Error> {
    #[inline]
    fn get_chunk(chunk: &[u8]) -> Result<&[u8], Error> {
        ensure!(!chunk.is_empty(), "Key buffer truncated");
        let len = chunk[0] as usize;
        ensure!(chunk.len() > len, "Key length exceeds buffer");
        Ok(&chunk[1..1 + len])
    }

    loop {
        let chunk_a = get_chunk(a)?;
        let chunk_b = get_chunk(b)?;

        let order = chunk_a.cmp(chunk_b);
        let has_more_a = chunk_a.len() == MAX_CHUNK_LEN;

        if order.is_ne() || !has_more_a {
            return Ok(order);
        }

        a = &a[MAX_CHUNK_LEN + 1..];
        b = &b[MAX_CHUNK_LEN + 1..];
    }
}

/// Serializes keys sequentially into binary format suitable for lexicographical comparisons.
///
/// The key layout contains:
/// - Sequences of 255-byte chunks, each prefixed with a 1-byte length.
/// - If a serialized key is <= 254 bytes, it is represented as a single chunk with a 1-byte length.
/// - If exactly a multiple of 255 bytes, it is terminated by a 0-length chunk.
pub struct KeySerializer<'a, B: Buffer> {
    buffer: &'a mut B,
    start_pos: usize,
    /// Set to true once `finalize` has been called.
    done: bool,
    /// Optional delta base subtracted from the first `u64` payload item.
    base: Option<u64>,
    /// Latched error encountered during serialization, returned upon `finalize`.
    error: Option<Error>,
}

impl<'a, B: Buffer> KeySerializer<'a, B> {
    /// Creates a new `KeySerializer` attached to a persistent storage buffer.
    #[inline]
    pub fn new(buffer: &'a mut B, base: Option<u64>) -> Self {
        let start_pos = buffer.as_ref().len();
        buffer.put(&[0]);
        Self { buffer, start_pos, done: false, base, error: None }
    }

    /// Returns true if no payload items have been written yet for this key.
    #[inline]
    fn is_first(&self) -> bool {
        self.buffer.as_ref().len() == self.start_pos + 1
    }

    /// Writes an order-preserving varint-encoded 64-bit payload to the buffer without base delta encoding.
    #[inline]
    pub fn write_varint(&mut self, v: u64) {
        if self.error.is_some() {
            return;
        }
        debug_assert!(!self.done);
        let (bytes, len) = varint::encode_varint_bytes(v);
        self.write_bytes(&bytes[..len]);
    }

    /// Writes an order-preserving varint-encoded 64-bit payload to the buffer.
    ///
    /// If a delta `base` was provided and this is the first item written, it writes the delta
    /// `v - base`. Otherwise, writes `v` as an order-preserving varint.
    #[inline]
    pub fn write_u64(&mut self, v: u64) {
        if self.error.is_some() {
            return;
        }
        debug_assert!(!self.done);
        if let Some(base) = self.base.take() {
            if !self.is_first() {
                self.error = Some(anyhow!("write_u64 with base must be the first item"));
                return;
            }
            if v < base {
                self.error = Some(anyhow!("Delta encoding underflow: v ({}) < base ({})", v, base));
                return;
            }
            self.write_varint(v - base);
        } else {
            self.write_varint(v);
        }
    }

    /// Writes a raw byte slice into the serialization stream. Non-terminal fields are assumed to
    /// fit within the initial chunk.
    #[inline]
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        if self.error.is_some() {
            return;
        }
        debug_assert!(!self.done);
        debug_assert!(
            self.buffer.as_ref().len() - self.start_pos - 1 + bytes.len() <= MAX_CHUNK_LEN,
            "Non-terminal key fields must fit within the initial chunk"
        );
        self.buffer.put(bytes);
    }

    /// Writes trailing variable-length dynamic bytes to the serialization buffer, crossing chunk
    /// boundaries as needed, then immediately finalizes the key.
    #[inline]
    pub fn write_last(&mut self, mut bytes: &[u8]) {
        if self.error.is_some() {
            return;
        }
        debug_assert!(!self.done);
        while !bytes.is_empty() {
            let total_minus_1 = self.buffer.as_ref().len() - self.start_pos - 1;
            let chunk_len = total_minus_1 & 0xff;
            let space = MAX_CHUNK_LEN - chunk_len;
            if space == 0 {
                let chunk_start = self.start_pos + (total_minus_1 & !0xff);
                self.buffer.as_mut()[chunk_start] = MAX_CHUNK_LEN as u8;
                self.buffer.put(&[0]);
                continue;
            }
            let to_write = std::cmp::min(bytes.len(), space);
            self.buffer.put(&bytes[..to_write]);
            bytes = &bytes[to_write..];
        }
        let _ = self.finalize();
    }

    /// Resolves chunk lengths and finalizes the key serialization.
    ///
    /// If an error occurred during serialization, truncates the buffer back to start position
    /// and returns the error.
    #[inline]
    pub fn finalize(&mut self) -> Result<(), Error> {
        if let Some(err) = &self.error {
            if !self.done {
                self.done = true;
                self.buffer.truncate(self.start_pos);
            }
            return Err(anyhow!("{}", err));
        }
        if self.done {
            return Ok(());
        }
        let total_minus_1 = self.buffer.as_ref().len() - self.start_pos - 1;
        let chunk_len = total_minus_1 & 0xff;
        let chunk_start = self.start_pos + (total_minus_1 & !0xff);
        if chunk_len == MAX_CHUNK_LEN {
            self.buffer.as_mut()[chunk_start] = MAX_CHUNK_LEN as u8;
            self.buffer.put(&[0]);
        } else {
            self.buffer.as_mut()[chunk_start] = chunk_len as u8;
        }
        self.done = true;
        Ok(())
    }
}

impl<B: Buffer> Drop for KeySerializer<'_, B> {
    fn drop(&mut self) {
        // Roll back unfinalized key bytes on error or early exit to maintain buffer integrity.
        if !self.done {
            self.buffer.truncate(self.start_pos);
            if !std::thread::panicking() && self.error.is_none() {
                debug_assert!(false, "KeySerializer dropped without being finalized");
            }
        }
    }
}

/// Handles decoding of sequential serialized key payloads.
pub struct KeyDeserializer<'a> {
    /// Current chunk unconsumed payload bytes.
    chunk: &'a [u8],
    /// Remaining buffer starting at next chunk header (or empty if no more chunks).
    remaining_chunks: &'a [u8],
    /// Optional delta base added to the first `u64` payload item.
    base: Option<u64>,
    /// Whether the next item to read is the first item (eligible for base delta decoding).
    is_first: bool,
}

impl<'a> KeyDeserializer<'a> {
    /// Parses a serialized chunked key from the front of `data`.
    ///
    /// Returns the deserializer positioned at the key payload, along with the total
    /// bytes consumed in `data` (headers + payload chunks).
    #[inline]
    pub fn new(data: &'a [u8], base: Option<u64>) -> Result<(Self, usize), Error> {
        ensure!(!data.is_empty(), "Key buffer truncated");
        let first_len = data[0] as usize;
        ensure!(data.len() > first_len, "Key length exceeds buffer");
        let chunk = &data[1..1 + first_len];
        let mut total_len = 1 + first_len;
        let remaining_chunks = if first_len == MAX_CHUNK_LEN {
            let mut rem = &data[MAX_CHUNK_LEN + 1..];
            let mut last_len = first_len;
            while last_len == MAX_CHUNK_LEN {
                ensure!(!rem.is_empty(), "Key buffer truncated");
                let l = rem[0] as usize;
                ensure!(rem.len() > l, "Key length exceeds buffer");
                total_len += 1 + l;
                rem = if l == MAX_CHUNK_LEN { &rem[MAX_CHUNK_LEN + 1..] } else { &[] };
                last_len = l;
            }
            &data[MAX_CHUNK_LEN + 1..total_len]
        } else {
            &[]
        };
        Ok((Self { chunk, remaining_chunks, base, is_first: true }, total_len))
    }

    /// Returns true if all payload bytes have been consumed.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.chunk.is_empty() && self.remaining_chunks.is_empty()
    }

    /// Reads exactly `buf.len()` bytes into `buf`. Non-terminal fields are assumed to fit within
    /// the initial chunk.
    #[inline]
    pub fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), Error> {
        self.is_first = false;
        ensure!(self.chunk.len() >= buf.len(), "Data array boundary overrun");
        buf.copy_from_slice(&self.chunk[..buf.len()]);
        self.chunk = &self.chunk[buf.len()..];
        Ok(())
    }

    /// Reads a 64-bit unsigned integer. If a base was provided and this is the first item,
    /// adds the base to reconstruct the original value.
    #[inline]
    pub fn read_u64(&mut self) -> Result<u64, Error> {
        let is_first = self.is_first;
        let v = self.read_varint()?;
        if let Some(base) = self.base.take() {
            ensure!(is_first, "read_u64 with base must be the first item");
            Ok(v.checked_add(base).ok_or_else(|| {
                anyhow::anyhow!("Delta decoding overflow: v ({}) + base ({})", v, base)
            })?)
        } else {
            Ok(v)
        }
    }

    /// Extracts an order-preserving decoded 64-bit variable length integer from stream.
    #[inline]
    pub fn read_varint(&mut self) -> Result<u64, Error> {
        self.is_first = false;
        let (v, remainder) = varint::decode_varint(self.chunk)?;
        self.chunk = remainder;
        Ok(v)
    }

    /// Reads a fixed-length vector of `len` bytes from the unconsumed key payload.
    #[inline]
    pub fn read_bytes(&mut self, len: usize) -> Result<Vec<u8>, Error> {
        self.is_first = false;
        ensure!(self.chunk.len() >= len, "Data array boundary overrun");
        let vec = self.chunk[..len].to_vec();
        self.chunk = &self.chunk[len..];
        Ok(vec)
    }

    /// Consumes and returns all remaining bytes in the key payload.
    #[inline]
    pub fn read_last(&mut self) -> Vec<u8> {
        self.is_first = false;
        let mut result = Vec::with_capacity(self.chunk.len() + self.remaining_chunks.len());
        result.extend_from_slice(self.chunk);
        self.chunk = &[];
        while !self.remaining_chunks.is_empty() {
            let len = self.remaining_chunks[0] as usize;
            result.extend_from_slice(&self.remaining_chunks[1..1 + len]);
            self.remaining_chunks = if len == MAX_CHUNK_LEN {
                &self.remaining_chunks[MAX_CHUNK_LEN + 1..]
            } else {
                &[]
            };
        }
        result
    }
}

/// Trait defining the translation logic from Fxfs types into order-consistent binaries.
pub trait SerializeKey: Sized {
    /// Encodes key representation sequentially into serialization stream.
    fn serialize_key_to<B: Buffer>(&self, serializer: &mut KeySerializer<'_, B>);

    /// Decodes serializations sequentially from underlying raw bytes.
    fn deserialize_key_from(deserializer: &mut KeyDeserializer<'_>) -> Result<Self, Error>;

    /// Serializes this key directly into `buffer` with its length prefix and optional delta base.
    ///
    /// Matches the ergonomics of serde/bincode `serialize_into`.
    #[inline]
    fn serialize_key_into<B: Buffer>(
        &self,
        buffer: &mut B,
        base: Option<u64>,
    ) -> Result<(), Error> {
        let mut serializer = KeySerializer::new(buffer, base);
        self.serialize_key_to(&mut serializer);
        serializer.finalize()
    }
}

impl SerializeKey for u8 {
    #[inline]
    fn serialize_key_to<B: Buffer>(&self, serializer: &mut KeySerializer<'_, B>) {
        serializer.write_bytes(std::slice::from_ref(self));
    }
    #[inline]
    fn deserialize_key_from(deserializer: &mut KeyDeserializer<'_>) -> Result<Self, Error> {
        let mut b = [0u8; 1];
        deserializer.read_exact(&mut b)?;
        Ok(b[0])
    }
}

impl SerializeKey for u32 {
    #[inline]
    fn serialize_key_to<B: Buffer>(&self, serializer: &mut KeySerializer<'_, B>) {
        serializer.write_varint(*self as u64)
    }
    #[inline]
    fn deserialize_key_from(deserializer: &mut KeyDeserializer<'_>) -> Result<Self, Error> {
        Ok(deserializer.read_varint()?.try_into()?)
    }
}

impl SerializeKey for u64 {
    #[inline]
    fn serialize_key_to<B: Buffer>(&self, serializer: &mut KeySerializer<'_, B>) {
        serializer.write_u64(*self);
    }
    #[inline]
    fn deserialize_key_from(deserializer: &mut KeyDeserializer<'_>) -> Result<Self, Error> {
        deserializer.read_u64()
    }
}

impl SerializeKey for String {
    fn serialize_key_to<B: Buffer>(&self, serializer: &mut KeySerializer<'_, B>) {
        serializer.write_last(self.as_bytes());
    }
    fn deserialize_key_from(deserializer: &mut KeyDeserializer<'_>) -> Result<Self, Error> {
        Ok(String::from_utf8(deserializer.read_last())?)
    }
}

impl SerializeKey for fxfs_unicode::CasefoldString {
    fn serialize_key_to<B: Buffer>(&self, serializer: &mut KeySerializer<'_, B>) {
        let s: &str = self.as_str();
        serializer.write_last(s.as_bytes());
    }
    fn deserialize_key_from(deserializer: &mut KeyDeserializer<'_>) -> Result<Self, Error> {
        Ok(Self::new(String::deserialize_key_from(deserializer)?))
    }
}

impl SerializeKey for Vec<u8> {
    fn serialize_key_to<B: Buffer>(&self, serializer: &mut KeySerializer<'_, B>) {
        serializer.write_last(self);
    }
    fn deserialize_key_from(deserializer: &mut KeyDeserializer<'_>) -> Result<Self, Error> {
        Ok(deserializer.read_last())
    }
}

impl SerializeKey for std::ops::Range<u64> {
    #[inline]
    fn serialize_key_to<B: Buffer>(&self, serializer: &mut KeySerializer<'_, B>) {
        // Range upper-bounds are typically critical when evaluating extent allocations
        // in tree merges, so we write end values before length (end - start) values,
        // which makes narrower ranges sort first on ties, matching OrdUpperBound.
        self.end.serialize_key_to(serializer);
        self.end.saturating_sub(self.start).serialize_key_to(serializer);
    }
    #[inline]
    fn deserialize_key_from(deserializer: &mut KeyDeserializer<'_>) -> Result<Self, Error> {
        let end = u64::deserialize_key_from(deserializer)?;
        let len = u64::deserialize_key_from(deserializer)?;
        let start = end.checked_sub(len).ok_or_else(|| anyhow!("Underflow in range start"))?;
        Ok(start..end)
    }
}

impl SerializeKey for std::num::NonZeroU64 {
    #[inline]
    fn serialize_key_to<B: Buffer>(&self, serializer: &mut KeySerializer<'_, B>) {
        self.get().serialize_key_to(serializer);
    }
    #[inline]
    fn deserialize_key_from(deserializer: &mut KeyDeserializer<'_>) -> Result<Self, Error> {
        let raw = u64::deserialize_key_from(deserializer)?;
        Self::new(raw).ok_or_else(|| anyhow::anyhow!("Expected non-zero value"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsm_tree::types::OrdUpperBound;
    use crate::object_store::allocator::AllocatorKey;
    use crate::object_store::object_record::{ObjectKey, ObjectKeyData};
    use crate::object_store::{AttributeId, Extent};

    #[test]
    fn test_object_key_order_matches_cmp_upper_bound() {
        let mut keys = Vec::new();
        // Varint edge cases
        keys.push(ObjectKey::object(0));
        keys.push(ObjectKey::object(1));
        keys.push(ObjectKey::object(0xbe));
        keys.push(ObjectKey::object(0xbf));
        keys.push(ObjectKey::object(0xc0));
        keys.push(ObjectKey::object(0x1ffe));
        keys.push(ObjectKey::object(0x1fff));
        keys.push(ObjectKey::object(0x2000));
        keys.push(ObjectKey::object(0x0fff_ffff));
        keys.push(ObjectKey::object(0x1000_0000));

        // String edge cases
        keys.push(ObjectKey { object_id: 1, data: ObjectKeyData::Child { name: "".to_string() } });
        keys.push(ObjectKey { object_id: 1, data: ObjectKeyData::Child { name: "a".to_string() } });
        keys.push(ObjectKey { object_id: 1, data: ObjectKeyData::Child { name: "b".to_string() } });
        keys.push(ObjectKey {
            object_id: 1,
            data: ObjectKeyData::Child { name: "aa".to_string() },
        });
        keys.push(ObjectKey { object_id: 1, data: ObjectKeyData::Child { name: "a".repeat(300) } });

        // Extent edge cases
        keys.push(ObjectKey::extent(1, AttributeId::TEST_ID, 100 * 512..200 * 512));
        keys.push(ObjectKey::extent(1, AttributeId::TEST_ID, 100 * 512..150 * 512));
        keys.push(ObjectKey::extent(1, AttributeId::TEST_ID, 50 * 512..150 * 512));
        keys.push(ObjectKey::extent(1, AttributeId::TEST_ID, 150 * 512..200 * 512));
        keys.push(ObjectKey::extent(2, AttributeId::TEST_ID, 100 * 512..200 * 512));
        keys.push(ObjectKey::extent(1, AttributeId::TEST_ID, 0..100 * 512));
        keys.push(ObjectKey::extent(1, AttributeId::TEST_ID, 50 * 512..100 * 512));

        // Compare all pairs. We compare against `cmp_upper_bound` which is now a total order
        // for ranges (comparing end then start), matching serialization order.
        for i in 0..keys.len() {
            for j in 0..keys.len() {
                let mut buf_a = Vec::new();
                let mut ser_a = KeySerializer::new(&mut buf_a, Some(0));
                keys[i].serialize_key_to(&mut ser_a);
                ser_a.finalize().unwrap();

                let mut buf_b = Vec::new();
                let mut ser_b = KeySerializer::new(&mut buf_b, Some(0));
                keys[j].serialize_key_to(&mut ser_b);
                ser_b.finalize().unwrap();

                std::mem::drop(ser_a);
                std::mem::drop(ser_b);

                let cmp = keys[i].cmp_upper_bound(&keys[j]);
                let ser_cmp = compare_keys(&buf_a, &buf_b).unwrap();
                assert_eq!(cmp, ser_cmp, "Mismatch for keys {:?} and {:?}", keys[i], keys[j]);
            }
        }
    }

    #[test]
    fn test_allocator_key_order_matches_cmp_upper_bound() {
        let mut keys = Vec::new();
        keys.push(AllocatorKey { device_range: Extent(0..100 * 512) });
        keys.push(AllocatorKey { device_range: Extent(0..200 * 512) });
        keys.push(AllocatorKey { device_range: Extent(100 * 512..200 * 512) });
        keys.push(AllocatorKey { device_range: Extent(100 * 512..150 * 512) });
        keys.push(AllocatorKey { device_range: Extent(50 * 512..150 * 512) });
        keys.push(AllocatorKey { device_range: Extent(0..50 * 512) });
        keys.push(AllocatorKey { device_range: Extent(50 * 512..100 * 512) });

        // Compare all pairs. We compare against `cmp_upper_bound` which is now a total order
        // for ranges, matching serialization order.
        for i in 0..keys.len() {
            for j in 0..keys.len() {
                let mut buf_a = Vec::new();
                let mut ser_a = KeySerializer::new(&mut buf_a, Some(0));
                keys[i].serialize_key_to(&mut ser_a);
                ser_a.finalize().unwrap();

                let mut buf_b = Vec::new();
                let mut ser_b = KeySerializer::new(&mut buf_b, Some(0));
                keys[j].serialize_key_to(&mut ser_b);
                ser_b.finalize().unwrap();

                std::mem::drop(ser_a);
                std::mem::drop(ser_b);

                let cmp = keys[i].cmp_upper_bound(&keys[j]);
                let ser_cmp = compare_keys(&buf_a, &buf_b).unwrap();

                assert_eq!(cmp, ser_cmp, "Mismatch for keys {:?} and {:?}", keys[i], keys[j]);
            }
        }
    }

    #[test]
    fn test_delta_encoding() {
        let mut buf = Vec::new();
        let base = 100;
        let val = 150;

        // Serialize
        {
            let mut ser = KeySerializer::new(&mut buf, Some(base));
            ser.write_u64(val);
            ser.finalize().unwrap();
        }

        // Deserialize
        let (mut deser, length) = KeyDeserializer::new(&buf, Some(base)).unwrap();
        assert_eq!(length, buf.len());
        let decoded_val = deser.read_u64().unwrap();

        assert_eq!(val, decoded_val);

        // Verify bytes (val - base = 50)
        assert_eq!(buf, vec![1, 50]);
    }

    #[test]
    fn test_delta_encoding_underflow_returns_error() {
        let mut buf = Vec::new();
        let base = 100;
        let val = 50;

        let mut ser = KeySerializer::new(&mut buf, Some(base));
        ser.write_u64(val);
        assert!(ser.finalize().is_err());
        std::mem::drop(ser);
        assert_eq!(buf.len(), 0);

        let mut buf2 = Vec::new();
        assert!(val.serialize_key_into(&mut buf2, Some(base)).is_err());
        assert_eq!(buf2.len(), 0);
    }

    #[test]
    fn test_finalize_after_error_and_write_last_is_idempotent() {
        let mut buf = Vec::new();
        let mut ser = KeySerializer::new(&mut buf, Some(100));
        ser.write_u64(50); // Underflow error latched.
        ser.write_last(b"trailing"); // Internally calls finalize(), discarding error.
        assert!(ser.finalize().is_err());
        std::mem::drop(ser);
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn test_delta_encoding_only_first() {
        let mut buf = Vec::new();
        let base = 100;
        let val1 = 150;
        let val2 = 200;

        // Serialize
        {
            let mut ser = KeySerializer::new(&mut buf, Some(base));
            ser.write_u64(val1);
            ser.write_u64(val2);
            ser.finalize().unwrap();
        }

        // Deserialize
        let (mut deser, length) = KeyDeserializer::new(&buf, Some(base)).unwrap();
        assert_eq!(length, buf.len());
        let decoded_val1 = deser.read_u64().unwrap();
        let decoded_val2 = deser.read_u64().unwrap();

        assert_eq!(val1, decoded_val1);
        assert_eq!(val2, decoded_val2);
    }

    #[test]
    fn test_delta_encoding_none() {
        let mut buf = Vec::new();
        let val = 150;

        // Serialize
        {
            let mut ser = KeySerializer::new(&mut buf, None);
            ser.write_u64(val);
            ser.finalize().unwrap();
        }

        // Deserialize
        let (mut deser, length) = KeyDeserializer::new(&buf, None).unwrap();
        assert_eq!(length, buf.len());
        let decoded_val = deser.read_u64().unwrap();

        assert_eq!(val, decoded_val);

        // Verify bytes (should be regular varint of 150, which fits in 1 byte in this encoding)
        assert_eq!(buf, vec![1, 150]);
    }

    #[test]
    fn test_delta_encoding_overflow_on_read_returns_error() {
        // Forge a buffer with a large varint.
        // Varint of u64::MAX is 9 bytes of 0xff.
        let buf = vec![9, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];

        // Deserialize with base = Some(1).
        // read_u64 should try to add 1 to u64::MAX and return error.
        let (mut deser, length) = KeyDeserializer::new(&buf, Some(1)).unwrap();
        assert_eq!(length, buf.len());
        assert!(deser.read_u64().is_err());
    }

    #[test]
    fn test_write_u64_with_base_not_first_returns_error() {
        let mut buf = Vec::new();
        let base = 100;
        let val = 150;

        let mut ser = KeySerializer::new(&mut buf, Some(base));
        ser.write_varint(5); // Write something else first
        ser.write_u64(val);
        assert!(ser.finalize().is_err());
        std::mem::drop(ser);
        assert_eq!(buf.len(), 0);
    }

    #[test]
    #[should_panic(expected = "read_u64 with base must be the first item")]
    fn test_read_u64_with_base_not_first_panics() {
        let mut buf = Vec::new();
        let base = 100;
        let val1 = 150;
        let val2 = 200;

        {
            let mut ser = KeySerializer::new(&mut buf, Some(base));
            ser.write_u64(val1);
            ser.write_u64(val2);
            ser.finalize().unwrap();
        }

        let (mut deser, length) = KeyDeserializer::new(&buf, Some(base)).unwrap();
        assert_eq!(length, buf.len());
        deser.read_varint().unwrap(); // Reads val1 (delta)
        deser.read_u64().unwrap(); // Tries to read val2 as u64.
    }

    #[test]
    fn test_casefold_string_serialization_preserves_case() {
        use fxfs_unicode::CasefoldString;
        let s = CasefoldString::new("Hello World".to_string());

        let mut buf = Vec::new();
        {
            let mut ser = KeySerializer::new(&mut buf, None);
            s.serialize_key_to(&mut ser);
            ser.finalize().unwrap();
        }

        let (mut deser, len) = KeyDeserializer::new(&buf, None).unwrap();
        assert_eq!(len, buf.len());
        let deserialized = CasefoldString::deserialize_key_from(&mut deser).unwrap();
        assert_eq!(deserialized.as_str(), "Hello World");
        assert_eq!(deserialized, s);
    }

    #[test]
    fn test_chunk_prefix_lengths() {
        use std::cmp::Ordering;

        // Key < 254 bytes: single byte prefix.
        let key_100 = vec![42u8; 100];
        let mut buf_100 = Vec::new();
        key_100.serialize_key_into(&mut buf_100, None).unwrap();
        assert_eq!(buf_100[0], 100);
        assert_eq!(buf_100.len(), 101);
        let (mut deser, len) = KeyDeserializer::new(&buf_100, None).unwrap();
        assert_eq!(len, 101);
        assert_eq!(deser.read_last(), key_100.as_slice());
        assert!(deser.is_empty());

        // Key == 254 bytes: single byte prefix (254).
        let key_254 = vec![42u8; 254];
        let mut buf_254 = Vec::new();
        key_254.serialize_key_into(&mut buf_254, None).unwrap();
        assert_eq!(buf_254[0], 254);
        assert_eq!(buf_254.len(), 255);
        let (mut deser, len) = KeyDeserializer::new(&buf_254, None).unwrap();
        assert_eq!(len, 255);
        assert_eq!(deser.read_last(), key_254.as_slice());
        assert!(deser.is_empty());

        // Key == 255 bytes: chunk 0 has len 255 (256 bytes), chunk 1 has len 0 (1 byte).
        let key_255 = vec![42u8; 255];
        let mut buf_255 = Vec::new();
        key_255.serialize_key_into(&mut buf_255, None).unwrap();
        assert_eq!(buf_255[0], 255);
        assert_eq!(buf_255[256], 0);
        assert_eq!(buf_255.len(), 257);
        let (mut deser, len) = KeyDeserializer::new(&buf_255, None).unwrap();
        assert_eq!(len, 257);
        assert_eq!(deser.read_last(), key_255.as_slice());
        assert!(deser.is_empty());

        // Key == 256 bytes: chunk 0 has len 255 (256 bytes), chunk 1 has len 1 (2 bytes).
        let key_256 = vec![42u8; 256];
        let mut buf_256 = Vec::new();
        key_256.serialize_key_into(&mut buf_256, None).unwrap();
        assert_eq!(buf_256[0], 255);
        assert_eq!(buf_256[256], 1);
        assert_eq!(buf_256.len(), 258);
        let (mut deser, len) = KeyDeserializer::new(&buf_256, None).unwrap();
        assert_eq!(len, 258);
        assert_eq!(deser.read_last(), key_256.as_slice());
        assert!(deser.is_empty());

        // Key == 510 bytes: chunk 0 (256 bytes), chunk 1 (256 bytes), chunk 2 has len 0 (1 byte).
        let key_510 = vec![42u8; 510];
        let mut buf_510 = Vec::new();
        key_510.serialize_key_into(&mut buf_510, None).unwrap();
        assert_eq!(buf_510[0], 255);
        assert_eq!(buf_510[256], 255);
        assert_eq!(buf_510[512], 0);
        assert_eq!(buf_510.len(), 513);
        let (mut deser, len) = KeyDeserializer::new(&buf_510, None).unwrap();
        assert_eq!(len, 513);
        assert_eq!(deser.read_last(), key_510.as_slice());
        assert!(deser.is_empty());

        // Relative ordering: shorter prefixes compare Less than longer extensions.
        assert_eq!(compare_keys(&buf_100, &buf_254).unwrap(), Ordering::Less);
        assert_eq!(compare_keys(&buf_254, &buf_100).unwrap(), Ordering::Greater);
        assert_eq!(compare_keys(&buf_254, &buf_255).unwrap(), Ordering::Less);
        assert_eq!(compare_keys(&buf_255, &buf_254).unwrap(), Ordering::Greater);
        assert_eq!(compare_keys(&buf_255, &buf_256).unwrap(), Ordering::Less);
        assert_eq!(compare_keys(&buf_256, &buf_255).unwrap(), Ordering::Greater);
        assert_eq!(compare_keys(&buf_256, &buf_510).unwrap(), Ordering::Less);
        assert_eq!(compare_keys(&buf_510, &buf_256).unwrap(), Ordering::Greater);
        assert_eq!(compare_keys(&buf_255, &buf_255).unwrap(), Ordering::Equal);
        assert_eq!(compare_keys(&buf_510, &buf_510).unwrap(), Ordering::Equal);
    }

    #[test]
    fn test_chunk_empty_key() {
        use std::cmp::Ordering;

        let empty: Vec<u8> = Vec::new();
        let mut buf = Vec::new();
        empty.serialize_key_into(&mut buf, None).unwrap();
        assert_eq!(buf, &[0]);

        let (mut deser, len) = KeyDeserializer::new(&buf, None).unwrap();
        assert_eq!(len, 1);
        assert!(deser.is_empty());
        assert_eq!(deser.read_last(), &[0u8; 0]);

        let non_empty: Vec<u8> = vec![1];
        let mut buf_non_empty = Vec::new();
        non_empty.serialize_key_into(&mut buf_non_empty, None).unwrap();

        assert_eq!(compare_keys(&buf, &buf).unwrap(), Ordering::Equal);
        assert_eq!(compare_keys(&buf, &buf_non_empty).unwrap(), Ordering::Less);
        assert_eq!(compare_keys(&buf_non_empty, &buf).unwrap(), Ordering::Greater);
    }

    #[test]
    fn test_chunk_multiples_of_255() {
        use std::cmp::Ordering;

        // Multiples: 1 * 255, 2 * 255, 3 * 255, 4 * 255.
        for multiple in 1..=4 {
            let num_bytes = multiple * MAX_CHUNK_LEN;
            let payload: Vec<u8> = (0..num_bytes).map(|i| (i % 251) as u8).collect();
            let mut buf = Vec::new();
            payload.serialize_key_into(&mut buf, None).unwrap();

            // Expected buffer layout: `multiple` chunks of (1 byte length 255 + 255 bytes payload),
            // followed by a single 0 byte (terminating chunk).
            let expected_len = multiple * (1 + MAX_CHUNK_LEN) + 1;
            assert_eq!(buf.len(), expected_len, "Failed for multiple {}", multiple);

            for c in 0..multiple {
                assert_eq!(buf[c * (1 + MAX_CHUNK_LEN)], MAX_CHUNK_LEN as u8);
            }
            assert_eq!(buf[multiple * (1 + MAX_CHUNK_LEN)], 0);

            // Deserialization round-trip.
            let (mut deser, len) = KeyDeserializer::new(&buf, None).unwrap();
            assert_eq!(len, expected_len);
            let read_back = deser.read_last();
            assert_eq!(read_back, payload.as_slice());
            assert!(deser.is_empty());

            // Self-comparison.
            assert_eq!(compare_keys(&buf, &buf).unwrap(), Ordering::Equal);
        }
    }

    #[test]
    fn test_chunk_off_by_one_boundaries() {
        let sizes = [
            0, 1, 2, 253, 254, 255, 256, 257, 508, 509, 510, 511, 512, 763, 764, 765, 766, 767,
            1019, 1020, 1021,
        ];

        let mut buffers = Vec::new();
        for &size in &sizes {
            let payload = vec![0x77u8; size];
            let mut buf = Vec::new();
            payload.serialize_key_into(&mut buf, None).unwrap();

            let (mut deser, len) = KeyDeserializer::new(&buf, None).unwrap();
            assert_eq!(len, buf.len());
            assert_eq!(deser.read_last(), payload.as_slice());
            assert!(deser.is_empty());

            buffers.push((size, buf));
        }

        // Pairwise comparison ordering matches length ordering since all payloads are identical bytes.
        for i in 0..buffers.len() {
            for j in 0..buffers.len() {
                let (len_i, ref buf_i) = buffers[i];
                let (len_j, ref buf_j) = buffers[j];

                let expected = len_i.cmp(&len_j);
                let actual = compare_keys(buf_i, buf_j).unwrap();
                assert_eq!(
                    actual, expected,
                    "Comparison mismatch between size {} and size {}",
                    len_i, len_j
                );
            }
        }
    }

    #[test]
    fn test_chunk_boundary_divergence() {
        use std::cmp::Ordering;

        let divergence_indices = [0, 1, 100, 253, 254, 255, 256, 300, 508, 509, 510, 511, 600];
        let total_len = 700;

        for &idx in &divergence_indices {
            let mut p_a = vec![0x33u8; total_len];
            let mut p_b = vec![0x33u8; total_len];
            p_a[idx] = 0x10;
            p_b[idx] = 0x20;

            let mut buf_a = Vec::new();
            p_a.serialize_key_into(&mut buf_a, None).unwrap();
            let mut buf_b = Vec::new();
            p_b.serialize_key_into(&mut buf_b, None).unwrap();

            assert_eq!(
                compare_keys(&buf_a, &buf_b).unwrap(),
                Ordering::Less,
                "Failed at divergence index {}",
                idx
            );
            assert_eq!(
                compare_keys(&buf_b, &buf_a).unwrap(),
                Ordering::Greater,
                "Failed at divergence index {}",
                idx
            );
        }
    }

    #[test]
    fn test_chunk_prefix_and_multi_chunk_last() {
        use std::cmp::Ordering;

        let prefix_val = 12345u64;
        let trailing_payload: Vec<u8> = (0..600).map(|i| (i * 7 % 256) as u8).collect();

        let mut buf = Vec::new();
        {
            let mut ser = KeySerializer::new(&mut buf, None);
            ser.write_u64(prefix_val);
            ser.write_last(&trailing_payload);
        }

        let (mut deser, len) = KeyDeserializer::new(&buf, None).unwrap();
        assert_eq!(len, buf.len());
        assert_eq!(deser.read_u64().unwrap(), prefix_val);
        assert_eq!(deser.read_last(), trailing_payload.as_slice());
        assert!(deser.is_empty());

        // Test comparison: difference in prefix vs difference in trailing payload.
        let mut buf_diff_prefix = Vec::new();
        {
            let mut ser = KeySerializer::new(&mut buf_diff_prefix, None);
            ser.write_u64(prefix_val + 1);
            ser.write_last(&trailing_payload);
        }
        assert_eq!(compare_keys(&buf, &buf_diff_prefix).unwrap(), Ordering::Less);
        assert_eq!(compare_keys(&buf_diff_prefix, &buf).unwrap(), Ordering::Greater);

        let mut buf_diff_trailing = Vec::new();
        let mut diff_trailing_payload = trailing_payload.clone();
        diff_trailing_payload[400] = diff_trailing_payload[400].wrapping_add(1);
        {
            let mut ser = KeySerializer::new(&mut buf_diff_trailing, None);
            ser.write_u64(prefix_val);
            ser.write_last(&diff_trailing_payload);
        }
        assert_ne!(compare_keys(&buf, &buf_diff_trailing).unwrap(), Ordering::Equal);
    }

    #[test]
    fn test_chunk_trailing_garbage() {
        use std::cmp::Ordering;

        let test_payloads = [
            vec![0x11; 50],  // Single chunk (< 255)
            vec![0x22; 255], // Exact 255 (with 0-terminator)
            vec![0x33; 300], // Multi-chunk (255 + 45)
            vec![0x44; 510], // Exact 510 (with 0-terminator)
        ];

        for payload in &test_payloads {
            let mut clean_buf = Vec::new();
            payload.serialize_key_into(&mut clean_buf, None).unwrap();

            // Append garbage to buffer A and different garbage to buffer B.
            let mut buf_with_garbage_a = clean_buf.clone();
            buf_with_garbage_a.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef, 0x99, 0x88]);

            let mut buf_with_garbage_b = clean_buf.clone();
            buf_with_garbage_b.extend_from_slice(&[0x12, 0x34, 0x56, 0x78]);

            // compare_keys must only inspect key chunks and ignore trailing bytes.
            assert_eq!(
                compare_keys(&buf_with_garbage_a, &buf_with_garbage_b).unwrap(),
                Ordering::Equal
            );
            assert_eq!(compare_keys(&clean_buf, &buf_with_garbage_a).unwrap(), Ordering::Equal);

            // KeyDeserializer::new must return the exact length of the serialized key, not the full buffer.
            let (mut deser, parsed_len) = KeyDeserializer::new(&buf_with_garbage_a, None).unwrap();
            assert_eq!(parsed_len, clean_buf.len());
            assert_eq!(deser.read_last(), payload.as_slice());
            assert!(deser.is_empty());
        }
    }

    #[test]
    fn test_chunk_malformed_buffers() {
        // 1. Completely empty buffer.
        assert!(compare_keys(&[], &[0]).is_err());
        assert!(compare_keys(&[0], &[]).is_err());
        assert!(KeyDeserializer::new(&[], None).is_err());

        // 2. Chunk 0 specifies length beyond buffer.
        let truncated1 = [5u8, 1, 2]; // Claims 5 bytes, only 2 exist.
        assert!(compare_keys(&truncated1, &[0]).is_err());
        assert!(KeyDeserializer::new(&truncated1, None).is_err());

        // 3. Chunk 0 specifies 255, but fewer than 255 payload bytes exist.
        let mut truncated2 = vec![255u8];
        truncated2.extend_from_slice(&[0xaa; 200]);
        assert!(compare_keys(&truncated2, &[0]).is_err());
        assert!(KeyDeserializer::new(&truncated2, None).is_err());

        // 4. Chunk 0 has exactly 255 bytes, but buffer ends abruptly without next chunk header.
        let mut truncated3 = vec![255u8];
        truncated3.extend_from_slice(&[0xaa; 255]);
        // When chunk 0 matches, compare_keys must inspect chunk 1 and fail because it is missing.
        assert!(compare_keys(&truncated3, &truncated3).is_err());
        assert!(KeyDeserializer::new(&truncated3, None).is_err());

        // 5. Next chunk specifies length beyond buffer.
        let mut truncated4 = truncated3.clone();
        truncated4.push(10); // Next chunk claims 10 bytes
        truncated4.extend_from_slice(&[0xbb; 3]); // only 3 provided
        assert!(compare_keys(&truncated4, &truncated4).is_err());
        assert!(KeyDeserializer::new(&truncated4, None).is_err());

        // 6. Overrun on deser.read_exact.
        let valid = [2u8, 0x11, 0x22];
        let (mut deser, _) = KeyDeserializer::new(&valid, None).unwrap();
        let mut dst = [0u8; 5];
        assert!(deser.read_exact(&mut dst).is_err());
    }

    #[test]
    fn test_chunk_serializer_rollback() {
        let mut buf = vec![1, 2, 3];
        // Rollback on finalize error (delta underflow).
        {
            let mut ser = KeySerializer::new(&mut buf, Some(100));
            ser.write_u64(50);
            assert!(ser.finalize().is_err());
        }
        assert_eq!(buf, vec![1, 2, 3]);

        // Rollback on drop with latched error.
        {
            let mut ser = KeySerializer::new(&mut buf, Some(100));
            ser.write_u64(50);
            // Dropped with error latched: should roll back to start_pos without panic.
        }
        assert_eq!(buf, vec![1, 2, 3]);
    }

    #[test]
    fn test_word_probe_comparison() {
        use std::cmp::Ordering;

        // Keys with early divergence (first 8 bytes differ).
        let mut key1 = Vec::new();
        100u64.serialize_key_into(&mut key1, None).unwrap();
        let mut key2 = Vec::new();
        200u64.serialize_key_into(&mut key2, None).unwrap();
        assert_eq!(compare_keys(&key1, &key2).unwrap(), Ordering::Less);
        assert_eq!(compare_keys(&key2, &key1).unwrap(), Ordering::Greater);
        assert_eq!(compare_keys(&key1, &key1).unwrap(), Ordering::Equal);

        // Keys with late divergence (first 8 bytes identical, differing at byte 8+).
        let mut long1 = Vec::new();
        {
            let mut ser = KeySerializer::new(&mut long1, None);
            ser.write_u64(500);
            ser.write_last(b"abc");
        }
        let mut long2 = Vec::new();
        {
            let mut ser = KeySerializer::new(&mut long2, None);
            ser.write_u64(500);
            ser.write_last(b"abd");
        }
        let mut long3 = Vec::new();
        {
            let mut ser = KeySerializer::new(&mut long3, None);
            ser.write_u64(500);
            ser.write_last(b"abcd");
        }
        assert_eq!(compare_keys(&long1, &long2).unwrap(), Ordering::Less);
        assert_eq!(compare_keys(&long2, &long1).unwrap(), Ordering::Greater);
        assert_eq!(compare_keys(&long1, &long3).unwrap(), Ordering::Less);
        assert_eq!(compare_keys(&long3, &long1).unwrap(), Ordering::Greater);
        assert_eq!(compare_keys(&long1, &long1).unwrap(), Ordering::Equal);

        // Keys shorter than 8 bytes.
        let mut short1 = Vec::new();
        10u32.serialize_key_into(&mut short1, None).unwrap();
        let mut short2 = Vec::new();
        20u32.serialize_key_into(&mut short2, None).unwrap();
        assert_eq!(compare_keys(&short1, &short2).unwrap(), Ordering::Less);
        assert_eq!(compare_keys(&short2, &short1).unwrap(), Ordering::Greater);
        assert_eq!(compare_keys(&short1, &short1).unwrap(), Ordering::Equal);
    }

    #[test]
    fn test_compare_properties() {
        use std::cmp::Ordering;

        let vals = [0u64, 1, 50, 100, 191, 192, 200, 1000, 50000, u64::MAX / 2, u64::MAX];
        let bases = [None, Some(0)];

        for &base in &bases {
            for i in 0..vals.len() {
                for j in 0..vals.len() {
                    let a = vals[i];
                    let b = vals[j];

                    let mut buf_a = Vec::new();
                    a.serialize_key_into(&mut buf_a, base).unwrap();
                    let mut buf_b = Vec::new();
                    b.serialize_key_into(&mut buf_b, base).unwrap();

                    let cmp_ab = compare_keys(&buf_a, &buf_b).unwrap();
                    let cmp_ba = compare_keys(&buf_b, &buf_a).unwrap();

                    // Anti-symmetry: cmp(a, b) == cmp(b, a).reverse()
                    assert_eq!(cmp_ab, cmp_ba.reverse());
                    assert_eq!(cmp_ab, a.cmp(&b));

                    // Transitivity check with a third element
                    for k in 0..vals.len() {
                        let c = vals[k];
                        let mut buf_c = Vec::new();
                        c.serialize_key_into(&mut buf_c, base).unwrap();
                        let cmp_bc = compare_keys(&buf_b, &buf_c).unwrap();
                        let cmp_ac = compare_keys(&buf_a, &buf_c).unwrap();

                        if cmp_ab == Ordering::Less && cmp_bc == Ordering::Less {
                            assert_eq!(cmp_ac, Ordering::Less);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(fuzz)]
#[cfg(fuzz_target = "fuzz_object_key_compare")]
mod fuzz_object_key_compare {
    use super::*;
    use crate::lsm_tree::types::OrdUpperBound;
    use crate::object_store::object_record::{ObjectKey, ObjectKeyData};
    use ::fuzz::fuzz;

    #[fuzz]
    fn fuzz_object_key_compare(input: (Vec<u8>, Vec<u8>)) {
        let Ok((mut deser_a, len_a)) = KeyDeserializer::new(&input.0, None) else {
            return;
        };
        let Ok(key_a) = ObjectKey::deserialize_key_from(&mut deser_a) else {
            return;
        };
        // LegacyCasefoldChild is ignored because it is not order-preserving (uses
        // case-preserving serialization but case-insensitive comparison) and is not used in
        // recently written formats.
        if matches!(key_a.data, ObjectKeyData::LegacyCasefoldChild(_)) {
            return;
        }
        if !deser_a.is_empty() {
            return;
        }

        let Ok((mut deser_b, len_b)) = KeyDeserializer::new(&input.1, None) else {
            return;
        };
        let Ok(key_b) = ObjectKey::deserialize_key_from(&mut deser_b) else {
            return;
        };
        if matches!(key_b.data, ObjectKeyData::LegacyCasefoldChild(_)) {
            return;
        }
        if !deser_b.is_empty() {
            return;
        }

        let mut buf_a = Vec::new();
        let mut ser_a = KeySerializer::new(&mut buf_a, Some(0));
        key_a.serialize_key_to(&mut ser_a);
        let _ = ser_a.finalize();
        std::mem::drop(ser_a);
        assert_eq!(buf_a, input.0[..len_a]);

        let mut buf_b = Vec::new();
        let mut ser_b = KeySerializer::new(&mut buf_b, Some(0));
        key_b.serialize_key_to(&mut ser_b);
        let _ = ser_b.finalize();
        std::mem::drop(ser_b);
        assert_eq!(buf_b, input.1[..len_b]);

        let cmp = key_a.cmp_upper_bound(&key_b);
        let ser_cmp = compare_keys(&buf_a, &buf_b).unwrap();
        assert_eq!(cmp, ser_cmp, "Mismatch for keys {:?} and {:?}", key_a, key_b);
    }
}

#[cfg(fuzz)]
#[cfg(fuzz_target = "fuzz_allocator_key_compare")]
mod fuzz_allocator_key_compare {
    use super::*;
    use crate::lsm_tree::types::OrdUpperBound;
    use crate::object_store::allocator::AllocatorKey;
    use ::fuzz::fuzz;

    #[fuzz]
    fn fuzz_allocator_key_compare(input: (Vec<u8>, Vec<u8>)) {
        let Ok((mut deser_a, len_a)) = KeyDeserializer::new(&input.0, None) else {
            return;
        };
        let Ok(key_a) = AllocatorKey::deserialize_key_from(&mut deser_a) else {
            return;
        };
        if !deser_a.is_empty() {
            return;
        }

        let Ok((mut deser_b, len_b)) = KeyDeserializer::new(&input.1, None) else {
            return;
        };
        let Ok(key_b) = AllocatorKey::deserialize_key_from(&mut deser_b) else {
            return;
        };
        if !deser_b.is_empty() {
            return;
        }

        let mut buf_a = Vec::new();
        let mut ser_a = KeySerializer::new(&mut buf_a, Some(0));
        key_a.serialize_key_to(&mut ser_a);
        let _ = ser_a.finalize();
        std::mem::drop(ser_a);
        assert_eq!(buf_a, input.0[..len_a]);

        let mut buf_b = Vec::new();
        let mut ser_b = KeySerializer::new(&mut buf_b, Some(0));
        key_b.serialize_key_to(&mut ser_b);
        let _ = ser_b.finalize();
        std::mem::drop(ser_b);
        assert_eq!(buf_b, input.1[..len_b]);

        let cmp = key_a.cmp_upper_bound(&key_b);
        let ser_cmp = compare_keys(&buf_a, &buf_b).unwrap();
        assert_eq!(cmp, ser_cmp, "Mismatch for keys {:?} and {:?}", key_a, key_b);
    }
}

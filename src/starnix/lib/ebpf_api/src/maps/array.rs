// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::buffer::{MapBuffer, VmoOrName};
use super::{MapError, MapImpl, MapKey, MapValueRef};
use ebpf::{EbpfBufferPtr, MapSchema};
use linux_uapi::BPF_NOEXIST;
use std::fmt::Debug;
use std::sync::Arc;

fn array_key_to_index(key: &[u8]) -> u32 {
    u32::from_ne_bytes(key.try_into().expect("incorrect key length"))
}

fn get_value_storage_size(value_size: usize) -> Option<usize> {
    Some(value_size.checked_add(MapBuffer::ALIGNMENT - 1)? & !(MapBuffer::ALIGNMENT - 1))
}

#[derive(Debug)]
pub struct Array {
    buffer: MapBuffer,

    num_entries: usize,
    value_size: usize,

    // Number of bytes per element. May be greater than `value_size` to ensure
    // proper alignment for the elements.
    bytes_per_element: usize,
}

impl Array {
    const MAX_ARRAY_SIZE: usize = u32::MAX as usize;

    pub fn new(schema: &MapSchema, vmo: impl Into<VmoOrName>) -> Result<Self, MapError> {
        // From <https://man7.org/linux/man-pages/man2/bpf.2.html>:
        //   The key is an array index, and must be exactly four
        //   bytes.
        if schema.key_size != 4 {
            return Err(MapError::InvalidParam);
        }

        let bytes_per_element =
            get_value_storage_size(schema.value_size as usize).ok_or(MapError::InvalidParam)?;
        let size = bytes_per_element
            .checked_mul(schema.max_entries as usize)
            .ok_or(MapError::InvalidParam)?;
        if size == 0 {
            return Err(MapError::InvalidParam);
        }
        if size > Self::MAX_ARRAY_SIZE {
            return Err(MapError::NoMemory);
        }

        let buffer = MapBuffer::new(size, vmo.into().with_name_prefix("ebpf:array"))?;
        Ok(Array {
            buffer,
            num_entries: schema.max_entries as usize,
            value_size: schema.value_size as usize,
            bytes_per_element,
        })
    }

    fn value_ptr<'a>(&'a self, key: &[u8]) -> Option<EbpfBufferPtr<'a>> {
        let index = array_key_to_index(key) as usize;
        let base = index * self.bytes_per_element;
        let limit = base + self.value_size;
        self.buffer.ptr().slice(base..limit)
    }
}

impl MapImpl for Array {
    fn lookup<'a>(&'a self, key: &[u8]) -> Option<MapValueRef<'a>> {
        self.value_ptr(key).map(|ptr| MapValueRef::new(ptr))
    }

    fn update(&self, key: &[u8], value: EbpfBufferPtr<'_>, flags: u64) -> Result<(), MapError> {
        assert!(value.len() == self.value_size);

        let ptr = self.value_ptr(key).ok_or(MapError::SizeLimit)?;

        if flags == BPF_NOEXIST as u64 {
            return Err(MapError::EntryExists);
        }

        ptr.copy(&value);

        Ok(())
    }

    fn delete(&self, _key: &[u8]) -> Result<(), MapError> {
        // From https://man7.org/linux/man-pages/man2/bpf.2.html:
        //
        //  map_delete_elem() fails with the error EINVAL, since
        //  elements cannot be deleted.
        Err(MapError::InvalidParam)
    }

    fn get_next_key(&self, key: Option<&[u8]>) -> Result<MapKey, MapError> {
        let next_index = key.map(|v| array_key_to_index(v) + 1).unwrap_or(0);
        if next_index as usize >= self.num_entries {
            return Err(MapError::InvalidKey);
        }
        Ok(MapKey::from_slice(&next_index.to_ne_bytes()))
    }

    fn vmo(&self) -> &Arc<zx::Vmo> {
        self.buffer.vmo()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use ebpf::MapFlags;
    use linux_uapi::bpf_map_type_BPF_MAP_TYPE_ARRAY;

    // Verifies that array elements are always 8-byte aligned.
    #[fuchsia::test]
    fn test_alignment() {
        let schema = MapSchema {
            map_type: bpf_map_type_BPF_MAP_TYPE_ARRAY,
            key_size: 4,
            value_size: 5,
            max_entries: 10,
            flags: MapFlags::empty(),
        };

        let array = Array::new(&schema, "test").unwrap();
        let value1 = array.lookup(&[0, 0, 0, 0]).unwrap();
        assert_eq!(value1.ptr().raw_ptr() as usize % 8, 0);
        assert_eq!(value1.ptr().len(), 5);

        let value2 = array.lookup(&[1, 0, 0, 0]).unwrap();
        assert_eq!(value2.ptr().raw_ptr() as usize % 8, 0);
        assert_eq!(value2.ptr().len(), 5);

        let schema = MapSchema {
            map_type: bpf_map_type_BPF_MAP_TYPE_ARRAY,
            key_size: 4,
            value_size: 10,
            max_entries: 10,
            flags: MapFlags::empty(),
        };

        let array = Array::new(&schema, "test").unwrap();
        let value1 = array.lookup(&[0, 0, 0, 0]).unwrap();
        assert_eq!(value1.ptr().raw_ptr() as usize % 8, 0);
        assert_eq!(value1.ptr().len(), 10);

        let value2 = array.lookup(&[1, 0, 0, 0]).unwrap();
        assert_eq!(value2.ptr().raw_ptr() as usize % 8, 0);
        assert_eq!(value2.ptr().len(), 10);
    }
}

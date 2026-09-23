// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(non_upper_case_globals)]

mod array;
mod buffer;
mod hashmap;
mod lock;
mod lpm_trie;
mod ring_buffer;
mod vmar;

pub use ring_buffer::RINGBUF_SIGNAL;
pub(crate) use ring_buffer::{RingBuffer, RingBufferWakeupPolicy};

use ebpf::{BpfValue, EbpfBufferPtr, MapFlags, MapReference, MapSchema};
use fidl_fuchsia_ebpf as febpf;
use inspect_stubs::track_stub;
use linux_uapi::{
    BPF_EXIST, BPF_NOEXIST, bpf_map_type, bpf_map_type_BPF_MAP_TYPE_ARENA,
    bpf_map_type_BPF_MAP_TYPE_ARRAY, bpf_map_type_BPF_MAP_TYPE_ARRAY_OF_MAPS,
    bpf_map_type_BPF_MAP_TYPE_BLOOM_FILTER, bpf_map_type_BPF_MAP_TYPE_CGROUP_ARRAY,
    bpf_map_type_BPF_MAP_TYPE_CGROUP_STORAGE, bpf_map_type_BPF_MAP_TYPE_CGRP_STORAGE,
    bpf_map_type_BPF_MAP_TYPE_CPUMAP, bpf_map_type_BPF_MAP_TYPE_DEVMAP,
    bpf_map_type_BPF_MAP_TYPE_DEVMAP_HASH, bpf_map_type_BPF_MAP_TYPE_HASH,
    bpf_map_type_BPF_MAP_TYPE_HASH_OF_MAPS, bpf_map_type_BPF_MAP_TYPE_INODE_STORAGE,
    bpf_map_type_BPF_MAP_TYPE_LPM_TRIE, bpf_map_type_BPF_MAP_TYPE_LRU_HASH,
    bpf_map_type_BPF_MAP_TYPE_LRU_PERCPU_HASH, bpf_map_type_BPF_MAP_TYPE_PERCPU_ARRAY,
    bpf_map_type_BPF_MAP_TYPE_PERCPU_CGROUP_STORAGE, bpf_map_type_BPF_MAP_TYPE_PERCPU_HASH,
    bpf_map_type_BPF_MAP_TYPE_PERF_EVENT_ARRAY, bpf_map_type_BPF_MAP_TYPE_PROG_ARRAY,
    bpf_map_type_BPF_MAP_TYPE_QUEUE, bpf_map_type_BPF_MAP_TYPE_REUSEPORT_SOCKARRAY,
    bpf_map_type_BPF_MAP_TYPE_RINGBUF, bpf_map_type_BPF_MAP_TYPE_SK_STORAGE,
    bpf_map_type_BPF_MAP_TYPE_SOCKHASH, bpf_map_type_BPF_MAP_TYPE_SOCKMAP,
    bpf_map_type_BPF_MAP_TYPE_STACK, bpf_map_type_BPF_MAP_TYPE_STACK_TRACE,
    bpf_map_type_BPF_MAP_TYPE_STRUCT_OPS, bpf_map_type_BPF_MAP_TYPE_TASK_STORAGE,
    bpf_map_type_BPF_MAP_TYPE_UNSPEC, bpf_map_type_BPF_MAP_TYPE_USER_RINGBUF,
    bpf_map_type_BPF_MAP_TYPE_XSKMAP,
};
use std::fmt::Debug;
use std::ops::Deref;
use std::pin::Pin;
use std::sync::Arc;

use crate::maps::buffer::VmoOrName;

#[derive(Debug, Eq, PartialEq)]
pub enum MapError {
    // Equivalent of EINVAL.
    InvalidParam,

    // No entry with the specified key,
    InvalidKey,

    // Entry already exists..
    EntryExists,

    // Map size limit has been reached.
    SizeLimit,

    // Cannot allocate memory.
    NoMemory,

    // Invalid VMO was passed for a shared map.
    InvalidVmo,

    // Specified map type is not supported.
    MapTypeNotSupported,

    // Specified map configuration is not supported.
    NotSupported,

    // An internal issue, e.g. failed to allocate VMO.
    Internal,
}
const SUPPORTED_FLAGS: MapFlags = MapFlags::NoPrealloc
    .union(MapFlags::SyscallReadOnly)
    .union(MapFlags::SyscallWriteOnly)
    .union(MapFlags::Mmapable)
    .union(MapFlags::Clone);

fn map_flags_from_fidl(flags: febpf::MapFlags) -> MapFlags {
    let mut r = MapFlags::empty();
    if flags.contains(febpf::MapFlags::NO_PREALLOC) {
        r = r | MapFlags::NoPrealloc;
    }
    if flags.contains(febpf::MapFlags::SYSCALL_READ_ONLY) {
        r = r | MapFlags::SyscallReadOnly;
    }
    if flags.contains(febpf::MapFlags::SYSCALL_WRITE_ONLY) {
        r = r | MapFlags::SyscallWriteOnly;
    }
    if flags.contains(febpf::MapFlags::MMAPABLE) {
        r = r | MapFlags::Mmapable;
    }
    if flags.contains(febpf::MapFlags::CLONE) {
        r = r | MapFlags::Clone;
    }
    r
}

fn map_flags_to_fidl(flags: MapFlags) -> Result<febpf::MapFlags, MapError> {
    if flags.contains(!SUPPORTED_FLAGS) {
        return Err(MapError::NotSupported);
    }

    let mut r = febpf::MapFlags::empty();
    if flags.contains(MapFlags::NoPrealloc) {
        r = r | febpf::MapFlags::NO_PREALLOC;
    }
    if flags.contains(MapFlags::SyscallReadOnly) {
        r = r | febpf::MapFlags::SYSCALL_READ_ONLY;
    }
    if flags.contains(MapFlags::SyscallWriteOnly) {
        r = r | febpf::MapFlags::SYSCALL_WRITE_ONLY;
    }
    if flags.contains(MapFlags::Mmapable) {
        r = r | febpf::MapFlags::MMAPABLE;
    }
    if flags.contains(MapFlags::Clone) {
        r = r | febpf::MapFlags::CLONE;
    }
    Ok(r)
}

fn validate_map_flags(schema: &MapSchema) -> Result<(), MapError> {
    let flags = schema.flags;
    if flags.contains(!SUPPORTED_FLAGS) {
        return Err(MapError::InvalidParam);
    }

    // Read-only and write-only flags are mutually exclusive.
    if flags.contains(MapFlags::SyscallReadOnly) && flags.contains(MapFlags::SyscallWriteOnly) {
        return Err(MapError::InvalidParam);
    }

    // `MMAPABLE` is valid only for arrays.
    if flags.contains(MapFlags::Mmapable) && schema.map_type != bpf_map_type_BPF_MAP_TYPE_ARRAY {
        return Err(MapError::InvalidParam);
    }

    // `CLONE` is valid only for socket storage maps.
    if flags.contains(MapFlags::Clone) && schema.map_type != bpf_map_type_BPF_MAP_TYPE_SK_STORAGE {
        return Err(MapError::InvalidParam);
    }

    Ok(())
}

trait MapImpl: Send + Sync + Debug {
    fn lookup<'a>(&'a self, key: &[u8]) -> Option<MapValueRef<'a>>;
    fn update(&self, key: &[u8], value: EbpfBufferPtr<'_>, flags: u64) -> Result<(), MapError>;
    fn delete(&self, key: &[u8]) -> Result<(), MapError>;
    fn get_next_key(&self, key: Option<&[u8]>) -> Result<MapKey, MapError>;
    fn vmo(&self) -> &Arc<zx::Vmo>;

    // Returns true if `POLLIN` is signaled for the map FD. Should be
    // overridden only for ring buffers.
    fn can_read(&self) -> Option<bool> {
        None
    }

    fn ringbuf_reserve(&self, _size: u32, _flags: u64) -> Result<usize, MapError> {
        Err(MapError::InvalidParam)
    }
}

/// A BPF map. This is a hashtable that can be accessed both by BPF programs and userspace.
#[derive(Debug)]
pub struct Map {
    pub schema: MapSchema,

    // The impl because it's required for some map implementations need to be
    // pinned, particularly ring buffers.
    map_impl: Pin<Box<dyn MapImpl + Sync>>,
}

/// Maps are normally kept pinned in memory since linked eBPF programs store direct pointers to
/// the maps they depend on.
#[derive(Debug, Clone)]
pub struct PinnedMap(Pin<Arc<Map>>);

impl Deref for PinnedMap {
    type Target = Map;
    fn deref(&self) -> &Self::Target {
        self.0.deref()
    }
}

impl MapReference for PinnedMap {
    fn schema(&self) -> &MapSchema {
        &self.0.schema
    }

    fn as_bpf_value(&self) -> BpfValue {
        BpfValue::from(self.deref() as *const Map)
    }

    fn get_data_ptr(&self) -> Option<BpfValue> {
        assert!(self.0.schema.map_type == bpf_map_type_BPF_MAP_TYPE_ARRAY);

        let key = [0u8; 4];
        self.0.lookup(&key).map(|v| BpfValue::from(v.ptr().raw_ptr()))
    }
}

// Avoid allocation for eBPF keys smaller than 16 bytes.
pub type MapKey = smallvec::SmallVec<[u8; 16]>;

// Avoid allocation for eBPF values smaller than 64 bytes.
pub type MapValue = smallvec::SmallVec<[u8; 64]>;

// Access rights required for a map VMO handle. Should be consistent with the
// rights specified in FIDL. READ, WRITE and MAP rights are required to access
// the map contents. SIGNAL and WAIT rights are used for synchronization.
// LINT.IfChange(map_rights)
const BASE_MAP_RIGHTS: zx::Rights = zx::Rights::READ
    .union(zx::Rights::WRITE)
    .union(zx::Rights::MAP)
    .union(zx::Rights::SIGNAL)
    .union(zx::Rights::WAIT);
// LINT.ThenChange(//sdk/fidl/fuchsia.ebpf/ebpf.fidl:map_rights)

// Rights for the VMO handle when sharing a map.
const SHARED_MAP_RIGHTS: zx::Rights = BASE_MAP_RIGHTS.union(zx::Rights::TRANSFER);

impl Map {
    pub fn new(schema: MapSchema, name: &str) -> Result<PinnedMap, MapError> {
        validate_map_flags(&schema)?;
        let map_impl = create_map_impl(&schema, name.to_string())?;
        Ok(PinnedMap(Arc::pin(Self { schema, map_impl })))
    }

    pub fn new_shared(shared: febpf::Map) -> Result<PinnedMap, MapError> {
        let febpf::Map { schema: Some(fidl_schema), vmo: Some(vmo), .. } = shared else {
            return Err(MapError::InvalidParam);
        };

        // Check VMO rights.
        let vmo_info = vmo.basic_info().map_err(|_| MapError::InvalidVmo)?;
        if !vmo_info.rights.contains(BASE_MAP_RIGHTS) {
            return Err(MapError::InvalidVmo);
        }

        let schema = MapSchema {
            map_type: fidl_map_type_to_bpf_map_type(fidl_schema.type_),
            key_size: fidl_schema.key_size,
            value_size: fidl_schema.value_size,
            max_entries: fidl_schema.max_entries,
            flags: map_flags_from_fidl(fidl_schema.flags),
        };

        let map_impl = create_map_impl(&schema, vmo)?;
        Ok(PinnedMap(Arc::pin(Self { schema, map_impl })))
    }

    pub fn share(&self) -> Result<febpf::Map, MapError> {
        Ok(febpf::Map {
            schema: Some(febpf::MapSchema {
                type_: bpf_map_type_to_fidl_map_type(self.schema.map_type),
                key_size: self.schema.key_size,
                value_size: self.schema.value_size,
                max_entries: self.schema.max_entries,
                flags: map_flags_to_fidl(self.schema.flags)?,
            }),
            vmo: Some(
                self.map_impl
                    .vmo()
                    .duplicate_handle(SHARED_MAP_RIGHTS)
                    .map_err(|_| MapError::Internal)?,
            ),
            ..Default::default()
        })
    }

    pub fn lookup<'a>(&'a self, key: &[u8]) -> Option<MapValueRef<'a>> {
        self.map_impl.lookup(key)
    }

    pub fn load(&self, key: &[u8]) -> Option<MapValue> {
        self.lookup(key).map(|v| v.ptr().load())
    }

    pub fn update(&self, key: &[u8], value: EbpfBufferPtr<'_>, flags: u64) -> Result<(), MapError> {
        if flags & (BPF_EXIST as u64) > 0 && flags & (BPF_NOEXIST as u64) > 0 {
            return Err(MapError::InvalidParam);
        }

        self.map_impl.update(key, value, flags)
    }

    pub fn delete(&self, key: &[u8]) -> Result<(), MapError> {
        self.map_impl.delete(key)
    }

    pub fn get_next_key(&self, key: Option<&[u8]>) -> Result<MapKey, MapError> {
        self.map_impl.get_next_key(key)
    }

    pub fn vmo(&self) -> &Arc<zx::Vmo> {
        self.map_impl.vmo()
    }

    pub fn can_read(&self) -> Option<bool> {
        self.map_impl.can_read()
    }

    pub fn ringbuf_reserve(&self, size: u32, flags: u64) -> Result<usize, MapError> {
        self.map_impl.ringbuf_reserve(size, flags)
    }

    pub fn uses_locks(&self) -> bool {
        self.schema.map_type != bpf_map_type_BPF_MAP_TYPE_ARRAY
    }
}

pub enum MapValueRef<'a> {
    PlainRef(EbpfBufferPtr<'a>),
    HashMapRef(hashmap::HashMapEntryRef<'a>),
    LpmTrieRef(lpm_trie::LpmTrieEntryRef<'a>),
}

impl<'a> MapValueRef<'a> {
    fn new(buf: EbpfBufferPtr<'a>) -> Self {
        Self::PlainRef(buf)
    }

    fn new_from_hashmap(hash_map_ref: hashmap::HashMapEntryRef<'a>) -> Self {
        Self::HashMapRef(hash_map_ref)
    }

    fn new_from_lpm_trie(lpm_trie_ref: lpm_trie::LpmTrieEntryRef<'a>) -> Self {
        Self::LpmTrieRef(lpm_trie_ref)
    }

    pub fn is_ref_counted(&self) -> bool {
        match self {
            Self::PlainRef(_) => false,
            Self::HashMapRef(_) | Self::LpmTrieRef(_) => true,
        }
    }

    pub fn ptr(&self) -> EbpfBufferPtr<'a> {
        match self {
            Self::PlainRef(buf) => *buf,
            Self::HashMapRef(hash_map_ref) => hash_map_ref.ptr(),
            Self::LpmTrieRef(lpm_trie_ref) => lpm_trie_ref.ptr(),
        }
    }
}

const SK_STORAGE_MAX_ENTRIES: u32 = 8192;

fn create_map_impl(
    schema: &MapSchema,
    vmo: impl Into<VmoOrName>,
) -> Result<Pin<Box<dyn MapImpl>>, MapError> {
    // The list of supported maps should be kept in sync with the enum values in
    // `fuchsia.ebpf.MapType`.
    match schema.map_type {
        // LINT.IfChange(supported_maps)
        bpf_map_type_BPF_MAP_TYPE_ARRAY => Ok(Box::pin(array::Array::new(schema, vmo)?)),
        bpf_map_type_BPF_MAP_TYPE_HASH => Ok(Box::pin(hashmap::HashMap::new(schema, vmo)?)),
        bpf_map_type_BPF_MAP_TYPE_RINGBUF => Ok(ring_buffer::RingBuffer::new(schema, vmo)?),
        bpf_map_type_BPF_MAP_TYPE_LPM_TRIE => Ok(Box::pin(lpm_trie::LpmTrie::new(schema, vmo)?)),
        bpf_map_type_BPF_MAP_TYPE_SK_STORAGE => {
            if schema.key_size != 4 || schema.max_entries != 0 {
                return Err(MapError::InvalidParam);
            }

            // SK_STORAGE maps are implemented as hashmaps with socket cookie used as a key.
            let schema = MapSchema {
                map_type: bpf_map_type_BPF_MAP_TYPE_HASH,
                key_size: 8,
                max_entries: SK_STORAGE_MAX_ENTRIES,
                value_size: schema.value_size,
                flags: MapFlags::NoPrealloc,
            };
            Ok(Box::pin(hashmap::HashMap::new(&schema, vmo)?))
        }

        // These types are in use, but not yet implemented. Incorrectly use Array or Hash for
        // these
        bpf_map_type_BPF_MAP_TYPE_DEVMAP_HASH => {
            track_stub!(TODO("https://fxbug.dev/323847465"), "BPF_MAP_TYPE_DEVMAP_HASH");
            // `BPF_F_RDONLY_PROG` is not yet implemented, but it's always set
            // for `DEVMAP` maps.
            let schema =
                MapSchema { flags: schema.flags.difference(MapFlags::ProgReadOnly), ..*schema };
            Ok(Box::pin(hashmap::HashMap::new(&schema, vmo)?))
        }
        bpf_map_type_BPF_MAP_TYPE_PERCPU_HASH => {
            track_stub!(TODO("https://fxbug.dev/323847465"), "BPF_MAP_TYPE_PERCPU_HASH");
            Ok(Box::pin(hashmap::HashMap::new(schema, vmo)?))
        }
        bpf_map_type_BPF_MAP_TYPE_PERCPU_ARRAY => {
            track_stub!(TODO("https://fxbug.dev/323847465"), "BPF_MAP_TYPE_PERCPU_ARRAY");
            Ok(Box::pin(array::Array::new(schema, vmo)?))
        }
        bpf_map_type_BPF_MAP_TYPE_LRU_HASH => {
            track_stub!(TODO("https://fxbug.dev/323847465"), "BPF_MAP_TYPE_LRU_HASH");
            Ok(Box::pin(hashmap::HashMap::new(schema, vmo)?))
        }
        // LINT.ThenChange(:fidl_map_types)

        // Unimplemented types
        bpf_map_type_BPF_MAP_TYPE_UNSPEC => {
            track_stub!(TODO("https://fxbug.dev/323847465"), "BPF_MAP_TYPE_UNSPEC");
            Err(MapError::MapTypeNotSupported)
        }
        bpf_map_type_BPF_MAP_TYPE_PROG_ARRAY => {
            track_stub!(TODO("https://fxbug.dev/323847465"), "BPF_MAP_TYPE_PROG_ARRAY");
            Err(MapError::MapTypeNotSupported)
        }
        bpf_map_type_BPF_MAP_TYPE_PERF_EVENT_ARRAY => {
            track_stub!(TODO("https://fxbug.dev/323847465"), "BPF_MAP_TYPE_PERF_EVENT_ARRAY");
            Err(MapError::MapTypeNotSupported)
        }
        bpf_map_type_BPF_MAP_TYPE_STACK_TRACE => {
            track_stub!(TODO("https://fxbug.dev/323847465"), "BPF_MAP_TYPE_STACK_TRACE");
            Err(MapError::MapTypeNotSupported)
        }
        bpf_map_type_BPF_MAP_TYPE_CGROUP_ARRAY => {
            track_stub!(TODO("https://fxbug.dev/323847465"), "BPF_MAP_TYPE_CGROUP_ARRAY");
            Err(MapError::MapTypeNotSupported)
        }
        bpf_map_type_BPF_MAP_TYPE_LRU_PERCPU_HASH => {
            track_stub!(TODO("https://fxbug.dev/323847465"), "BPF_MAP_TYPE_LRU_PERCPU_HASH");
            Err(MapError::MapTypeNotSupported)
        }
        bpf_map_type_BPF_MAP_TYPE_ARRAY_OF_MAPS => {
            track_stub!(TODO("https://fxbug.dev/323847465"), "BPF_MAP_TYPE_ARRAY_OF_MAPS");
            Err(MapError::MapTypeNotSupported)
        }
        bpf_map_type_BPF_MAP_TYPE_HASH_OF_MAPS => {
            track_stub!(TODO("https://fxbug.dev/323847465"), "BPF_MAP_TYPE_HASH_OF_MAPS");
            Err(MapError::MapTypeNotSupported)
        }
        bpf_map_type_BPF_MAP_TYPE_DEVMAP => {
            track_stub!(TODO("https://fxbug.dev/323847465"), "BPF_MAP_TYPE_DEVMAP");
            Err(MapError::MapTypeNotSupported)
        }
        bpf_map_type_BPF_MAP_TYPE_SOCKMAP => {
            track_stub!(TODO("https://fxbug.dev/323847465"), "BPF_MAP_TYPE_SOCKMAP");
            Err(MapError::MapTypeNotSupported)
        }
        bpf_map_type_BPF_MAP_TYPE_CPUMAP => {
            track_stub!(TODO("https://fxbug.dev/323847465"), "BPF_MAP_TYPE_CPUMAP");
            Err(MapError::MapTypeNotSupported)
        }
        bpf_map_type_BPF_MAP_TYPE_XSKMAP => {
            track_stub!(TODO("https://fxbug.dev/323847465"), "BPF_MAP_TYPE_XSKMAP");
            Err(MapError::MapTypeNotSupported)
        }
        bpf_map_type_BPF_MAP_TYPE_SOCKHASH => {
            track_stub!(TODO("https://fxbug.dev/323847465"), "BPF_MAP_TYPE_SOCKHASH");
            Err(MapError::MapTypeNotSupported)
        }
        bpf_map_type_BPF_MAP_TYPE_CGROUP_STORAGE => {
            track_stub!(TODO("https://fxbug.dev/323847465"), "BPF_MAP_TYPE_CGROUP_STORAGE");
            Err(MapError::MapTypeNotSupported)
        }
        bpf_map_type_BPF_MAP_TYPE_REUSEPORT_SOCKARRAY => {
            track_stub!(TODO("https://fxbug.dev/323847465"), "BPF_MAP_TYPE_REUSEPORT_SOCKARRAY");
            Err(MapError::MapTypeNotSupported)
        }
        bpf_map_type_BPF_MAP_TYPE_PERCPU_CGROUP_STORAGE => {
            track_stub!(TODO("https://fxbug.dev/323847465"), "BPF_MAP_TYPE_PERCPU_CGROUP_STORAGE");
            Err(MapError::MapTypeNotSupported)
        }
        bpf_map_type_BPF_MAP_TYPE_QUEUE => {
            track_stub!(TODO("https://fxbug.dev/323847465"), "BPF_MAP_TYPE_QUEUE");
            Err(MapError::MapTypeNotSupported)
        }
        bpf_map_type_BPF_MAP_TYPE_STACK => {
            track_stub!(TODO("https://fxbug.dev/323847465"), "BPF_MAP_TYPE_STACK");
            Err(MapError::MapTypeNotSupported)
        }
        bpf_map_type_BPF_MAP_TYPE_STRUCT_OPS => {
            track_stub!(TODO("https://fxbug.dev/323847465"), "BPF_MAP_TYPE_STRUCT_OPS");
            Err(MapError::MapTypeNotSupported)
        }
        bpf_map_type_BPF_MAP_TYPE_INODE_STORAGE => {
            track_stub!(TODO("https://fxbug.dev/323847465"), "BPF_MAP_TYPE_INODE_STORAGE");
            Err(MapError::MapTypeNotSupported)
        }
        bpf_map_type_BPF_MAP_TYPE_TASK_STORAGE => {
            track_stub!(TODO("https://fxbug.dev/323847465"), "BPF_MAP_TYPE_TASK_STORAGE");
            Err(MapError::MapTypeNotSupported)
        }
        bpf_map_type_BPF_MAP_TYPE_BLOOM_FILTER => {
            track_stub!(TODO("https://fxbug.dev/323847465"), "BPF_MAP_TYPE_BLOOM_FILTER");
            Err(MapError::MapTypeNotSupported)
        }
        bpf_map_type_BPF_MAP_TYPE_USER_RINGBUF => {
            track_stub!(TODO("https://fxbug.dev/323847465"), "BPF_MAP_TYPE_USER_RINGBUF");
            Err(MapError::MapTypeNotSupported)
        }
        bpf_map_type_BPF_MAP_TYPE_CGRP_STORAGE => {
            track_stub!(TODO("https://fxbug.dev/323847465"), "BPF_MAP_TYPE_CGRP_STORAGE");
            Err(MapError::MapTypeNotSupported)
        }
        bpf_map_type_BPF_MAP_TYPE_ARENA => {
            track_stub!(TODO("https://fxbug.dev/323847465"), "BPF_MAP_TYPE_ARENA");
            Err(MapError::MapTypeNotSupported)
        }
        _ => {
            track_stub!(
                TODO("https://fxbug.dev/323847465"),
                "unknown bpf map type",
                schema.map_type
            );
            Err(MapError::InvalidParam)
        }
    }
}

pub fn compute_map_storage_size(schema: &MapSchema) -> Result<usize, MapError> {
    schema.value_size.checked_mul(schema.max_entries).map(|v| v as usize).ok_or(MapError::NoMemory)
}

// LINT.IfChange(fidl_map_types)
fn bpf_map_type_to_fidl_map_type(map_type: bpf_map_type) -> febpf::MapType {
    match map_type {
        bpf_map_type_BPF_MAP_TYPE_ARRAY => febpf::MapType::Array,
        bpf_map_type_BPF_MAP_TYPE_HASH => febpf::MapType::HashMap,
        bpf_map_type_BPF_MAP_TYPE_RINGBUF => febpf::MapType::RingBuffer,
        bpf_map_type_BPF_MAP_TYPE_PERCPU_ARRAY => febpf::MapType::PercpuArray,
        bpf_map_type_BPF_MAP_TYPE_PERCPU_HASH => febpf::MapType::PercpuHash,
        bpf_map_type_BPF_MAP_TYPE_DEVMAP_HASH => febpf::MapType::DevmapHash,
        bpf_map_type_BPF_MAP_TYPE_LPM_TRIE => febpf::MapType::LpmTrie,
        bpf_map_type_BPF_MAP_TYPE_LRU_HASH => febpf::MapType::LruHash,
        bpf_map_type_BPF_MAP_TYPE_SK_STORAGE => febpf::MapType::SkStorage,
        _ =>
        // Other map types are rejected in `create_map_impl()`.
        {
            unreachable!("unsupported map type {:?}", map_type)
        }
    }
}

fn fidl_map_type_to_bpf_map_type(map_type: febpf::MapType) -> bpf_map_type {
    match map_type {
        febpf::MapType::Array => bpf_map_type_BPF_MAP_TYPE_ARRAY,
        febpf::MapType::HashMap => bpf_map_type_BPF_MAP_TYPE_HASH,
        febpf::MapType::RingBuffer => bpf_map_type_BPF_MAP_TYPE_RINGBUF,
        febpf::MapType::PercpuArray => bpf_map_type_BPF_MAP_TYPE_PERCPU_ARRAY,
        febpf::MapType::PercpuHash => bpf_map_type_BPF_MAP_TYPE_PERCPU_HASH,
        febpf::MapType::DevmapHash => bpf_map_type_BPF_MAP_TYPE_DEVMAP_HASH,
        febpf::MapType::LpmTrie => bpf_map_type_BPF_MAP_TYPE_LPM_TRIE,
        febpf::MapType::LruHash => bpf_map_type_BPF_MAP_TYPE_LRU_HASH,
        febpf::MapType::SkStorage => bpf_map_type_BPF_MAP_TYPE_SK_STORAGE,
    }
}
// LINT.ThenChange(:supported_maps, //sdk/fidl/fuchsia.ebpf/ebpf.fidl:map_types)

#[cfg(test)]
mod test {
    use super::*;

    #[fuchsia::test]
    fn test_sharing_array() {
        let schema = MapSchema {
            map_type: bpf_map_type_BPF_MAP_TYPE_ARRAY,
            key_size: 4,
            value_size: 4,
            max_entries: 10,
            flags: MapFlags::empty(),
        };

        // Create two array maps sharing the content.
        let map1 = Map::new(schema, "test").unwrap();
        let map2 = Map::new_shared(map1.share().unwrap()).unwrap();

        // Set a value in one map and check that it's updated in the other.
        let key = vec![0, 0, 0, 0];
        let mut value = [0, 1, 2, 3];
        map1.update(&MapKey::from_vec(key.clone()), (&mut value).into(), 0).unwrap();
        assert_eq!(&map2.load(&key).unwrap()[..], &value);
    }

    #[fuchsia::test]
    fn test_sharing_hash_map() {
        let schema = MapSchema {
            map_type: bpf_map_type_BPF_MAP_TYPE_HASH,
            key_size: 4,
            value_size: 4,
            max_entries: 10,
            flags: MapFlags::empty(),
        };

        // Create two array maps sharing the content.
        let map1 = Map::new(schema, "test").unwrap();
        let map2 = Map::new_shared(map1.share().unwrap()).unwrap();

        // Set a value in one map and check that it's updated in the other.
        let key = vec![0, 0, 0, 0];
        let mut value = [0, 1, 2, 3];
        map1.update(&MapKey::from_vec(key.clone()), (&mut value).into(), 0).unwrap();
        assert_eq!(&map2.load(&key).unwrap()[..], &value);
    }

    #[fuchsia::test]
    fn test_hash_map() {
        let schema = MapSchema {
            map_type: bpf_map_type_BPF_MAP_TYPE_HASH,
            key_size: 5,
            value_size: 25,
            max_entries: 10000,
            flags: MapFlags::empty(),
        };

        let get_key = |i| {
            MapKey::from_vec(vec![
                (i & 0xffusize) as u8,
                0,
                ((i >> 4) & 0xffusize) as u8,
                0,
                ((i >> 8) & 0xffusize) as u8,
            ])
        };
        let get_value = |i, v| format!("--{:010} {:010}--", i, v).into_bytes();

        let map = Map::new(schema, "test").unwrap();

        for i in 0..10000 {
            assert!(map.update(&get_key(i), (&mut get_value(i, 0)).into(), 0).is_ok());
        }

        // Should fail to add another entry when the map is full.
        assert_eq!(
            map.update(&get_key(10001), (&mut get_value(10001, 1)).into(), 0),
            Err(MapError::SizeLimit)
        );

        for i in 0..10000 {
            assert_eq!(&map.load(&get_key(i)).unwrap()[..], &get_value(i, 0));
        }

        // Update some elements.
        for i in 8000..9000 {
            assert!(map.update(&get_key(i), (&mut get_value(i, 1)).into(), 0).is_ok());
        }
        for i in 8000..9000 {
            assert_eq!(&map.load(&get_key(i)).unwrap()[..], &get_value(i, 1));
        }

        // Delete half of the entries.
        for i in 5000..10000 {
            assert!(map.delete(&get_key(i)).is_ok());
        }
        for i in 5000..10000 {
            assert_eq!(map.load(&get_key(i)), None);
        }

        // Replace removed entries with new ones
        for i in 10000..15000 {
            assert!(map.update(&get_key(i), (&mut get_value(i, 2)).into(), 0).is_ok());
        }

        for i in 0..5000 {
            assert_eq!(&map.load(&get_key(i)).unwrap()[..], &get_value(i, 0));
        }
        for i in 10000..15000 {
            assert_eq!(&map.load(&get_key(i)).unwrap()[..], &get_value(i, 2));
        }
    }

    #[fuchsia::test]
    fn test_hash_map_overflow() {
        let schema = MapSchema {
            map_type: bpf_map_type_BPF_MAP_TYPE_HASH,
            key_size: 8,
            value_size: u32::MAX,
            max_entries: u32::MAX,
            flags: MapFlags::empty(),
        };
        assert_eq!(Map::new(schema, "test").err(), Some(MapError::InvalidParam));
    }

    #[fuchsia::test]
    fn test_lpm_trie_overflow() {
        let schema = MapSchema {
            map_type: bpf_map_type_BPF_MAP_TYPE_LPM_TRIE,
            key_size: 8,
            value_size: u32::MAX,
            max_entries: u32::MAX,
            flags: MapFlags::NoPrealloc,
        };
        assert_eq!(Map::new(schema, "test").err(), Some(MapError::InvalidParam));
    }

    #[fuchsia::test]
    fn test_lpm_trie_invalid_key_size() {
        let make_schema = |key_size| MapSchema {
            map_type: bpf_map_type_BPF_MAP_TYPE_LPM_TRIE,
            key_size,
            value_size: 4,
            max_entries: 10,
            flags: MapFlags::NoPrealloc,
        };

        // Key size must be at least 5 bytes
        assert_eq!(Map::new(make_schema(4), "test").err(), Some(MapError::InvalidParam));
        assert!(Map::new(make_schema(5), "test").is_ok());

        // Key size must be at most 260 bytes
        assert!(Map::new(make_schema(260), "test").is_ok());
        assert_eq!(Map::new(make_schema(261), "test").err(), Some(MapError::InvalidParam));
    }

    #[fuchsia::test]
    fn test_hash_map_update_direct() {
        let schema = MapSchema {
            map_type: bpf_map_type_BPF_MAP_TYPE_HASH,
            key_size: 5,
            value_size: 11,
            max_entries: 10,
            flags: MapFlags::empty(),
        };

        let map = Map::new(schema, "test").unwrap();
        let key = MapKey::from_vec("12345".to_string().into_bytes());
        let mut value = (0..11).collect::<Vec<u8>>();
        assert!(map.update(&key.clone(), (&mut value).into(), 0).is_ok());

        // Access a value directly the way eBPF programs do.
        let value_ref = map.lookup(&key).unwrap();
        value_ref.ptr().slice(0..4).unwrap().store(&[0xae, 0xad, 0xac, 0xab]);

        assert_eq!(&map.load(&key).unwrap()[..], &[0xae, 0xad, 0xac, 0xab, 4, 5, 6, 7, 8, 9, 10]);
    }

    #[fuchsia::test]
    fn test_hash_map_ref_counting() {
        let schema = MapSchema {
            map_type: bpf_map_type_BPF_MAP_TYPE_HASH,
            key_size: 5,
            value_size: 11,
            max_entries: 2,
            flags: MapFlags::empty(),
        };

        let map = Map::new(schema, "test").unwrap();
        let key = MapKey::from_vec("12345".to_string().into_bytes());
        let key2 = MapKey::from_vec("24122".to_string().into_bytes());
        let mut value = (0..11).collect::<Vec<u8>>();
        assert!(map.update(&key.clone(), (&mut value).into(), 0).is_ok());
        assert!(map.update(&key2.clone(), (&mut value).into(), 0).is_ok());

        let value_ref = map.lookup(&key).unwrap();

        // Delete an element. The corresponding data entry should not be
        // released until `value_ref` is dropped.
        assert!(map.delete(&key).is_ok());
        assert_eq!(map.update(&key.clone(), (&mut value).into(), 0), Err(MapError::SizeLimit));
        drop(value_ref);
        assert!(map.update(&key.clone(), (&mut value).into(), 0).is_ok());
    }

    #[fuchsia::test]
    fn test_ringbug_sharing() {
        let schema = MapSchema {
            map_type: bpf_map_type_BPF_MAP_TYPE_RINGBUF,
            key_size: 0,
            value_size: 0,
            max_entries: 4096 * 2,
            flags: MapFlags::empty(),
        };

        let map = Map::new(schema, "test").unwrap();
        map.ringbuf_reserve(8000, 0).expect("ringbuf_reserve failed");

        let map2 = Map::new_shared(map.share().unwrap()).unwrap();

        // Expected to fail since there is no space left.
        map2.ringbuf_reserve(2000, 0).expect_err("ringbuf_reserve expected to fail");
    }

    // Verifies that all supported map types are shareable.
    #[fuchsia::test]
    fn test_all_maps_shareable() {
        for map_type in 1..linux_uapi::bpf_map_type___MAX_BPF_MAP_TYPE {
            let (key_size, value_size, max_entries, flags) = match map_type {
                bpf_map_type_BPF_MAP_TYPE_RINGBUF => (0, 0, 4096, MapFlags::empty()),
                bpf_map_type_BPF_MAP_TYPE_LPM_TRIE => (8, 4, 4096, MapFlags::NoPrealloc),
                bpf_map_type_BPF_MAP_TYPE_SK_STORAGE => (4, 4, 0, MapFlags::NoPrealloc),
                _ => (4, 4, 1, MapFlags::empty()),
            };
            let schema = MapSchema { map_type, key_size, value_size, max_entries, flags };

            let map = match Map::new(schema, "test") {
                Ok(map) => map,
                Err(MapError::MapTypeNotSupported) => {
                    continue;
                }
                Err(e) => {
                    panic!("Failed to create map of type {:?}: {:?}", map_type, e);
                }
            };

            let map_fidl = map.share().expect("Failed to share map");
            let _: PinnedMap = Map::new_shared(map_fidl).expect("Failed to initialize shared map");
        }
    }
}

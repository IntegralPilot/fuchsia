// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::SecurityId;
use crate::access_vector_cache::{
    AccessQueryArgs, KernelXpermsAccessDecision, XpermsAccessQueryArgs,
};
use crate::concurrent_cache::{LockFreeQueryCache, StorageStrategy};
use crate::kernel_permissions::ClassPermission;
use crate::policy::{KernelAccessDecision, XpermsBitmap, XpermsKind};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU8, AtomicU16, AtomicU32, AtomicU64, Ordering};
use zerocopy::IntoBytes;

/// Cache for access decisions.
/// This cache has 4 slots per bucket, with 25 bytes of inline storage. A bucket is 64 bytes.
pub type ConcurrentAccessCache = LockFreeQueryCache<
    AccessCacheStorage,
    /*ways4*/ 1,
    /*u64*/ 3,
    /*u32*/ 0,
    /*u16*/ 0,
    /*u8*/ 1,
    /*out_of_line_u64s*/ 0,
>;

/// Cache for extended access decisions.
/// This cache has 4 slots per bucket, with 11 bytes of inline storage and 256 bytes of out-of-line
/// storage. A bucket is 64 bytes.
pub(super) type ConcurrentXpermsCache = LockFreeQueryCache<
    XpermsAccessCacheStorage,
    /*ways4*/ 1,
    /*u64*/ 1,
    /*u32*/ 0,
    /*u16*/ 1,
    /*u8*/ 1,
    /*out_of_line_u64s*/ 8,
>;

/// Cache for computed SIDs.
/// This cache has 8 slots per bucket, with 13 bytes of inline storage. A bucket is 128 bytes.
pub(super) type ConcurrentSidCache = LockFreeQueryCache<
    SidCacheStorage,
    /*ways4*/ 2,
    /*u64*/ 1,
    /*u32*/ 1,
    /*u16*/ 0,
    /*u8*/ 1,
    /*out_of_line_u64s*/ 0,
>;

#[derive(Default)]
pub struct AccessCacheStorage;

/// Storage for an access vector cache entry. We store the two sids (4 bytes each) in a u64, the
/// allow and audit AccessVectors in another u64, and the class in a u8.
impl
    StorageStrategy<
        /*u64*/ 3,
        /*u32*/ 0,
        /*u16*/ 0,
        /*u8*/ 1,
        /*out_of_line_u64s*/ 0,
    > for AccessCacheStorage
{
    type Key = AccessQueryArgs;
    type Value = KernelAccessDecision;

    #[inline(always)]
    fn hash_key(&self, key: &Self::Key) -> u64 {
        rapidhash::rapidhash(key.as_bytes())
    }

    #[inline(always)]
    fn check_key(
        &self,
        key: &Self::Key,
        inline_u64s: &[AtomicU64; 3],
        _inline_u32s: &[AtomicU32; 0],
        _inline_u16s: &[AtomicU16; 0],
        inline_u8s: &[AtomicU8; 1],
        _out_of_line_u64s: &[AtomicU64; 0],
    ) -> bool {
        inline_u8s[0].load(Ordering::Relaxed) == key.target_class as u8
            && inline_u64s[0].load(Ordering::Relaxed)
                == (key.source_sid.0.get() as u64 | (key.target_sid.0.get() as u64) << 32)
    }

    #[inline(always)]
    fn read_value(
        &self,
        inline_u64s: &[AtomicU64; 3],
        _inline_u32s: &[AtomicU32; 0],
        _inline_u16s: &[AtomicU16; 0],
        _inline_u8s: &[AtomicU8; 1],
        _out_of_line_u64s: &[AtomicU64; 0],
    ) -> Self::Value {
        let u64_1 = inline_u64s[1].load(Ordering::Relaxed);
        let u64_2 = inline_u64s[2].load(Ordering::Relaxed);

        let allow_u32 = (u64_1 >> 32) as u32;
        let audit_u32 = (u64_1 & 0xFFFFFFFF) as u32;
        let flags = (u64_2 >> 32) as u32;
        let todo_u64 = u64_2 & 0xFFFFFFFF;

        KernelAccessDecision {
            allow: allow_u32.into(),
            audit: audit_u32.into(),
            flags,
            todo_bug: if todo_u64 == 0 {
                None
            } else {
                Some(std::num::NonZeroU32::new(todo_u64 as u32).unwrap())
            },
        }
    }

    #[inline(always)]
    fn write_key_value(
        &self,
        key: &Self::Key,
        value: &Self::Value,
        inline_u64s: &[AtomicU64; 3],
        _inline_u32s: &[AtomicU32; 0],
        _inline_u16s: &[AtomicU16; 0],
        inline_u8s: &[AtomicU8; 1],
        _out_of_line_u64s: &[AtomicU64; 0],
    ) {
        let source_sid = key.source_sid.0.get() as u64;
        let target_sid = key.target_sid.0.get() as u64;
        let target_class = key.target_class.clone() as u8;

        let allow_u32: u32 = value.allow.into();
        let allow = allow_u32 as u64;
        let audit_u32: u32 = value.audit.into();
        let audit = audit_u32 as u64;
        let flags = value.flags as u64;
        let todo_bug = match value.todo_bug {
            Some(n) => n.get() as u64,
            None => 0,
        };

        let u64_0 = source_sid | (target_sid << 32);
        let u64_1 = audit | (allow << 32);
        let u64_2 = todo_bug | (flags << 32);

        inline_u64s[0].store(u64_0, Ordering::Relaxed);
        inline_u64s[1].store(u64_1, Ordering::Relaxed);
        inline_u64s[2].store(u64_2, Ordering::Relaxed);
        inline_u8s[0].store(target_class, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub(super) struct XpermsAccessCacheStorage;

impl XpermsAccessCacheStorage {
    const PERMISSION_ID_MASK: u8 = 0b0011_1111;
    const XPERMS_KIND_BIT_INDEX: usize = 5;
    const PERMISSIVE_BIT_INDEX: usize = 6;
    const HAS_TODO_BIT_INDEX: usize = 7;
}

/// Xperms storage: we store the two sids (4 bytes each) in a u64, the class and xperms_prefix in a
/// u16, and we pack the permission, xperms_kind and 2 bits of flags in an u8. The xperm bitmaps
/// (64 bytes in total) are stored out of line.
impl
    StorageStrategy<
        /*u64*/ 1,
        /*u32*/ 0,
        /*u16*/ 1,
        /*u8*/ 1,
        /*out_of_line_u64s*/ 8,
    > for XpermsAccessCacheStorage
{
    type Key = XpermsAccessQueryArgs;
    type Value = KernelXpermsAccessDecision;

    #[inline(always)]
    fn hash_key(&self, key: &Self::Key) -> u64 {
        let mut hasher = rapidhash::RapidInlineHasher::default();
        key.hash(&mut hasher);
        hasher.finish()
    }

    #[inline(always)]
    fn check_key(
        &self,
        key: &Self::Key,
        inline_u64s: &[AtomicU64; 1],
        _inline_u32s: &[AtomicU32; 0],
        inline_u16s: &[AtomicU16; 1],
        inline_u8s: &[AtomicU8; 1],
        _out_of_line_u64s: &[AtomicU64; 8],
    ) -> bool {
        let source_sid = key.source_sid.0.get() as u64;
        let target_sid = key.target_sid.0.get() as u64;
        let class = key.permission.class() as u16;
        let xperms_prefix = key.xperms_prefix as u16;
        let permission_id = key.permission.id() as u8;
        let xperms_kind_bit = match key.xperms_kind {
            XpermsKind::Ioctl => 0,
            XpermsKind::Nlmsg => 1,
        };

        let u64_0_matches =
            inline_u64s[0].load(Ordering::Relaxed) == (source_sid | (target_sid << 32));
        let u16_0_matches =
            inline_u16s[0].load(Ordering::Relaxed) == (class | (xperms_prefix << 8));
        let u8_0_val = inline_u8s[0].load(Ordering::Relaxed);
        let u8_0_matches = (u8_0_val
            & (Self::PERMISSION_ID_MASK | (1u8 << Self::XPERMS_KIND_BIT_INDEX)))
            == (permission_id | (xperms_kind_bit << Self::XPERMS_KIND_BIT_INDEX));

        u64_0_matches && u16_0_matches && u8_0_matches
    }

    #[inline(always)]
    fn read_value(
        &self,
        _inline_u64s: &[AtomicU64; 1],
        _inline_u32s: &[AtomicU32; 0],
        _inline_u16s: &[AtomicU16; 1],
        inline_u8s: &[AtomicU8; 1],
        out_of_line_u64s: &[AtomicU64; 8],
    ) -> Self::Value {
        let u8_0 = inline_u8s[0].load(Ordering::Relaxed);
        let permissive = (u8_0 & (1u8 << Self::PERMISSIVE_BIT_INDEX)) != 0;
        let has_todo = (u8_0 & (1u8 << Self::HAS_TODO_BIT_INDEX)) != 0;

        let (chunks, _) = out_of_line_u64s.as_chunks::<{ XpermsBitmap::BITMAP_BLOCKS }>();
        let allow = XpermsBitmap::from_atomics(&chunks[0]);
        let audit = XpermsBitmap::from_atomics(&chunks[1]);

        KernelXpermsAccessDecision { allow, audit, permissive, has_todo }
    }

    #[inline(always)]
    fn write_key_value(
        &self,
        key: &Self::Key,
        value: &Self::Value,
        inline_u64s: &[AtomicU64; 1],
        _inline_u32s: &[AtomicU32; 0],
        inline_u16s: &[AtomicU16; 1],
        inline_u8s: &[AtomicU8; 1],
        out_of_line_u64s: &[AtomicU64; 8],
    ) {
        let source_sid = key.source_sid.0.get() as u64;
        let target_sid = key.target_sid.0.get() as u64;
        let class = key.permission.class() as u16;
        let xperms_prefix = key.xperms_prefix as u16;
        let permission_id = key.permission.id() as u8;
        let xperms_kind_bit = match key.xperms_kind {
            XpermsKind::Ioctl => 0,
            XpermsKind::Nlmsg => 1,
        };

        let u64_0 = source_sid | (target_sid << 32);
        let u16_0 = class | (xperms_prefix << 8);
        let u8_0 = permission_id
            | (xperms_kind_bit << Self::XPERMS_KIND_BIT_INDEX)
            | ((value.permissive as u8) << Self::PERMISSIVE_BIT_INDEX)
            | ((value.has_todo as u8) << Self::HAS_TODO_BIT_INDEX);

        inline_u64s[0].store(u64_0, Ordering::Relaxed);
        inline_u16s[0].store(u16_0, Ordering::Relaxed);
        inline_u8s[0].store(u8_0, Ordering::Relaxed);

        let (chunks, _) = out_of_line_u64s.as_chunks::<{ XpermsBitmap::BITMAP_BLOCKS }>();
        value.allow.to_atomics(&chunks[0]);
        value.audit.to_atomics(&chunks[1]);
    }
}

#[derive(Default)]
pub(super) struct SidCacheStorage;

/// Storage for a SID cache entry. We store the two sids (4 bytes each) in a u64, the class in a
/// u8, and the resulting SID in an u32.
impl
    StorageStrategy<
        /*u64*/ 1,
        /*u32*/ 1,
        /*u16*/ 0,
        /*u8*/ 1,
        /*out_of_line_u64s*/ 0,
    > for SidCacheStorage
{
    type Key = AccessQueryArgs;
    type Value = SecurityId;

    #[inline(always)]
    fn hash_key(&self, key: &Self::Key) -> u64 {
        rapidhash::rapidhash(key.as_bytes())
    }

    #[inline(always)]
    fn check_key(
        &self,
        key: &Self::Key,
        inline_u64s: &[AtomicU64; 1],
        _inline_u32s: &[AtomicU32; 1],
        _inline_u16s: &[AtomicU16; 0],
        inline_u8s: &[AtomicU8; 1],
        _out_of_line_u64s: &[AtomicU64; 0],
    ) -> bool {
        inline_u8s[0].load(Ordering::Relaxed) == key.target_class as u8
            && inline_u64s[0].load(Ordering::Relaxed)
                == (key.source_sid.0.get() as u64 | (key.target_sid.0.get() as u64) << 32)
    }

    #[inline(always)]
    fn read_value(
        &self,
        _inline_u64s: &[AtomicU64; 1],
        inline_u32s: &[AtomicU32; 1],
        _inline_u16s: &[AtomicU16; 0],
        _inline_u8s: &[AtomicU8; 1],
        _out_of_line_u64s: &[AtomicU64; 0],
    ) -> Self::Value {
        let u32_val = inline_u32s[0].load(Ordering::Relaxed);
        SecurityId(std::num::NonZeroU32::new(u32_val).unwrap())
    }

    #[inline(always)]
    fn write_key_value(
        &self,
        key: &Self::Key,
        value: &Self::Value,
        inline_u64s: &[AtomicU64; 1],
        inline_u32s: &[AtomicU32; 1],
        _inline_u16s: &[AtomicU16; 0],
        inline_u8s: &[AtomicU8; 1],
        _out_of_line_u64s: &[AtomicU64; 0],
    ) {
        let source_sid = key.source_sid.0.get() as u64;
        let target_sid = key.target_sid.0.get() as u64;
        let target_class = key.target_class.clone() as u8;
        let value_sid = value.0.get() as u32;

        let u64_0 = source_sid | (target_sid << 32);

        inline_u64s[0].store(u64_0, Ordering::Relaxed);
        inline_u32s[0].store(value_sid, Ordering::Relaxed);
        inline_u8s[0].store(target_class, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel_permissions::{DirPermission, KernelClass, KernelPermission};
    use crate::policy::{AccessVector, XpermsBitmap};

    #[test]
    fn test_access_cache_storage_roundtrip() {
        let key = AccessQueryArgs {
            source_sid: SecurityId(1.try_into().unwrap()),
            target_sid: SecurityId(2.try_into().unwrap()),
            target_class: KernelClass::File,
        };
        let value = KernelAccessDecision {
            allow: AccessVector::from(4),
            audit: AccessVector::from(5),
            flags: 42,
            todo_bug: Some(12345.try_into().unwrap()),
        };

        let inline_u64s = std::array::from_fn(|_| AtomicU64::new(0));
        let inline_u32s = std::array::from_fn(|_| AtomicU32::new(0));
        let inline_u16s = std::array::from_fn(|_| AtomicU16::new(0));
        let inline_u8s = std::array::from_fn(|_| AtomicU8::new(0));
        let out_of_line_u64s = std::array::from_fn(|_| AtomicU64::new(0));

        AccessCacheStorage::default().write_key_value(
            &key,
            &value,
            &inline_u64s,
            &inline_u32s,
            &inline_u16s,
            &inline_u8s,
            &out_of_line_u64s,
        );

        assert!(AccessCacheStorage::default().check_key(
            &key,
            &inline_u64s,
            &inline_u32s,
            &inline_u16s,
            &inline_u8s,
            &out_of_line_u64s,
        ));

        let read_val = AccessCacheStorage::default().read_value(
            &inline_u64s,
            &inline_u32s,
            &inline_u16s,
            &inline_u8s,
            &out_of_line_u64s,
        );

        assert_eq!(read_val, value);
    }

    #[test]
    fn test_xperms_access_cache_storage_roundtrip() {
        let key = XpermsAccessQueryArgs {
            xperms_kind: XpermsKind::Ioctl,
            source_sid: SecurityId(1.try_into().unwrap()),
            target_sid: SecurityId(2.try_into().unwrap()),
            permission: KernelPermission::Dir(DirPermission::AddName),
            xperms_prefix: 0,
        };
        let value = KernelXpermsAccessDecision {
            allow: XpermsBitmap::NONE,
            audit: XpermsBitmap::NONE,
            permissive: false,
            has_todo: true,
        };

        let inline_u64s = std::array::from_fn(|_| AtomicU64::new(0));
        let inline_u32s = std::array::from_fn(|_| AtomicU32::new(0));
        let inline_u16s = std::array::from_fn(|_| AtomicU16::new(0));
        let inline_u8s = std::array::from_fn(|_| AtomicU8::new(0));
        let out_of_line_u64s = std::array::from_fn(|_| AtomicU64::new(0));

        XpermsAccessCacheStorage::default().write_key_value(
            &key,
            &value,
            &inline_u64s,
            &inline_u32s,
            &inline_u16s,
            &inline_u8s,
            &out_of_line_u64s,
        );

        assert!(XpermsAccessCacheStorage::default().check_key(
            &key,
            &inline_u64s,
            &inline_u32s,
            &inline_u16s,
            &inline_u8s,
            &out_of_line_u64s,
        ));

        let read_val = XpermsAccessCacheStorage::default().read_value(
            &inline_u64s,
            &[],
            &inline_u16s,
            &inline_u8s,
            &out_of_line_u64s,
        );

        assert_eq!(read_val, value);
    }

    #[test]
    fn test_sid_cache_storage_roundtrip() {
        let key = AccessQueryArgs {
            source_sid: SecurityId(1.try_into().unwrap()),
            target_sid: SecurityId(2.try_into().unwrap()),
            target_class: KernelClass::Process,
        };
        let value = SecurityId(3.try_into().unwrap());

        let inline_u64s = std::array::from_fn(|_| AtomicU64::new(0));
        let inline_u32s = std::array::from_fn(|_| AtomicU32::new(0));
        let inline_u16s = std::array::from_fn(|_| AtomicU16::new(0));
        let inline_u8s = std::array::from_fn(|_| AtomicU8::new(0));
        let out_of_line_u64s = std::array::from_fn(|_| AtomicU64::new(0));

        SidCacheStorage::default().write_key_value(
            &key,
            &value,
            &inline_u64s,
            &inline_u32s,
            &inline_u16s,
            &inline_u8s,
            &out_of_line_u64s,
        );

        assert!(SidCacheStorage::default().check_key(
            &key,
            &inline_u64s,
            &inline_u32s,
            &inline_u16s,
            &inline_u8s,
            &out_of_line_u64s,
        ));

        let read_val = SidCacheStorage::default().read_value(
            &inline_u64s,
            &inline_u32s,
            &inline_u16s,
            &inline_u8s,
            &out_of_line_u64s,
        );

        assert_eq!(read_val, value);
    }

    #[test]
    fn test_access_cache_bucket_size() {
        // The access cache packs 4 entries in 128 bytes.
        assert_eq!(ConcurrentAccessCache::bucket_size(), 128);
    }

    #[test]
    fn test_xperms_cache_bucket_size() {
        // The xperms cache packs 4 entries in 64 bytes (and stores the rest out-of-line).
        assert_eq!(ConcurrentXpermsCache::bucket_size(), 64);
    }

    #[test]
    fn test_sid_cache_bucket_size() {
        // The SID cache packs 8 entries in 128 bytes.
        assert_eq!(ConcurrentSidCache::bucket_size(), 128);
    }
}

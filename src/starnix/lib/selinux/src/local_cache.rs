// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::permission_check::PermissionCheckResult;
use crate::policy::{AccessVector, KernelAccessDecision, XpermsKind};
use crate::{ClassPermission, KernelClass, KernelPermission, PolicySeqNo, SecurityId};
use std::cell::Cell;

/// A simple allow decision (allowed, not audited, not permissive, no TODO bug).
const SIMPLE_ALLOW: PermissionCheckResult =
    PermissionCheckResult { granted: true, audit: false, permissive: false, todo_bug: None };

/// Sizes for the different caches. These have been experimentally determined.
const FD_USE_CACHE_SIZE: usize = 4;
const ACCESS_CACHE_SIZE: usize = 8;
const XPERM_CACHE_SIZE: usize = 2;

/// Each nibble in LruState stores the index of the entry in that cache position, from most recent
/// (least significant nibble) to least recent (most significant nibble).
#[derive(Clone, Copy, Debug)]
struct LruState<const ENTRIES: usize>(u32);

impl<const ENTRIES: usize> LruState<ENTRIES> {
    fn new() -> Self {
        const {
            assert!(ENTRIES > 0 && ENTRIES <= 8);
        }
        let mask = if ENTRIES >= 8 { u32::MAX } else { (1_u32 << (ENTRIES * 4)) - 1 };
        Self(0x76543210 & mask)
    }

    /// Finds an entry to evict and put it back as most recently used.
    fn evict(&mut self) -> usize {
        // We evict the least recently used entry, which is the one at the highest index.
        let shift = (ENTRIES - 1) * 4;
        let evicted = (self.0 >> shift) & 0xF;
        // The evicted entry is now first, all other entries are shifted down by one position.
        self.0 = (self.0 << 4) | evicted;
        evicted as usize
    }

    /// Moves the entry from position `hit_idx` to most recently used (index 0).
    fn touch_mru_idx(&mut self, hit_idx: usize) {
        if hit_idx >= ENTRIES || hit_idx == 0 {
            return;
        }
        let pos = hit_idx * 4;
        let val = (self.0 >> pos) & 0xF; // The nibble to move to index 0.

        // 1. Get the bits below the nibble at `pos`. These represent indices 0 to hit_idx - 1.
        //    These bits need to be shifted left by 4.
        let lower_mask = (1_u32 << pos) - 1;
        let lower_part = self.0 & lower_mask;
        let shifted_lower = lower_part << 4;

        // 2. Get the bits above the nibble at `pos`. These represent indices hit_idx + 1 to entries - 1.
        //    These bits remain in place relative to each other.
        //    Pre-apply the shift of 4 to avoid an illegal shift of 32 when pos == 28 (hit_idx == 7).
        let upper_mask = (!0_u32 << 4) << pos;
        let upper_part = self.0 & upper_mask;

        // 3. Combine the parts: [upper_part] | [shifted_lower] | [val]
        self.0 = upper_part | shifted_lower | val;
    }
}

/// Packs two SecurityIds into a u64 for efficient aligned comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SidPair(u64);

impl SidPair {
    fn new(source: SecurityId, target: SecurityId) -> Self {
        Self((source.0.get() as u64) << 32 | (target.0.get() as u64))
    }
    const NONE: Self = Self(0);
}

impl Default for SidPair {
    fn default() -> Self {
        Self::NONE
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct XpermCacheKey {
    sids: SidPair,
    // Packs (kind as u8, permission.class() as u8, permission.id() as u8, xperm as u16)
    details: u64,
}

impl XpermCacheKey {
    fn new(
        source_sid: SecurityId,
        target_sid: SecurityId,
        kind: XpermsKind,
        permission: &KernelPermission,
        xperm: u16,
    ) -> Self {
        let kind_num = match kind {
            XpermsKind::Ioctl => 0u64,
            XpermsKind::Nlmsg => 1u64,
        };
        let class_num = permission.class() as u64;
        let id_num = permission.id() as u64;
        let xperm_num = xperm as u64;
        let details = (kind_num << 40) | (class_num << 32) | (id_num << 16) | xperm_num;
        Self { sids: SidPair::new(source_sid, target_sid), details }
    }
}

/// Per-thread cache for SELinux policy decisions.
#[derive(Debug)]
pub struct PerThreadCache {
    /// The policy version for which this cache is valid.
    policy_seqno: Cell<PolicySeqNo>,
    /// fd_use cache. Stores "simple allow" decisions (allowed, not audited, not permissive, no TODO bug).
    fd_use_cache: [Cell<SidPair>; FD_USE_CACHE_SIZE],
    fd_use_lru: Cell<LruState<FD_USE_CACHE_SIZE>>,
    /// Access query cache. This only stores "simple" (non-permissive, non-TODO) access decisions.
    /// Decomposed as a structure-of-array for efficient packing.
    access_cache_sid_idx: [Cell<SidPair>; ACCESS_CACHE_SIZE],
    access_cache_class_idx: [Cell<KernelClass>; ACCESS_CACHE_SIZE],
    access_cache_result: [Cell<(AccessVector, AccessVector)>; ACCESS_CACHE_SIZE],
    access_lru: Cell<LruState<ACCESS_CACHE_SIZE>>,
    /// Xperm query cache. This cache is indexed by the exact extended permission required.
    xperm_cache: [Cell<XpermCacheKey>; XPERM_CACHE_SIZE],
    xperm_lru: Cell<LruState<XPERM_CACHE_SIZE>>,
}

impl Default for PerThreadCache {
    fn default() -> Self {
        Self {
            policy_seqno: Cell::new(PolicySeqNo::INITIAL),
            fd_use_cache: std::array::from_fn(|_| Cell::new(SidPair::NONE)),
            fd_use_lru: Cell::new(LruState::new()),
            access_cache_sid_idx: std::array::from_fn(|_| Cell::new(SidPair::NONE)),
            access_cache_class_idx: std::array::from_fn(|_| Cell::new(KernelClass::File)),
            access_cache_result: std::array::from_fn(|_| {
                Cell::new((AccessVector::NONE, AccessVector::NONE))
            }),
            access_lru: Cell::new(LruState::new()),
            xperm_cache: std::array::from_fn(|_| Cell::new(XpermCacheKey::default())),
            xperm_lru: Cell::new(LruState::new()),
        }
    }
}

impl PerThreadCache {
    fn reset(&self) {
        self.fd_use_cache.iter().for_each(|c| c.set(SidPair::NONE));
        self.fd_use_lru.set(LruState::new());
        self.access_cache_sid_idx.iter().for_each(|c| c.set(SidPair::NONE));
        self.access_lru.set(LruState::new());
        self.xperm_cache.iter().for_each(|c| c.set(XpermCacheKey::default()));
        self.xperm_lru.set(LruState::new());
    }

    /// Checks whether the policy version has changed since the last time the cache was accessed, and
    /// resets the cache in this case.
    fn check_policy_version(&self, policy_seqno: PolicySeqNo) {
        if self.policy_seqno.get() != policy_seqno {
            self.reset();
            self.policy_seqno.set(policy_seqno);
        }
    }

    /// Looks up a fd use decision in cache, or falls back to using `compute`.
    #[inline]
    pub fn lookup_fd_use<F>(
        &self,
        policy_seqno: PolicySeqNo,
        source_sid: SecurityId,
        target_sid: SecurityId,
        compute: F,
    ) -> PermissionCheckResult
    where
        F: FnOnce() -> PermissionCheckResult,
    {
        self.check_policy_version(policy_seqno);

        let key = SidPair::new(source_sid, target_sid);
        let mut lru = self.fd_use_lru.get();
        let mut sequence = lru.0;
        for hit_idx in 0..FD_USE_CACHE_SIZE {
            let i = (sequence & 0xF) as usize;
            if self.fd_use_cache[i].get() == key {
                if hit_idx != 0 {
                    lru.touch_mru_idx(hit_idx);
                    self.fd_use_lru.set(lru);
                }
                return SIMPLE_ALLOW;
            }
            sequence >>= 4;
        }
        let result = compute();
        // Only cache simple "allow" decisions. This keeps the cache smaller and focuses on the
        // most common case.
        if result == SIMPLE_ALLOW {
            let evicted = lru.evict();
            self.fd_use_lru.set(lru);
            self.fd_use_cache[evicted].set(key);
        }
        result
    }

    /// Looks up an xperms access decision in cache, or falls back to calling `compute`.
    #[inline]
    pub(crate) fn check_xperm<F>(
        &self,
        policy_seqno: PolicySeqNo,
        kind: XpermsKind,
        source_sid: SecurityId,
        target_sid: SecurityId,
        permission: KernelPermission,
        xperm: u16,
        compute: F,
    ) -> PermissionCheckResult
    where
        F: FnOnce() -> PermissionCheckResult,
    {
        self.check_policy_version(policy_seqno);
        let key = XpermCacheKey::new(source_sid, target_sid, kind, &permission, xperm);
        let mut lru = self.xperm_lru.get();
        let mut sequence = lru.0;
        for hit_idx in 0..XPERM_CACHE_SIZE {
            let i = (sequence & 0xF) as usize;
            if self.xperm_cache[i].get() == key {
                if hit_idx != 0 {
                    lru.touch_mru_idx(hit_idx);
                    self.xperm_lru.set(lru);
                }
                return SIMPLE_ALLOW;
            }
            sequence >>= 4;
        }
        let result = compute();
        // Only cache simple "allow" decisions. This keeps the cache smaller and focuses on the
        // most common case.
        if result == SIMPLE_ALLOW {
            let evicted = lru.evict();
            self.xperm_lru.set(lru);
            self.xperm_cache[evicted].set(key);
        }
        result
    }

    /// Looks up an access decision in cache, or falls back to calling `compute`. This caches the
    /// whole access vector instead of individual permissions so that multiple checks for different
    /// permissions on the same (source, target, class) triple can make use of the cache.
    #[inline]
    pub(crate) fn lookup_access_decision<F>(
        &self,
        policy_seqno: PolicySeqNo,
        source_sid: SecurityId,
        target_sid: SecurityId,
        class: KernelClass,
        compute: F,
    ) -> KernelAccessDecision
    where
        F: FnOnce() -> KernelAccessDecision,
    {
        self.check_policy_version(policy_seqno);
        let key = SidPair::new(source_sid, target_sid);
        let mut lru = self.access_lru.get();
        let mut sequence = lru.0;
        for hit_idx in 0..ACCESS_CACHE_SIZE {
            let i = (sequence & 0xF) as usize;
            if key == self.access_cache_sid_idx[i].get()
                && class == self.access_cache_class_idx[i].get()
            {
                if hit_idx != 0 {
                    lru.touch_mru_idx(hit_idx);
                    self.access_lru.set(lru);
                }
                let (allow, audit) = self.access_cache_result[i].get();
                return KernelAccessDecision { allow, audit, flags: 0, todo_bug: None };
            }
            sequence >>= 4;
        }

        let result = compute();

        // Only cache decisions that do not have an associated todo bug or flags. This keeps the
        // cache smaller and focused on the common case.
        if result.todo_bug.is_none() && result.flags == 0 {
            let evicted = lru.evict();
            self.access_lru.set(lru);
            self.access_cache_sid_idx[evicted].set(key);
            self.access_cache_class_idx[evicted].set(class);
            self.access_cache_result[evicted].set((result.allow, result.audit));
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FilePermission;

    #[test]
    fn test_lru_state() {
        let mut lru = LruState::<4>::new();
        assert_eq!(lru.0, 0x3210);

        // Touch 2 at index 2 - it moves to front.
        lru.touch_mru_idx(2);
        assert_eq!(lru.0, 0x3102);

        // Evict LRU.
        assert_eq!(lru.evict(), 3);
        assert_eq!(lru.0 & 0xFFFF, 0x1023);

        // Touch 0 at index 2.
        lru.touch_mru_idx(2);
        assert_eq!(lru.0 & 0xFFFF, 0x1230);

        // Evict LRU.
        assert_eq!(lru.evict(), 1);
        assert_eq!(lru.0 & 0xFFFF, 0x2301);
    }

    #[test]
    fn test_touch_mru_idx_max_entries() {
        let mut lru = LruState::<8>::new();
        // Touch index 7 (last entry).
        lru.touch_mru_idx(7);
        // Verify no panic and state is updated.
        // Initial state for 8 entries is 0x76543210.
        // Touching 7 (at pos 28) means moving nibble 7 to front.
        assert_eq!(lru.0, 0x65432107);
    }

    #[test]
    fn test_cache_lookup_hit() {
        let cache = PerThreadCache::default();
        let sid1 = SecurityId(1.try_into().unwrap());
        let sid2 = SecurityId(2.try_into().unwrap());

        // First lookup: miss, calls compute.
        let mut compute_called = false;
        let result = cache.lookup_fd_use(PolicySeqNo::INITIAL, sid1, sid2, || {
            compute_called = true;
            PermissionCheckResult { granted: true, audit: false, permissive: false, todo_bug: None }
        });
        assert!(compute_called);
        assert!(result.granted);

        // Second lookup: hit, does not call compute.
        compute_called = false;
        let result2 = cache.lookup_fd_use(PolicySeqNo::INITIAL, sid1, sid2, || {
            compute_called = true;
            PermissionCheckResult {
                granted: false,
                audit: false,
                permissive: false,
                todo_bug: None,
            }
        });
        assert!(!compute_called);
        assert!(result2.granted);
    }

    #[test]
    fn test_fd_use_cache_invalidation_on_policy_change() {
        let cache = PerThreadCache::default();
        let sid1 = SecurityId(1.try_into().unwrap());
        let sid2 = SecurityId(2.try_into().unwrap());

        // Cache a result with policy change count 0.
        cache.lookup_fd_use(PolicySeqNo::INITIAL, sid1, sid2, || PermissionCheckResult {
            granted: true,
            audit: false,
            permissive: false,
            todo_bug: None,
        });

        // Lookup with policy change count 1: should miss because policy changed.
        let mut compute_called = false;
        let result = cache.lookup_fd_use(PolicySeqNo::OTHER, sid1, sid2, || {
            compute_called = true;
            PermissionCheckResult {
                granted: false,
                audit: false,
                permissive: false,
                todo_bug: None,
            }
        });
        assert!(compute_called);
        assert!(!result.granted);
    }

    #[test]
    fn test_access_cache_lookup() {
        let cache = PerThreadCache::default();
        let sid1 = SecurityId(1.try_into().unwrap());
        let sid2 = SecurityId(2.try_into().unwrap());
        let class = KernelClass::File;

        let mut compute_called = false;
        let result = cache.lookup_access_decision(PolicySeqNo::INITIAL, sid1, sid2, class, || {
            compute_called = true;
            KernelAccessDecision {
                allow: AccessVector::from(1),
                audit: AccessVector::NONE,
                flags: 0,
                todo_bug: None,
            }
        });
        assert!(compute_called);
        assert_eq!(result.allow, AccessVector::from(1));

        compute_called = false;
        let result2 = cache.lookup_access_decision(PolicySeqNo::INITIAL, sid1, sid2, class, || {
            compute_called = true;
            KernelAccessDecision {
                allow: AccessVector::NONE,
                audit: AccessVector::NONE,
                flags: 0,
                todo_bug: None,
            }
        });
        assert!(!compute_called);
        assert_eq!(result2.allow, AccessVector::from(1));
    }

    #[test]
    fn test_access_cache_todo_uncached() {
        let cache = PerThreadCache::default();
        let sid1 = SecurityId(1.try_into().unwrap());
        let sid2 = SecurityId(2.try_into().unwrap());
        let class = KernelClass::File;

        cache.lookup_access_decision(PolicySeqNo::INITIAL, sid1, sid2, class, || {
            KernelAccessDecision {
                allow: AccessVector::from(1),
                audit: AccessVector::NONE,
                flags: 0,
                todo_bug: Some(123.try_into().unwrap()),
            }
        });

        let mut compute_called = false;
        cache.lookup_access_decision(PolicySeqNo::INITIAL, sid1, sid2, class, || {
            compute_called = true;
            KernelAccessDecision {
                allow: AccessVector::NONE,
                audit: AccessVector::NONE,
                flags: 0,
                todo_bug: None,
            }
        });
        assert!(compute_called);
    }

    #[test]
    fn test_access_cache_permissive_uncached() {
        let cache = PerThreadCache::default();
        let sid1 = SecurityId(1.try_into().unwrap());
        let sid2 = SecurityId(2.try_into().unwrap());
        let class = KernelClass::File;

        cache.lookup_access_decision(PolicySeqNo::INITIAL, sid1, sid2, class, || {
            KernelAccessDecision {
                allow: AccessVector::from(1),
                audit: AccessVector::NONE,
                flags: 1,
                todo_bug: None,
            }
        });

        let mut compute_called = false;
        cache.lookup_access_decision(PolicySeqNo::INITIAL, sid1, sid2, class, || {
            compute_called = true;
            KernelAccessDecision {
                allow: AccessVector::NONE,
                audit: AccessVector::NONE,
                flags: 0,
                todo_bug: None,
            }
        });
        assert!(compute_called);
    }

    #[test]
    fn test_xperm_cache_lookup() {
        let cache = PerThreadCache::default();
        let sid1 = SecurityId(1.try_into().unwrap());
        let sid2 = SecurityId(2.try_into().unwrap());
        let permission = KernelPermission::File(FilePermission::Ioctl);

        let mut compute_called = false;
        let result = cache.check_xperm(
            PolicySeqNo::INITIAL,
            XpermsKind::Ioctl,
            sid1,
            sid2,
            permission.clone(),
            1,
            || {
                compute_called = true;
                PermissionCheckResult {
                    granted: true,
                    audit: false,
                    permissive: false,
                    todo_bug: None,
                }
            },
        );
        assert!(compute_called);
        assert!(result.granted);

        compute_called = false;
        let result2 = cache.check_xperm(
            PolicySeqNo::INITIAL,
            XpermsKind::Ioctl,
            sid1,
            sid2,
            permission,
            1,
            || {
                compute_called = true;
                PermissionCheckResult {
                    granted: false,
                    audit: false,
                    permissive: false,
                    todo_bug: None,
                }
            },
        );
        assert!(!compute_called);
        assert!(result2.granted);
    }

    #[test]
    fn test_access_cache_invalidation_on_policy_change() {
        let cache = PerThreadCache::default();
        let sid1 = SecurityId(1.try_into().unwrap());
        let sid2 = SecurityId(2.try_into().unwrap());
        let class = KernelClass::File;

        cache.lookup_access_decision(PolicySeqNo::INITIAL, sid1, sid2, class, || {
            KernelAccessDecision {
                allow: AccessVector::from(1),
                audit: AccessVector::NONE,
                flags: 0,
                todo_bug: None,
            }
        });

        let mut compute_called = false;
        let result = cache.lookup_access_decision(PolicySeqNo::OTHER, sid1, sid2, class, || {
            compute_called = true;
            KernelAccessDecision {
                allow: AccessVector::NONE,
                audit: AccessVector::NONE,
                flags: 0,
                todo_bug: None,
            }
        });
        assert!(compute_called);
        assert_eq!(result.allow, AccessVector::NONE);
    }

    #[test]
    fn test_xperm_cache_invalidation_on_policy_change() {
        let cache = PerThreadCache::default();
        let sid1 = SecurityId(1.try_into().unwrap());
        let sid2 = SecurityId(2.try_into().unwrap());
        let permission = KernelPermission::File(FilePermission::Ioctl);

        cache.check_xperm(
            PolicySeqNo::INITIAL,
            XpermsKind::Ioctl,
            sid1,
            sid2,
            permission.clone(),
            1,
            || PermissionCheckResult {
                granted: true,
                audit: false,
                permissive: false,
                todo_bug: None,
            },
        );

        let mut compute_called = false;
        let result = cache.check_xperm(
            PolicySeqNo::OTHER,
            XpermsKind::Ioctl,
            sid1,
            sid2,
            permission,
            1,
            || {
                compute_called = true;
                PermissionCheckResult {
                    granted: false,
                    audit: false,
                    permissive: false,
                    todo_bug: None,
                }
            },
        );
        assert!(compute_called);
        assert!(!result.granted);
    }
}

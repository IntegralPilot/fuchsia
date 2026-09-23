// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::concurrent_access_cache::{
    ConcurrentAccessCache, ConcurrentSidCache, ConcurrentXpermsCache,
};
use crate::kernel_permissions::KernelPermission;
use crate::policy::{KernelAccessDecision, XpermsBitmap, XpermsKind};
use crate::security_server::SecurityServerBackend;
use crate::{KernelClass, SecurityId};
use std::hash::Hash;
use std::sync::Arc;

pub use crate::cache_stats::CacheStats;

/// An xperm access decision as seen from the kernel.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct KernelXpermsAccessDecision {
    /// The set of xperms that are allowed.
    pub allow: XpermsBitmap,
    /// The set of xperms that should be audited (as allowed or denials depending on `allow`)
    pub audit: XpermsBitmap,
    /// Whether the domain is permissive.
    pub permissive: bool,
    /// Whether the entry has an associated todo.
    pub has_todo: bool,
}

/// Interface used internally by the `SecurityServer` implementation to implement policy queries
/// such as looking up the set of permissions to grant, or the Security Context to apply to new
/// files, etc.
///
/// This trait allows layering of caching, delegation, and thread-safety between the policy-backed
/// calculations, and the caller-facing permission-check interface.
pub(super) trait Query {
    /// Computes the [`AccessDecision`] permitted to `source_sid` for accessing `target_sid`, an
    /// object of type `target_class`.
    fn compute_access_decision(
        &self,
        source_sid: SecurityId,
        target_sid: SecurityId,
        target_class: KernelClass,
    ) -> KernelAccessDecision;

    /// Returns the security identifier (SID) with which to label a new object of `target_class`.
    /// The label is calculated based on the creating `source_sid` and the `target_sid` of the
    /// container (e.g. file-system, parent file node, process, etc) and optional `name`.
    /// Callers pass an empty slice (`&[]`) for `name` to express nameless transitions.
    fn compute_create_sid(
        &self,
        source_sid: SecurityId,
        target_sid: SecurityId,
        target_class: KernelClass,
        name: &[u8],
    ) -> Result<SecurityId, anyhow::Error>;

    /// Computes the [`XpermsAccessDecision`] permitted to `source_sid` for accessing `target_sid`,
    /// an object of type `target_class`, for xperms of kind `xperms_kind` with high byte
    /// `xperms_prefix`.
    fn compute_xperms_access_decision(
        &self,
        xperms_kind: XpermsKind,
        source_sid: SecurityId,
        target_sid: SecurityId,
        permission: KernelPermission,
        xperms_prefix: u8,
    ) -> KernelXpermsAccessDecision;
}

#[derive(Clone, PartialEq, Eq, zerocopy::IntoBytes, zerocopy::Immutable)]
#[repr(C)]
pub struct AccessQueryArgs {
    pub source_sid: SecurityId,
    pub target_sid: SecurityId,
    pub target_class: KernelClass,
}

#[derive(Clone, Hash, PartialEq, Eq)]
pub(super) struct XpermsAccessQueryArgs {
    pub(super) xperms_kind: XpermsKind,
    pub(super) source_sid: SecurityId,
    pub(super) target_sid: SecurityId,
    pub(super) permission: KernelPermission,
    pub(super) xperms_prefix: u8,
}

/// Concurrent set-associative cache with capacity defined at construction and CLOCK eviction.
pub(super) struct FifoQueryCache {
    access_cache: ConcurrentAccessCache,
    create_sid_cache: ConcurrentSidCache,
    xperms_access_cache: ConcurrentXpermsCache,
}

#[derive(Copy, Clone, Debug)]
pub struct QueryCacheCapacity {
    /// Capacities for the different caches. Due to limitations of the cache implementation,
    /// these will be rounded up so the number of buckets is a power of two.
    pub access_cache_capacity: usize,
    pub sid_cache_capacity: usize,
    pub xperms_cache_capacity: usize,
}

impl FifoQueryCache {
    /// Constructs a fixed-size access vector cache.
    pub fn new(capacity: QueryCacheCapacity) -> Self {
        Self {
            access_cache: ConcurrentAccessCache::new(capacity.access_cache_capacity),
            create_sid_cache: ConcurrentSidCache::new(capacity.sid_cache_capacity),
            xperms_access_cache: ConcurrentXpermsCache::new(capacity.xperms_cache_capacity),
        }
    }

    pub fn cache_stats(&self) -> CacheStats {
        let stats = &self.access_cache.cache_stats() + &self.create_sid_cache.cache_stats();
        &stats + &self.xperms_access_cache.cache_stats()
    }

    pub fn compute_kernel_access_decision(
        &self,
        delegate: &impl Query,
        source_sid: SecurityId,
        target_sid: SecurityId,
        target_class: KernelClass,
    ) -> KernelAccessDecision {
        let query_args = AccessQueryArgs { source_sid, target_sid, target_class };
        self.access_cache.get_or_insert(&query_args, || {
            delegate.compute_access_decision(source_sid, target_sid, target_class)
        })
    }

    pub fn compute_create_sid(
        &self,
        delegate: &impl Query,
        source_sid: SecurityId,
        target_sid: SecurityId,
        target_class: KernelClass,
        name: &[u8],
    ) -> Result<SecurityId, anyhow::Error> {
        if !name.is_empty() {
            delegate.compute_create_sid(source_sid, target_sid, target_class, name)
        } else {
            let query_args = AccessQueryArgs { source_sid, target_sid, target_class };
            self.create_sid_cache.get_or_try_insert(&query_args, || {
                delegate.compute_create_sid(source_sid, target_sid, target_class, name)
            })
        }
    }

    pub fn compute_kernel_xperms_access_decision(
        &self,
        delegate: &impl Query,
        xperms_kind: XpermsKind,
        source_sid: SecurityId,
        target_sid: SecurityId,
        permission: KernelPermission,
        xperms_prefix: u8,
    ) -> KernelXpermsAccessDecision {
        let query_args = XpermsAccessQueryArgs {
            xperms_kind,
            source_sid,
            target_sid,
            permission,
            xperms_prefix,
        };
        self.xperms_access_cache.get_or_insert(&query_args, || {
            delegate.compute_xperms_access_decision(
                xperms_kind,
                source_sid,
                target_sid,
                permission,
                xperms_prefix,
            )
        })
    }

    pub fn reset(&self) {
        self.access_cache.reset();
        self.create_sid_cache.reset();
        self.xperms_access_cache.reset();
    }

    /// Returns true if the main access decision cache has reached capacity.
    #[cfg(test)]
    fn access_cache_is_full(&self) -> bool {
        self.access_cache.is_full()
    }
}

/// Default size of an access vector cache shared by all threads in the system.
pub const DEFAULT_SHARED_SIZE: QueryCacheCapacity = QueryCacheCapacity {
    // This was empirically determined to be a good default,
    access_cache_capacity: 2048,
    // The following were determined as a fraction of the access cache capacity.
    sid_cache_capacity: 2048,
    xperms_cache_capacity: 512,
};

/// An access vector cache.
#[derive(Clone)]
pub(super) struct AccessVectorCache {
    cache: Arc<FifoQueryCache>,
    backend: Arc<SecurityServerBackend>,
}

impl AccessVectorCache {
    pub fn new(backend: Arc<SecurityServerBackend>) -> Self {
        let cache = FifoQueryCache::new(DEFAULT_SHARED_SIZE);
        Self { cache: Arc::new(cache), backend }
    }

    pub fn cache_stats(&self) -> CacheStats {
        self.cache.cache_stats()
    }

    pub fn reset(&self) {
        self.cache.reset()
    }
}

impl Query for AccessVectorCache {
    fn compute_access_decision(
        &self,
        source_sid: SecurityId,
        target_sid: SecurityId,
        target_class: KernelClass,
    ) -> KernelAccessDecision {
        self.cache.compute_kernel_access_decision(
            self.backend.as_ref(),
            source_sid,
            target_sid,
            target_class,
        )
    }

    fn compute_create_sid(
        &self,
        source_sid: SecurityId,
        target_sid: SecurityId,
        target_class: KernelClass,
        name: &[u8],
    ) -> Result<SecurityId, anyhow::Error> {
        self.cache.compute_create_sid(
            self.backend.as_ref(),
            source_sid,
            target_sid,
            target_class,
            name,
        )
    }

    fn compute_xperms_access_decision(
        &self,
        xperms_kind: XpermsKind,
        source_sid: SecurityId,
        target_sid: SecurityId,
        permission: KernelPermission,
        xperms_prefix: u8,
    ) -> KernelXpermsAccessDecision {
        self.cache.compute_kernel_xperms_access_decision(
            self.backend.as_ref(),
            xperms_kind,
            source_sid,
            target_sid,
            permission,
            xperms_prefix,
        )
    }
}

/// Test constants and helpers shared by `tests` and `starnix_tests`.
#[cfg(test)]
mod testing {
    use super::*;
    use crate::SecurityId;

    use std::num::NonZeroU32;
    use std::sync::LazyLock;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// SID to use where any value will do.
    pub(super) static A_TEST_SID: LazyLock<SecurityId> = LazyLock::new(unique_sid);

    /// Default fixed cache capacity to request in tests.
    pub(super) const TEST_CAPACITY: QueryCacheCapacity = QueryCacheCapacity {
        access_cache_capacity: 16,
        sid_cache_capacity: 16,
        xperms_cache_capacity: 4,
    };

    /// Returns a new `SecurityId` with unique id.
    pub(super) fn unique_sid() -> SecurityId {
        static NEXT_ID: AtomicU32 = AtomicU32::new(1000);
        SecurityId(NonZeroU32::new(NEXT_ID.fetch_add(1, Ordering::AcqRel)).unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    use crate::policy::{AccessVector, XpermsBitmap};
    use crate::{KernelClass, ProcessPermission};

    use std::sync::atomic::{AtomicUsize, Ordering};

    /// No-op policy query delegate that allows all permissions and maintains no internal state, for testing.
    #[derive(Default)]
    struct TestDelegate {
        query_count: AtomicUsize,
    }

    impl TestDelegate {
        fn query_count(&self) -> usize {
            self.query_count.load(Ordering::Relaxed)
        }
    }

    impl Query for TestDelegate {
        fn compute_access_decision(
            &self,
            _source_sid: SecurityId,
            _target_sid: SecurityId,
            _target_class: KernelClass,
        ) -> KernelAccessDecision {
            self.query_count.fetch_add(1, Ordering::Relaxed);
            KernelAccessDecision {
                allow: AccessVector::ALL,
                audit: AccessVector::NONE,
                flags: 0,
                todo_bug: None,
            }
        }

        fn compute_create_sid(
            &self,
            _source_sid: SecurityId,
            _target_sid: SecurityId,
            _target_class: KernelClass,
            _name: &[u8],
        ) -> Result<SecurityId, anyhow::Error> {
            unreachable!()
        }

        fn compute_xperms_access_decision(
            &self,
            _xperms_kind: XpermsKind,
            _source_sid: SecurityId,
            _target_sid: SecurityId,
            _target_class: KernelPermission,
            _xperms_prefix: u8,
        ) -> KernelXpermsAccessDecision {
            self.query_count.fetch_add(1, Ordering::Relaxed);
            KernelXpermsAccessDecision {
                allow: XpermsBitmap::ALL,
                audit: XpermsBitmap::NONE,
                permissive: false,
                has_todo: false,
            }
        }
    }

    #[test]
    fn fixed_access_vector_cache_add_entry() {
        let delegate = TestDelegate::default();
        let avc = FifoQueryCache::new(TEST_CAPACITY);
        assert_eq!(0, delegate.query_count());
        assert_eq!(
            AccessVector::ALL,
            avc.compute_kernel_access_decision(
                &delegate,
                A_TEST_SID.clone(),
                A_TEST_SID.clone(),
                KernelClass::Process
            )
            .allow
        );
        assert_eq!(1, delegate.query_count());
        assert_eq!(
            AccessVector::ALL,
            avc.compute_kernel_access_decision(
                &delegate,
                A_TEST_SID.clone(),
                A_TEST_SID.clone(),
                KernelClass::Process
            )
            .allow
        );
        assert_eq!(1, delegate.query_count());
        assert_eq!(false, avc.access_cache_is_full());
    }

    #[test]
    fn fixed_access_vector_cache_reset() {
        let delegate = TestDelegate::default();
        let avc = FifoQueryCache::new(TEST_CAPACITY);

        avc.reset();
        assert_eq!(false, avc.access_cache_is_full());

        assert_eq!(0, delegate.query_count());
        assert_eq!(
            AccessVector::ALL,
            avc.compute_kernel_access_decision(
                &delegate,
                A_TEST_SID.clone(),
                A_TEST_SID.clone(),
                KernelClass::Process
            )
            .allow
        );
        assert_eq!(1, delegate.query_count());
        assert_eq!(false, avc.access_cache_is_full());

        avc.reset();
        assert_eq!(false, avc.access_cache_is_full());
    }

    #[test]
    fn access_vector_cache_ioctl_hit() {
        let delegate = TestDelegate::default();
        let avc = FifoQueryCache::new(TEST_CAPACITY);
        assert_eq!(0, delegate.query_count());
        assert_eq!(
            XpermsBitmap::ALL,
            avc.compute_kernel_xperms_access_decision(
                &delegate,
                XpermsKind::Ioctl,
                A_TEST_SID.clone(),
                A_TEST_SID.clone(),
                ProcessPermission::Fork.into(),
                0x0,
            )
            .allow
        );
        assert_eq!(1, delegate.query_count());
        // The second request for the same key is a cache hit.
        assert_eq!(
            XpermsBitmap::ALL,
            avc.compute_kernel_xperms_access_decision(
                &delegate,
                XpermsKind::Ioctl,
                A_TEST_SID.clone(),
                A_TEST_SID.clone(),
                ProcessPermission::Fork.into(),
                0x0
            )
            .allow
        );
        assert_eq!(1, delegate.query_count());
    }

    #[test]
    fn access_vector_cache_nlmsg_hit() {
        let delegate = TestDelegate::default();
        let avc = FifoQueryCache::new(TEST_CAPACITY);
        assert_eq!(0, delegate.query_count());
        assert_eq!(
            XpermsBitmap::ALL,
            avc.compute_kernel_xperms_access_decision(
                &delegate,
                XpermsKind::Nlmsg,
                A_TEST_SID.clone(),
                A_TEST_SID.clone(),
                ProcessPermission::Fork.into(),
                0x0,
            )
            .allow
        );
        assert_eq!(1, delegate.query_count());
        // The second request for the same key is a cache hit.
        assert_eq!(
            XpermsBitmap::ALL,
            avc.compute_kernel_xperms_access_decision(
                &delegate,
                XpermsKind::Nlmsg,
                A_TEST_SID.clone(),
                A_TEST_SID.clone(),
                ProcessPermission::Fork.into(),
                0x0
            )
            .allow
        );
        assert_eq!(1, delegate.query_count());
    }

    #[test]
    fn access_vector_cache_nlmsg_and_ioctl() {
        let delegate = TestDelegate::default();
        let avc = FifoQueryCache::new(TEST_CAPACITY);

        avc.compute_kernel_xperms_access_decision(
            &delegate,
            XpermsKind::Ioctl,
            A_TEST_SID.clone(),
            A_TEST_SID.clone(),
            ProcessPermission::Fork.into(),
            0x0,
        );
        assert_eq!(avc.cache_stats().allocs, 1);

        // Query for an `nlmsg` extended permission for the same source, target, class,
        // and prefix. This should cause a new allocation.
        avc.compute_kernel_xperms_access_decision(
            &delegate,
            XpermsKind::Nlmsg,
            A_TEST_SID.clone(),
            A_TEST_SID.clone(),
            ProcessPermission::Fork.into(),
            0x0,
        );
        assert_eq!(avc.cache_stats().allocs, 2);
    }
}

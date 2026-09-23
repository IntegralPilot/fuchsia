// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mm::{MemoryManager, PAGE_SIZE};
use bitflags::bitflags;
use range_map::RangeMap;
use starnix_logging::track_stub;
use starnix_sync::{LockDepMutex, UserFaultInner};
use starnix_uapi::errors::Errno;
use starnix_uapi::user_address::UserAddress;
use starnix_uapi::{
    UFFD_FEATURE_EVENT_FORK, UFFD_FEATURE_EVENT_REMAP, UFFD_FEATURE_EVENT_REMOVE,
    UFFD_FEATURE_EVENT_UNMAP, UFFD_FEATURE_MINOR_HUGETLBFS, UFFD_FEATURE_MINOR_SHMEM,
    UFFD_FEATURE_MISSING_HUGETLBFS, UFFD_FEATURE_MISSING_SHMEM, UFFD_FEATURE_SIGBUS,
    UFFD_FEATURE_THREAD_ID, UFFDIO_CONTINUE_MODE_DONTWAKE, UFFDIO_COPY_MODE_DONTWAKE,
    UFFDIO_COPY_MODE_WP, UFFDIO_REGISTER_MODE_MINOR, UFFDIO_REGISTER_MODE_MISSING,
    UFFDIO_REGISTER_MODE_WP, UFFDIO_ZEROPAGE_MODE_DONTWAKE, errno, error,
};
use std::ops::Range;
use std::sync::{Arc, Weak};

#[derive(Debug)]
pub struct UserFault {
    mm: Weak<MemoryManager>,
    state: LockDepMutex<UserFaultState, UserFaultInner>,
}

#[derive(Debug, Clone)]
struct UserFaultState {
    /// If initialized, contains features that this userfault was initialized with
    features: Option<UserFaultFeatures>,

    /// Pages that are currently registered with this userfault object, and whether they are
    /// already populated.
    userfault_pages: RangeMap<UserAddress, bool>,
}

impl UserFault {
    pub fn new(mm: Weak<MemoryManager>) -> Self {
        Self { mm, state: LockDepMutex::new(UserFaultState::new()) }
    }

    pub fn insert_pages(&self, range: Range<UserAddress>, value: bool) {
        // RangeMap uses #[must_use] for its default usecase but this drop is trivial.
        let _ = self.state.lock().userfault_pages.insert(range, value);
    }

    pub fn remove_pages(&self, range: Range<UserAddress>) -> bool {
        !self.state.lock().userfault_pages.remove(range).is_empty()
    }

    pub fn get_registered_pages_overlapping_range(
        &self,
        range: Range<UserAddress>,
    ) -> Vec<Range<UserAddress>> {
        self.state.lock().userfault_pages.get_keys(range).cloned().collect()
    }

    pub fn contains_addr(&self, addr: UserAddress) -> bool {
        self.state.lock().userfault_pages.get(addr).is_some()
    }

    pub fn get_first_populated_page_after(&self, addr: UserAddress) -> Option<UserAddress> {
        self.state.lock().userfault_pages.get(addr).map(|(affected_range, is_populated)| {
            if *is_populated { addr } else { affected_range.end }
        })
    }

    pub fn is_initialized(self: &Arc<Self>) -> bool {
        self.state.lock().features.is_some()
    }

    pub fn has_features(self: &Arc<Self>, features: UserFaultFeatures) -> bool {
        self.state.lock().features.map(|f| f.contains(features)).unwrap_or(false)
    }

    pub fn initialize(self: &Arc<Self>, features: UserFaultFeatures) {
        self.state.lock().features = Some(features);
    }

    pub fn op_register(
        self: &Arc<Self>,
        start: UserAddress,
        len: u64,
        mode: FaultRegisterMode,
    ) -> Result<SupportedUserFaultIoctls, Errno> {
        if !self.is_initialized() {
            return error!(EINVAL);
        }
        if !self.has_features(UserFaultFeatures::SIGBUS) {
            track_stub!(TODO("https://fxbug.dev/391599171"), "userfault without SIGBUS feature");
            return error!(ENOTSUP);
        }
        check_op_range(start, len)?;
        let mm = self.mm.upgrade().ok_or_else(|| errno!(EINVAL))?;

        mm.register_with_uffd(start, len as usize, self, mode)?;
        Ok(SupportedUserFaultIoctls::COPY | SupportedUserFaultIoctls::ZERO_PAGE)
    }

    pub fn op_unregister(self: &Arc<Self>, start: UserAddress, len: u64) -> Result<(), Errno> {
        if !self.is_initialized() {
            return error!(EINVAL);
        }
        check_op_range(start, len)?;
        let mm = self.mm.upgrade().ok_or_else(|| errno!(EINVAL))?;
        mm.unregister_range_from_uffd(self, start, len as usize)
    }

    pub fn op_copy(
        self: &Arc<Self>,
        mm_source: &MemoryManager,
        source: UserAddress,
        dest: UserAddress,
        len: u64,
        _mode: FaultCopyMode,
    ) -> Result<usize, Errno> {
        if !self.is_initialized() {
            return error!(EINVAL);
        }
        check_op_range(source, len)?;
        check_op_range(dest, len)?;
        let mm = self.mm.upgrade().ok_or_else(|| errno!(EINVAL))?;

        // If the copy happens inside the same process, do it inside this process' memory manager
        // so that the lock is held throughout the operation.
        if Arc::as_ptr(&mm) == mm_source as *const MemoryManager {
            mm.copy_from_uffd(source, dest, len as usize, self)
        } else {
            let mut buf = vec![std::mem::MaybeUninit::uninit(); len as usize];
            let buf = mm_source.syscall_read_memory(source, &mut buf)?;
            mm.fill_from_uffd(dest, buf, len as usize, self)
        }
    }

    pub fn op_zero(
        self: &Arc<Self>,
        start: UserAddress,
        len: u64,
        _mode: FaultZeroMode,
    ) -> Result<usize, Errno> {
        if !self.is_initialized() {
            return error!(EINVAL);
        }
        check_op_range(start, len)?;
        let mm = self.mm.upgrade().ok_or_else(|| errno!(EINVAL))?;
        mm.zero_from_uffd(start, len as usize, self)
    }

    pub fn cleanup(self: &Arc<Self>) {
        if let Some(mm) = self.mm.upgrade() {
            mm.unregister_uffd(self);
        }
    }
}

impl UserFaultState {
    pub fn new() -> Self {
        Self { features: None, userfault_pages: RangeMap::default() }
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, Eq, PartialEq)]
    pub struct UserFaultFeatures: u32 {
        const ALL_SUPPORTED = UFFD_FEATURE_SIGBUS;
        const EVENT_FORK = UFFD_FEATURE_EVENT_FORK;
        const EVENT_REMAP = UFFD_FEATURE_EVENT_REMAP;
        const EVENT_REMOVE = UFFD_FEATURE_EVENT_REMOVE;
        const EVENT_UNMAP = UFFD_FEATURE_EVENT_UNMAP;
        const MISSING_HUGETLBFS = UFFD_FEATURE_MISSING_HUGETLBFS;
        const MISSING_SHMEM = UFFD_FEATURE_MISSING_SHMEM;
        const SIGBUS = UFFD_FEATURE_SIGBUS;
        const THREAD_ID = UFFD_FEATURE_THREAD_ID;
        const MINOR_HUGETLBFS = UFFD_FEATURE_MINOR_HUGETLBFS;
        const MINOR_SHMEM = UFFD_FEATURE_MINOR_SHMEM;
    }

    #[derive(Debug, Clone, Copy, Eq, PartialEq)]
    pub struct FaultRegisterMode: u32 {
        const MINOR = UFFDIO_REGISTER_MODE_MINOR;
        const MISSING = UFFDIO_REGISTER_MODE_MISSING;
        const WRITE_PROTECT = UFFDIO_REGISTER_MODE_WP;
    }

    pub struct FaultCopyMode: u32 {
        const DONT_WAKE = UFFDIO_COPY_MODE_DONTWAKE;
        const WRITE_PROTECT = UFFDIO_COPY_MODE_WP;
    }

    pub struct FaultZeroMode: u32 {
        const DONT_WAKE = UFFDIO_ZEROPAGE_MODE_DONTWAKE;
    }

    pub struct FaultContinueMode: u32 {
        const DONT_WAKE = UFFDIO_CONTINUE_MODE_DONTWAKE;
    }


    pub struct SupportedUserFaultIoctls: u64 {
        const COPY = 1 << starnix_uapi::_UFFDIO_COPY;
        const WAKE = 1 << starnix_uapi::_UFFDIO_WAKE;
        const WRITE_PROTECT = 1 << starnix_uapi::_UFFDIO_WRITEPROTECT;
        const ZERO_PAGE = 1 << starnix_uapi::_UFFDIO_ZEROPAGE;
        const CONTINUE = 1 << starnix_uapi::_UFFDIO_CONTINUE;
    }
}

fn check_op_range(addr: UserAddress, len: u64) -> Result<(), Errno> {
    if addr.is_aligned(*PAGE_SIZE) && len % *PAGE_SIZE == 0 && len > 0 {
        Ok(())
    } else {
        error!(EINVAL)
    }
}

// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mm::barrier::{BarrierType, system_barrier};
use crate::mm::mapping::MappingBackingMemory;
use crate::mm::memory::MemoryObject;
use crate::mm::private_anonymous_memory_manager::PrivateAnonymousMemoryManager;
use crate::mm::{
    FaultRegisterMode, FutexTable, InflightVmsplicedPayloads, MapInfoCache, Mapping,
    MappingBacking, MappingFlags, MappingMode, MappingName, MappingNameRef, MlockPinFlavor,
    PrivateFutexKey, ProtectionFlags, UserFault, VMEX_RESOURCE, VmsplicePayload,
    VmsplicePayloadSegment,
};
use crate::security;
use crate::signals::{SignalDetail, SignalInfo};
use crate::task::{CurrentTask, ExceptionResult, PageFaultExceptionReport, Pid, Task};
use crate::vfs::aio::AioContext;
use crate::vfs::buffers::{InputBuffer, OutputBuffer};
use crate::vfs::pseudo::dynamic_file::{
    DynamicFile, DynamicFileBuf, DynamicFileSource, SequenceFileSource,
};
use crate::vfs::{
    FileObject, FileOps, FsContext, FsString, NamespaceNode, fileops_impl_noop_sync,
    fileops_impl_seekable,
};
use anyhow::{Error, anyhow};
use bitflags::bitflags;
use flyweights::FlyByteStr;
use linux_uapi::BUS_ADRERR;
use memory_pinning::PinnedMapping;
use range_map::RangeMap;
use smallvec::SmallVec;
use starnix_ext::map_ext::EntryExt;
use starnix_lifecycle::DropNotifier;
use starnix_logging::{CATEGORY_STARNIX_MM, impossible_error, log_error, log_warn, track_stub};
use starnix_sync::{LockDepMutex, MmDumpable, Mutex, RwLock, RwLockWriteGuard, ordered_write_lock};
use starnix_types::arch::ArchWidth;
use starnix_types::futex_address::FutexAddress;
use starnix_types::math::{round_down_to_system_page_size, round_up_to_system_page_size};
use starnix_types::user_buffer::{UserBuffer, UserBuffers};
use starnix_uapi::auth::CAP_IPC_LOCK;
use starnix_uapi::errors::Errno;
use starnix_uapi::range_ext::RangeExt;
use starnix_uapi::resource_limits::Resource;
use starnix_uapi::restricted_aspace::{
    RESTRICTED_ASPACE_BASE, RESTRICTED_ASPACE_HIGHEST_ADDRESS, RESTRICTED_ASPACE_RANGE,
    RESTRICTED_ASPACE_SIZE,
};
use starnix_uapi::signals::{SIGBUS, SIGSEGV};
use starnix_uapi::user_address::{ArchSpecific, UserAddress};
use starnix_uapi::{
    MADV_COLD, MADV_COLLAPSE, MADV_DODUMP, MADV_DOFORK, MADV_DONTDUMP, MADV_DONTFORK,
    MADV_DONTNEED, MADV_DONTNEED_LOCKED, MADV_FREE, MADV_HUGEPAGE, MADV_HWPOISON, MADV_KEEPONFORK,
    MADV_MERGEABLE, MADV_NOHUGEPAGE, MADV_NORMAL, MADV_PAGEOUT, MADV_POPULATE_READ, MADV_RANDOM,
    MADV_REMOVE, MADV_SEQUENTIAL, MADV_SOFT_OFFLINE, MADV_UNMERGEABLE, MADV_WILLNEED,
    MADV_WIPEONFORK, MREMAP_DONTUNMAP, MREMAP_FIXED, MREMAP_MAYMOVE, errno, error,
    from_status_like_fdio,
};
use std::collections::HashMap;
use std::hash::Hasher;
use std::mem::MaybeUninit;
use std::ops::{ControlFlow, Deref, DerefMut, Range, RangeBounds};
use std::sync::{Arc, LazyLock, Weak};
use zerocopy::IntoBytes;
use zx::{Rights, VmoChildOptions};

pub const ZX_VM_SPECIFIC_OVERWRITE: zx::VmarFlags =
    zx::VmarFlags::from_bits_retain(zx::VmarFlagsExtended::SPECIFIC_OVERWRITE.bits());

/// Initializes the usercopy utilities.
///
/// It is useful to explicitly call this so that the usercopy is initialized
/// at a known instant. For example, Starnix may want to make sure the usercopy
/// thread created to support user copying is associated to the Starnix process
/// and not a restricted-mode process.
pub fn init_usercopy() {
    // This call lazily initializes the `Usercopy` instance.
    let _ = usercopy();
}

thread_local! {
    /// The last mapping generation seen by this thread.
    /// Used to prevent infinite loops in page fault handling.
    static LAST_SEEN_MAPPING_GENERATION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

pub const GUARD_PAGE_COUNT_FOR_GROWSDOWN_MAPPINGS: usize = 256;

#[cfg(target_arch = "x86_64")]
const ASLR_RANDOM_BITS: usize = 27;

#[cfg(target_arch = "aarch64")]
const ASLR_RANDOM_BITS: usize = 28;

#[cfg(target_arch = "riscv64")]
const ASLR_RANDOM_BITS: usize = 18;

/// Number of bits of entropy for processes running in 32 bits mode.
const ASLR_32_RANDOM_BITS: usize = 8;

// The biggest we expect stack to be; increase as needed
// TODO(https://fxbug.dev/322874791): Once setting RLIMIT_STACK is implemented, we should use it.
const MAX_STACK_SIZE: usize = 512 * 1024 * 1024;

// Value to report temporarily as the VM RSS HWM.
// TODO(https://fxbug.dev/396221597): Need support from the kernel to track the committed bytes high
// water mark.
const STUB_VM_RSS_HWM: usize = 2 * 1024 * 1024;

fn usercopy() -> &'static usercopy::Usercopy {
    static USERCOPY: LazyLock<usercopy::Usercopy> = LazyLock::new(|| {
        // ASUMPTION: All Starnix managed Linux processes have the same
        // restricted mode address range.
        usercopy::Usercopy::new(RESTRICTED_ASPACE_RANGE).unwrap()
    });

    LazyLock::force(&USERCOPY)
}

/// Provides an implementation for zxio's `zxio_maybe_faultable_copy` that supports
/// catching faults.
///
/// See zxio's `zxio_maybe_faultable_copy` documentation for more details.
///
/// # Safety
///
/// Only one of `src`/`dest` may be an address to a buffer owned by user/restricted-mode
/// (`ret_dest` indicates whether the user-owned buffer is `dest` when `true`).
/// The other must be a valid Starnix/normal-mode buffer that will never cause a fault
/// when the first `count` bytes are read/written.
#[unsafe(no_mangle)]
pub unsafe fn zxio_maybe_faultable_copy_impl(
    dest: *mut u8,
    src: *const u8,
    count: usize,
    ret_dest: bool,
) -> bool {
    // SAFETY: Only one of `src`/`dest` may be an address in user/restricted-mode by to our own
    // SAFETY guarantee.
    let ret = unsafe { usercopy().raw_hermetic_copy(dest, src, count, ret_dest) };
    ret == count
}

pub static PAGE_SIZE: LazyLock<u64> = LazyLock::new(|| zx::system_get_page_size() as u64);

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct MappingOptions: u16 {
      const SHARED      = 1 << 0;
      const ANONYMOUS   = 1 << 1;
      const LOWER_32BIT = 1 << 2;
      const GROWSDOWN   = 1 << 3;
      const ELF_BINARY  = 1 << 4;
      const DONTFORK    = 1 << 5;
      const WIPEONFORK  = 1 << 6;
      const DONT_SPLIT  = 1 << 7;
      const DONT_EXPAND = 1 << 8;
      const POPULATE    = 1 << 9;
    }
}

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct MremapFlags: u32 {
        const MAYMOVE = MREMAP_MAYMOVE;
        const FIXED = MREMAP_FIXED;
        const DONTUNMAP = MREMAP_DONTUNMAP;
    }
}

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct MsyncFlags: u32 {
        const ASYNC = starnix_uapi::MS_ASYNC;
        const INVALIDATE = starnix_uapi::MS_INVALIDATE;
        const SYNC = starnix_uapi::MS_SYNC;
    }
}

const PROGRAM_BREAK_LIMIT: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Eq, PartialEq)]
struct ProgramBreak {
    // These base address at which the data segment is mapped.
    base: UserAddress,

    // The current program break.
    //
    // The addresses from [base, current.round_up(*PAGE_SIZE)) are mapped into the
    // client address space from the underlying |memory|.
    current: UserAddress,
}

/// The policy about whether the address space can be dumped.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum DumpPolicy {
    /// The address space cannot be dumped.
    ///
    /// Corresponds to SUID_DUMP_DISABLE.
    Disable,

    /// The address space can be dumped.
    ///
    /// Corresponds to SUID_DUMP_USER.
    User,
}

// Supported types of membarriers.
pub enum MembarrierType {
    Memory,   // MEMBARRIER_CMD_GLOBAL, etc
    SyncCore, // MEMBARRIER_CMD_..._SYNC_CORE
}

// Tracks the types of membarriers this address space is registered to receive.
#[derive(Default, Clone)]
struct MembarrierRegistrations {
    memory: bool,
    sync_core: bool,
}

#[derive(Default)]
struct Mappings {
    /// The mappings record which object backs each address.
    map: RangeMap<UserAddress, Mapping>,

    /// Generation counter for mappings. Incremented on any modification to `mappings`.
    ///
    /// This is used to detect stale mappings in `handle_page_fault`.
    generation: u64,

    /// The cached sum of the lengths of all mapped ranges.
    total_usage: usize,
}

impl Deref for Mappings {
    type Target = RangeMap<UserAddress, Mapping>;

    fn deref(&self) -> &Self::Target {
        &self.map
    }
}

impl Mappings {
    pub fn insert(&mut self, range: std::ops::Range<UserAddress>, value: Mapping) -> Vec<Mapping> {
        self.generation = self.generation.wrapping_add(1);
        let range_len = range.end - range.start;
        let removed_len: usize = self
            .map
            .range(range.clone())
            .map(|(r, _)| {
                let intersection = r.intersect(&range);
                intersection.end - intersection.start
            })
            .sum();
        let removed = self.map.insert(range, value);
        self.total_usage = self.total_usage.saturating_add(range_len).saturating_sub(removed_len);
        removed
    }

    pub fn remove(&mut self, range: std::ops::Range<UserAddress>) -> Vec<Mapping> {
        self.generation = self.generation.wrapping_add(1);
        let removed_len: usize = self
            .map
            .range(range.clone())
            .map(|(r, _)| {
                let intersection = r.intersect(&range);
                intersection.end - intersection.start
            })
            .sum();
        let removed = self.map.remove(range);
        self.total_usage = self.total_usage.saturating_sub(removed_len);
        removed
    }

    pub fn append_non_overlapping(
        &mut self,
        range: std::ops::Range<UserAddress>,
        value: Mapping,
    ) -> bool {
        self.generation = self.generation.wrapping_add(1);
        let range_len = range.end - range.start;
        if self.map.append_non_overlapping(range, value) {
            self.total_usage = self.total_usage.saturating_add(range_len);
            true
        } else {
            false
        }
    }

    pub fn update_exact<F, E>(
        &mut self,
        range: &std::ops::Range<UserAddress>,
        f: F,
    ) -> Result<bool, E>
    where
        F: FnOnce(&mut Mapping) -> Result<(), E>,
    {
        self.generation = self.generation.wrapping_add(1);
        self.map.update_exact(range, f)
    }
}

pub struct MemoryManagerState {
    /// The memory mappings currently used by this address space.
    mappings: Mappings,

    /// UserFaults registered with this memory manager.
    userfaultfds: Vec<Weak<UserFault>>,

    /// Shadow mappings for mlock()'d pages.
    ///
    /// Used for MlockPinFlavor::ShadowProcess to keep track of when we need to unmap
    /// memory from the shadow process.
    shadow_mappings_for_mlock: RangeMap<UserAddress, Arc<PinnedMapping>>,

    forkable_state: MemoryManagerForkableState,
}

// 64k under the 4GB
const LOWER_4GB_LIMIT: UserAddress = UserAddress::const_from(0xffff_0000);

#[derive(Default, Clone)]
pub struct MemoryManagerForkableState {
    /// State for the brk and sbrk syscalls.
    brk: Option<ProgramBreak>,

    /// The namespace node that represents the executable associated with this task.
    executable_node: Option<NamespaceNode>,

    pub stack_size: usize,
    pub stack_start: UserAddress,
    pub auxv_start: UserAddress,
    pub auxv_end: UserAddress,
    pub argv_start: UserAddress,
    pub argv_end: UserAddress,
    pub environ_start: UserAddress,
    pub environ_end: UserAddress,

    /// vDSO location
    pub vdso_base: UserAddress,

    /// Randomized regions:
    pub mmap_top: UserAddress,
    pub stack_origin: UserAddress,
    pub brk_origin: UserAddress,

    // Membarrier registrations
    membarrier_registrations: MembarrierRegistrations,
}

impl Deref for MemoryManagerState {
    type Target = MemoryManagerForkableState;
    fn deref(&self) -> &Self::Target {
        &self.forkable_state
    }
}

impl DerefMut for MemoryManagerState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.forkable_state
    }
}

#[derive(Debug, Default)]
struct ReleasedMappings {
    doomed: Vec<Mapping>,
    doomed_pins: Vec<Arc<PinnedMapping>>,
}

impl ReleasedMappings {
    fn extend(&mut self, mappings: impl IntoIterator<Item = Mapping>) {
        self.doomed.extend(mappings);
    }

    fn extend_pins(&mut self, mappings: impl IntoIterator<Item = Arc<PinnedMapping>>) {
        self.doomed_pins.extend(mappings);
    }

    fn is_empty(&self) -> bool {
        self.doomed.is_empty() && self.doomed_pins.is_empty()
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.doomed.len() + self.doomed_pins.len()
    }

    fn finalize(&mut self, mm_state: RwLockWriteGuard<'_, MemoryManagerState>) {
        // Drop the state before the unmapped mappings, since dropping a mapping may acquire a lock
        // in `DirEntry`'s `drop`.
        std::mem::drop(mm_state);
        std::mem::take(&mut self.doomed);
        std::mem::take(&mut self.doomed_pins);
    }
}

impl Drop for ReleasedMappings {
    fn drop(&mut self) {
        assert!(self.is_empty(), "ReleasedMappings::finalize() must be called before drop");
    }
}

fn map_in_vmar(
    vmar: &zx::Vmar,
    vmar_info: &zx::VmarInfo,
    addr: SelectedAddress,
    memory: &MemoryObject,
    memory_offset: u64,
    length: usize,
    flags: MappingFlags,
    populate: bool,
) -> Result<(), Errno> {
    let vmar_offset = addr.addr().checked_sub(vmar_info.base).ok_or_else(|| errno!(ENOMEM))?;
    let vmar_extra_flags = match addr {
        SelectedAddress::Fixed(_) => zx::VmarFlags::SPECIFIC,
        SelectedAddress::FixedOverwrite(_) => ZX_VM_SPECIFIC_OVERWRITE,
    };

    if populate {
        let op = if flags.contains(MappingFlags::WRITE) {
            // Requires ZX_RIGHT_WRITEABLE which we should expect when the mapping is writeable.
            zx::VmoOp::COMMIT
        } else {
            // When we don't expect to have ZX_RIGHT_WRITEABLE, fall back to a VMO op that doesn't
            // need it.
            zx::VmoOp::PREFETCH
        };
        fuchsia_trace::duration!(CATEGORY_STARNIX_MM, "MmapCommitPages");
        let _ = memory.op_range(op, memory_offset, length as u64);
        // "The mmap() call doesn't fail if the mapping cannot be populated."
    }

    let vmar_maybe_map_range = if populate && !vmar_extra_flags.contains(ZX_VM_SPECIFIC_OVERWRITE) {
        zx::VmarFlags::MAP_RANGE
    } else {
        zx::VmarFlags::empty()
    };
    let vmar_flags = flags.access_flags().to_vmar_flags()
        | zx::VmarFlags::ALLOW_FAULTS
        | vmar_extra_flags
        | vmar_maybe_map_range;

    let map_result = memory.map_in_vmar(vmar, vmar_offset.ptr(), memory_offset, length, vmar_flags);
    let mapped_addr = map_result.map_err(MemoryManager::get_errno_for_map_err)?;

    let expected_addr = addr.addr().ptr();
    debug_assert_eq!(
        mapped_addr, expected_addr,
        "Zircon mapped to a different address than requested!"
    );

    Ok(())
}

impl MemoryManagerState {
    /// Returns occupied address ranges that intersect with the given range.
    ///
    /// An address range is "occupied" if (a) there is already a mapping in that range or (b) there
    /// is a GROWSDOWN mapping <= 256 pages above that range. The 256 pages below a GROWSDOWN
    /// mapping is the "guard region." The memory manager avoids mapping memory in the guard region
    /// in some circumstances to preserve space for the GROWSDOWN mapping to grow down.
    fn get_occupied_address_ranges<'a>(
        &'a self,
        subrange: &'a Range<UserAddress>,
    ) -> impl Iterator<Item = Range<UserAddress>> + 'a {
        let query_range = subrange.start
            ..(subrange
                .end
                .saturating_add(*PAGE_SIZE as usize * GUARD_PAGE_COUNT_FOR_GROWSDOWN_MAPPINGS));
        self.mappings.range(query_range).filter_map(|(range, mapping)| {
            let occupied_range = mapping.inflate_to_include_guard_pages(range);
            if occupied_range.start < subrange.end && subrange.start < occupied_range.end {
                Some(occupied_range)
            } else {
                None
            }
        })
    }

    fn count_possible_placements(
        &self,
        length: usize,
        subrange: &Range<UserAddress>,
    ) -> Option<usize> {
        let mut occupied_ranges = self.get_occupied_address_ranges(subrange);
        let mut possible_placements = 0;
        // If the allocation is placed at the first available address, every page that is left
        // before the next mapping or the end of subrange is +1 potential placement.
        let mut first_fill_end = subrange.start.checked_add(length)?;
        while first_fill_end <= subrange.end {
            let Some(mapping) = occupied_ranges.next() else {
                possible_placements += (subrange.end - first_fill_end) / (*PAGE_SIZE as usize) + 1;
                break;
            };
            if mapping.start >= first_fill_end {
                possible_placements += (mapping.start - first_fill_end) / (*PAGE_SIZE as usize) + 1;
            }
            first_fill_end = mapping.end.checked_add(length)?;
        }
        Some(possible_placements)
    }

    fn pick_placement(
        &self,
        length: usize,
        mut chosen_placement_idx: usize,
        subrange: &Range<UserAddress>,
    ) -> Option<UserAddress> {
        let mut candidate =
            Range { start: subrange.start, end: subrange.start.checked_add(length)? };
        let mut occupied_ranges = self.get_occupied_address_ranges(subrange);
        loop {
            let Some(mapping) = occupied_ranges.next() else {
                // No more mappings: treat the rest of the index as an offset.
                let res =
                    candidate.start.checked_add(chosen_placement_idx * *PAGE_SIZE as usize)?;
                debug_assert!(res.checked_add(length)? <= subrange.end);
                return Some(res);
            };
            if mapping.start < candidate.end {
                // doesn't fit, skip
                candidate = Range { start: mapping.end, end: mapping.end.checked_add(length)? };
                continue;
            }
            let unused_space =
                (mapping.start.ptr() - candidate.end.ptr()) / (*PAGE_SIZE as usize) + 1;
            if unused_space > chosen_placement_idx {
                // Chosen placement is within the range; treat the rest of the index as an offset.
                let res =
                    candidate.start.checked_add(chosen_placement_idx * *PAGE_SIZE as usize)?;
                return Some(res);
            }

            // chosen address is further up, skip
            chosen_placement_idx -= unused_space;
            candidate = Range { start: mapping.end, end: mapping.end.checked_add(length)? };
        }
    }

    fn find_random_unused_range(
        &self,
        length: usize,
        subrange: &Range<UserAddress>,
    ) -> Option<UserAddress> {
        let possible_placements = self.count_possible_placements(length, subrange)?;
        if possible_placements == 0 {
            return None;
        }
        let chosen_placement_idx = rand::random_range(0..possible_placements);
        self.pick_placement(length, chosen_placement_idx, subrange)
    }

    // Find the first unused range of addresses that fits a mapping of `length` bytes, searching
    // from `mmap_top` downwards.
    pub fn find_next_unused_range(&self, length: usize) -> Option<UserAddress> {
        let gap_size = length as u64;
        let mut upper_bound = self.mmap_top;

        loop {
            let gap_end = self.mappings.find_gap_end(gap_size, &upper_bound);
            let candidate = gap_end.checked_sub(length)?;

            // Is there a next mapping? If not, the candidate is already good.
            let Some((occupied_range, mapping)) = self.mappings.get(gap_end) else {
                return Some(candidate);
            };
            let occupied_range = mapping.inflate_to_include_guard_pages(occupied_range);
            // If it doesn't overlap, the gap is big enough to fit.
            if occupied_range.start >= gap_end {
                return Some(candidate);
            }
            // If there was a mapping in the way, use the start of that range as the upper bound.
            upper_bound = occupied_range.start;
        }
    }

    // Accept the hint if the range is unused and within the range available for mapping.
    fn is_hint_acceptable(&self, hint_addr: UserAddress, length: usize) -> bool {
        let Some(hint_end) = hint_addr.checked_add(length) else {
            return false;
        };
        if !RESTRICTED_ASPACE_RANGE.contains(&hint_addr.ptr())
            || !RESTRICTED_ASPACE_RANGE.contains(&hint_end.ptr())
        {
            return false;
        };
        self.get_occupied_address_ranges(&(hint_addr..hint_end)).next().is_none()
    }

    fn select_address(
        &self,
        addr: DesiredAddress,
        length: usize,
        flags: MappingFlags,
    ) -> Result<SelectedAddress, Errno> {
        let adjusted_length = round_up_to_system_page_size(length).or_else(|_| error!(ENOMEM))?;

        let find_address = || -> Result<SelectedAddress, Errno> {
            let new_addr = if flags.contains(MappingFlags::LOWER_32BIT) {
                // MAP_32BIT specifies that the memory allocated will
                // be within the first 2 GB of the process address space.
                self.find_random_unused_range(
                    adjusted_length,
                    &(UserAddress::from_ptr(RESTRICTED_ASPACE_BASE)
                        ..UserAddress::from_ptr(0x80000000)),
                )
                .ok_or_else(|| errno!(ENOMEM))?
            } else {
                self.find_next_unused_range(adjusted_length).ok_or_else(|| errno!(ENOMEM))?
            };

            Ok(SelectedAddress::Fixed(new_addr))
        };

        Ok(match addr {
            DesiredAddress::Any => find_address()?,
            DesiredAddress::Hint(hint_addr) => {
                // Round down to page size
                let hint_addr =
                    UserAddress::from_ptr(hint_addr.ptr() - hint_addr.ptr() % *PAGE_SIZE as usize);
                if self.is_hint_acceptable(hint_addr, adjusted_length) {
                    SelectedAddress::Fixed(hint_addr)
                } else {
                    find_address()?
                }
            }
            DesiredAddress::Fixed(addr) => SelectedAddress::Fixed(addr),
            DesiredAddress::FixedOverwrite(addr) => SelectedAddress::FixedOverwrite(addr),
        })
    }

    fn validate_addr(&self, addr: DesiredAddress, length: usize) -> Result<(), Errno> {
        if length > RESTRICTED_ASPACE_SIZE {
            return error!(ENOMEM);
        }
        match addr {
            DesiredAddress::Fixed(a) | DesiredAddress::FixedOverwrite(a) => {
                let end = a.checked_add(length).ok_or_else(|| errno!(ENOMEM))?;
                if end > UserAddress::from_ptr(RESTRICTED_ASPACE_HIGHEST_ADDRESS as usize) {
                    return error!(ENOMEM);
                }
                if self.check_has_unauthorized_splits(a, length) {
                    return error!(ENOMEM);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn add_memory_mapping(
        &mut self,
        mm: &Arc<MemoryManager>,
        addr: DesiredAddress,
        memory: Arc<MemoryObject>,
        memory_offset: u64,
        length: usize,
        flags: MappingFlags,
        populate: bool,
        name: MappingName,
        mapping_mode: MappingMode,
        released_mappings: &mut ReleasedMappings,
    ) -> Result<UserAddress, Errno> {
        self.validate_addr(addr, length)?;

        let selected_address = self.select_address(addr, length, flags)?;
        let mapped_addr = selected_address.addr();
        if mapping_mode == MappingMode::Eager {
            mm.mapping_context.map_in_user_vmar(
                selected_address,
                &memory,
                memory_offset,
                length,
                flags,
                populate,
            )?;
        }

        let end = (mapped_addr + length)?.round_up(*PAGE_SIZE)?;

        if let DesiredAddress::FixedOverwrite(addr) = addr {
            assert_eq!(addr, mapped_addr);
            self.update_after_unmap(mm, addr, end - addr, released_mappings);
        }

        let mapping = Mapping::with_name(
            self.create_memory_backing(mapped_addr, memory, memory_offset),
            flags,
            name,
            mapping_mode,
        );
        released_mappings.extend(self.mappings.insert(mapped_addr..end, mapping));

        Ok(mapped_addr)
    }

    fn map_private_anonymous(
        &mut self,
        mm: &Arc<MemoryManager>,
        addr: DesiredAddress,
        length: usize,
        prot_flags: ProtectionFlags,
        options: MappingOptions,
        populate: bool,
        name: MappingName,
        released_mappings: &mut ReleasedMappings,
    ) -> Result<UserAddress, Errno> {
        self.validate_addr(addr, length)?;

        let flags = MappingFlags::from_access_flags_and_options(prot_flags, options);
        let selected_addr = self.select_address(addr, length, flags)?;
        let mapped_addr = selected_addr.addr();
        let backing_memory_offset = selected_addr.addr().ptr();

        mm.mapping_context.map_in_user_vmar(
            selected_addr,
            &mm.mapping_context.private_anonymous.backing,
            backing_memory_offset as u64,
            length,
            flags,
            populate,
        )?;

        let end = (mapped_addr + length)?.round_up(*PAGE_SIZE)?;
        if let DesiredAddress::FixedOverwrite(addr) = addr {
            assert_eq!(addr, mapped_addr);
            self.update_after_unmap(mm, addr, end - addr, released_mappings);
        }

        let mapping = Mapping::new_private_anonymous(flags, name, MappingMode::Eager);
        released_mappings.extend(self.mappings.insert(mapped_addr..end, mapping));

        Ok(mapped_addr)
    }

    fn map_anonymous(
        &mut self,
        mm: &Arc<MemoryManager>,
        addr: DesiredAddress,
        length: usize,
        prot_flags: ProtectionFlags,
        options: MappingOptions,
        name: MappingName,
        released_mappings: &mut ReleasedMappings,
    ) -> Result<UserAddress, Errno> {
        if !options.contains(MappingOptions::SHARED) {
            return self.map_private_anonymous(
                mm,
                addr,
                length,
                prot_flags,
                options,
                options.contains(MappingOptions::POPULATE),
                name,
                released_mappings,
            );
        }
        let memory = create_anonymous_mapping_memory(length as u64)?;
        let flags = MappingFlags::from_access_flags_and_options(prot_flags, options);
        self.add_memory_mapping(
            mm,
            addr,
            memory,
            0,
            length,
            flags,
            options.contains(MappingOptions::POPULATE),
            name,
            MappingMode::Eager,
            released_mappings,
        )
    }

    fn any_ranges_lazy<I>(&self, ranges: I) -> bool
    where
        I: IntoIterator<Item = (UserAddress, Option<usize>)>,
    {
        for (addr, length) in ranges {
            match length {
                None => {
                    if let Some((_, mapping)) = self.mappings.get(addr) {
                        if mapping.mapping_mode() == MappingMode::Lazy {
                            return true;
                        }
                    }
                }
                Some(len) => {
                    assert!(len > 0);
                    let end = addr.checked_add(len).expect("address overflowed after validation");
                    if self
                        .mappings
                        .range(addr..end)
                        .any(|(_, mapping)| mapping.mapping_mode() == MappingMode::Lazy)
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn ensure_range_mapped_in_user_vmar(
        &mut self,
        addr: UserAddress,
        length: Option<usize>,
        context: &MappingContext,
    ) -> Result<bool, Errno> {
        self.ensure_ranges_mapped_in_user_vmar(std::iter::once((addr, length)), context)
    }

    fn ensure_ranges_mapped_in_user_vmar<I>(
        &mut self,
        ranges: I,
        context: &MappingContext,
    ) -> Result<bool, Errno>
    where
        I: IntoIterator<Item = (UserAddress, Option<usize>)>,
    {
        // This is most likely to contain one range, so use `SmallVec` to avoid
        // heap allocation and better performance in the common case.
        let mut ranges_to_update = SmallVec::<[std::ops::Range<UserAddress>; 1]>::new();
        for (addr, length) in ranges {
            match length {
                None => {
                    if let Some((range, mapping)) = self.mappings.get(addr) {
                        if mapping.mapping_mode() == MappingMode::Lazy {
                            ranges_to_update.push(range.clone());
                        }
                    }
                }
                Some(len) => {
                    assert!(len > 0);
                    let end = addr.checked_add(len).expect("address overflowed after validation");
                    for (range, mapping) in self.mappings.range(addr..end) {
                        if mapping.mapping_mode() == MappingMode::Lazy {
                            ranges_to_update.push(range.clone());
                        }
                    }
                }
            }
        }

        if ranges_to_update.is_empty() {
            return Ok(false);
        }

        for range in ranges_to_update {
            let updated = self.mappings.update_exact(&range, |mapping| {
                let addr = SelectedAddress::FixedOverwrite(range.start);
                let flags = mapping.flags();
                let (backing, backing_memory_offset) = match mapping.get_backing_internal() {
                    MappingBacking::Memory(backing) => {
                        (backing.memory(), backing.address_to_offset(addr.addr()))
                    }
                    MappingBacking::PrivateAnonymous => {
                        (&context.private_anonymous.backing, addr.addr().ptr() as u64)
                    }
                };

                let mapping_length = range.end - range.start;
                context.map_in_user_vmar(
                    addr,
                    backing,
                    backing_memory_offset,
                    mapping_length,
                    flags,
                    false,
                )?;

                mapping.set_mapping_mode(MappingMode::Eager);
                Ok(())
            })?;
            assert!(updated, "Expected to update exactly one mapping");
        }

        Ok(true)
    }

    fn remap(
        &mut self,
        _current_task: &CurrentTask,
        mm: &Arc<MemoryManager>,
        old_addr: UserAddress,
        old_length: usize,
        new_length: usize,
        flags: MremapFlags,
        new_addr: UserAddress,
        released_mappings: &mut ReleasedMappings,
    ) -> Result<UserAddress, Errno> {
        // MREMAP_FIXED moves a mapping, which requires MREMAP_MAYMOVE.
        if flags.contains(MremapFlags::FIXED) && !flags.contains(MremapFlags::MAYMOVE) {
            return error!(EINVAL);
        }

        // MREMAP_DONTUNMAP is always a move, so it requires MREMAP_MAYMOVE.
        // There is no resizing allowed either.
        if flags.contains(MremapFlags::DONTUNMAP)
            && (!flags.contains(MremapFlags::MAYMOVE) || old_length != new_length)
        {
            return error!(EINVAL);
        }

        if new_length == 0 {
            return error!(EINVAL);
        }

        // Make sure old_addr is page-aligned.
        if !old_addr.is_aligned(*PAGE_SIZE) {
            return error!(EINVAL);
        }

        let old_length = round_up_to_system_page_size(old_length)?;
        let new_length = round_up_to_system_page_size(new_length)?;

        // Make sure old_addr is mapped.
        if self.mappings.get(old_addr).is_none() {
            return error!(EFAULT);
        }

        // In-place copies are invalid.
        if !flags.contains(MremapFlags::MAYMOVE) && old_length == 0 {
            return error!(ENOMEM);
        }

        if self.check_has_unauthorized_splits(old_addr, old_length) {
            return error!(EINVAL);
        }

        if self.check_has_unauthorized_splits(new_addr, new_length) {
            return error!(EINVAL);
        }

        if !flags.contains(MremapFlags::DONTUNMAP)
            && !flags.contains(MremapFlags::FIXED)
            && old_length != 0
        {
            // We are not requested to remap to a specific address, so first we see if we can remap
            // in-place. In-place copies (old_length == 0) are not allowed.
            if let Some(new_addr) =
                self.try_remap_in_place(mm, old_addr, old_length, new_length, released_mappings)?
            {
                return Ok(new_addr);
            }
        }

        // There is no space to grow in place, or there is an explicit request to move.
        if flags.contains(MremapFlags::MAYMOVE) {
            let dst_address =
                if flags.contains(MremapFlags::FIXED) { Some(new_addr) } else { None };
            self.remap_move(
                mm,
                old_addr,
                old_length,
                dst_address,
                new_length,
                flags.contains(MremapFlags::DONTUNMAP),
                released_mappings,
            )
        } else {
            error!(ENOMEM)
        }
    }

    /// Attempts to grow or shrink the mapping in-place. Returns `Ok(Some(addr))` if the remap was
    /// successful. Returns `Ok(None)` if there was no space to grow.
    fn try_remap_in_place(
        &mut self,
        mm: &Arc<MemoryManager>,
        old_addr: UserAddress,
        old_length: usize,
        new_length: usize,
        released_mappings: &mut ReleasedMappings,
    ) -> Result<Option<UserAddress>, Errno> {
        let old_range = old_addr..old_addr.checked_add(old_length).ok_or_else(|| errno!(EINVAL))?;
        let new_range_in_place =
            old_addr..old_addr.checked_add(new_length).ok_or_else(|| errno!(EINVAL))?;

        if new_length <= old_length {
            // Shrink the mapping in-place, which should always succeed.
            // This is done by unmapping the extraneous region.
            if new_length != old_length {
                self.unmap(mm, new_range_in_place.end, old_length - new_length, released_mappings)?;
            }
            return Ok(Some(old_addr));
        }

        if self.mappings.range(old_range.end..new_range_in_place.end).next().is_some() {
            // There is some mapping in the growth range prevening an in-place growth.
            return Ok(None);
        }

        // There is space to grow in-place. The old range must be one contiguous mapping.
        let (original_range, mapping) =
            self.mappings.get(old_addr).ok_or_else(|| errno!(EFAULT))?;

        if old_range.end > original_range.end {
            return error!(EFAULT);
        }

        if mapping.flags().contains(MappingFlags::DONT_EXPAND) {
            return error!(EFAULT);
        }

        let original_range = original_range.clone();
        let original_mapping = mapping.clone();

        // Compute the new length of the entire mapping once it has grown.
        let final_length = (original_range.end - original_range.start) + (new_length - old_length);

        match self.get_mapping_backing(&original_mapping) {
            MappingBacking::Memory(backing) => {
                // Re-map the original range, which may include pages before the requested range.
                Ok(Some(self.add_memory_mapping(
                    mm,
                    DesiredAddress::FixedOverwrite(original_range.start),
                    backing.memory().clone(),
                    backing.address_to_offset(original_range.start),
                    final_length,
                    original_mapping.flags(),
                    false,
                    original_mapping.name().to_owned(),
                    original_mapping.mapping_mode(),
                    released_mappings,
                )?))
            }
            MappingBacking::PrivateAnonymous => {
                let growth_start = original_range.end;
                let growth_length = new_length - old_length;
                let final_end = (original_range.start + final_length)?;
                // Map new pages to back the growth.
                mm.mapping_context.map_in_user_vmar(
                    SelectedAddress::FixedOverwrite(growth_start),
                    &mm.mapping_context.private_anonymous.backing,
                    growth_start.ptr() as u64,
                    growth_length,
                    original_mapping.flags(),
                    false,
                )?;
                // Overwrite the mapping entry with the new larger size.
                released_mappings.extend(
                    self.mappings.insert(original_range.start..final_end, original_mapping.clone()),
                );
                Ok(Some(original_range.start))
            }
        }
    }

    /// Grows or shrinks the mapping while moving it to a new destination.
    fn remap_move(
        &mut self,
        mm: &Arc<MemoryManager>,
        src_addr: UserAddress,
        src_length: usize,
        dst_addr: Option<UserAddress>,
        dst_length: usize,
        keep_source: bool,
        released_mappings: &mut ReleasedMappings,
    ) -> Result<UserAddress, Errno> {
        let src_range = src_addr..src_addr.checked_add(src_length).ok_or_else(|| errno!(EINVAL))?;
        let (original_range, src_mapping) =
            self.mappings.get(src_addr).ok_or_else(|| errno!(EFAULT))?;
        let original_range = original_range.clone();
        let src_mapping = src_mapping.clone();

        if src_length == 0 && !src_mapping.flags().contains(MappingFlags::SHARED) {
            // src_length == 0 means that the mapping is to be copied. This behavior is only valid
            // with MAP_SHARED mappings.
            return error!(EINVAL);
        }

        // If the destination range is smaller than the source range, we must first shrink
        // the source range in place. This must be done now and visible to processes, even if
        // a later failure causes the remap operation to fail.
        if src_length != 0 && src_length > dst_length {
            self.unmap(mm, (src_addr + dst_length)?, src_length - dst_length, released_mappings)?;
        }

        let dst_addr_for_map = match dst_addr {
            None => DesiredAddress::Any,
            Some(dst_addr) => {
                // The mapping is being moved to a specific address.
                let dst_range =
                    dst_addr..(dst_addr.checked_add(dst_length).ok_or_else(|| errno!(EINVAL))?);
                if !src_range.intersect(&dst_range).is_empty() {
                    return error!(EINVAL);
                }

                // The destination range must be unmapped. This must be done now and visible to
                // processes, even if a later failure causes the remap operation to fail.
                self.unmap(mm, dst_addr, dst_length, released_mappings)?;

                DesiredAddress::Fixed(dst_addr)
            }
        };

        // According to gVisor's aio_test, Linux checks for DONT_EXPAND after unmapping the dst
        // range.
        if dst_length > src_length && src_mapping.flags().contains(MappingFlags::DONT_EXPAND) {
            return error!(EFAULT);
        }

        if src_range.end > original_range.end {
            // The source range is not one contiguous mapping. This check must be done only after
            // the source range is shrunk and the destination unmapped.
            return error!(EFAULT);
        }

        match self.get_mapping_backing(&src_mapping) {
            MappingBacking::PrivateAnonymous => {
                let dst_addr =
                    self.select_address(dst_addr_for_map, dst_length, src_mapping.flags())?.addr();
                let dst_end = (dst_addr + dst_length)?;

                let length_to_move = std::cmp::min(dst_length, src_length) as u64;
                let growth_start_addr = (dst_addr + length_to_move)?;

                if dst_addr != src_addr {
                    let src_move_end = (src_range.start + length_to_move)?;
                    let range_to_move = src_range.start..src_move_end;
                    // Move the previously mapped pages into their new location.
                    mm.mapping_context.private_anonymous.move_pages(&range_to_move, dst_addr)?;
                }

                // Userfault registration is not preserved by remap
                let new_flags =
                    src_mapping.flags().difference(MappingFlags::UFFD | MappingFlags::UFFD_MISSING);
                if src_mapping.mapping_mode() == MappingMode::Eager {
                    mm.mapping_context.map_in_user_vmar(
                        SelectedAddress::FixedOverwrite(dst_addr),
                        &mm.mapping_context.private_anonymous.backing,
                        dst_addr.ptr() as u64,
                        dst_length,
                        new_flags,
                        false,
                    )?;

                    if dst_length > src_length {
                        // The mapping has grown, map new pages in to cover the growth.
                        let growth_length = dst_length - src_length;

                        self.map_private_anonymous(
                            mm,
                            DesiredAddress::FixedOverwrite(growth_start_addr),
                            growth_length,
                            new_flags.access_flags(),
                            new_flags.options(),
                            false,
                            src_mapping.name().to_owned(),
                            released_mappings,
                        )?;
                    }
                }

                released_mappings.extend(self.mappings.insert(
                    dst_addr..dst_end,
                    Mapping::new_private_anonymous(
                        new_flags,
                        src_mapping.name().to_owned(),
                        src_mapping.mapping_mode(),
                    ),
                ));

                if dst_addr != src_addr && src_length != 0 && !keep_source {
                    self.unmap(mm, src_addr, src_length, released_mappings)?;
                }

                return Ok(dst_addr);
            }
            MappingBacking::Memory(backing) => {
                // This mapping is backed by an FD or is a shared anonymous mapping. Just map the
                // range of the memory object covering the moved pages. If the memory object already
                // had COW semantics, this preserves them.
                let (dst_memory_offset, memory) =
                    (backing.address_to_offset(src_addr), backing.memory().clone());

                let new_address = self.add_memory_mapping(
                    mm,
                    dst_addr_for_map,
                    memory,
                    dst_memory_offset,
                    dst_length,
                    src_mapping.flags(),
                    false,
                    src_mapping.name().to_owned(),
                    src_mapping.mapping_mode(),
                    released_mappings,
                )?;

                if src_length != 0 && !keep_source {
                    // Only unmap the source range if this is not a copy and if there was not a specific
                    // request to not unmap. It was checked earlier that in case of src_length == 0
                    // this mapping is MAP_SHARED.
                    self.unmap(mm, src_addr, src_length, released_mappings)?;
                }

                return Ok(new_address);
            }
        };
    }

    // Checks if an operation may be performed over the target mapping that may
    // result in a split mapping.
    //
    // An operation may be forbidden if the target mapping only partially covers
    // an existing mapping with the `MappingOptions::DONT_SPLIT` flag set.
    fn check_has_unauthorized_splits(&self, addr: UserAddress, length: usize) -> bool {
        let query_range = addr..addr.saturating_add(length);
        let mut intersection = self.mappings.range(query_range.clone());

        // A mapping is not OK if it disallows splitting and the target range
        // does not fully cover the mapping range.
        let check_if_mapping_has_unauthorized_split =
            |mapping: Option<(&Range<UserAddress>, &Mapping)>| {
                mapping.is_some_and(|(mapping_range, mapping)| {
                    mapping.flags().contains(MappingFlags::DONT_SPLIT)
                        && (mapping_range.start < query_range.start
                            || query_range.end < mapping_range.end)
                })
            };

        // We only check the first and last mappings in the range because naturally,
        // the mappings in the middle are fully covered by the target mapping and
        // won't be split.
        check_if_mapping_has_unauthorized_split(intersection.next())
            || check_if_mapping_has_unauthorized_split(intersection.next_back())
    }

    /// Unmaps the specified range. Unmapped mappings are placed in `released_mappings`.
    fn unmap(
        &mut self,
        mm: &Arc<MemoryManager>,
        addr: UserAddress,
        length: usize,
        released_mappings: &mut ReleasedMappings,
    ) -> Result<(), Errno> {
        if !addr.is_aligned(*PAGE_SIZE) {
            return error!(EINVAL);
        }
        let length = round_up_to_system_page_size(length)?;
        if length == 0 {
            return error!(EINVAL);
        }

        if self.check_has_unauthorized_splits(addr, length) {
            return error!(EINVAL);
        }

        // Unmap the range, including the the tail of any range that would have been split. This
        // operation is safe because we're operating on another process.
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        match unsafe { mm.mapping_context.user_vmar.unmap(addr.ptr(), length) } {
            Ok(_) => (),
            Err(zx::Status::NOT_FOUND) => (),
            Err(zx::Status::INVALID_ARGS) => return error!(EINVAL),
            Err(status) => {
                impossible_error(status);
            }
        };

        self.update_after_unmap(mm, addr, length, released_mappings);

        Ok(())
    }

    // Updates `self.mappings` after the specified range was unmaped.
    //
    // The range to unmap can span multiple mappings, and can split mappings if
    // the range start or end falls in the middle of a mapping.
    //
    // Private anonymous memory is contained in the same memory object; The pages of that object
    // that are no longer reachable should be released.
    //
    // File-backed mappings don't need to have their memory object modified.
    //
    // Unmapped mappings are placed in `released_mappings`.
    fn update_after_unmap(
        &mut self,
        mm: &Arc<MemoryManager>,
        addr: UserAddress,
        length: usize,
        released_mappings: &mut ReleasedMappings,
    ) {
        let end_addr = addr.checked_add(length).expect("address overflow during unmap");
        let unmap_range = addr..end_addr;

        // Remove any shadow mappings for mlock()'d pages that are now unmapped.
        released_mappings.extend_pins(self.shadow_mappings_for_mlock.remove(unmap_range.clone()));

        for (range, mapping) in self.mappings.range(unmap_range.clone()) {
            // Deallocate any pages in the private, anonymous backing that are now unreachable.
            if let MappingBacking::PrivateAnonymous = self.get_mapping_backing(mapping) {
                let unmapped_range = &unmap_range.intersect(range);

                if let Err(e) = mm
                    .inflight_vmspliced_payloads
                    .handle_unmapping(&mm.mapping_context.private_anonymous.backing, unmapped_range)
                {
                    log_error!("Failed to handle vmsplice unmapping: {:?}", e);
                }

                if let Err(e) = mm
                    .mapping_context
                    .private_anonymous
                    .zero(unmapped_range.start, unmapped_range.end - unmapped_range.start)
                {
                    log_error!("Failed to zero private anonymous memory: {:?}", e);
                }
            }
        }
        released_mappings.extend(self.mappings.remove(unmap_range));
    }

    fn protect(
        &mut self,
        current_task: &CurrentTask,
        addr: UserAddress,
        length: usize,
        prot_flags: ProtectionFlags,
        released_mappings: &mut ReleasedMappings,
    ) -> Result<(), Errno> {
        let vmar_flags = prot_flags.to_vmar_flags();
        let page_size = *PAGE_SIZE;
        let end = addr.checked_add(length).ok_or_else(|| errno!(EINVAL))?.round_up(page_size)?;

        if self.check_has_unauthorized_splits(addr, length) {
            return error!(EINVAL);
        }

        let prot_range = if prot_flags.contains(ProtectionFlags::GROWSDOWN) {
            let mut start = addr;
            let Some((range, mapping)) = self.mappings.get(start) else {
                return error!(EINVAL);
            };
            // Ensure that the mapping has GROWSDOWN if PROT_GROWSDOWN was specified.
            if !mapping.flags().contains(MappingFlags::GROWSDOWN) {
                return error!(EINVAL);
            }
            let access_flags = mapping.flags().access_flags();
            // From <https://man7.org/linux/man-pages/man2/mprotect.2.html>:
            //
            //   PROT_GROWSDOWN
            //     Apply the protection mode down to the beginning of a
            //     mapping that grows downward (which should be a stack
            //     segment or a segment mapped with the MAP_GROWSDOWN flag
            //     set).
            start = range.start;
            while let Some((range, mapping)) =
                self.mappings.get(start.saturating_sub(page_size as usize))
            {
                if !mapping.flags().contains(MappingFlags::GROWSDOWN)
                    || mapping.flags().access_flags() != access_flags
                {
                    break;
                }
                start = range.start;
            }
            start..end
        } else {
            addr..end
        };

        let mut range_list = vec![];
        let mapping_context = &current_task.mm()?.mapping_context;
        let length = prot_range.end - prot_range.start;
        self.ensure_range_mapped_in_user_vmar(prot_range.start, Some(length), mapping_context)?;

        for (range, mapping) in self.mappings.range(prot_range.clone()) {
            range_list.push((range.clone(), mapping.clone()));
        }

        let mut start_cursor = prot_range.start;
        let mut updates = vec![];
        let mut final_result = Ok(());

        for (range, mapping) in range_list {
            if range.start > start_cursor {
                final_result = error!(ENOMEM);
                break;
            }

            let intersection = range.intersect(&prot_range);
            if let Err(e) =
                security::file_mprotect(current_task, &intersection, &mapping, prot_flags)
            {
                final_result = Err(e);
                break;
            }

            if mapping.flags().contains(MappingFlags::UFFD) {
                track_stub!(
                    TODO("https://fxbug.dev/297375964"),
                    "mprotect on uffd-registered range should not alter protections"
                );
                final_result = error!(EINVAL);
                break;
            }

            let mapped_len = intersection.end - intersection.start;

            // SAFETY: This is safe because the vmar belongs to a different process.
            let protect_result = unsafe {
                mapping_context.user_vmar.protect(intersection.start.ptr(), mapped_len, vmar_flags)
            }
            .map_err(|s| match s {
                zx::Status::INVALID_ARGS => errno!(EINVAL),
                zx::Status::NOT_FOUND => errno!(ENOMEM),
                zx::Status::ACCESS_DENIED => errno!(EACCES),
                _ => impossible_error(s),
            });

            if let Err(e) = protect_result {
                final_result = Err(e);
                break;
            }

            let mut new_mapping = mapping;
            new_mapping.set_flags(new_mapping.flags().with_access_flags(prot_flags));
            let push_range = intersection.clone();
            start_cursor = intersection.end;
            updates.push((push_range, new_mapping));
        }

        if final_result.is_ok() && start_cursor < prot_range.end {
            final_result = error!(ENOMEM);
        }

        for (r, m) in updates {
            released_mappings.extend(self.mappings.insert(r, m));
        }

        final_result
    }

    fn madvise(
        &mut self,
        context: &MappingContext,
        addr: UserAddress,
        length: usize,
        advice: u32,
        released_mappings: &mut ReleasedMappings,
    ) -> Result<(), Errno> {
        if !addr.is_aligned(*PAGE_SIZE) {
            return error!(EINVAL);
        }

        let end_addr =
            addr.checked_add(length).ok_or_else(|| errno!(EINVAL))?.round_up(*PAGE_SIZE)?;
        if end_addr > context.max_address() {
            return error!(EFAULT);
        }

        if advice == MADV_NORMAL {
            track_stub!(TODO("https://fxbug.dev/322874202"), "madvise undo hints for MADV_NORMAL");
            return Ok(());
        }

        let mut updates = vec![];
        let range_for_op = addr..end_addr;
        for (range, mapping) in self.mappings.range(range_for_op.clone()) {
            let range_to_zero = range.intersect(&range_for_op);
            if range_to_zero.is_empty() {
                continue;
            }
            let start_offset = mapping.address_to_offset(range_to_zero.start);
            let end_offset = mapping.address_to_offset(range_to_zero.end);
            if advice == MADV_DONTFORK
                || advice == MADV_DOFORK
                || advice == MADV_WIPEONFORK
                || advice == MADV_KEEPONFORK
                || advice == MADV_DONTDUMP
                || advice == MADV_DODUMP
                || advice == MADV_MERGEABLE
                || advice == MADV_UNMERGEABLE
            {
                // WIPEONFORK is only supported on private anonymous mappings per madvise(2).
                // KEEPONFORK can be specified on ranges that cover other sorts of mappings. It should
                // have no effect on mappings that are not private and anonymous as such mappings cannot
                // have the WIPEONFORK option set.
                if advice == MADV_WIPEONFORK && !mapping.private_anonymous() {
                    return error!(EINVAL);
                }
                let new_flags = match advice {
                    MADV_DONTFORK => mapping.flags() | MappingFlags::DONTFORK,
                    MADV_DOFORK => mapping.flags() & MappingFlags::DONTFORK.complement(),
                    MADV_WIPEONFORK => mapping.flags() | MappingFlags::WIPEONFORK,
                    MADV_KEEPONFORK => mapping.flags() & MappingFlags::WIPEONFORK.complement(),
                    MADV_DONTDUMP => {
                        track_stub!(TODO("https://fxbug.dev/322874202"), "MADV_DONTDUMP");
                        mapping.flags()
                    }
                    MADV_DODUMP => {
                        track_stub!(TODO("https://fxbug.dev/322874202"), "MADV_DODUMP");
                        mapping.flags()
                    }
                    MADV_MERGEABLE => {
                        track_stub!(TODO("https://fxbug.dev/322874202"), "MADV_MERGEABLE");
                        mapping.flags()
                    }
                    MADV_UNMERGEABLE => {
                        track_stub!(TODO("https://fxbug.dev/322874202"), "MADV_UNMERGEABLE");
                        mapping.flags()
                    }
                    // Only the variants in this match should be reachable given the condition for
                    // the containing branch.
                    unknown_advice => unreachable!("unknown advice {unknown_advice}"),
                };
                let mut new_mapping = mapping.clone();
                new_mapping.set_flags(new_flags);
                updates.push((range_to_zero, new_mapping));
            } else {
                if mapping.flags().contains(MappingFlags::SHARED) {
                    continue;
                }
                let op = match advice {
                    MADV_DONTNEED if !mapping.flags().contains(MappingFlags::ANONYMOUS) => {
                        // Note, we cannot simply implemented MADV_DONTNEED with
                        // zx::VmoOp::DONT_NEED because they have different
                        // semantics.
                        track_stub!(
                            TODO("https://fxbug.dev/322874496"),
                            "MADV_DONTNEED with file-backed mapping"
                        );
                        return Ok(());
                    }
                    MADV_DONTNEED if mapping.flags().contains(MappingFlags::LOCKED) => {
                        return error!(EINVAL);
                    }
                    MADV_DONTNEED => zx::VmoOp::ZERO,
                    MADV_DONTNEED_LOCKED => {
                        track_stub!(TODO("https://fxbug.dev/322874202"), "MADV_DONTNEED_LOCKED");
                        return error!(EINVAL);
                    }
                    MADV_WILLNEED => {
                        if mapping.flags().contains(MappingFlags::WRITE) {
                            zx::VmoOp::COMMIT
                        } else {
                            zx::VmoOp::PREFETCH
                        }
                    }
                    MADV_COLD => {
                        track_stub!(TODO("https://fxbug.dev/322874202"), "MADV_COLD");
                        return error!(EINVAL);
                    }
                    MADV_PAGEOUT => {
                        track_stub!(TODO("https://fxbug.dev/322874202"), "MADV_PAGEOUT");
                        return error!(EINVAL);
                    }
                    MADV_POPULATE_READ => {
                        track_stub!(TODO("https://fxbug.dev/322874202"), "MADV_POPULATE_READ");
                        return error!(EINVAL);
                    }
                    MADV_RANDOM => {
                        track_stub!(TODO("https://fxbug.dev/322874202"), "MADV_RANDOM");
                        return error!(EINVAL);
                    }
                    MADV_SEQUENTIAL => {
                        track_stub!(TODO("https://fxbug.dev/322874202"), "MADV_SEQUENTIAL");
                        return error!(EINVAL);
                    }
                    MADV_FREE if !mapping.flags().contains(MappingFlags::ANONYMOUS) => {
                        track_stub!(
                            TODO("https://fxbug.dev/411748419"),
                            "MADV_FREE with file-backed mapping"
                        );
                        return error!(EINVAL);
                    }
                    MADV_FREE if mapping.flags().contains(MappingFlags::LOCKED) => {
                        return error!(EINVAL);
                    }
                    MADV_FREE => {
                        track_stub!(TODO("https://fxbug.dev/411748419"), "MADV_FREE");
                        // TODO(https://fxbug.dev/411748419) For now, treat MADV_FREE like
                        // MADV_DONTNEED as a stopgap until we have proper support.
                        zx::VmoOp::ZERO
                    }
                    MADV_REMOVE => {
                        track_stub!(TODO("https://fxbug.dev/322874202"), "MADV_REMOVE");
                        return error!(EINVAL);
                    }
                    MADV_HWPOISON => {
                        track_stub!(TODO("https://fxbug.dev/322874202"), "MADV_HWPOISON");
                        return error!(EINVAL);
                    }
                    MADV_SOFT_OFFLINE => {
                        track_stub!(TODO("https://fxbug.dev/322874202"), "MADV_SOFT_OFFLINE");
                        return error!(EINVAL);
                    }
                    MADV_HUGEPAGE => {
                        track_stub!(TODO("https://fxbug.dev/322874202"), "MADV_HUGEPAGE");
                        return error!(EINVAL);
                    }
                    MADV_COLLAPSE => {
                        track_stub!(TODO("https://fxbug.dev/322874202"), "MADV_COLLAPSE");
                        return error!(EINVAL);
                    }
                    MADV_NOHUGEPAGE => return Ok(()),
                    advice => {
                        track_stub!(TODO("https://fxbug.dev/322874202"), "madvise", advice);
                        return error!(EINVAL);
                    }
                };

                let memory = match self.get_mapping_backing(mapping) {
                    MappingBacking::Memory(backing) => backing.memory(),
                    MappingBacking::PrivateAnonymous => &context.private_anonymous.backing,
                };
                memory.op_range(op, start_offset, end_offset - start_offset).map_err(
                    |s| match s {
                        zx::Status::OUT_OF_RANGE => errno!(EINVAL),
                        zx::Status::NO_MEMORY => errno!(ENOMEM),
                        zx::Status::INVALID_ARGS => errno!(EINVAL),
                        zx::Status::ACCESS_DENIED => errno!(EACCES),
                        _ => impossible_error(s),
                    },
                )?;
            }
        }
        // Use a separate loop to avoid mutating the mappings structure while iterating over it.
        for (range, mapping) in updates {
            released_mappings.extend(self.mappings.insert(range, mapping));
        }
        Ok(())
    }

    fn mlock(
        &mut self,
        context: &MappingContext,
        current_task: &CurrentTask,
        desired_addr: UserAddress,
        desired_length: usize,
        on_fault: bool,
        released_mappings: &mut ReleasedMappings,
    ) -> Result<(), Errno> {
        let desired_end_addr =
            desired_addr.checked_add(desired_length).ok_or_else(|| errno!(EINVAL))?;
        let start_addr = round_down_to_system_page_size(desired_addr)?;
        let end_addr = round_up_to_system_page_size(desired_end_addr)?;

        let mut updates = vec![];
        let mut bytes_mapped_in_range = 0;
        let mut num_new_locked_bytes = 0;
        let mut failed_to_lock = false;
        for (range, mapping) in self.mappings.range(start_addr..end_addr) {
            let mut range = range.clone();
            let mut mapping = mapping.clone();

            // Handle mappings that start before the region to be locked.
            range.start = std::cmp::max(range.start, start_addr);
            // Handle mappings that extend past the region to be locked.
            range.end = std::cmp::min(range.end, end_addr);

            bytes_mapped_in_range += (range.end - range.start) as u64;

            // PROT_NONE mappings generate ENOMEM but are left locked.
            if !mapping
                .flags()
                .intersects(MappingFlags::READ | MappingFlags::WRITE | MappingFlags::EXEC)
            {
                failed_to_lock = true;
            }

            if !mapping.flags().contains(MappingFlags::LOCKED) {
                num_new_locked_bytes += (range.end - range.start) as u64;
                let shadow_mapping = match current_task.kernel().features.mlock_pin_flavor {
                    // Pin the memory by mapping the backing memory into the high priority vmar.
                    MlockPinFlavor::ShadowProcess => {
                        let shadow_process =
                            current_task.kernel().expando.get_or_try_init(|| {
                                memory_pinning::ShadowProcess::new(zx::Name::new_lossy(
                                    "starnix_mlock_pins",
                                ))
                                .map(MlockShadowProcess)
                                .map_err(|_| errno!(EPERM))
                            })?;

                        let (vmo, offset) = match self.get_mapping_backing(&mapping) {
                            MappingBacking::Memory(m) => (
                                m.memory().as_vmo().ok_or_else(|| errno!(ENOMEM))?,
                                m.address_to_offset(range.start),
                            ),
                            MappingBacking::PrivateAnonymous => (
                                context
                                    .private_anonymous
                                    .backing
                                    .as_vmo()
                                    .ok_or_else(|| errno!(ENOMEM))?,
                                range.start.ptr() as u64,
                            ),
                        };
                        Some(shadow_process.0.pin_pages(vmo, offset, range.end - range.start)?)
                    }

                    // Relying on VMAR-level operations means just flags are set per-mapping.
                    MlockPinFlavor::Noop | MlockPinFlavor::VmarAlwaysNeed => None,
                };
                mapping.set_mlock();
                updates.push((range, mapping, shadow_mapping));
            }
        }

        if bytes_mapped_in_range as usize != end_addr - start_addr {
            return error!(ENOMEM);
        }

        let memlock_rlimit = current_task.thread_group().get_rlimit(Resource::MEMLOCK);
        let total_locked = self.num_locked_bytes(
            UserAddress::from(context.user_vmar_info.base as u64)
                ..UserAddress::from(
                    (context.user_vmar_info.base + context.user_vmar_info.len) as u64,
                ),
        );
        if total_locked + num_new_locked_bytes > memlock_rlimit {
            if crate::security::check_task_capable(current_task, CAP_IPC_LOCK).is_err() {
                let code = if memlock_rlimit > 0 { errno!(ENOMEM) } else { errno!(EPERM) };
                return Err(code);
            }
        }

        let op_range_status_to_errno = |e| match e {
            zx::Status::BAD_STATE | zx::Status::NOT_SUPPORTED => errno!(ENOMEM),
            zx::Status::INVALID_ARGS | zx::Status::OUT_OF_RANGE => errno!(EINVAL),
            zx::Status::ACCESS_DENIED => {
                unreachable!("user vmar should always have needed rights")
            }
            zx::Status::BAD_HANDLE => {
                unreachable!("user vmar should always be a valid handle")
            }
            zx::Status::WRONG_TYPE => unreachable!("user vmar handle should be a vmar"),
            _ => unreachable!("unknown error from op_range on user vmar for mlock: {e}"),
        };

        self.ensure_range_mapped_in_user_vmar(start_addr, Some(end_addr - start_addr), context)?;

        if !on_fault && !current_task.kernel().features.mlock_always_onfault {
            context
                .user_vmar
                .op_range(zx::VmarOp::PREFETCH, start_addr.ptr(), end_addr - start_addr)
                .map_err(op_range_status_to_errno)?;
        }

        match current_task.kernel().features.mlock_pin_flavor {
            MlockPinFlavor::VmarAlwaysNeed => {
                context
                    .user_vmar
                    .op_range(zx::VmarOp::ALWAYS_NEED, start_addr.ptr(), end_addr - start_addr)
                    .map_err(op_range_status_to_errno)?;
            }
            // The shadow process doesn't use any vmar-level operations to pin memory.
            MlockPinFlavor::Noop | MlockPinFlavor::ShadowProcess => (),
        }

        for (range, mapping, shadow_mapping) in updates {
            if let Some(shadow_mapping) = shadow_mapping {
                released_mappings.extend_pins(
                    self.shadow_mappings_for_mlock.insert(range.clone(), shadow_mapping),
                );
            }
            released_mappings.extend(self.mappings.insert(range, mapping));
        }

        if failed_to_lock { error!(ENOMEM) } else { Ok(()) }
    }

    fn munlock(
        &mut self,
        _current_task: &CurrentTask,
        desired_addr: UserAddress,
        desired_length: usize,
        released_mappings: &mut ReleasedMappings,
    ) -> Result<(), Errno> {
        let desired_end_addr =
            desired_addr.checked_add(desired_length).ok_or_else(|| errno!(EINVAL))?;
        let start_addr = round_down_to_system_page_size(desired_addr)?;
        let end_addr = round_up_to_system_page_size(desired_end_addr)?;

        let mut updates = vec![];
        let mut bytes_mapped_in_range = 0;
        for (range, mapping) in self.mappings.range(start_addr..end_addr) {
            let mut range = range.clone();
            let mut mapping = mapping.clone();

            // Handle mappings that start before the region to be locked.
            range.start = std::cmp::max(range.start, start_addr);
            // Handle mappings that extend past the region to be locked.
            range.end = std::cmp::min(range.end, end_addr);

            bytes_mapped_in_range += (range.end - range.start) as u64;

            if mapping.flags().contains(MappingFlags::LOCKED) {
                // This clears the locking for the shadow process pin flavor. It's not currently
                // possible to actually unlock pages that were locked with the
                // ZX_VMAR_OP_ALWAYS_NEED pin flavor.
                mapping.clear_mlock();
                updates.push((range, mapping));
            }
        }

        if bytes_mapped_in_range as usize != end_addr - start_addr {
            return error!(ENOMEM);
        }

        for (range, mapping) in updates {
            released_mappings.extend(self.mappings.insert(range.clone(), mapping));
            released_mappings.extend_pins(self.shadow_mappings_for_mlock.remove(range));
        }

        Ok(())
    }

    pub fn num_locked_bytes(&self, range: impl RangeBounds<UserAddress>) -> u64 {
        self.mappings
            .map
            .range(range)
            .filter(|(_, mapping)| mapping.flags().contains(MappingFlags::LOCKED))
            .map(|(range, _)| (range.end - range.start) as u64)
            .sum()
    }

    fn get_mappings_for_vmsplice(
        &self,
        mm: &Arc<MemoryManager>,
        buffers: &UserBuffers,
    ) -> Result<Vec<Arc<VmsplicePayload>>, Errno> {
        let mut vmsplice_mappings = Vec::new();

        for UserBuffer { mut address, length } in buffers.iter().copied() {
            let mappings = self.get_contiguous_mappings_at(address, length, &mm.mapping_context)?;
            for (mapping, length) in mappings {
                let vmsplice_payload = match self.get_mapping_backing(mapping) {
                    MappingBacking::Memory(m) => VmsplicePayloadSegment {
                        addr_offset: address,
                        length,
                        memory: m.memory().clone(),
                        memory_offset: m.address_to_offset(address),
                        should_snapshot_on_unmap: false,
                    },
                    MappingBacking::PrivateAnonymous => VmsplicePayloadSegment {
                        addr_offset: address,
                        length,
                        memory: mm.mapping_context.private_anonymous.backing.clone(),
                        memory_offset: address.ptr() as u64,
                        should_snapshot_on_unmap: true,
                    },
                };
                vmsplice_mappings.push(VmsplicePayload::new(Arc::downgrade(mm), vmsplice_payload));

                address = (address + length)?;
            }
        }

        Ok(vmsplice_mappings)
    }

    /// Returns all the mappings starting at `addr`, and continuing until either `length` bytes have
    /// been covered or an unmapped page is reached.
    ///
    /// Mappings are returned in ascending order along with the number of bytes that intersect the
    /// requested range. The returned mappings are guaranteed to be contiguous and the total length
    /// corresponds to the number of contiguous mapped bytes starting from `addr`, i.e.:
    /// - 0 (empty iterator) if `addr` is not mapped.
    /// - exactly `length` if the requested range is fully mapped.
    /// - the offset of the first unmapped page (between 0 and `length`) if the requested range is
    ///   only partially mapped.
    ///
    /// Returns EFAULT if the requested range overflows or extends past the end of the vmar.
    fn get_contiguous_mappings_at(
        &self,
        addr: UserAddress,
        length: usize,
        context: &MappingContext,
    ) -> Result<impl Iterator<Item = (&Mapping, usize)>, Errno> {
        let end_addr = addr.checked_add(length).ok_or_else(|| errno!(EFAULT))?;
        if end_addr > context.max_address() {
            return error!(EFAULT);
        }

        // Iterate over all contiguous mappings intersecting the requested range.
        let mut mappings = self.mappings.range(addr..end_addr);
        let mut prev_range_end = None;
        let mut offset = 0;
        let result = std::iter::from_fn(move || {
            if offset != length {
                if let Some((range, mapping)) = mappings.next() {
                    return match prev_range_end {
                        // If this is the first mapping that we are considering, it may not actually
                        // contain `addr` at all.
                        None if range.start > addr => None,

                        // Subsequent mappings may not be contiguous.
                        Some(prev_range_end) if range.start != prev_range_end => None,

                        // This mapping can be returned.
                        _ => {
                            let mapping_length = std::cmp::min(length, range.end - addr) - offset;
                            offset += mapping_length;
                            prev_range_end = Some(range.end);
                            Some((mapping, mapping_length))
                        }
                    };
                }
            }

            None
        });

        Ok(result)
    }

    /// Determines whether a fault at the given address could be covered by extending a growsdown
    /// mapping.
    ///
    /// If the address already belongs to a mapping, this function returns `None`. If the next
    /// mapping above the given address has the `MappingFlags::GROWSDOWN` flag, this function
    /// returns the address at which that mapping starts and the mapping itself. Otherwise, this
    /// function returns `None`.
    fn find_growsdown_mapping(&self, addr: UserAddress) -> Option<(UserAddress, &Mapping)> {
        match self.mappings.range(addr..).next() {
            Some((range, mapping)) => {
                if range.contains(&addr) {
                    // |addr| is already contained within a mapping, nothing to grow.
                    return None;
                } else if !mapping.flags().contains(MappingFlags::GROWSDOWN) {
                    // The next mapping above the given address does not have the
                    // `MappingFlags::GROWSDOWN` flag.
                    None
                } else {
                    Some((range.start, mapping))
                }
            }
            None => None,
        }
    }

    /// Determines if an access at a given address could be covered by extending a growsdown mapping
    /// and extends it if possible. Returns true if the given address is covered by a mapping.
    fn extend_growsdown_mapping_to_address(
        &mut self,
        mm: &Arc<MemoryManager>,
        addr: UserAddress,
        is_write: bool,
    ) -> Result<bool, Error> {
        let Some((mapping_low_addr, mapping_to_grow)) = self.find_growsdown_mapping(addr) else {
            return Ok(false);
        };
        if is_write && !mapping_to_grow.can_write() {
            // Don't grow a read-only GROWSDOWN mapping for a write fault, it won't work.
            return Ok(false);
        }
        if !mapping_to_grow.flags().contains(MappingFlags::ANONYMOUS) {
            // Currently, we only grow anonymous mappings.
            return Ok(false);
        }
        let low_addr = (addr - (addr.ptr() as u64 % *PAGE_SIZE))?;
        let high_addr = mapping_low_addr;

        let length = high_addr
            .ptr()
            .checked_sub(low_addr.ptr())
            .ok_or_else(|| anyhow!("Invalid growth range"))?;

        let mut released_mappings = ReleasedMappings::default();
        self.map_anonymous(
            mm,
            DesiredAddress::FixedOverwrite(low_addr),
            length,
            mapping_to_grow.flags().access_flags(),
            mapping_to_grow.flags().options(),
            mapping_to_grow.name().to_owned(),
            &mut released_mappings,
        )?;
        // We can't have any released mappings because `find_growsdown_mapping` will return None if
        // the mapping already exists in this range.
        assert!(
            released_mappings.is_empty(),
            "expected to not remove mappings by inserting, got {released_mappings:#?}"
        );
        Ok(true)
    }

    /// Reads exactly `bytes.len()` bytes of memory.
    ///
    /// # Parameters
    /// - `addr`: The address to read data from.
    /// - `bytes`: The byte array to read into.
    fn read_memory<'a>(
        &self,
        addr: UserAddress,
        bytes: &'a mut [MaybeUninit<u8>],
        context: &MappingContext,
    ) -> Result<&'a mut [u8], Errno> {
        let mut bytes_read = 0;
        for (mapping, len) in self.get_contiguous_mappings_at(addr, bytes.len(), context)? {
            let next_offset = bytes_read + len;
            self.read_mapping_memory(
                (addr + bytes_read)?,
                mapping,
                &mut bytes[bytes_read..next_offset],
                context,
            )?;
            bytes_read = next_offset;
        }

        if bytes_read != bytes.len() {
            error!(EFAULT)
        } else {
            // SAFETY: The created slice is properly aligned/sized since it
            // is a subset of the `bytes` slice. Note that `MaybeUninit<T>` has
            // the same layout as `T`. Also note that `bytes_read` bytes have
            // been properly initialized.
            let bytes = unsafe {
                std::slice::from_raw_parts_mut(bytes.as_mut_ptr() as *mut u8, bytes_read)
            };
            Ok(bytes)
        }
    }

    /// Reads exactly `bytes.len()` bytes of memory from `addr`.
    ///
    /// # Parameters
    /// - `addr`: The address to read data from.
    /// - `bytes`: The byte array to read into.
    fn read_mapping_memory<'a>(
        &self,
        addr: UserAddress,
        mapping: &Mapping,
        bytes: &'a mut [MaybeUninit<u8>],
        context: &MappingContext,
    ) -> Result<&'a mut [u8], Errno> {
        if !mapping.can_read() {
            return error!(EFAULT, "read_mapping_memory called on unreadable mapping");
        }
        match self.get_mapping_backing(mapping) {
            MappingBacking::Memory(backing) => backing.read_memory(addr, bytes),
            MappingBacking::PrivateAnonymous => context.private_anonymous.read_memory(addr, bytes),
        }
    }

    /// Reads bytes starting at `addr`, continuing until either `bytes.len()` bytes have been read
    /// or no more bytes can be read.
    ///
    /// This is used, for example, to read null-terminated strings where the exact length is not
    /// known, only the maximum length is.
    ///
    /// # Parameters
    /// - `addr`: The address to read data from.
    /// - `bytes`: The byte array to read into.
    fn read_memory_partial<'a>(
        &self,
        addr: UserAddress,
        bytes: &'a mut [MaybeUninit<u8>],
        context: &MappingContext,
    ) -> Result<&'a mut [u8], Errno> {
        let mut bytes_read = 0;
        for (mapping, len) in self.get_contiguous_mappings_at(addr, bytes.len(), context)? {
            let next_offset = bytes_read + len;
            if self
                .read_mapping_memory(
                    (addr + bytes_read)?,
                    mapping,
                    &mut bytes[bytes_read..next_offset],
                    context,
                )
                .is_err()
            {
                break;
            }
            bytes_read = next_offset;
        }

        // If at least one byte was requested but we got none, it means that `addr` was invalid.
        if !bytes.is_empty() && bytes_read == 0 {
            error!(EFAULT)
        } else {
            // SAFETY: The created slice is properly aligned/sized since it
            // is a subset of the `bytes` slice. Note that `MaybeUninit<T>` has
            // the same layout as `T`. Also note that `bytes_read` bytes have
            // been properly initialized.
            let bytes = unsafe {
                std::slice::from_raw_parts_mut(bytes.as_mut_ptr() as *mut u8, bytes_read)
            };
            Ok(bytes)
        }
    }

    /// Like `read_memory_partial` but only returns the bytes up to and including
    /// a null (zero) byte.
    fn read_memory_partial_until_null_byte<'a>(
        &self,
        addr: UserAddress,
        bytes: &'a mut [MaybeUninit<u8>],
        context: &MappingContext,
    ) -> Result<&'a mut [u8], Errno> {
        let read_bytes = self.read_memory_partial(addr, bytes, context)?;
        let max_len = memchr::memchr(b'\0', read_bytes)
            .map_or_else(|| read_bytes.len(), |null_index| null_index + 1);
        Ok(&mut read_bytes[..max_len])
    }

    /// Writes the provided bytes.
    ///
    /// In case of success, the number of bytes written will always be `bytes.len()`.
    ///
    /// # Parameters
    /// - `addr`: The address to write to.
    /// - `bytes`: The bytes to write.
    fn write_memory(
        &self,
        addr: UserAddress,
        bytes: &[u8],
        context: &MappingContext,
    ) -> Result<usize, Errno> {
        let mut bytes_written = 0;
        for (mapping, len) in self.get_contiguous_mappings_at(addr, bytes.len(), context)? {
            let next_offset = bytes_written + len;
            self.write_mapping_memory(
                (addr + bytes_written)?,
                mapping,
                &bytes[bytes_written..next_offset],
                context,
            )?;
            bytes_written = next_offset;
        }

        if bytes_written != bytes.len() { error!(EFAULT) } else { Ok(bytes.len()) }
    }

    /// Writes the provided bytes to `addr`.
    ///
    /// # Parameters
    /// - `addr`: The address to write to.
    /// - `bytes`: The bytes to write to the memory object.
    fn write_mapping_memory(
        &self,
        addr: UserAddress,
        mapping: &Mapping,
        bytes: &[u8],
        context: &MappingContext,
    ) -> Result<(), Errno> {
        if !mapping.can_write() {
            return error!(EFAULT, "write_mapping_memory called on unwritable memory");
        }
        match self.get_mapping_backing(mapping) {
            MappingBacking::Memory(backing) => backing.write_memory(addr, bytes),
            MappingBacking::PrivateAnonymous => context.private_anonymous.write_memory(addr, bytes),
        }
    }

    /// Writes bytes starting at `addr`, continuing until either `bytes.len()` bytes have been
    /// written or no more bytes can be written.
    ///
    /// # Parameters
    /// - `addr`: The address to read data from.
    /// - `bytes`: The byte array to write from.
    fn write_memory_partial(
        &self,
        addr: UserAddress,
        bytes: &[u8],
        context: &MappingContext,
    ) -> Result<usize, Errno> {
        let mut bytes_written = 0;
        for (mapping, len) in self.get_contiguous_mappings_at(addr, bytes.len(), context)? {
            let next_offset = bytes_written + len;
            if self
                .write_mapping_memory(
                    (addr + bytes_written)?,
                    mapping,
                    &bytes[bytes_written..next_offset],
                    context,
                )
                .is_err()
            {
                break;
            }
            bytes_written = next_offset;
        }

        if !bytes.is_empty() && bytes_written == 0 { error!(EFAULT) } else { Ok(bytes.len()) }
    }

    fn zero(
        &self,
        addr: UserAddress,
        length: usize,
        context: &MappingContext,
    ) -> Result<usize, Errno> {
        let mut bytes_written = 0;
        for (mapping, len) in self.get_contiguous_mappings_at(addr, length, context)? {
            let next_offset = bytes_written + len;
            if self.zero_mapping((addr + bytes_written)?, mapping, len, context).is_err() {
                break;
            }
            bytes_written = next_offset;
        }

        if length != bytes_written { error!(EFAULT) } else { Ok(length) }
    }

    fn zero_mapping(
        &self,
        addr: UserAddress,
        mapping: &Mapping,
        length: usize,
        context: &MappingContext,
    ) -> Result<usize, Errno> {
        if !mapping.can_write() {
            return error!(EFAULT);
        }

        match self.get_mapping_backing(mapping) {
            MappingBacking::Memory(backing) => backing.zero(addr, length),
            MappingBacking::PrivateAnonymous => context.private_anonymous.zero(addr, length),
        }
    }

    pub fn create_memory_backing(
        &self,
        base: UserAddress,
        memory: Arc<MemoryObject>,
        memory_offset: u64,
    ) -> MappingBacking {
        MappingBacking::Memory(Box::new(MappingBackingMemory::new(base, memory, memory_offset)))
    }

    pub fn get_mapping_backing<'a>(&self, mapping: &'a Mapping) -> &'a MappingBacking {
        mapping.get_backing_internal()
    }

    fn get_aio_context(&self, addr: UserAddress) -> Option<(Range<UserAddress>, Arc<AioContext>)> {
        let Some((range, mapping)) = self.mappings.get(addr) else {
            return None;
        };
        let MappingNameRef::AioContext(ref aio_context) = mapping.name() else {
            return None;
        };
        if !mapping.can_read() {
            return None;
        }
        Some((range.clone(), Arc::clone(aio_context)))
    }

    fn find_uffd(&self, addr: UserAddress) -> Option<Arc<UserFault>> {
        for userfault in self.userfaultfds.iter() {
            if let Some(userfault) = userfault.upgrade() {
                if userfault.contains_addr(addr) {
                    return Some(userfault);
                }
            }
        }
        None
    }

    fn cache_flush(
        &self,
        range: Range<UserAddress>,
        context: &MappingContext,
    ) -> Result<(), Errno> {
        let mut addr = range.start;
        let size = range.end - range.start;
        for (mapping, len) in self.get_contiguous_mappings_at(addr, size, context)? {
            if !mapping.can_read() {
                return error!(EFAULT);
            }
            if mapping.mapping_mode() == MappingMode::Lazy {
                addr = (addr + len)?;
                continue;
            }
            // SAFETY: This is operating on a readable restricted mode mapping and will not fault.
            zx::Status::ok(unsafe {
                zx::sys::zx_cache_flush(
                    addr.ptr() as *const u8,
                    len,
                    zx::sys::ZX_CACHE_FLUSH_DATA | zx::sys::ZX_CACHE_FLUSH_INSN,
                )
            })
            .map_err(impossible_error)?;

            addr = (addr + len).unwrap(); // unwrap since we're iterating within the address space.
        }
        // Did we flush the entire range?
        if addr != range.end { error!(EFAULT) } else { Ok(()) }
    }

    /// Register the address space managed by this memory manager for interest in
    /// receiving private expedited memory barriers of the given kind.
    pub fn register_membarrier_private_expedited(
        &mut self,
        mtype: MembarrierType,
    ) -> Result<(), Errno> {
        let registrations = &mut self.forkable_state.membarrier_registrations;
        match mtype {
            MembarrierType::Memory => {
                registrations.memory = true;
            }
            MembarrierType::SyncCore => {
                registrations.sync_core = true;
            }
        }
        Ok(())
    }

    /// Checks if the address space managed by this memory manager is registered
    /// for interest in private expedited barriers of the given kind.
    pub fn membarrier_private_expedited_registered(&self, mtype: MembarrierType) -> bool {
        let registrations = &self.forkable_state.membarrier_registrations;
        match mtype {
            MembarrierType::Memory => registrations.memory,
            MembarrierType::SyncCore => registrations.sync_core,
        }
    }

    fn force_write_memory(
        &mut self,
        context: &MappingContext,
        addr: UserAddress,
        bytes: &[u8],
        released_mappings: &mut ReleasedMappings,
    ) -> Result<(), Errno> {
        let (range, mapping) = {
            let (r, m) = self.mappings.get(addr).ok_or_else(|| errno!(EFAULT))?;
            (r.clone(), m.clone())
        };
        if range.end < addr.saturating_add(bytes.len()) {
            track_stub!(
                TODO("https://fxbug.dev/445790710"),
                "ptrace poke across multiple mappings"
            );
            return error!(EFAULT);
        }

        // Don't create CoW copy of shared memory, go through regular syscall writing.
        if mapping.flags().contains(MappingFlags::SHARED) {
            if !mapping.can_write() {
                // Linux returns EIO here instead of EFAULT.
                return error!(EIO);
            }
            return self.write_mapping_memory(addr, &mapping, &bytes, context);
        }

        let backing = match self.get_mapping_backing(&mapping) {
            MappingBacking::PrivateAnonymous => {
                // Starnix has a writable handle to private anonymous memory.
                return context.private_anonymous.write_memory(addr, &bytes);
            }
            MappingBacking::Memory(backing) => backing,
        };

        let vmo = backing.memory().as_vmo().ok_or_else(|| errno!(EFAULT))?;
        let addr_offset = backing.address_to_offset(addr);
        let can_exec =
            vmo.basic_info().expect("get VMO handle info").rights.contains(Rights::EXECUTE);

        // Attempt to write to existing VMO
        match vmo.write(&bytes, addr_offset) {
            Ok(()) => {
                if can_exec {
                    // Issue a barrier to avoid executing stale instructions.
                    system_barrier(BarrierType::InstructionStream);
                }
                return Ok(());
            }

            Err(zx::Status::ACCESS_DENIED) => { /* Fall through */ }

            Err(status) => {
                return Err(MemoryManager::get_errno_for_vmo_err(status));
            }
        }

        // Create a CoW child of the entire VMO and swap with the backing.
        let mapping_offset = backing.address_to_offset(range.start);
        let len = range.end - range.start;

        // 1. Obtain a writable child of the VMO.
        let size = vmo.get_size().map_err(MemoryManager::get_errno_for_vmo_err)?;
        let child_vmo = vmo
            .create_child(VmoChildOptions::SNAPSHOT_AT_LEAST_ON_WRITE, 0, size)
            .map_err(MemoryManager::get_errno_for_vmo_err)?;

        // 2. Modify the memory.
        child_vmo.write(&bytes, addr_offset).map_err(MemoryManager::get_errno_for_vmo_err)?;

        // 3. If needed, remint the VMO as executable. Zircon flushes instruction caches when
        // mapping executable memory below, so a barrier isn't necessary here.
        let child_vmo = if can_exec {
            child_vmo
                .replace_as_executable(&VMEX_RESOURCE)
                .map_err(MemoryManager::get_errno_for_vmo_err)?
        } else {
            child_vmo
        };

        // Ensure that the mapping that `addr` falls into is mapped in the user VMAR.
        // This ensures that the mapping's mode becomes `Eager` (if it was `Lazy`),
        // otherwise, we might clone a `Lazy` mapping but map it unconditionally below,
        // leading to state drift where a mapping is mapped in Zircon but marked as lazy in Starnix.
        self.ensure_range_mapped_in_user_vmar(addr, None, context)?;

        // 4. Map the new VMO into user VMAR
        let memory = Arc::new(MemoryObject::from(child_vmo));
        context.map_in_user_vmar(
            SelectedAddress::FixedOverwrite(range.start),
            &memory,
            mapping_offset,
            len,
            mapping.flags(),
            false,
        )?;

        // 5. Update mappings
        let new_backing = MappingBackingMemory::new(range.start, memory, mapping_offset);

        let mut new_mapping = mapping.clone();
        new_mapping.set_backing_internal(MappingBacking::Memory(Box::new(new_backing)));

        released_mappings.extend(self.mappings.insert(range, new_mapping));

        Ok(())
    }

    fn set_brk(
        &mut self,
        current_task: &CurrentTask,
        mm: &Arc<MemoryManager>,
        addr: UserAddress,
        released_mappings: &mut ReleasedMappings,
    ) -> Result<UserAddress, Errno> {
        let rlimit_data = std::cmp::min(
            PROGRAM_BREAK_LIMIT,
            current_task.thread_group().get_rlimit(Resource::DATA),
        );

        let brk = match self.brk.clone() {
            None => {
                let brk = ProgramBreak { base: self.brk_origin, current: self.brk_origin };
                self.brk = Some(brk.clone());
                brk
            }
            Some(brk) => brk,
        };

        let Ok(last_address) = brk.base + rlimit_data else {
            // The requested program break is out-of-range. We're supposed to simply
            // return the current program break.
            return Ok(brk.current);
        };

        if addr < brk.base || addr > last_address {
            // The requested program break is out-of-range. We're supposed to simply
            // return the current program break.
            return Ok(brk.current);
        }

        let old_end = brk.current.round_up(*PAGE_SIZE).unwrap();
        let new_end = addr.round_up(*PAGE_SIZE).unwrap();

        match new_end.cmp(&old_end) {
            std::cmp::Ordering::Less => {
                // Shrinking the program break removes any mapped pages in the
                // affected range, regardless of whether they were actually program
                // break pages, or other mappings.
                let delta = old_end - new_end;

                if self.unmap(mm, new_end, delta, released_mappings).is_err() {
                    return Ok(brk.current);
                }
            }
            std::cmp::Ordering::Greater => {
                let range = old_end..new_end;
                let delta = new_end - old_end;

                // Check for mappings over the program break region.
                if self.mappings.range(range).next().is_some() {
                    return Ok(brk.current);
                }

                if self
                    .map_anonymous(
                        mm,
                        DesiredAddress::FixedOverwrite(old_end),
                        delta,
                        ProtectionFlags::READ | ProtectionFlags::WRITE,
                        MappingOptions::ANONYMOUS,
                        MappingName::Heap,
                        released_mappings,
                    )
                    .is_err()
                {
                    return Ok(brk.current);
                }
            }
            _ => {}
        };

        // Any required updates to the program break succeeded, so update internal state.
        let mut new_brk = brk;
        new_brk.current = addr;
        self.brk = Some(new_brk);

        Ok(addr)
    }

    fn register_with_uffd(
        &mut self,
        mm: &MemoryManager,
        addr: UserAddress,
        length: usize,
        userfault: &Arc<UserFault>,
        mode: FaultRegisterMode,
        released_mappings: &mut ReleasedMappings,
    ) -> Result<(), Errno> {
        let end_addr = addr.checked_add(length).ok_or_else(|| errno!(EINVAL))?;
        let range_for_op = addr..end_addr;
        let mut updates = vec![];

        for (range, mapping) in self.mappings.range(range_for_op.clone()) {
            if !mapping.private_anonymous() {
                track_stub!(TODO("https://fxbug.dev/391599171"), "uffd for shmem and hugetlbfs");
                return error!(EINVAL);
            }
            if mapping.flags().contains(MappingFlags::UFFD) {
                return error!(EBUSY);
            }
            let range = range.intersect(&range_for_op);
            let mut mapping = mapping.clone();
            mapping.set_uffd(mode);
            updates.push((range, mapping));
        }
        if updates.is_empty() {
            return error!(EINVAL);
        }

        mm.protect_vmar_range(addr, length, ProtectionFlags::empty())
            .expect("Failed to remove protections on uffd-registered range");

        // Use a separate loop to avoid mutating the mappings structure while iterating over it.
        for (range, mapping) in updates {
            released_mappings.extend(self.mappings.insert(range, mapping));
        }

        userfault.insert_pages(range_for_op, false);

        Ok(())
    }

    fn unregister_range_from_uffd(
        &mut self,
        mm: &MemoryManager,
        userfault: &Arc<UserFault>,
        addr: UserAddress,
        length: usize,
        released_mappings: &mut ReleasedMappings,
    ) -> Result<(), Errno> {
        let end_addr = addr.checked_add(length).ok_or_else(|| errno!(EINVAL))?;
        let range_for_op = addr..end_addr;
        let mut updates = vec![];

        for (range, mapping) in self.mappings.range(range_for_op.clone()) {
            if !mapping.private_anonymous() {
                track_stub!(TODO("https://fxbug.dev/391599171"), "uffd for shmem and hugetlbfs");
                return error!(EINVAL);
            }
            if mapping.flags().contains(MappingFlags::UFFD) {
                let range = range.intersect(&range_for_op);
                if userfault.remove_pages(range.clone()) {
                    let mut mapping = mapping.clone();
                    mapping.clear_uffd();
                    updates.push((range, mapping));
                }
            }
        }
        for (range, mapping) in updates {
            let length = range.end - range.start;
            let restored_flags = mapping.flags().access_flags();

            released_mappings.extend(self.mappings.insert(range.clone(), mapping));

            mm.protect_vmar_range(range.start, length, restored_flags)
                .expect("Failed to restore original protection bits on uffd-registered range");
        }
        Ok(())
    }

    fn unregister_uffd(
        &mut self,
        mm: &MemoryManager,
        userfault: &Arc<UserFault>,
        released_mappings: &mut ReleasedMappings,
    ) {
        let mut updates = vec![];

        for (range, mapping) in self.mappings.iter() {
            if mapping.flags().contains(MappingFlags::UFFD) {
                for range in userfault.get_registered_pages_overlapping_range(range.clone()) {
                    let mut mapping = mapping.clone();
                    mapping.clear_uffd();
                    updates.push((range, mapping));
                }
            }
        }
        // Use a separate loop to avoid mutating the mappings structure while iterating over it.
        for (range, mapping) in updates {
            let length = range.end - range.start;
            let restored_flags = mapping.flags().access_flags();
            released_mappings.extend(self.mappings.insert(range.clone(), mapping));
            // We can't recover from an error here as this is run during the cleanup.
            mm.protect_vmar_range(range.start, length, restored_flags)
                .expect("Failed to restore original protection bits on uffd-registered range");
        }

        userfault.remove_pages(
            UserAddress::from_ptr(RESTRICTED_ASPACE_BASE)
                ..UserAddress::from_ptr(RESTRICTED_ASPACE_HIGHEST_ADDRESS),
        );

        let weak_userfault = Arc::downgrade(userfault);
        self.userfaultfds.retain(|uf| !Weak::ptr_eq(uf, &weak_userfault));
    }

    fn set_mapping_name(
        &mut self,
        addr: UserAddress,
        length: usize,
        name: Option<FsString>,
        released_mappings: &mut ReleasedMappings,
    ) -> Result<(), Errno> {
        if addr.ptr() % *PAGE_SIZE as usize != 0 {
            return error!(EINVAL);
        }
        let end = match addr.checked_add(length) {
            Some(addr) => addr.round_up(*PAGE_SIZE).map_err(|_| errno!(ENOMEM))?,
            None => return error!(EINVAL),
        };

        let mappings_in_range =
            self.mappings.range(addr..end).map(|(r, m)| (r.clone(), m.clone())).collect::<Vec<_>>();

        if mappings_in_range.is_empty() {
            return error!(EINVAL);
        }
        if !mappings_in_range.first().unwrap().0.contains(&addr) {
            return error!(ENOMEM);
        }

        let mut last_range_end = None;
        // There's no get_mut on RangeMap, because it would be hard to implement correctly in
        // combination with merging of adjacent mappings. Instead, make a copy, change the copy,
        // and insert the copy.
        for (mut range, mut mapping) in mappings_in_range {
            if mapping.name().is_file() {
                // It's invalid to assign a name to a file-backed mapping.
                return error!(EBADF);
            }
            // Handle mappings that start before the region to be named.
            range.start = std::cmp::max(range.start, addr);
            // Handle mappings that extend past the region to be named.
            range.end = std::cmp::min(range.end, end);

            if let Some(last_range_end) = last_range_end {
                if last_range_end != range.start {
                    // The name must apply to a contiguous range of mapped pages.
                    return error!(ENOMEM);
                }
            }
            last_range_end = Some(range.end.round_up(*PAGE_SIZE)?);
            // TODO(b/310255065): We have no place to store names in a way visible to programs outside of Starnix
            // such as memory analysis tools.
            if let MappingBacking::Memory(backing) = self.get_mapping_backing(&mapping) {
                match &name {
                    Some(memory_name) => {
                        backing.memory().set_zx_name(memory_name);
                    }
                    None => {
                        backing.memory().set_zx_name(b"");
                    }
                }
            }
            mapping.set_name(match &name {
                Some(name) => MappingName::Vma(FlyByteStr::new(name.as_bytes())),
                None => MappingName::None,
            });
            released_mappings.extend(self.mappings.insert(range, mapping));
        }
        if let Some(last_range_end) = last_range_end {
            if last_range_end < end {
                // The name must apply to a contiguous range of mapped pages.
                return error!(ENOMEM);
            }
        }
        Ok(())
    }
}

/// The memory pinning shadow process used for mlock().
///
/// Uses its own distinct shadow process so that it doesn't interfere with other uses of memory
/// pinning.
pub struct MlockShadowProcess(memory_pinning::ShadowProcess);

impl MemoryManager {
    /// Ensures that any mapping at `addr` is actually mapped at in the user vmar.
    ///
    /// If `length` is `None`, it will ensure the mapping only on the page `addr` falls into.
    /// Returns `true` if any lazy mappings are mapped.
    pub fn ensure_range_mapped_in_user_vmar(
        &self,
        addr: UserAddress,
        length: Option<usize>,
    ) -> Result<bool, Errno> {
        if !self.state.read().any_ranges_lazy(std::iter::once((addr, length))) {
            return Ok(false);
        }
        self.state.write().ensure_ranges_mapped_in_user_vmar(
            std::iter::once((addr, length)),
            &self.mapping_context,
        )
    }

    /// Ensures that any mappings in the specified ranges are actually mapped in the user vmar.
    ///
    /// If `length` is `None`, it will ensure the mapping only on the page `addr` falls into.
    /// Returns `true` if any lazy mappings are mapped.
    pub fn ensure_ranges_mapped_in_user_vmar<I>(&self, ranges: I) -> Result<bool, Errno>
    where
        I: IntoIterator<Item = (UserAddress, Option<usize>)>,
    {
        // Collect ranges into a SmallVec with capacity 4 to avoid heap allocations in the common
        // case where there are only a few ranges (e.g., socket read/write buffers).
        let ranges = ranges.into_iter().collect::<SmallVec<[_; 4]>>();
        if !self.state.read().any_ranges_lazy(ranges.iter().cloned()) {
            return Ok(false);
        }
        self.state.write().ensure_ranges_mapped_in_user_vmar(ranges, &self.mapping_context)
    }

    pub fn mrelease(&self) -> Result<(), Errno> {
        self.mapping_context.private_anonymous.zero(
            UserAddress::from_ptr(self.mapping_context.user_vmar_info.base),
            self.mapping_context.user_vmar_info.len,
        )?;
        Ok(())
    }

    pub fn summarize(&self, summary: &mut crate::mm::MappingSummary) {
        let state = self.state.read();
        for (_, mapping) in state.mappings.iter() {
            summary.add(&state, mapping);
        }
    }

    pub fn get_mappings_for_vmsplice(
        self: &Arc<MemoryManager>,
        buffers: &UserBuffers,
    ) -> Result<Vec<Arc<VmsplicePayload>>, Errno> {
        self.state.read().get_mappings_for_vmsplice(self, buffers)
    }

    pub fn has_same_address_space(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }

    fn unified_transfer_loop<F>(
        &self,
        addr: UserAddress,
        len: usize,
        mut transfer_fn: F,
    ) -> Result<usize, Errno>
    where
        F: FnMut(UserAddress, usize) -> Result<ControlFlow<usize, usize>, Errno>,
    {
        let mut copied = 0;
        while copied < len {
            match transfer_fn((addr + copied)?, copied)? {
                ControlFlow::Continue(num_copied) => {
                    if num_copied == 0 {
                        let fault_addr = (addr + copied)?;
                        // If we successfully mapped a lazy mapping, retry the copy.
                        // Otherwise, this might be a permission fault or invalid address, so we
                        // stop and return the partial result.
                        //
                        // NOTE: We lazily materialize mappings one page at a time here.
                        // An alternative approach would be to materialize the entire range
                        // or the first mapping up front. That might avoid bouncing between
                        // threads on faults, but adds overhead (locks and range lookups)
                        // if the memory is already mapped. We use the reactive approach
                        // for now, but this could be tuned in the future.
                        if self.ensure_range_mapped_in_user_vmar(fault_addr, None)? {
                            continue;
                        } else {
                            break;
                        }
                    }
                    copied += num_copied;
                }
                ControlFlow::Break(num_copied) => {
                    copied += num_copied;
                    break;
                }
            }
        }
        Ok(copied)
    }

    pub fn unified_read_memory<'a>(
        &self,
        current_task: &CurrentTask,
        addr: UserAddress,
        bytes: &'a mut [MaybeUninit<u8>],
    ) -> Result<&'a mut [u8], Errno> {
        debug_assert!(self.has_same_address_space(&current_task.mm().unwrap()));

        let buf_ptr = bytes.as_mut_ptr();
        let buf_len = bytes.len();

        let copied = self.unified_transfer_loop(addr, buf_len, |cur_addr, offset| {
            // SAFETY: Exclusive access to `bytes` for the lifetime of this function.
            let current_bytes =
                unsafe { std::slice::from_raw_parts_mut(buf_ptr.add(offset), buf_len - offset) };
            let (read_bytes, _unread_bytes) = usercopy().copyin(cur_addr.ptr(), current_bytes);
            Ok(ControlFlow::Continue(read_bytes.len()))
        })?;
        if copied < bytes.len() {
            error!(EFAULT)
        } else {
            // SAFETY: All bytes up to `buf_len` have been initialized.
            Ok(unsafe { std::slice::from_raw_parts_mut(buf_ptr as *mut u8, buf_len) })
        }
    }

    pub fn syscall_read_memory<'a>(
        &self,
        addr: UserAddress,
        bytes: &'a mut [MaybeUninit<u8>],
    ) -> Result<&'a mut [u8], Errno> {
        self.state.read().read_memory(addr, bytes, &self.mapping_context)
    }

    pub fn unified_read_memory_partial_until_null_byte<'a>(
        &self,
        current_task: &CurrentTask,
        addr: UserAddress,
        bytes: &'a mut [MaybeUninit<u8>],
    ) -> Result<&'a mut [u8], Errno> {
        debug_assert!(self.has_same_address_space(&current_task.mm().unwrap()));

        let buf_ptr = bytes.as_mut_ptr();
        let buf_len = bytes.len();

        let copied = self.unified_transfer_loop(addr, buf_len, |cur_addr, offset| {
            // SAFETY: Exclusive access to `bytes` for the lifetime of this function.
            let current_bytes =
                unsafe { std::slice::from_raw_parts_mut(buf_ptr.add(offset), buf_len - offset) };
            let (read_bytes, _unread_bytes) =
                usercopy().copyin_until_null_byte(cur_addr.ptr(), current_bytes);

            let num_copied = read_bytes.len();
            if read_bytes.last().map(|b| *b == 0).unwrap_or(false) {
                Ok(ControlFlow::Break(num_copied))
            } else {
                Ok(ControlFlow::Continue(num_copied))
            }
        })?;
        if copied == 0 && !bytes.is_empty() {
            error!(EFAULT)
        } else {
            // SAFETY: Bytes up to `copied` have been initialized.
            Ok(unsafe { std::slice::from_raw_parts_mut(buf_ptr as *mut u8, copied) })
        }
    }

    pub fn syscall_read_memory_partial_until_null_byte<'a>(
        &self,
        addr: UserAddress,
        bytes: &'a mut [MaybeUninit<u8>],
    ) -> Result<&'a mut [u8], Errno> {
        self.state.read().read_memory_partial_until_null_byte(addr, bytes, &self.mapping_context)
    }

    pub fn unified_read_memory_partial<'a>(
        &self,
        current_task: &CurrentTask,
        addr: UserAddress,
        bytes: &'a mut [MaybeUninit<u8>],
    ) -> Result<&'a mut [u8], Errno> {
        debug_assert!(self.has_same_address_space(&current_task.mm().unwrap()));

        let buf_ptr = bytes.as_mut_ptr();
        let buf_len = bytes.len();

        let copied = self.unified_transfer_loop(addr, buf_len, |cur_addr, offset| {
            // SAFETY: Exclusive access to `bytes` for the lifetime of this function.
            let current_bytes =
                unsafe { std::slice::from_raw_parts_mut(buf_ptr.add(offset), buf_len - offset) };
            let (read_bytes, _unread_bytes) = usercopy().copyin(cur_addr.ptr(), current_bytes);
            Ok(ControlFlow::Continue(read_bytes.len()))
        })?;
        if copied == 0 && !bytes.is_empty() {
            error!(EFAULT)
        } else {
            // SAFETY: Bytes up to `copied` have been initialized.
            Ok(unsafe { std::slice::from_raw_parts_mut(buf_ptr as *mut u8, copied) })
        }
    }

    pub fn syscall_read_memory_partial<'a>(
        &self,
        addr: UserAddress,
        bytes: &'a mut [MaybeUninit<u8>],
    ) -> Result<&'a mut [u8], Errno> {
        self.state.read().read_memory_partial(addr, bytes, &self.mapping_context)
    }

    pub fn unified_write_memory(
        &self,
        current_task: &CurrentTask,
        addr: UserAddress,
        bytes: &[u8],
    ) -> Result<usize, Errno> {
        debug_assert!(self.has_same_address_space(&current_task.mm().unwrap()));

        let len = bytes.len();
        let copied = self.unified_transfer_loop(addr, len, |cur_addr, offset| {
            Ok(ControlFlow::Continue(usercopy().copyout(&bytes[offset..], cur_addr.ptr())))
        })?;
        if copied < bytes.len() { error!(EFAULT) } else { Ok(copied) }
    }

    /// Write `bytes` to memory address `addr`, making a copy-on-write child of the VMO backing and
    /// replacing the mapping if necessary.
    ///
    /// NOTE: this bypasses userspace's memory protection configuration and should only be called
    /// by codepaths like ptrace which bypass memory protection.
    pub fn force_write_memory(&self, addr: UserAddress, bytes: &[u8]) -> Result<(), Errno> {
        let mut state = self.state.write();
        let mut released_mappings = ReleasedMappings::default();
        let result =
            state.force_write_memory(&self.mapping_context, addr, bytes, &mut released_mappings);
        released_mappings.finalize(state);
        result
    }

    pub fn syscall_write_memory(&self, addr: UserAddress, bytes: &[u8]) -> Result<usize, Errno> {
        self.state.read().write_memory(addr, bytes, &self.mapping_context)
    }

    pub fn unified_write_memory_partial(
        &self,
        current_task: &CurrentTask,
        addr: UserAddress,
        bytes: &[u8],
    ) -> Result<usize, Errno> {
        debug_assert!(self.has_same_address_space(&current_task.mm().unwrap()));

        let len = bytes.len();
        let copied = self.unified_transfer_loop(addr, len, |cur_addr, offset| {
            Ok(ControlFlow::Continue(usercopy().copyout(&bytes[offset..], cur_addr.ptr())))
        })?;
        if copied == 0 && !bytes.is_empty() { error!(EFAULT) } else { Ok(copied) }
    }

    pub fn syscall_write_memory_partial(
        &self,
        addr: UserAddress,
        bytes: &[u8],
    ) -> Result<usize, Errno> {
        self.state.read().write_memory_partial(addr, bytes, &self.mapping_context)
    }

    pub fn unified_zero(
        &self,
        current_task: &CurrentTask,
        addr: UserAddress,
        length: usize,
    ) -> Result<usize, Errno> {
        debug_assert!(self.has_same_address_space(&current_task.mm().unwrap()));

        {
            let page_size = *PAGE_SIZE as usize;
            // Get the page boundary immediately following `addr` if `addr` is
            // not page aligned.
            let next_page_boundary = round_up_to_system_page_size(addr.ptr())?;
            // The number of bytes needed to zero at least a full page (not just
            // a pages worth of bytes) starting at `addr`.
            let length_with_atleast_one_full_page = page_size + (next_page_boundary - addr.ptr());
            // If at least one full page is being zeroed, go through the memory object since Zircon
            // can swap the mapped pages with the zero page which should be cheaper than zeroing
            // out a pages worth of bytes manually.
            //
            // If we are not zeroing out a full page, then go through usercopy
            // if unified aspaces is enabled.
            if length >= length_with_atleast_one_full_page {
                return self.syscall_zero(addr, length);
            }
        }

        let copied = self.unified_transfer_loop(addr, length, |cur_addr, offset| {
            Ok(ControlFlow::Continue(usercopy().zero(cur_addr.ptr(), length - offset)))
        })?;
        if copied == 0 && length > 0 { error!(EFAULT) } else { Ok(copied) }
    }

    pub fn syscall_zero(&self, addr: UserAddress, length: usize) -> Result<usize, Errno> {
        self.state.read().zero(addr, length, &self.mapping_context)
    }

    /// Performs a data and instruction cache flush over the given address range.
    pub fn cache_flush(&self, range: Range<UserAddress>) -> Result<(), Errno> {
        self.state.read().cache_flush(range, &self.mapping_context)
    }

    /// Register the address space managed by this memory manager for interest in
    /// receiving private expedited memory barriers of the given type.
    pub fn register_membarrier_private_expedited(
        &self,
        mtype: MembarrierType,
    ) -> Result<(), Errno> {
        self.state.write().register_membarrier_private_expedited(mtype)
    }

    /// Checks if the address space managed by this memory manager is registered
    /// for interest in private expedited barriers of the given kind.
    pub fn membarrier_private_expedited_registered(&self, mtype: MembarrierType) -> bool {
        self.state.read().membarrier_private_expedited_registered(mtype)
    }
}

/// State and resources of the `MemoryManager` that are either immutable after creation
/// or handle their own interior mutability (e.g., `private_anonymous`).
///
/// This is distinct from `MemoryManagerState` in that the fields here do not require
/// acquisition of the `MemoryManager`'s main lock for access. This allows concurrent
/// access to these resources without lock contention.
///
/// This structure primarily holds the Zircon VMAR handle and the manager for private
/// anonymous memory, which are the core primitives used to manipulate the address space.
pub struct MappingContext {
    /// The VMAR in which userspace mappings occur.
    ///
    /// We map userspace memory in this child VMAR so that we can destroy the
    /// entire VMAR during exec.
    /// For 32-bit tasks, we limit the user_vmar to correspond to the available memory.
    ///
    /// This field is set to `ZX_HANDLE_INVALID` when the address-space has been destroyed (e.g. on
    /// `exec()`), allowing the value to be pro-actively checked for, or the `ZX_ERR_BAD_HANDLE`
    /// status return from Zircon operations handled, to suit the call-site.
    pub user_vmar: zx::Vmar,

    /// Cached VmarInfo for user_vmar.
    pub user_vmar_info: zx::VmarInfo,

    /// Memory object backing private, anonymous memory allocations in this address space.
    pub private_anonymous: PrivateAnonymousMemoryManager,
}

impl MappingContext {
    fn map_in_user_vmar(
        &self,
        addr: SelectedAddress,
        memory: &MemoryObject,
        memory_offset: u64,
        length: usize,
        flags: MappingFlags,
        populate: bool,
    ) -> Result<(), Errno> {
        map_in_vmar(
            &self.user_vmar,
            &self.user_vmar_info,
            addr,
            memory,
            memory_offset,
            length,
            flags,
            populate,
        )
    }

    pub fn max_address(&self) -> UserAddress {
        UserAddress::from_ptr(self.user_vmar_info.base + self.user_vmar_info.len)
    }
}

pub struct MemoryManager {
    /// The base address of the root_vmar.
    pub base_addr: UserAddress,

    /// The futexes in this address space.
    pub futex: Arc<FutexTable<PrivateFutexKey>>,

    /// The mapping context for this address space.
    pub mapping_context: MappingContext,

    /// Mutable state for the memory manager.
    pub state: RwLock<MemoryManagerState>,

    /// Whether this address space is dumpable.
    pub dumpable: LockDepMutex<DumpPolicy, MmDumpable>,

    /// Maximum valid user address for this vmar.
    pub maximum_valid_user_address: UserAddress,

    /// In-flight payloads enqueued to a pipe as a consequence of a `vmsplice(2)`
    /// operation.
    ///
    /// For details on why we need to keep track of in-flight vmspliced payloads,
    /// see [`VmsplicePayload`].
    ///
    /// For details on why this isn't under the `LockDepRwLock` protected `MemoryManagerState`,
    /// See [`InflightVmsplicedPayloads::payloads`].
    pub inflight_vmspliced_payloads: InflightVmsplicedPayloads,

    /// A mechanism to be notified when this `MemoryManager` is destroyed.
    pub drop_notifier: DropNotifier,

    /// The architecture width of the process.
    pub arch_width: ArchWidth,

    /// Cached memory stats to avoid expensive Zircon VMAR walks on sequential reads.
    pub cached_stats: Mutex<Option<(zx::MonotonicInstant, MemoryStats)>>,
}

impl ArchSpecific for MemoryManager {
    fn is_arch32(&self) -> bool {
        self.arch_width.is_arch32()
    }
}

fn check_access_permissions_in_page_fault(
    decoded: &PageFaultExceptionReport,
    mapping: &Mapping,
) -> bool {
    let exec_denied = decoded.is_execute && !mapping.can_exec();
    let write_denied = decoded.is_write && !mapping.can_write();
    let read_denied = (!decoded.is_execute && !decoded.is_write) && !mapping.can_read();
    !exec_denied && !write_denied && !read_denied
}

impl MemoryManager {
    /// Returns a new `MemoryManager` suitable for use in tests.
    pub fn new_for_test(root_vmar: zx::Unowned<'_, zx::Vmar>, arch_width: ArchWidth) -> Arc<Self> {
        Self::new(root_vmar, arch_width, None, None).expect("can create MemoryManager")
    }

    // Returns details of mappings in the `user_vmar`, or an empty vector if the `user_vmar` has
    // been destroyed.
    fn with_zx_mappings<R>(
        &self,
        current_task: &CurrentTask,
        op: impl FnOnce(&[zx::MapInfo]) -> R,
    ) -> R {
        MapInfoCache::get_or_init(current_task)
            .expect("must be able to retrieve map info cache")
            .with_map_infos(&self.mapping_context.user_vmar, |infos| match infos {
                Ok(infos) => op(infos),
                Err(_) => op(&[]),
            })
    }

    fn protect_vmar_range(
        &self,
        addr: UserAddress,
        length: usize,
        prot_flags: ProtectionFlags,
    ) -> Result<(), Errno> {
        let vmar_flags = prot_flags.to_vmar_flags();
        // SAFETY: Modifying user vmar
        unsafe { self.mapping_context.user_vmar.protect(addr.ptr(), length, vmar_flags) }.map_err(
            |s| match s {
                zx::Status::INVALID_ARGS => errno!(EINVAL),
                zx::Status::NOT_FOUND => errno!(ENOMEM),
                zx::Status::ACCESS_DENIED => errno!(EACCES),
                _ => impossible_error(s),
            },
        )
    }

    pub fn total_locked_bytes(&self) -> u64 {
        self.state.read().num_locked_bytes(
            UserAddress::from(self.mapping_context.user_vmar_info.base as u64)
                ..UserAddress::from(
                    (self.mapping_context.user_vmar_info.base
                        + self.mapping_context.user_vmar_info.len) as u64,
                ),
        )
    }

    /// Returns a new `MemoryManager` initialized with a new userspace VMAR matching the specified
    /// `arch_width`, under the specified restricted-mode `root_vmar`.  The `executable_node` that
    /// the new address-space will execute may optionally be supplied.
    fn new(
        root_vmar: zx::Unowned<'_, zx::Vmar>,
        arch_width: ArchWidth,
        executable_node: Option<NamespaceNode>,
        private_anonymous: Option<PrivateAnonymousMemoryManager>,
    ) -> Result<Arc<Self>, Errno> {
        debug_assert!(!root_vmar.is_invalid());

        let mut vmar_info = root_vmar.info().map_err(|status| from_status_like_fdio!(status))?;
        if arch_width.is_arch32() {
            vmar_info.len = (LOWER_4GB_LIMIT.ptr() - vmar_info.base) as usize;
        }

        let (user_vmar, ptr) = root_vmar
            .allocate(
                0,
                vmar_info.len,
                zx::VmarFlags::SPECIFIC
                    | zx::VmarFlags::CAN_MAP_SPECIFIC
                    | zx::VmarFlags::CAN_MAP_READ
                    | zx::VmarFlags::CAN_MAP_WRITE
                    | zx::VmarFlags::CAN_MAP_EXECUTE,
            )
            .map_err(|status| from_status_like_fdio!(status))?;
        assert_eq!(ptr, vmar_info.base);

        let user_vmar_info = user_vmar.info().map_err(|status| from_status_like_fdio!(status))?;

        // Ensure that the `user_vmar_info` matches assumptions for the requested layout.
        debug_assert_eq!(RESTRICTED_ASPACE_BASE, user_vmar_info.base);
        if arch_width.is_arch32() {
            debug_assert_eq!(LOWER_4GB_LIMIT.ptr() - user_vmar_info.base, user_vmar_info.len);
        } else {
            debug_assert_eq!(RESTRICTED_ASPACE_SIZE, user_vmar_info.len);
        }

        // The private anonymous backing memory object extend from the user address 0 up to the
        // highest mappable address. The pages below `user_vmar_info.base` are never mapped, but
        // including them in the memory object makes the math for mapping address to memory object
        // offsets simpler.
        let backing_size = (user_vmar_info.base + user_vmar_info.len) as u64;

        // Place the stack at the end of the address space, subject to ASLR adjustment. The stack
        // grows down, so the origin corresponds to the top of the initial stack.
        let stack_origin = UserAddress::from_ptr(
            user_vmar_info.base + user_vmar_info.len - generate_random_offset_for_aslr(arch_width),
        )
        .round_up(*PAGE_SIZE)?;

        // Set the highest address that `mmap` will assign to the allocations that don't ask for a
        // specific address, subject to ASLR adjustment.
        let mmap_top = stack_origin
            .checked_sub(MAX_STACK_SIZE + generate_random_offset_for_aslr(arch_width))
            .ok_or_else(|| errno!(EINVAL))?;

        Ok(Arc::new(MemoryManager {
            base_addr: UserAddress::from_ptr(user_vmar_info.base),
            futex: Arc::<FutexTable<PrivateFutexKey>>::default(),
            mapping_context: MappingContext {
                user_vmar,
                user_vmar_info,
                private_anonymous: private_anonymous
                    .unwrap_or_else(|| PrivateAnonymousMemoryManager::new(backing_size)),
            },
            state: MemoryManagerState {
                mappings: Default::default(),
                userfaultfds: Default::default(),
                shadow_mappings_for_mlock: Default::default(),
                forkable_state: MemoryManagerForkableState {
                    executable_node,
                    stack_origin,
                    mmap_top,
                    ..Default::default()
                },
            }
            .into(),
            // TODO(security): Reset to DISABLE, or the value in the fs.suid_dumpable sysctl, under
            // certain conditions as specified in the prctl(2) man page.
            dumpable: LockDepMutex::new(DumpPolicy::User),
            maximum_valid_user_address: UserAddress::from_ptr(
                user_vmar_info.base + user_vmar_info.len,
            ),
            inflight_vmspliced_payloads: Default::default(),
            drop_notifier: DropNotifier::default(),
            arch_width,
            cached_stats: Mutex::default(),
        }))
    }

    pub fn set_brk(
        self: &Arc<Self>,
        current_task: &CurrentTask,
        addr: UserAddress,
    ) -> Result<UserAddress, Errno> {
        let mut state = self.state.write();
        let mut released_mappings = ReleasedMappings::default();
        let result = state.set_brk(current_task, self, addr, &mut released_mappings);
        released_mappings.finalize(state);
        result
    }

    pub fn register_uffd(&self, userfault: &Arc<UserFault>) {
        let mut state = self.state.write();
        state.userfaultfds.push(Arc::downgrade(userfault));
    }

    /// Register a given memory range with a userfault object.
    pub fn register_with_uffd(
        self: &Arc<Self>,
        addr: UserAddress,
        length: usize,
        userfault: &Arc<UserFault>,
        mode: FaultRegisterMode,
    ) -> Result<(), Errno> {
        let mut state = self.state.write();
        let mut released_mappings = ReleasedMappings::default();
        let result =
            state.register_with_uffd(self, addr, length, userfault, mode, &mut released_mappings);
        released_mappings.finalize(state);
        result
    }

    /// Unregister a given range from any userfault objects associated with it.
    pub fn unregister_range_from_uffd(
        &self,
        userfault: &Arc<UserFault>,
        addr: UserAddress,
        length: usize,
    ) -> Result<(), Errno> {
        let mut state = self.state.write();
        let mut released_mappings = ReleasedMappings::default();
        let result =
            state.unregister_range_from_uffd(self, userfault, addr, length, &mut released_mappings);
        released_mappings.finalize(state);
        result
    }

    /// Unregister any mappings registered with a given userfault object. Used when closing the last
    /// file descriptor associated to it.
    pub fn unregister_uffd(&self, userfault: &Arc<UserFault>) {
        let mut state = self.state.write();
        let mut released_mappings = ReleasedMappings::default();
        state.unregister_uffd(self, userfault, &mut released_mappings);
        released_mappings.finalize(state);
    }

    /// Populate a range of pages registered with an userfaulfd according to a `populate` function.
    /// This will fail if the pages were not registered with userfaultfd, or if the page at `addr`
    /// was already populated. If any page other than the first one was populated, the `length`
    /// is adjusted to only include the first N unpopulated pages, and this adjusted length
    /// is then passed to `populate`. On success, returns the number of populated bytes.
    pub fn populate_from_uffd<F>(
        &self,
        addr: UserAddress,
        length: usize,
        userfault: &Arc<UserFault>,
        populate: F,
    ) -> Result<usize, Errno>
    where
        F: FnOnce(&MemoryManagerState, usize) -> Result<usize, Errno>,
    {
        let state = self.state.read();
        // Check that the addr..length range is a contiguous range of mappings which are all
        // registered with an userfault object.
        let mut bytes_registered_with_uffd = 0;
        for (mapping, len) in
            state.get_contiguous_mappings_at(addr, length, &self.mapping_context)?
        {
            if mapping.flags().contains(MappingFlags::UFFD) {
                // Check that the mapping is registered with the same uffd. This is not required,
                // but we don't support cross-uffd operations yet.
                if !userfault.contains_addr(addr) {
                    track_stub!(
                        TODO("https://fxbug.dev/391599171"),
                        "operations across different uffds"
                    );
                    return error!(ENOTSUP);
                };
            } else {
                return error!(ENOENT);
            }
            bytes_registered_with_uffd += len;
        }
        if bytes_registered_with_uffd != length {
            return error!(ENOENT);
        }

        let end_addr = addr.checked_add(length).ok_or_else(|| errno!(EINVAL))?;

        // Determine how many pages in the requested range are already populated
        let first_populated =
            userfault.get_first_populated_page_after(addr).ok_or_else(|| errno!(ENOENT))?;
        // If the very first page is already populated, uffd operations should just return EEXIST
        if first_populated == addr {
            return error!(EEXIST);
        }
        // Otherwise it is possible to do an incomplete operation by only populating pages until
        // the first populated one.
        let trimmed_end = std::cmp::min(first_populated, end_addr);
        let effective_length = trimmed_end - addr;

        populate(&state, effective_length)?;
        userfault.insert_pages(addr..trimmed_end, true);

        // Since we used protection bits to force pagefaults, we now need to reverse this change by
        // restoring the protections on the underlying Zircon mappings to the "real" protection bits
        // that were kept in the Starnix mappings. This will prevent new pagefaults from being
        // generated. Only do this on the pages that were populated by this operation.
        for (range, mapping) in state.mappings.range(addr..trimmed_end) {
            let range_to_protect = range.intersect(&(addr..trimmed_end));
            let restored_flags = mapping.flags().access_flags();
            let length = range_to_protect.end - range_to_protect.start;
            self.protect_vmar_range(range_to_protect.start, length, restored_flags)
                .expect("Failed to restore original protection bits on uffd-registered range");
        }
        // Return the number of effectively populated bytes, which might be smaller than the
        // requested number.
        Ok(effective_length)
    }

    pub fn zero_from_uffd(
        &self,
        addr: UserAddress,
        length: usize,
        userfault: &Arc<UserFault>,
    ) -> Result<usize, Errno> {
        self.populate_from_uffd(addr, length, userfault, |state, effective_length| {
            state.zero(addr, effective_length, &self.mapping_context)
        })
    }

    pub fn fill_from_uffd(
        &self,
        addr: UserAddress,
        buf: &[u8],
        length: usize,
        userfault: &Arc<UserFault>,
    ) -> Result<usize, Errno> {
        self.populate_from_uffd(addr, length, userfault, |state, effective_length| {
            state.write_memory(addr, &buf[..effective_length], &self.mapping_context)
        })
    }

    pub fn copy_from_uffd(
        &self,
        source_addr: UserAddress,
        dst_addr: UserAddress,
        length: usize,
        userfault: &Arc<UserFault>,
    ) -> Result<usize, Errno> {
        self.populate_from_uffd(dst_addr, length, userfault, |state, effective_length| {
            let mut buf = vec![std::mem::MaybeUninit::uninit(); effective_length];
            let buf = state.read_memory(source_addr, &mut buf, &self.mapping_context)?;
            state.write_memory(dst_addr, &buf[..effective_length], &self.mapping_context)
        })
    }

    /// Returns the new `MemoryManager` for a process, pre-populated with a snapshot of the layout
    /// and mappings of `source_mm`.  This is used during `CurrentTask::clone()` operations to
    /// create the initial address-space for the cloned child process.
    pub fn snapshot_of(
        source_mm: &Arc<MemoryManager>,
        root_vmar: zx::Unowned<'_, zx::Vmar>,
        arch_width: ArchWidth,
    ) -> Result<Arc<Self>, Errno> {
        fuchsia_trace::duration!(CATEGORY_STARNIX_MM, "snapshot_of");
        let backing_size = (source_mm.mapping_context.user_vmar_info.base
            + source_mm.mapping_context.user_vmar_info.len) as u64;
        let private_anonymous =
            source_mm.mapping_context.private_anonymous.snapshot(backing_size)?;
        let target = MemoryManager::new(
            root_vmar,
            arch_width,
            source_mm.executable_node(),
            Some(private_anonymous),
        )?;

        // Hold the lock throughout the operation to uphold memory manager's invariants.
        // See mm/README.md.
        {
            let (state, mut target_state) = ordered_write_lock(&source_mm.state, &target.state);
            debug_assert_eq!(
                source_mm.mapping_context.user_vmar_info,
                target.mapping_context.user_vmar_info
            );

            let mut clone_cache = HashMap::<zx::Koid, Arc<MemoryObject>>::new();

            for (range, mapping) in state.mappings.iter() {
                if mapping.flags().contains(MappingFlags::DONTFORK) {
                    continue;
                }
                // Locking is not inherited when forking.
                let target_mapping_flags = mapping.flags().difference(MappingFlags::LOCKED);
                match state.get_mapping_backing(mapping) {
                    MappingBacking::Memory(backing) => {
                        fuchsia_trace::duration!(CATEGORY_STARNIX_MM, "memory_backing_clone");
                        let memory_offset = backing.address_to_offset(range.start);

                        let target_memory = if mapping.flags().contains(MappingFlags::SHARED)
                            || mapping.name().is_vvar()
                        {
                            // Note that the Vvar is a special mapping that behaves like a shared mapping but
                            // is private to each process.
                            backing.memory().clone()
                        } else {
                            let memory_obj = backing.memory();
                            let options = mapping.flags().options();
                            let mut rights = memory_obj.get_rights();
                            if mapping.flags().contains(MappingFlags::WRITE) {
                                rights |= zx::Rights::WRITE;
                            }
                            let memory =
                                clone_cache.entry(memory_obj.get_koid()).or_insert_with_fallible(
                                    || memory_obj.clone_memory(rights, options),
                                )?;
                            memory.clone()
                        };

                        let mapping = Mapping::with_name(
                            MappingBacking::Memory(Box::new(MappingBackingMemory::new(
                                range.start,
                                target_memory,
                                memory_offset,
                            ))),
                            target_mapping_flags,
                            mapping.name().to_owned(),
                            MappingMode::Lazy,
                        );
                        assert!(
                            target_state.mappings.append_non_overlapping(range.clone(), mapping)
                        );
                    }
                    MappingBacking::PrivateAnonymous => {
                        fuchsia_trace::duration!(
                            CATEGORY_STARNIX_MM,
                            "private_anonymous_backing_clone"
                        );
                        let length = range.end - range.start;
                        if mapping.flags().contains(MappingFlags::WIPEONFORK) {
                            target
                                .mapping_context
                                .private_anonymous
                                .zero(range.start, length)
                                .map_err(|_| errno!(ENOMEM))?;
                        }

                        let mapping = Mapping::new_private_anonymous(
                            target_mapping_flags,
                            mapping.name().to_owned(),
                            MappingMode::Lazy,
                        );
                        assert!(
                            target_state.mappings.append_non_overlapping(range.clone(), mapping)
                        );
                    }
                };
            }

            target_state.forkable_state = state.forkable_state.clone();
        }

        let self_dumpable = *source_mm.dumpable.lock();
        *target.dumpable.lock() = self_dumpable;

        Ok(target)
    }

    /// Returns the replacement `MemoryManager` to be used by the `exec()`ing task.
    ///
    /// POSIX requires that "a call to any exec function from a process with more than one thread
    /// shall result in all threads being terminated and the new executable being loaded and
    /// executed. No destructor functions or cleanup handlers shall be called".
    /// The caller is responsible for having ensured that this is the only `Task` in the
    /// `ThreadGroup`, and thereby the `zx::process`, such that it is safe to tear-down the Zircon
    /// userspace VMAR for the current address-space.
    pub fn exec(
        root_vmar: zx::Unowned<'_, zx::Vmar>,
        old_mm: Option<Arc<Self>>,
        exe_node: NamespaceNode,
        arch_width: ArchWidth,
    ) -> Result<Arc<Self>, Errno> {
        // To safeguard against concurrent accesses by other tasks through this `MemoryManager`, the
        // following steps are performed while holding the write lock on the old MM, if any:
        //
        // 1. All `mappings` are removed, so that remote `MemoryAccessor` calls will fail.
        // 2. The `user_vmar` is `destroy()`ed to free-up the user address-space.
        //
        // Once these steps are complete it is safe for the old mappings to be dropped.
        if let Some(old_mm) = old_mm {
            let _old_mappings = {
                let mut state = old_mm.state.write();

                // SAFETY: This operation is safe because this is the only `Task` active in the address-
                // space, and accesses by remote tasks will use syscalls on the `root_vmar`.
                unsafe {
                    old_mm
                        .mapping_context
                        .user_vmar
                        .destroy()
                        .map_err(|status| from_status_like_fdio!(status))?
                }

                std::mem::replace(&mut state.mappings, Default::default())
            };
        }

        Self::new(root_vmar, arch_width, Some(exe_node), None)
    }

    pub fn initialize_brk_origin(
        &self,
        arch_width: ArchWidth,
        executable_end: UserAddress,
    ) -> Result<(), Errno> {
        self.state.write().brk_origin = executable_end
            .checked_add(generate_random_offset_for_aslr(arch_width))
            .ok_or_else(|| errno!(EINVAL))?;
        Ok(())
    }

    // Get a randomised address for loading a position-independent executable.
    pub fn get_random_base_for_executable(
        &self,
        arch_width: ArchWidth,
        length: usize,
    ) -> Result<UserAddress, Errno> {
        let state = self.state.read();

        // Place it at approx. 2/3 of the available mmap space, subject to ASLR adjustment.
        let base = round_up_to_system_page_size(2 * state.mmap_top.ptr() / 3).unwrap()
            + generate_random_offset_for_aslr(arch_width);
        if base.checked_add(length).ok_or_else(|| errno!(EINVAL))? <= state.mmap_top.ptr() {
            Ok(UserAddress::from_ptr(base))
        } else {
            error!(EINVAL)
        }
    }
    pub fn executable_node(&self) -> Option<NamespaceNode> {
        self.state.read().executable_node.clone()
    }

    #[track_caller]
    pub fn get_errno_for_map_err(status: zx::Status) -> Errno {
        match status {
            zx::Status::INVALID_ARGS => errno!(EINVAL),
            zx::Status::ACCESS_DENIED => errno!(EPERM),
            zx::Status::NOT_SUPPORTED => errno!(ENODEV),
            zx::Status::NO_MEMORY => errno!(ENOMEM),
            zx::Status::NO_RESOURCES => errno!(ENOMEM),
            zx::Status::OUT_OF_RANGE => errno!(ENOMEM),
            zx::Status::ALREADY_EXISTS => errno!(EEXIST),
            zx::Status::BAD_STATE => errno!(EINVAL),
            _ => impossible_error(status),
        }
    }

    #[track_caller]
    pub fn get_errno_for_vmo_err(status: zx::Status) -> Errno {
        match status {
            zx::Status::NO_MEMORY => errno!(ENOMEM),
            zx::Status::ACCESS_DENIED => errno!(EPERM),
            zx::Status::NOT_SUPPORTED => errno!(EIO),
            zx::Status::BAD_STATE => errno!(EIO),
            _ => return impossible_error(status),
        }
    }

    pub fn map_memory(
        self: &Arc<Self>,
        addr: DesiredAddress,
        memory: Arc<MemoryObject>,
        memory_offset: u64,
        length: usize,
        prot_flags: ProtectionFlags,
        options: MappingOptions,
        name: MappingName,
    ) -> Result<UserAddress, Errno> {
        let flags = MappingFlags::from_access_flags_and_options(prot_flags, options);

        // Unmapped mappings must be released after the state is unlocked.
        let mut released_mappings = ReleasedMappings::default();
        // Hold the lock throughout the operation to uphold memory manager's invariants.
        // See mm/README.md.
        let mut state = self.state.write();
        let result = state.add_memory_mapping(
            self,
            addr,
            memory,
            memory_offset,
            length,
            flags,
            options.contains(MappingOptions::POPULATE),
            name,
            MappingMode::Eager,
            &mut released_mappings,
        );

        // Drop the state before the unmapped mappings, since dropping a mapping may acquire a lock
        // in `DirEntry`'s `drop`.
        released_mappings.finalize(state);

        result
    }

    pub fn map_anonymous(
        self: &Arc<Self>,
        addr: DesiredAddress,
        length: usize,
        prot_flags: ProtectionFlags,
        options: MappingOptions,
        name: MappingName,
    ) -> Result<UserAddress, Errno> {
        let mut released_mappings = ReleasedMappings::default();
        // Hold the lock throughout the operation to uphold memory manager's invariants.
        // See mm/README.md.
        let mut state = self.state.write();
        let result = state.map_anonymous(
            self,
            addr,
            length,
            prot_flags,
            options,
            name,
            &mut released_mappings,
        );

        released_mappings.finalize(state);

        result
    }

    /// Map the stack into a pre-selected address region
    pub fn map_stack(
        self: &Arc<Self>,
        length: usize,
        prot_flags: ProtectionFlags,
    ) -> Result<UserAddress, Errno> {
        assert!(length <= MAX_STACK_SIZE);
        let addr = (self.state.read().stack_origin - length)?;
        // The address range containing stack_origin should normally be available: it's above the
        // mmap_top, and this method is called early enough in the process lifetime that only the
        // main ELF and the interpreter are already loaded. However, in the rare case that the
        // static position-independent executable is overlapping the chosen address, mapping as Hint
        // will make mmap choose a new place for it.
        // TODO(https://fxbug.dev/370027241): Consider a more robust approach
        let stack_addr = self.map_anonymous(
            DesiredAddress::Hint(addr),
            length,
            prot_flags,
            MappingOptions::ANONYMOUS | MappingOptions::GROWSDOWN,
            MappingName::Stack,
        )?;
        if stack_addr != addr {
            log_warn!(
                "An address designated for stack ({}) was unavailable, mapping at {} instead.",
                addr,
                stack_addr
            );
        }
        Ok(stack_addr)
    }

    pub fn remap(
        self: &Arc<Self>,
        current_task: &CurrentTask,
        addr: UserAddress,
        old_length: usize,
        new_length: usize,
        flags: MremapFlags,
        new_addr: UserAddress,
    ) -> Result<UserAddress, Errno> {
        let mut released_mappings = ReleasedMappings::default();
        // Hold the lock throughout the operation to uphold memory manager's invariants.
        // See mm/README.md.
        let mut state = self.state.write();
        let result = state.remap(
            current_task,
            self,
            addr,
            old_length,
            new_length,
            flags,
            new_addr,
            &mut released_mappings,
        );

        released_mappings.finalize(state);

        result
    }

    pub fn unmap(self: &Arc<Self>, addr: UserAddress, length: usize) -> Result<(), Errno> {
        let mut released_mappings = ReleasedMappings::default();
        // Hold the lock throughout the operation to uphold memory manager's invariants.
        // See mm/README.md.
        let mut state = self.state.write();
        let result = state.unmap(self, addr, length, &mut released_mappings);

        released_mappings.finalize(state);

        result
    }

    pub fn protect(
        &self,
        current_task: &CurrentTask,
        addr: UserAddress,
        length: usize,
        prot_flags: ProtectionFlags,
    ) -> Result<(), Errno> {
        let page_size = *PAGE_SIZE;
        if !addr.is_aligned(page_size) {
            return error!(EINVAL);
        }
        if length == 0 {
            return Ok(());
        }
        let end = addr.checked_add(length).ok_or_else(|| errno!(ENOMEM))?.round_up(page_size)?;
        if end > self.maximum_valid_user_address {
            return error!(ENOMEM);
        }

        // Hold the lock throughout the operation to uphold memory manager's invariants.
        // See mm/README.md.
        let mut state = self.state.write();
        let mut released_mappings = ReleasedMappings::default();
        let result = state.protect(current_task, addr, length, prot_flags, &mut released_mappings);
        released_mappings.finalize(state);
        result
    }

    pub fn msync(
        &self,
        current_task: &CurrentTask,
        addr: UserAddress,
        length: usize,
        flags: MsyncFlags,
    ) -> Result<(), Errno> {
        // According to POSIX, either MS_SYNC or MS_ASYNC must be specified in flags,
        // and indeed failure to include one of these flags will cause msync() to fail
        // on some systems.  However, Linux permits a call to msync() that specifies
        // neither of these flags, with semantics that are (currently) equivalent to
        // specifying MS_ASYNC.

        // Both MS_SYNC and MS_ASYNC are set in flags
        if flags.contains(MsyncFlags::ASYNC) && flags.contains(MsyncFlags::SYNC) {
            return error!(EINVAL);
        }

        if !addr.is_aligned(*PAGE_SIZE) {
            return error!(EINVAL);
        }

        // We collect the nodes to sync first, release the memory manager lock, and then sync them.
        // This avoids holding the lock during blocking I/O operations (sync), which prevents
        // stalling other memory operations and avoids potential deadlocks.
        // It also allows us to deduplicate nodes, avoiding redundant sync calls for the same file.
        let mut nodes_to_sync = {
            let mm_state = self.state.read();

            let length_rounded = round_up_to_system_page_size(length)?;
            let end_addr = addr.checked_add(length_rounded).ok_or_else(|| errno!(EINVAL))?;

            let mut last_end = addr;
            let mut nodes = vec![];
            for (range, mapping) in mm_state.mappings.range(addr..end_addr) {
                // Check if there is a gap between the last mapped address and the current mapping.
                // msync requires the entire range to be mapped, so any gap results in ENOMEM.
                if range.start > last_end {
                    return error!(ENOMEM);
                }
                last_end = range.end;

                if flags.contains(MsyncFlags::INVALIDATE)
                    && mapping.flags().contains(MappingFlags::LOCKED)
                {
                    return error!(EBUSY);
                }

                if flags.contains(MsyncFlags::SYNC) {
                    if let MappingNameRef::File(file_mapping) = mapping.name() {
                        nodes.push(file_mapping.node().clone());
                    }
                }
            }
            if last_end < end_addr {
                return error!(ENOMEM);
            }
            nodes
        };

        // Deduplicate nodes to avoid redundant sync calls.
        nodes_to_sync.sort_by_key(|n| Arc::as_ptr(n) as usize);
        nodes_to_sync.dedup_by(|a, b| Arc::ptr_eq(a, b));

        for node in nodes_to_sync {
            // Range-based sync is non-trivial for Fxfs to support due to its complicated
            // reservation system (b/322874588#comment5). Naive range-based sync could exhaust
            // space reservations if called page-by-page, as transaction costs are based on the
            // number of dirty pages rather than file ranges. We use whole-file sync for now
            // to ensure data durability without adding excessive complexity.
            node.ops().sync(&node, current_task)?;
        }
        Ok(())
    }

    pub fn madvise(&self, addr: UserAddress, length: usize, advice: u32) -> Result<(), Errno> {
        let mut state = self.state.write();
        let mut released_mappings = ReleasedMappings::default();
        let result =
            state.madvise(&self.mapping_context, addr, length, advice, &mut released_mappings);
        released_mappings.finalize(state);
        result
    }

    pub fn mlock(
        &self,
        current_task: &CurrentTask,
        desired_addr: UserAddress,
        desired_length: usize,
        on_fault: bool,
    ) -> Result<(), Errno> {
        let mut state = self.state.write();
        let mut released_mappings = ReleasedMappings::default();
        let result = state.mlock(
            &self.mapping_context,
            current_task,
            desired_addr,
            desired_length,
            on_fault,
            &mut released_mappings,
        );
        released_mappings.finalize(state);
        result
    }

    pub fn munlock(
        &self,
        current_task: &CurrentTask,
        desired_addr: UserAddress,
        desired_length: usize,
    ) -> Result<(), Errno> {
        let mut state = self.state.write();
        let mut released_mappings = ReleasedMappings::default();
        let result =
            state.munlock(current_task, desired_addr, desired_length, &mut released_mappings);
        released_mappings.finalize(state);
        result
    }

    pub fn log_memory_map(&self, task: &Task, fault_address: UserAddress) {
        let state = self.state.read();
        log_warn!("Memory map for pid={}:", task.pid);
        let mut last_end = UserAddress::from_ptr(0);
        for (range, map) in state.mappings.iter() {
            if fault_address >= last_end && fault_address < range.start {
                log_warn!("{:08x} <= FAULT", fault_address.ptr());
            }

            let perms = format!(
                "{}{}{}{}",
                if map.can_read() { 'r' } else { '-' },
                if map.can_write() { 'w' } else { '-' },
                if map.can_exec() { 'x' } else { '-' },
                if map.flags().contains(MappingFlags::SHARED) { 's' } else { 'p' }
            );

            let backing = match state.get_mapping_backing(map) {
                MappingBacking::Memory(backing) => backing.address_to_offset(range.start),
                MappingBacking::PrivateAnonymous => 0,
            };

            let name_str = match &map.name() {
                MappingNameRef::File(file) => {
                    let Ok(running_state) = task.running_state() else {
                        log_warn!("Task {} is not running", task.get_tid());
                        continue;
                    };
                    String::from_utf8_lossy(&file.name().path(&running_state.fs())).into_owned()
                }
                MappingNameRef::None | MappingNameRef::AioContext(_) => {
                    if map.flags().contains(MappingFlags::SHARED)
                        && map.flags().contains(MappingFlags::ANONYMOUS)
                    {
                        "/dev/zero (deleted)".to_string()
                    } else {
                        "".to_string()
                    }
                }
                MappingNameRef::Stack => "[stack]".to_string(),
                MappingNameRef::Heap => "[heap]".to_string(),
                MappingNameRef::Vdso => "[vdso]".to_string(),
                MappingNameRef::Vvar => "[vvar]".to_string(),
                _ => format!("{:?}", map.name()),
            };

            let fault_marker = if range.contains(&fault_address) { " <= FAULT" } else { "" };

            log_warn!(
                "{:08x}-{:08x} {} {:08x} {}{}",
                range.start.ptr(),
                range.end.ptr(),
                perms,
                backing,
                name_str,
                fault_marker
            );
            last_end = range.end;
        }

        if fault_address >= last_end {
            log_warn!("{:08x} <= FAULT", fault_address.ptr());
        }
    }

    pub fn handle_page_fault(
        self: &Arc<Self>,
        decoded: PageFaultExceptionReport,
        error_code: zx::Status,
    ) -> ExceptionResult {
        #[cfg(target_arch = "aarch64")]
        // On aarch64, 64-bit processes can use Top Byte Ignore (TBI). We need to mask out the
        // top byte of the faulting address to get the actual userspace address.
        let addr = if self.is_arch64() {
            UserAddress::from(decoded.faulting_address & 0x00FF_FFFF_FFFF_FFFF)
        } else {
            UserAddress::from(decoded.faulting_address)
        };
        #[cfg(not(target_arch = "aarch64"))]
        let addr = UserAddress::from(decoded.faulting_address);

        // On uffd-registered range, handle according to the uffd rules
        if error_code == zx::Status::ACCESS_DENIED {
            let state = self.state.write();
            if let Some((_, mapping)) = state.mappings.get(addr) {
                if mapping.flags().contains(MappingFlags::UFFD) {
                    // TODO(https://fxbug.dev/391599171): Support other modes
                    assert!(mapping.flags().contains(MappingFlags::UFFD_MISSING));

                    if let Some(_uffd) = state.find_uffd(addr) {
                        // If the SIGBUS feature was set, no event will be sent to the file.
                        // Instead, SIGBUS is delivered to the process that triggered the fault.
                        // TODO(https://fxbug.dev/391599171): For now we only support this feature,
                        // so we assume it is set.
                        // Check for the SIGBUS feature when we start supporting running without it.
                        return ExceptionResult::Signal(SignalInfo::with_detail(
                            SIGBUS,
                            BUS_ADRERR as i32,
                            SignalDetail::SigFault { addr: decoded.faulting_address },
                        ));
                    };
                }
                // There is a data race resulting from uffd unregistration and page fault happening
                // at the same time. To detect it, we check if the access was meant to be rejected
                // according to Starnix own information about the mapping.
                if check_access_permissions_in_page_fault(&decoded, mapping) {
                    track_stub!(
                        TODO("https://fxbug.dev/435171399"),
                        "Inconsistent permission fault"
                    );
                    return ExceptionResult::Handled;
                }
            }
            std::mem::drop(state);
        }

        if decoded.not_present {
            {
                let mut state = self.state.write();
                match state.ensure_range_mapped_in_user_vmar(addr, None, &self.mapping_context) {
                    Ok(true) => return ExceptionResult::Handled,
                    Ok(false) => {
                        // If the mapping generation has changed since the last time this thread
                        // saw it, we return `Handled` to retry the faulting instruction.
                        // This handles cases where the fault was spurious due to a concurrent
                        // mapping operation. We update the counter here to ensure we converge and
                        // don't loop infinitely.
                        let current_gen = state.mappings.generation;
                        let old_gen = LAST_SEEN_MAPPING_GENERATION.with(|c| c.replace(current_gen));
                        if current_gen != old_gen {
                            return ExceptionResult::Handled;
                        }
                    }
                    Err(e) => {
                        log_error!("Failed to map lazy memory: {e}")
                    }
                }
            }

            // A page fault may be resolved by extending a growsdown mapping to cover the faulting
            // address. Mark the exception handled if so. Otherwise let the regular handling proceed.

            // We should only attempt growth on a not-present fault and we should only extend if the
            // access type matches the protection on the GROWSDOWN mapping.
            match self.extend_growsdown_mapping_to_address(
                UserAddress::from(decoded.faulting_address),
                decoded.is_write,
            ) {
                Ok(true) => {
                    return ExceptionResult::Handled;
                }
                Err(e) => {
                    log_warn!("Error handling page fault: {e}")
                }
                _ => {}
            }
        }

        // For this exception type, the synth_code field in the exception report's context is the
        // error generated by the page fault handler. For us this is used to distinguish between a
        // segmentation violation and a bus error. Unfortunately this detail is not documented in
        // Zircon's public documentation and is only described in the architecture-specific
        // exception definitions such as:
        // zircon/kernel/arch/x86/include/arch/x86.h
        // zircon/kernel/arch/arm64/include/arch/arm64.h
        let (signo, si_code) = match error_code {
            zx::Status::OUT_OF_RANGE => (SIGBUS, linux_uapi::BUS_ADRERR as i32),
            _ => {
                let code = if self.state.read().mappings.get(addr).is_some() {
                    linux_uapi::SEGV_ACCERR
                } else {
                    linux_uapi::SEGV_MAPERR
                };
                (SIGSEGV, code as i32)
            }
        };
        ExceptionResult::Signal(SignalInfo::with_detail(
            signo,
            si_code,
            SignalDetail::SigFault { addr: decoded.faulting_address },
        ))
    }

    pub fn set_mapping_name(
        &self,
        addr: UserAddress,
        length: usize,
        name: Option<FsString>,
    ) -> Result<(), Errno> {
        let mut state = self.state.write();
        let mut released_mappings = ReleasedMappings::default();
        let result = state.set_mapping_name(addr, length, name, &mut released_mappings);
        released_mappings.finalize(state);
        result
    }

    /// Returns [`Ok`] if the entire range specified by `addr..(addr+length)` contains valid
    /// mappings.
    ///
    /// # Errors
    ///
    /// Returns [`Err(errno)`] where `errno` is:
    ///
    ///   - `EINVAL`: `addr` is not page-aligned, or the range is too large,
    ///   - `ENOMEM`: one or more pages in the range are not mapped.
    pub fn ensure_mapped(&self, addr: UserAddress, length: usize) -> Result<(), Errno> {
        if !addr.is_aligned(*PAGE_SIZE) {
            return error!(EINVAL);
        }

        let length = round_up_to_system_page_size(length)?;
        let end_addr = addr.checked_add(length).ok_or_else(|| errno!(EINVAL))?;
        let state = self.state.read();
        let mut last_end = addr;
        for (range, _) in state.mappings.range(addr..end_addr) {
            if range.start > last_end {
                // This mapping does not start immediately after the last.
                return error!(ENOMEM);
            }
            last_end = range.end;
        }
        if last_end < end_addr {
            // There is a gap of no mappings at the end of the range.
            error!(ENOMEM)
        } else {
            Ok(())
        }
    }

    /// Returns the memory object mapped at the address and the offset into the memory object of
    /// the address. Intended for implementing futexes.
    pub fn get_mapping_memory(
        &self,
        addr: UserAddress,
        perms: ProtectionFlags,
    ) -> Result<(Arc<MemoryObject>, u64), Errno> {
        let state = self.state.read();
        let (_, mapping) = state.mappings.get(addr).ok_or_else(|| errno!(EFAULT))?;
        if !mapping.flags().access_flags().contains(perms) {
            return error!(EACCES);
        }
        match state.get_mapping_backing(mapping) {
            MappingBacking::Memory(backing) => {
                Ok((Arc::clone(backing.memory()), mapping.address_to_offset(addr)))
            }
            MappingBacking::PrivateAnonymous => {
                Ok((Arc::clone(&self.mapping_context.private_anonymous.backing), addr.ptr() as u64))
            }
        }
    }

    /// Does a rough check that the given address is plausibly in the address space of the
    /// application. This does not mean the pointer is valid for any particular purpose or that
    /// it will remain so!
    ///
    /// In some syscalls, Linux seems to do some initial validation of the pointer up front to
    /// tell the caller early if it's invalid. For example, in epoll_wait() it's returning a vector
    /// of events. If the caller passes an invalid pointer, it wants to fail without dropping any
    /// events. Failing later when actually copying the required events to userspace would mean
    /// those events will be lost. But holding a lock on the memory manager for an asynchronous
    /// wait is not desirable.
    ///
    /// Testing shows that Linux seems to do some initial plausibility checking of the pointer to
    /// be able to report common usage errors before doing any (possibly unreversable) work. This
    /// checking is easy to get around if you try, so this function is also not required to
    /// be particularly robust. Certainly the more advanced cases of races (the memory could be
    /// unmapped after this call but before it's used) are not handled.
    ///
    /// The buffer_size variable is the size of the data structure that needs to fit
    /// in the given memory.
    ///
    /// Returns the error EFAULT if invalid.
    pub fn check_plausible(&self, addr: UserAddress, buffer_size: usize) -> Result<(), Errno> {
        let state = self.state.read();

        if let Some(range) = state.mappings.last_range() {
            if (range.end - buffer_size)? >= addr {
                return Ok(());
            }
        }
        error!(EFAULT)
    }

    pub fn get_aio_context(&self, addr: UserAddress) -> Option<Arc<AioContext>> {
        let state = self.state.read();
        state.get_aio_context(addr).map(|(_, aio_context)| aio_context)
    }

    pub fn destroy_aio_context(
        self: &Arc<Self>,
        addr: UserAddress,
    ) -> Result<Arc<AioContext>, Errno> {
        let mut released_mappings = ReleasedMappings::default();

        // Hold the lock throughout the operation to uphold memory manager's invariants.
        // See mm/README.md.
        let mut state = self.state.write();

        // Validate that this address actually has an AioContext. We need to hold the state lock
        // until we actually remove the mappings to ensure that another thread does not manipulate
        // the mappings after we've validated that they contain an AioContext.
        let Some((range, aio_context)) = state.get_aio_context(addr) else {
            return error!(EINVAL);
        };

        let length = range.end - range.start;
        let result = state.unmap(self, range.start, length, &mut released_mappings);

        released_mappings.finalize(state);

        result.map(|_| aio_context)
    }

    #[cfg(test)]
    pub fn get_mapping_name(
        &self,
        addr: UserAddress,
    ) -> Result<Option<flyweights::FlyByteStr>, Errno> {
        let state = self.state.read();
        let (_, mapping) = state.mappings.get(addr).ok_or_else(|| errno!(EFAULT))?;
        if let MappingNameRef::Vma(name) = mapping.name() {
            Ok(Some(name.clone()))
        } else {
            Ok(None)
        }
    }

    #[cfg(test)]
    pub fn get_mapping_count(&self) -> usize {
        let state = self.state.read();
        state.mappings.iter().count()
    }

    pub fn extend_growsdown_mapping_to_address(
        self: &Arc<Self>,
        addr: UserAddress,
        is_write: bool,
    ) -> Result<bool, Error> {
        self.state.write().extend_growsdown_mapping_to_address(self, addr, is_write)
    }

    pub fn get_total_usage(&self) -> usize {
        self.state.read().mappings.total_usage
    }

    pub fn get_stats(&self, current_task: &CurrentTask) -> MemoryStats {
        // Acquiring stats is an intensive operation, so when diagnostic processes request
        // stats of a process in fast succession, we return a short-lived cached value.
        // However, if the process is inspecting itself, bypass the cache to ensure immediate
        // causal consistency (e.g. after mmap, munmap, or memory writes).
        let is_self_read =
            current_task.mm().map_or(false, |mm| std::ptr::eq(Arc::as_ptr(&mm), self));
        let now = zx::MonotonicInstant::get();
        if !is_self_read {
            const CACHE_TTL: zx::MonotonicDuration = zx::MonotonicDuration::from_millis(250);
            let cached = self.cached_stats.lock();
            if let Some((timestamp, stats)) = *cached {
                if now - timestamp < CACHE_TTL {
                    return stats;
                }
            }
        }

        // Grab our state lock before reading zircon mappings so that the two are consistent.
        // Other Starnix threads should not make any changes to the Zircon mappings while we hold
        // a read lock to the memory manager state.
        let state = self.state.read();

        let mut stats = MemoryStats::default();
        stats.vm_stack = state.stack_size;

        self.with_zx_mappings(current_task, |zx_mappings| {
            for zx_mapping in zx_mappings {
                // We only care about map info for actual mappings.
                let zx_details = zx_mapping.details();
                let Some(zx_details) = zx_details.as_mapping() else { continue };
                let user_address = UserAddress::from(zx_mapping.base as u64);
                let (_, mm_mapping) = state
                    .mappings
                    .get(user_address)
                    .unwrap_or_else(|| panic!("mapping bookkeeping must be consistent with zircon's: not found: {user_address:?}"));
                debug_assert_eq!(
                    match state.get_mapping_backing(mm_mapping) {
                        MappingBacking::Memory(m)=>m.memory().get_koid(),
                        MappingBacking::PrivateAnonymous=>self.mapping_context.private_anonymous.backing.get_koid(),
                    },
                    zx_details.vmo_koid,
                    "MemoryManager and Zircon must agree on which VMO is mapped in this range",
                );

                stats.vm_size += zx_mapping.size;

                stats.vm_rss += zx_details.committed_bytes;
                stats.vm_swap += zx_details.populated_bytes - zx_details.committed_bytes;

                if mm_mapping.flags().contains(MappingFlags::SHARED) {
                    stats.rss_shared += zx_details.committed_bytes;
                } else if mm_mapping.flags().contains(MappingFlags::ANONYMOUS) {
                    stats.rss_anonymous += zx_details.committed_bytes;
                } else if mm_mapping.name().is_file() {
                    stats.rss_file += zx_details.committed_bytes;
                }

                if mm_mapping.flags().contains(MappingFlags::LOCKED) {
                    stats.vm_lck += zx_details.committed_bytes;
                }

                if mm_mapping.flags().contains(MappingFlags::ELF_BINARY)
                    && mm_mapping.flags().contains(MappingFlags::WRITE)
                {
                    stats.vm_data += zx_mapping.size;
                }

                if mm_mapping.flags().contains(MappingFlags::ELF_BINARY)
                    && mm_mapping.flags().contains(MappingFlags::EXEC)
                {
                    stats.vm_exe += zx_mapping.size;
                }
            }
        });

        // TODO(https://fxbug.dev/396221597): Placeholder for now. We need kernel support to track
        // the committed bytes high water mark.
        stats.vm_rss_hwm = STUB_VM_RSS_HWM;
        *self.cached_stats.lock() = Some((now, stats));
        stats
    }

    fn run_atomic_op<F, T>(&self, futex_addr: FutexAddress, mut op: F) -> Result<T, Errno>
    where
        F: FnMut(&usercopy::Usercopy) -> Result<T, ()>,
    {
        let uc = usercopy();
        // Try the lock-free fast path first.
        // Note: `op` returns `Err(())` strictly on memory access faults. For
        // compare-exchange operations, a logical mismatch is wrapped inside a
        // successful `Ok(value_or_error)`, meaning we will short-circuit here
        // and won't incorrectly retry on logical failures.
        if let Ok(val) = op(uc) {
            return Ok(val);
        }
        self.ensure_range_mapped_in_user_vmar(futex_addr.into(), None)?;
        op(uc).map_err(|_| errno!(EFAULT))
    }

    pub fn atomic_load_u32_acquire(&self, futex_addr: FutexAddress) -> Result<u32, Errno> {
        self.run_atomic_op(futex_addr, |uc| uc.atomic_load_u32_acquire(futex_addr.ptr()))
    }

    pub fn atomic_load_u32_relaxed(&self, futex_addr: FutexAddress) -> Result<u32, Errno> {
        self.run_atomic_op(futex_addr, |uc| uc.atomic_load_u32_relaxed(futex_addr.ptr()))
    }

    pub fn atomic_store_u32_relaxed(
        &self,
        futex_addr: FutexAddress,
        value: u32,
    ) -> Result<(), Errno> {
        self.run_atomic_op(futex_addr, |uc| uc.atomic_store_u32_relaxed(futex_addr.ptr(), value))
    }

    pub fn atomic_compare_exchange_u32_acq_rel(
        &self,
        futex_addr: FutexAddress,
        current: u32,
        new: u32,
    ) -> CompareExchangeResult<u32> {
        CompareExchangeResult::from_usercopy(self.run_atomic_op(futex_addr, |uc| {
            uc.atomic_compare_exchange_u32_acq_rel(futex_addr.ptr(), current, new)
        }))
    }

    pub fn atomic_compare_exchange_weak_u32_acq_rel(
        &self,
        futex_addr: FutexAddress,
        current: u32,
        new: u32,
    ) -> CompareExchangeResult<u32> {
        CompareExchangeResult::from_usercopy(self.run_atomic_op(futex_addr, |uc| {
            uc.atomic_compare_exchange_weak_u32_acq_rel(futex_addr.ptr(), current, new)
        }))
    }
}

/// The result of an atomic compare/exchange operation on user memory.
#[derive(Debug, Clone)]
pub enum CompareExchangeResult<T> {
    /// The current value provided matched the one observed in memory and the new value provided
    /// was written.
    Success,
    /// The provided current value did not match the current value in memory.
    Stale { observed: T },
    /// There was a general error while accessing the requested memory.
    Error(Errno),
}

impl<T> CompareExchangeResult<T> {
    fn from_usercopy(res: Result<Result<T, T>, Errno>) -> Self {
        match res {
            Ok(Ok(_)) => Self::Success,
            Ok(Err(observed)) => Self::Stale { observed },
            Err(e) => Self::Error(e),
        }
    }
}

impl<T> From<Errno> for CompareExchangeResult<T> {
    fn from(e: Errno) -> Self {
        Self::Error(e)
    }
}

/// The user-space address at which a mapping should be placed. Used by [`MemoryManager::map`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesiredAddress {
    /// Map at any address chosen by the kernel.
    Any,
    /// The address is a hint. If the address overlaps an existing mapping a different address may
    /// be chosen.
    Hint(UserAddress),
    /// The address is a requirement. If the address overlaps an existing mapping (and cannot
    /// overwrite it), mapping fails.
    Fixed(UserAddress),
    /// The address is a requirement. If the address overlaps an existing mapping (and cannot
    /// overwrite it), they should be unmapped.
    FixedOverwrite(UserAddress),
}

/// The user-space address at which a mapping should be placed. Used by [`map_in_vmar`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectedAddress {
    /// See DesiredAddress::Fixed.
    Fixed(UserAddress),
    /// See DesiredAddress::FixedOverwrite.
    FixedOverwrite(UserAddress),
}

impl SelectedAddress {
    fn addr(&self) -> UserAddress {
        match self {
            SelectedAddress::Fixed(addr) => *addr,
            SelectedAddress::FixedOverwrite(addr) => *addr,
        }
    }
}

/// Write one line of the memory map intended for adding to `/proc/self/maps`.
fn write_map(
    task: &Task,
    fs_context: Option<&FsContext>,
    sink: &mut DynamicFileBuf,
    state: &MemoryManagerState,
    range: &Range<UserAddress>,
    map: &Mapping,
) -> Result<(), Errno> {
    let line_length = write!(
        sink,
        "{:08x}-{:08x} {}{}{}{} {:08x} 00:00 {} ",
        range.start.ptr(),
        range.end.ptr(),
        if map.can_read() { 'r' } else { '-' },
        if map.can_write() { 'w' } else { '-' },
        if map.can_exec() { 'x' } else { '-' },
        if map.flags().contains(MappingFlags::SHARED) { 's' } else { 'p' },
        match state.get_mapping_backing(map) {
            MappingBacking::Memory(backing) => backing.address_to_offset(range.start),
            MappingBacking::PrivateAnonymous => 0,
        },
        if let MappingNameRef::File(file) = &map.name() { file.node().ino } else { 0 }
    )?;
    let fill_to_name = |sink: &mut DynamicFileBuf| {
        // The filename goes at >= the 74th column (73rd when zero indexed)
        for _ in line_length..73 {
            sink.write(b" ");
        }
    };
    match &map.name() {
        MappingNameRef::None | MappingNameRef::AioContext(_) => {
            if map.flags().contains(MappingFlags::SHARED)
                && map.flags().contains(MappingFlags::ANONYMOUS)
            {
                // See proc(5), "/proc/[pid]/map_files/"
                fill_to_name(sink);
                sink.write(b"/dev/zero (deleted)");
            }
        }
        MappingNameRef::Stack => {
            fill_to_name(sink);
            sink.write(b"[stack]");
        }
        MappingNameRef::Heap => {
            fill_to_name(sink);
            sink.write(b"[heap]");
        }
        MappingNameRef::Vdso => {
            fill_to_name(sink);
            sink.write(b"[vdso]");
        }
        MappingNameRef::Vvar => {
            fill_to_name(sink);
            sink.write(b"[vvar]");
        }
        MappingNameRef::File(file) => {
            fill_to_name(sink);
            // File names can have newlines that need to be escaped before printing.
            // According to https://man7.org/linux/man-pages/man5/proc.5.html the only
            // escaping applied to paths is replacing newlines with an octal sequence.
            let path = if let Some(fs_context) = fs_context {
                file.name().path(fs_context)
            } else {
                file.name().path(&task.running_state()?.fs())
            };
            sink.write_iter(
                path.iter()
                    .flat_map(|b| if *b == b'\n' { b"\\012" } else { std::slice::from_ref(b) })
                    .copied(),
            );
        }
        MappingNameRef::Vma(name) => {
            fill_to_name(sink);
            sink.write(b"[anon:");
            sink.write(name.as_bytes());
            sink.write(b"]");
        }
        MappingNameRef::Ashmem(name) => {
            fill_to_name(sink);
            sink.write(b"/dev/ashmem/");
            sink.write(name.as_bytes());
        }
    }
    sink.write(b"\n");
    Ok(())
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MemoryStats {
    pub vm_size: usize,
    pub vm_rss: usize,
    pub vm_rss_hwm: usize,
    pub rss_anonymous: usize,
    pub rss_file: usize,
    pub rss_shared: usize,
    pub vm_data: usize,
    pub vm_stack: usize,
    pub vm_exe: usize,
    pub vm_swap: usize,
    pub vm_lck: usize,
}

/// Implements `/proc/self/maps`.
#[derive(Clone)]
pub struct ProcMapsFile {
    mm: Weak<MemoryManager>,
    task: Weak<Task>,
}
impl ProcMapsFile {
    pub fn new(task: Arc<Task>) -> DynamicFile<Self> {
        // "maps" is empty for kthreads, rather than inaccessible.
        let mm = task.mm().map_or_else(|_| Weak::default(), |mm| Arc::downgrade(&mm));
        DynamicFile::new(Self { mm, task: Arc::downgrade(&task) })
    }
}

impl SequenceFileSource for ProcMapsFile {
    type Cursor = UserAddress;

    fn next(
        &self,
        _current_task: &CurrentTask,
        cursor: UserAddress,
        sink: &mut DynamicFileBuf,
    ) -> Result<Option<UserAddress>, Errno> {
        let task = Task::from_weak(&self.task)?;
        // /proc/<pid>/maps is empty for kthreads and tasks whose memory manager has changed.
        let Some(mm) = self.mm.upgrade() else {
            return Ok(None);
        };
        let state = mm.state.read();
        if let Some((range, map)) = state.mappings.find_at_or_after(cursor) {
            let fs_context = task.running_state().ok().map(|rs| rs.fs());
            write_map(&task, fs_context.as_deref(), sink, &state, range, map)?;
            return Ok(Some(range.end));
        }
        Ok(None)
    }
}

#[derive(Clone)]
pub struct ProcSmapsFile {
    mm: Weak<MemoryManager>,
    task: Weak<Task>,
}
impl ProcSmapsFile {
    pub fn new(task: Arc<Task>) -> DynamicFile<Self> {
        // "smaps" is empty for kthreads, rather than inaccessible.
        let mm = task.mm().map_or_else(|_| Weak::default(), |mm| Arc::downgrade(&mm));
        DynamicFile::new(Self { mm, task: Arc::downgrade(&task) })
    }
}

impl DynamicFileSource for ProcSmapsFile {
    fn generate(&self, current_task: &CurrentTask, sink: &mut DynamicFileBuf) -> Result<(), Errno> {
        let page_size_kb = *PAGE_SIZE / 1024;
        let task = Task::from_weak(&self.task)?;
        // /proc/<pid>/smaps is empty for kthreads and tasks whose memory manager has changed.
        let Some(mm) = self.mm.upgrade() else {
            return Ok(());
        };

        // Ensure any lazy mappings are mapped into the user vmar so their committed
        // bytes can be discovered through Zircon map info. We only map actual mappings
        // in `state.mappings` rather than the entire 48-bit address space.
        let lazy_ranges: SmallVec<[_; 4]> = {
            let state = mm.state.read();
            state
                .mappings
                .iter()
                .filter(|(_, m)| m.mapping_mode() == MappingMode::Lazy)
                .map(|(range, _)| (range.start, Some(range.end - range.start)))
                .collect()
        };
        if !lazy_ranges.is_empty() {
            mm.state.write().ensure_ranges_mapped_in_user_vmar(lazy_ranges, &mm.mapping_context)?;
        }

        let state = mm.state.read();
        let committed_bytes_vec = mm.with_zx_mappings(current_task, |zx_mappings| {
            let mut zx_memory_info = RangeMap::<UserAddress, usize>::default();
            for idx in 0..zx_mappings.len() {
                let zx_mapping = zx_mappings[idx];
                // RangeMap uses #[must_use] for its default usecase but this drop is trivial.
                let _ = zx_memory_info.insert(
                    UserAddress::from_ptr(zx_mapping.base)
                        ..UserAddress::from_ptr(zx_mapping.base + zx_mapping.size),
                    idx,
                );
            }

            let mut committed_bytes_vec = Vec::new();
            for (mm_range, mm_mapping) in state.mappings.iter() {
                let mut committed_bytes = 0;

                for (zx_range, zx_mapping_idx) in zx_memory_info.range(mm_range.clone()) {
                    let intersect_range = zx_range.intersect(mm_range);
                    let zx_mapping = zx_mappings[*zx_mapping_idx];
                    let zx_details = zx_mapping.details();
                    let Some(zx_details) = zx_details.as_mapping() else { continue };
                    let zx_committed_bytes = zx_details.committed_bytes;

                    // TODO(https://fxbug.dev/419882465): It can happen that the same Zircon mapping
                    // is covered by more than one Starnix mapping. In this case we don't have
                    // enough granularity to answer the question of how many committed bytes belong
                    // to one mapping or another. Make a best-effort approximation by dividing the
                    // committed bytes of a Zircon mapping proportionally.
                    committed_bytes += if intersect_range != *zx_range {
                        let intersection_size =
                            intersect_range.end.ptr() - intersect_range.start.ptr();
                        let part = intersection_size as f32 / zx_mapping.size as f32;
                        let prorated_committed_bytes: f32 = part * zx_committed_bytes as f32;
                        prorated_committed_bytes as u64
                    } else {
                        zx_committed_bytes as u64
                    };
                    assert_eq!(
                        match state.get_mapping_backing(mm_mapping) {
                            MappingBacking::Memory(m) => m.memory().get_koid(),
                            MappingBacking::PrivateAnonymous =>
                                mm.mapping_context.private_anonymous.backing.get_koid(),
                        },
                        zx_details.vmo_koid,
                        "MemoryManager and Zircon must agree on which VMO is mapped in this range",
                    );
                }
                committed_bytes_vec.push(committed_bytes);
            }
            Ok(committed_bytes_vec)
        })?;

        let fs_context = task.running_state().ok().map(|rs| rs.fs());
        let fs_context_ref = fs_context.as_deref();
        for ((mm_range, mm_mapping), committed_bytes) in
            state.mappings.iter().zip(committed_bytes_vec.into_iter())
        {
            write_map(&task, fs_context_ref, sink, &state, mm_range, mm_mapping)?;

            let size_kb = (mm_range.end.ptr() - mm_range.start.ptr()) / 1024;
            writeln!(sink, "Size:           {size_kb:>8} kB",)?;
            let share_count = match state.get_mapping_backing(mm_mapping) {
                MappingBacking::Memory(backing) => {
                    let memory = backing.memory();
                    if memory.is_clock() {
                        // Clock memory mappings are not shared in a meaningful way.
                        1
                    } else {
                        let memory_info = backing.memory().info()?;
                        memory_info.share_count as u64
                    }
                }
                MappingBacking::PrivateAnonymous => {
                    1 // Private mapping
                }
            };

            let rss_kb = committed_bytes / 1024;
            writeln!(sink, "Rss:            {rss_kb:>8} kB")?;

            let pss_kb = if mm_mapping.flags().contains(MappingFlags::SHARED) {
                rss_kb / share_count
            } else {
                rss_kb
            };
            writeln!(sink, "Pss:            {pss_kb:>8} kB")?;

            track_stub!(TODO("https://fxbug.dev/322874967"), "smaps dirty pages");
            let (shared_dirty_kb, private_dirty_kb) = (0, 0);

            let is_shared = share_count > 1;
            let shared_clean_kb = if is_shared { rss_kb } else { 0 };
            writeln!(sink, "Shared_Clean:   {shared_clean_kb:>8} kB")?;
            writeln!(sink, "Shared_Dirty:   {shared_dirty_kb:>8} kB")?;

            let private_clean_kb = if is_shared { 0 } else { rss_kb };
            writeln!(sink, "Private_Clean:  {private_clean_kb:>8} kB")?;
            writeln!(sink, "Private_Dirty:  {private_dirty_kb:>8} kB")?;

            let anonymous_kb = if mm_mapping.private_anonymous() { rss_kb } else { 0 };
            writeln!(sink, "Anonymous:      {anonymous_kb:>8} kB")?;
            writeln!(sink, "KernelPageSize: {page_size_kb:>8} kB")?;
            writeln!(sink, "MMUPageSize:    {page_size_kb:>8} kB")?;

            let locked_kb =
                if mm_mapping.flags().contains(MappingFlags::LOCKED) { rss_kb } else { 0 };
            writeln!(sink, "Locked:         {locked_kb:>8} kB")?;
            writeln!(sink, "VmFlags: {}", mm_mapping.vm_flags())?;

            track_stub!(TODO("https://fxbug.dev/297444691"), "optional smaps fields");
        }

        Ok(())
    }
}

/// Implements `/proc/<pid>/smaps_rollup`.
#[derive(Clone)]
pub struct ProcSmapsRollupFile {
    mm: Weak<MemoryManager>,
    task: Weak<Task>,
}
impl ProcSmapsRollupFile {
    // Linux 6.6 allows open() without an mm and fails with ESRCH on read(). Linux 6.11+
    // fails with ESRCH on open(). Match Linux 6.6 since Starnix targets 6.6.
    pub fn new(task: Arc<Task>) -> DynamicFile<Self> {
        let mm = task.mm().map_or_else(|_| Weak::default(), |mm| Arc::downgrade(&mm));
        DynamicFile::new(Self { mm, task: Arc::downgrade(&task) })
    }
}
impl DynamicFileSource for ProcSmapsRollupFile {
    fn generate(&self, current_task: &CurrentTask, sink: &mut DynamicFileBuf) -> Result<(), Errno> {
        let _task = Task::from_weak(&self.task)?;
        let Some(mm) = self.mm.upgrade() else {
            return error!(ESRCH);
        };

        let mem_stats = mm.get_stats(current_task);
        let rss_kb = mem_stats.vm_rss / 1024;
        let anon_kb = mem_stats.rss_anonymous / 1024;
        let file_kb = mem_stats.rss_file / 1024;
        let shmem_kb = mem_stats.rss_shared / 1024;
        let swap_kb = mem_stats.vm_swap / 1024;
        let locked_kb = mem_stats.vm_lck / 1024;

        // Anonymous memory is private dirty (heap/stack/anon).
        let private_dirty_kb = anon_kb;
        // File-backed and shared memory are clean pages shared across processes.
        let shared_clean_kb = file_kb + shmem_kb;
        let private_clean_kb = 0;
        // Proportional Set Size: private memory + proportional share of shared clean pages.
        // Assuming an average sharing factor of ~5 across system processes for file mappings.
        let pss_file_kb = file_kb / 5;
        let pss_shmem_kb = shmem_kb / 5;
        let pss_kb = anon_kb + pss_file_kb + pss_shmem_kb;

        writeln!(sink, "00000000-ffffffffffffffff ---p 00000000 00:00 0 [rollup]")?;
        writeln!(sink, "Rss:            {rss_kb:>8} kB")?;
        writeln!(sink, "Pss:            {pss_kb:>8} kB")?;
        writeln!(sink, "Pss_Dirty:             0 kB")?;
        writeln!(sink, "Pss_Anon:       {anon_kb:>8} kB")?;
        writeln!(sink, "Pss_File:       {pss_file_kb:>8} kB")?;
        writeln!(sink, "Pss_Shmem:      {pss_shmem_kb:>8} kB")?;
        writeln!(sink, "Shared_Clean:   {shared_clean_kb:>8} kB")?;
        writeln!(sink, "Shared_Dirty:          0 kB")?;
        writeln!(sink, "Private_Clean:  {private_clean_kb:>8} kB")?;
        writeln!(sink, "Private_Dirty:  {private_dirty_kb:>8} kB")?;
        writeln!(sink, "Referenced:     {rss_kb:>8} kB")?;
        writeln!(sink, "Anonymous:      {anon_kb:>8} kB")?;
        writeln!(sink, "KSM:                   0 kB")?;
        writeln!(sink, "LazyFree:              0 kB")?;
        writeln!(sink, "AnonHugePages:         0 kB")?;
        writeln!(sink, "ShmemPmdMapped:        0 kB")?;
        writeln!(sink, "FilePmdMapped:         0 kB")?;
        writeln!(sink, "Shared_Hugetlb:        0 kB")?;
        writeln!(sink, "Private_Hugetlb:       0 kB")?;
        writeln!(sink, "Swap:           {swap_kb:>8} kB")?;
        writeln!(sink, "SwapPss:        {swap_kb:>8} kB")?;
        writeln!(sink, "Locked:         {locked_kb:>8} kB")?;
        Ok(())
    }
}

const PAGEMAP_PFN_MASK: u64 = (1u64 << 55) - 1;

/// Computes a synthetic 55-bit Page Frame Number (PFN) for `/proc/<pid>/pagemap`.
///
/// Linux pagemap entries reserve bits 0..54 for the physical page frame number.
/// Because Zircon does not expose physical RAM addresses to userspace, Starnix
/// synthesizes pseudo-PFNs by hashing the backing Zircon VMO's KOID to generate a
/// 55-bit base address and adding the page offset within that VMO.
///
/// Tradeoffs:
/// - Mappings in different processes that share the same underlying VMO at the same offset
///   (such as shared libraries like `libc.so` or shared ashmem) produce identical pseudo-PFNs,
///   allowing memory tools (e.g. `procrank`, `librank`) to accurately compute proportional set
///   sizes (PSS).
/// - Unrelated processes with private memory at identical virtual addresses have distinct
///   private VMO KOIDs and will not falsely collide.
/// - Consecutive virtual pages within the same VMO mapping have strictly adjacent pseudo-PFNs
///   (`pfn + 1`).
/// - On `fork()`, Zircon creates a child snapshot VMO with a new KOID for private memory,
///   meaning parent and child private pages will have different pseudo-PFNs immediately
///   rather than waiting for Copy-on-Write modifications. As a result, memory might end up
///   significantly bigger in accounting tools (in particular, RELRO pages will be over-counted
///   for processes forked from the zygote).
fn compute_pseudo_pfn(koid: zx::Koid, vmo_page_idx: u64) -> u64 {
    let mut hasher = rustc_hash::FxHasher::default();
    hasher.write_u64(koid.raw_koid());
    hasher.finish().wrapping_add(vmo_page_idx) & PAGEMAP_PFN_MASK
}

/// Implements `/proc/<pid>/pagemap`.
#[derive(Clone)]
pub struct ProcPagemapFile {
    pid: Pid,
}

impl ProcPagemapFile {
    pub fn new(pid: Pid) -> Self {
        Self { pid }
    }
}

impl FileOps for ProcPagemapFile {
    fileops_impl_seekable!();
    fileops_impl_noop_sync!();

    fn read(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        offset: usize,
        dst: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        let task = self.pid.get_task()?;
        let Ok(mm) = task.mm() else {
            return Ok(0);
        };

        let to_read = std::cmp::min(dst.available(), 1024 * 1024);
        if to_read == 0 {
            return Ok(0);
        }

        let entry_size = std::mem::size_of::<u64>();
        let page_size = *PAGE_SIZE as usize;

        let unaligned_offset = offset % entry_size;
        let start_page_idx = offset / entry_size;
        let total_bytes_needed = unaligned_offset + to_read;
        let num_pages = (total_bytes_needed + entry_size - 1) / entry_size;

        let state = mm.state.read();
        let can_read_pfn =
            security::is_task_capable_noaudit(current_task, starnix_uapi::auth::CAP_SYS_ADMIN);

        let get_mapping_vmo_info = |mm_mapping: &Mapping, addr: UserAddress| -> (zx::Koid, u64) {
            match state.get_mapping_backing(mm_mapping) {
                MappingBacking::Memory(backing) => {
                    (backing.memory().get_koid(), backing.address_to_offset(addr))
                }
                MappingBacking::PrivateAnonymous => {
                    (mm.mapping_context.private_anonymous.backing.get_koid(), addr.ptr() as u64)
                }
            }
        };

        // Bit 63 indicates the page is present in RAM, bit 61 indicates file-page or
        // shared-anon, and bit 57 indicates an exclusively mapped page (anonymous private).
        //
        // On Linux, bit 63 indicates physical hardware residency in CPU page tables. Zircon
        // uses demand paging and does not expose per-page commit/PTE status to userspace
        // without faulting in the page. We treat all registered mappings as present so tools
        // (e.g. procrank, librank, showmap) can query page sharing, while aggregate resident
        // sizes (RSS) are obtained from /proc/<pid>/smaps.
        let compute_entry_flags = |mm_mapping: &Mapping| -> u64 {
            let is_shared = mm_mapping.flags().contains(MappingFlags::SHARED);
            let is_file = matches!(mm_mapping.name(), MappingNameRef::File(_));
            (1u64 << 63) | if is_shared || is_file { 1u64 << 61 } else { 1u64 << 57 }
        };

        // Ultra-fast path for single-page reads (the dominant case in procrank/smapinfo).
        if to_read == entry_size && unaligned_offset == 0 {
            let start_vaddr = match start_page_idx.checked_mul(page_size) {
                Some(addr) => UserAddress::from(addr as u64),
                None => return Ok(0),
            };
            let mut entry = 0u64;
            if let Some((_, mm_mapping)) = state.mappings.get(start_vaddr) {
                let flags = compute_entry_flags(mm_mapping);
                let pfn = if can_read_pfn {
                    let (koid, vmo_offset) = get_mapping_vmo_info(mm_mapping, start_vaddr);
                    compute_pseudo_pfn(koid, vmo_offset / page_size as u64)
                } else {
                    0
                };
                entry = flags | pfn;
            }
            dst.write_all(&entry.to_ne_bytes())?;
            return Ok(entry_size);
        }

        const CHUNK_PAGES: usize = 512;
        let mut chunk_buf = [0u64; CHUNK_PAGES];

        let mut pages_processed = 0;
        let mut bytes_written_total = 0;

        while pages_processed < num_pages && bytes_written_total < to_read {
            let chunk_start_page = start_page_idx + pages_processed;
            let current_chunk_pages = std::cmp::min(CHUNK_PAGES, num_pages - pages_processed);
            let chunk_slice = &mut chunk_buf[..current_chunk_pages];

            let chunk_start_vaddr = match chunk_start_page.checked_mul(page_size) {
                Some(addr) => UserAddress::from(addr as u64),
                None => break,
            };
            let chunk_end_vaddr =
                match (chunk_start_page + current_chunk_pages).checked_mul(page_size) {
                    Some(addr) => UserAddress::from(addr as u64),
                    None => UserAddress::from(u64::MAX),
                };

            // Fast path: entire chunk is contained within a single mapping.
            if let Some((mm_range, mm_mapping)) = state.mappings.get(chunk_start_vaddr) {
                if mm_range.end >= chunk_end_vaddr {
                    let flags = compute_entry_flags(mm_mapping);

                    let start_pfn = if can_read_pfn {
                        let (koid, vmo_offset) =
                            get_mapping_vmo_info(mm_mapping, chunk_start_vaddr);
                        compute_pseudo_pfn(koid, vmo_offset / page_size as u64)
                    } else {
                        0
                    };

                    for (i, entry) in chunk_slice.iter_mut().enumerate() {
                        let pfn = if can_read_pfn {
                            (start_pfn.wrapping_add(i as u64)) & PAGEMAP_PFN_MASK
                        } else {
                            0
                        };
                        *entry = flags | pfn;
                    }

                    let byte_slice = chunk_slice.as_bytes();
                    let chunk_byte_offset = if pages_processed == 0 { unaligned_offset } else { 0 };
                    let chunk_available_bytes = byte_slice.len().saturating_sub(chunk_byte_offset);
                    let chunk_write_len =
                        std::cmp::min(to_read - bytes_written_total, chunk_available_bytes);

                    dst.write_all(
                        &byte_slice[chunk_byte_offset..chunk_byte_offset + chunk_write_len],
                    )?;
                    bytes_written_total += chunk_write_len;
                    pages_processed += current_chunk_pages;
                    continue;
                }
            }

            // General path: spans multiple mappings or unmapped memory.
            chunk_slice.fill(0);

            for (mm_range, mm_mapping) in state.mappings.range(chunk_start_vaddr..chunk_end_vaddr) {
                let flags = compute_entry_flags(mm_mapping);

                let map_start_page = (mm_range.start.ptr() as usize) / page_size;
                let map_end_page = (mm_range.end.ptr() as usize) / page_size;

                let first_page = std::cmp::max(chunk_start_page, map_start_page);
                let last_page = std::cmp::min(chunk_start_page + current_chunk_pages, map_end_page);

                let first_page_vaddr = UserAddress::from((first_page * page_size) as u64);
                let start_pfn = if can_read_pfn {
                    let (koid, vmo_offset) = get_mapping_vmo_info(mm_mapping, first_page_vaddr);
                    compute_pseudo_pfn(koid, vmo_offset / page_size as u64)
                } else {
                    0
                };

                for page_idx in first_page..last_page {
                    let pfn = if can_read_pfn {
                        (start_pfn.wrapping_add((page_idx - first_page) as u64)) & PAGEMAP_PFN_MASK
                    } else {
                        0
                    };
                    chunk_slice[page_idx - chunk_start_page] = flags | pfn;
                }
            }

            let byte_slice = chunk_slice.as_bytes();
            let chunk_byte_offset = if pages_processed == 0 { unaligned_offset } else { 0 };
            let chunk_available_bytes = byte_slice.len().saturating_sub(chunk_byte_offset);
            let chunk_write_len =
                std::cmp::min(to_read - bytes_written_total, chunk_available_bytes);

            dst.write_all(&byte_slice[chunk_byte_offset..chunk_byte_offset + chunk_write_len])?;
            bytes_written_total += chunk_write_len;
            pages_processed += current_chunk_pages;
        }

        Ok(bytes_written_total)
    }

    fn write(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        _data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        starnix_uapi::error!(EPERM)
    }
}

/// Creates a memory object that can be used in an anonymous mapping for the `mmap` syscall.
pub fn create_anonymous_mapping_memory(size: u64) -> Result<Arc<MemoryObject>, Errno> {
    // mremap can grow memory regions, so make sure the memory object is resizable.
    let mut memory = MemoryObject::from(
        zx::Vmo::create_with_opts(zx::VmoOptions::RESIZABLE, size).map_err(|s| match s {
            zx::Status::NO_MEMORY => errno!(ENOMEM),
            zx::Status::OUT_OF_RANGE => errno!(ENOMEM),
            _ => impossible_error(s),
        })?,
    )
    .with_zx_name(b"starnix:memory_manager");

    memory.set_zx_name(b"starnix-anon");

    // TODO(https://fxbug.dev/42056890): Audit replace_as_executable usage
    memory = memory.replace_as_executable(&VMEX_RESOURCE).map_err(impossible_error)?;
    Ok(Arc::new(memory))
}

fn generate_random_offset_for_aslr(arch_width: ArchWidth) -> usize {
    // Generate a number with ASLR_RANDOM_BITS.
    let randomness = {
        let random_bits =
            if arch_width.is_arch32() { ASLR_32_RANDOM_BITS } else { ASLR_RANDOM_BITS };
        let mask = (1 << random_bits) - 1;
        let mut bytes = [0; std::mem::size_of::<usize>()];
        starnix_crypto::cprng_draw(&mut bytes);
        usize::from_le_bytes(bytes) & mask
    };

    // Transform it into a page-aligned offset.
    randomness * (*PAGE_SIZE as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mm::memory_accessor::{MemoryAccessor, MemoryAccessorExt};
    use crate::mm::syscalls::do_mmap;
    use crate::task::syscalls::sys_prctl;
    use crate::testing::*;
    use crate::vfs::FdNumber;
    use assert_matches::assert_matches;
    use itertools::assert_equal;
    use starnix_uapi::user_address::{UserCString, UserRef};
    use starnix_uapi::{
        MAP_ANONYMOUS, MAP_FIXED, MAP_GROWSDOWN, MAP_PRIVATE, MAP_SHARED, PR_SET_VMA,
        PR_SET_VMA_ANON_NAME, PROT_NONE, PROT_READ,
    };
    use std::ffi::CString;
    use zerocopy::{FromBytes, Immutable, KnownLayout};

    #[::fuchsia::test]
    fn test_mapping_flags() {
        let options = MappingOptions::ANONYMOUS;
        let access_flags = ProtectionFlags::READ | ProtectionFlags::WRITE;
        let mapping_flags = MappingFlags::from_access_flags_and_options(access_flags, options);
        assert_eq!(mapping_flags.access_flags(), access_flags);
        assert_eq!(mapping_flags.options(), options);

        let new_access_flags = ProtectionFlags::READ | ProtectionFlags::EXEC;
        let adusted_mapping_flags = mapping_flags.with_access_flags(new_access_flags);
        assert_eq!(adusted_mapping_flags.access_flags(), new_access_flags);
        assert_eq!(adusted_mapping_flags.options(), options);
    }

    #[::fuchsia::test]
    async fn test_any_ranges_lazy() {
        spawn_kernel_and_run(async |current_task| {
            let mm = current_task.mm().unwrap();
            let page_size = *PAGE_SIZE as usize;
            let addr = (mm.base_addr + 10 * page_size).unwrap();
            let length = page_size;

            let memory = create_anonymous_mapping_memory(length as u64).unwrap();
            let flags = MappingFlags::from_access_flags_and_options(
                ProtectionFlags::READ | ProtectionFlags::WRITE,
                MappingOptions::empty(),
            );

            let mapping = Mapping::with_name(
                MappingBacking::Memory(Box::new(MappingBackingMemory::new(addr, memory, 0))),
                flags,
                MappingName::None,
                MappingMode::Lazy,
            );

            {
                let mut state = mm.state.write();
                state.mappings.insert(addr..(addr + length).unwrap(), mapping);
            }

            {
                let state = mm.state.read();
                assert!(state.any_ranges_lazy(std::iter::once((addr, Some(length)))));
            }

            assert!(mm.ensure_range_mapped_in_user_vmar(addr, Some(length)).unwrap());

            {
                let state = mm.state.read();
                assert!(!state.any_ranges_lazy(std::iter::once((addr, Some(length)))));
            }
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_brk() {
        spawn_kernel_and_run(async |current_task| {
            let mm = current_task.mm().unwrap();

            // Look up the given addr in the mappings table.
            let get_range = |addr: UserAddress| {
                let state = mm.state.read();
                state
                    .mappings
                    .map
                    .get(addr)
                    .map(|(range, mapping)| (range.clone(), mapping.clone()))
            };

            // Initialize the program break.
            let base_addr = mm
                .set_brk(&current_task, UserAddress::default())
                .expect("failed to set initial program break");
            assert!(base_addr > UserAddress::default());

            // Page containing the program break address should not be mapped.
            assert_eq!(get_range(base_addr), None);

            // Growing it by a single byte results in that page becoming mapped.
            let addr0 =
                mm.set_brk(&current_task, (base_addr + 1u64).unwrap()).expect("failed to grow brk");
            assert!(addr0 > base_addr);
            let (range0, _) = get_range(base_addr).expect("base_addr should be mapped");
            assert_eq!(range0.start, base_addr);
            assert_eq!(range0.end, (base_addr + *PAGE_SIZE).unwrap());

            // Grow the program break by another byte, which won't be enough to cause additional pages to be mapped.
            let addr1 =
                mm.set_brk(&current_task, (base_addr + 2u64).unwrap()).expect("failed to grow brk");
            assert_eq!(addr1, (base_addr + 2u64).unwrap());
            let (range1, _) = get_range(base_addr).expect("base_addr should be mapped");
            assert_eq!(range1.start, range0.start);
            assert_eq!(range1.end, range0.end);

            // Grow the program break by a non-trival amount and observe the larger mapping.
            let addr2 = mm
                .set_brk(&current_task, (base_addr + 24893u64).unwrap())
                .expect("failed to grow brk");
            assert_eq!(addr2, (base_addr + 24893u64).unwrap());
            let (range2, _) = get_range(base_addr).expect("base_addr should be mapped");
            assert_eq!(range2.start, base_addr);
            assert_eq!(range2.end, addr2.round_up(*PAGE_SIZE).unwrap());

            // Shrink the program break and observe the smaller mapping.
            let addr3 = mm
                .set_brk(&current_task, (base_addr + 14832u64).unwrap())
                .expect("failed to shrink brk");
            assert_eq!(addr3, (base_addr + 14832u64).unwrap());
            let (range3, _) = get_range(base_addr).expect("base_addr should be mapped");
            assert_eq!(range3.start, base_addr);
            assert_eq!(range3.end, addr3.round_up(*PAGE_SIZE).unwrap());

            // Shrink the program break close to zero and observe the smaller mapping.
            let addr4 = mm
                .set_brk(&current_task, (base_addr + 3u64).unwrap())
                .expect("failed to drastically shrink brk");
            assert_eq!(addr4, (base_addr + 3u64).unwrap());
            let (range4, _) = get_range(base_addr).expect("base_addr should be mapped");
            assert_eq!(range4.start, base_addr);
            assert_eq!(range4.end, addr4.round_up(*PAGE_SIZE).unwrap());

            // Shrink the program break to zero and observe that the mapping is entirely gone.
            let addr5 = mm
                .set_brk(&current_task, base_addr)
                .expect("failed to drastically shrink brk to zero");
            assert_eq!(addr5, base_addr);
            assert_eq!(get_range(base_addr), None);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_mm_exec() {
        spawn_kernel_and_run(async |current_task| {
            let mm = current_task.mm().unwrap();

            let has = |addr: UserAddress| -> bool {
                let state = mm.state.read();
                state.mappings.get(addr).is_some()
            };

            let brk_addr = mm
                .set_brk(&current_task, UserAddress::default())
                .expect("failed to set initial program break");
            assert!(brk_addr > UserAddress::default());

            // Allocate a single page of BRK space, so that the break base address is mapped.
            let _ = mm
                .set_brk(&current_task, (brk_addr + 1u64).unwrap())
                .expect("failed to grow program break");
            assert!(has(brk_addr));

            let mapped_addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            assert!(mapped_addr > UserAddress::default());
            assert!(has(mapped_addr));

            let node = current_task.lookup_path_from_root("/".into()).unwrap();
            let new_mm = MemoryManager::exec(
                current_task.thread_group().root_vmar.unowned(),
                current_task.running_state().mm.upgrade(),
                node,
                ArchWidth::Arch64,
            )
            .expect("failed to exec memory manager");
            current_task.running_state().mm.update(Some(new_mm));

            assert!(!has(brk_addr));
            assert!(!has(mapped_addr));

            // Check that the old addresses are actually available for mapping.
            let brk_addr2 = map_memory(&current_task, brk_addr, *PAGE_SIZE);
            assert_eq!(brk_addr, brk_addr2);
            let mapped_addr2 = map_memory(&current_task, mapped_addr, *PAGE_SIZE);
            assert_eq!(mapped_addr, mapped_addr2);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_get_contiguous_mappings_at() {
        spawn_kernel_and_run(async |current_task| {
            let mm = current_task.mm().unwrap();
            let context = &mm.mapping_context;

            // Create four one-page mappings with a hole between the third one and the fourth one.
            let page_size = *PAGE_SIZE as usize;
            let addr_a = (mm.base_addr + 10 * page_size).unwrap();
            let addr_b = (mm.base_addr + 11 * page_size).unwrap();
            let addr_c = (mm.base_addr + 12 * page_size).unwrap();
            let addr_d = (mm.base_addr + 14 * page_size).unwrap();
            assert_eq!(map_memory(&current_task, addr_a, *PAGE_SIZE), addr_a);
            assert_eq!(map_memory(&current_task, addr_b, *PAGE_SIZE), addr_b);
            assert_eq!(map_memory(&current_task, addr_c, *PAGE_SIZE), addr_c);
            assert_eq!(map_memory(&current_task, addr_d, *PAGE_SIZE), addr_d);

            {
                let mm_state = mm.state.read();
                // Verify that requesting an unmapped address returns an empty iterator.
                assert_equal(
                    mm_state
                        .get_contiguous_mappings_at((addr_a - 100u64).unwrap(), 50, &context)
                        .unwrap(),
                    vec![],
                );
                assert_equal(
                    mm_state
                        .get_contiguous_mappings_at((addr_a - 100u64).unwrap(), 200, &context)
                        .unwrap(),
                    vec![],
                );

                // Verify that requesting zero bytes returns an empty iterator.
                assert_equal(
                    mm_state.get_contiguous_mappings_at(addr_a, 0, &context).unwrap(),
                    vec![],
                );

                // Verify errors.
                assert_eq!(
                    mm_state
                        .get_contiguous_mappings_at(UserAddress::from(100), usize::MAX, &context)
                        .err()
                        .unwrap(),
                    errno!(EFAULT)
                );
                assert_eq!(
                    mm_state
                        .get_contiguous_mappings_at(
                            (context.max_address() + 1u64).unwrap(),
                            0,
                            &context
                        )
                        .err()
                        .unwrap(),
                    errno!(EFAULT)
                );
            }

            assert_eq!(mm.get_mapping_count(), 2);
            let mm_state = mm.state.read();
            let (map_a, map_b) = {
                let mut it = mm_state.mappings.iter();
                (it.next().unwrap().1, it.next().unwrap().1)
            };

            assert_equal(
                mm_state.get_contiguous_mappings_at(addr_a, page_size, &context).unwrap(),
                vec![(map_a, page_size)],
            );

            assert_equal(
                mm_state.get_contiguous_mappings_at(addr_a, page_size / 2, &context).unwrap(),
                vec![(map_a, page_size / 2)],
            );

            assert_equal(
                mm_state.get_contiguous_mappings_at(addr_a, page_size * 3, &context).unwrap(),
                vec![(map_a, page_size * 3)],
            );

            assert_equal(
                mm_state.get_contiguous_mappings_at(addr_b, page_size, &context).unwrap(),
                vec![(map_a, page_size)],
            );

            assert_equal(
                mm_state.get_contiguous_mappings_at(addr_d, page_size, &context).unwrap(),
                vec![(map_b, page_size)],
            );

            // Verify that results stop if there is a hole.
            assert_equal(
                mm_state
                    .get_contiguous_mappings_at(
                        (addr_a + page_size / 2).unwrap(),
                        page_size * 10,
                        &context,
                    )
                    .unwrap(),
                vec![(map_a, page_size * 2 + page_size / 2)],
            );

            // Verify that results stop at the last mapped page.
            assert_equal(
                mm_state.get_contiguous_mappings_at(addr_d, page_size * 10, &context).unwrap(),
                vec![(map_b, page_size)],
            );
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_read_write_crossing_mappings() {
        spawn_kernel_and_run(async |current_task| {
            let mm = current_task.mm().unwrap();
            let ma = current_task.deref();

            // Map two contiguous pages at fixed addresses, but backed by distinct mappings.
            let page_size = *PAGE_SIZE;
            let addr = (mm.base_addr + 10 * page_size).unwrap();
            assert_eq!(map_memory(&current_task, addr, page_size), addr);
            assert_eq!(
                map_memory(&current_task, (addr + page_size).unwrap(), page_size),
                (addr + page_size).unwrap()
            );
            // Mappings get merged since they are baked by the same memory object
            assert_eq!(mm.get_mapping_count(), 1);

            // Write a pattern crossing our two mappings.
            let test_addr = (addr + page_size / 2).unwrap();
            let data: Vec<u8> = (0..page_size).map(|i| (i % 256) as u8).collect();
            ma.write_memory(test_addr, &data).expect("failed to write test data");

            // Read it back.
            let data_readback =
                ma.read_memory_to_vec(test_addr, data.len()).expect("failed to read test data");
            assert_eq!(&data, &data_readback);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_read_write_errors() {
        spawn_kernel_and_run(async |current_task| {
            let ma = current_task.deref();

            let page_size = *PAGE_SIZE;
            let addr = map_memory(&current_task, UserAddress::default(), page_size);
            let buf = vec![0u8; page_size as usize];

            // Verify that accessing data that is only partially mapped is an error.
            let partial_addr_before = (addr - page_size / 2).unwrap();
            assert_eq!(ma.write_memory(partial_addr_before, &buf), error!(EFAULT));
            assert_eq!(ma.read_memory_to_vec(partial_addr_before, buf.len()), error!(EFAULT));
            let partial_addr_after = (addr + page_size / 2).unwrap();
            assert_eq!(ma.write_memory(partial_addr_after, &buf), error!(EFAULT));
            assert_eq!(ma.read_memory_to_vec(partial_addr_after, buf.len()), error!(EFAULT));

            // Verify that accessing unmapped memory is an error.
            let unmapped_addr = (addr - 10 * page_size).unwrap();
            assert_eq!(ma.write_memory(unmapped_addr, &buf), error!(EFAULT));
            assert_eq!(ma.read_memory_to_vec(unmapped_addr, buf.len()), error!(EFAULT));

            // However, accessing zero bytes in unmapped memory is not an error.
            ma.write_memory(unmapped_addr, &[]).expect("failed to write no data");
            ma.read_memory_to_vec(unmapped_addr, 0).expect("failed to read no data");
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_read_c_string_to_vec_large() {
        spawn_kernel_and_run(async |current_task| {
            let mm = current_task.mm().unwrap();
            let ma = current_task.deref();

            let page_size = *PAGE_SIZE;
            let max_size = 4 * page_size as usize;
            let addr = (mm.base_addr + 10 * page_size).unwrap();

            assert_eq!(map_memory(&current_task, addr, max_size as u64), addr);

            let mut random_data = vec![0; max_size];
            starnix_crypto::cprng_draw(&mut random_data);
            // Remove all NUL bytes.
            for i in 0..random_data.len() {
                if random_data[i] == 0 {
                    random_data[i] = 1;
                }
            }
            random_data[max_size - 1] = 0;

            ma.write_memory(addr, &random_data).expect("failed to write test string");
            // We should read the same value minus the last byte (NUL char).
            assert_eq!(
                ma.read_c_string_to_vec(UserCString::new(current_task, addr), max_size).unwrap(),
                random_data[..max_size - 1]
            );
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_read_c_string_to_vec() {
        spawn_kernel_and_run(async |current_task| {
            let mm = current_task.mm().unwrap();
            let ma = current_task.deref();

            let page_size = *PAGE_SIZE;
            let max_size = 2 * page_size as usize;
            let addr = (mm.base_addr + 10 * page_size).unwrap();

            // Map a page at a fixed address and write an unterminated string at the end of it.
            assert_eq!(map_memory(&current_task, addr, page_size), addr);
            let test_str = b"foo!";
            let test_addr =
                addr.checked_add(page_size as usize).unwrap().checked_sub(test_str.len()).unwrap();
            ma.write_memory(test_addr, test_str).expect("failed to write test string");

            // Expect error if the string is not terminated.
            assert_eq!(
                ma.read_c_string_to_vec(UserCString::new(current_task, test_addr), max_size),
                error!(ENAMETOOLONG)
            );

            // Expect success if the string is terminated.
            ma.write_memory((addr + (page_size - 1)).unwrap(), b"\0").expect("failed to write nul");
            assert_eq!(
                ma.read_c_string_to_vec(UserCString::new(current_task, test_addr), max_size)
                    .unwrap(),
                "foo"
            );

            // Expect success if the string spans over two mappings.
            assert_eq!(
                map_memory(&current_task, (addr + page_size).unwrap(), page_size),
                (addr + page_size).unwrap()
            );
            // TODO: Adjacent private anonymous mappings are collapsed. To test this case this test needs to
            // provide a backing for the second mapping.
            // assert_eq!(mm.get_mapping_count(), 2);
            ma.write_memory((addr + (page_size - 1)).unwrap(), b"bar\0")
                .expect("failed to write extra chars");
            assert_eq!(
                ma.read_c_string_to_vec(UserCString::new(current_task, test_addr), max_size)
                    .unwrap(),
                "foobar",
            );

            // Expect error if the string exceeds max limit
            assert_eq!(
                ma.read_c_string_to_vec(UserCString::new(current_task, test_addr), 2),
                error!(ENAMETOOLONG)
            );

            // Expect error if the address is invalid.
            assert_eq!(
                ma.read_c_string_to_vec(UserCString::null(current_task), max_size),
                error!(EFAULT)
            );
        })
        .await;
    }

    #[::fuchsia::test]
    async fn can_read_argv_like_regions() {
        spawn_kernel_and_run(async |current_task| {
            let ma = current_task.deref();

            // Map a page.
            let page_size = *PAGE_SIZE;
            let addr = map_memory_anywhere(&current_task, page_size);
            assert!(!addr.is_null());

            // Write an unterminated string.
            let mut payload = "first".as_bytes().to_vec();
            let mut expected_parses = vec![];
            ma.write_memory(addr, &payload).unwrap();

            // Expect success if the string is terminated.
            expected_parses.push(payload.clone());
            payload.push(0);
            ma.write_memory(addr, &payload).unwrap();
            assert_eq!(
                ma.read_nul_delimited_c_string_list(addr, payload.len()).unwrap(),
                expected_parses,
            );

            // Make sure we can parse multiple strings from the same region.
            let second = b"second";
            payload.extend(second);
            payload.push(0);
            expected_parses.push(second.to_vec());

            let third = b"third";
            payload.extend(third);
            payload.push(0);
            expected_parses.push(third.to_vec());

            ma.write_memory(addr, &payload).unwrap();
            assert_eq!(
                ma.read_nul_delimited_c_string_list(addr, payload.len()).unwrap(),
                expected_parses,
            );
        })
        .await;
    }

    #[::fuchsia::test]
    async fn truncate_argv_like_regions() {
        spawn_kernel_and_run(async |current_task| {
            let ma = current_task.deref();

            // Map a page.
            let page_size = *PAGE_SIZE;
            let addr = map_memory_anywhere(&current_task, page_size);
            assert!(!addr.is_null());

            let payload = b"first\0second\0third\0";
            ma.write_memory(addr, payload).unwrap();
            assert_eq!(
                ma.read_nul_delimited_c_string_list(addr, payload.len() - 3).unwrap(),
                vec![b"first".to_vec(), b"second".to_vec(), b"thi".to_vec()],
                "Skipping last three bytes of payload should skip last two bytes of 3rd string"
            );
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_read_c_string() {
        spawn_kernel_and_run(async |current_task| {
            let mm = current_task.mm().unwrap();
            let ma = current_task.deref();

            let page_size = *PAGE_SIZE;
            let buf_cap = 2 * page_size as usize;
            let mut buf = Vec::with_capacity(buf_cap);
            // We can't just use `spare_capacity_mut` because `Vec::with_capacity`
            // returns a `Vec` with _at least_ the requested capacity.
            let buf = &mut buf.spare_capacity_mut()[..buf_cap];
            let addr = (mm.base_addr + 10 * page_size).unwrap();

            // Map a page at a fixed address and write an unterminated string at the end of it..
            assert_eq!(map_memory(&current_task, addr, page_size), addr);
            let test_str = b"foo!";
            let test_addr = (addr + (page_size - test_str.len() as u64)).unwrap();
            ma.write_memory(test_addr, test_str).expect("failed to write test string");

            // Expect error if the string is not terminated.
            assert_eq!(
                ma.read_c_string(UserCString::new(current_task, test_addr), buf),
                error!(ENAMETOOLONG)
            );

            // Expect success if the string is terminated.
            ma.write_memory((addr + (page_size - 1)).unwrap(), b"\0").expect("failed to write nul");
            assert_eq!(
                ma.read_c_string(UserCString::new(current_task, test_addr), buf).unwrap(),
                "foo"
            );

            // Expect success if the string spans over two mappings.
            assert_eq!(
                map_memory(&current_task, (addr + page_size).unwrap(), page_size),
                (addr + page_size).unwrap()
            );
            // TODO: To be multiple mappings we need to provide a file backing for the next page or the
            // mappings will be collapsed.
            //assert_eq!(mm.get_mapping_count(), 2);
            ma.write_memory((addr + (page_size - 1)).unwrap(), b"bar\0")
                .expect("failed to write extra chars");
            assert_eq!(
                ma.read_c_string(UserCString::new(current_task, test_addr), buf).unwrap(),
                "foobar"
            );

            // Expect error if the string does not fit in the provided buffer.
            assert_eq!(
                ma.read_c_string(
                    UserCString::new(current_task, test_addr),
                    &mut [MaybeUninit::uninit(); 2]
                ),
                error!(ENAMETOOLONG)
            );

            // Expect error if the address is invalid.
            assert_eq!(ma.read_c_string(UserCString::null(current_task), buf), error!(EFAULT));
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_find_next_unused_range() {
        spawn_kernel_and_run(async |current_task| {
            let mm = current_task.mm().unwrap();

            let mmap_top = mm.state.read().find_next_unused_range(0).unwrap().ptr();
            let page_size = *PAGE_SIZE as usize;
            assert!(mmap_top <= RESTRICTED_ASPACE_HIGHEST_ADDRESS);

            // No mappings - top address minus requested size is available
            assert_eq!(
                mm.state.read().find_next_unused_range(page_size).unwrap(),
                UserAddress::from_ptr(mmap_top - page_size)
            );

            // Fill it.
            let addr = UserAddress::from_ptr(mmap_top - page_size);
            assert_eq!(map_memory(&current_task, addr, *PAGE_SIZE), addr);

            // The next available range is right before the new mapping.
            assert_eq!(
                mm.state.read().find_next_unused_range(page_size).unwrap(),
                UserAddress::from_ptr(addr.ptr() - page_size)
            );

            // Allocate an extra page before a one-page gap.
            let addr2 = UserAddress::from_ptr(addr.ptr() - 2 * page_size);
            assert_eq!(map_memory(&current_task, addr2, *PAGE_SIZE), addr2);

            // Searching for one-page range still gives the same result
            assert_eq!(
                mm.state.read().find_next_unused_range(page_size).unwrap(),
                UserAddress::from_ptr(addr.ptr() - page_size)
            );

            // Searching for a bigger range results in the area before the second mapping
            assert_eq!(
                mm.state.read().find_next_unused_range(2 * page_size).unwrap(),
                UserAddress::from_ptr(addr2.ptr() - 2 * page_size)
            );

            // Searching for more memory than available should fail.
            assert_eq!(mm.state.read().find_next_unused_range(mmap_top), None);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_count_placements() {
        spawn_kernel_and_run(async |current_task| {
            let mm = current_task.mm().unwrap();

            // ten-page range
            let page_size = *PAGE_SIZE as usize;
            let subrange_ten = UserAddress::from_ptr(RESTRICTED_ASPACE_BASE)
                ..UserAddress::from_ptr(RESTRICTED_ASPACE_BASE + 10 * page_size);

            assert_eq!(
                mm.state.read().count_possible_placements(11 * page_size, &subrange_ten),
                Some(0)
            );
            assert_eq!(
                mm.state.read().count_possible_placements(10 * page_size, &subrange_ten),
                Some(1)
            );
            assert_eq!(
                mm.state.read().count_possible_placements(9 * page_size, &subrange_ten),
                Some(2)
            );
            assert_eq!(
                mm.state.read().count_possible_placements(page_size, &subrange_ten),
                Some(10)
            );

            // map 6th page
            let addr = UserAddress::from_ptr(RESTRICTED_ASPACE_BASE + 5 * page_size);
            assert_eq!(map_memory(&current_task, addr, *PAGE_SIZE), addr);

            assert_eq!(
                mm.state.read().count_possible_placements(10 * page_size, &subrange_ten),
                Some(0)
            );
            assert_eq!(
                mm.state.read().count_possible_placements(5 * page_size, &subrange_ten),
                Some(1)
            );
            assert_eq!(
                mm.state.read().count_possible_placements(4 * page_size, &subrange_ten),
                Some(3)
            );
            assert_eq!(
                mm.state.read().count_possible_placements(page_size, &subrange_ten),
                Some(9)
            );
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_pick_placement() {
        spawn_kernel_and_run(async |current_task| {
            let mm = current_task.mm().unwrap();

            let page_size = *PAGE_SIZE as usize;
            let subrange_ten = UserAddress::from_ptr(RESTRICTED_ASPACE_BASE)
                ..UserAddress::from_ptr(RESTRICTED_ASPACE_BASE + 10 * page_size);

            let addr = UserAddress::from_ptr(RESTRICTED_ASPACE_BASE + 5 * page_size);
            assert_eq!(map_memory(&current_task, addr, *PAGE_SIZE), addr);
            assert_eq!(
                mm.state.read().count_possible_placements(4 * page_size, &subrange_ten),
                Some(3)
            );

            assert_eq!(
                mm.state.read().pick_placement(4 * page_size, 0, &subrange_ten),
                Some(UserAddress::from_ptr(RESTRICTED_ASPACE_BASE))
            );
            assert_eq!(
                mm.state.read().pick_placement(4 * page_size, 1, &subrange_ten),
                Some(UserAddress::from_ptr(RESTRICTED_ASPACE_BASE + page_size))
            );
            assert_eq!(
                mm.state.read().pick_placement(4 * page_size, 2, &subrange_ten),
                Some(UserAddress::from_ptr(RESTRICTED_ASPACE_BASE + 6 * page_size))
            );
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_find_random_unused_range() {
        spawn_kernel_and_run(async |current_task| {
            let mm = current_task.mm().unwrap();

            // ten-page range
            let page_size = *PAGE_SIZE as usize;
            let subrange_ten = UserAddress::from_ptr(RESTRICTED_ASPACE_BASE)
                ..UserAddress::from_ptr(RESTRICTED_ASPACE_BASE + 10 * page_size);

            for _ in 0..10 {
                let addr = mm.state.read().find_random_unused_range(page_size, &subrange_ten);
                assert!(addr.is_some());
                assert_eq!(map_memory(&current_task, addr.unwrap(), *PAGE_SIZE), addr.unwrap());
            }
            assert_eq!(mm.state.read().find_random_unused_range(page_size, &subrange_ten), None);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_grows_down_near_aspace_base() {
        spawn_kernel_and_run(async |current_task| {
            let mm = current_task.mm().unwrap();

            let page_count = 10;

            let page_size = *PAGE_SIZE as usize;
            let addr =
                (UserAddress::from_ptr(RESTRICTED_ASPACE_BASE) + page_count * page_size).unwrap();
            assert_eq!(
                map_memory_with_flags(
                    &current_task,
                    addr,
                    page_size as u64,
                    MAP_ANONYMOUS | MAP_PRIVATE | MAP_GROWSDOWN
                ),
                addr
            );

            let subrange_ten = UserAddress::from_ptr(RESTRICTED_ASPACE_BASE)..addr;
            assert_eq!(mm.state.read().find_random_unused_range(page_size, &subrange_ten), None);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_unmap_returned_mappings() {
        spawn_kernel_and_run(async |current_task| {
            let mm = current_task.mm().unwrap();

            let addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE * 2);

            let mut released_mappings = ReleasedMappings::default();
            let mut mm_state = mm.state.write();
            let unmap_result =
                mm_state.unmap(&mm, addr, *PAGE_SIZE as usize, &mut released_mappings);
            assert!(unmap_result.is_ok());
            assert_eq!(released_mappings.len(), 1);
            released_mappings.finalize(mm_state);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_unmap_returns_multiple_mappings() {
        spawn_kernel_and_run(async |current_task| {
            let mm = current_task.mm().unwrap();

            let addr = mm.state.read().find_next_unused_range(3 * *PAGE_SIZE as usize).unwrap();
            let addr = map_memory(&current_task, addr, *PAGE_SIZE);
            let _ = map_memory(&current_task, (addr + 2 * *PAGE_SIZE).unwrap(), *PAGE_SIZE);

            let mut released_mappings = ReleasedMappings::default();
            let mut mm_state = mm.state.write();
            let unmap_result =
                mm_state.unmap(&mm, addr, (*PAGE_SIZE * 3) as usize, &mut released_mappings);
            assert!(unmap_result.is_ok());
            assert_eq!(released_mappings.len(), 2);
            released_mappings.finalize(mm_state);
        })
        .await;
    }

    /// Maps two pages in separate mappings next to each other, then unmaps the first page.
    /// The second page should not be modified.
    #[::fuchsia::test]
    async fn test_map_two_unmap_one() {
        spawn_kernel_and_run(async |current_task| {
            let mm = current_task.mm().unwrap();

            // reserve memory for both pages
            let addr_reserve = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE * 2);
            let addr1 = do_mmap(
                &current_task,
                addr_reserve,
                *PAGE_SIZE as usize,
                PROT_READ, // Map read-only to avoid merging of the two mappings
                MAP_ANONYMOUS | MAP_SHARED | MAP_FIXED,
                FdNumber::from_raw(-1),
                0,
            )
            .expect("failed to mmap");
            let addr2 = map_memory_with_flags(
                &current_task,
                (addr_reserve + *PAGE_SIZE).unwrap(),
                *PAGE_SIZE,
                MAP_ANONYMOUS | MAP_SHARED | MAP_FIXED,
            );
            let state = mm.state.read();
            let (range1, _) = state.mappings.get(addr1).expect("mapping");
            assert_eq!(range1.start, addr1);
            assert_eq!(range1.end, (addr1 + *PAGE_SIZE).unwrap());
            let (range2, mapping2) = state.mappings.get(addr2).expect("mapping");
            assert_eq!(range2.start, addr2);
            assert_eq!(range2.end, (addr2 + *PAGE_SIZE).unwrap());
            let original_memory2 = {
                match state.get_mapping_backing(mapping2) {
                    MappingBacking::Memory(backing) => {
                        assert_eq!(backing.memory().get_size(), *PAGE_SIZE);
                        backing.memory().clone()
                    }
                    MappingBacking::PrivateAnonymous => {
                        panic!("Unexpected private anonymous mapping")
                    }
                }
            };
            std::mem::drop(state);

            assert_eq!(mm.unmap(addr1, *PAGE_SIZE as usize), Ok(()));

            let state = mm.state.read();

            // The first page should be unmapped.
            assert!(state.mappings.get(addr1).is_none());

            // The second page should remain unchanged.
            let (range2, mapping2) = state.mappings.get(addr2).expect("second page");
            assert_eq!(range2.start, addr2);
            assert_eq!(range2.end, (addr2 + *PAGE_SIZE).unwrap());
            match state.get_mapping_backing(mapping2) {
                MappingBacking::Memory(backing) => {
                    assert_eq!(backing.memory().get_size(), *PAGE_SIZE);
                    assert_eq!(original_memory2.get_koid(), backing.memory().get_koid());
                }
                MappingBacking::PrivateAnonymous => panic!("Unexpected private anonymous mapping"),
            }
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_read_write_objects() {
        spawn_kernel_and_run(async |current_task| {
            let ma = current_task.deref();
            let addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            let items_ref = UserRef::<i32>::new(addr);

            let items_written = vec![0, 2, 3, 7, 1];
            ma.write_objects(items_ref, &items_written).expect("Failed to write object array.");

            let items_read = ma
                .read_objects_to_vec(items_ref, items_written.len())
                .expect("Failed to read object array.");

            assert_eq!(items_written, items_read);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_read_write_objects_null() {
        spawn_kernel_and_run(async |current_task| {
            let ma = current_task.deref();
            let items_ref = UserRef::<i32>::new(UserAddress::default());

            let items_written = vec![];
            ma.write_objects(items_ref, &items_written)
                .expect("Failed to write empty object array.");

            let items_read = ma
                .read_objects_to_vec(items_ref, items_written.len())
                .expect("Failed to read empty object array.");

            assert_eq!(items_written, items_read);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_read_object_partial() {
        #[derive(Debug, Default, Copy, Clone, KnownLayout, FromBytes, Immutable, PartialEq)]
        struct Items {
            val: [i32; 4],
        }

        spawn_kernel_and_run(async |current_task| {
            let ma = current_task.deref();
            let addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            let items_array_ref = UserRef::<i32>::new(addr);

            // Populate some values.
            let items_written = vec![75, 23, 51, 98];
            ma.write_objects(items_array_ref, &items_written)
                .expect("Failed to write object array.");

            // Full read of all 4 values.
            let items_ref = UserRef::<Items>::new(addr);
            let items_read = ma
                .read_object_partial(items_ref, std::mem::size_of::<Items>())
                .expect("Failed to read object");
            assert_eq!(items_written, items_read.val);

            // Partial read of the first two.
            let items_read = ma.read_object_partial(items_ref, 8).expect("Failed to read object");
            assert_eq!(vec![75, 23, 0, 0], items_read.val);

            // The API currently allows reading 0 bytes (this could be re-evaluated) so test that does
            // the right thing.
            let items_read = ma.read_object_partial(items_ref, 0).expect("Failed to read object");
            assert_eq!(vec![0, 0, 0, 0], items_read.val);

            // Size bigger than the object.
            assert_eq!(
                ma.read_object_partial(items_ref, std::mem::size_of::<Items>() + 8),
                error!(EINVAL)
            );

            // Bad pointer.
            assert_eq!(
                ma.read_object_partial(UserRef::<Items>::new(UserAddress::from(1)), 16),
                error!(EFAULT)
            );
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_partial_read() {
        spawn_kernel_and_run(async |current_task| {
            let mm = current_task.mm().unwrap();
            let ma = current_task.deref();

            let addr = mm.state.read().find_next_unused_range(2 * *PAGE_SIZE as usize).unwrap();
            let addr = map_memory(&current_task, addr, *PAGE_SIZE);
            let second_map = map_memory(&current_task, (addr + *PAGE_SIZE).unwrap(), *PAGE_SIZE);

            let bytes = vec![0xf; (*PAGE_SIZE * 2) as usize];
            assert!(ma.write_memory(addr, &bytes).is_ok());
            let mut state = mm.state.write();
            let mut released_mappings = ReleasedMappings::default();
            state
                .protect(
                    ma,
                    second_map,
                    *PAGE_SIZE as usize,
                    ProtectionFlags::empty(),
                    &mut released_mappings,
                )
                .unwrap();
            released_mappings.finalize(state);
            assert_eq!(
                ma.read_memory_partial_to_vec(addr, bytes.len()).unwrap().len(),
                *PAGE_SIZE as usize,
            );
        })
        .await;
    }

    fn map_memory_growsdown(current_task: &CurrentTask, length: u64) -> UserAddress {
        map_memory_with_flags(
            current_task,
            UserAddress::default(),
            length,
            MAP_ANONYMOUS | MAP_PRIVATE | MAP_GROWSDOWN,
        )
    }

    #[::fuchsia::test]
    async fn test_grow_mapping_empty_mm() {
        spawn_kernel_and_run(async |current_task| {
            let mm = current_task.mm().unwrap();

            let addr = UserAddress::from(0x100000);

            assert_matches!(mm.extend_growsdown_mapping_to_address(addr, false), Ok(false));
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_grow_inside_mapping() {
        spawn_kernel_and_run(async |current_task| {
            let mm = current_task.mm().unwrap();

            let addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);

            assert_matches!(mm.extend_growsdown_mapping_to_address(addr, false), Ok(false));
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_grow_write_fault_inside_read_only_mapping() {
        spawn_kernel_and_run(async |current_task| {
            let mm = current_task.mm().unwrap();

            let addr = do_mmap(
                &current_task,
                UserAddress::default(),
                *PAGE_SIZE as usize,
                PROT_READ,
                MAP_ANONYMOUS | MAP_PRIVATE,
                FdNumber::from_raw(-1),
                0,
            )
            .expect("Could not map memory");

            assert_matches!(mm.extend_growsdown_mapping_to_address(addr, false), Ok(false));
            assert_matches!(mm.extend_growsdown_mapping_to_address(addr, true), Ok(false));
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_grow_fault_inside_prot_none_mapping() {
        spawn_kernel_and_run(async |current_task| {
            let mm = current_task.mm().unwrap();

            let addr = do_mmap(
                &current_task,
                UserAddress::default(),
                *PAGE_SIZE as usize,
                PROT_NONE,
                MAP_ANONYMOUS | MAP_PRIVATE,
                FdNumber::from_raw(-1),
                0,
            )
            .expect("Could not map memory");

            assert_matches!(mm.extend_growsdown_mapping_to_address(addr, false), Ok(false));
            assert_matches!(mm.extend_growsdown_mapping_to_address(addr, true), Ok(false));
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_grow_below_mapping() {
        spawn_kernel_and_run(async |current_task| {
            let mm = current_task.mm().unwrap();

            let addr = map_memory_growsdown(&current_task, *PAGE_SIZE) - *PAGE_SIZE;

            assert_matches!(mm.extend_growsdown_mapping_to_address(addr.unwrap(), false), Ok(true));
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_grow_above_mapping() {
        spawn_kernel_and_run(async |current_task| {
            let mm = current_task.mm().unwrap();

            let addr = map_memory_growsdown(&current_task, *PAGE_SIZE) + *PAGE_SIZE;

            assert_matches!(
                mm.extend_growsdown_mapping_to_address(addr.unwrap(), false),
                Ok(false)
            );
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_grow_write_fault_below_read_only_mapping() {
        spawn_kernel_and_run(async |current_task| {
            let mm = current_task.mm().unwrap();

            let mapped_addr = map_memory_growsdown(&current_task, *PAGE_SIZE);

            mm.protect(&current_task, mapped_addr, *PAGE_SIZE as usize, ProtectionFlags::READ)
                .unwrap();

            assert_matches!(
                mm.extend_growsdown_mapping_to_address((mapped_addr - *PAGE_SIZE).unwrap(), true),
                Ok(false)
            );

            assert_eq!(mm.get_mapping_count(), 1);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_snapshot_paged_memory() {
        use zx::sys::zx_page_request_command_t::ZX_PAGER_VMO_READ;

        spawn_kernel_and_run(async |current_task| {
            let mm = current_task.mm().unwrap();

            let port = Arc::new(zx::Port::create());
            let port_clone = port.clone();
            let pager =
                Arc::new(zx::Pager::create(zx::PagerOptions::empty()).expect("create failed"));
            let pager_clone = pager.clone();

            const VMO_SIZE: u64 = 128 * 1024;
            let vmo = Arc::new(
                pager
                    .create_vmo(zx::VmoOptions::RESIZABLE, &port, 1, VMO_SIZE)
                    .expect("create_vmo failed"),
            );
            let vmo_clone = vmo.clone();

            // Create a thread to service the port where we will receive pager requests.
            let thread = std::thread::spawn(move || {
                loop {
                    let packet =
                        port_clone.wait(zx::MonotonicInstant::INFINITE).expect("wait failed");
                    match packet.contents() {
                        zx::PacketContents::Pager(contents) => {
                            if contents.command() == ZX_PAGER_VMO_READ {
                                let range = contents.range();
                                let source_vmo = zx::Vmo::create(range.end - range.start)
                                    .expect("create failed");
                                pager_clone
                                    .supply_pages(&vmo_clone, range, &source_vmo, 0)
                                    .expect("supply_pages failed");
                            }
                        }
                        zx::PacketContents::User(_) => break,
                        _ => {}
                    }
                }
            });

            let child_vmo = vmo
                .create_child(zx::VmoChildOptions::SNAPSHOT_AT_LEAST_ON_WRITE, 0, VMO_SIZE)
                .unwrap();

            // Write something to the source VMO.
            vmo.write(b"foo", 0).expect("write failed");

            let prot_flags = ProtectionFlags::READ | ProtectionFlags::WRITE;
            let addr = mm
                .map_memory(
                    DesiredAddress::Any,
                    Arc::new(MemoryObject::from(child_vmo)),
                    0,
                    VMO_SIZE as usize,
                    prot_flags,
                    MappingOptions::empty(),
                    MappingName::None,
                )
                .expect("map failed");

            let target = current_task.clone_task_for_test(0, None);

            // Make sure target has what was in the source VMO.
            let buf = target.read_memory_to_vec(addr, 3).expect("read_memory failed");
            assert_eq!(buf, b"foo");

            let buf = current_task.deref().read_memory_to_vec(addr, 3).expect("read_memory failed");
            assert_eq!(buf, b"foo");

            port.queue(&zx::Packet::from_user_packet(0, 0, zx::UserPacket::from_u8_array([0; 32])))
                .unwrap();
            thread.join().unwrap();
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_set_vma_name() {
        spawn_kernel_and_run(async |mut current_task| {
            let name_addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);

            let vma_name = "vma name";
            current_task.write_memory(name_addr, vma_name.as_bytes()).unwrap();

            let mapping_addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);

            sys_prctl(
                &mut current_task,
                PR_SET_VMA,
                PR_SET_VMA_ANON_NAME as u64,
                mapping_addr.ptr() as u64,
                *PAGE_SIZE,
                name_addr.ptr() as u64,
            )
            .unwrap();

            assert_eq!(
                *current_task.mm().unwrap().get_mapping_name(mapping_addr).unwrap().unwrap(),
                vma_name
            );
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_set_vma_name_adjacent_mappings() {
        spawn_kernel_and_run(async |mut current_task| {
            let name_addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            current_task
                .write_memory(name_addr, CString::new("foo").unwrap().as_bytes_with_nul())
                .unwrap();

            let first_mapping_addr =
                map_memory(&current_task, UserAddress::default(), 2 * *PAGE_SIZE);
            let second_mapping_addr = map_memory_with_flags(
                &current_task,
                (first_mapping_addr + *PAGE_SIZE).unwrap(),
                *PAGE_SIZE,
                MAP_FIXED | MAP_PRIVATE | MAP_ANONYMOUS,
            );

            assert_eq!((first_mapping_addr + *PAGE_SIZE).unwrap(), second_mapping_addr);

            sys_prctl(
                &mut current_task,
                PR_SET_VMA,
                PR_SET_VMA_ANON_NAME as u64,
                first_mapping_addr.ptr() as u64,
                2 * *PAGE_SIZE,
                name_addr.ptr() as u64,
            )
            .unwrap();

            {
                let mm = current_task.mm().unwrap();
                let state = mm.state.read();

                // The name should apply to both mappings.
                let (_, mapping) = state.mappings.get(first_mapping_addr).unwrap();
                assert_eq!(mapping.name(), MappingName::Vma("foo".into()));

                let (_, mapping) = state.mappings.get(second_mapping_addr).unwrap();
                assert_eq!(mapping.name(), MappingName::Vma("foo".into()));
            }
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_set_vma_name_beyond_end() {
        spawn_kernel_and_run(async |mut current_task| {
            let name_addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            current_task
                .write_memory(name_addr, CString::new("foo").unwrap().as_bytes_with_nul())
                .unwrap();

            let mapping_addr = map_memory(&current_task, UserAddress::default(), 2 * *PAGE_SIZE);

            let second_page = (mapping_addr + *PAGE_SIZE).unwrap();
            current_task.mm().unwrap().unmap(second_page, *PAGE_SIZE as usize).unwrap();

            // This should fail with ENOMEM since it extends past the end of the mapping into unmapped memory.
            assert_eq!(
                sys_prctl(
                    &mut current_task,
                    PR_SET_VMA,
                    PR_SET_VMA_ANON_NAME as u64,
                    mapping_addr.ptr() as u64,
                    2 * *PAGE_SIZE,
                    name_addr.ptr() as u64,
                ),
                error!(ENOMEM)
            );

            // Despite returning an error, the prctl should still assign a name to the region at the start of the region.
            {
                let mm = current_task.mm().unwrap();
                let state = mm.state.read();

                let (_, mapping) = state.mappings.get(mapping_addr).unwrap();
                assert_eq!(mapping.name(), MappingName::Vma("foo".into()));
            }
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_set_vma_name_before_start() {
        spawn_kernel_and_run(async |mut current_task| {
            let name_addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            current_task
                .write_memory(name_addr, CString::new("foo").unwrap().as_bytes_with_nul())
                .unwrap();

            let mapping_addr = map_memory(&current_task, UserAddress::default(), 2 * *PAGE_SIZE);

            let second_page = (mapping_addr + *PAGE_SIZE).unwrap();
            current_task.mm().unwrap().unmap(mapping_addr, *PAGE_SIZE as usize).unwrap();

            // This should fail with ENOMEM since the start of the range is in unmapped memory.
            assert_eq!(
                sys_prctl(
                    &mut current_task,
                    PR_SET_VMA,
                    PR_SET_VMA_ANON_NAME as u64,
                    mapping_addr.ptr() as u64,
                    2 * *PAGE_SIZE,
                    name_addr.ptr() as u64,
                ),
                error!(ENOMEM)
            );

            // Unlike a range which starts within a mapping and extends past the end, this should not assign
            // a name to any mappings.
            {
                let mm = current_task.mm().unwrap();
                let state = mm.state.read();

                let (_, mapping) = state.mappings.get(second_page).unwrap();
                assert_eq!(mapping.name(), MappingName::None);
            }
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_set_vma_name_partial() {
        spawn_kernel_and_run(async |mut current_task| {
            let name_addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            current_task
                .write_memory(name_addr, CString::new("foo").unwrap().as_bytes_with_nul())
                .unwrap();

            let mapping_addr = map_memory(&current_task, UserAddress::default(), 3 * *PAGE_SIZE);

            assert_eq!(
                sys_prctl(
                    &mut current_task,
                    PR_SET_VMA,
                    PR_SET_VMA_ANON_NAME as u64,
                    (mapping_addr + *PAGE_SIZE).unwrap().ptr() as u64,
                    *PAGE_SIZE,
                    name_addr.ptr() as u64,
                ),
                Ok(starnix_syscalls::SUCCESS)
            );

            // This should split the mapping into 3 pieces with the second piece having the name "foo"
            {
                let mm = current_task.mm().unwrap();
                let state = mm.state.read();

                let (_, mapping) = state.mappings.get(mapping_addr).unwrap();
                assert_eq!(mapping.name(), MappingName::None);

                let (_, mapping) =
                    state.mappings.get((mapping_addr + *PAGE_SIZE).unwrap()).unwrap();
                assert_eq!(mapping.name(), MappingName::Vma("foo".into()));

                let (_, mapping) =
                    state.mappings.get((mapping_addr + (2 * *PAGE_SIZE)).unwrap()).unwrap();
                assert_eq!(mapping.name(), MappingName::None);
            }
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_preserve_name_snapshot() {
        spawn_kernel_and_run(async |mut current_task| {
            let name_addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            current_task
                .write_memory(name_addr, CString::new("foo").unwrap().as_bytes_with_nul())
                .unwrap();

            let mapping_addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);

            assert_eq!(
                sys_prctl(
                    &mut current_task,
                    PR_SET_VMA,
                    PR_SET_VMA_ANON_NAME as u64,
                    mapping_addr.ptr() as u64,
                    *PAGE_SIZE,
                    name_addr.ptr() as u64,
                ),
                Ok(starnix_syscalls::SUCCESS)
            );

            let target = current_task.clone_task_for_test(0, None);

            {
                let mm = target.mm().unwrap();
                let state = mm.state.read();

                let (_, mapping) = state.mappings.get(mapping_addr).unwrap();
                assert_eq!(mapping.name(), MappingName::Vma("foo".into()));
            }
        })
        .await;
    }
}

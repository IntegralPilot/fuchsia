// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Type-safe bindings for Zircon resources.

#![allow(clippy::bad_bit_mask)] // TODO(https://fxbug.dev/42080521): stop using bitflags for ResourceKind

use crate::{Event, MonotonicDuration, NullableHandle, ObjectQuery, Status, Topic, ok};
use bitflags::bitflags;
use zx_sys::{self as sys, ZX_MAX_NAME_LEN, zx_duration_mono_t, zx_duration_t};

/// An object representing a Zircon resource.
///
/// As essentially a subtype of `NullableHandle`, it can be freely interconverted.
#[derive(Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct Resource(NullableHandle);
impl_handle_based!(Resource);

sys::zx_info_kmem_stats_t!(MemStats);
sys::zx_info_kmem_stats_extended_t!(MemStatsExtended);
sys::zx_info_kmem_stats_compression_t!(MemStatsCompression);
sys::zx_info_cpu_stats_t!(PerCpuStats);
sys::zx_info_resource_t!(ResourceInfo);
sys::zx_info_memory_stall_t!(MemoryStall);

impl From<sys::zx_info_kmem_stats_t> for MemStats {
    fn from(info: sys::zx_info_kmem_stats_t) -> MemStats {
        let sys::zx_info_kmem_stats_t {
            total_bytes,
            free_bytes,
            free_loaned_bytes,
            wired_bytes,
            total_heap_bytes,
            free_heap_bytes,
            vmo_bytes,
            mmu_overhead_bytes,
            ipc_bytes,
            cache_bytes,
            slab_bytes,
            zram_bytes,
            other_bytes,
            vmo_reclaim_total_bytes,
            vmo_reclaim_newest_bytes,
            vmo_reclaim_oldest_bytes,
            vmo_reclaim_disabled_bytes,
            vmo_discardable_locked_bytes,
            vmo_discardable_unlocked_bytes,
        } = info;
        MemStats {
            total_bytes,
            free_bytes,
            free_loaned_bytes,
            wired_bytes,
            total_heap_bytes,
            free_heap_bytes,
            vmo_bytes,
            mmu_overhead_bytes,
            ipc_bytes,
            cache_bytes,
            slab_bytes,
            zram_bytes,
            other_bytes,
            vmo_reclaim_total_bytes,
            vmo_reclaim_newest_bytes,
            vmo_reclaim_oldest_bytes,
            vmo_reclaim_disabled_bytes,
            vmo_discardable_locked_bytes,
            vmo_discardable_unlocked_bytes,
        }
    }
}

impl From<sys::zx_info_kmem_stats_extended_t> for MemStatsExtended {
    fn from(info: sys::zx_info_kmem_stats_extended_t) -> MemStatsExtended {
        let sys::zx_info_kmem_stats_extended_t {
            total_bytes,
            free_bytes,
            wired_bytes,
            total_heap_bytes,
            free_heap_bytes,
            vmo_bytes,
            vmo_pager_total_bytes,
            vmo_pager_newest_bytes,
            vmo_pager_oldest_bytes,
            vmo_discardable_locked_bytes,
            vmo_discardable_unlocked_bytes,
            mmu_overhead_bytes,
            ipc_bytes,
            other_bytes,
            vmo_reclaim_disable_bytes,
        } = info;
        MemStatsExtended {
            total_bytes,
            free_bytes,
            wired_bytes,
            total_heap_bytes,
            free_heap_bytes,
            vmo_bytes,
            vmo_pager_total_bytes,
            vmo_pager_newest_bytes,
            vmo_pager_oldest_bytes,
            vmo_discardable_locked_bytes,
            vmo_discardable_unlocked_bytes,
            mmu_overhead_bytes,
            ipc_bytes,
            other_bytes,
            vmo_reclaim_disable_bytes,
        }
    }
}

impl From<sys::zx_info_kmem_stats_compression_t> for MemStatsCompression {
    fn from(info: sys::zx_info_kmem_stats_compression_t) -> MemStatsCompression {
        let sys::zx_info_kmem_stats_compression_t {
            uncompressed_storage_bytes,
            compressed_storage_bytes,
            compressed_fragmentation_bytes,
            compression_time,
            decompression_time,
            total_page_compression_attempts,
            failed_page_compression_attempts,
            total_page_decompressions,
            compressed_page_evictions,
            eager_page_compressions,
            memory_pressure_page_compressions,
            critical_memory_page_compressions,
            pages_decompressed_unit_ns,
            pages_decompressed_within_log_time,
        } = info;
        MemStatsCompression {
            uncompressed_storage_bytes,
            compressed_storage_bytes,
            compressed_fragmentation_bytes,
            compression_time,
            decompression_time,
            total_page_compression_attempts,
            failed_page_compression_attempts,
            total_page_decompressions,
            compressed_page_evictions,
            eager_page_compressions,
            memory_pressure_page_compressions,
            critical_memory_page_compressions,
            pages_decompressed_unit_ns,
            pages_decompressed_within_log_time,
        }
    }
}

impl From<sys::zx_info_cpu_stats_t> for PerCpuStats {
    fn from(info: sys::zx_info_cpu_stats_t) -> PerCpuStats {
        let sys::zx_info_cpu_stats_t {
            cpu_number,
            flags,
            idle_time,
            normalized_busy_time,
            reschedules,
            context_switches,
            irq_preempts,
            preempts,
            yields,
            ints,
            timer_ints,
            timers,
            page_faults,
            exceptions,
            syscalls,
            reschedule_ipis,
            generic_ipis,
            active_energy_consumption_nj,
            idle_energy_consumption_nj,
        } = info;
        PerCpuStats {
            cpu_number,
            flags,
            idle_time,
            normalized_busy_time,
            reschedules,
            context_switches,
            irq_preempts,
            preempts,
            yields,
            ints,
            timer_ints,
            timers,
            page_faults,
            exceptions,
            syscalls,
            reschedule_ipis,
            generic_ipis,
            active_energy_consumption_nj,
            idle_energy_consumption_nj,
        }
    }
}

impl From<sys::zx_info_resource_t> for ResourceInfo {
    fn from(info: sys::zx_info_resource_t) -> ResourceInfo {
        let sys::zx_info_resource_t { kind, flags, base, size, name } = info;
        ResourceInfo { kind, flags, base, size, name }
    }
}

impl From<sys::zx_info_memory_stall_t> for MemoryStall {
    fn from(info: sys::zx_info_memory_stall_t) -> MemoryStall {
        let sys::zx_info_memory_stall_t { stall_time_some, stall_time_full } = info;
        MemoryStall { stall_time_some, stall_time_full }
    }
}

unsafe impl ObjectQuery for MemStats {
    const TOPIC: Topic = Topic::KMEM_STATS;
    type InfoTy = MemStats;
}

unsafe impl ObjectQuery for MemStatsExtended {
    const TOPIC: Topic = Topic::KMEM_STATS_EXTENDED;
    type InfoTy = MemStatsExtended;
}

unsafe impl ObjectQuery for MemStatsCompression {
    const TOPIC: Topic = Topic::KMEM_STATS_COMPRESSION;
    type InfoTy = MemStatsCompression;
}

unsafe impl ObjectQuery for PerCpuStats {
    const TOPIC: Topic = Topic::CPU_STATS;
    type InfoTy = PerCpuStats;
}

unsafe impl ObjectQuery for ResourceInfo {
    const TOPIC: Topic = Topic::RESOURCE;
    type InfoTy = ResourceInfo;
}

unsafe impl ObjectQuery for MemoryStall {
    const TOPIC: Topic = Topic::MEMORY_STALL;
    type InfoTy = MemoryStall;
}

bitflags! {
    #[repr(transparent)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct ResourceKind: sys::zx_rsrc_kind_t {
       const MMIO       = sys::ZX_RSRC_KIND_MMIO;
       const IRQ        = sys::ZX_RSRC_KIND_IRQ;
       const IOPORT     = sys::ZX_RSRC_KIND_IOPORT;
       const SMC        = sys::ZX_RSRC_KIND_SMC;
       const SYSTEM     = sys::ZX_RSRC_KIND_SYSTEM;
    }
}

bitflags! {
    #[repr(transparent)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct ResourceFlag: sys::zx_rsrc_flags_t {
       const EXCLUSIVE = sys::ZX_RSRC_FLAG_EXCLUSIVE;
    }
}

bitflags! {
    #[repr(transparent)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct MemoryStallKind: sys::zx_system_memory_stall_type_t {
       const SOME       = sys::ZX_SYSTEM_MEMORY_STALL_SOME;
       const FULL       = sys::ZX_SYSTEM_MEMORY_STALL_FULL;
    }
}

impl Resource {
    /// Create a child resource object.
    ///
    /// Wraps the
    /// [zx_resource_create](https://fuchsia.dev/fuchsia-src/reference/syscalls/resource_create.md)
    /// syscall
    pub fn create_child(
        &self,
        kind: ResourceKind,
        flags: Option<ResourceFlag>,
        base: u64,
        size: usize,
        name: &[u8],
    ) -> Result<Resource, Status> {
        let mut resource_out = 0;
        let name_ptr = name.as_ptr();
        let name_len = name.len();
        let flag_bits: u32 = match flags {
            Some(flag) => flag.bits(),
            None => 0,
        };
        let option_bits: u32 = kind.bits() | flag_bits;

        let status = unsafe {
            sys::zx_resource_create(
                self.raw_handle(),
                option_bits,
                base,
                size,
                name_ptr,
                name_len,
                &mut resource_out,
            )
        };
        ok(status)?;
        unsafe { Ok(Resource::from(NullableHandle::from_raw(resource_out))) }
    }

    /// Wraps the
    /// [zx_object_get_info](https://fuchsia.dev/fuchsia-src/reference/syscalls/object_get_info.md)
    /// syscall for the ZX_INFO_RESOURCE topic.
    pub fn info(&self) -> Result<ResourceInfo, Status> {
        self.0.get_info_single::<ResourceInfo>()
    }

    /// Wraps the
    /// [zx_object_get_info](https://fuchsia.dev/fuchsia-src/reference/syscalls/object_get_info.md)
    /// syscall for the ZX_INFO_CPU_STATS topic.
    pub fn cpu_stats(&self) -> Result<Vec<PerCpuStats>, Status> {
        self.0.get_info_vec::<PerCpuStats>()
    }

    /// Wraps the
    /// [zx_object_get_info](https://fuchsia.dev/fuchsia-src/reference/syscalls/object_get_info.md)
    /// syscall for the ZX_INFO_KMEM_STATS topic.
    pub fn mem_stats(&self) -> Result<MemStats, Status> {
        self.0.get_info_single::<MemStats>()
    }

    /// Wraps the
    /// [zx_object_get_info](https://fuchsia.dev/fuchsia-src/reference/syscalls/object_get_info.md)
    /// syscall for the ZX_INFO_KMEM_STATS_EXTENDED topic.
    pub fn mem_stats_extended(&self) -> Result<MemStatsExtended, Status> {
        self.0.get_info_single::<MemStatsExtended>()
    }

    /// Wraps the
    /// [zx_object_get_info](https://fuchsia.dev/fuchsia-src/reference/syscalls/object_get_info.md)
    /// syscall for the ZX_INFO_KMEM_STATS_COMPRESSION topic.
    pub fn mem_stats_compression(&self) -> Result<MemStatsCompression, Status> {
        self.0.get_info_single::<MemStatsCompression>()
    }

    /// Wraps the
    /// [zx_object_get_info](https://fuchsia.dev/fuchsia-src/reference/syscalls/object_get_info.md)
    /// syscall for the ZX_INFO_MEMORY_STALL topic.
    pub fn memory_stall(&self) -> Result<MemoryStall, Status> {
        self.0.get_info_single::<MemoryStall>()
    }

    /// Retrieve an event that becomes signaled if the memory stall level exceeds a given threshold
    /// over a time window.
    ///
    /// Wraps the
    /// [zx_system_watch_memory_stall](https://fuchsia.dev/fuchsia-src/reference/syscalls/system_watch_memory_stall.md)
    /// syscall
    pub fn watch_memory_stall(
        &self,
        kind: MemoryStallKind,
        threshold: MonotonicDuration,
        window: MonotonicDuration,
    ) -> Result<Event, Status> {
        let mut event_out = 0;
        let status = unsafe {
            sys::zx_system_watch_memory_stall(
                self.raw_handle(),
                kind.bits(),
                threshold.into_nanos(),
                window.into_nanos(),
                &mut event_out,
            )
        };
        ok(status)?;
        unsafe { Ok(Event::from(NullableHandle::from_raw(event_out))) }
    }
}

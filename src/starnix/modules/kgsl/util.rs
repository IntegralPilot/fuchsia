// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bitfield::bitfield;
use starnix_core::task::CurrentTask;
use starnix_uapi::uapi;
use starnix_uapi::user_address::ArchSpecific;

#[rustfmt::skip]
pub fn canonicalize_ioctl_request(current_task: &CurrentTask, request: u32) -> u32 {
    //! Converts kgsl ioctl requests to their canonical (64-bit) values.
    if !current_task.is_arch32() {
        return request
    }
    // Some of the IOCTLS have duplicate values.
    #[allow(unreachable_patterns)]
    match request {
        // Regenerate with the following command:
        // sed -rn 's/.*(IOCTL_KGSL_[^:]*).*/uapi::arch32::\1 => uapi::\1,/p' arm64.rs
        uapi::arch32::IOCTL_KGSL_DEVICE_GETPROPERTY => uapi::IOCTL_KGSL_DEVICE_GETPROPERTY,
        uapi::arch32::IOCTL_KGSL_DEVICE_WAITTIMESTAMP => uapi::IOCTL_KGSL_DEVICE_WAITTIMESTAMP,
        uapi::arch32::IOCTL_KGSL_DEVICE_WAITTIMESTAMP_CTXTID => uapi::IOCTL_KGSL_DEVICE_WAITTIMESTAMP_CTXTID,
        uapi::arch32::IOCTL_KGSL_RINGBUFFER_ISSUEIBCMDS => uapi::IOCTL_KGSL_RINGBUFFER_ISSUEIBCMDS,
        uapi::arch32::IOCTL_KGSL_CMDSTREAM_READTIMESTAMP_OLD => uapi::IOCTL_KGSL_CMDSTREAM_READTIMESTAMP_OLD,
        uapi::arch32::IOCTL_KGSL_CMDSTREAM_READTIMESTAMP => uapi::IOCTL_KGSL_CMDSTREAM_READTIMESTAMP,
        uapi::arch32::IOCTL_KGSL_CMDSTREAM_FREEMEMONTIMESTAMP => uapi::IOCTL_KGSL_CMDSTREAM_FREEMEMONTIMESTAMP,
        uapi::arch32::IOCTL_KGSL_CMDSTREAM_FREEMEMONTIMESTAMP_OLD => uapi::IOCTL_KGSL_CMDSTREAM_FREEMEMONTIMESTAMP_OLD,
        uapi::arch32::IOCTL_KGSL_DRAWCTXT_CREATE => uapi::IOCTL_KGSL_DRAWCTXT_CREATE,
        uapi::arch32::IOCTL_KGSL_DRAWCTXT_DESTROY => uapi::IOCTL_KGSL_DRAWCTXT_DESTROY,
        uapi::arch32::IOCTL_KGSL_MAP_USER_MEM => uapi::IOCTL_KGSL_MAP_USER_MEM,
        uapi::arch32::IOCTL_KGSL_CMDSTREAM_READTIMESTAMP_CTXTID => uapi::IOCTL_KGSL_CMDSTREAM_READTIMESTAMP_CTXTID,
        uapi::arch32::IOCTL_KGSL_CMDSTREAM_FREEMEMONTIMESTAMP_CTXTID => uapi::IOCTL_KGSL_CMDSTREAM_FREEMEMONTIMESTAMP_CTXTID,
        uapi::arch32::IOCTL_KGSL_SHAREDMEM_FROM_PMEM => uapi::IOCTL_KGSL_SHAREDMEM_FROM_PMEM,
        uapi::arch32::IOCTL_KGSL_SHAREDMEM_FREE => uapi::IOCTL_KGSL_SHAREDMEM_FREE,
        uapi::arch32::IOCTL_KGSL_CFF_USER_EVENT => uapi::IOCTL_KGSL_CFF_USER_EVENT,
        uapi::arch32::IOCTL_KGSL_DRAWCTXT_BIND_GMEM_SHADOW => uapi::IOCTL_KGSL_DRAWCTXT_BIND_GMEM_SHADOW,
        uapi::arch32::IOCTL_KGSL_SHAREDMEM_FROM_VMALLOC => uapi::IOCTL_KGSL_SHAREDMEM_FROM_VMALLOC,
        uapi::arch32::IOCTL_KGSL_SHAREDMEM_FLUSH_CACHE => uapi::IOCTL_KGSL_SHAREDMEM_FLUSH_CACHE,
        uapi::arch32::IOCTL_KGSL_DRAWCTXT_SET_BIN_BASE_OFFSET => uapi::IOCTL_KGSL_DRAWCTXT_SET_BIN_BASE_OFFSET,
        uapi::arch32::IOCTL_KGSL_CMDWINDOW_WRITE => uapi::IOCTL_KGSL_CMDWINDOW_WRITE,
        uapi::arch32::IOCTL_KGSL_GPUMEM_ALLOC => uapi::IOCTL_KGSL_GPUMEM_ALLOC,
        uapi::arch32::IOCTL_KGSL_CFF_SYNCMEM => uapi::IOCTL_KGSL_CFF_SYNCMEM,
        uapi::arch32::IOCTL_KGSL_TIMESTAMP_EVENT_OLD => uapi::IOCTL_KGSL_TIMESTAMP_EVENT_OLD,
        uapi::arch32::IOCTL_KGSL_SETPROPERTY => uapi::IOCTL_KGSL_SETPROPERTY,
        uapi::arch32::IOCTL_KGSL_TIMESTAMP_EVENT => uapi::IOCTL_KGSL_TIMESTAMP_EVENT,
        uapi::arch32::IOCTL_KGSL_GPUMEM_ALLOC_ID => uapi::IOCTL_KGSL_GPUMEM_ALLOC_ID,
        uapi::arch32::IOCTL_KGSL_GPUMEM_FREE_ID => uapi::IOCTL_KGSL_GPUMEM_FREE_ID,
        uapi::arch32::IOCTL_KGSL_GPUMEM_GET_INFO => uapi::IOCTL_KGSL_GPUMEM_GET_INFO,
        uapi::arch32::IOCTL_KGSL_GPUMEM_SYNC_CACHE => uapi::IOCTL_KGSL_GPUMEM_SYNC_CACHE,
        uapi::arch32::IOCTL_KGSL_PERFCOUNTER_GET => uapi::IOCTL_KGSL_PERFCOUNTER_GET,
        uapi::arch32::IOCTL_KGSL_PERFCOUNTER_PUT => uapi::IOCTL_KGSL_PERFCOUNTER_PUT,
        uapi::arch32::IOCTL_KGSL_PERFCOUNTER_QUERY => uapi::IOCTL_KGSL_PERFCOUNTER_QUERY,
        uapi::arch32::IOCTL_KGSL_PERFCOUNTER_READ => uapi::IOCTL_KGSL_PERFCOUNTER_READ,
        uapi::arch32::IOCTL_KGSL_GPUMEM_SYNC_CACHE_BULK => uapi::IOCTL_KGSL_GPUMEM_SYNC_CACHE_BULK,
        uapi::arch32::IOCTL_KGSL_SUBMIT_COMMANDS => uapi::IOCTL_KGSL_SUBMIT_COMMANDS,
        uapi::arch32::IOCTL_KGSL_SYNCSOURCE_CREATE => uapi::IOCTL_KGSL_SYNCSOURCE_CREATE,
        uapi::arch32::IOCTL_KGSL_SYNCSOURCE_DESTROY => uapi::IOCTL_KGSL_SYNCSOURCE_DESTROY,
        uapi::arch32::IOCTL_KGSL_SYNCSOURCE_CREATE_FENCE => uapi::IOCTL_KGSL_SYNCSOURCE_CREATE_FENCE,
        uapi::arch32::IOCTL_KGSL_SYNCSOURCE_SIGNAL_FENCE => uapi::IOCTL_KGSL_SYNCSOURCE_SIGNAL_FENCE,
        uapi::arch32::IOCTL_KGSL_CFF_SYNC_GPUOBJ => uapi::IOCTL_KGSL_CFF_SYNC_GPUOBJ,
        uapi::arch32::IOCTL_KGSL_GPUOBJ_ALLOC => uapi::IOCTL_KGSL_GPUOBJ_ALLOC,
        uapi::arch32::IOCTL_KGSL_GPUOBJ_FREE => uapi::IOCTL_KGSL_GPUOBJ_FREE,
        uapi::arch32::IOCTL_KGSL_GPUOBJ_INFO => uapi::IOCTL_KGSL_GPUOBJ_INFO,
        uapi::arch32::IOCTL_KGSL_GPUOBJ_IMPORT => uapi::IOCTL_KGSL_GPUOBJ_IMPORT,
        uapi::arch32::IOCTL_KGSL_GPUOBJ_SYNC => uapi::IOCTL_KGSL_GPUOBJ_SYNC,
        uapi::arch32::IOCTL_KGSL_GPU_COMMAND => uapi::IOCTL_KGSL_GPU_COMMAND,
        uapi::arch32::IOCTL_KGSL_PREEMPTIONCOUNTER_QUERY => uapi::IOCTL_KGSL_PREEMPTIONCOUNTER_QUERY,
        uapi::arch32::IOCTL_KGSL_GPUOBJ_SET_INFO => uapi::IOCTL_KGSL_GPUOBJ_SET_INFO,
        uapi::arch32::IOCTL_KGSL_SPARSE_PHYS_ALLOC => uapi::IOCTL_KGSL_SPARSE_PHYS_ALLOC,
        uapi::arch32::IOCTL_KGSL_SPARSE_PHYS_FREE => uapi::IOCTL_KGSL_SPARSE_PHYS_FREE,
        uapi::arch32::IOCTL_KGSL_SPARSE_VIRT_ALLOC => uapi::IOCTL_KGSL_SPARSE_VIRT_ALLOC,
        uapi::arch32::IOCTL_KGSL_SPARSE_VIRT_FREE => uapi::IOCTL_KGSL_SPARSE_VIRT_FREE,
        uapi::arch32::IOCTL_KGSL_SPARSE_BIND => uapi::IOCTL_KGSL_SPARSE_BIND,
        uapi::arch32::IOCTL_KGSL_GPU_SPARSE_COMMAND => uapi::IOCTL_KGSL_GPU_SPARSE_COMMAND,
        uapi::arch32::IOCTL_KGSL_GPUMEM_BIND_RANGES => uapi::IOCTL_KGSL_GPUMEM_BIND_RANGES,
        uapi::arch32::IOCTL_KGSL_GPU_AUX_COMMAND => uapi::IOCTL_KGSL_GPU_AUX_COMMAND,
        uapi::arch32::IOCTL_KGSL_TIMELINE_CREATE => uapi::IOCTL_KGSL_TIMELINE_CREATE,
        uapi::arch32::IOCTL_KGSL_TIMELINE_WAIT => uapi::IOCTL_KGSL_TIMELINE_WAIT,
        uapi::arch32::IOCTL_KGSL_TIMELINE_QUERY => uapi::IOCTL_KGSL_TIMELINE_QUERY,
        uapi::arch32::IOCTL_KGSL_TIMELINE_SIGNAL => uapi::IOCTL_KGSL_TIMELINE_SIGNAL,
        uapi::arch32::IOCTL_KGSL_TIMELINE_FENCE_GET => uapi::IOCTL_KGSL_TIMELINE_FENCE_GET,
        uapi::arch32::IOCTL_KGSL_TIMELINE_DESTROY => uapi::IOCTL_KGSL_TIMELINE_DESTROY,
        uapi::arch32::IOCTL_KGSL_GET_FAULT_REPORT => uapi::IOCTL_KGSL_GET_FAULT_REPORT,
        uapi::arch32::IOCTL_KGSL_RECURRING_COMMAND => uapi::IOCTL_KGSL_RECURRING_COMMAND,
        uapi::arch32::IOCTL_KGSL_READ_CALIBRATED_TIMESTAMPS => uapi::IOCTL_KGSL_READ_CALIBRATED_TIMESTAMPS,
        _ => request,
    }
}

pub mod maur {
    //! Helper for MultiArchUserRef types.
    use starnix_core::mm::MemoryAccessorExt;
    use starnix_core::task::CurrentTask;
    use starnix_syscalls::{SUCCESS, SyscallResult};
    use starnix_uapi::errors::Errno;
    use starnix_uapi::uapi::{self, uaddr};
    use starnix_uapi::user_address::{MultiArchUserRef, UserAddress, UserRef};

    pub trait TaskWritable {
        fn write(self, task: &CurrentTask, addr: uaddr) -> Result<SyscallResult, Errno>;
    }

    impl TaskWritable for u32 {
        fn write(self, task: &CurrentTask, addr: uaddr) -> Result<SyscallResult, Errno> {
            let result_ref = UserRef::from(UserAddress::from(addr));
            task.write_object(result_ref, &self).map(|_| SUCCESS)
        }
    }

    impl TaskWritable for u64 {
        fn write(self, task: &CurrentTask, addr: uaddr) -> Result<SyscallResult, Errno> {
            let result_ref = UserRef::from(UserAddress::from(addr));
            task.write_object(result_ref, &self).map(|_| SUCCESS)
        }
    }

    macro_rules! create_multi_arch_types {
        ($($name:ident),+ $(,)?) => {
            $(
                // For each type foo, define a maur::foo type alias.
                #[allow(dead_code, non_camel_case_types)]
                pub type $name = MultiArchUserRef<uapi::$name, uapi::arch32::$name>;

                // Implements a multi-arch write for each type.
                impl TaskWritable for uapi::$name {
                    fn write(self, task: &CurrentTask, addr: uaddr) -> Result<SyscallResult, Errno> {
                        let result_ref = MultiArchUserRef::<uapi::$name, uapi::arch32::$name>::new(task, addr);
                        task.write_multi_arch_object(result_ref, self).map(|_| SUCCESS)
                    }
                }
            )+
        };
    }

    create_multi_arch_types!(
        // keep-sorted start
        kgsl_device_getproperty,
        kgsl_devinfo,
        kgsl_drawctxt_create,
        kgsl_drawctxt_destroy,
        kgsl_gpu_command,
        kgsl_gpuobj_alloc,
        kgsl_gpuobj_free,
        kgsl_gpuobj_info,
        kgsl_qdss_stm_prop,
        kgsl_qtimer_prop,
        kgsl_shadowprop,
        kgsl_syncsource_create,
        kgsl_syncsource_destroy,
        kgsl_ucode_version,
        // keep-sorted end
    );
}

macro_rules! kgsl_bitfield {
    (
        pub struct $name:ident($type:ty);
        $(
            pub $field_name:ident,
            $(bit($bit:path))?
            $(range($mask:path, $shift:path))?
            ;
        )*
    ) => {
        bitfield! {
            pub struct $name($type);
            impl Debug;
            $($(
                pub $field_name, _: $bit.trailing_zeros().try_into().unwrap();
            )?)*
            $($(
                pub $field_name, _: ($shift + $mask.count_ones() - 1) as usize, $shift.try_into().unwrap();
            )?)*
        }

        impl $name {
            pub const DEFINED_BITS_MASK: $type = 0
                $($( | ($bit as $type) )?)*
                $($( | ($mask as $type) )?)*
            ;
        }

        /// Returns the bitfield if all bits are defined, otherwise returns the undefined bits.
        impl TryFrom<$type> for $name {
            type Error = $type;
            fn try_from(value: $type) -> Result<Self, $type> {
                let undefined_bits = value & !Self::DEFINED_BITS_MASK;
                if undefined_bits != 0 {
                    return Err(undefined_bits);
                }
                Ok(Self(value))
            }
        }
    }
}

kgsl_bitfield! {
    // KGSL_CONTEXT flags
    pub struct KgslContextFlags(u32);
    pub save_gmem, bit(uapi::KGSL_CONTEXT_SAVE_GMEM);
    pub no_gmem_alloc, bit(uapi::KGSL_CONTEXT_NO_GMEM_ALLOC);
    pub submit_ib_list, bit(uapi::KGSL_CONTEXT_SUBMIT_IB_LIST);
    pub ctx_switch, bit(uapi::KGSL_CONTEXT_CTX_SWITCH);
    pub preamble, bit(uapi::KGSL_CONTEXT_PREAMBLE);
    pub trash_state, bit(uapi::KGSL_CONTEXT_TRASH_STATE);
    pub per_context_ts, bit(uapi::KGSL_CONTEXT_PER_CONTEXT_TS);
    pub user_generated_ts, bit(uapi::KGSL_CONTEXT_USER_GENERATED_TS);
    pub end_of_frame, bit(uapi::KGSL_CONTEXT_END_OF_FRAME);
    pub no_fault_tolerance, bit(uapi::KGSL_CONTEXT_NO_FAULT_TOLERANCE);
    pub sync, bit(uapi::KGSL_CONTEXT_SYNC);
    pub pwr_constraint, bit(uapi::KGSL_CONTEXT_PWR_CONSTRAINT);
    pub priority, range(uapi::KGSL_CONTEXT_PRIORITY_MASK, uapi::KGSL_CONTEXT_PRIORITY_SHIFT);
    pub ifh_nop, bit(uapi::KGSL_CONTEXT_IFH_NOP);
    pub secure, bit(uapi::KGSL_CONTEXT_SECURE);
    pub no_snapshot, bit(uapi::KGSL_CONTEXT_NO_SNAPSHOT);
    pub sparse, bit(uapi::KGSL_CONTEXT_SPARSE);
    pub preempt_style, range(uapi::KGSL_CONTEXT_PREEMPT_STYLE_MASK, uapi::KGSL_CONTEXT_PREEMPT_STYLE_SHIFT);
    pub type_, range(uapi::KGSL_CONTEXT_TYPE_MASK, uapi::KGSL_CONTEXT_TYPE_SHIFT);
}

kgsl_bitfield! {
    // KGSL_CMDBATCH flags
    pub struct KgslCmdBatchFlags(u64);
    pub memlist, bit(uapi::KGSL_CMDBATCH_MEMLIST);
    pub marker, bit(uapi::KGSL_CMDBATCH_MARKER);
    pub submit_ib_list, bit(uapi::KGSL_CMDBATCH_SUBMIT_IB_LIST);
    pub ctx_switch, bit(uapi::KGSL_CMDBATCH_CTX_SWITCH);
    pub profiling, bit(uapi::KGSL_CMDBATCH_PROFILING);
    pub profiling_ktime, bit(uapi::KGSL_CMDBATCH_PROFILING_KTIME);
    pub end_of_frame, bit(uapi::KGSL_CMDBATCH_END_OF_FRAME);
    pub sync, bit(uapi::KGSL_CMDBATCH_SYNC);
    pub pwr_constraint, bit(uapi::KGSL_CMDBATCH_PWR_CONSTRAINT);
    pub sparse, bit(uapi::KGSL_CMDBATCH_SPARSE);
}

kgsl_bitfield! {
    // KGSL_MEMFLAGS flags
    pub struct KgslMemFlags(u64);
    pub secure, bit(uapi::KGSL_MEMFLAGS_SECURE);
    pub gpu_read_only, bit(uapi::KGSL_MEMFLAGS_GPUREADONLY);
    pub gpu_write_only, bit(uapi::KGSL_MEMFLAGS_GPUWRITEONLY);
    pub force_32bit, bit(uapi::KGSL_MEMFLAGS_FORCE_32BIT);
    pub use_cpu_map, bit(uapi::KGSL_MEMFLAGS_USE_CPU_MAP);
    pub sparse_phys, bit(uapi::KGSL_MEMFLAGS_SPARSE_PHYS);
    pub sparse_virt, bit(uapi::KGSL_MEMFLAGS_SPARSE_VIRT);
    pub incoherent, bit(uapi::KGSL_MEMFLAGS_IOCOHERENT);
    pub type_, range(uapi::KGSL_MEMTYPE_MASK, uapi::KGSL_MEMTYPE_SHIFT);
    pub align_bits, range(uapi::KGSL_MEMALIGN_MASK, uapi::KGSL_MEMALIGN_SHIFT);
    // Caution! Unlike other range-based values, the USERMEM values are pre-shifted.
    pub usermem, range(uapi::KGSL_MEMFLAGS_USERMEM_MASK, uapi::KGSL_MEMFLAGS_USERMEM_SHIFT);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bitfields() {
        let raw_flags = uapi::KGSL_CONTEXT_SAVE_GMEM
            | uapi::KGSL_CONTEXT_PREAMBLE
            | (9 << uapi::KGSL_CONTEXT_PRIORITY_SHIFT)
            | (uapi::KGSL_CONTEXT_TYPE_VK << uapi::KGSL_CONTEXT_TYPE_SHIFT);
        let flags = KgslContextFlags(raw_flags);
        assert!(flags.save_gmem());
        assert!(flags.preamble());
        assert_eq!(flags.priority(), 9);
        assert_eq!(flags.type_(), uapi::KGSL_CONTEXT_TYPE_VK);
        let invalid_flags = 0xCC000000;
        let result = KgslCmdBatchFlags::try_from(invalid_flags | uapi::KGSL_CMDBATCH_MARKER as u64);
        assert!(result.is_err());
        assert_eq!(result.err().unwrap(), invalid_flags);
    }
}

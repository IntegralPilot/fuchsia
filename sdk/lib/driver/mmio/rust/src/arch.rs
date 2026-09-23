// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
//! Architecture specific instructions for working with device memory.
use core::ptr::NonNull;

// TODO(b/423071349): deprecate this module if core::arch::{load, store} get stabilized.
// https://github.com/rust-lang/unsafe-code-guidelines/issues/321.

// The default implementation uses core::ptr::read_volatile and core::ptr::write_volatile for
// all loads and stores. On certain platforms this may produce instructions that are
// incompatible with certain hypervisors.
#[cfg(all(not(target_arch = "x86_64"), not(target_arch = "aarch64")))]
mod ops {
    macro_rules! load_op {
        ($_:tt, $ptr:expr) => {
            // Safety: provided by caller.
            unsafe { core::ptr::read_volatile($ptr.as_ptr()) }
        };
    }

    macro_rules! store_op {
        ($_type:tt, $ptr:expr, $value:expr) => {
            // Safety: provided by caller.
            unsafe { core::ptr::write_volatile($ptr.as_ptr(), $value) }
        };
    }

    macro_rules! write_barrier_op {
        () => {
            std::sync::atomic::fence(std::sync::atomic::Ordering::Release)
        };
    }

    pub(super) use {load_op, store_op, write_barrier_op};
}

// KVM on x86 doesn't support Mmio using vector registers. Specify the instructions for this
// platform explicitly in order to reduce the demands on the hypervisor.
//
// For more details see:
// https://cs.opensource.google/fuchsia/fuchsia/+/main:zircon/system/ulib/mmio-ptr/include/lib/mmio-ptr/mmio-ptr.h;l=116;drc=36e68808ae97fc54675be5c3f57484726577425c
#[cfg(target_arch = "x86_64")]
mod ops {
    macro_rules! load_op {
        (u8, $ptr:expr) => {
            load_op!(INTERNAL u8, $ptr, "mov {d}, [{s}]", reg_byte)
        };
        (u16, $ptr:expr) => {
            load_op!(INTERNAL u16, $ptr, "mov {d:x}, [{s}]", reg)
        };
        (u32, $ptr:expr) => {
            load_op!(INTERNAL u32, $ptr, "mov {d:e}, [{s}]", reg)
        };
        (u64, $ptr:expr) => {
            load_op!(INTERNAL u64, $ptr, "mov {d:r}, [{s}]", reg)
        };
        (INTERNAL $type:ty, $ptr:expr, $inst:literal, $reg_class:ident) => {
            {
                let ptr = $ptr.as_ptr();
                let mut res: $type;
                // Safety: provided by caller.
                unsafe {
                    core::arch::asm!($inst, d = out($reg_class) res, s = in(reg) ptr, options(nostack));
                }
                res
            }
        }
    }

    macro_rules! store_op {
        (u8, $ptr:expr, $value:expr) => {
            store_op!(INTERNAL u8, $ptr, $value, "mov [{d}], {s}", reg_byte)
        };
        (u16, $ptr:expr, $value:expr) => {
            store_op!(INTERNAL u16, $ptr, $value, "mov [{d}], {s:x}", reg)
        };
        (u32, $ptr:expr, $value:expr) => {
            store_op!(INTERNAL u32, $ptr, $value, "mov [{d}], {s:e}", reg)
        };
        (u64, $ptr:expr, $value:expr) => {
            store_op!(INTERNAL u64, $ptr, $value, "mov [{d}], {s:r}", reg)
        };
        (INTERNAL $type:ty, $ptr:expr, $value:expr, $inst:literal, $reg_class:ident) => {
            {
                let ptr = $ptr.as_ptr();
                // Safety: provided by caller.
                unsafe {
                    core::arch::asm!($inst, d = in(reg) ptr, s = in($reg_class) $value, options(nostack));
                }
            }
        }
    }

    macro_rules! write_barrier_op {
        () => {
            // Safety: N/A; see [`crate::arch::write_barrier`].
            unsafe {
                core::arch::asm!("sfence", options(nostack, preserves_flags));
            }
        };
    }

    pub(super) use {load_op, store_op, write_barrier_op};
}

// rustc/llvm may generate instructions that are incompatible with being run in certain
// hypervisors.
//
// For more details see:
//
// https://github.com/rust-lang/rust/issues/131894 and
// https://cs.opensource.google/fuchsia/fuchsia/+/main:zircon/system/ulib/mmio-ptr/include/lib/mmio-ptr/mmio-ptr.h;l=57;drc=36e68808ae97fc54675be5c3f57484726577425c
#[cfg(target_arch = "aarch64")]
mod ops {
    macro_rules! load_op {
        (u8, $ptr:expr) => {
            load_op!(INTERNAL u8, $ptr, "ldrb {d:w}, [{s}]")
        };
        (u16, $ptr:expr) => {
            load_op!(INTERNAL u16, $ptr, "ldrh {d:w}, [{s}]")
        };
        (u32, $ptr:expr) => {
            load_op!(INTERNAL u32, $ptr, "ldr {d:w}, [{s}]")
        };
        (u64, $ptr:expr) => {
            load_op!(INTERNAL u64, $ptr, "ldr {d:x}, [{s}]")
        };
        (INTERNAL $type:ty, $ptr:expr, $inst:literal) => {
            {
                let ptr = $ptr.as_ptr();
                let mut res: $type;
                unsafe {
                    core::arch::asm!($inst, d = out(reg) res, s = in(reg) ptr, options(nostack));
                }
                res
            }
        }
    }

    macro_rules! store_op {
        (u8, $ptr:expr, $value:expr) => {
            store_op!(INTERNAL u8, $ptr, $value, "strb {s:w}, [{d}]")
        };
        (u16, $ptr:expr, $value:expr) => {
            store_op!(INTERNAL u16, $ptr, $value, "strh {s:w}, [{d}]")
        };
        (u32, $ptr:expr, $value:expr) => {
            store_op!(INTERNAL u32, $ptr, $value, "str {s:w}, [{d}]")
        };
        (u64, $ptr:expr, $value:expr) => {
            store_op!(INTERNAL u64, $ptr, $value, "str {s:x}, [{d}]")
        };
        (INTERNAL $type:ty, $ptr:expr, $value:expr, $inst:literal) => {
            {
                let ptr = $ptr.as_ptr();
                unsafe {
                    core::arch::asm!($inst, d = in(reg) ptr, s = in(reg) $value, options(nostack));
                }
            }
        }
    }

    macro_rules! write_barrier_op {
        () => {
            // Safety: N/A; see [`crate::arch::write_barrier`].
            unsafe {
                core::arch::asm!("dsb st", options(nostack, preserves_flags));
            }
        };
    }

    pub(super) use {load_op, store_op, write_barrier_op};
}

use ops::{load_op, store_op, write_barrier_op};

macro_rules! load_fn {
    ($name:ident, $type:tt) => {
        #[inline]
        /// Loads a value from the pointee suitably for device memory.
        ///
        /// # Safety
        /// - the pointer must be valid
        /// - the pointer must be aligned
        /// - there must not be concurrent stores overlapping this load
        /// - this pointer must be safe to access as with [core::ptr::read_volatile]
        pub(crate) unsafe fn $name(ptr: NonNull<$type>) -> $type {
            load_op!($type, ptr)
        }
    };
}

load_fn! {load8, u8}
load_fn! {load16, u16}
load_fn! {load32, u32}
load_fn! {load64, u64}

macro_rules! store_fn {
    ($type:tt, $name:ident) => {
        #[inline]
        /// Stores the given balue to the pointee suitably for device memory.
        ///
        /// # Safety
        /// - the pointer must be valid
        /// - the pointer must be aligned
        /// - there must be any concurrent operations overlapping this store
        /// - this pointer must be safe to access as with [core::ptr::write_volatile]
        pub(crate) unsafe fn $name(ptr: NonNull<$type>, value: $type) {
            store_op!($type, ptr, value)
        }
    };
}

store_fn! {u8, store8}
store_fn! {u16, store16}
store_fn! {u32, store32}
store_fn! {u64, store64}

#[inline]
pub(crate) fn write_barrier() {
    // Note that there are no safety requirements for issuing a write barrier, as this operation is
    // outside the scope of Rust's memory model.
    write_barrier_op!()
}

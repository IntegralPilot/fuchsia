// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PAddr(pub usize);

impl From<PAddr> for u64 {
    #[inline]
    fn from(paddr: PAddr) -> u64 {
        paddr.0 as u64
    }
}

impl From<u64> for PAddr {
    #[inline]
    fn from(addr: u64) -> PAddr {
        PAddr(addr as usize)
    }
}

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VAddr(pub usize);

impl From<VAddr> for u64 {
    #[inline]
    fn from(vaddr: VAddr) -> u64 {
        vaddr.0 as u64
    }
}

impl From<u64> for VAddr {
    #[inline]
    fn from(addr: u64) -> VAddr {
        VAddr(addr as usize)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Koid(pub u64);

#[allow(non_camel_case_types)]
pub type cpu_mask_t = u32;

#[allow(non_camel_case_types)]
pub type cpu_num_t = u32;

pub const INVALID_CPU: cpu_num_t = u32::MAX;
pub const CPU_MASK_ALL: cpu_mask_t = u32::MAX;

/// The CPU number of the boot CPU.
///
/// Mirrors `BOOT_CPU_ID` from `zircon/kernel/include/platform.h`.
pub const BOOT_CPU_ID: cpu_num_t = 0;

/// The maximum number of CPUs the kernel was built to support.
///
/// This value is read from the `SMP_MAX_CPUS` environment variable at build time.
pub const SMP_MAX_CPUS: usize =
    zr::parse_usize(env!("SMP_MAX_CPUS")).expect("SMP_MAX_CPUS invalid");

/// Returns `true` if `num` names a CPU the kernel was built to support.
///
/// Mirrors `is_valid_cpu_num()` from `zircon/kernel/include/kernel/cpu.h`.
#[inline]
pub const fn is_valid_cpu_num(num: cpu_num_t) -> bool {
    (num as usize) < SMP_MAX_CPUS
}

/// Returns a mask with only the bit for `num` set, or an empty mask if `num` is not a valid CPU
/// number.
///
/// Mirrors `cpu_num_to_mask()` from `zircon/kernel/include/kernel/cpu.h`.
#[inline]
pub const fn cpu_num_to_mask(num: cpu_num_t) -> cpu_mask_t {
    if !is_valid_cpu_num(num) {
        return 0;
    }
    1 << num
}

/// Returns a mask with the bits for every CPU except `num` set.
///
/// Mirrors `mask_all_but_one()` from `zircon/kernel/include/kernel/cpu.h`.
#[inline]
pub const fn mask_all_but_one(num: cpu_num_t) -> cpu_mask_t {
    CPU_MASK_ALL ^ cpu_num_to_mask(num)
}

/// Returns the number of the highest CPU set in `mask`, or 0 if the mask is empty.
///
/// Mirrors `highest_cpu_set()` from `zircon/kernel/include/kernel/cpu.h`.
#[inline]
pub const fn highest_cpu_set(mask: cpu_mask_t) -> cpu_num_t {
    if mask == 0 {
        return 0;
    }
    (cpu_mask_t::BITS - 1) - mask.leading_zeros()
}

/// Returns the number of the lowest CPU set in `mask`, or 0 if the mask is empty.
///
/// Mirrors `lowest_cpu_set()` from `zircon/kernel/include/kernel/cpu.h`.
#[inline]
pub const fn lowest_cpu_set(mask: cpu_mask_t) -> cpu_num_t {
    if mask == 0 {
        return 0;
    }
    mask.trailing_zeros()
}

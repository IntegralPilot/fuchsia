// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{Status, sys};
use bitflags::bitflags;

/// Retrieve the system memory page size in bytes.
///
/// Wraps the
/// [zx_system_get_page_size](https://fuchsia.dev/fuchsia-src/reference/syscalls/system_get_page_size.md)
/// syscall.
pub fn system_get_page_size() -> u32 {
    unsafe { sys::zx_system_get_page_size() }
}

/// Retrieve the system cache line size in bytes.
///
/// Wraps the
/// [zx_system_get_dcache_line_size](https://fuchsia.dev/fuchsia-src/reference/syscalls/system_get_dcache_line_size.md)
/// syscall.
pub fn system_get_dcache_line_size() -> u32 {
    unsafe { sys::zx_system_get_dcache_line_size() }
}

/// Get the amount of physical memory on the system, in bytes.
///
/// Wraps the
/// [zx_system_get_physmem](https://fuchsia.dev/fuchsia-src/reference/syscalls/system_get_physmem)
/// syscall.
pub fn system_get_physmem() -> u64 {
    unsafe { sys::zx_system_get_physmem() }
}

/// Get number of logical processors on the system.
///
/// Wraps the
/// [zx_system_get_num_cpus](https://fuchsia.dev/fuchsia-src/reference/syscalls/system_get_num_cpus)
/// syscall.
pub fn system_get_num_cpus() -> u32 {
    unsafe { sys::zx_system_get_num_cpus() }
}

/// The types of system features that may be requested.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
#[repr(u32)]
pub enum FeatureKind {
    HardwareBreakpointCount = sys::ZX_FEATURE_KIND_HW_BREAKPOINT_COUNT,
    HardwareWatchpointCount = sys::ZX_FEATURE_KIND_HW_WATCHPOINT_COUNT,
}

impl Into<u32> for FeatureKind {
    fn into(self) -> u32 {
        match self {
            FeatureKind::HardwareBreakpointCount => sys::ZX_FEATURE_KIND_HW_BREAKPOINT_COUNT,
            FeatureKind::HardwareWatchpointCount => sys::ZX_FEATURE_KIND_HW_WATCHPOINT_COUNT,
        }
    }
}

// We use a placeholder type to disallow any other implementations
// of the FeatureFlags trait below.
mod private {
    pub struct Internal;
}

// Trait which encodes the FeatureKind with the bitflags.
pub trait FeatureFlags: bitflags::Flags {
    fn kind(_: private::Internal) -> u32;
}

bitflags! {
    #[repr(transparent)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct VirtualMemoryFeatureFlags: u32 {
        // VirtualMemoryKind flags
        const VM_CAN_MAP_XOM = (1<<0);

        // Allow the source to set any bits.
        const _ = !0;
    }
}

impl FeatureFlags for VirtualMemoryFeatureFlags {
    fn kind(_: private::Internal) -> u32 {
        sys::ZX_FEATURE_KIND_VM
    }
}

bitflags! {
    #[repr(transparent)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct CpuFeatureFlags: u32 {

        // CpuFeatureKind flags
        const HAS_CPU_FEATURES = (1<<0);

        // Target-architecture CpuFeatureKind flags
        #[cfg(target_arch = "aarch64")]
        const ARM64_FEATURE_ISA_FP = sys::ZX_ARM64_FEATURE_ISA_FP;
        #[cfg(target_arch = "aarch64")]
        const ARM64_FEATURE_ISA_ASIMD = sys::ZX_ARM64_FEATURE_ISA_ASIMD;
        #[cfg(target_arch = "aarch64")]
        const ARM64_FEATURE_ISA_AES = sys::ZX_ARM64_FEATURE_ISA_AES;
        #[cfg(target_arch = "aarch64")]
        const ARM64_FEATURE_ISA_PMULL = sys::ZX_ARM64_FEATURE_ISA_PMULL;
        #[cfg(target_arch = "aarch64")]
        const ARM64_FEATURE_ISA_SHA1 = sys::ZX_ARM64_FEATURE_ISA_SHA1;
        #[cfg(target_arch = "aarch64")]
        const ARM64_FEATURE_ISA_SHA256 = sys::ZX_ARM64_FEATURE_ISA_SHA256;
        #[cfg(target_arch = "aarch64")]
        const ARM64_FEATURE_ISA_CRC32 = sys::ZX_ARM64_FEATURE_ISA_CRC32;
        #[cfg(target_arch = "aarch64")]
        const ARM64_FEATURE_ISA_ATOMICS = sys::ZX_ARM64_FEATURE_ISA_ATOMICS;
        #[cfg(target_arch = "aarch64")]
        const ARM64_FEATURE_ISA_RDM = sys::ZX_ARM64_FEATURE_ISA_RDM;
        #[cfg(target_arch = "aarch64")]
        const ARM64_FEATURE_ISA_SHA3 = sys::ZX_ARM64_FEATURE_ISA_SHA3;
        #[cfg(target_arch = "aarch64")]
        const ARM64_FEATURE_ISA_SM3 = sys::ZX_ARM64_FEATURE_ISA_SM3;
        #[cfg(target_arch = "aarch64")]
        const ARM64_FEATURE_ISA_SM4 = sys::ZX_ARM64_FEATURE_ISA_SM4;
        #[cfg(target_arch = "aarch64")]
        const ARM64_FEATURE_ISA_DP = sys::ZX_ARM64_FEATURE_ISA_DP;
        #[cfg(target_arch = "aarch64")]
        const ARM64_FEATURE_ISA_DPB = sys::ZX_ARM64_FEATURE_ISA_DPB;
        #[cfg(target_arch = "aarch64")]
        const ARM64_FEATURE_ISA_FHM = sys::ZX_ARM64_FEATURE_ISA_FHM;
        #[cfg(target_arch = "aarch64")]
        const ARM64_FEATURE_ISA_TS = sys::ZX_ARM64_FEATURE_ISA_TS;
        #[cfg(target_arch = "aarch64")]
        const ARM64_FEATURE_ISA_RNDR = sys::ZX_ARM64_FEATURE_ISA_RNDR;
        #[cfg(target_arch = "aarch64")]
        const ARM64_FEATURE_ISA_SHA512 = sys::ZX_ARM64_FEATURE_ISA_SHA512;
        #[cfg(target_arch = "aarch64")]
        const ARM64_FEATURE_ISA_I8MM = sys::ZX_ARM64_FEATURE_ISA_I8MM;
        #[cfg(target_arch = "aarch64")]
        const ARM64_FEATURE_ISA_SVE = sys::ZX_ARM64_FEATURE_ISA_SVE;
        #[cfg(target_arch = "aarch64")]
        const ARM64_FEATURE_ISA_ARM32 = sys::ZX_ARM64_FEATURE_ISA_ARM32;

        // This is an obsolete name for the same thing.
        #[cfg(target_arch = "aarch64")]
        const ARM64_FEATURE_ISA_SHA2 = sys::ZX_ARM64_FEATURE_ISA_SHA256;

        // Allow the source to set any bits.
        const _ = !0;
    }
}

impl FeatureFlags for CpuFeatureFlags {
    fn kind(_: private::Internal) -> u32 {
        sys::ZX_FEATURE_KIND_CPU
    }
}

bitflags! {
    #[repr(transparent)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct AddressTaggingFeatureFlags: u32 {
        // arm64 address-tagging features
        #[cfg(target_arch = "aarch64")]
        const ARM64_FEATURE_ADDRESS_TAGGING_TBI = sys::ZX_ARM64_FEATURE_ADDRESS_TAGGING_TBI;

        // Allow the source to set any bits.
        const _ = !0;
    }
}

impl FeatureFlags for AddressTaggingFeatureFlags {
    fn kind(_: private::Internal) -> u32 {
        sys::ZX_FEATURE_KIND_ADDRESS_TAGGING
    }
}

/// Get supported hardware capabilities bitflags.
///
/// Wraps the
/// [zx_system_get_features](https://fuchsia.dev/fuchsia-src/reference/syscalls/system_get_features)
/// syscall.
pub fn system_get_feature_flags<F: FeatureFlags>() -> Result<F, Status>
where
    F: bitflags::Flags<Bits = u32>,
{
    let mut raw_features: u32 = 0;
    let access = private::Internal;
    Status::ok(unsafe {
        sys::zx_system_get_features(F::kind(access), &mut raw_features as *mut u32)
    })
    .and_then(|_status| Ok(F::from_bits_retain(raw_features)))
}

/// Get supported hardware capabilities counts.
///
/// Wraps the
/// [zx_system_get_features](https://fuchsia.dev/fuchsia-src/reference/syscalls/system_get_features)
/// syscall.
pub fn system_get_feature_count(kind: FeatureKind) -> Result<u32, Status> {
    let mut raw_features: u32 = 0;
    Status::ok(unsafe { sys::zx_system_get_features(kind as u32, &mut raw_features as *mut u32) })
        .and_then(|_status| Ok(raw_features))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_get_features_cpu_kind() {
        // Clear extra CPU feature flags
        let result = system_get_feature_flags()
            .map(|flags: CpuFeatureFlags| flags & CpuFeatureFlags::HAS_CPU_FEATURES);

        // Only aarch64 should return Ok.
        #[cfg(target_arch = "aarch64")]
        assert_eq!(result, Ok(CpuFeatureFlags::HAS_CPU_FEATURES));

        #[cfg(not(target_arch = "aarch64"))]
        assert_eq!(result, Err(Status::NOT_SUPPORTED));
    }

    #[test]
    fn system_get_features_other_kinds() {
        assert!(system_get_feature_flags::<VirtualMemoryFeatureFlags>().is_ok());
        assert!(system_get_feature_flags::<AddressTaggingFeatureFlags>().is_ok());
        assert!(system_get_feature_count(FeatureKind::HardwareBreakpointCount).is_ok());
        assert!(system_get_feature_count(FeatureKind::HardwareWatchpointCount).is_ok());
    }
}

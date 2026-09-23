// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! RISC-V 64 CPU feature detection and configuration.

use super::arch::{
    ArchPhysHandoff, RISCV64_CSR_SSTATUS, RISCV64_CSR_SSTATUS_VS_INITIAL, RISCV64_CSR_VLENB,
    riscv64_csr_read, riscv64_csr_set, riscv64_csr_write,
};
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use debug::dprintf;
use libarch::riscv64::{RiscvFeature, RiscvFeatures};

unsafe extern "C" {
    fn cpp_riscv64_handoff_cpu_feature_bits(handoff: *const ArchPhysHandoff) -> u64;
    fn cpp_riscv64_handoff_cbom_size(handoff: *const ArchPhysHandoff) -> u16;
    fn cpp_riscv64_handoff_cboz_size(handoff: *const ArchPhysHandoff) -> u16;
}

// Global detected feature state.
static RISCV_FEATURES_BITS: AtomicU64 = AtomicU64::new(0);
static RISCV_CBOM_SIZE: AtomicU32 = AtomicU32::new(64);
static RISCV_CBOZ_SIZE: AtomicU32 = AtomicU32::new(64);
static RISCV_VLENB: AtomicU64 = AtomicU64::new(0);

/// Check if a feature is supported on the current system.
#[inline(always)]
fn has_feature(feature: RiscvFeature) -> bool {
    let bits = RISCV_FEATURES_BITS.load(Ordering::Relaxed);
    (bits & (1 << (feature as u32))) != 0
}

/// Check if the Vector (V) extension is supported and enabled.
#[inline(always)]
pub fn has_vector() -> bool {
    has_feature(RiscvFeature::Vector)
}

/// Check if the Cache-Block Management (Zicbom) extension is supported.
#[inline(always)]
pub fn has_zicbom() -> bool {
    has_feature(RiscvFeature::Zicbom)
}

/// Check if the Cache-Block Zero (Zicboz) extension is supported.
#[inline(always)]
pub fn has_zicboz() -> bool {
    has_feature(RiscvFeature::Zicboz)
}

/// Check if the Supervisor Timer (Sstc) extension is supported.
#[inline(always)]
pub fn has_sstc() -> bool {
    has_feature(RiscvFeature::Sstc)
}

/// Check if Counter CSRs (Zicntr) are supported.
#[inline(always)]
pub fn has_zicntr() -> bool {
    has_feature(RiscvFeature::Zicntr)
}

/// Check if Page-Based Memory Types (Svpbmt) are supported.
#[inline(always)]
pub fn has_svpbmt() -> bool {
    has_feature(RiscvFeature::Svpbmt)
}

/// Get the detected CBOM size in bytes.
#[inline(always)]
pub fn cbom_size() -> u32 {
    RISCV_CBOM_SIZE.load(Ordering::Relaxed)
}

/// Get the detected CBOZ size in bytes.
#[inline(always)]
pub fn cboz_size() -> u32 {
    RISCV_CBOZ_SIZE.load(Ordering::Relaxed)
}

/// Get the vector register length in bytes (`vlenb`).
#[inline(always)]
pub fn vlenb() -> u64 {
    RISCV_VLENB.load(Ordering::Relaxed)
}

/// Early feature initialization called before MMU/heap.
///
/// # Safety
/// Caller guarantees `handoff` points to the C++ `ArchPhysHandoff` from physboot.
pub unsafe fn riscv64_feature_early_init(handoff: *const ArchPhysHandoff) {
    // SAFETY: The caller guarantees `handoff`; these accessors only read from it.
    let (bits, handoff_cbom, handoff_cboz) = unsafe {
        (
            cpp_riscv64_handoff_cpu_feature_bits(handoff),
            cpp_riscv64_handoff_cbom_size(handoff),
            cpp_riscv64_handoff_cboz_size(handoff),
        )
    };
    let mut features = RiscvFeatures::from_bits(bits);

    let cbom = if handoff_cbom == 0 { 64 } else { handoff_cbom as u32 };
    let cboz = if handoff_cboz == 0 { 64 } else { handoff_cboz as u32 };
    RISCV_CBOM_SIZE.store(cbom, Ordering::Relaxed);
    RISCV_CBOZ_SIZE.store(cboz, Ordering::Relaxed);

    if features.supports(RiscvFeature::Vector) {
        // We need vectors to have been enabled in order to read vlenb, but cannot
        // assume that they have been enabled at this point.  Here is as good a place
        // as any to initially turn them on.
        // SAFETY: Vectors must be enabled in sstatus in order to read vlenb CSR.
        unsafe {
            let sstatus_initial = riscv64_csr_read::<RISCV64_CSR_SSTATUS>();
            riscv64_csr_set::<RISCV64_CSR_SSTATUS>(RISCV64_CSR_SSTATUS_VS_INITIAL);
            let vlenb_val = riscv64_csr_read::<RISCV64_CSR_VLENB>();
            RISCV_VLENB.store(vlenb_val, Ordering::Relaxed);

            // Current support is provisional and only for 16-byte vector registers.
            if vlenb_val != 16 {
                riscv64_csr_write::<RISCV64_CSR_SSTATUS>(sstatus_initial);
                features.set(RiscvFeature::Vector, false);
            }
        }
    }

    RISCV_FEATURES_BITS.store(features.bits(), Ordering::Relaxed);
}

/// Feature initialization and informational logging.
pub fn riscv64_feature_init() {
    if has_zicntr() {
        dprintf!(INFO, "RISCV: feature zicntr\n");
    }
    if has_zicbom() {
        let size = cbom_size();
        dprintf!(INFO, "RISCV: feature cbom, size {:#x}\n", size);
        // Make sure the detected cbom size is usable.
        debug_assert!(size > 0 && size.is_power_of_two());
    } else {
        RISCV_CBOM_SIZE.store(0, Ordering::Relaxed);
    }
    if has_zicboz() {
        let size = cboz_size();
        dprintf!(INFO, "RISCV: feature cboz, size {:#x}\n", size);
        // Make sure the detected cboz size is usable.
        debug_assert!(size > 0 && size.is_power_of_two() && size < 4096);
    } else {
        RISCV_CBOZ_SIZE.store(0, Ordering::Relaxed);
    }
    if has_sstc() {
        dprintf!(INFO, "RISCV: feature sstc\n");
    }
    if has_svpbmt() {
        dprintf!(INFO, "RISCV: feature svpbmt\n");
    }
    let vlen = vlenb();
    if has_vector() {
        dprintf!(INFO, "RISCV: feature vector, register length = {:#x}\n", vlen);
    } else if vlen > 0 {
        dprintf!(INFO, "RISCV: feature vector disabled; register length ({:#x}) too large\n", vlen);
    }
}

// C FFI exports for C++ callers

#[unsafe(no_mangle)]
pub extern "C" fn rust_riscv64_feature_has_vector() -> bool {
    has_vector()
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_riscv64_feature_has_zicbom() -> bool {
    has_zicbom()
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_riscv64_feature_has_zicboz() -> bool {
    has_zicboz()
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_riscv64_feature_has_svpbmt() -> bool {
    has_svpbmt()
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_riscv64_feature_has_zicntr() -> bool {
    has_zicntr()
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_riscv64_feature_has_sstc() -> bool {
    has_sstc()
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_riscv64_feature_cbom_size() -> u32 {
    cbom_size()
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_riscv64_feature_cboz_size() -> u32 {
    cboz_size()
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_riscv64_feature_vlenb() -> u64 {
    vlenb()
}

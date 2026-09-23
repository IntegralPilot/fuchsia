// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use magma::MAGMA_QUERY_VENDOR_PARAM_0;
use zerocopy::{FromBytes, Immutable, IntoBytes};

pub const MAGMA_QCOM_ADRENO_QUERY_KGSL_PARAMS: u64 = MAGMA_QUERY_VENDOR_PARAM_0 + 3000;

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, Immutable, Default)]
#[repr(packed)]
pub struct AdrenoKgslParams {
    pub device_id: u32,
    pub chip_id: u32,
    pub gpu_id: u32,
    pub device_shadow_size: u64,
    pub device_shadow_flags: u32,
    pub mmu_enabled: u32,
    pub gmem_sizebytes: u64,
    pub highest_bank_bit: u32,
    pub device_bitness: u32,
    pub ucode_version_pfp: u32,
    pub ucode_version_pm4: u32,
    pub min_access_length: u32,
    pub ubwc_mode: u32,
    pub secure_ctxt_support: u32,
    pub secure_buf_alignment: u32,
    pub gpu_secure_va_size: u64,
    pub gpu_va64_size: u64,
    pub gpu_model: [u8; 32],
    pub vk_device_id: u32,
}

// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod arguments;
pub mod cpu_resource;
pub mod debug_resource;
pub mod debuglog_resource;
pub mod energy_info_resource;
pub mod factory_items;
pub mod hypervisor_resource;
pub mod info_resource;
pub mod iommu_resource;
#[cfg(target_arch = "x86_64")]
pub mod ioport_resource;
pub mod irq_resource;
pub mod items;
pub mod kernel_stats;
pub mod mexec_resource;
pub mod mmio_resource;
pub mod msi_resource;
pub mod power_resource;
pub mod profile_resource;
pub mod root_job;
pub mod sampling_resource;
#[cfg(target_arch = "aarch64")]
pub mod smc_resource;
pub mod stall_resource;
pub mod tracing_resource;
pub mod vmex_resource;

// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::utils;
use bitfield::bitfield;
use mmio::{Mmio, ReadableRegister, register};

register! {
    #[register(offset = 0x0, mode = RO)]
    pub struct GpuId(u32);

    #[register(offset = 0x24, mode = RW)]
    pub struct GpuIrqClear(u32);

    #[register(offset = 0x28, mode = RW)]
    pub struct GpuIrqMask(u32) {
        pub gpu_fault, set_gpu_fault: 0, 0;
        pub multiple_gpu_faults, set_multiple_gpu_faults: 7, 7;
        pub reset_completed, set_reset_completed: 8, 8;
        pub power_changed_single, set_power_changed_single: 9, 9;
        pub power_changed_all, set_power_changed_all: 10, 10;
        pub performace_counter_sample_completed, set_performance_counter_sample_completed: 16, 16;
        pub clean_caches_completed, set_clean_caches_completed: 17, 17;
        pub doorbell_mirror, set_doorbell_mirror: 18, 18;
        pub mcu_status, set_mcu_status: 19, 19;
    }

    #[register(offset = 0x2c, mode = RO)]
    pub struct GpuIrqStatus(u32);

    #[register(offset = 0x30, mode = WO)]
    pub struct GpuCommand(u32);

    #[register(offset = 0x48, mode = WO)]
    pub struct L2Config(u32) {
        pub cache_size, set_cache_size: 23, 16;
        pub hash_enable, set_hash_enable: 24, 24;
        pub hash, set_hash: 31, 24;
    }

    #[register(offset = 0x3c, mode = RO)]
    pub struct GpuFaultStatus(u32);

    #[register(offset = 0x40, mode = RO)]
    pub struct GpuFaultAddress(u32);

    #[register(offset = 0x100, mode = RO)]
    pub struct GpuShaderPresentLow(u32);

    #[register(offset = 0x160, mode = RO)]
    pub struct L2Ready(u32) {
        pub enabled, set_enabled: 1, 0;
    }

    #[register(offset = 0x1a0, mode = WO)]
    pub struct L2Power(u32);

    #[register(offset = 0x2c0, mode = WO)]
    pub struct AddressSpaceHash0(u32);

    #[register(offset = 0x2c4, mode = WO)]
    pub struct AddressSpaceHash1(u32);

    #[register(offset = 0x2c8, mode = WO)]
    pub struct AddressSpaceHash2(u32);

    #[register(offset = 0x300, mode = RO)]
    pub struct CoherencyFeatures(u32) {
        // The GPU can snoop on CPU caches.
        pub ace_lite, set_ace_lite: 0, 0;
        // Both GPU and CPU can snoop on each other's caches.
        pub ace, set_ace: 1, 1;
        pub none, set_none: 31, 31;
    }

    #[register(offset = 0x304, mode = RW)]
    pub struct CoherencyEnable(u32);

    #[register(offset = 0x700, mode = RW)]
    pub struct McuControl(u32) {
        pub field, set_field: 1, 0;
    }

    #[register(offset = 0x704, mode = RW)]
    pub struct McuStatus(u32) {
        pub value, set_value: 3, 0;
    }

    #[register(offset = 0xf00, mode = WO)]
    pub struct CsfConfig(u32);

    #[register(offset = 0xf04, mode = WO)]
    pub struct ShaderConfig(u32);

    #[register(offset = 0xf08, mode = WO)]
    pub struct TilerConfig(u32);

    #[register(offset = 0xf0c, mode = WO)]
    pub struct L2MmuConfig(u32);

    #[register(offset = 0x1004, mode = WO)]
    pub struct JobIrqClear(u32);

    #[register(offset = 0x1008, mode = RW)]
    pub struct JobIrqMask(u32);

    #[register(offset = 0x100c, mode = RO)]
    pub struct JobIrqStatus(u32) {
        pub global_interface_ready, set_global_interface_ready: 31, 31;
    }

    #[register(offset = 0x2000, mode = RW)]
    pub struct MmuIrqRawStatus(u32) {
        pub address_space, set_address_space: 15, 0;
    }

    #[register(offset = 0x2004, mode = RW)]
    pub struct MmuIrqClear(u32);

    #[register(offset = 0x2008, mode = RW)]
    pub struct MmuIrqMask(u32);

    #[register(offset = 0x200c, mode = RW)]
    pub struct MmuIrqStatus(u32) {
        pub address_space, set_address_space: 15, 0;
    }
}

impl GpuIrqMask {
    pub fn to_clear(&self) -> GpuIrqClear {
        GpuIrqClear(self.0)
    }

    pub fn read_status(mmio: &impl Mmio) -> Self {
        Self(GpuIrqStatus::read(mmio).0)
    }
}

impl GpuCommand {
    pub fn soft_reset() -> Self {
        Self(1 | (1 << 8))
    }
}

impl L2Power {
    pub fn enable() -> Self {
        Self(1)
    }
}

impl McuControl {
    pub fn auto() -> Self {
        McuControl(0x2)
    }
}

const MMU_BASE_OFFSET: u64 = 0x2400;
const MMU_ADDRESS_SPACE_SHIFT: u64 = 6;

const MMU_TRANSLATION_TABLE_LOW_OFFSET: u64 = 0x0;
const MMU_MEMORY_ATTRIBUTE_LOW_OFFSET: u64 = 0x8;

const MMU_COMMAND_OFFSET: u64 = 0x18;
const MMU_FAULT_STATUS_OFFSET: u64 = 0x1C;
const MMU_FAULT_ADDRESS_LOW_OFFSET: u64 = 0x20;
const MMU_STATUS_OFFSET: u64 = 0x28;
const MMU_TRANSLATION_CONFIG_LOW_OFFSET: u64 = 0x30;

#[allow(unused)]
pub enum MmuCommand {
    Update = 1,
    FlushPageTables = 4,
    FlushMemory = 5,
}

bitfield! {
    pub struct AddressSpaceStatus(u32);
    impl Debug;
    pub active, set_active: 0, 0;
}

bitfield! {
    pub struct AddressSpaceFault(u32);
    impl Debug;
    pub execption_type, set_exception_type: 7, 0;
    pub access_type, set_access_type: 10, 8;
    pub source_id, set_source_id: 31, 16;
}

// MemoryAttribute is a u64 broken up into 8 attributes. We define one byte here
// and use bit manipulation to fill in the rest.
bitfield! {
    pub struct MemoryAttribute(u64);
    impl Debug;
    pub write_allocate, set_write_allocate: 0, 0;
    pub read_allocate, set_read_allocate: 1, 1;
    pub allocate_mode, set_allocate_mode: 3, 2;
    pub coherency, set_coherency: 5, 4;
    pub memory_type, set_memory_type: 7, 6;
}

const MEMORY_ATTRIBUTE_ALLOCATE_MODE_IMPLEMENTATION: u64 = 0x2;
// In this mode allocation policy is determined by `write_allocate` `read_allocate` fields.
const MEMORY_ATTRIBUTE_ALLOCATE_MODE_ALLOC: u64 = 0x3;

const MEMORY_ATTRIBUTE_MEMORY_TYPE_SHARED: u64 = 0x0;
const MEMORY_ATTRIBUTE_MEMORY_TYPE_NON_CACHEABLE: u64 = 0x1;
const MEMORY_ATTRIBUTE_MEMORY_TYPE_WRITE_BACK: u64 = 0x2;
#[allow(unused)]
const MEMORY_ATTRIBUTE_MEMORY_TYPE_FAULT: u64 = 0x3;

#[allow(unused)]
#[repr(u32)]
pub enum MemoryAttributeSlot {
    ImplementationDefinedCachePolicy = 0,
    ForceCacheAll = 1,
    InnerWriteAlloc = 2,
    ImplementationDefinedOuterCaching = 3,
    WritebackOuterCaching = 4,
    NonCacheable = 5,
    Shared = 6,
    Blank = 7,
}

impl MemoryAttribute {
    #[allow(unused)]
    pub fn from_u64(val: u64) -> [MemoryAttribute; 8] {
        std::array::from_fn(|i| MemoryAttribute((val >> (8 * i)) & 0xFF))
    }

    pub fn from_attribute_array(array: [MemoryAttribute; 8]) -> MemoryAttribute {
        let mut val = 0;
        for i in 0..8 {
            val |= (array[i].0 & 0xFF) << (i * 8);
        }
        MemoryAttribute(val)
    }

    pub fn default() -> MemoryAttribute {
        // Use GPU defined caching policy
        let mut implementation_defined_cache_policy = MemoryAttribute(0);
        implementation_defined_cache_policy
            .set_allocate_mode(MEMORY_ATTRIBUTE_ALLOCATE_MODE_IMPLEMENTATION);
        implementation_defined_cache_policy
            .set_memory_type(MEMORY_ATTRIBUTE_MEMORY_TYPE_WRITE_BACK);

        // Force all resources to be cached.
        let mut force_cache_all = MemoryAttribute(0);
        force_cache_all.set_write_allocate(1);
        force_cache_all.set_read_allocate(1);
        force_cache_all.set_allocate_mode(MEMORY_ATTRIBUTE_ALLOCATE_MODE_ALLOC);
        force_cache_all.set_memory_type(MEMORY_ATTRIBUTE_MEMORY_TYPE_WRITE_BACK);

        // Inner write-alloc caching, no outer caching.
        let mut inner_write_alloc = MemoryAttribute(0);
        inner_write_alloc.set_write_allocate(1);
        inner_write_alloc.set_allocate_mode(MEMORY_ATTRIBUTE_ALLOCATE_MODE_ALLOC);
        inner_write_alloc.set_memory_type(MEMORY_ATTRIBUTE_MEMORY_TYPE_WRITE_BACK);

        let mut implementation_defined_outer_caching = MemoryAttribute(0);
        implementation_defined_outer_caching
            .set_allocate_mode(MEMORY_ATTRIBUTE_ALLOCATE_MODE_IMPLEMENTATION);
        implementation_defined_outer_caching
            .set_memory_type(MEMORY_ATTRIBUTE_MEMORY_TYPE_WRITE_BACK);

        let mut write_back_outer_caching = MemoryAttribute(0);
        write_back_outer_caching.set_write_allocate(1);
        write_back_outer_caching.set_allocate_mode(MEMORY_ATTRIBUTE_ALLOCATE_MODE_ALLOC);
        write_back_outer_caching.set_memory_type(MEMORY_ATTRIBUTE_MEMORY_TYPE_WRITE_BACK);

        let mut non_cacheable = MemoryAttribute(0);
        non_cacheable.set_allocate_mode(MEMORY_ATTRIBUTE_ALLOCATE_MODE_ALLOC);
        non_cacheable.set_memory_type(MEMORY_ATTRIBUTE_MEMORY_TYPE_NON_CACHEABLE);

        let mut shared = MemoryAttribute(0);
        shared.set_allocate_mode(MEMORY_ATTRIBUTE_ALLOCATE_MODE_IMPLEMENTATION);
        shared.set_memory_type(MEMORY_ATTRIBUTE_MEMORY_TYPE_SHARED);

        let blank = MemoryAttribute(0);

        Self::from_attribute_array([
            implementation_defined_cache_policy,
            force_cache_all,
            inner_write_alloc,
            implementation_defined_outer_caching,
            write_back_outer_caching,
            non_cacheable,
            shared,
            blank,
        ])
    }
}

pub struct AddressSpaceRegs {
    number: u64,
}

impl AddressSpaceRegs {
    pub fn new(number: u64) -> Self {
        Self { number }
    }

    fn offset_to_address(&self, offset: u64) -> usize {
        let address = MMU_BASE_OFFSET + (self.number << MMU_ADDRESS_SPACE_SHIFT);
        (address + offset) as usize
    }

    fn read_u64(&self, mmio: &impl Mmio, low_offset: u64) -> u64 {
        (mmio.load32(self.offset_to_address(low_offset)) as u64)
            | utils::upper_u32_to_u64(mmio.load32(self.offset_to_address(low_offset + 0x4)))
    }

    fn write_u64(&self, mmio: &mut impl Mmio, low_offset: u64, data: u64) {
        mmio.store32(self.offset_to_address(low_offset), utils::lower_u32(data));
        mmio.store32(self.offset_to_address(low_offset + 0x4), utils::upper_u32(data));
    }

    pub fn read_status(&self, mmio: &impl Mmio) -> AddressSpaceStatus {
        AddressSpaceStatus(mmio.load32(self.offset_to_address(MMU_STATUS_OFFSET)))
    }

    pub fn read_fault_status(&self, mmio: &impl Mmio) -> AddressSpaceFault {
        AddressSpaceFault(mmio.load32(self.offset_to_address(MMU_FAULT_STATUS_OFFSET)))
    }

    pub fn read_fault_address(&self, mmio: &impl Mmio) -> u64 {
        self.read_u64(mmio, MMU_FAULT_ADDRESS_LOW_OFFSET)
    }

    pub fn write_command(&self, mmio: &mut impl Mmio, command: MmuCommand) {
        mmio.store32(self.offset_to_address(MMU_COMMAND_OFFSET), command as u32);
    }

    pub fn write_translation_table(&self, mmio: &mut impl Mmio, transtab: u64) {
        self.write_u64(mmio, MMU_TRANSLATION_TABLE_LOW_OFFSET, transtab);
    }

    pub fn write_translation_config(&self, mmio: &mut impl Mmio, transcfg: u64) {
        self.write_u64(mmio, MMU_TRANSLATION_CONFIG_LOW_OFFSET, transcfg);
    }

    pub fn write_memory_attribute(&self, mmio: &mut impl Mmio, memory_attribute: MemoryAttribute) {
        self.write_u64(mmio, MMU_MEMORY_ATTRIBUTE_LOW_OFFSET, memory_attribute.0);
    }
}

pub const GLOBAL_DOORBELL_ID: u64 = 0;
pub struct Doorbell {
    id: u64,
}

impl Doorbell {
    pub fn new(id: u64) -> Self {
        Self { id }
    }

    fn number_to_offset(&self) -> usize {
        0x80000 + (self.id as usize * 0x10000)
    }

    pub fn ring(&self, mmio: &mut impl Mmio) {
        mmio.store32(self.number_to_offset(), 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    fn test_memory_attribute_default() {
        assert_eq!(MemoryAttribute::default().0, 0x84c8d888d8f88u64);
    }
}

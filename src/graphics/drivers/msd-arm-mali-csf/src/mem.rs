// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::utils;
use crate::utils::LogError;

/// A GPU address is a virtual address used by a GPU program, like a shader.
#[derive(Debug, Clone, Copy, Ord, PartialOrd, Eq, PartialEq)]
pub struct GpuAddress(pub u64);
impl GpuAddress {
    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

/// A physical address is one given to the GPU to back virtual memory.
#[derive(Debug, Clone, Copy)]
pub struct PhysicalAddress(pub u64);
impl PhysicalAddress {
    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

/// A CPU address is one that the CPU can read and write to.
#[derive(Debug, Clone, Copy)]
pub struct CpuAddress(pub u64);
impl CpuAddress {
    pub fn as_u64(&self) -> u64 {
        self.0
    }

    pub fn as_address(&self) -> *const () {
        self.0 as *const ()
    }

    pub fn as_mut_address(&mut self) -> *mut () {
        self.0 as *mut ()
    }

    pub fn offset(&self, offset: u64) -> Self {
        CpuAddress(self.0 + offset)
    }
}

// TODO(https://fxbug.dev/492132218) Share this code with adreno.
pub struct Buffer {
    pub vmo: zx::Vmo,
    size: usize,
}

impl Buffer {
    pub fn new(size: usize, cache_policy: zx::CachePolicy) -> Result<Self, zx::Status> {
        debug_assert!(size % utils::PAGE_SIZE == 0, "Buffer must be page multiple: {:x}", size);
        let vmo = zx::Vmo::create(size as u64)?;
        vmo.set_cache_policy(cache_policy)?;
        Ok(Buffer { vmo, size })
    }

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn map(
        &self,
        offset: usize,
        size: usize,
        rights: zx::VmarFlags,
    ) -> Result<MappedMemory, zx::Status> {
        MappedMemory::new(&self.vmo, offset, size, rights)
    }

    pub fn pin(
        &self,
        mapper: &dyn BusMapper,
        offset: usize,
        size: usize,
        options: zx::BtiOptions,
    ) -> Result<PinnedMemory, zx::Status> {
        debug_assert!(offset % utils::PAGE_SIZE == 0, "Offset must be page multiple: {:x}", offset);
        debug_assert!(size % utils::PAGE_SIZE == 0, "Size must be page multiple: {:x}", size);
        mapper.map(&self.vmo, offset / utils::PAGE_SIZE, size / utils::PAGE_SIZE, options)
    }
}

/// Mapped memory comes from a buffer, and can be read and written to by the CPU.
/// This will automatically unmap the memory when it is dropped.
pub struct MappedMemory {
    pub cpu_address: CpuAddress,
    pub size: usize,
}

impl Drop for MappedMemory {
    fn drop(&mut self) {
        let unmap_result = unsafe {
            fuchsia_runtime::vmar_root_self().unmap(self.cpu_address.as_u64() as usize, self.size)
        };
        utils::debug_assert_ok!(unmap_result);
    }
}

impl MappedMemory {
    pub fn new(
        vmo: &zx::Vmo,
        offset: usize,
        size: usize,
        rights: zx::VmarFlags,
    ) -> Result<MappedMemory, zx::Status> {
        let vmar_offset = 0;
        let cpu_address = CpuAddress(
            fuchsia_runtime::vmar_root_self()
                .map(vmar_offset, vmo, offset.try_into().unwrap(), size.try_into().unwrap(), rights)
                .map_err(|_| zx::Status::INVALID_ARGS)? as u64,
        );

        Ok(MappedMemory { cpu_address, size })
    }

    pub fn as_u8(&self) -> &[u8] {
        // SAFETY: We know this is readable memory, and the borrow checker ties the lifetime to us.
        unsafe { core::slice::from_raw_parts(self.cpu_address.0 as *const u8, self.size) }
    }

    pub fn read32(&self, offset: usize) -> u32 {
        assert!(offset < self.size, "Read out of bounds! Offset {}, size {}", offset, self.size);
        let ptr = self.cpu_address.0 as *const u32;
        // SAFETY: We are guaranteed that cpu_addr is valid and we bounds checked.
        unsafe { ptr.add(offset / std::mem::size_of::<u32>()).read_volatile() }
    }

    pub fn write32(&mut self, offset: usize, val: u32) {
        assert!(offset < self.size, "Write out of bounds! Offset {}, size {}", offset, self.size);
        let ptr = self.cpu_address.as_mut_address() as *mut u32;
        // SAFETY: We are guaranteed that cpu_addr is valid and we bounds checked.
        unsafe { ptr.add(offset / std::mem::size_of::<u32>()).write_volatile(val) }
        // TODO(https://fxbug.dev/503722844): Remove this flush.
        self.flush_cache_bytes(offset, std::mem::size_of::<u32>());
    }

    pub fn write_bytes(&mut self, index: usize, bytes: &[u8]) {
        debug_assert!(
            (index + bytes.len()) <= self.size,
            "Writing too many bytes {} > {}",
            bytes.len(),
            self.size
        );
        // SAFETY: We know the memory exists and is writable.
        unsafe {
            for i in 0..bytes.len() {
                (self.cpu_address.as_mut_address() as *mut u8)
                    .add(index + i)
                    .write_volatile(bytes[i]);
            }
        }
        // TODO(https://fxbug.dev/503722844): Remove this flush.
        self.flush_cache_bytes(index, bytes.len());
    }

    pub fn flush_cache_bytes(&self, offset: usize, size: usize) {
        unsafe {
            let zx_status = zx::sys::zx_cache_flush(
                (self.cpu_address.0 as *const u8).add(offset),
                size,
                zx::sys::ZX_CACHE_FLUSH_DATA,
            );
            debug_assert_eq!(zx_status, zx::sys::ZX_OK);
        }
    }

    pub fn flush_cache(&self) {
        self.flush_cache_bytes(0, self.size)
    }
}

// A bus mapper takes a VMO and pins it to physical memory.
// The pinned memory it returns has "bus addresses" which are GPU physical addresses.
pub trait BusMapper: Send {
    fn map(
        &self,
        vmo: &zx::Vmo,
        start_page_index: usize,
        page_count: usize,
        options: zx::BtiOptions,
    ) -> Result<PinnedMemory, zx::Status>;
}

impl BusMapper for zx::Bti {
    fn map(
        &self,
        vmo: &zx::Vmo,
        start_page_index: usize,
        page_count: usize,
        options: zx::BtiOptions,
    ) -> Result<PinnedMemory, zx::Status> {
        let mut bus_addrs: Vec<zx::sys::zx_paddr_t> = vec![0; page_count];
        let offset = start_page_index * utils::PAGE_SIZE;
        let size = page_count * utils::PAGE_SIZE;

        let pmt = self
            .pin(options, vmo, offset as u64, size as u64, &mut bus_addrs)
            .log_err("Failed to pin BTI")?;

        let bus_addrs = bus_addrs.iter().map(|&addr| PhysicalAddress(addr as u64)).collect();
        Ok(PinnedMemory { pmt, bus_addrs })
    }
}

// This is a bus mapper for when fuchsia is running in a virtual machine.
// We first use the BTI to pin the memory in the guest, so it cannot be reclaimed.
// We then make a hyper call out to the host, so the host pins the memory as well,
// and it returns the Host Physical Address, which is what the GPU expects.
pub struct CrosVmMapper {
    bti: zx::Bti,
    smc: zx::NullableHandle,
}

impl CrosVmMapper {
    pub fn new(bti: zx::Bti, smc: zx::NullableHandle) -> Self {
        Self { bti, smc }
    }

    // This takes our Guest Physical Address and tells the host we are using it
    // for hardware. The host will pin the memory and return a Host Physical Address,
    // which is what the GPU expects.
    fn pin_to_hypervisor(&self, guest_physical_address: PhysicalAddress) -> PhysicalAddress {
        let mut parameters = zx::sys::zx_smc_parameters_t::default();
        // This is our user defined hypercall.
        parameters.func_id = 0x86000080;
        parameters.arg1 = guest_physical_address.0;

        // SAFETY: We are calling the syscall correctly.
        let new_addr = unsafe {
            let mut result: zx::sys::zx_smc_result_t = std::mem::zeroed();
            let status = zx::sys::zx_smc_call(self.smc.raw_handle(), &parameters, &mut result);
            assert_eq!(status, zx::sys::ZX_OK);
            result.arg0
        };
        PhysicalAddress(new_addr)
    }
}

impl BusMapper for CrosVmMapper {
    fn map(
        &self,
        vmo: &zx::Vmo,
        start_page_index: usize,
        page_count: usize,
        options: zx::BtiOptions,
    ) -> Result<PinnedMemory, zx::Status> {
        let mut pinned = self.bti.map(vmo, start_page_index, page_count, options)?;
        pinned.bus_addrs = std::mem::take(&mut pinned.bus_addrs)
            .into_iter()
            .map(|addr| self.pin_to_hypervisor(addr))
            .collect();
        Ok(pinned)
    }
}

/// This represents memory pinned to physical addresses.
pub struct PinnedMemory {
    pmt: zx::Pmt,
    pub bus_addrs: Vec<PhysicalAddress>,
}

impl PinnedMemory {
    #[cfg(test)]
    pub fn new_for_test(bus_addrs: Vec<PhysicalAddress>) -> Self {
        Self { pmt: zx::NullableHandle::invalid().into(), bus_addrs }
    }

    pub fn size(&self) -> usize {
        self.bus_addrs.len() * utils::PAGE_SIZE
    }
}

impl Drop for PinnedMemory {
    fn drop(&mut self) {
        if !self.pmt.is_invalid() {
            log::error!("Leaking pinned memory: GPU addresses {:#?}", self.bus_addrs);
            // TODO: Add an assert here after we finish bringup and are confident we won't leak.
        }
    }
}

impl PinnedMemory {
    // NOTE: This needs to be called before this struct is dropped otherwise this will leak.
    // NOTE: This should only be called when we are sure the hardware is done accessing this
    // memory.
    pub fn unpin(mut self) {
        let pmt = std::mem::replace(&mut self.pmt, zx::NullableHandle::invalid().into());
        let result = unsafe { pmt.unpin() };
        utils::debug_assert_ok!(result);
    }
}

pub fn allocate_map_pin(
    size: usize,
    bus_mapper: &dyn BusMapper,
) -> Result<(Buffer, MappedMemory, PinnedMemory), zx::Status> {
    let buffer = Buffer::new(size, zx::CachePolicy::UnCached)?;
    let mapping = buffer.map(0, size, zx::VmarFlags::PERM_WRITE | zx::VmarFlags::PERM_READ)?;
    let pin =
        buffer.pin(bus_mapper, 0, size, zx::BtiOptions::PERM_READ | zx::BtiOptions::PERM_WRITE)?;
    Ok((buffer, mapping, pin))
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::cell::RefCell;

    pub struct FakeBusMapper {
        next_address: RefCell<u64>,
    }

    impl FakeBusMapper {
        pub fn new(starting_address: u64) -> Self {
            Self { next_address: RefCell::new(starting_address) }
        }
    }

    impl BusMapper for FakeBusMapper {
        fn map(
            &self,
            _vmo: &zx::Vmo,
            _start_page_index: usize,
            page_count: usize,
            _options: zx::BtiOptions,
        ) -> Result<PinnedMemory, zx::Status> {
            let addresses = (0..page_count)
                .map(|_| {
                    let next: u64 = *self.next_address.borrow();
                    *self.next_address.borrow_mut() += 0x1000;
                    PhysicalAddress(next)
                })
                .collect();
            Ok(PinnedMemory::new_for_test(addresses))
        }
    }

    #[fuchsia::test]
    fn map_fake() {
        const STARTING_ADDRESS: u64 = 0x1000;
        let bus_mapper = FakeBusMapper::new(STARTING_ADDRESS);

        const PAGE_COUNT: usize = 4;
        let vmo =
            zx::Vmo::create((PAGE_COUNT * utils::PAGE_SIZE) as u64).expect("VMO create failed");
        let map_result = bus_mapper.map(&vmo, 0, PAGE_COUNT, zx::BtiOptions::PERM_READ);
        let pinned_memory = map_result.unwrap();
        assert_eq!(pinned_memory.bus_addrs.len(), PAGE_COUNT as usize);
        for i in 0..PAGE_COUNT {
            assert_eq!(
                pinned_memory.bus_addrs[i as usize].0,
                STARTING_ADDRESS + (utils::PAGE_SIZE * i) as u64
            );
        }
    }

    #[fuchsia::test]
    fn mapped_read_write() {
        const PAGE_COUNT: usize = 4;
        const PAGE_SIZE: usize = 0x1000;
        const VMO_SIZE: usize = PAGE_COUNT * PAGE_SIZE;
        let buffer = Buffer::new(VMO_SIZE, zx::CachePolicy::Cached).expect("buffer new failed");

        const VMAR_READ_OFFSET: usize = std::mem::size_of::<u32>() * 7;
        let write_data: [u8; 4] = [0xef, 0xbe, 0xad, 0xde];
        buffer.vmo.write(&write_data, VMAR_READ_OFFSET as u64).expect("VMO write failed");

        const VMAR_WRITE_OFFSET: usize = 0x1000 + std::mem::size_of::<u32>() * 10;
        let mut read_data: [u8; 4] = [0, 0, 0, 0];
        buffer.vmo.read(&mut read_data, VMAR_WRITE_OFFSET as u64).expect("VMO read failed");
        assert_eq!(read_data, [0, 0, 0, 0]);

        let mut mapped_memory = buffer
            .map(
                /*vmo_offset=*/ 0,
                VMO_SIZE,
                zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE,
            )
            .expect("map failed");

        for index in 0..VMO_SIZE / std::mem::size_of::<u32>() {
            let offset = index * std::mem::size_of::<u32>();
            match offset {
                VMAR_READ_OFFSET => assert_eq!(mapped_memory.read32(offset), 0xdeadbeef),
                VMAR_WRITE_OFFSET => {
                    assert_eq!(mapped_memory.read32(offset), 0);
                    println!("offset {} vmo_size {}", offset, VMO_SIZE);
                    mapped_memory.write_bytes(offset, &0xabcd1234u32.to_ne_bytes());
                }
                _ => {
                    assert_eq!(mapped_memory.read32(offset), 0);
                }
            }
        }

        buffer.vmo.read(&mut read_data, VMAR_WRITE_OFFSET as u64).expect("VMO read failed");
        assert_eq!(read_data, [0x34, 0x12, 0xcd, 0xab]);
    }
}

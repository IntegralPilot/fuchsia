// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{hardware, mem, regs};
use bitfield::bitfield;
use mmio::Mmio;

const MALI_PAGE_SHIFT: u64 = 12;
const MALI_PAGE_SIZE: u64 = 1 << MALI_PAGE_SHIFT;
const PAGE_TABLE_ENTRIES: u64 = MALI_PAGE_SIZE / (std::mem::size_of::<u64>() as u64);
const PAGE_TABLE_MASK: u64 = PAGE_TABLE_ENTRIES - 1;
const PAGE_OFFSET_BITS: u64 = 9;
const PAGE_DIRECTORY_LEVELS: u32 = 4;
const PAGE_DIRECTORY_MAX_LEVEL: u32 = PAGE_DIRECTORY_LEVELS - 1;

#[repr(u64)]
enum EntryType {
    // Address Translation Entry, this means that this entry is pointing to physical memory.
    // This is used on level 2 of the table for a 2Mb mapping.
    // This is used on level 1 of the table for a 1Gb mapping.
    // This is not used on level 3, please use `Pte` there.
    #[allow(unused)]
    Ate = 1,
    // This entry is marked invalid.
    Invalid = 2,
    // Page Table Entry, this means this entry points to the next level in the table.
    // For a level 3 table, this means the entry points to physical memory.
    Pte = 3,
}

pub enum Coherency {
    #[allow(unused)]
    Coherent,
    NonCoherent,
}
bitfield! {
    pub struct AccessFlags(u64);
    impl Debug;
    pub attribute_slot, set_attribute_slot: 4, 2;
    pub access_flag_read, set_access_flag_read: 6, 6;
    pub access_flag_no_write, set_access_flag_no_write: 7, 7;
    pub access_flag_share, set_access_flag_share: 9, 8;
    pub access_flag_no_exec, set_access_flag_no_exec: 54, 54;
}

bitfield! {
    pub struct MaliPte(u64);
    impl Debug;
    pub entry_type, set_entry_type: 1, 0;
    pub attribute_slot, set_attribute_slot: 4, 2;
    pub access_flag_read, set_access_flag_read: 6, 6;
    pub access_flag_no_write, set_access_flag_no_write: 7, 7;
    pub access_flag_share, set_access_flag_share: 9, 8;
    pub access_bit, set_access_bit: 10, 10;
    pub access_flag_no_exec, set_access_flag_no_exec: 54, 54;
}

impl MaliPte {
    fn new_directory_entry(address: mem::PhysicalAddress) -> Self {
        MaliPte(address.as_u64() | EntryType::Pte as u64)
    }

    fn new_pte_entry(physical_address: mem::PhysicalAddress, flags: &AccessFlags) -> Self {
        let mut pte = MaliPte(flags.0 | physical_address.as_u64());
        // NOTE: At level 3 of the page table the PTE entry type points to physical memory.
        pte.set_entry_type(EntryType::Pte as u64);
        pte.set_access_bit(1);
        pte
    }
}

/// A mapping store holds a tree of GPU virtual addresses and their pinned
/// memory counterparts.
struct MappingStore {
    mappings: std::collections::BTreeMap<mem::GpuAddress, mem::PinnedMemory>,
}

impl MappingStore {
    fn new() -> Self {
        Self { mappings: std::collections::BTreeMap::new() }
    }

    fn insert(&mut self, address: mem::GpuAddress, memory: mem::PinnedMemory) {
        self.mappings.insert(address, memory);
    }

    fn unpin_and_release_all(&mut self) {
        let mappings = std::mem::replace(&mut self.mappings, std::collections::BTreeMap::new());
        for (_, pinned) in mappings.into_iter() {
            pinned.unpin();
        }
    }
}

/// This represents a single GPU virtual address space.
pub struct AddressSpace {
    cache_coherence: Coherency,
    root_page_directory: Box<PageTable>,
    mapping_store: MappingStore,
}

impl AddressSpace {
    pub fn new(
        cache_coherence: Coherency,
        mapper: &dyn mem::BusMapper,
    ) -> Result<Self, zx::Status> {
        let root_page_directory = Box::new(PageTable::new(mapper, &cache_coherence)?);
        Ok(Self { cache_coherence, root_page_directory, mapping_store: MappingStore::new() })
    }

    pub fn translation_table_physical_address(&self) -> u64 {
        self.root_page_directory.page.physical_address().as_u64()
    }

    pub fn bind_to_hardware(
        &self,
        mmio: &mut impl Mmio,
        address_space_number: u64,
    ) -> Result<(), zx::Status> {
        const AS_TRANSCFG_MEMATTR_WRITE_BACK: u64 = 1 << 25;
        const AS_TRANSCFG_READ_ALLOCATE: u64 = 1 << 30;
        const AS_TRANSCFG_MODE_AARCH64_4K: u64 = 1 << 2 | 1 << 1;
        let translation_config = AS_TRANSCFG_MEMATTR_WRITE_BACK
            | AS_TRANSCFG_READ_ALLOCATE
            | AS_TRANSCFG_MODE_AARCH64_4K;

        let memory_attribute = regs::MemoryAttribute::default();

        hardware::enable_address_space(
            mmio,
            address_space_number,
            self.translation_table_physical_address(),
            translation_config,
            memory_attribute,
        )
    }

    #[allow(unused)]
    pub fn unbind_from_hardware_and_erase(
        &mut self,
        mmio: &mut impl Mmio,
        address_space_number: u64,
    ) -> Result<(), zx::Status> {
        hardware::disable_address_space(mmio, address_space_number)?;

        self.root_page_directory.reset(&self.cache_coherence);
        self.mapping_store.unpin_and_release_all();
        Ok(())
    }

    fn insert_pte(
        &mut self,
        gpu_address: mem::GpuAddress,
        physical_address: mem::PhysicalAddress,
        flags: &AccessFlags,
        mapper: &dyn mem::BusMapper,
    ) -> Result<(), zx::Status> {
        let pte = MaliPte::new_pte_entry(physical_address, flags);
        let page_table_level_three = self.root_page_directory.create_to_level(
            gpu_address,
            0,
            PAGE_DIRECTORY_MAX_LEVEL,
            mapper,
            &self.cache_coherence,
        )?;
        let page_index = PageTable::gpu_addr_to_page_index(gpu_address, PAGE_DIRECTORY_MAX_LEVEL);
        page_table_level_three.write_pte(page_index, pte, &self.cache_coherence);
        Ok(())
    }

    // Insert a buffer at a given address space.
    // This will overwrite any mappings at that specific space.
    // TODO(https://fxbug.dev/488424335): Update this function to check if the address is already mapped.
    pub fn insert_buffer(
        &mut self,
        start_addr: mem::GpuAddress,
        pinned_memory: mem::PinnedMemory,
        flags: &AccessFlags,
        mapper: &dyn mem::BusMapper,
    ) -> Result<(), zx::Status> {
        let mut gpu_addr = start_addr;
        // TODO(https://fxbug.dev/488424335): Update this so we don't walk all the page tables each time.
        for physical_addr in &pinned_memory.bus_addrs {
            self.insert_pte(gpu_addr, *physical_addr, flags, mapper)?;
            gpu_addr = mem::GpuAddress(gpu_addr.as_u64() + MALI_PAGE_SIZE as u64);
        }
        self.mapping_store.insert(start_addr, pinned_memory);
        Ok(())
    }

    pub fn insert_buffer_auto_address(
        &mut self,
        pinned_memory: mem::PinnedMemory,
        flags: &AccessFlags,
        mapper: &dyn mem::BusMapper,
    ) -> Result<mem::GpuAddress, zx::Status> {
        // TODO(https://fxbug.dev/503722277): Something smarter than this.
        let highest = match self.mapping_store.mappings.last_key_value() {
            Some((key, value)) => mem::GpuAddress(key.0 + value.size() as u64),
            None => mem::GpuAddress(0x1000),
        };
        self.insert_buffer(highest, pinned_memory, flags, mapper)?;
        Ok(highest)
    }

    #[cfg(test)]
    fn read_pte(&self, gpu_address: mem::GpuAddress) -> Option<MaliPte> {
        let page_table_level_three =
            self.root_page_directory.read_to_level(gpu_address, 0, PAGE_DIRECTORY_MAX_LEVEL)?;
        let page_index = PageTable::gpu_addr_to_page_index(gpu_address, PAGE_DIRECTORY_MAX_LEVEL);
        Some(page_table_level_three.read_pte(page_index))
    }
}

/// This represents a single page table page represented in hardware.
struct PageTablePage {
    // The buffer backing this page.
    #[allow(dead_code)]
    buffer: mem::Buffer,
    // A CPU mapping of this page.
    mapping: mem::MappedMemory,
    // A token which pins this page to a GPU physical address.
    gpu_mapping: mem::PinnedMemory,
}

impl PageTablePage {
    fn new(
        bus_mapper: &dyn mem::BusMapper,
        cache_coherence: &Coherency,
    ) -> Result<Self, zx::Status> {
        let buffer = mem::Buffer::new(MALI_PAGE_SIZE as usize, zx::CachePolicy::Cached)?;
        let gpu_mapping =
            buffer.pin(bus_mapper, 0, MALI_PAGE_SIZE as usize, zx::BtiOptions::PERM_READ)?;
        let mapping = buffer.map(
            0,
            MALI_PAGE_SIZE as usize,
            zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE,
        )?;
        let mut page = Self { buffer, mapping, gpu_mapping };
        for entry in 0..PAGE_TABLE_ENTRIES {
            page.write_entry(entry as usize, EntryType::Invalid as u64);
        }
        page.clean_cache(cache_coherence);

        Ok(page)
    }

    /// Writes an entry into the page table.
    /// The called is responsible for flushing the cache if necessary.
    fn write_entry(&mut self, index: usize, value: u64) {
        assert!(index < PAGE_TABLE_ENTRIES as usize);
        // SAFETY: we know that cpu_address is valid is large enough for our write.
        unsafe {
            let ptr = (self.mapping.cpu_address.as_mut_address() as *mut u64).add(index);
            ptr.write_volatile(value);
        }
    }

    #[cfg(test)]
    fn as_entries(&self) -> &[u64] {
        // Safety: We know that the memory backing this page can be viewed as a slice of u64s.
        // The borrow checker will make sure this slice does not outlive us.
        unsafe {
            core::slice::from_raw_parts(
                self.mapping.cpu_address.as_address() as *const u64,
                PAGE_TABLE_ENTRIES as usize,
            )
        }
    }

    fn physical_address(&self) -> mem::PhysicalAddress {
        self.gpu_mapping.bus_addrs[0]
    }

    fn clean_cache(&self, cache_coherence: &Coherency) {
        match cache_coherence {
            Coherency::Coherent => (),
            Coherency::NonCoherent => self.mapping.flush_cache(),
        }
    }
}

/// This represents a single page table level.
struct PageTable {
    // The actual physical page.
    page: PageTablePage,
    // Pointers to the next pages.
    next_level: Box<[Option<PageTable>; PAGE_TABLE_ENTRIES as usize]>,
}

impl PageTable {
    fn new(mapper: &dyn mem::BusMapper, cache_coherence: &Coherency) -> Result<Self, zx::Status> {
        Ok(Self {
            page: PageTablePage::new(mapper, cache_coherence)?,
            next_level: Box::new(std::array::from_fn(|_| None)),
        })
    }

    fn reset(&mut self, cache_coherence: &Coherency) {
        for i in 0..self.next_level.len() {
            if let Some(page) = self.next_level[i].as_mut() {
                page.reset(cache_coherence);
            }
            self.next_level[i] = None;
            self.page.write_entry(i, EntryType::Invalid as u64);
        }
        self.clean_cache(cache_coherence);
    }

    fn gpu_addr_to_page_index(gpu_addr: mem::GpuAddress, current_level: u32) -> usize {
        let shift = ((PAGE_DIRECTORY_MAX_LEVEL - current_level) as u64) * PAGE_OFFSET_BITS
            + MALI_PAGE_SHIFT;
        (gpu_addr.as_u64() as usize >> shift) & (PAGE_TABLE_MASK as usize)
    }

    fn directory_entry(&self) -> MaliPte {
        MaliPte::new_directory_entry(self.page.physical_address())
    }

    fn write_pte(&mut self, page_index: usize, pte: MaliPte, cache_coherence: &Coherency) {
        self.page.write_entry(page_index, pte.0);
        self.clean_cache(cache_coherence);
    }

    #[cfg(test)]
    fn read_pte(&self, page_index: usize) -> MaliPte {
        MaliPte(self.page.as_entries()[page_index])
    }

    fn clean_cache(&self, cache_coherence: &Coherency) {
        self.page.clean_cache(cache_coherence)
    }

    fn add_directory_entry(
        &mut self,
        page_index: usize,
        mapper: &dyn mem::BusMapper,
        cache_coherence: &Coherency,
    ) -> Result<(), zx::Status> {
        let next = PageTable::new(mapper, cache_coherence)?;
        self.page.write_entry(page_index, next.directory_entry().0);
        self.next_level[page_index] = Some(next);
        self.clean_cache(cache_coherence);
        Ok(())
    }

    fn create_to_level(
        &mut self,
        gpu_addr: mem::GpuAddress,
        current_level: u32,
        max_level: u32,
        mapper: &dyn mem::BusMapper,
        cache_coherence: &Coherency,
    ) -> Result<&mut PageTable, zx::Status> {
        if current_level == max_level {
            return Ok(self);
        }
        let page_index = Self::gpu_addr_to_page_index(gpu_addr, current_level);
        if self.next_level[page_index].is_none() {
            self.add_directory_entry(page_index, mapper, cache_coherence)?;
        }
        let next = self.next_level[page_index].as_mut().unwrap();
        next.create_to_level(gpu_addr, current_level + 1, max_level, mapper, cache_coherence)
    }

    #[cfg(test)]
    fn read_to_level(
        &self,
        gpu_addr: mem::GpuAddress,
        current_level: u32,
        max_level: u32,
    ) -> Option<&PageTable> {
        if current_level == max_level {
            return Some(self);
        }
        let page_index = Self::gpu_addr_to_page_index(gpu_addr, current_level);
        let next = self.next_level[page_index].as_ref()?;
        next.read_to_level(gpu_addr, current_level + 1, max_level)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mem::tests::FakeBusMapper;

    const ACCESS_FLAG_ACCESS_BIT: u64 = 1 << 10;

    #[fuchsia::test]
    fn test_init() {
        let bus_mapper = FakeBusMapper::new(0x1000);
        let address_space = AddressSpace::new(Coherency::Coherent, &bus_mapper).unwrap();
        for next in address_space.root_page_directory.next_level.iter() {
            assert!(next.is_none());
        }
        for pte in address_space.root_page_directory.page.as_entries().iter() {
            assert_eq!(*pte, EntryType::Invalid as u64);
        }
    }

    #[fuchsia::test]
    fn test_insert() {
        let bus_mapper = FakeBusMapper::new(0x1000);
        let mut address_space = AddressSpace::new(Coherency::Coherent, &bus_mapper).unwrap();

        let gpu_addr = mem::GpuAddress(0x12345678u64);
        let physical_addr = mem::PhysicalAddress(0x87654000u64);
        let mut flags = AccessFlags(0);
        flags.set_access_flag_no_write(1);
        flags.set_access_flag_read(1);
        address_space.insert_pte(gpu_addr, physical_addr, &flags, &bus_mapper).unwrap();
        assert_eq!(
            address_space.read_pte(mem::GpuAddress(gpu_addr.as_u64() + 0x1000)).unwrap().0,
            EntryType::Invalid as u64,
        );
        assert_eq!(
            address_space.read_pte(gpu_addr).unwrap().0,
            physical_addr.as_u64() | EntryType::Pte as u64 | flags.0 | ACCESS_FLAG_ACCESS_BIT
        );
    }

    #[fuchsia::test]
    fn test_buffer() {
        const PHYSICAL_START: u64 = 0x1_000_000;
        const GPU_START: u64 = 0x123_456_789;
        let pages = 0x0..0x1000;

        let bus_mapper = FakeBusMapper::new(0x1000);
        let mut address_space = AddressSpace::new(Coherency::Coherent, &bus_mapper).unwrap();

        let pinned_memory = mem::PinnedMemory::new_for_test(
            pages
                .clone()
                .map(|i| mem::PhysicalAddress(PHYSICAL_START + i * MALI_PAGE_SIZE))
                .collect(),
        );

        address_space
            .insert_buffer(mem::GpuAddress(GPU_START), pinned_memory, &AccessFlags(0), &bus_mapper)
            .unwrap();

        for i in pages {
            let gpu_addr = mem::GpuAddress(GPU_START + i * MALI_PAGE_SIZE);
            let physical_addr = PHYSICAL_START + i * MALI_PAGE_SIZE;
            assert_eq!(
                address_space.read_pte(gpu_addr).unwrap().0,
                physical_addr | EntryType::Pte as u64 | ACCESS_FLAG_ACCESS_BIT
            );
        }
    }

    #[fuchsia::test]
    fn test_insert_auto_address() {
        let bus_mapper = FakeBusMapper::new(0x1000);
        let mut address_space = AddressSpace::new(Coherency::Coherent, &bus_mapper).unwrap();

        // Map one page.
        let pinned_memory = mem::PinnedMemory::new_for_test(vec![mem::PhysicalAddress(0xcafe)]);
        let gpu_address = address_space
            .insert_buffer_auto_address(pinned_memory, &AccessFlags(0), &bus_mapper)
            .unwrap();

        // Check that our first address maps at 0x1000.
        // (We want to keep the zero page clear)
        assert_eq!(gpu_address.0, MALI_PAGE_SIZE);

        // Map another page.
        let pinned_memory2 = mem::PinnedMemory::new_for_test(vec![mem::PhysicalAddress(0xbeef)]);
        let gpu_address2 = address_space
            .insert_buffer_auto_address(pinned_memory2, &AccessFlags(0), &bus_mapper)
            .unwrap();

        // Check that our second address is after the first one.
        assert_eq!(gpu_address2.0, MALI_PAGE_SIZE + MALI_PAGE_SIZE);
    }
}

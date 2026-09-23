// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "address_space.h"

#include <lib/magma/util/short_macros.h>

const AddressSpace::pte_t AddressSpace::kInvalidPte =
    AddressSpace::pte_encode_unsafe(0xdead1000, false, false, true);
const AddressSpace::pde_t AddressSpace::kInvalidPde =
    AddressSpace::pte_encode_unsafe(0xdead2000, false, false, true);

bool AddressSpace::Page::Init(bool cached) {
  buffer_ = magma::PlatformBuffer::Create(zx_system_get_page_size(), "page table");
  if (!buffer_) {
    MAGMA_LOG(ERROR, "couldn't create buffer");
    return false;
  }

  if (!cached && !buffer_->SetCachePolicy(MAGMA_CACHE_POLICY_UNCACHED)) {
    MAGMA_LOG(ERROR, "couldn't set buffer uncached");
    return false;
  }

  if (!buffer_->MapCpu(&mapping_)) {
    MAGMA_LOG(ERROR, "failed to map cpu");
    return false;
  }

  bus_mapping_ = owner_->GetBusMapper()->MapPageRangeBus(buffer_.get(), 0, 1);
  if (!bus_mapping_) {
    MAGMA_LOG(ERROR, "failed to map page range bus");
    return false;
  }

  return true;
}

std::unique_ptr<AddressSpace::PageTable> AddressSpace::PageTable::Create(Owner* owner) {
  auto page_table = std::make_unique<PageTable>(owner);
  if (!page_table->Init(true)) {  // cached
    MAGMA_LOG(ERROR, "page table init failed");
    return nullptr;
  }

  for (uint32_t i = 0; i < kPageTableEntries; i++) {
    *page_table->entry(i) = kInvalidPte;
  }
  page_table->Flush();

  return page_table;
}

std::unique_ptr<AddressSpace::PageDirectory> AddressSpace::PageDirectory::Create(Owner* owner) {
  auto dir = std::make_unique<PageDirectory>(owner);
  if (!dir->Init(false)) {  // not cached
    MAGMA_LOG(ERROR, "init failed");
    return nullptr;
  }

  for (uint32_t i = 0; i < kPageDirectoryEntries; i++) {
    *dir->entry(i) = kInvalidPde;
  }
  return dir;
}

AddressSpace::PageTable* AddressSpace::PageDirectory::GetPageTable(uint32_t index, bool alloc) {
  DASSERT(index < kPageDirectoryEntries);
  if (!page_tables_[index]) {
    if (!alloc) {
      MAGMA_LOG(ERROR, "No scratch table");
      return nullptr;
    }
    page_tables_[index] = PageTable::Create(owner());
    if (!page_tables_[index]) {
      MAGMA_LOG(ERROR, "couldn't create page table");
      return nullptr;
    }

    pde_t* pde = entry(index);
    if (!pde_encode(page_tables_[index]->bus_addr(), true, pde)) {
      MAGMA_LOG(ERROR, "failed to encode pde");
      return nullptr;
    }
  }
  return page_tables_[index].get();
}

AddressSpace::pte_t* AddressSpace::PageDirectory::GetPageTableEntry(uint32_t page_directory_index,
                                                                    uint32_t page_table_index,
                                                                    uint32_t* valid_count_out) {
  DASSERT(page_directory_index < kPageDirectoryEntries);
  *valid_count_out = valid_counts_[page_directory_index];
  auto table = GetPageTable(page_directory_index, true);
  if (!table) {
    MAGMA_LOG(ERROR, "Page table entry not found");
    return nullptr;
  }
  return table->entry(page_table_index);
}

void AddressSpace::PageDirectory::PageTableUpdated(uint32_t page_directory_index,
                                                   uint32_t valid_count) {
  DASSERT(valid_count <= kPageDirectoryEntries);
  DASSERT(page_directory_index < kPageDirectoryEntries);
  valid_counts_[page_directory_index] = valid_count;
  page_tables_[page_directory_index]->Flush();
  if (valid_count == 0) {
    *entry(page_directory_index) = kInvalidPde;
    page_tables_[page_directory_index].reset();
  }
}

//////////////////////////////////////////////////////////////////////////////////////////////////

std::unique_ptr<AddressSpace> AddressSpace::Create(Owner* owner, uint32_t page_table_array_slot) {
  const size_t page_size = zx_system_get_page_size();
  ZX_ASSERT_MSG(page_size == 0x1000, "Supported only for 4KiB pages; not %#zx", page_size);

  auto address_space = std::make_unique<AddressSpace>(owner, page_table_array_slot);
  if (!address_space->Init()) {
    MAGMA_LOG(ERROR, "Failed to init");
    return nullptr;
  }
  return address_space;
}

bool AddressSpace::Init() {
  root_ = PageDirectory::Create(owner_);
  if (!root_) {
    MAGMA_LOG(ERROR, "Failed to create page directory");
    return false;
  }

  return true;
}

bool AddressSpace::InsertLocked(uint64_t addr, magma::PlatformBusMapper::BusMapping* bus_mapping,
                                uint32_t guard_page_count) {
  DASSERT(magma::is_page_aligned(addr));
  DASSERT(guard_page_count == 0);

  auto& bus_addr_array = bus_mapping->Get();
  uint64_t page_count = bus_addr_array.size();

  if (page_count > bus_addr_array.size()) {
    MAGMA_LOG(ERROR, "page_count %lu larger than bus mapping length %zu", page_count,
              bus_addr_array.size());
    return false;
  }

  uint32_t page_table_index = (addr >>= PageShift()) & kPageTableMask;
  uint32_t page_directory_index = (addr >>= kPageTableShift) & kPageDirectoryMask;

  DLOG("insert pd %i pt %u", page_directory_index, page_table_index);

  uint32_t valid_count;
  auto page_table_entry =
      root_->GetPageTableEntry(page_directory_index, page_table_index, &valid_count);

  for (uint64_t i = 0; i < page_count; i++) {
    pte_t pte;
    if (!pte_encode(bus_addr_array[i], true, true, true, &pte)) {
      MAGMA_LOG(ERROR, "failed to encode pte");
      return false;
    }

    if (!page_table_entry) {
      MAGMA_LOG(ERROR, "couldn't get page table entry");
      return false;
    }

    if (*page_table_entry == kInvalidPte) {
      valid_count++;
    }
    *page_table_entry++ = pte;

    if (++page_table_index == kPageTableEntries) {
      root_->PageTableUpdated(page_directory_index, valid_count);
      page_table_index = 0;

      if (++page_directory_index == kPageDirectoryEntries) {
        // That's all folks.
        return true;
      }
      page_table_entry =
          root_->GetPageTableEntry(page_directory_index, page_table_index, &valid_count);
    }
  }

  root_->PageTableUpdated(page_directory_index, valid_count);

  return true;
}

bool AddressSpace::ClearLocked(uint64_t addr, magma::PlatformBusMapper::BusMapping* bus_mapping) {
  DASSERT(magma::is_page_aligned(addr));
  uint64_t page_count = bus_mapping->page_count();

  const size_t page_shift = PageShift();
  if ((addr >> page_shift) + page_count > (1l << (kVirtualAddressBits - page_shift))) {
    MAGMA_LOG(ERROR, "Virtual address too large");
    return false;
  }

  uint32_t page_table_index = (addr >>= page_shift) & kPageTableMask;
  uint32_t page_directory_index = (addr >>= kPageTableShift) & kPageDirectoryMask;

  DLOG("clear pd %i pt %u", page_directory_index, page_table_index);

  uint32_t valid_count;
  auto page_table_entry =
      root_->GetPageTableEntry(page_directory_index, page_table_index, &valid_count);

  for (uint64_t i = 0; i < page_count; i++) {
    if (!page_table_entry) {
      MAGMA_LOG(ERROR, "couldn't get page table entry");
      return false;
    }

    if (*page_table_entry != kInvalidPte) {
      DASSERT(valid_count > 0);
      valid_count--;
      *page_table_entry = kInvalidPte;
    }
    page_table_entry++;

    if (++page_table_index == kPageTableEntries) {
      root_->PageTableUpdated(page_directory_index, valid_count);
      page_table_index = 0;

      if (++page_directory_index == kPageDirectoryEntries) {
        // That's all folks.
        return true;
      }
      page_table_entry =
          root_->GetPageTableEntry(page_directory_index, page_table_index, &valid_count);
    }
  }

  root_->PageTableUpdated(page_directory_index, valid_count);

  return true;
}

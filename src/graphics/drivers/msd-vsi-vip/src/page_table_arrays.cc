// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "page_table_arrays.h"

#include <lib/magma/util/short_macros.h>

#include "registers.h"

const uint32_t PageTableArrays::kPageTableArrayEntries =
    kPageTableArraySizeInPages * zx_system_get_page_size() / sizeof(uint64_t);

std::unique_ptr<PageTableArrays> PageTableArrays::Create(magma::PlatformBusMapper* bus_mapper) {
  auto address_manager = std::make_unique<PageTableArrays>();
  if (!address_manager->Init(bus_mapper)) {
    MAGMA_LOG(ERROR, "Failed to initialize");
    return nullptr;
  }

  return address_manager;
}

bool PageTableArrays::Init(magma::PlatformBusMapper* bus_mapper) {
  const size_t page_size = zx_system_get_page_size();
  page_table_array_ =
      magma::PlatformBuffer::Create(kPageTableArraySizeInPages * page_size, "page_table_array");
  if (!page_table_array_) {
    MAGMA_LOG(ERROR, "Failed to allocate page_table_array");
    return false;
  }

  if (!page_table_array_->SetCachePolicy(MAGMA_CACHE_POLICY_UNCACHED)) {
    MAGMA_LOG(ERROR, "Failed to set cache policy");
    return false;
  }

  if (!page_table_array_->MapCpu(reinterpret_cast<void**>(&master_tlb_bus_addr_))) {
    MAGMA_LOG(ERROR, "Failed to map page table array");
    return false;
  }

  bus_mapping_ =
      bus_mapper->MapPageRangeBus(page_table_array_.get(), 0, kPageTableArraySizeInPages);
  if (!bus_mapping_) {
    MAGMA_LOG(ERROR, "Failed to bus map page table array");
    return false;
  }

  security_safe_page_ = magma::PlatformBuffer::Create(page_size, "security safe page");
  if (!security_safe_page_) {
    MAGMA_LOG(ERROR, "Failed to allocated security safe page");
    return false;
  }

  security_safe_page_bus_mapping_ = bus_mapper->MapPageRangeBus(security_safe_page_.get(), 0, 1);
  if (!security_safe_page_bus_mapping_) {
    MAGMA_LOG(ERROR, "Failed to bus map security safe page");
    return false;
  }

  non_security_safe_page_ = magma::PlatformBuffer::Create(page_size, "security safe page");
  if (!non_security_safe_page_) {
    MAGMA_LOG(ERROR, "Failed to allocated non security safe page");
    return false;
  }

  non_security_safe_page_bus_mapping_ =
      bus_mapper->MapPageRangeBus(non_security_safe_page_.get(), 0, 1);
  if (!non_security_safe_page_bus_mapping_) {
    MAGMA_LOG(ERROR, "Failed to bus map non security safe page");
    return false;
  }

  return true;
}

void PageTableArrays::HardwareInit(magma::RegisterIo* register_io) {
  uint64_t security_safe_page_bus_addr = security_safe_page_bus_mapping_->Get()[0];
  uint64_t non_security_safe_page_bus_addr = non_security_safe_page_bus_mapping_->Get()[0];

  DASSERT(fits_in_40_bits(security_safe_page_bus_addr));
  DASSERT(fits_in_40_bits(non_security_safe_page_bus_addr));

  Enable(register_io, false);

  {
    auto reg = registers::PageTableArrayAddressLow::Get().FromValue(0);
    reg.set_reg_value(magma::lower_32_bits(bus_addr()));
    reg.WriteTo(register_io);
  }
  {
    auto reg = registers::PageTableArrayAddressHigh::Get().FromValue(0);
    reg.set_reg_value(magma::upper_32_bits(bus_addr()));
    reg.WriteTo(register_io);
  }
  {
    auto reg = registers::PageTableArrayControl::Get().FromValue(0);
    reg.set_enable(1);
    reg.WriteTo(register_io);
  }
  {
    auto reg = registers::MmuNonSecuritySafeAddressLow::Get().FromValue(0);
    reg.set_reg_value(magma::lower_32_bits(non_security_safe_page_bus_addr));
    reg.WriteTo(register_io);
  }
  {
    auto reg = registers::MmuSecuritySafeAddressLow::Get().FromValue(0);
    reg.set_reg_value(magma::lower_32_bits(security_safe_page_bus_addr));
    reg.WriteTo(register_io);
  }
  {
    auto reg = registers::MmuSafeAddressConfig::Get().FromValue(0);
    reg.set_non_security_safe_address_high(non_security_safe_page_bus_addr & 0xFF);
    reg.set_security_safe_address_high(security_safe_page_bus_addr & 0xFF);
    reg.WriteTo(register_io);
  }
}

void PageTableArrays::Enable(magma::RegisterIo* register_io, bool enable) {
  auto reg_mmu_sec_ctrl = registers::MmuSecureControl::Get().ReadFrom(register_io);
  reg_mmu_sec_ctrl.set_enable(enable ? 1 : 0);
  reg_mmu_sec_ctrl.WriteTo(register_io);
}

void PageTableArrays::AssignAddressSpace(uint32_t index, AddressSpace* address_space) {
  DASSERT(index <= kPageTableArrayEntries);
  master_tlb_bus_addr_[index] = address_space->bus_addr();
}

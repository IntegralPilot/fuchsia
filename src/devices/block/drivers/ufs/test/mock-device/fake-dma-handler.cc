// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "fake-dma-handler.h"

#include <lib/fit/defer.h>
#include <lib/zx/vmar.h>

namespace ufs {
namespace ufs_mock_device {

FakeDmaHandler::FakeDmaHandler() {
  for (size_t i = 0; i < std::size(fake_bti_paddrs_); i++) {
    fake_bti_paddrs_[i] = FAKE_BTI_PHYS_ADDR * (i + 1);
  }

  fake_bti_create_with_paddrs(fake_bti_paddrs_, kFakeBtiAddrsCount,
                              fake_bti_.reset_and_get_address());
}

FakeDmaHandler::~FakeDmaHandler() { Reset(); }

void FakeDmaHandler::Reset() {
  for (const auto& reg : mapped_addrs_) {
    zx::vmar::root_self()->unmap(reg.vaddr, reg.size);
  }
  mapped_addrs_.clear();

  fake_bti_.reset();
  fake_bti_create_with_paddrs(fake_bti_paddrs_, kFakeBtiAddrsCount,
                              fake_bti_.reset_and_get_address());
}

zx::result<zx_vaddr_t> FakeDmaHandler::PhysToVirt(zx_paddr_t paddr) {
  uint64_t page_offset = paddr % zx_system_get_page_size();
  zx_paddr_t base_paddr = paddr - page_offset;

  for (const auto& reg : mapped_addrs_) {
    for (size_t k = 0; k < reg.paddrs.size(); ++k) {
      if (reg.paddrs[k] == base_paddr) {
        return zx::ok(reg.vaddr + k * zx_system_get_page_size() + page_offset);
      }
    }
  }

  if (base_paddr % FAKE_BTI_PHYS_ADDR != 0 || base_paddr == 0) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  if (base_paddr > FAKE_BTI_PHYS_ADDR * kFakeBtiAddrsCount) {
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }

  size_t vmo_info_num = 0;
  if (auto status = fake_bti_get_pinned_vmos(fake_bti_.get(), nullptr, 0, &vmo_info_num);
      status != ZX_OK) {
    return zx::error(status);
  }

  std::vector<fake_bti_pinned_vmo_info_t> vmo_infos(vmo_info_num);
  if (auto status = fake_bti_get_pinned_vmos(fake_bti_.get(), vmo_infos.data(), vmo_infos.size(),
                                             &vmo_info_num);
      status != ZX_OK) {
    return zx::error(status);
  }

  // fake_bti_get_pinned_vmos returns raw handles so fit::defer is required to prevent handle leaks.
  auto defer = fit::defer([&]() {
    for (size_t i = 0; i < vmo_info_num; ++i) {
      if (vmo_infos[i].vmo != ZX_HANDLE_INVALID) {
        zx_handle_close(vmo_infos[i].vmo);
      }
    }
  });

  for (ssize_t i = static_cast<ssize_t>(vmo_info_num) - 1; i >= 0; --i) {
    zx::unowned_vmo vmo(vmo_infos[i].vmo);
    size_t num_paddrs;
    std::vector<zx_paddr_t> paddrs(kFakeBtiAddrsCount);
    if (auto status = fake_bti_get_phys_from_pinned_vmo(
            fake_bti_.get(), vmo_infos[i], paddrs.data(), kFakeBtiAddrsCount, &num_paddrs);
        status != ZX_OK) {
      return zx::error(status);
    }

    for (uint32_t paddr_index = 0; paddr_index < num_paddrs; ++paddr_index) {
      if (base_paddr == paddrs[paddr_index]) {
        zx_vaddr_t vaddr;
        if (auto status =
                zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, *vmo,
                                           vmo_infos[i].offset, vmo_infos[i].size, &vaddr);
            status != ZX_OK) {
          return zx::error(status);
        }
        paddrs.resize(num_paddrs);
        mapped_addrs_.push_back({vaddr, vmo_infos[i].size, std::move(paddrs)});
        return zx::ok(vaddr + paddr_index * zx_system_get_page_size() + page_offset);
      }
    }
  }

  return zx::error(ZX_ERR_NOT_FOUND);
}

}  // namespace ufs_mock_device
}  // namespace ufs

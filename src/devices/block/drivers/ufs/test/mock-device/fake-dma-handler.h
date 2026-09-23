// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_UFS_TEST_MOCK_DEVICE_FAKE_DMA_HANDLER_H_
#define SRC_DEVICES_BLOCK_DRIVERS_UFS_TEST_MOCK_DEVICE_FAKE_DMA_HANDLER_H_

#include <lib/fake-bti/bti.h>
#include <lib/zx/bti.h>
#include <lib/zx/result.h>

#include <vector>

namespace ufs {
namespace ufs_mock_device {

constexpr uint32_t kFakeBtiAddrsCount = 1024;

struct MappedRegion {
  zx_vaddr_t vaddr;
  size_t size;
  std::vector<zx_paddr_t> paddrs;
};

class FakeDmaHandler {
 public:
  FakeDmaHandler();
  FakeDmaHandler(const FakeDmaHandler &) = delete;
  FakeDmaHandler &operator=(const FakeDmaHandler &) = delete;
  FakeDmaHandler(const FakeDmaHandler &&) = delete;
  FakeDmaHandler &operator=(const FakeDmaHandler &&) = delete;
  ~FakeDmaHandler();

  zx::bti DuplicateFakeBti() {
    zx::bti fake_bti_duplicated;
    fake_bti_.duplicate(ZX_RIGHT_SAME_RIGHTS, &fake_bti_duplicated);
    return fake_bti_duplicated;
  }

  // You can convert the physical address of the vmo page pinned to |fake_bti_| to a virtual address
  // through this function. This function allows the mock device to access the vmo corresponding to
  // the physical address to read and write data. The |paddr| must be the physical address of the
  // vmo page pinned from |fake_bti_| by zx_bti_pin(), otherwise it returns ZX_ERR_NOT_FOUND.
  zx::result<zx_vaddr_t> PhysToVirt(zx_paddr_t paddr);

  void Reset();

 private:
  zx_paddr_t fake_bti_paddrs_[kFakeBtiAddrsCount];
  zx::bti fake_bti_;
  std::vector<MappedRegion> mapped_addrs_;
};

}  // namespace ufs_mock_device
}  // namespace ufs

#endif  // SRC_DEVICES_BLOCK_DRIVERS_UFS_TEST_MOCK_DEVICE_FAKE_DMA_HANDLER_H_

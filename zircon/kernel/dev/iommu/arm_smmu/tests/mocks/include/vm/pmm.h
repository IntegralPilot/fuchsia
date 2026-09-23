// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_TESTS_MOCKS_INCLUDE_VM_PMM_H_
#define ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_TESTS_MOCKS_INCLUDE_VM_PMM_H_

#include <lib/fit/defer.h>
#include <sys/types.h>
#include <zircon/assert.h>
#include <zircon/types.h>

#include <memory>
#include <mutex>
#include <optional>
#include <unordered_map>

#include <vm/page.h>

#define PMM_ALLOC_FLAG_ANY 0

class PmmMock {
 public:
  static PmmMock& Get() { return instance_; }

  zx_status_t AllocPage(uint32_t alloc_flags, vm_page_t** p);
  void Free(VmPageDoublyLinkedList* list);
  size_t GetAllocatedPageCount();
  vm_page_t* PaddrToVmPage(paddr_t pa);

  void SetAllocationLimit(uint64_t limit);
  void ClearAllocationLimit();

 private:
  PmmMock() = default;
  ~PmmMock() = default;

  static PmmMock instance_;

  std::unordered_map<paddr_t, std::unique_ptr<vm_page_t>> allocated_pages_;
  std::mutex lock_;
  std::optional<uint64_t> alloc_limit_;
};

inline PmmMock PmmMock::instance_;

zx_status_t pmm_alloc_page(uint32_t alloc_flags, vm_page_t** p);
zx_status_t pmm_alloc_page(uint32_t alloc_flags, vm_page_t** p, paddr_t* pa);

vm_page_t* paddr_to_vm_page(paddr_t pa);

void pmm_free(VmPageDoublyLinkedList* list);

#endif  // ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_TESTS_MOCKS_INCLUDE_VM_PMM_H_

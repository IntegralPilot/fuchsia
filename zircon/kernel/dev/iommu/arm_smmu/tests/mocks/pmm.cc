// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <string.h>

#include <vm/pmm.h>

zx_status_t PmmMock::AllocPage(uint32_t alloc_flags, vm_page_t** p) {
  {
    std::lock_guard<std::mutex> guard(lock_);
    if (alloc_limit_) {
      if (*alloc_limit_ == 0) {
        return ZX_ERR_NO_MEMORY;
      }
      --(*alloc_limit_);
    }
  }

  std::unique_ptr<vm_page_t> page = std::make_unique<vm_page_t>();

  // Allocate a separate 4k page for each vm_page.
  uint8_t* page_mem = new (std::align_val_t(kPageSize)) uint8_t[kPageSize];
  if (!page_mem) {
    return ZX_ERR_NO_MEMORY;
  }
  memset(page_mem, 0xa5, kPageSize);
  page->paddr_priv.reset(page_mem);

  std::lock_guard<std::mutex> guard(lock_);
  *p = page.get();
  paddr_t pa = (*p)->paddr();
  allocated_pages_[pa] = std::move(page);
  return ZX_OK;
}

void PmmMock::Free(VmPageDoublyLinkedList* list) {
  std::lock_guard<std::mutex> guard(lock_);
  while (!list->is_empty()) {
    vm_page_t* p = list->pop_front();
    uint8_t* ptr = p->paddr_priv.get();
    for (size_t i = 0; i < kPageSize; ++i) {
      ZX_ASSERT_MSG(ptr[i] == 0, "Page at paddr 0x%lx not zeroed at offset 0x%zx (val=0x%x)",
                    p->paddr(), i, ptr[i]);
    }
    allocated_pages_.erase(p->paddr());
  }
}

size_t PmmMock::GetAllocatedPageCount() {
  std::lock_guard<std::mutex> guard(lock_);
  return allocated_pages_.size();
}

vm_page_t* PmmMock::PaddrToVmPage(paddr_t pa) {
  std::lock_guard<std::mutex> guard(lock_);
  auto it = allocated_pages_.find(pa);
  if (it != allocated_pages_.end()) {
    return it->second.get();
  }
  return nullptr;
}

void PmmMock::SetAllocationLimit(uint64_t limit) {
  std::lock_guard<std::mutex> guard(lock_);
  alloc_limit_ = limit;
}

void PmmMock::ClearAllocationLimit() {
  std::lock_guard<std::mutex> guard(lock_);
  alloc_limit_ = std::nullopt;
}

zx_status_t pmm_alloc_page(uint32_t alloc_flags, vm_page_t** p) {
  return PmmMock::Get().AllocPage(alloc_flags, p);
}

zx_status_t pmm_alloc_page(uint32_t alloc_flags, vm_page_t** p, paddr_t* pa) {
  zx_status_t status = pmm_alloc_page(alloc_flags, p);
  if (status == ZX_OK) {
    *pa = (*p)->paddr();
  }
  return status;
}

void pmm_free(VmPageDoublyLinkedList* list) { PmmMock::Get().Free(list); }

vm_page_t* paddr_to_vm_page(paddr_t pa) { return PmmMock::Get().PaddrToVmPage(pa); }

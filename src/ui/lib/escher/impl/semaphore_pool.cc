// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/lib/escher/impl/semaphore_pool.h"

#include <lib/syslog/cpp/macros.h>

#include "src/ui/lib/escher/impl/vulkan_utils.h"
#include "src/ui/lib/escher/util/trace_macros.h"

namespace escher {

SemaphorePool::SemaphorePool(vk::Device device, vk::detail::DispatchLoaderDynamic dispatch_loader)
    : device_(device), dispatch_loader_(dispatch_loader) {}

SemaphorePool::~SemaphorePool() {
  FX_DCHECK(outstanding_semaphores_ == 0)
      << "SemaphorePool destroyed with " << outstanding_semaphores_
      << " outstanding semaphores still in use!";

  // SemaphorePtr objects are destroyed with the free list vectors, which drops their ref count
  // to zero. This triggers Semaphore::OnZeroRefCount() which will attempt to return the semaphore
  // to a partially destroyed pool, if its pool_ pointer is not cleared.
  for (auto& sem : cleaned_free_list_) {
    sem->pool_ = nullptr;
  }
  for (auto& sem : uncleaned_free_list_) {
    sem->pool_ = nullptr;
  }
}

SemaphorePtr SemaphorePool::Allocate() {
  ++outstanding_semaphores_;

  // Prefer pulling an already-clean semaphore from the cleaned list to achieve 0 imports on
  // the critical path.
  if (!cleaned_free_list_.empty()) {
    auto sem = std::move(cleaned_free_list_.back());
    cleaned_free_list_.pop_back();
    return sem;
  }

  // Fall back to the uncleaned list and perform a lazy clean to guarantee it is safe before reuse.
  if (!uncleaned_free_list_.empty()) {
    auto sem = std::move(uncleaned_free_list_.back());
    uncleaned_free_list_.pop_back();

    zx::event cleaning_event;
    zx_status_t status = zx::event::create(0, &cleaning_event);
    FX_DCHECK(status == ZX_OK);

    vk::ImportSemaphoreZirconHandleInfoFUCHSIA info;
    info.semaphore = sem->vk_semaphore();
    info.zirconHandle = cleaning_event.release();
    info.handleType = vk::ExternalSemaphoreHandleTypeFlagBits::eZirconEventFUCHSIA;

    {
      TRACE_DURATION("gfx",
                     "SemaphorePool::Allocate[importSemaphoreZirconHandleFUCHSIA] (cleaning)");
      ESCHER_CHECKED_VK_RESULT(device_.importSemaphoreZirconHandleFUCHSIA(info, dispatch_loader_));
    }
    return sem;
  }

  // Pool is completely empty; allocate a brand new semaphore.
  return Semaphore::New(device_, this);
}

SemaphorePtr SemaphorePool::AllocateAndImport(zx::event event_to_import) {
  ++outstanding_semaphores_;

  // Prefer pulling from the uncleaned list because the subsequent import will naturally
  // overwrite and clean it, avoiding a double-import.
  SemaphorePtr sem;
  if (!uncleaned_free_list_.empty()) {
    sem = std::move(uncleaned_free_list_.back());
    uncleaned_free_list_.pop_back();
  } else if (!cleaned_free_list_.empty()) {
    sem = std::move(cleaned_free_list_.back());
    cleaned_free_list_.pop_back();
  } else {
    sem = Semaphore::New(device_, this);
  }

  vk::ImportSemaphoreZirconHandleInfoFUCHSIA info;
  info.semaphore = sem->vk_semaphore();
  info.zirconHandle = event_to_import.release();
  info.handleType = vk::ExternalSemaphoreHandleTypeFlagBits::eZirconEventFUCHSIA;

  {
    TRACE_DURATION("gfx", "SemaphorePool::AllocateAndImport[importSemaphoreZirconHandleFUCHSIA]");
    ESCHER_CHECKED_VK_RESULT(device_.importSemaphoreZirconHandleFUCHSIA(info, dispatch_loader_));
  }
  sem->set_is_imported(true);
  return sem;
}

void SemaphorePool::ReturnSemaphore(SemaphorePtr semaphore) {
  --outstanding_semaphores_;

  // If the semaphore previously required an import, we return it directly to the uncleaned list.
  // Semaphores with previous imports will likely be required to import a Zircon event again
  // when reused, so it makes sense not to clean them here.
  if (semaphore->is_imported()) {
    // Reset the flag to false during retirement. This flag is set again in AllocateAndImport
    // if the semaphore imports a Zircon event when re-allocated.
    semaphore->set_is_imported(false);
    uncleaned_free_list_.push_back(std::move(semaphore));
    return;
  }

  // If the semaphore did not previously import a Zircon event, it is "cleaned" with a fresh
  // Zircon event to guarantee it is unsignaled and returned to the cleaned list.
  zx::event cleaning_event;
  zx_status_t status = zx::event::create(0, &cleaning_event);
  FX_DCHECK(status == ZX_OK);

  vk::ImportSemaphoreZirconHandleInfoFUCHSIA info;
  info.semaphore = semaphore->vk_semaphore();
  info.zirconHandle = cleaning_event.release();
  info.handleType = vk::ExternalSemaphoreHandleTypeFlagBits::eZirconEventFUCHSIA;

  {
    TRACE_DURATION("gfx",
                   "SemaphorePool::ReturnSemaphore[importSemaphoreZirconHandleFUCHSIA] (cleaning)");
    ESCHER_CHECKED_VK_RESULT(device_.importSemaphoreZirconHandleFUCHSIA(info, dispatch_loader_));
  }

  cleaned_free_list_.push_back(std::move(semaphore));
}

}  // namespace escher

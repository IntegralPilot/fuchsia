// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_LIB_ESCHER_IMPL_SEMAPHORE_POOL_H_
#define SRC_UI_LIB_ESCHER_IMPL_SEMAPHORE_POOL_H_

#include <lib/zx/event.h>

#include <vector>

#include "src/lib/fxl/macros.h"
#include "src/ui/lib/escher/renderer/semaphore.h"

#include <vulkan/vulkan.hpp>

namespace escher {

// A single-threaded pool for recycling Vulkan semaphores.
class SemaphorePool {
 public:
  SemaphorePool(vk::Device device, vk::detail::DispatchLoaderDynamic dispatch_loader);
  ~SemaphorePool();

  // Pull a clean, unsignaled semaphore from the pool or lazily create a new one.
  SemaphorePtr Allocate();

  // Pull a semaphore and immediately import a Zircon event payload.
  SemaphorePtr AllocateAndImport(zx::event event_to_import);

 private:
  // Make escher::Semaphore a friend so its OnZeroRefCount() hook can return itself.
  friend class Semaphore;

  // Return a semaphore to the pool. Routes it to either the cleaned or uncleaned
  // free list depending on its current import state, and resets its imported flag.
  void ReturnSemaphore(SemaphorePtr semaphore);

  vk::Device device_;
  vk::detail::DispatchLoaderDynamic dispatch_loader_;

  std::vector<SemaphorePtr> cleaned_free_list_;
  std::vector<SemaphorePtr> uncleaned_free_list_;
  uint32_t outstanding_semaphores_ = 0;

  FXL_DISALLOW_COPY_AND_ASSIGN(SemaphorePool);
};

}  // namespace escher

#endif  // SRC_UI_LIB_ESCHER_IMPL_SEMAPHORE_POOL_H_

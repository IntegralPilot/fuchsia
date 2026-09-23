// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_SYS_DRIVER_MAGMA_SYSTEM_SEMAPHORE_H_
#define SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_SYS_DRIVER_MAGMA_SYSTEM_SEMAPHORE_H_

#include <lib/magma_service/msd.h>

#include <memory>

namespace msd {
class MagmaSystemSemaphore {
 public:
  static std::unique_ptr<MagmaSystemSemaphore> Create(msd::Driver* device, zx::handle handle,
                                                      uint64_t client_id, uint64_t flags);

  uint64_t global_id() const { return global_id_; }

  msd::Semaphore* msd_semaphore() { return msd_semaphore_.get(); }

 private:
  MagmaSystemSemaphore(uint64_t global_id, std::unique_ptr<msd::Semaphore> msd_semaphore_t);

  uint64_t global_id_;
  std::unique_ptr<msd::Semaphore> msd_semaphore_;
};

}  // namespace msd

#endif  // SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_SYS_DRIVER_MAGMA_SYSTEM_SEMAPHORE_H_

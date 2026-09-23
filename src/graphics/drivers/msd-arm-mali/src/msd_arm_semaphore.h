// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef MSD_ARM_SEMAPHORE_H
#define MSD_ARM_SEMAPHORE_H

#include <lib/magma/platform/platform_semaphore.h>
#include <lib/magma/util/short_macros.h>
#include <lib/magma_service/msd.h>

class MsdArmAbiSemaphore : public msd::Semaphore {
 public:
  MsdArmAbiSemaphore(std::shared_ptr<magma::PlatformSemaphore> ptr) : ptr_(std::move(ptr)) {
    magic_ = kMagic;
  }

  static MsdArmAbiSemaphore* cast(msd::Semaphore* semaphore) {
    DASSERT(semaphore);
    auto sem = static_cast<MsdArmAbiSemaphore*>(semaphore);
    DASSERT(sem->magic_ == kMagic);
    return sem;
  }

  std::shared_ptr<magma::PlatformSemaphore> ptr() { return ptr_; }

 private:
  std::shared_ptr<magma::PlatformSemaphore> ptr_;

  static constexpr uint32_t kMagic = 0x73656d61;  // "sema"
  uint32_t magic_;
};

#endif  // MSD_ARM_SEMAPHORE_H

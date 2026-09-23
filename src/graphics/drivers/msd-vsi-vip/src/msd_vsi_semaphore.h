// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef MSD_VSI_SEMAPHORE_H
#define MSD_VSI_SEMAPHORE_H

#include <lib/magma/platform/platform_semaphore.h>
#include <lib/magma/util/short_macros.h>
#include <lib/magma_service/msd.h>

class MsdVsiAbiSemaphore : public msd::Semaphore {
 public:
  explicit MsdVsiAbiSemaphore(std::shared_ptr<magma::PlatformSemaphore> ptr)
      : ptr_(std::move(ptr)), magic_(kMagic) {}

  static MsdVsiAbiSemaphore* cast(msd::Semaphore* sema) {
    DASSERT(sema);
    auto semaphore = static_cast<MsdVsiAbiSemaphore*>(sema);
    DASSERT(semaphore->magic_ == kMagic);
    return semaphore;
  }

  std::shared_ptr<magma::PlatformSemaphore> ptr() { return ptr_; }

 private:
  std::shared_ptr<magma::PlatformSemaphore> ptr_;

  static constexpr uint32_t kMagic = 0x73656d61;  // "sema"
  const uint32_t magic_;
};

#endif  // MSD_VSI_SEMAPHORE_H

// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef MSD_INTEL_SEMAPHORE_H
#define MSD_INTEL_SEMAPHORE_H

#include <lib/magma/platform/platform_semaphore.h>
#include <lib/magma_service/msd.h>

class MsdIntelAbiSemaphore : public msd::Semaphore {
 public:
  explicit MsdIntelAbiSemaphore(std::shared_ptr<magma::PlatformSemaphore> ptr)
      : ptr_(std::move(ptr)) {}

  std::shared_ptr<magma::PlatformSemaphore> ptr() { return ptr_; }

 private:
  std::shared_ptr<magma::PlatformSemaphore> ptr_;
};

#endif  // MSD_INTEL_SEMAPHORE_H

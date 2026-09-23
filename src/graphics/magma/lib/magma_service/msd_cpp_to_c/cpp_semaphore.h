// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_MSD_CPP_TO_C_CPP_SEMAPHORE_H_
#define SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_MSD_CPP_TO_C_CPP_SEMAPHORE_H_

#include <lib/magma_service/msd.h>
#include <lib/magma_service/msd_c.h>

namespace msd {

class CppSemaphore : public Semaphore {
 public:
  explicit CppSemaphore(struct MsdSemaphore* semaphore);

  ~CppSemaphore() override;

  struct MsdSemaphore* semaphore() { return semaphore_; }

 private:
  struct MsdSemaphore* semaphore_;

  CppSemaphore(const CppSemaphore&) = delete;
  CppSemaphore& operator=(const CppSemaphore&) = delete;
};
}  // namespace msd

#endif  // SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_MSD_CPP_TO_C_CPP_SEMAPHORE_H_

// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "cpp_semaphore.h"

#include <lib/magma/util/macros.h>

namespace msd {

CppSemaphore::CppSemaphore(struct MsdSemaphore* semaphore) : semaphore_(semaphore) {
  MAGMA_DASSERT(semaphore_);
}

CppSemaphore::~CppSemaphore() { msd_semaphore_release(semaphore_); }

}  // namespace msd

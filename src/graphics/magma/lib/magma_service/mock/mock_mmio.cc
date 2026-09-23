// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/magma/util/dlog.h>
#include <lib/magma_service/mock/mock_mmio.h>
#include <stdlib.h>

std::unique_ptr<MockMmio> MockMmio::Create(uint64_t size) {
  void* addr = calloc(size, 1);
  return std::unique_ptr<MockMmio>(new MockMmio(addr, size));
}

MockMmio::MockMmio(void* addr, uint64_t size)
    : magma::PlatformMmio(reinterpret_cast<MMIO_PTR void*>(reinterpret_cast<uintptr_t>(addr)),
                          size) {}

MockMmio::~MockMmio() {
  MAGMA_DLOG("MockMmio dtor");
  free(reinterpret_cast<void*>(reinterpret_cast<uintptr_t>(addr())));
}

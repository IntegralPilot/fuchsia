// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_KERNEL_KOID_H_
#define ZIRCON_KERNEL_INCLUDE_KERNEL_KOID_H_

#include <zircon/types.h>

#include <ktl/atomic.h>

// Generates unique 64bit ids for kernel objects.
class KernelObjectId {
 public:
  // Allocates and returns a KOID.
  static zx_koid_t Generate() { return koid_generator_.fetch_add(1ULL, ktl::memory_order_relaxed); }

  // Allocates a range of sequential KOIDs and returns the first KOID in the range.
  static zx_koid_t GenerateRange(size_t count) {
    return koid_generator_.fetch_add(count, ktl::memory_order_relaxed);
  }

 private:
  inline static ktl::atomic<zx_koid_t> koid_generator_{ZX_KOID_FIRST};
};

#endif  // ZIRCON_KERNEL_INCLUDE_KERNEL_KOID_H_

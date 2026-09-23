// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PHYS_LIB_MEMALLOC_TEST_H_
#define ZIRCON_KERNEL_PHYS_LIB_MEMALLOC_TEST_H_

#include <lib/memalloc/pool.h>
#include <lib/memalloc/range.h>
#include <stdio.h>

#include <memory>
#include <span>
#include <vector>

//
// Some shared test utilities.
//

// PoolContext holds both a Pool and the memory that backs its bookkeeping.
// To decouple memory ranges that are tracked from memory ranges that are
// actually used, we translate any nominal bookkeeping space to the heap.
struct PoolContext {
  std::vector<std::unique_ptr<std::byte[]>> bookkeeping;

  memalloc::Pool pool = memalloc::Pool([this](uint64_t addr, uint64_t size) {
    bookkeeping.push_back(std::make_unique<std::byte[]>(size));
    return bookkeeping.back().get();
  });
};

inline std::span<memalloc::Range> RangesFromBytes(const std::vector<std::byte>& bytes) {
  void* ptr = const_cast<void*>(static_cast<const void*>(bytes.data()));
  size_t space = bytes.size();
  for (size_t size = space; size > 0; --size) {
    if (void* aligned = std::align(alignof(memalloc::Range), size, ptr, space); aligned) {
      return {static_cast<memalloc::Range*>(aligned), size / sizeof(memalloc::Range)};
    }
  }
  return {};
}

#endif  // ZIRCON_KERNEL_PHYS_LIB_MEMALLOC_TEST_H_

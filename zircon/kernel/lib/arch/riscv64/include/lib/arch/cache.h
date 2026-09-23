// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_ARCH_RISCV64_INCLUDE_LIB_ARCH_CACHE_H_
#define ZIRCON_KERNEL_LIB_ARCH_RISCV64_INCLUDE_LIB_ARCH_CACHE_H_

#ifndef __ASSEMBLER__
#include <lib/arch/riscv64/paging-traits.h>
#include <zircon/assert.h>

#include <cstddef>
#include <cstdint>

namespace arch {

// Ensures that the instruction and data caches are in coherence after the
// modification of provided address ranges. The caches are regarded as coherent
// - with respect to the ranges passed to SyncRange() - only after the
// associated object is destroyed.
class GlobalCacheConsistencyContext {
 public:
  // Ensures consistency on destruction.
  ~GlobalCacheConsistencyContext() { __asm__ volatile("fence.i" ::: "memory"); }

  // Records a virtual address range that should factor into consistency.
  void SyncRange(uintptr_t vaddr, size_t size) {}
};

inline void InvalidateLocalTlbs() { __asm__ volatile("sfence.vma zero, zero" ::: "memory"); }

}  // namespace arch

#endif  // __ASSEMBLER__

#endif  // ZIRCON_KERNEL_LIB_ARCH_RISCV64_INCLUDE_LIB_ARCH_CACHE_H_

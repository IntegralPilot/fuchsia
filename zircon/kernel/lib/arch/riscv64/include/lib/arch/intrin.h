// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_ARCH_RISCV64_INCLUDE_LIB_ARCH_INTRIN_H_
#define ZIRCON_KERNEL_LIB_ARCH_RISCV64_INCLUDE_LIB_ARCH_INTRIN_H_

// No intrinsics are used so far.  <riscv_vector.h> requires -m switches.

#ifndef __ASSEMBLER__

// Provide the machine-independent <lib/arch/intrin.h> API.

#include <stdint.h>

#ifdef __cplusplus

namespace arch {

/// Yield the processor momentarily.  This should be used in busy waits.
inline void Yield() { __asm__("pause"); }

// TODO(https://fxbug.dev/42126965): Improve the docs on the barrier APIs, maybe rename/refine.

/// Synchronize all memory accesses of all kinds.
inline void DeviceMemoryBarrier() { __asm__("fence iorw, iorw" ::: "memory"); }

/// Synchronize the ordering of all memory accesses wrt other CPUs.
inline void ThreadMemoryBarrier() { __asm__("fence iorw, iorw" ::: "memory"); }

/// Ensure all stores that appear before this barrier (in program order) complete before any stores
/// that appear after this barrier.
inline void StoreMemoryBarrier() { __asm__("fence w, w" ::: "memory"); }

/// Return the current CPU cycle count.
inline uint64_t Cycles() {
  uint64_t time;
  __asm__ volatile("rdcycle %0" : "=r"(time));
  return time;
}

}  // namespace arch

#endif  // __cplusplus

#endif  // !__ASSEMBLER__

#endif  // ZIRCON_KERNEL_LIB_ARCH_RISCV64_INCLUDE_LIB_ARCH_INTRIN_H_

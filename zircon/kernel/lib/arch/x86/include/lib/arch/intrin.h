// Copyright 2017 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_ARCH_X86_INCLUDE_LIB_ARCH_INTRIN_H_
#define ZIRCON_KERNEL_LIB_ARCH_X86_INCLUDE_LIB_ARCH_INTRIN_H_

#ifndef __ASSEMBLER__
#include <stddef.h>
#include <stdint.h>

// Provide the standard Intel x86 intrinsics API via the compiler headers.

#include <immintrin.h>
#include <x86intrin.h>

#ifdef __cplusplus

// Provide the machine-independent <lib/arch/intrin.h> API.

namespace arch {

/// Yield the processor momentarily.  This should be used in busy waits.
inline void Yield() { _mm_pause(); }

// TODO(https://fxbug.dev/42126965): Improve the docs on the barrier APIs, maybe rename/refine.

/// Synchronize all memory accesses of all kinds.
inline void DeviceMemoryBarrier() { __asm__ volatile("mfence" ::: "memory"); }

/// Synchronize the ordering of all memory accesses wrt other CPUs.
inline void ThreadMemoryBarrier() { DeviceMemoryBarrier(); }

/// Ensure all stores that appear before this barrier (in program order) complete before any stores
/// that appear after this barrier.
inline void StoreMemoryBarrier() {
  // No need to emit a fence instruction. Stores will not be re-ordered with other stores.
  // [intel/vol3]: 8.2.2 Memory Ordering in P6 and More Recent Processor Families
  // [amd/vol2]: 7.2 Multiprocessor Memory Access Ordering
  __asm__ volatile("" ::: "memory");
}

// Force the processor to complete all modifications to register state and
// memory by previous instructions (including draining any buffered writes)
// before the next instruction is fetched.
inline void SerializeInstructions() {
  // [intel/vol3]: 8.3  Serializing Instructions.
  // [amd/vol2]: 7.6.4  Serializing Instructions.
  //
  // `cpuid` is a serializaing instruction.
  uintptr_t rax = 0;
  __asm__ volatile("cpuid" : "+a"(rax) : : "rbx", "rcx", "rdx");
}

/// Return the current CPU cycle count.
inline uint64_t Cycles() { return _rdtsc(); }

}  // namespace arch

#endif  // __cplusplus

#endif  // !__ASSEMBLER__

#endif  // ZIRCON_KERNEL_LIB_ARCH_X86_INCLUDE_LIB_ARCH_INTRIN_H_

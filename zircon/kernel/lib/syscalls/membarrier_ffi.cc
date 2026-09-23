// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <zircon/types.h>

#include <arch/ops.h>
#include <kernel/ffi.h>
#include <kernel/mp.h>

#include "membarrier_priv.h"

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
extern "C" FFI_ALWAYS_INLINE void cpp_membarrier_data_barrier() { arch::ThreadMemoryBarrier(); }

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
extern "C" FFI_ALWAYS_INLINE void cpp_membarrier_instruction_barrier() {
  // All architectures require a memory barrier before serializing the instruction stream.
  arch::ThreadMemoryBarrier();
  // The intrinsics for serializing the instruction stream vary by architecture.
  // TODO(https://fxbug.dev/42126965): Rationalize these.
#if defined(__aarch64__)
  __isb(ARM_MB_SY);
#elif defined(__x86_64__)
  arch::SerializeInstructions();
#elif defined(__riscv)
  __asm__ volatile("fence.i");
#else
#error Unknown architecture.
#endif
}

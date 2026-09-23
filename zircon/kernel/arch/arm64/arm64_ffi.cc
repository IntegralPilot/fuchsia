// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <arch/arm64/mp.h>
#include <kernel/cpu.h>
#include <kernel/ffi.h>

extern "C" {

uint64_t cpp_arm64_cpu_num_to_mpidr(cpu_num_t cpu_num);

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE uint64_t cpp_arm64_cpu_num_to_mpidr(cpu_num_t cpu_num) {
  return arch_cpu_num_to_mpidr(cpu_num);
}

}  // extern "C"

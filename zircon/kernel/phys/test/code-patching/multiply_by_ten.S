// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/asm.h>
#include "test.h"

.text

// uint64_t multiply_by_ten(uint64_t x).
.function multiply_by_ten, global
#if defined(__aarch64__)
  add x0, x0, x0, lsl #2
  lsl x0, x0, #1
#elif defined(__x86_64__)
  add %rdi, %rdi
  lea (%rdi, %rdi, 4), %rax
#elif defined(__riscv)
  sll a1, a0, 2
  add a1, a1, a0
  sll a0, a1, 1
#else
  .error "unknown architecture"
#endif
  ret
  .if (. - multiply_by_ten) != PATCH_SIZE_MULTIPLY_BY_FACTOR
    .error "PATCH_SIZE_MULTIPLY_BY_FACTOR doesn't match the code"
  .endif
.end_function

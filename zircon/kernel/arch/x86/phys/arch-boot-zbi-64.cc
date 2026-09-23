// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <phys/boot-zbi.h>

void BootZbi::ZbiBoot(uintptr_t entry, void* data) const {
  // Clear the stack and frame pointers so no misleading breadcrumbs are left.
  // Use a register constraint for the indirect jump operand so that it can't
  // materialize it via %rbp or %rsp as "g" could (since the compiler doesn't
  // support using those as clobbers).
  __asm__ volatile(
      R"""(
      xor %%ebp, %%ebp
      xor %%esp, %%esp
      cld
      cli
      jmp *%0
      )"""
      :
      : "r"(entry), "S"(data)
      : "cc", "memory");
  __builtin_unreachable();
}

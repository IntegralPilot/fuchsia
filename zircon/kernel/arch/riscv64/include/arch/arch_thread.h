// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_ARCH_THREAD_H_
#define ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_ARCH_THREAD_H_

#ifndef __ASSEMBLER__

#include <stdint.h>
#include <sys/types.h>
#include <zircon/tls.h>

#include <arch/riscv64/fpu.h>
#include <arch/riscv64/vector.h>

struct arch_thread {
  // The compiler knows the position of these two fields relative to tp, which
  // is what __builtin_thread_pointer() returns.  The thread_pointer_location
  // pseudo-member marks where tp points.
  uintptr_t stack_guard;
  vaddr_t unsafe_sp;  // Never used in the kernel, but the ABI needs the space.
  union {
    char thread_pointer_location;
    vaddr_t sp;
  };

  // Debugger access to userspace general regs while suspended or stopped
  // in an exception.
  // The regs are saved on the stack and then a pointer is stored here.
  // Nullptr if not suspended or not stopped in an exception.
  // TODO(https://fxbug.dev/42105394): Also nullptr for synthetic exceptions that don't provide
  // them yet.
  struct iframe_t* suspended_general_regs;

  // If non-NULL, address to return to on data fault.
  uint64_t data_fault_resume;

  // Record whether or not the floating point or vector state was ever modified
  // on this thread.
  bool fpu_dirty;
  bool vector_dirty;

  // Full snapshot of the double precision and vector state at context switch
  // time.
  riscv64_fpu_state fpu_state;
  riscv64_vector_state vector_state;
};

#define thread_pointer_offsetof(field)        \
  ((int)offsetof(struct arch_thread, field) - \
   (int)offsetof(struct arch_thread, thread_pointer_location))

static_assert(thread_pointer_offsetof(stack_guard) == ZX_TLS_STACK_GUARD_OFFSET,
              "stack_guard field in wrong place");
static_assert(thread_pointer_offsetof(unsafe_sp) == ZX_TLS_UNSAFE_SP_OFFSET,
              "unsafe_sp field in wrong place");

#endif  // __ASSEMBLER__

#endif  // ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_ARCH_THREAD_H_

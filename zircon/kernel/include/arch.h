// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2008-2014 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_ARCH_H_
#define ZIRCON_KERNEL_INCLUDE_ARCH_H_

#include <stdio.h>
#include <sys/types.h>
#include <zircon/compiler.h>

struct iframe_t;

// Entry state for a thread.  This is the user-space, machine-independent
// view that's translated to iframe_t by arch_prepare_uspace().
struct UserEntryState {
  uint64_t pc = 0;
  uint64_t sp = 0;
  uint64_t arg1 = 0;
  uint64_t arg2 = 0;
  uint64_t tp = 0;
  uint64_t abi_reg = 0;
};

void PrintFrame(const iframe_t&, FILE* = stdout);

extern "C" {
// Early platform initialization, before UART, MMU, kernel command line args, etc.
void arch_early_init();

// Perform any set up required before virtual memory is enabled, or the heap is set up.
void arch_prevm_init();

// Perform any set up required after heap/MMU is available.
void arch_init();

// Perform any per-CPU set up required.
void arch_late_init_percpu();

// Fill in *|out| with the iframe_t that should be passed to arch_enter_uspace()
// on the current thread.  Initialize it and other user-visible hardware state
// first.  The rest of the current thread's state must already have been
// appropriately initialized (as viewable from a debugger at the
// ZX_EXCP_THREAD_STARTING exception).  This can be called with interrupts still
// enabled.  It's just preparatory to calling arch_enter_uspace().
//
// |out| need not be initialized on entry; every field is written.
void arch_prepare_uspace(const UserEntryState* state, iframe_t* out);

// Enter userspace, after initialization with arch_prepare_uspace().
// Must be called with interrupts disabled.
[[noreturn]] void arch_enter_uspace(const iframe_t* iframe);
}  // extern "C"

// On x86, user mode general registers are stored in one of two structures depending on how the
// thread entered the kernel.  If via interrupt/exception, they are stored in an iframe_t.  If via
// syscall, they are stored in an syscall_regs_t.
//
// On arm64, user mode general registers are stored in an iframe_t regardless of how the thread
// entered the kernel.
enum class GeneralRegsSource : uint32_t {
  None = 0u,
  Iframe = 1u,
#if defined(__x86_64__)
  Syscall = 2u,
#endif
};

/* arch specific bits */

#endif  // ZIRCON_KERNEL_INCLUDE_ARCH_H_

// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <debug.h>
#include <lib/counters.h>
#include <lib/crashlog.h>
#include <lib/thread-stack/abi.h>
#include <platform.h>
#include <stdio.h>
#include <zircon/syscalls/exception.h>
#include <zircon/types.h>

#include <arch/arch_ops.h>
#include <arch/crashlog.h>
#include <arch/exception.h>
#include <arch/regs.h>
#include <arch/riscv64.h>
#include <arch/thread.h>
#include <kernel/ffi.h>
#include <kernel/interrupt.h>
#include <kernel/thread.h>
#include <pretty/hexdump.h>
#include <syscalls/syscalls.h>
#include <vm/fault.h>
#include <vm/vm.h>

extern "C" {

[[noreturn]] void rust_riscv64_emergency_exception(iframe_t* iframe, uint64_t pc, uint64_t status,
                                                   int64_t cause);
void rust_riscv64_print_frame(void (*write_cb)(void*, const char*, size_t), void* ctx,
                              const iframe_t* iframe);

// C++ kernel runtime bridge hooks called from Rust.
void cpp_set_crashlog_regs(const iframe_t* iframe, int64_t cause, uint64_t tval) {
  g_crashlog.regs.iframe = const_cast<iframe_t*>(iframe);
  g_crashlog.regs.cause = cause;
  g_crashlog.regs.tval = tval;
}
zx_status_t cpp_dispatch_user_exception(uint32_t exception_type,
                                        const arch_exception_context_t* context) {
  return dispatch_user_exception(static_cast<zx_excp_type_t>(exception_type), context);
}

}  // extern "C"

void riscv64_print_frame(FILE* target, const iframe_t* iframe) {
  auto write_fn = [](void* ctx, const char* str, size_t len) {
    auto* f = static_cast<FILE*>(ctx);
    fwrite(str, 1, len, f);
  };
  rust_riscv64_print_frame(write_fn, target, iframe);
}

// Low-level trap entry points called from stvec.S assembly.

void Riscv64EmergencyException(iframe_t* iframe, uint64_t pc, uint64_t status, int64_t cause) {
  // The assembly code switches to a dedicated "emergency" stack (and shadow
  // call stack) before calling here.  These just live in kernel bss that is
  // never used except in this panic path, and don't bother with guard regions
  // or page alignment.  The symbols are only referenced in stvec.S assembly,
  // but they need to be defined somewhere.  They're defined here inside the
  // function to take advantage of asm operands for the size constants.
  __asm__ volatile(
      R"""(
      .pushsection .bss, "aw?", %%nobits
      .balign 16
      .globl riscv64_emergency_stack_base
      .hidden riscv64_emergency_stack_base
      riscv64_emergency_stack_base: .space %0
      .globl riscv64_emergency_stack_top
      .hidden riscv64_emergency_stack_top
      riscv64_emergency_stack_top:
      .popsection
      )"""
      :
      : "i"(kMachineStack.size_bytes));
#if __has_feature(shadow_call_stack)
  __asm__ volatile(
      R"""(
      .pushsection .bss, "aw?", %%nobits
      .balign 8
      .globl riscv64_emergency_shadow_call_stack_base
      .hidden riscv64_emergency_shadow_call_stack_base
      riscv64_emergency_shadow_call_stack_base: .space %0
      .globl riscv64_emergency_shadow_call_stack_top
      .hidden riscv64_emergency_shadow_call_stack_top
      riscv64_emergency_shadow_call_stack_top:
      .popsection
      )"""
      :
      : "i"(kShadowCallStack.size_bytes));
#endif
  rust_riscv64_emergency_exception(iframe, pc, status, cause);
}

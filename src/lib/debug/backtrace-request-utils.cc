// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/debug/backtrace-request-utils.h"

#include <zircon/syscalls.h>

#include "src/lib/debug/backtrace-request.h"

namespace {

uint64_t get_swbreak_argument(const zx_thread_state_general_regs_t* regs) {
#if defined(__x86_64__)
  return regs->rax;
#elif defined(__aarch64__)
  return regs->r[0];
#elif defined(__riscv)
  return regs->a0;
#endif
}

bool have_swbreak_magic(const zx_thread_state_general_regs_t* regs) {
  uint64_t arg = get_swbreak_argument(regs);
  return arg == BACKTRACE_REQUEST_MAGIC_ALL_THREADS ||
         arg == BACKTRACE_REQUEST_MAGIC_CURRENT_THREAD ||
         arg == BACKTRACE_REQUEST_MAGIC_SPECIFIC_THREAD;
}

}  // namespace

// TODO: consider disabling this feature for non-development builds.
bool is_backtrace_request(zx_excp_type_t excp_type, const zx_thread_state_general_regs_t* regs) {
  return excp_type == ZX_EXCP_SW_BREAKPOINT && regs != nullptr && have_swbreak_magic(regs);
}

bool is_backtrace_request_current_thread(const zx_thread_state_general_regs_t* regs) {
  return get_swbreak_argument(regs) == BACKTRACE_REQUEST_MAGIC_CURRENT_THREAD;
}

bool is_backtrace_request_specific_thread(const zx_thread_state_general_regs_t* regs) {
  return get_swbreak_argument(regs) == BACKTRACE_REQUEST_MAGIC_SPECIFIC_THREAD;
}

uint64_t get_backtrace_request_koid(const zx_thread_state_general_regs_t* regs) {
#if defined(__x86_64__)
  return regs->rbx;
#elif defined(__aarch64__)
  return regs->r[1];
#elif defined(__riscv)
  return regs->a1;
#endif
}

zx_status_t cleanup_backtrace_request(zx_handle_t thread, zx_thread_state_general_regs_t* regs) {
#if defined(__x86_64__)
  // On x86, the pc is left at one past the s/w break insn,
  // so there's nothing more we need to do.
  return ZX_OK;
#elif defined(__aarch64__)
  // Skip past the brk instruction.
  regs->pc += 4;
  return zx_thread_write_state(thread, ZX_THREAD_STATE_GENERAL_REGS, regs, sizeof(*regs));
#elif defined(__riscv)
  // Skip past the EBREAK instruction. This assumes that the instruction was compressed, which is
  // a safe assumption because `backtrace-request.h` explicitly uses `C.EBREAK`.
  regs->pc += 2;
  return zx_thread_write_state(thread, ZX_THREAD_STATE_GENERAL_REGS, regs, sizeof(*regs));
#else
  return ZX_ERR_NOT_SUPPORTED;
#endif
}

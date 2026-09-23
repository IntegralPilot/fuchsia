// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_CAPTIVE_THREAD_FAKE_STEP_H_
#define SRC_LIB_CAPTIVE_THREAD_FAKE_STEP_H_

#include <lib/captive-thread/captive-thread.h>
#include <lib/stdcompat/inplace_vector.h>
#include <zircon/assert.h>
#include <zircon/syscalls/debug.h>
#include <zircon/syscalls/exception.h>

#include <cstdint>
#include <type_traits>

namespace captive_thread {

class CaptiveThread::FakeStep {
 public:
  static constexpr bool kImplemented =
#ifdef __riscv
      true
#else
      false
#endif
      ;

  ~FakeStep() { ZX_DEBUG_ASSERT(breakpoints_.empty()); }

  // Set breakpoints at all possible successor instructions to the current PC.
  zx::result<> SetBreakpoints(const zx_thread_state_general_regs_t&);

  // Undo what SetBreakpoints() did.
  zx::result<> ClearBreakpoints();

  // If the exception looks like a breakpoint hit, see if it's one set by the
  // last SetBreakpoints() call.  If so, mutate the exception into looking like
  // a single-step exception.
  void FixupException(CaptiveThread& thread, zx_exception_report_t& report) const;

 private:
#ifdef __riscv_c
  static constexpr uint16_t kBreakpointInsn = 0x9002;  // c.ebreak
#else
  static constexpr uint32_t kBreakpointInsn = 0x00100073;  // ebreak
#endif
  using BreakpointInsn = std::decay_t<decltype(kBreakpointInsn)>;

  struct Breakpoint {
    uintptr_t location;
    BreakpointInsn saved;
  };

  cpp26::inplace_vector<Breakpoint, 2> breakpoints_;
};

}  // namespace captive_thread

#endif  // SRC_LIB_CAPTIVE_THREAD_FAKE_STEP_H_

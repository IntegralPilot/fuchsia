// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zx/vmo.h>
#include <zircon/status.h>
#include <zircon/syscalls-next.h>
#include <zircon/syscalls.h>
#include <zircon/testonly-syscalls.h>
#include <zircon/types.h>

#include <perftest/perftest.h>

#include "assert.h"

#if defined(__x86_64__) || defined(__aarch64__)
#if defined(__x86_64__)
asm(R"(
.globl vectab
vectab:
  // back from restricted mode
  // rdi holds the context
  // rsi holds the error code
  mov  %rdi,%rsp
  pop  %rsp
  pop  %r15
  pop  %r14
  pop  %r13
  pop  %r12
  pop  %rbp
  pop  %rbx

  // pop the error code return slot
  pop  %rdx

  // return the error code from this function
  mov  %rax,(%rdx)

  // return back to whatever the address was on the stack
  // make it appear as if the wrapper had returned ZX_OK
  xor  %rax,%rax
  ret
)");

asm(R"(
.globl bounce
bounce:
  // write rcx and r11 to fs and gs base since they are both
  // trashed by the syscall. also tests that fs and gs base are
  // set properly.
  mov   %rcx, %fs:0
  mov   %r11, %gs:0

0:
  syscall
.globl bounce_post_syscall
bounce_post_syscall:
  jmp 0b
)");

asm(R"(
.globl restricted_enter_wrapper
restricted_enter_wrapper:
  // args 0 - 1 are already in place in rdi, rsi

  // save the return code pointer on the stack
  push  %rdx

  // save the callee saved regs since the return from restricted mode
  // will zero out all of the registers except rdi and rsi
  push  %rbx
  push  %rbp
  push  %r12
  push  %r13
  push  %r14
  push  %r15
  push  %rsp

  // save the pointer the stack as the context pointer in the syscall
  mov   %rsp,%rdx

  // call the syscall
  call  zx_restricted_enter

  // if we got here it must have failed
  add   $(8*8),%rsp // pop the previous state on the stack
  ret
)");

asm(R"(
.globl generate_exception
generate_exception:
  ud2
)");
#elif defined(__aarch64__)
asm(R"(
.globl vectab
vectab:
  // Back from restricted mode
  // x0 holds the context, which is the stack pointer
  // x1 holds the reason code

  // Restore the stack pointer at the point of the restricted enter wrapper.
  mov  sp,x0

  // Load the frame pointer and return address from the wrapper.
  ldp x30, x29, [sp], #16

  // Restore the callee saved registers.
  ldp x28, x27, [sp], #16
  ldp x26, x25, [sp], #16
  ldp x24, x23, [sp], #16
  ldp x22, x21, [sp], #16
  ldp x20, x19, [sp], #16

  // Load the shadow call stack pointer and reason code pointer.
  ldp x2, x18, [sp], #16

  // Return the reason code from this function by setting the reason code pointer.
  str  x1, [x2]

  // Return back to whatever the address was in the link register.
  // Make it appear as if the wrapper had returned ZX_OK
  mov  x0, xzr
  ret
)");

asm(R"(
.globl bounce
bounce:
  mov x16, xzr
  add x16, x16, #64
.Lsyscall:
  svc #0
  b .Lsyscall
)");

asm(R"(
.globl restricted_enter_wrapper
restricted_enter_wrapper:
  // Args 0 - 1 are already in place in X0 and X1.

  // Save the reason code pointer and shadow call stack pointer on the stack.
  stp x2, x18, [sp, #-16]!

  // Save the callee saved regs since the return from restricted mode
  // will modify all registers.
  stp x20, x19, [sp, #-16]!
  stp x22, x21, [sp, #-16]!
  stp x24, x23, [sp, #-16]!
  stp x26, x25, [sp, #-16]!
  stp x28, x27, [sp, #-16]!

  // Save the frame pointer and return address to the stack.
  stp x30, x29, [sp, #-16]!

  // Pass the stack pointer as the context argument to the syscall.
  mov x2, sp

  bl zx_restricted_enter

  // if we got here it must have failed
  add  sp, sp, #(14*8) // pop the previous state on the stack
  ret
)");

asm(R"(
.globl generate_exception
generate_exception:
  udf #0
)");
#endif

extern "C" void vectab();
extern "C" void bounce();
extern "C" void generate_exception();
extern "C" zx_status_t restricted_enter_wrapper(uint32_t options, uintptr_t vector_table,
                                                uint64_t* exit_code);

class RestrictedState {
 public:
  RestrictedState(uintptr_t pc) {
    // Create a VMO and bind it to the current thread.
    zx::vmo vmo;
    ASSERT_OK(zx_restricted_bind_state(0, vmo.reset_and_get_address(), nullptr));

#if defined(__x86_64__)
    state_.ip = pc;
    state_.flags = 0;
    state_.fs_base = reinterpret_cast<uintptr_t>(&fs_val_);
    state_.gs_base = reinterpret_cast<uintptr_t>(&gs_val_);
#elif defined(__aarch64__)
    state_.pc = pc;
    state_.tpidr_el0 = reinterpret_cast<uintptr_t>(&tls_val_);
#endif

    // Set the state
    ASSERT_OK(vmo.write(&state_, 0, sizeof(state_)));
  }

  ~RestrictedState() { ASSERT_OK(zx_restricted_unbind_state(0)); }

 private:
#if defined(__x86_64__)
  uint64_t fs_val_ = 0;
  uint64_t gs_val_ = 0;
#elif defined(__aarch64__)
  uint64_t tls_val_ = 0;
#endif
  zx_restricted_state state_{};
};

bool RestrictedEnterAndExitViaSyscall(perftest::RepeatState* state) {
  RestrictedState restricted_state(reinterpret_cast<uintptr_t>(bounce));
  uint64_t exit_code;
  while (state->KeepRunning()) {
    ASSERT_OK(restricted_enter_wrapper(0, reinterpret_cast<uintptr_t>(vectab), &exit_code));
  }
  return true;
}

bool RestrictedEnterAndExitViaException(perftest::RepeatState* state) {
  RestrictedState restricted_state(reinterpret_cast<uintptr_t>(generate_exception));
  uint64_t exit_code;
  while (state->KeepRunning()) {
    ASSERT_OK(restricted_enter_wrapper(0, reinterpret_cast<uintptr_t>(vectab), &exit_code));
  }
  return true;
}

void RegisterTests() {
  perftest::RegisterTest("RestrictedEnterAndExitViaSyscall", RestrictedEnterAndExitViaSyscall);
  perftest::RegisterTest("RestrictedEnterAndExitViaException", RestrictedEnterAndExitViaException);
}

PERFTEST_CTOR(RegisterTests)

#endif  // __x86_64__ || __aarch64

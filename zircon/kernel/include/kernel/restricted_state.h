// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_KERNEL_RESTRICTED_STATE_H_
#define ZIRCON_KERNEL_INCLUDE_KERNEL_RESTRICTED_STATE_H_

#include <assert.h>
#include <lib/user_copy/user_ptr.h>
#include <lib/zx/result.h>
#include <stddef.h>
#include <stdint.h>
#include <zircon/syscalls-next.h>

#include <arch/exception.h>
#include <arch/regs.h>
#include <ktl/type_traits.h>
#include <ktl/unique_ptr.h>

// Encapsulates a thread's restricted mode state.
//
// The underlying state is allocated and managed entirely in Rust
// (`zircon/kernel/kernel/restricted_state.rs`). In C++, `RestrictedState` is an incomplete struct
// type serving as an opaque handle so that C++ code can safely pass around strongly-typed
// `RestrictedState*` pointers without exposing Rust-side layout details. Lifecycle management is
// handled via `RestrictedStatePtr` (`ktl::unique_ptr<RestrictedState, RestrictedStateDeleter>`),
// which dispatches to `rust_restricted_state_destroy` upon destruction.
struct RestrictedState;
struct Thread;

extern "C" {

RestrictedState* cpp_thread_current_restricted_state();
void cpp_thread_current_set_restricted_state(RestrictedState* raw_rs);
bool cpp_thread_current_is_signaled();
bool cpp_thread_current_check_for_restricted_kick();
bool cpp_thread_is_in_restricted_mode(Thread* thread);
void cpp_vmm_set_active_aspace_normal();
void cpp_vmm_set_active_aspace_restricted();

void rust_restricted_state_destroy(RestrictedState* ptr);
bool rust_restricted_state_in_restricted(const RestrictedState* state);

void rust_redirect_restricted_exception_to_normal_mode(RestrictedState* rs,
                                                       const zx_exception_report_t* report);
[[noreturn]] void rust_restricted_leave_iframe(const iframe_t* iframe,
                                               zx_restricted_reason_t reason);
[[noreturn]] void rust_restricted_leave_syscall(const syscall_regs_t* regs,
                                                zx_restricted_reason_t reason);

}  // extern "C"

// Custom deleter for Rust-allocated RestrictedState.
struct RestrictedStateDeleter {
  void operator()(RestrictedState* ptr) const {
    if (ptr) {
      rust_restricted_state_destroy(ptr);
    }
  }
};

using RestrictedStatePtr = ktl::unique_ptr<RestrictedState, RestrictedStateDeleter>;

#endif  // ZIRCON_KERNEL_INCLUDE_KERNEL_RESTRICTED_STATE_H_

// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_KERNEL_RESTRICTED_H_
#define ZIRCON_KERNEL_INCLUDE_KERNEL_RESTRICTED_H_

#include <lib/user_copy/user_ptr.h>

#include <arch/exception.h>
#include <arch/regs.h>
#include <kernel/restricted_state.h>

// Routines to support restricted mode.

// Called as part of a synchronous exception that is to be handled by the normal mode in-thread
// exception handler.
//
// Specifically this function will:
//   1. Copy the exception report into either the restricted mode state VMO or to the user_ptr
//      provided during the bind_state call.
//
//   2. Update the exception `iframe_t` on the stack such that the thread will return from this
//      exception to normal mode instead of restricted mode. The restricted `reason code` will be
//      set to `ZX_RESTRICTED_REASON_EXCEPTION` or, in the case that a user-copy was attempted and
//      failed, `ZX_RESTRICTED_REASON_EXCEPTION_LOST`.
//
// The caller must ensure that this is only called on a thread that is in restricted mode.
void RedirectRestrictedExceptionToNormalMode(RestrictedState* rs,
                                             const zx_exception_report_t& report);

// Leave restricted mode on the current thread and return to normal mode.
//
// There are two variants of this function, one for each way a thread may have entered kernel mode
// (interrupt or syscall).
//
// These routines do not return normally and instead enter userspace in normal mode. They also
// cannot fail and require that interrupts be disabled before calling them.
[[noreturn]] void RestrictedLeaveIframe(const iframe_t* iframe, zx_restricted_reason_t reason);
[[noreturn]] void RestrictedLeaveSyscall(const syscall_regs_t* regs, zx_restricted_reason_t reason);

// Dispatched directly from arch-specific syscall handler. Called after saving state
// on the stack, but before trying to dispatch as a zircon syscall.
extern "C" [[noreturn]] void syscall_from_restricted(const syscall_regs_t* regs);

#endif  // ZIRCON_KERNEL_INCLUDE_KERNEL_RESTRICTED_H_

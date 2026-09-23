// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <arch.h>
#include <lib/ktrace.h>
#include <zircon/types.h>

#include <kernel/restricted.h>
#include <kernel/restricted_state.h>

void RedirectRestrictedExceptionToNormalMode(RestrictedState* rs,
                                             const zx_exception_report_t& report) {
  rust_redirect_restricted_exception_to_normal_mode(rs, &report);
}

[[noreturn]] void RestrictedLeaveIframe(const iframe_t* iframe, zx_restricted_reason_t reason) {
  rust_restricted_leave_iframe(iframe, reason);
  __UNREACHABLE;
}

[[noreturn]] void RestrictedLeaveSyscall(const syscall_regs_t* regs,
                                         zx_restricted_reason_t reason) {
  rust_restricted_leave_syscall(regs, reason);
  __UNREACHABLE;
}

// This works around a bug in the GCC build where all string categories have to be referenced
// somewhere in C++ in order for the interned string category registration to work.
void UnusedGccWorkaroundKtraceRegistration() {
  KTRACE_DURATION_BEGIN("kernel:restricted", "unused");
}

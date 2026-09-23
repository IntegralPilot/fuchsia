// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <kernel/cpu.h>
#include <kernel/ffi.h>
#include <kernel/scheduler.h>

extern "C" {

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE cpu_mask_t cpp_scheduler_peek_active_mask() {
  return Scheduler::PeekActiveMask();
}

}  // extern "C"

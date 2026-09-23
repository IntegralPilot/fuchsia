// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "vm/scanner_ffi.h"

#include <zircon/types.h>

#include <kernel/ffi.h>

#include "vm/scanner.h"

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
extern "C" {

FFI_ALWAYS_INLINE void cpp_scanner_push_disable_count(void) { scanner_push_disable_count(); }

FFI_ALWAYS_INLINE void cpp_scanner_pop_disable_count(void) { scanner_pop_disable_count(); }

FFI_ALWAYS_INLINE bool cpp_scanner_needs_accessed_scan(zx_instant_mono_t update_time) {
  return scanner_needs_accessed_scan(update_time);
}

FFI_ALWAYS_INLINE void cpp_scanner_wait_for_accessed_scan(zx_instant_mono_t update_time) {
  scanner_wait_for_accessed_scan(update_time);
}

}  // extern "C"

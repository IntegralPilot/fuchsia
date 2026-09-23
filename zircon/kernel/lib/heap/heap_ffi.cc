// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "lib/heap_ffi.h"

#include <lib/heap.h>

#include <kernel/ffi.h>

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
extern "C" {

FFI_ALWAYS_INLINE uint32_t cpp_profile_track_alloc(size_t size) {
  return profile_track_alloc(size);
}

FFI_ALWAYS_INLINE void cpp_profile_track_free(uint32_t handle, size_t size) {
  profile_track_free(handle, size);
}

}  // extern "C"

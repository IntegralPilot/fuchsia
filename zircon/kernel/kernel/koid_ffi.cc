// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <kernel/ffi.h>
#include <kernel/koid.h>

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
extern "C" {

zx_koid_t cpp_koid_generate();

FFI_ALWAYS_INLINE zx_koid_t cpp_koid_generate() { return KernelObjectId::Generate(); }

}  // extern "C"

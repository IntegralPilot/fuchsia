// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "vm/evictor_ffi.h"

#include <kernel/ffi.h>
#include <ktl/memory.h>

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
extern "C" {

FFI_ALWAYS_INLINE void cpp_evictor_destroy(Evictor* evictor) { ktl::destroy_at(evictor); }
FFI_ALWAYS_INLINE void cpp_evictor_init(ffi::Uninitialized<Evictor>* evictor) {
  evictor->Initialize();
}

}  // extern "C"

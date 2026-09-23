// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <arch/arm64/periphmap.h>
#include <dev/power/iris/init.h>
#include <kernel/ffi.h>

extern "C" {

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE uintptr_t cpp_iris_get_opp_vaddr() {
  return reinterpret_cast<uintptr_t>(periph_paddr_to_vaddr(0x200c0790));
}

}  // extern "C"

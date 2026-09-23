// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "vm/physmap_ffi.h"

#include <zircon/types.h>

#include <kernel/ffi.h>
#include <vm/physmap.h>

extern "C" {

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_vaddr_t cpp_paddr_to_physmap(zx_paddr_t paddr) {
  return reinterpret_cast<zx_vaddr_t>(paddr_to_physmap(paddr));
}

FFI_ALWAYS_INLINE zx_paddr_t cpp_physmap_to_paddr(zx_vaddr_t vaddr) {
  return physmap_to_paddr(reinterpret_cast<const void*>(vaddr));
}

}  // extern "C"

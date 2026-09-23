// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_PHYSMAP_FFI_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_PHYSMAP_FFI_H_

#include <sys/types.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

__BEGIN_CDECLS

zx_vaddr_t cpp_paddr_to_physmap(zx_paddr_t paddr);
zx_paddr_t cpp_physmap_to_paddr(zx_vaddr_t vaddr);

__END_CDECLS

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_PHYSMAP_FFI_H_

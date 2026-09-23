// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_PAGE_FFI_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_PAGE_FFI_H_

#include <zircon/compiler.h>
#include <zircon/types.h>

#include "vm/page.h"

__BEGIN_CDECLS

void rust_vm_page_dump(const vm_page_t* page);
uint64_t rust_vm_page_get_count(vm_page_state state);
void rust_vm_page_add_to_initial_count(vm_page_state state, uint64_t n);

__END_CDECLS

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_PAGE_FFI_H_

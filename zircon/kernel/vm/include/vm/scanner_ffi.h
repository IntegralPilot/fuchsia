// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_SCANNER_FFI_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_SCANNER_FFI_H_

#include <zircon/compiler.h>
#include <zircon/types.h>

__BEGIN_CDECLS

void cpp_scanner_push_disable_count(void);
void cpp_scanner_pop_disable_count(void);
bool cpp_scanner_needs_accessed_scan(zx_instant_mono_t update_time);
void cpp_scanner_wait_for_accessed_scan(zx_instant_mono_t update_time);

__END_CDECLS

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_SCANNER_FFI_H_

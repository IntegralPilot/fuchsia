// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_SYSCALLS_OBJECT_PROPERTY_PRIV_H_
#define ZIRCON_KERNEL_LIB_SYSCALLS_OBJECT_PROPERTY_PRIV_H_

#include <stddef.h>
#include <zircon/types.h>

class Dispatcher;

extern "C" {

// C++ fallback handlers for property topics on dispatchers not yet ported to Rust.
zx_status_t cpp_object_get_property_cpp_types(const Dispatcher* dispatcher, uint32_t property,
                                              void* value, size_t size);

zx_status_t cpp_object_set_property_cpp_types(Dispatcher* dispatcher, uint32_t property,
                                              const void* value, size_t size, zx_rights_t rights);

}  // extern "C"

#endif  // ZIRCON_KERNEL_LIB_SYSCALLS_OBJECT_PROPERTY_PRIV_H_

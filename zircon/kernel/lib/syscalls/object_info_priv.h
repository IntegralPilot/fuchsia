// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_SYSCALLS_OBJECT_INFO_PRIV_H_
#define ZIRCON_KERNEL_LIB_SYSCALLS_OBJECT_INFO_PRIV_H_

#include <stddef.h>
#include <zircon/types.h>

extern "C" {
zx_status_t cpp_object_get_info_cpp_types(zx_handle_t handle, uint32_t topic, void* buffer,
                                          size_t buffer_size, size_t* actual, size_t* avail);
}

#endif  // ZIRCON_KERNEL_LIB_SYSCALLS_OBJECT_INFO_PRIV_H_

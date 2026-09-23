// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_ROOT_RESOURCE_FILTER_ROOT_RESOURCE_FILTER_FFI_H_
#define ZIRCON_KERNEL_LIB_ROOT_RESOURCE_FILTER_ROOT_RESOURCE_FILTER_FFI_H_

#include <stdint.h>
#include <zircon/compiler.h>

using root_resource_filter_range_cb_t = void (*)(void* context, uint64_t base, uint64_t size);

extern "C" void cpp_root_resource_filter_populate_finalized_ranges(
    void* context, root_resource_filter_range_cb_t callback);

#endif  // ZIRCON_KERNEL_LIB_ROOT_RESOURCE_FILTER_ROOT_RESOURCE_FILTER_FFI_H_

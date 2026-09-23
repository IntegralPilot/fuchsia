// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_ZIRCON_TESTS_RESTRICTED_MODE_SHARED_HELPERS_HELPERS_H_
#define SRC_ZIRCON_TESTS_RESTRICTED_MODE_SHARED_HELPERS_HELPERS_H_

#include <lib/stdcompat/span.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/syscalls-next.h>

// Because zx_restricted_enter doesn't return like normal syscalls,
// we use a wrapper function to make it easier to use.
extern "C" zx_status_t restricted_enter_wrapper(uint32_t options,
                                                zx_restricted_reason_t* reason_code);

// Maps the code blob into the restricted VMAR with executable permissions.
zx::result<zx_vaddr_t> SetupCodeSegment(zx_handle_t restricted_vmar_handle,
                                        cpp20::span<const std::byte> code_blob);

// Sets up a stack of the given size in the restricted VMAR. On success, stack_vmo is set to the
// VMO holding the stack.
zx::result<zx_vaddr_t> SetupStack(zx_handle_t restricted_vmar_handle, size_t size,
                                  zx::vmo* stack_vmo);

#endif  // SRC_ZIRCON_TESTS_RESTRICTED_MODE_SHARED_HELPERS_HELPERS_H_

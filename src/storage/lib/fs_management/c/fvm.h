// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_FS_MANAGEMENT_C_FVM_H_
#define SRC_STORAGE_LIB_FS_MANAGEMENT_C_FVM_H_

#include <lib/zx/result.h>

extern "C" {

// Initialize an fvm partition on the device backed by |handle|. This function does not take
// ownership of the handle. It should be the equivalent of a
// fidl::UnownedClientEnd<fidl_fuchsia_storage_block::Block>.
//
// This function is primarily for use with the FFI. When using C++ directly, use the functions in
// fs_management/cpp/fvm.h instead, which provide clearer ownership semantics.
zx_status_t fvm_init(zx_handle_t device, size_t slice_size);
}

#endif  // SRC_STORAGE_LIB_FS_MANAGEMENT_C_FVM_H_

// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/fs_management/c/fvm.h"

#include "src/storage/lib/fs_management/cpp/fvm.h"

zx_status_t fvm_init(zx_handle_t device, size_t slice_size) {
  return fs_management::FvmInit(fidl::UnownedClientEnd<fuchsia_storage_block::Block>(device),
                                slice_size);
}

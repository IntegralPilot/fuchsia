// Copyright 2025 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_ZXIO_HANDLE_HOLDER_H_
#define LIB_ZXIO_HANDLE_HOLDER_H_

#include <lib/zx/handle.h>
#include <lib/zxio/types.h>

namespace zxio {

// Initialize a zxio object into |storage| that holds on to |handle|.  This
// object support closing the handle via zxio_destroy and releasing the contained
// handle via zxio_release.
void handle_holder_init(zxio_storage_t* storage, zx::handle handle);

}  // namespace zxio

#endif  // LIB_ZXIO_HANDLE_HOLDER_H_

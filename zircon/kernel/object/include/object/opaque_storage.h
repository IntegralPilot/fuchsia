// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_OPAQUE_STORAGE_H_
#define ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_OPAQUE_STORAGE_H_

#include <stddef.h>

#include <ktl/byte.h>

template <size_t Size, size_t Align>
struct alignas(Align) OpaqueStorage {
  ktl::byte bytes[Size];
};

#endif  // ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_OPAQUE_STORAGE_H_

// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <array>
#include <concepts>
#include <cstddef>

#include <zxtest/zxtest.h>

#include "src/__support/common.h"

namespace LIBC_NAMESPACE_DECL {

// Verify that default construction is the same as no construction (when the
// initial storage bytes are already zero).  Even if it's not technically
// trivial, if all zero bytes is what it produces, then it's fine.
template <std::default_initializable T>
inline void ExpectSafeZeroConstruction() {
  using Bytes = std::array<std::byte, sizeof(T)>;

  alignas(T) Bytes storage{};
  new (storage.data()) T();

  static constexpr Bytes kZero{};
  EXPECT_BYTES_EQ(storage.data(), kZero.data(), kZero.size());
}

}  // namespace LIBC_NAMESPACE_DECL

// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ZXDUMP_BUFFER_IMPL_H_
#define SRC_LIB_ZXDUMP_BUFFER_IMPL_H_

#include <lib/zxdump/buffer.h>

#include <vector>

namespace zxdump::internal {

// BufferImplVector is just a std::vector holding the data.

using ByteVector = std::vector<std::byte>;

class BufferImplVector final : public BufferImpl, public ByteVector {
 public:
  using ByteVector::ByteVector;

  BufferImplVector(BufferImplVector&&) noexcept = default;

  explicit BufferImplVector(ByteVector&& other) : ByteVector(std::move(other)) {}

  using ByteVector::operator=;

  BufferImplVector& operator=(BufferImplVector&&) noexcept = default;

  ~BufferImplVector() override;
};

}  // namespace zxdump::internal

#endif  // SRC_LIB_ZXDUMP_BUFFER_IMPL_H_

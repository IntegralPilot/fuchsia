// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/buffer/resizeable_vmo_buffer.h"

#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <cstddef>
#include <string_view>
#include <utility>

#include "src/storage/lib/buffer/vmoid_registry.h"

namespace storage {

zx::result<> ResizeableVmoBuffer::Attach(std::string_view name, storage::VmoidRegistry& device) {
  ZX_DEBUG_ASSERT(!vmoid_.IsAttached());
  if (zx::result<> result = mapped_vmo_.CreateAndMap(block_size_, name); result.is_error()) {
    return result.take_error();
  }
  return zx::make_result(device.BlockAttachVmo(mapped_vmo_.vmo(), &vmoid_));
}

zx::result<> ResizeableVmoBuffer::Detach(storage::VmoidRegistry& device) {
  return zx::make_result(device.BlockDetachVmo(std::move(vmoid_)));
}

zx_status_t ResizeableVmoBuffer::Zero(size_t index, size_t count) {
  return mapped_vmo_.vmo().op_range(ZX_VMO_OP_ZERO, index * BlockSize(), count * BlockSize(),
                                    nullptr, 0);
}

}  // namespace storage

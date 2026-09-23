// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/block_client/cpp/block_device.h"

namespace block_client {

zx_status_t BlockDevice::BlockDetachVmo(storage::Vmoid vmoid) {
  if (!vmoid.IsAttached()) {
    return ZX_OK;
  }
  BlockFifoRequest request = {};
  request.command = {.opcode = BLOCK_OPCODE_CLOSE_VMO, .flags = 0};
  request.vmoid = vmoid.TakeId();
  return FifoTransaction(&request, 1);
}

}  // namespace block_client

// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_NVME_IO_COMMAND_H_
#define SRC_DEVICES_BLOCK_DRIVERS_NVME_IO_COMMAND_H_

#include <lib/fit/function.h>
#include <zircon/assert.h>
#include <zircon/listnode.h>
#include <zircon/types.h>

#include <optional>

#include "src/storage/lib/block_server/block_server.h"

namespace nvme {

class Namespace;

struct IoCommand {
  void Complete(zx_status_t status) {
    ZX_ASSERT(completion_cb);
    completion_cb(status);
  }

  block_server::Operation operation;
  zx::unowned<zx::vmo> vmo;
  fit::function<void(zx_status_t)> completion_cb;

  uint32_t namespace_id;
  uint32_t block_size_bytes;

  std::optional<block_server::RequestId> request_id;
  Namespace* ns;

  list_node_t node;
};

}  // namespace nvme

#endif  // SRC_DEVICES_BLOCK_DRIVERS_NVME_IO_COMMAND_H_

// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/vfs/cpp/transaction/transaction_handler.h"

#include <zircon/types.h>

#include "src/storage/lib/buffer/block_buffer.h"
#include "src/storage/lib/operation/operation.h"

namespace fs {

zx_status_t TransactionHandler::RunOperation(const storage::Operation& operation,
                                             storage::BlockBuffer* buffer) {
  return RunRequests({storage::BufferedOperation{
#ifdef __Fuchsia__
      .vmoid = buffer->vmoid(),
#else
      .data = buffer->Data(0),
#endif
      .op = operation}});
}

}  // namespace fs

// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_BLOCK_CLIENT_CPP_CLIENT_H_
#define SRC_STORAGE_LIB_BLOCK_CLIENT_CPP_CLIENT_H_

#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/zx/fifo.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/types.h>

#include <condition_variable>
#include <mutex>

#include "src/storage/lib/block_protocol/block-fifo.h"
#include "src/storage/lib/buffer/owned_vmoid.h"
#include "src/storage/lib/buffer/vmoid_registry.h"

namespace block_client {

// Provides a simple synchronous wrapper for talking to a block device over a FIFO.
//
// Block devices can support several (MAX_TXN_GROUP_COUNT) requests in-flight at once and this class
// is threadsafe to support this many requests from different threads in parallel. Exceeding
// MAX_TXN_GROUP_COUNT parallel transactions will block future requests until a transaction group
// becomes available.
//
// When the Client is destroyed, its session will be synchronously closed.
class Client : public storage::VmoidRegistry {
 public:
  Client(fidl::ClientEnd<fuchsia_storage_block::Session> session, zx::fifo fifo);
  ~Client();

  zx_status_t BlockAttachVmo(const zx::vmo& vmo, storage::Vmoid* out) override;
  zx_status_t BlockDetachVmo(storage::Vmoid vmoid) override;

  zx::result<storage::OwnedVmoid> RegisterVmo(const zx::vmo& vmo);

  // Issues a group of block requests over the underlying fifo, and waits for a response.
  zx_status_t Transaction(BlockFifoRequest* requests, size_t count);

 private:
  struct BlockCompletion {
    bool in_use = false;
    bool done = false;
    zx_status_t status = ZX_ERR_IO;
  };

  zx_status_t DoRead(BlockFifoResponse* response, size_t* count);
  zx_status_t DoWrite(BlockFifoRequest* request, size_t count);

  const fidl::ClientEnd<fuchsia_storage_block::Session> session_;
  const zx::fifo fifo_;

  std::mutex mutex_;

  BlockCompletion groups_[MAX_TXN_GROUP_COUNT];  // Guarded by mutex.

  std::condition_variable condition_;
  bool reading_ = false;  // Guarded by mutex.
};

}  // namespace block_client

#endif  // SRC_STORAGE_LIB_BLOCK_CLIENT_CPP_CLIENT_H_

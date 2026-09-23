// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/block_client/cpp/client.h"

#include <lib/zx/fifo.h>
#include <stdlib.h>
#include <zircon/assert.h>
#include <zircon/types.h>

#include "src/storage/lib/block_protocol/block-fifo.h"
#include "src/storage/lib/buffer/owned_vmoid.h"

namespace block_client {

Client::Client(fidl::ClientEnd<fuchsia_storage_block::Session> session, zx::fifo fifo)
    : session_(std::move(session)), fifo_(std::move(fifo)) {}

Client::~Client() { [[maybe_unused]] fidl::WireResult result = fidl::WireCall(session_)->Close(); }

zx_status_t Client::BlockAttachVmo(const zx::vmo& vmo, storage::Vmoid* out) {
  zx::vmo dup;
  if (zx_status_t status = vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup); status != ZX_OK) {
    return status;
  }
  const fidl::WireResult result = fidl::WireCall(session_)->AttachVmo(std::move(dup));
  if (!result.ok()) {
    return result.status();
  }
  fit::result response = result.value();
  if (response.is_error()) {
    return response.error_value();
  }
  *out = storage::Vmoid(response->vmoid.id);
  return ZX_OK;
}

zx_status_t Client::BlockDetachVmo(storage::Vmoid vmoid) {
  if (!vmoid.IsAttached()) {
    return ZX_OK;
  }
  BlockFifoRequest request = {};
  request.command = {.opcode = BLOCK_OPCODE_CLOSE_VMO, .flags = 0};
  request.vmoid = vmoid.TakeId();
  return Transaction(&request, 1);
}

zx::result<storage::OwnedVmoid> Client::RegisterVmo(const zx::vmo& vmo) {
  storage::Vmoid vmoid;
  if (zx_status_t status = BlockAttachVmo(vmo, &vmoid); status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(storage::OwnedVmoid(std::move(vmoid), this));
}

zx_status_t Client::Transaction(BlockFifoRequest* requests, size_t count) {
  if (count == 0)
    return ZX_OK;

  // Find a group we can use.
  BlockCompletion* block_completion = nullptr;
  groupid_t group;
  {
    std::unique_lock<std::mutex> lock(mutex_);
    for (;;) {
      for (group = 0; group < MAX_TXN_GROUP_COUNT && groups_[group].in_use; ++group) {
      }
      if (group < MAX_TXN_GROUP_COUNT)
        break;  // Found a free one.

      // No free groups so wait.
      condition_.wait(lock);
    }
    block_completion = &groups_[group];
    block_completion->in_use = true;
    block_completion->done = false;
    block_completion->status = ZX_ERR_IO;
  }

  // Avoid using 0 to mitigate accidents where the wrong reqid is used.
  requests[count - 1].reqid = group + 1;

  if (count > 1) {
    for (size_t i = 0; i < count; i++) {
      requests[i].group = group;
      requests[i].command.flags |= BLOCK_IO_FLAG_GROUP_ITEM;
    }
    requests[count - 1].command.flags |= BLOCK_IO_FLAG_GROUP_LAST;
  }

  if (zx_status_t status = DoWrite(requests, count); status != ZX_OK) {
    {
      std::unique_lock<std::mutex> lock(mutex_);
      block_completion->in_use = false;
    }
    condition_.notify_all();
    return status;
  }

  // As expected by the protocol, when we send one "BLOCK_GROUP_LAST" message, we
  // must read a reply message.
  zx_status_t status = ZX_OK;
  {
    std::unique_lock<std::mutex> lock(mutex_);

    while (!block_completion->done) {
      // Only let one thread do the reading at a time.
      if (!reading_) {
        reading_ = true;

        constexpr size_t kMaxResponseCount = 8;
        BlockFifoResponse response[kMaxResponseCount];
        size_t count = kMaxResponseCount;

        // Unlocked block.
        {
          lock.unlock();
          status = DoRead(response, &count);
          lock = std::unique_lock<std::mutex>(mutex_);
        }
        reading_ = false;

        if (status != ZX_OK) {
          block_completion->in_use = false;
          lock.unlock();
          condition_.notify_all();
          return status;
        }

        // Record all the responses.
        for (size_t i = 0; i < count; ++i) {
          uint32_t reqid = response[i].reqid;
          if (reqid < 1 || reqid > MAX_TXN_GROUP_COUNT || !groups_[reqid - 1].in_use) {
            ZX_DEBUG_ASSERT(false);
            continue;
          }
          --reqid;
          groups_[reqid].status = response[i].status;
          groups_[reqid].done = true;
        }
        condition_.notify_all();  // Signal all threads that might be waiting for responses.
      } else {
        condition_.wait(lock);
      }
    }

    // Free the group.
    status = block_completion->status;
    block_completion->in_use = false;
  }
  condition_.notify_all();  // Signal a thread that might be waiting for a free group.

  return status;
}

zx_status_t Client::DoRead(BlockFifoResponse* response, size_t* count) {
  while (true) {
    switch (zx_status_t status = fifo_.read(sizeof(BlockFifoRequest), response, *count, count);
            status) {
      case ZX_ERR_SHOULD_WAIT: {
        zx_signals_t signals;
        if (zx_status_t status = fifo_.wait_one(ZX_FIFO_READABLE | ZX_FIFO_PEER_CLOSED,
                                                zx::time::infinite(), &signals);
            status != ZX_OK) {
          return status;
        }
        continue;
      }
      default:
        return status;
    }
  }
}

zx_status_t Client::DoWrite(BlockFifoRequest* request, size_t count) {
  while (true) {
    size_t actual;
    switch (zx_status_t status = fifo_.write(sizeof(BlockFifoRequest), request, count, &actual);
            status) {
      case ZX_OK:
        count -= actual;
        request += actual;
        if (count == 0) {
          return ZX_OK;
        }
        break;
      case ZX_ERR_SHOULD_WAIT: {
        zx_signals_t signals;
        if (zx_status_t status = fifo_.wait_one(ZX_FIFO_WRITABLE | ZX_FIFO_PEER_CLOSED,
                                                zx::time::infinite(), &signals);
            status != ZX_OK) {
          return status;
        }
        continue;
      }
      default:
        return status;
    }
  }
}

}  // namespace block_client

// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/minfs/bcache.h"

#include <assert.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/syslog/cpp/macros.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <zircon/compiler.h>
#include <zircon/errors.h>

#include <atomic>
#include <utility>

#include <fbl/alloc_checker.h>
#include <fbl/ref_ptr.h>

#include "src/storage/lib/buffer/block_buffer.h"
#include "src/storage/lib/buffer/vmo_buffer.h"
#include "src/storage/lib/operation/operation.h"
#include "src/storage/lib/trace/trace.h"
#include "src/storage/minfs/format.h"
#include "zircon/assert.h"
#include "zircon/status.h"

namespace minfs {

std::unique_ptr<block_client::BlockDevice> Bcache::Destroy(std::unique_ptr<Bcache> bcache) {
  {
    // Destroy the VmoBuffer before extracting the underlying device, as it needs
    // to de-register itself from the underlying block device to be terminated.
    [[maybe_unused]] auto unused = std::move(bcache->buffer_);
  }
  return std::move(bcache->owned_device_);
}

zx::result<> Bcache::Readblk(blk_t bno, void* data) {
  TRACE_DURATION("minfs", "Bcache::Readblk", "blk", bno);
  storage::Operation operation = {};
  operation.type = storage::OperationType::kRead;
  operation.vmo_offset = 0;
  operation.dev_offset = bno;
  operation.length = 1;
  zx_status_t status = RunOperation(operation, &buffer_);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  memcpy(data, buffer_.Data(0), kMinfsBlockSize);
  return zx::ok();
}

void Bcache::DieOnMutationFailure(bool setting) {
  die_on_mutation_failure_.store(setting, std::memory_order::relaxed);
}

zx_status_t Bcache::RunRequests(const std::vector<storage::BufferedOperation>& operations) {
  zx_status_t status;
  {
    std::shared_lock lock(mutex_);
    status = DeviceTransactionHandler::RunRequests(operations);
  }
#ifdef __Fuchsia__
  // Any mutation failures can leave the device in an unknown state.
  if (status != ZX_OK && die_on_mutation_failure_.load(std::memory_order_relaxed)) {
    for (const storage::BufferedOperation& op : operations) {
      switch (op.op.type) {
        case storage::OperationType::kWrite:
        case storage::OperationType::kWriteFua:
        case storage::OperationType::kTrim:
          ZX_PANIC("Mutation failure. Disk no longer consistent: %s", zx_status_get_string(status));
          break;
        default:
          break;
      }
    }
  }
#endif
  return status;
}

zx_status_t Bcache::Flush() {
  zx_status_t status;
  {
    std::shared_lock lock(mutex_);
    status = DeviceTransactionHandler::Flush();
  }
#ifdef __Fuchsia__
  if (status != ZX_OK && die_on_mutation_failure_.load(std::memory_order_relaxed)) {
    ZX_PANIC("Flush failure. Disk no longer consistent: %s", zx_status_get_string(status));
  }
#endif
  return status;
}

zx::result<> Bcache::Writeblk(blk_t bno, const void* data) {
  TRACE_DURATION("minfs", "Bcache::Writeblk", "blk", bno);
  storage::Operation operation = {};
  operation.type = storage::OperationType::kWrite;
  operation.vmo_offset = 0;
  operation.dev_offset = bno;
  operation.length = 1;
  memcpy(buffer_.Data(0), data, kMinfsBlockSize);
  return zx::make_result(RunOperation(operation, &buffer_));
}

zx_status_t Bcache::BlockAttachVmo(const zx::vmo& vmo, storage::Vmoid* out) {
  return device()->BlockAttachVmo(vmo, out);
}

zx_status_t Bcache::BlockDetachVmo(storage::Vmoid vmoid) {
  return device()->BlockDetachVmo(std::move(vmoid));
}

zx::result<> Bcache::Sync() {
  BlockFifoRequest request = {};
  request.command = {.opcode = BLOCK_OPCODE_FLUSH, .flags = 0};
  return zx::make_result(device_->FifoTransaction(&request, 1));
}

zx::result<std::unique_ptr<Bcache>> Bcache::Create(
    std::unique_ptr<block_client::BlockDevice> device, uint32_t max_blocks) {
  auto bcache_or = Create(device.get(), max_blocks);
  if (bcache_or.is_ok()) {
    bcache_or->owned_device_ = std::move(device);
  }
  return bcache_or;
}

zx::result<std::unique_ptr<Bcache>> Bcache::Create(block_client::BlockDevice* device,
                                                   uint32_t max_blocks) {
  std::unique_ptr<Bcache> bcache(new Bcache(device, max_blocks));

  if (zx_status_t status =
          bcache->buffer_.Initialize(bcache.get(), 1, kMinfsBlockSize, "scratch-block");
      status != ZX_OK) {
    return zx::error(status);
  }

  if (auto status = bcache->VerifyDeviceInfo(); status.is_error()) {
    return status.take_error();
  }

  return zx::ok(std::move(bcache));
}

uint32_t Bcache::DeviceBlockSize() const { return info_.block_size; }

Bcache::Bcache(block_client::BlockDevice* device, uint32_t max_blocks)
    : max_blocks_(max_blocks), device_(device) {}

zx::result<> Bcache::VerifyDeviceInfo() {
  zx_status_t status = device_->BlockGetInfo(&info_);
  if (status != ZX_OK) {
    FX_LOGS(ERROR) << "cannot get block device information: " << status;
    return zx::error(status);
  }

  if (kMinfsBlockSize % info_.block_size != 0) {
    FX_LOGS(ERROR) << "minfs Block size not multiple of underlying block size: "
                   << info_.block_size;
    return zx::error(ZX_ERR_BAD_STATE);
  }
  return zx::ok();
}

// TODO(https://fxbug.dev/42080556): change this to __TA_ACQUIRE(mutex_) after clang roll.
void Bcache::Pause() __TA_NO_THREAD_SAFETY_ANALYSIS { mutex_.lock(); }

// TODO(https://fxbug.dev/42080556): change this to __TA_RELEASE(mutex_) after clang roll.
void Bcache::Resume() __TA_NO_THREAD_SAFETY_ANALYSIS { mutex_.unlock(); }

}  // namespace minfs

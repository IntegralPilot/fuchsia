// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/block/drivers/virtio/block.h"

#include <fidl/fuchsia.storage.block/cpp/fidl.h>
#include <inttypes.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fit/defer.h>
#include <lib/virtio/driver_utils.h>
#include <lib/zircon-internal/align.h>
#include <lib/zx/clock.h>
#include <lib/zx/time.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/param.h>
#include <zircon/compiler.h>

#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <memory>
#include <mutex>
#include <utility>

#include <fbl/algorithm.h>
#include <fbl/auto_lock.h>

#include "src/devices/block/lib/common/include/common.h"
#include "src/storage/lib/block_server/block_server.h"

#define LOCAL_TRACE 0

namespace virtio {

namespace {

// Cache some page size calculations that are used frequently.
const uint32_t kPageSize = zx_system_get_page_size();
const uint32_t kPageMask = kPageSize - 1;
const uint32_t kMaxMaxXfer = (MAX_SCATTER - 1) * kPageSize;

// See 5.2.2 in the virtio spec
const uint16_t kVirtioBlkRequestQueueIndex = 0;

uint32_t VirtioRequestType(const block_server::Operation& operation) {
  switch (operation.tag) {
    case block_server::Operation::Tag::Read:
      return VIRTIO_BLK_T_IN;
    case block_server::Operation::Tag::Write:
      return VIRTIO_BLK_T_OUT;
    case block_server::Operation::Tag::Trim:
      return VIRTIO_BLK_T_DISCARD;
    case block_server::Operation::Tag::Flush:
      return VIRTIO_BLK_T_FLUSH;
    case block_server::Operation::Tag::CloseVmo:
    case block_server::Operation::Tag::StartDecompressedRead:
    case block_server::Operation::Tag::ContinueDecompressedRead:
      __UNREACHABLE;
  }
}

}  // namespace

void BlockDevice::CompleteTxn(block_txn_t* transaction, zx_status_t status) {
  fdf::trace("Complete txn {} {}", static_cast<const void*>(transaction),
             zx_status_get_string(status));
  if (transaction->pmt != ZX_HANDLE_INVALID) {
    zx_pmt_unpin(transaction->pmt);
    transaction->pmt = ZX_HANDLE_INVALID;
  }
  {
    std::lock_guard<std::mutex> lock(watchdog_lock_);
    blk_req_start_timestamps_[transaction->req_index] = zx::time::infinite();
  }
  // Save the request ID before we release the transaction's resources, because for block server
  // requests, the req_index is used to reserve a slot in our pool for txn, so once we free that,
  // txn could be reused.
  uint64_t request_id = transaction->request;
  {
    fbl::AutoLock lock(&txn_lock_);
    // NB: req_index might be invalid (>= kBlkReqCount) if transaction comes from Banjo but never
    // even made it as far as allocating a req_index.  That's tolerated by FreeBlkReqLocked.
    FreeBlkReqLocked(transaction->req_index);
    if (transaction->discard_req_index) {
      FreeBlkReqLocked(*transaction->discard_req_index);
    }
    list_delete(&transaction->node);
  }
  txn_cond_.Broadcast();
  std::lock_guard lock(block_server_lock_);
  if (block_server_) {
    block_server_->SendReply(request_id, zx::make_result(status));
  }
}

uint32_t BlockDevice::GetMaxTransferSize() const {
  const uint32_t max_transfer_size = static_cast<uint32_t>(kPageSize * (kRingSize - 2));
  // Limit max transfer to our worst case scatter list size.
  return std::min(max_transfer_size, kMaxMaxXfer);
}

fuchsia_storage_block::wire::DeviceFlag BlockDevice::GetFlags() const {
  fuchsia_storage_block::wire::DeviceFlag flags = {};
  if (supports_discard_) {
    flags |= fuchsia_storage_block::wire::DeviceFlag::kTrimSupport;
  }
  return flags;
}
void BlockDevice::OnRequests(std::span<block_server::Request> requests) {
  for (auto& request : requests) {
    if (zx_status_t status = block_server::CheckIoRange(request, config_.capacity);
        status != ZX_OK) {
      fdf::warn("Invalid request range.");
      std::lock_guard lock(block_server_lock_);
      ZX_DEBUG_ASSERT(block_server_);
      if (block_server_) {
        block_server_->SendReply(request.request_id, zx::make_result(status));
      }
      continue;
    }
    zx::result status = SubmitBlockServerRequest(request);
    if (status.is_error()) {
      fdf::warn("Failed to submit request: {}", status);
      std::lock_guard lock(block_server_lock_);
      if (block_server_) {
        block_server_->SendReply(request.request_id, status.take_error());
      }
    }
  }
}

void BlockDevice::ServeRequests(fidl::ServerEnd<fuchsia_storage_block::Block> server_end) {
  std::lock_guard lock(block_server_lock_);
  if (block_server_) {
    block_server_->Serve(std::move(server_end));
  }
}

void BlockDevice::ServeMapperRequests(fidl::ServerEnd<fuchsia_storage_block::Mapper> server_end) {
  std::lock_guard lock(block_server_lock_);
  if (block_server_) {
    block_server_->ServeMapper(std::move(server_end));
  }
}

BlockDevice::BlockDevice(zx::bti bti, std::unique_ptr<Backend> backend, fdf::Logger& logger)
    : virtio::Device(std::move(bti), std::move(backend)), logger_(logger) {
  for (auto& time : blk_req_start_timestamps_) {
    time = zx::time::infinite();
  }
}

zx_status_t BlockDevice::Init() {
  DeviceReset();
  CopyDeviceConfig(&config_, sizeof(config_));

  // TODO(cja): The blk_size provided in the device configuration is only
  // populated if a specific feature bit has been negotiated during
  // initialization, otherwise it is 0, at least in Virtio 0.9.5. Use 512
  // as a default as a stopgap for now until proper feature negotiation
  // is supported.
  if (config_.blk_size == 0)
    config_.blk_size = 512;

  fdf::debug("capacity {:#x}", config_.capacity);
  fdf::debug("size_max {:#x}", config_.size_max);
  fdf::debug("seg_max  {:#x}", config_.seg_max);
  fdf::debug("blk_size {:#x}", config_.blk_size);

  DriverStatusAck();

  uint64_t features = DeviceFeaturesSupported();
  if (!(features & VIRTIO_F_VERSION_1)) {
    // Declaring non-support until there is a need in the future.
    fdf::error("Legacy virtio interface is not supported by this driver");
    return ZX_ERR_NOT_SUPPORTED;
  }
  if (features & VIRTIO_BLK_F_DISCARD) {
    fdf::info("virtio device supports discard");
    supports_discard_ = true;
  }

  if (features & VIRTIO_BLK_F_BARRIER) {
    fdf::info("virtio device supports barriers");
    supports_barriers_ = true;
  }

  features &= (VIRTIO_F_VERSION_1 | VIRTIO_BLK_F_DISCARD);
  DriverFeaturesAck(features);
  if (zx_status_t status = DeviceStatusFeaturesOk(); status != ZX_OK) {
    fdf::error("Feature negotiation failed: {}", zx_status_get_string(status));
    return status;
  }

  // Allocate the main vring.
  auto err = vring_.Init(kVirtioBlkRequestQueueIndex, kRingSize);
  if (err < 0) {
    fdf::error("failed to allocate vring");
    return err;
  }

  // Allocate a queue of block requests.
  size_t size = sizeof(virtio_blk_req_t) * kBlkReqCount + sizeof(uint8_t) * kBlkReqCount;

  auto buffer_factory = dma_buffer::CreateBufferFactory();
  const size_t buffer_size = fbl::round_up(size, zx_system_get_page_size());
  zx_status_t status = buffer_factory->CreateContiguous(
      bti_, buffer_size, 0, dma_buffer::CacheOptions::kEnabled, &blk_req_buf_);
  if (status != ZX_OK) {
    fdf::error("cannot alloc blk_req buffers: {}", zx_status_get_string(status));
    return status;
  }
  blk_req_ = static_cast<virtio_blk_req_t*>(blk_req_buf_->virt());

  fdf::trace("allocated blk request at {}, physical address {:#x}",
             static_cast<const void*>(blk_req_), blk_req_buf_->phys());

  // Responses are 32 words at the end of the allocated block.
  blk_res_pa_ = blk_req_buf_->phys() + sizeof(virtio_blk_req_t) * kBlkReqCount;
  blk_res_ = reinterpret_cast<uint8_t*>(
      (reinterpret_cast<uintptr_t>(blk_req_) + sizeof(virtio_blk_req_t) * kBlkReqCount));

  fdf::trace("allocated blk responses at {}, physical address {:#x}",
             static_cast<const void*>(blk_res_), blk_res_pa_);

  StartIrqThread();
  DriverStatusOk();

  {
    std::lock_guard lock(block_server_lock_);
    block_server_.emplace(
        block_server::PartitionInfo{
            .device_flags = static_cast<uint32_t>(GetFlags()),
            .block_count = GetBlockCount(),
            .block_size = GetBlockSize(),
            .max_transfer_size = GetMaxTransferSize(),
        },
        this);
  }

  auto watchdog_thread_entry = [](void* ctx) {
    auto bd = static_cast<BlockDevice*>(ctx);
    bd->WatchdogThread();
    return ZX_OK;
  };
  int ret = thrd_create_with_name(&watchdog_thread_, watchdog_thread_entry, this,
                                  "virtio-block-watchdog");
  if (ret != thrd_success) {
    return ZX_ERR_INTERNAL;
  }
  return ZX_OK;
}

void BlockDevice::Release() {
  watchdog_shutdown_.store(true);
  sync_completion_signal(&watchdog_signal_);
  thrd_join(watchdog_thread_, nullptr);

  {
    fbl::AutoLock lock(&txn_lock_);
    shutdown_ = true;
  }
  txn_cond_.Broadcast();

  // After `shutdown_ = true`, no more transactions will be created, but we need to clean up those
  // which are already in-flight so we don't block shutdown.
  CleanUpPendingTxns();

  {
    std::lock_guard lock(block_server_lock_);
    block_server_.reset();
  }

  virtio::Device::Release();
}

struct vring_desc* BlockDevice::FreeDescChainLocked(uint16_t index) {
  struct vring_desc* desc = vring_.DescFromIndex(index);
  auto head_desc = desc;  // Save the first element.
  {
    for (;;) {
      std::optional<uint16_t> next;
      if (fdf::Logger::GlobalInstance()->GetSeverity() <= FUCHSIA_LOG_TRACE) {
        virtio_dump_desc(desc);
      }
      if (desc->flags & VRING_DESC_F_NEXT) {
        next = desc->next;
      }

      vring_.FreeDesc(index);

      if (!next) {
        // End of chain
        break;
      }
      index = *next;
      desc = vring_.DescFromIndex(index);
    }
  }
  return head_desc;
}

void BlockDevice::IrqRingUpdate() {
  // Parse our descriptor chain and add back to the free queue.
  auto free_chain = [this](vring_used_elem* used_elem) {
    std::optional<uint8_t> status;
    block_txn_t* txn = nullptr;
    {
      fbl::AutoLock lock(&txn_lock_);
      struct vring_desc* head_desc;
      {
        std::lock_guard<std::mutex> lock2(ring_lock_);
        head_desc = FreeDescChainLocked(static_cast<uint16_t>(used_elem->id));
      }

      // Search our pending txn list to see if this completes it.
      list_for_every_entry (&pending_txn_list_, txn, block_txn_t, node) {
        if (txn->desc == head_desc) {
          fdf::trace("completes txn {}", static_cast<const void*>(txn));
          status = blk_res_[txn->req_index];
          // NB: We can't free the transaction's resources until we complete it, because the
          // req_index is used to allocate requests out of the pool.
          break;
        }
      }
    }

    if (status) {
      zx_status_t zx_status = ZX_ERR_IO;
      switch (*status) {
        case VIRTIO_BLK_S_OK:
          zx_status = ZX_OK;
          break;
        case VIRTIO_BLK_S_IOERR:
          break;
        case VIRTIO_BLK_S_UNSUPP:
          zx_status = ZX_ERR_NOT_SUPPORTED;
      }
      CompleteTxn(txn, zx_status);
    }
  };

  // Tell the ring to find free chains and hand it back to our lambda.
  vring_.IrqRingUpdate(free_chain);
}

void BlockDevice::IrqConfigChange() {}

void BlockDevice::QueueTxn(block_txn_t* txn, RequestContext context) {
  ZX_DEBUG_ASSERT(txn);

  uint32_t type = VirtioRequestType(txn->operation);
  txn->req_index = context.req_index();
  txn->discard_req_index = context.discard_req_index();
  txn->desc = context.desc();
  txn->pmt = context.pmt();

  auto req = &blk_req_[txn->req_index];
  req->type = type;
  req->ioprio = 0;

  if (txn->operation.tag == block_server::Operation::Tag::Write &&
      txn->operation.write.options.flags.is_pre_barrier()) {
    // The block server library should only send barriers down if we advertise support for them.
    ZX_DEBUG_ASSERT(supports_barriers_);
    req->type |= VIRTIO_BLK_T_BARRIER;
  }

  if (req->type == VIRTIO_BLK_T_FLUSH) {
    req->sector = 0;
  } else {
    if (txn->operation.tag == block_server::Operation::Tag::Read) {
      req->sector = txn->operation.read.device_block_offset;
    } else if (txn->operation.tag == block_server::Operation::Tag::Write) {
      req->sector = txn->operation.write.device_block_offset;
    }
  }
  fdf::trace("blk_req type {} ioprio {} sector {}", req->type, req->ioprio, req->sector);

  if (type == VIRTIO_BLK_T_DISCARD) {
    // NOTE: if we decide to later send multiple virtio_blk_discard_write_zeroes at once, we must
    // respect the max_discard_seg configuration of the device.
    static_assert(sizeof(virtio_blk_discard_write_zeroes_t) <= sizeof(virtio_blk_req_t));
    virtio_blk_discard_write_zeroes_t* req =
        reinterpret_cast<virtio_blk_discard_write_zeroes_t*>(&blk_req_[*txn->discard_req_index]);
    req->sector = txn->operation.trim.device_block_offset;
    req->num_sectors = txn->operation.trim.block_count;
    req->flags = 0;
    fdf::trace("blk_dwz_req sector {} num_sectors {}", req->sector, req->num_sectors);
  }
  fdf::trace("page count {}", context.num_pages());

  // Set up the head descriptor.
  struct vring_desc* desc = context.desc();
  desc->addr = blk_req_buf_->phys() + txn->req_index * sizeof(virtio_blk_req_t);
  desc->len = sizeof(virtio_blk_req_t);
  desc->flags = VRING_DESC_F_NEXT;
  if (fdf::Logger::GlobalInstance()->GetSeverity() <= FUCHSIA_LOG_TRACE) {
    virtio_dump_desc(txn->desc);
  }

  size_t bytes = type == VIRTIO_BLK_T_IN || type == VIRTIO_BLK_T_OUT
                     ? txn->operation.read.block_count * config_.blk_size
                     : 0;
  for (size_t n = 0; n < context.num_pages(); n++) {
    desc = vring_.DescFromIndex(desc->next);
    desc->addr = context.pages()[n];  // |pages| are all page-aligned addresses.
    desc->len = static_cast<uint32_t>((bytes > kPageSize) ? kPageSize : bytes);
    if (n == 0) {
      // First entry may not be page aligned.
      size_t page0_offset = txn->operation.read.vmo_offset & kPageMask;

      // Adjust starting address.
      desc->addr += page0_offset;

      // Trim length if necessary.
      size_t max = kPageSize - page0_offset;
      if (desc->len > max) {
        desc->len = static_cast<uint32_t>(max);
      }
    }
    desc->flags = VRING_DESC_F_NEXT;
    fdf::trace("pa {:#x}, len {:#x}", desc->addr, desc->len);

    // Mark buffer as write-only if its a block read.
    if (type == VIRTIO_BLK_T_IN) {
      desc->flags |= VRING_DESC_F_WRITE;
    }

    bytes -= desc->len;
  }
  assert(bytes == 0);

  if (type == VIRTIO_BLK_T_DISCARD) {
    desc = vring_.DescFromIndex(desc->next);
    desc->addr = blk_req_buf_->phys() + *context.discard_req_index() * sizeof(virtio_blk_req_t);
    desc->len = sizeof(virtio_blk_discard_write_zeroes_t);
    desc->flags = VRING_DESC_F_NEXT;
    if (fdf::Logger::GlobalInstance()->GetSeverity() <= FUCHSIA_LOG_TRACE) {
      virtio_dump_desc(desc);
    }
  }

  // Set up the descriptor pointing to the response.
  desc = vring_.DescFromIndex(desc->next);
  desc->addr = blk_res_pa_ + context.req_index();
  desc->len = 1;
  desc->flags = VRING_DESC_F_WRITE;
  if (fdf::Logger::GlobalInstance()->GetSeverity() <= FUCHSIA_LOG_TRACE) {
    virtio_dump_desc(desc);
  }

  {
    std::lock_guard<std::mutex> lock(watchdog_lock_);
    blk_req_start_timestamps_[context.req_index()] = zx::clock::get_monotonic();
  }

  fbl::AutoLock lock(&txn_lock_);
  list_add_tail(&pending_txn_list_, &txn->node);
  vring_.SubmitChain(context.desc_index());
  vring_.Kick();
  fdf::trace("Submitted txn {} (desc {})", static_cast<const void*>(txn), context.desc_index());
  context.Release();
}

zx::result<zx_handle_t> BlockDevice::PinPages(zx_handle_t bti, zx_handle_t vmo,
                                              uint64_t vmo_offset_bytes, uint32_t num_blocks,
                                              std::array<zx_paddr_t, MAX_SCATTER>* pages,
                                              size_t* num_pages) const {
  uint64_t suboffset = vmo_offset_bytes & kPageMask;
  uint64_t aligned_offset = vmo_offset_bytes & ~kPageMask;
  size_t pin_size =
      ZX_ROUNDUP(suboffset + (static_cast<uint64_t>(num_blocks * config_.blk_size)), kPageSize);
  *num_pages = pin_size / kPageSize;
  if (*num_pages > pages->size()) {
    fdf::error("transaction too large");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  zx_handle_t pmt;
  zx_status_t status = zx_bti_pin(bti, ZX_BTI_PERM_READ | ZX_BTI_PERM_WRITE, vmo, aligned_offset,
                                  pin_size, pages->data(), *num_pages, &pmt);
  if (status != ZX_OK) {
    fdf::error("could not pin pages: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  return zx::ok(pmt);
}

zx::result<> BlockDevice::SubmitBlockServerRequest(const block_server::Request& request) {
  uint32_t type = VirtioRequestType(request.operation);

  uint64_t vmo_offset_bytes = 0;
  uint32_t num_blocks = 0;

  switch (request.operation.tag) {
    case block_server::Operation::Tag::Read:
      vmo_offset_bytes = request.operation.read.vmo_offset;
      num_blocks = request.operation.read.block_count;
      break;
    case block_server::Operation::Tag::Write:
      vmo_offset_bytes = request.operation.write.vmo_offset;
      num_blocks = request.operation.write.block_count;
      break;
    case block_server::Operation::Tag::Trim:
      num_blocks = request.operation.trim.block_count;
      break;
    case block_server::Operation::Tag::Flush:
      break;
    case block_server::Operation::Tag::CloseVmo:
    case block_server::Operation::Tag::StartDecompressedRead:
    case block_server::Operation::Tag::ContinueDecompressedRead:
      __UNREACHABLE;
  }

  std::array<zx_paddr_t, MAX_SCATTER> pages;
  zx::result<RequestContext> context =
      AllocateRequestContext(type, request.vmo->get(), vmo_offset_bytes, num_blocks, &pages);
  if (context.is_error()) {
    return context.take_error();
  }
  block_txn_t* txn = &block_server_request_pool_[context->req_index()];

  txn->request = request.request_id;
  txn->operation = request.operation;
  txn->vmo = request.vmo;

  // A flush operation should complete after any in-flight transactions, so wait for all
  // pending txns to complete before submitting a flush txn. This is necessary because a virtio
  // block device may service requests in any order.
  if (type == VIRTIO_BLK_T_FLUSH) {
    if (zx::result result = FlushPendingTxns(); result.is_error()) {
      fbl::AutoLock lock(&txn_lock_);
      std::lock_guard lock2(ring_lock_);
      FreeRequestContext(*context);
      return result.take_error();
    }
  }

  QueueTxn(txn, std::move(*context));

  // A flush operation should complete before any subsequent transactions. So, we wait
  // for all pending transactions (including the flush) to complete before continuing.
  // We don't need to check the result of this second FlushPendingTxns because the actual
  // status is captured in the completion path (CompleteTxn).
  if (type == VIRTIO_BLK_T_FLUSH) {
    [[maybe_unused]] zx::result result = FlushPendingTxns();
  }

  return zx::ok();
}

BlockDevice::RequestContext::RequestContext(BlockDevice::RequestContext&& other) {
  *this = std::move(other);
}

BlockDevice::RequestContext& BlockDevice::RequestContext::operator=(RequestContext&& other) {
  if (this != &other) {
    released_ = other.released_;
    req_index_ = other.req_index_;
    discard_req_index_ = other.discard_req_index_;
    desc_index_ = other.desc_index_;
    desc_ = other.desc_;
    pages_ = other.pages_;
    num_pages_ = other.num_pages_;
    pmt_ = other.pmt_;
    other.Release();
  }
  return *this;
}

BlockDevice::RequestContext::~RequestContext() {
  // RAII-style cleanup isn't a good option, because the destructor would need to take locks (see
  // BlockDevice::FreeRequestContext), which could cause difficult-to-spot deadlocks.
  ZX_ASSERT_MSG(released_, "Did you forget to call Release/FreeRequestContext?");
}

void BlockDevice::FreeRequestContext(BlockDevice::RequestContext& context) {
  if (context.pmt() != ZX_HANDLE_INVALID) {
    zx_pmt_unpin(context.pmt());
  }
  if (context.desc() != nullptr) {
    FreeDescChainLocked(context.desc_index());
  }
  if (context.req_index() < kBlkReqCount) {
    FreeBlkReqLocked(context.req_index());
    if (context.discard_req_index()) {
      FreeBlkReqLocked(*context.discard_req_index());
    }
  }
  context.Release();
  txn_cond_.Broadcast();
}

void BlockDevice::RequestContext::Release() {
  ZX_ASSERT_MSG(!released_, "Release called twice");
  released_ = true;
}

// Thread safety: Disable thread safety analysis because TA doesn't understand std::unique_lock,
// which is required by std::condition_variable.
zx::result<BlockDevice::RequestContext> BlockDevice::AllocateRequestContext(
    uint32_t type, zx_handle_t vmo, uint64_t vmo_offset_bytes, uint32_t num_blocks,
    std::array<zx_paddr_t, MAX_SCATTER>* pages) TA_NO_THREAD_SAFETY_ANALYSIS {
  for (;;) {
    fbl::AutoLock lock(&txn_lock_);
    if (shutdown_) {
      return zx::error(ZX_ERR_IO_NOT_PRESENT);
    }
    zx::result<std::optional<RequestContext>> result;
    {
      std::unique_lock lock2(ring_lock_);
      result = TryAllocateRequestContextLocked(type, vmo, vmo_offset_bytes, num_blocks, pages);
    }
    if (result.is_error()) {
      fdf::error("failed to allocate virtio resources: {}", result);
      return result.take_error();
    }
    if (result.value().has_value()) {
      return zx::ok(std::move(*result).value());
    }

    // No resources; try again.
    txn_cond_.Wait(&txn_lock_);
  }
}

zx::result<std::optional<BlockDevice::RequestContext>> BlockDevice::TryAllocateRequestContextLocked(
    uint32_t type, zx_handle_t vmo, uint64_t vmo_offset_bytes, uint32_t num_blocks,
    std::array<zx_paddr_t, MAX_SCATTER>* pages) {
  RequestContext out;
  // Thread safety: TryAllocateRequestContextLocked requires the necessary locks.
  auto cleanup = fit::defer([&]() TA_NO_THREAD_SAFETY_ANALYSIS { FreeRequestContext(out); });
  std::optional<size_t> idx = AllocateBlkReqLocked();
  if (!idx) {
    fdf::trace("too many block requests queued!");
    return zx::ok(std::nullopt);
  }
  out.set_req_index(*idx);

  if (type == VIRTIO_BLK_T_DISCARD) {
    // A second descriptor needs to be allocated for discard requests.
    idx = AllocateBlkReqLocked();
    if (!idx) {
      fdf::trace("too many block requests queued!");
      return zx::ok(std::nullopt);
    }
    out.set_discard_req_index(*idx);
  }

  if (type == VIRTIO_BLK_T_IN || type == VIRTIO_BLK_T_OUT) {
    size_t num_pages;
    zx::result result = PinPages(bti_.get(), vmo, vmo_offset_bytes, num_blocks, pages, &num_pages);
    if (result.is_error()) {
      return result.take_error();
    }
    out.set_pages(pages->data(), num_pages);
    out.set_pmt(result.value());
  }

  uint16_t num_descriptors =
      (type == VIRTIO_BLK_T_DISCARD ? 3u : 2u) + static_cast<uint16_t>(out.num_pages());
  uint16_t desc_index;
  struct vring_desc* desc = nullptr;
  desc = vring_.AllocDescChain(num_descriptors, &desc_index);
  if (!desc) {
    fdf::trace("failed to allocate descriptor chain of length {}", 2u + out.num_pages());
    return zx::ok(std::nullopt);
  }
  out.set_desc(desc, desc_index);
  cleanup.cancel();
  return zx::ok(std::move(out));
}

void BlockDevice::WatchdogThread() {
  for (;;) {
    sync_completion_wait(&watchdog_signal_, kWatchdogInterval.get());
    if (watchdog_shutdown_.load()) {
      return;
    }
    zx::time now = zx::clock::get_monotonic();
    {
      std::lock_guard<std::mutex> lock(watchdog_lock_);
      int idx = 0;
      for (const auto& start_time : blk_req_start_timestamps_) {
        if (now - kWatchdogInterval >= start_time) {
          // Round down to the interval
          zx::duration latency = ((now - start_time) / kWatchdogInterval) * kWatchdogInterval;
          // LINT.IfChange(watchdog_tefmo)
          fdf::warn("txn {} has not completed after {}s!", idx, latency.to_secs());
          // LINT.ThenChange(/tools/testing/tefmocheck/string_in_log_check.go:watchdog_tefmo)
        }
        idx += 1;
      }
    }
  }
}

zx::result<> BlockDevice::FlushPendingTxns() {
  fbl::AutoLock lock(&txn_lock_);
  while (!shutdown_ && !list_is_empty(&pending_txn_list_)) {
    txn_cond_.Wait(&txn_lock_);
  }
  if (shutdown_) {
    return zx::error(ZX_ERR_IO_NOT_PRESENT);
  }
  return zx::ok();
}

void BlockDevice::CleanUpPendingTxns() {
  // Virtio specification 3.3.1 Driver Requirements: Device Cleanup
  // A driver MUST ensure a virtqueue isn’t live (by device reset) before removing exposed
  // buffers.
  DeviceReset();
  block_txn_t* txn = nullptr;
  block_txn_t* temp_entry = nullptr;
  fbl::AutoLock lock(&txn_lock_);
  list_for_every_entry_safe (&pending_txn_list_, txn, temp_entry, block_txn_t, node) {
    FreeBlkReqLocked(txn->req_index);
    if (txn->discard_req_index) {
      FreeBlkReqLocked(*txn->discard_req_index);
    }
    list_delete(&txn->node);
    CompleteTxn(txn, ZX_ERR_IO_NOT_PRESENT);
  }
}

zx::result<> BlockDriver::Start(fdf::DriverContext context) {
  auto incoming = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  node_token_ = context.take_node_token();

  zx::result device = CreateBlockDevice(*incoming);
  if (device.is_error()) {
    return device.take_error();
  }
  block_device_ = std::move(*device);

  zx_status_t status = block_device_->Init();
  if (status != ZX_OK) {
    return zx::error(status);
  }

  auto [controller_client_end, controller_server_end] =
      fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();

  node_controller_.Bind(std::move(controller_client_end));

  fidl::Arena arena;

  const auto args =
      fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena).name(arena, name()).Build();

  auto result =
      fidl::WireCall(node().borrow())->AddChild(args, std::move(controller_server_end), {});
  if (!result.ok()) {
    fdf::error("Failed to add child: {}", result.status_string());
    return zx::error(result.status());
  }

  if (zx::result result = outgoing()->AddService<fuchsia_hardware_block_volume::Service>(
          fuchsia_hardware_block_volume::Service::InstanceHandler({
              .volume =
                  [this](fidl::ServerEnd<fuchsia_storage_block::Block> server_end) {
                    block_device_->ServeRequests(std::move(server_end));
                  },
              .mapper =
                  [this](fidl::ServerEnd<fuchsia_storage_block::Mapper> server_end) {
                    block_device_->ServeMapperRequests(std::move(server_end));
                  },
              .token =
                  [this](fidl::ServerEnd<fuchsia_driver_token::NodeToken> server_end) {
                    fidl::BindServer(fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                     std::move(server_end), this);
                  },
          }));
      result.is_error()) {
    fdf::error("Failed to add volume service instance: {}", result);
    return result.take_error();
  }

  return zx::ok();
}

zx::result<std::unique_ptr<BlockDevice>> BlockDriver::CreateBlockDevice(
    const fdf::Namespace& incoming) {
  zx::result<fidl::ClientEnd<fuchsia_hardware_pci::Device>> pci_client_result =
      incoming.Connect<fuchsia_hardware_pci::Service::Device>();
  if (pci_client_result.is_error()) {
    fdf::error("Failed to get pci client: {}", pci_client_result);
    return pci_client_result.take_error();
  }

  zx::result<std::pair<zx::bti, std::unique_ptr<virtio::Backend>>> bti_and_backend_result =
      virtio::GetBtiAndBackend(std::move(pci_client_result).value());
  if (!bti_and_backend_result.is_ok()) {
    fdf::error("GetBtiAndBackend failed: {}", bti_and_backend_result);
    return bti_and_backend_result.take_error();
  }
  auto [bti, backend] = std::move(bti_and_backend_result).value();

  return zx::ok(std::make_unique<BlockDevice>(std::move(bti), std::move(backend), logger()));
}

void BlockDriver::Stop(fdf::StopCompleter completer) {
  if (block_device_) {
    block_device_->Release();
  }
  completer(zx::ok());
}
void BlockDriver::Get(GetCompleter::Sync& completer) {
  zx::event token = node_token();
  if (token.is_valid()) {
    completer.Reply(zx::ok(std::move(token)));
  } else {
    completer.Reply(zx::error(ZX_ERR_NOT_FOUND));
  }
}

}  // namespace virtio

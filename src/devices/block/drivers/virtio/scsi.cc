// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "scsi.h"

#include <inttypes.h>
#include <lib/async/cpp/task.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fit/defer.h>
#include <lib/scsi/block-device.h>
#include <lib/scsi/controller.h>
#include <lib/virtio/driver_utils.h>
#include <netinet/in.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/param.h>
#include <zircon/compiler.h>

#include <algorithm>
#include <optional>
#include <utility>

#include <fbl/algorithm.h>
#include <fbl/auto_lock.h>
#include <safemath/safe_math.h>
#include <virtio/scsi.h>

#include "src/devices/bus/lib/virtio/trace.h"

#define LOCAL_TRACE 0

namespace virtio {

// Fill in req->lun with a single-level LUN structure representing target:lun.
void ScsiDevice::FillLUNStructure(struct virtio_scsi_req_cmd* req, uint8_t target, uint16_t lun) {
  req->lun[0] = 1;
  req->lun[1] = target;
  req->lun[2] = 0x40 | static_cast<uint8_t>(lun >> 8);
  req->lun[3] = static_cast<uint8_t>(lun) & 0xff;
}

ScsiDevice::scsi_io_slot* ScsiDevice::GetIO() {
  // For testing purposes, this condition can be triggered
  // by lowering MAX_IOS (to say 2). And running biotime
  // (with default IO concurrency).
  while (active_ios_ == MAX_IOS && !released_) {
    ioslot_cv_.Wait(&lock_);
  }
  if (released_) {
    return nullptr;
  }
  active_ios_++;
  for (int i = 0; i < MAX_IOS; i++) {
    if (scsi_io_slot_table_[i].avail) {
      scsi_io_slot_table_[i].avail = false;
      return &scsi_io_slot_table_[i];
    }
  }
  ZX_DEBUG_ASSERT(false);  // Unexpected.
  return nullptr;
}

void* ScsiDevice::GetIOForTesting() {
  fbl::AutoLock lock(&lock_);
  return GetIO();
}

void ScsiDevice::FreeIO(scsi_io_slot* io_slot) {
  // The callback must be invoked before the slot is freed.
  ZX_DEBUG_ASSERT(!io_slot->callback);
  io_slot->trim_data_vmo.reset();
  io_slot->callback = nullptr;
  io_slot->avail = true;
  active_ios_--;
  ioslot_cv_.Signal();
}

std::vector<fit::callback<void(zx_status_t)>> ScsiDevice::CleanUpPendingTxns() {
  // Virtio specification 3.3.1 Driver Requirements: Device Cleanup
  // A driver MUST ensure a virtqueue isn’t live (by device reset) before removing exposed
  // buffers.
  DeviceReset();
  std::vector<fit::callback<void(zx_status_t)>> callbacks;
  for (auto& io_slot : scsi_io_slot_table_) {
    if (!io_slot.avail) {
      if (io_slot.data_vmo->is_valid()) {
        if (io_slot.vmar_mapped) {
          zx_vmar_unmap(zx_vmar_root_self(), reinterpret_cast<zx_vaddr_t>(io_slot.data),
                        io_slot.transfer_bytes);
        } else {
          free(io_slot.data);
        }
      }
      if (io_slot.callback) {
        callbacks.push_back(std::move(io_slot.callback));
      }
      FreeIO(&io_slot);
    }
  }
  ioslot_cv_.Broadcast();
  desc_cv_.Broadcast();
  return callbacks;
}

void ScsiDevice::Release() {
  bool should_release_virtio = false;
  std::vector<fit::callback<void(zx_status_t)>> callbacks;
  {
    fbl::AutoLock lock(&lock_);
    if (released_) {
      return;
    }
    released_ = true;
    callbacks = CleanUpPendingTxns();
    if (irq_thread_started_) {
      should_release_virtio = true;
      irq_thread_started_ = false;
    }
  }
  for (auto& callback : callbacks) {
    callback(ZX_ERR_IO_NOT_PRESENT);
  }
  if (should_release_virtio) {
    virtio::Device::Release();
  }
}

ScsiDevice::~ScsiDevice() { Release(); }

void ScsiDevice::IrqRingUpdate() {
  // Parse our descriptor chain and add back to the free queue.
  auto free_chain = [this](vring_used_elem* elem) TA_NO_THREAD_SAFETY_ANALYSIS {
    if (released_) {
      return;
    }
    auto index = static_cast<uint16_t>(elem->id);
    vring_desc const* tail_desc = nullptr;

    // Reclaim the entire descriptor chain.
    for (;;) {
      vring_desc const* desc = request_queue_.DescFromIndex(index);
      if (!desc) {
        fdf::error("Invalid descriptor index {} in used ring", index);
        break;
      }
      const bool has_next = desc->flags & VRING_DESC_F_NEXT;
      const auto next = desc->next;

      this->request_queue_.FreeDesc(index);
      if (!has_next) {
        tail_desc = desc;
        break;
      }
      index = next;
    }
    desc_cv_.Broadcast();
    if (!tail_desc) {
      return;
    }
    // Search for the IO that just completed, using tail_desc.
    for (int i = 0; i < MAX_IOS; i++) {
      scsi_io_slot* io_slot = &scsi_io_slot_table_[i];
      if (io_slot->avail)
        continue;
      if (io_slot->tail_desc == tail_desc) {
        // Capture response before freeing iobuffer.
        zx_status_t status;
        if (io_slot->response->response || io_slot->response->status) {
          if (io_slot->response->sense_len == sizeof(scsi::FixedFormatSenseDataHeader) &&
              reinterpret_cast<scsi::FixedFormatSenseDataHeader*>(io_slot->response->sense)
                      ->sense_key() == scsi::SenseKey::UNIT_ATTENTION) {
            status = ZX_ERR_UNAVAILABLE;
          } else {
            status = ZX_ERR_INTERNAL;
          }
        } else {
          status = ZX_OK;
        }

        // If Read, copy data from iobuffer to the iovec.
        const bool read_success = status == ZX_OK && !io_slot->is_write && io_slot->transfer_bytes;
        if (read_success) {
          memcpy(io_slot->data, io_slot->data_in_region, io_slot->transfer_bytes);
        }

        // Undo previous zx_vmar_map or free temp buffer (allocated if offset, length were not page
        // aligned).
        if (io_slot->data_vmo->is_valid()) {
          if (io_slot->vmar_mapped) {
            status = zx_vmar_unmap(zx_vmar_root_self(), reinterpret_cast<zx_vaddr_t>(io_slot->data),
                                   io_slot->transfer_bytes);
          } else {
            if (read_success) {
              status = zx_vmo_write(io_slot->data_vmo->get(), io_slot->data,
                                    io_slot->vmo_offset_bytes, io_slot->transfer_bytes);
            }
            free(io_slot->data);
          }
        }

        fit::callback<void(zx_status_t)> callback = std::move(io_slot->callback);
        FreeIO(io_slot);
        lock_.Release();
        if (callback) {
          callback(status);
        }
        lock_.Acquire();
        return;
      }
    }
    ZX_DEBUG_ASSERT(false);  // Unexpected.
  };

  // Tell the ring to find free chains and hand it back to our lambda.
  fbl::AutoLock lock(&lock_);
  if (released_) {
    return;
  }
  request_queue_.IrqRingUpdate(free_chain);
}

zx::result<> ScsiDriver::Start(fdf::DriverContext context) {
  incoming_ = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  node_name_ = context.node_name();

  auto [controller_client_end, controller_server_end] =
      fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();
  auto [node_client_end, node_server_end] =
      fidl::Endpoints<fuchsia_driver_framework::Node>::Create();

  node_controller_.Bind(std::move(controller_client_end));
  root_node_.Bind(std::move(node_client_end));

  fidl::Arena arena;

  const auto args =
      fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena).name(arena, name()).Build();

  // Add root device, which will contain block devices for logical units
  auto result = fidl::WireCall(node().borrow())
                    ->AddChild(args, std::move(controller_server_end), std::move(node_server_end));
  if (!result.ok()) {
    fdf::error("Failed to add child: {}", result.status_string());
    return zx::error(result.status());
  }

  zx::result<fidl::ClientEnd<fuchsia_hardware_pci::Device>> pci_client_result =
      driver_incoming()->Connect<fuchsia_hardware_pci::Service::Device>();
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

  scsi_device_ = std::make_unique<ScsiDevice>(this, std::move(bti), std::move(backend));
  zx_status_t status = scsi_device_->Init();
  if (status != ZX_OK) {
    return zx::error(status);
  }
  status = scsi_device_->ProbeLuns();
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok();
}

void ScsiDriver::Stop(fdf::StopCompleter completer) {
  if (scsi_device_) {
    scsi_device_->Release();
  }

  std::vector<scsi::BlockDevice*> devs;
  {
    std::lock_guard<std::mutex> lock(lock_);
    for (auto& [target, luns] : block_devs_) {
      for (auto& [lun, dev] : luns) {
        if (dev) {
          devs.push_back(dev.get());
        }
      }
    }

    if (devs.empty()) {
      block_devs_.clear();
      completer(zx::ok());
      return;
    }
  }

  auto shared_completer = std::make_shared<fdf::StopCompleter>(std::move(completer));
  auto remaining = std::make_shared<std::atomic<size_t>>(devs.size());

  for (auto* dev : devs) {
    dev->ShutdownAsync([this, shared_completer, remaining]() {
      if (remaining->fetch_sub(1) == 1) {
        auto cleanup = [this, shared_completer]() {
          {
            std::lock_guard<std::mutex> lock(lock_);
            block_devs_.clear();
          }
          (*shared_completer)(zx::ok());
        };
        if (dispatcher()) {
          async::PostTask(dispatcher(), std::move(cleanup));
        } else {
          cleanup();
        }
      }
    });
  }
}

zx_status_t ScsiDriver::ExecuteCommandSync(uint8_t target, uint16_t lun, iovec cdb, bool is_write,
                                           iovec data) {
  if (!scsi_device_) {
    fdf::error("ExecuteCommandSync called for driver that has not been started.");
    return ZX_ERR_INTERNAL;
  }

  struct scsi_sync_callback_state {
    sync_completion_t completion;
    zx_status_t status;
  };
  scsi_sync_callback_state cookie;
  sync_completion_reset(&cookie.completion);
  auto callback = [&cookie](zx_status_t status) {
    cookie.status = status;
    sync_completion_signal(&cookie.completion);
  };
  scsi_device_->QueueCommand(target, lun, cdb, is_write, zx::unowned_vmo(), 0, data.iov_len,
                             std::move(callback), data.iov_base, /*vmar_mapped=*/false);
  sync_completion_wait(&cookie.completion, ZX_TIME_INFINITE);
  return cookie.status;
}

zx::result<> ScsiDevice::AllocatePages(zx::vmo& vmo, fzl::VmoMapper& mapper, size_t size) {
  const uint32_t data_size =
      fbl::round_up(safemath::checked_cast<uint32_t>(size), zx_system_get_page_size());
  if (zx_status_t status = zx::vmo::create(data_size, 0, &vmo); status != ZX_OK) {
    return zx::error(status);
  }

  if (zx_status_t status = mapper.Map(vmo, 0, data_size); status != ZX_OK) {
    fdf::error("Failed to map IO buffer: {}", zx_status_get_string(status));
    return zx::error(status);
  }
  return zx::ok();
}

void ScsiDriver::ExecuteCommandsAsync(uint8_t target, uint16_t lun,
                                      std::span<scsi::ScsiRequest> batch) {
  if (!scsi_device_) {
    fdf::error("ExecuteCommandsAsync called for driver that has not been started.");
    for (auto& req : batch) {
      req.Complete(ZX_ERR_INTERNAL);
    }
    return;
  }

  uint32_t block_size_bytes = 0;
  {
    std::lock_guard<std::mutex> lock(lock_);
    if (auto target_it = block_devs_.find(target); target_it != block_devs_.end()) {
      if (auto lun_it = target_it->second.find(lun); lun_it != target_it->second.end()) {
        block_size_bytes = lun_it->second->block_size_bytes();
      }
    }
  }

  for (auto& req : batch) {
    const auto& cdb = req.cdb();
    iovec cdb_iov = {
        .iov_base = const_cast<uint8_t*>(cdb.data()),
        .iov_len = cdb.size(),
    };
    bool is_write = req.is_write();

    zx_handle_t data_vmo = req.data_vmo()->is_valid() ? req.data_vmo()->get() : ZX_HANDLE_INVALID;
    zx_off_t vmo_offset_bytes = req.vmo_offset();
    size_t transfer_bytes = req.immediate_data().size() > 0
                                ? req.immediate_data().size()
                                : (req.block_size() > 0 ? req.transfer_length_bytes()
                                                        : req.transfer_length() * block_size_bytes);
    std::optional<zx::vmo> trim_data_vmo;

    if (req.immediate_data().size() > 0) {
      zx::vmo vmo;
      trim_data_vmo = std::move(vmo);
      fzl::VmoMapper mapper;
      if (zx::result<> result = scsi_device_->AllocatePages(trim_data_vmo.value(), mapper,
                                                            req.immediate_data().size());
          result.is_error()) {
        fdf::error("Failed to allocate data buffer: {}", result);
        req.Complete(result.status_value());
        continue;
      }
      memcpy(mapper.start(), req.immediate_data().data(), req.immediate_data().size());

      data_vmo = trim_data_vmo->get();
      vmo_offset_bytes = 0;
    }

    // Map IO data into process memory.
    void* rw_data = nullptr;
    bool vmar_mapped = false;
    if (data_vmo != ZX_HANDLE_INVALID) {
      // To use zx_vmar_map, offset, length must be page aligned. If it isn't (uncommon),
      // allocate a temp buffer and do a copy.
      if ((transfer_bytes > 0) && ((transfer_bytes % zx_system_get_page_size()) == 0) &&
          ((vmo_offset_bytes % zx_system_get_page_size()) == 0)) {
        vmar_mapped = true;
        zx_vaddr_t mapped_addr;
        // This is later unmapped in IrqRingUpdate().
        zx_status_t status = zx_vmar_map(zx_vmar_root_self(), ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0,
                                         data_vmo, vmo_offset_bytes, transfer_bytes, &mapped_addr);
        if (status != ZX_OK) {
          req.Complete(status);
          continue;
        }
        rw_data = reinterpret_cast<void*>(mapped_addr);
      } else {
        // This is later freed in IrqRingUpdate().
        rw_data = calloc(1, transfer_bytes);
        if (is_write) {
          zx_status_t status = zx_vmo_read(data_vmo, rw_data, vmo_offset_bytes, transfer_bytes);
          if (status != ZX_OK) {
            free(rw_data);
            req.Complete(status);
            continue;
          }
        }
      }
    }

    auto callback = [request = std::move(req)](zx_status_t status) mutable {
      request.Complete(status);
    };

    scsi_device_->QueueCommand(target, lun, cdb_iov, is_write, zx::unowned_vmo(data_vmo),
                               vmo_offset_bytes, transfer_bytes, std::move(callback), rw_data,
                               vmar_mapped, std::move(trim_data_vmo));
  }
}

void ScsiDevice::QueueCommand(uint8_t target, uint16_t lun, iovec cdb, bool is_write,
                              zx::unowned_vmo data_vmo, zx_off_t vmo_offset_bytes,
                              size_t transfer_bytes, fit::callback<void(zx_status_t)> cb,
                              void* data, bool vmar_mapped, std::optional<zx::vmo> trim_data_vmo) {
  auto cleanup = fit::defer([=] {
    if (data_vmo->is_valid()) {
      if (vmar_mapped) {
        zx_vmar_unmap(zx_vmar_root_self(), reinterpret_cast<zx_vaddr_t>(data), transfer_bytes);
      } else {
        free(data);
      }
    }
  });

  iovec data_in = {nullptr, 0};
  iovec data_out = {nullptr, 0};
  if (transfer_bytes) {
    if (is_write) {
      data_out = {data, transfer_bytes};
    } else {
      data_in = {data, transfer_bytes};
    }
  }

  // We do all of the error checking up front, so we don't need to fail the IO
  // after acquiring the IO slot and the descriptors.
  // If data_in fits within request_buffers_, all the regions of this request will fit.
  if ((sizeof(struct virtio_scsi_req_cmd) + data_out.iov_len + sizeof(struct virtio_scsi_resp_cmd) +
       data_in.iov_len) > request_buffers_size_) {
    if (cb) {
      cb(ZX_ERR_NO_MEMORY);
    }
    return;
  }

  uint16_t descriptor_chain_length = 2;
  if (data_out.iov_len) {
    descriptor_chain_length++;
  }
  if (data_in.iov_len) {
    descriptor_chain_length++;
  }

  lock_.Acquire();
  if (released_) {
    lock_.Release();
    if (cb) {
      cb(ZX_ERR_IO_NOT_PRESENT);
    }
    return;
  }
  // Get both the IO slot and the descriptors needed up front.
  auto io_slot = GetIO();
  if (released_ || !io_slot) {
    if (io_slot) {
      FreeIO(io_slot);
    }
    lock_.Release();
    if (cb) {
      cb(ZX_ERR_IO_NOT_PRESENT);
    }
    return;
  }
  uint16_t id = 0;
  auto request_desc = request_queue_.AllocDescChain(/*count=*/descriptor_chain_length, &id);
  // For testing purposes, this condition can be triggered by failing
  // AllocDescChain every N attempts. But we would have to Signal the cv
  // somewhere. A good place to do that is at the bottom of ProbeLuns,
  // after the luns are probed, in a loop. If we do the signaling there,
  // we'd need to ensure error injection doesn't start until after luns are
  // probed.
  while (request_desc == nullptr) {
    // Drop the request buf, before blocking, waiting for descs to free up.
    FreeIO(io_slot);
    if (released_) {
      lock_.Release();
      if (cb) {
        cb(ZX_ERR_IO_NOT_PRESENT);
      }
      return;
    }
    desc_cv_.Wait(&lock_);
    if (released_) {
      lock_.Release();
      if (cb) {
        cb(ZX_ERR_IO_NOT_PRESENT);
      }
      return;
    }
    io_slot = GetIO();
    if (released_ || !io_slot) {
      if (io_slot) {
        FreeIO(io_slot);
      }
      lock_.Release();
      if (cb) {
        cb(ZX_ERR_IO_NOT_PRESENT);
      }
      return;
    }
    request_desc = request_queue_.AllocDescChain(/*count=*/descriptor_chain_length, &id);
  }

  auto* request_buffers = io_slot->request_buffer.get();
  // virtio-scsi requests have a 'request' region, an optional data-out region, a 'response'
  // region, and an optional data-in region. Allocate and fill them and then execute the request.
  const auto request_offset = 0ull;
  const auto data_out_offset = request_offset + sizeof(struct virtio_scsi_req_cmd);
  const auto response_offset = data_out_offset + data_out.iov_len;
  const auto data_in_offset = response_offset + sizeof(struct virtio_scsi_resp_cmd);

  auto* const request_buffers_addr = reinterpret_cast<uint8_t*>(request_buffers->virt());
  auto* const req =
      reinterpret_cast<struct virtio_scsi_req_cmd*>(request_buffers_addr + request_offset);
  auto* const data_out_region = reinterpret_cast<uint8_t*>(request_buffers_addr + data_out_offset);
  auto* const response =
      reinterpret_cast<struct virtio_scsi_resp_cmd*>(request_buffers_addr + response_offset);
  auto* const data_in_region = reinterpret_cast<uint8_t*>(request_buffers_addr + data_in_offset);

  memset(req, 0, sizeof(*req));
  memset(response, 0, sizeof(*response));
  memcpy(&req->cdb, cdb.iov_base, cdb.iov_len);
  FillLUNStructure(req, target, lun);
  req->id = scsi_transport_tag_++;

  vring_desc* tail_desc;
  request_desc->addr = request_buffers->phys() + request_offset;
  request_desc->len = sizeof(*req);
  request_desc->flags = VRING_DESC_F_NEXT;
  auto next_id = request_desc->next;

  if (data_out.iov_len) {
    memcpy(data_out_region, data_out.iov_base, data_out.iov_len);
    auto* data_out_desc = request_queue_.DescFromIndex(next_id);
    data_out_desc->addr = request_buffers->phys() + data_out_offset;
    data_out_desc->len = static_cast<uint32_t>(data_out.iov_len);
    data_out_desc->flags = VRING_DESC_F_NEXT;
    next_id = data_out_desc->next;
  }

  auto* response_desc = request_queue_.DescFromIndex(next_id);
  response_desc->addr = request_buffers->phys() + response_offset;
  response_desc->len = sizeof(*response);
  response_desc->flags = VRING_DESC_F_WRITE;

  if (data_in.iov_len) {
    response_desc->flags |= VRING_DESC_F_NEXT;
    auto* data_in_desc = request_queue_.DescFromIndex(response_desc->next);
    data_in_desc->addr = request_buffers->phys() + data_in_offset;
    data_in_desc->len = static_cast<uint32_t>(data_in.iov_len);
    data_in_desc->flags = VRING_DESC_F_WRITE;
    tail_desc = data_in_desc;
  } else {
    tail_desc = response_desc;
  }

  io_slot->data_vmo = data_vmo;
  io_slot->vmo_offset_bytes = vmo_offset_bytes;
  io_slot->transfer_bytes = transfer_bytes;
  io_slot->is_write = is_write;
  io_slot->data = data;
  io_slot->vmar_mapped = vmar_mapped;
  io_slot->tail_desc = tail_desc;
  io_slot->data_in_region = data_in_region;
  io_slot->callback = std::move(cb);
  io_slot->request_buffers = request_buffers;
  io_slot->response = response;
  io_slot->trim_data_vmo = std::move(trim_data_vmo);

  cleanup.cancel();

  request_queue_.SubmitChain(id);
  request_queue_.Kick();

  lock_.Release();
}

constexpr uint32_t SCSI_SECTOR_SIZE = 512;
constexpr uint32_t SCSI_MAX_XFER_SECTORS = ScsiDevice::kMaxXferSectors;

zx_status_t ScsiDevice::ProbeLuns() {
  uint8_t max_target;
  uint16_t max_lun;
  uint32_t max_sectors;
  {
    fbl::AutoLock lock(&lock_);
    // TODO: Return error if config_.max_target > (UINT8_MAX - 1) || config_.max_lun > (UINT16_MAX).
    // virtio-scsi has a 16-bit max_target field, but the encoding we use limits us to one byte
    // target identifiers.
    max_target =
        static_cast<uint8_t>(std::min(config_.max_target, static_cast<uint16_t>(UINT8_MAX - 1)));
    // virtio-scsi has a 32-bit max_lun field, but the encoding we use limits us to 16-bit.
    max_lun = static_cast<uint16_t>(std::min(config_.max_lun, static_cast<uint32_t>(UINT16_MAX)));
    // Smaller of controller's max transfer sectors and the 512K clamp.
    max_sectors = std::min(config_.max_sectors, SCSI_MAX_XFER_SECTORS);
  }

  scsi::DeviceOptions options(/*check_unmap_support=*/true, /*use_mode_sense_6=*/true,
                              /*use_read_write_12=*/false);

  // virtio-scsi nominally supports multiple channels, but the device support is not
  // complete. The device encoding for targets in commands does not allow encoding the
  // channel number, so we do not attempt to scan beyond channel 0 here.
  //
  // QEMU and GCE disagree on the definition of the max_target and max_lun config fields;
  // QEMU's max_target/max_lun refer to the last valid whereas GCE's refers to the first
  // invalid target/lun. Use <= to handle both.
  for (uint8_t target = 0u; target <= max_target; target++) {
    zx::result<uint32_t> lun_count = scsi_driver_->ScanAndBindLogicalUnits(
        target, max_sectors * SCSI_SECTOR_SIZE, max_lun, nullptr, options);
    if (lun_count.is_error() || lun_count.value() == 0) {
      // For now, assume REPORT LUNS is supported. A failure indicates no LUNs on this target.
      continue;
    }
  }
  return ZX_OK;
}

zx_status_t ScsiDevice::Init() {
  LTRACE_ENTRY;

  DeviceReset();
  ReadDeviceConfig<uint32_t>(offsetof(virtio_scsi_config, num_queues), &config_.num_queues);
  ReadDeviceConfig<uint32_t>(offsetof(virtio_scsi_config, seg_max), &config_.seg_max);
  ReadDeviceConfig<uint32_t>(offsetof(virtio_scsi_config, max_sectors), &config_.max_sectors);
  ReadDeviceConfig<uint32_t>(offsetof(virtio_scsi_config, cmd_per_lun), &config_.cmd_per_lun);
  ReadDeviceConfig<uint32_t>(offsetof(virtio_scsi_config, event_info_size),
                             &config_.event_info_size);
  ReadDeviceConfig<uint32_t>(offsetof(virtio_scsi_config, sense_size), &config_.sense_size);
  ReadDeviceConfig<uint32_t>(offsetof(virtio_scsi_config, cdb_size), &config_.cdb_size);
  ReadDeviceConfig<uint16_t>(offsetof(virtio_scsi_config, max_channel), &config_.max_channel);
  ReadDeviceConfig<uint16_t>(offsetof(virtio_scsi_config, max_target), &config_.max_target);
  ReadDeviceConfig<uint32_t>(offsetof(virtio_scsi_config, max_lun), &config_.max_lun);

  // Validate config.
  {
    fbl::AutoLock lock(&lock_);
    if (config_.max_channel > 1) {
      fdf::warn("config_.max_channel {} not expected.", config_.max_channel);
    }
  }

  DriverStatusAck();

  if (DeviceFeaturesSupported() & VIRTIO_F_VERSION_1) {
    DriverFeaturesAck(VIRTIO_F_VERSION_1);
    if (zx_status_t status = DeviceStatusFeaturesOk(); status != ZX_OK) {
      fdf::error("Feature negotiation failed: {}", zx_status_get_string(status));
      return status;
    }
  }

  if (!bti().is_valid()) {
    fdf::error("invalid bti handle");
    return ZX_ERR_BAD_HANDLE;
  }
  {
    fbl::AutoLock lock(&lock_);
    auto err = control_ring_.Init(/*index=*/Queue::CONTROL);
    if (err) {
      fdf::error("failed to allocate control queue");
      return err;
    }

    err = request_queue_.Init(/*index=*/Queue::REQUEST);
    if (err) {
      fdf::error("failed to allocate request queue");
      return err;
    }
    request_buffers_size_ =
        (SCSI_SECTOR_SIZE * std::min(config_.max_sectors, SCSI_MAX_XFER_SECTORS)) +
        (sizeof(struct virtio_scsi_req_cmd) + sizeof(struct virtio_scsi_resp_cmd));
    const size_t buffer_size = fbl::round_up(request_buffers_size_, zx_system_get_page_size());
    auto buffer_factory = dma_buffer::CreateBufferFactory();
    for (int i = 0; i < MAX_IOS; i++) {
      auto status = buffer_factory->CreateContiguous(bti(), buffer_size, 0,
                                                     dma_buffer::CacheOptions::kEnabled,
                                                     &scsi_io_slot_table_[i].request_buffer);
      if (status) {
        fdf::error("failed to allocate queue working memory");
        return status;
      }
      scsi_io_slot_table_[i].avail = true;
    }
    active_ios_ = 0;
    scsi_transport_tag_ = 0;
  }
  StartIrqThread();
  {
    fbl::AutoLock lock(&lock_);
    irq_thread_started_ = true;
  }
  DriverStatusOk();
  return ZX_OK;
}

}  // namespace virtio

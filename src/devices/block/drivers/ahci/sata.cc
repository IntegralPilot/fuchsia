// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "sata.h"

#include <inttypes.h>
#include <lib/sync/completion.h>
#include <lib/zx/vmo.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/param.h>
#include <zircon/types.h>

#include <bind/fuchsia/cpp/bind.h>
#include <fbl/alloc_checker.h>

#include "controller.h"
#include "src/devices/block/lib/common/include/common.h"

namespace ahci {

void SataTransaction::Complete(zx_status_t status) {
  if (completion_cb) {
    completion_cb(status);
  }
}

constexpr size_t kQemuMaxTransferBlocks = 1024;  // Linux kernel limit

static bool IsModelIdQemu(char* model_id) {
  constexpr char kQemuModelId[] = "QEMU HARDDISK";
  return !memcmp(model_id, kQemuModelId, sizeof(kQemuModelId) - 1);
}

zx_status_t SataDevice::Init() {
  // Set default devinfo
  SataDeviceInfo di;
  di.block_size = 512;
  di.max_cmd = 1;
  controller_->SetDevInfo(port_, &di);

  // send IDENTIFY DEVICE
  zx::vmo vmo;
  zx_status_t status = zx::vmo::create(512, 0, &vmo);
  if (status != ZX_OK) {
    fdf::error("Failed to allocate vmo: {}", zx_status_get_string(status));
    return status;
  }

  sync_completion_t completion;
  zx_status_t completion_status = ZX_OK;
  SataTransaction txn = {};
  txn.operation = block_server::Operation{
      .tag = block_server::Operation::Tag::Read,
      .read = {.device_block_offset = 0, .block_count = 1, .vmo_offset = 0},
  };
  txn.vmo = vmo.borrow();
  txn.completion_cb = [&completion, &completion_status](zx_status_t status) {
    completion_status = status;
    sync_completion_signal(&completion);
  };
  txn.cmd = SATA_CMD_IDENTIFY_DEVICE;
  txn.device = 0;

  controller_->Queue(port_, &txn);
  sync_completion_wait(&completion, ZX_TIME_INFINITE);

  status = completion_status;
  if (status != ZX_OK) {
    fdf::error("{}: Failed IDENTIFY_DEVICE: {}", DriverName().c_str(),
               zx_status_get_string(status));
    return status;
  }

  // parse results
  SataIdentifyDeviceResponse devinfo;
  status = vmo.read(&devinfo, 0, sizeof(devinfo));
  if (status != ZX_OK) {
    fdf::error("Failed vmo_read: {}", zx_status_get_string(status));
    return ZX_ERR_INTERNAL;
  }
  vmo.reset();

  // Strings are 16-bit byte-flipped. Fix in place.
  // Strings are NOT null-terminated.
  SataStringFix(devinfo.serial.word, sizeof(devinfo.serial.word));
  SataStringFix(devinfo.firmware_rev.word, sizeof(devinfo.firmware_rev.word));
  SataStringFix(devinfo.model_id.word, sizeof(devinfo.model_id.word));

  auto model_number = std::string(devinfo.model_id.string, sizeof(devinfo.model_id.string));
  auto serial_number = std::string(devinfo.serial.string, sizeof(devinfo.serial.string));
  auto firmware_rev = std::string(devinfo.firmware_rev.string, sizeof(devinfo.firmware_rev.string));
  // Some vendors don't pad the strings with spaces (0x20). Null-terminate strings to avoid printing
  // illegal characters.
  model_number = std::string(model_number.c_str());
  serial_number = std::string(serial_number.c_str());
  firmware_rev = std::string(firmware_rev.c_str());
  fdf::info("Model number:  '{}'", model_number.c_str());
  fdf::info("Firmware rev.: '{}'", firmware_rev.c_str());

  auto inspect_device = controller_->inspect_node().CreateChild(DriverName());
  inspect_device.RecordString("model_number", model_number);
  inspect_device.RecordString("serial_number", serial_number);
  inspect_device.RecordString("firmware_rev", firmware_rev);

  switch (32 - __builtin_clz(devinfo.major_version) - 1) {
    case 11:
      inspect_device.RecordString("major_version", "ACS4");
      break;
    case 10:
      inspect_device.RecordString("major_version", "ACS3");
      break;
    case 9:
      inspect_device.RecordString("major_version", "ACS2");
      break;
    case 8:
      inspect_device.RecordString("major_version", "ATA8-ACS");
      break;
    case 7:
    case 6:
    case 5:
      inspect_device.RecordString("major_version", "ATA/ATAPI");
      break;
    default:
      inspect_device.RecordString("major_version", "Obsolete");
      break;
  }

  uint16_t cap = devinfo.capabilities_1;
  if (cap & (1 << 8)) {
    inspect_device.RecordString("capabilities", "DMA");
  } else {
    inspect_device.RecordString("capabilities", "PIO");
  }
  uint32_t max_cmd = devinfo.queue_depth;
  inspect_device.RecordUint("max_commands", max_cmd + 1);

  uint32_t block_size = 512;  // default
  uint64_t block_count = 0;
  if (cap & (1 << 9)) {
    if ((devinfo.sector_size & 0xd000) == 0x5000) {
      block_size = 2 * devinfo.logical_sector_size;
    }
    if (devinfo.command_set1_1 & (1 << 10)) {
      block_count = devinfo.lba_capacity2;
      inspect_device.RecordString("addressing", "48-bit LBA");
    } else {
      block_count = devinfo.lba_capacity;
      inspect_device.RecordString("addressing", "28-bit LBA");
    }
    inspect_device.RecordUint("sector_count", block_count);
    inspect_device.RecordUint("sector_size", block_size);
  } else {
    inspect_device.RecordString("addressing", "CHS unsupported");
  }

  partition_info_.block_size = block_size;
  partition_info_.block_count = block_count;

  const bool volatile_write_cache_supported =
      devinfo.command_set1_0 & SATA_DEVINFO_CMD_SET1_0_VOLATILE_WRITE_CACHE_SUPPORTED;
  const bool volatile_write_cache_enabled =
      devinfo.command_set2_0 & SATA_DEVINFO_CMD_SET2_0_VOLATILE_WRITE_CACHE_ENABLED;
  inspect_device.RecordBool("volatile_write_cache_supported", volatile_write_cache_supported);
  inspect_device.RecordBool("volatile_write_cache_enabled", volatile_write_cache_enabled);

  // READ_FPDMA_QUEUED and WRITE_FPDMA_QUEUED commands support FUA, whereas for non-NCQ, FUA read
  // commands do not exist (FUA writes do).
  if (use_command_queue_) {
    partition_info_.device_flags |=
        static_cast<uint32_t>(fuchsia_storage_block::wire::DeviceFlag::kFuaSupport);
  }

  uint32_t max_sg_size = SATA_MAX_BLOCK_COUNT * block_size;  // SATA cmd limit
  if (IsModelIdQemu(devinfo.model_id.string)) {
    max_sg_size = MIN(max_sg_size, kQemuMaxTransferBlocks * block_size);
  }
  partition_info_.max_transfer_size = MIN(AHCI_MAX_BYTES, max_sg_size);

  // set devinfo on controller
  di.block_size = block_size;
  di.max_cmd = max_cmd;
  controller_->SetDevInfo(port_, &di);

  controller_->inspect().emplace(std::move(inspect_device));

  block_server::PartitionInfo info = {
      .device_flags = partition_info_.device_flags,
      .start_block = 0,
      .block_count = partition_info_.block_count,
      .block_size = partition_info_.block_size,
      .type_guid = {},
      .instance_guid = {},
      .name = "sata",
      .flags = 0,
      .max_transfer_size = partition_info_.max_transfer_size,
  };
  {
    fbl::AutoLock lock(&lock_);
    block_server_.emplace(info, this);
  }

  return ZX_OK;
}

// implement device protocol:

zx::result<std::unique_ptr<SataDevice>> SataDevice::Bind(Controller* controller, uint32_t port,
                                                         bool use_command_queue) {
  // initialize the device
  fbl::AllocChecker ac;
  auto device = fbl::make_unique_checked<SataDevice>(&ac, controller, port, use_command_queue);
  if (!ac.check()) {
    fdf::error("Failed to allocate memory for SATA device at port {}.", port);
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  zx_status_t status = device->AddDevice();
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(std::move(device));
}

zx_status_t SataDevice::AddDevice() {
  zx_status_t status = Init();
  if (status != ZX_OK) {
    return status;
  }

  {
    auto handlers = fuchsia_hardware_block_volume::Service::InstanceHandler({
        .volume =
            [this](fidl::ServerEnd<fuchsia_storage_block::Block> server_end) {
              ServeRequests(std::move(server_end));
            },
        .token =
            [this](fidl::ServerEnd<fuchsia_driver_token::NodeToken> server_end) {
              fidl::BindServer(fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                               std::move(server_end), this);
            },
    });

    auto add_service_result =
        controller_->driver_outgoing()->AddService<fuchsia_hardware_block_volume::Service>(
            std::move(handlers), DriverName().c_str());
    if (add_service_result.is_error()) {
      fdf::error("Failed to add volume service instance: {}", add_service_result.status_string());
      return add_service_result.status_value();
    }
  }

  auto [controller_client_end, controller_server_end] =
      fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();

  node_controller_.Bind(std::move(controller_client_end));

  fidl::Arena arena;

  const auto args =
      fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena).name(arena, DriverName()).Build();

  auto result = controller_->root_node()->AddChild(args, std::move(controller_server_end), {});
  if (!result.ok()) {
    fdf::error("Failed to add child SATA device: {}", result.status_string());
    return result.status();
  }
  return ZX_OK;
}

void SataDevice::CompleteTransaction(SataTransaction* txn, zx_status_t status) {
  fbl::AutoLock lock(&lock_);
  ZX_ASSERT(block_server_);
  block_server_->SendReply(txn->request_id, zx::make_result(status));
  FreeSataTransactionLocked(txn);
}

void SataDevice::ServeRequests(fidl::ServerEnd<fuchsia_storage_block::Block> server_end) {
  fbl::AutoLock lock(&lock_);
  if (is_shutting_down_ || !block_server_) {
    return;
  }
  block_server_->Serve(std::move(server_end));
}

void SataDevice::OnRequests(std::span<block_server::Request> requests) {
  fbl::AutoLock lock(&lock_);
  ZX_ASSERT(block_server_);
  if (is_shutting_down_) {
    for (const auto& request : requests) {
      block_server_->SendReply(request.request_id, zx::error(ZX_ERR_PEER_CLOSED));
    }
    return;
  }

  for (const auto& request : requests) {
    SataTransaction* txn = AllocateSataTransaction();
    if (!txn) {
      block_server_->SendReply(request.request_id, zx::error(ZX_ERR_PEER_CLOSED));
      continue;
    }

    txn->request_id = request.request_id;
    txn->device_ptr = this;
    txn->operation = request.operation;
    txn->vmo = request.vmo;

    txn->completion_cb = [this, txn](zx_status_t status) { CompleteTransaction(txn, status); };

    uint64_t length = 0;
    uint64_t offset_dev = 0;
    uint64_t offset_vmo = 0;

    switch (request.operation.tag) {
      case block_server::Operation::Tag::Read:
        length = request.operation.read.block_count;
        offset_dev = request.operation.read.device_block_offset;
        offset_vmo = request.operation.read.vmo_offset / partition_info_.block_size;
        txn->device = 0x40;
        txn->cmd = use_command_queue_ ? SATA_CMD_READ_FPDMA_QUEUED : SATA_CMD_READ_DMA_EXT;
        fdf::trace("read {} @ {}", length, offset_dev);
        break;
      case block_server::Operation::Tag::Write:
        length = request.operation.write.block_count;
        offset_dev = request.operation.write.device_block_offset;
        offset_vmo = request.operation.write.vmo_offset / partition_info_.block_size;
        txn->device = 0x40;
        txn->cmd = use_command_queue_ ? SATA_CMD_WRITE_FPDMA_QUEUED : SATA_CMD_WRITE_DMA_EXT;
        if (request.operation.write.options.flags.is_force_access()) {
          // If NCQ is disabled, the device will not advertise FUA support to the block server
          // library, so no FUA requests should make it here.
          ZX_DEBUG_ASSERT(use_command_queue_);
          txn->device |= (1 << 7);  // set fua
        }
        fdf::trace("write {} @ {}", length, offset_dev);
        break;
      case block_server::Operation::Tag::Flush:
        txn->cmd = SATA_CMD_FLUSH_EXT;
        txn->device = 0x00;
        fdf::trace("flush");
        break;
      default:
        fdf::error("Unsupported operation tag: {}", static_cast<uint32_t>(request.operation.tag));
        block_server_->SendReply(request.request_id, zx::error(ZX_ERR_NOT_SUPPORTED));
        FreeSataTransactionLocked(txn);
        continue;
    }

    if (length > 0) {
      if (zx_status_t status = block_server::CheckIoRange(request, partition_info_.block_count);
          status != ZX_OK) {
        block_server_->SendReply(request.request_id, zx::error(status));
        FreeSataTransactionLocked(txn);
        continue;
      }
      if (length > partition_info_.max_transfer_size / partition_info_.block_size) {
        fdf::error("Io request size {} is larger than max transfer size {} blocks", length,
                   partition_info_.max_transfer_size / partition_info_.block_size);
        block_server_->SendReply(request.request_id, zx::error(ZX_ERR_INVALID_ARGS));
        FreeSataTransactionLocked(txn);
        continue;
      }
    }

    controller_->Queue(port_, txn);
  }
}

SataTransaction* SataDevice::AllocateSataTransaction() {
  while (txns_allocated_.all() && !is_shutting_down_) {
    pool_cond_.Wait(&lock_);
  }
  if (is_shutting_down_) {
    return nullptr;
  }
  for (size_t i = 0; i < txns_.size(); i++) {
    if (!txns_allocated_.test(i)) {
      txns_allocated_.set(i);
      return &txns_[i];
    }
  }
  return nullptr;
}

void SataDevice::FreeSataTransactionLocked(SataTransaction* txn) {
  ZX_ASSERT(txn >= txns_.data());
  size_t idx = txn - txns_.data();
  ZX_ASSERT(idx < txns_.size());
  txns_allocated_.reset(idx);
  pool_cond_.Signal();
  if (is_shutting_down_ && txns_allocated_.none()) {
    all_transactions_completed_.Signal();
  }
}

void SataDevice::Shutdown(std::function<void()> callback) {
  bool wait = false;
  {
    fbl::AutoLock lock(&lock_);
    is_shutting_down_ = true;
    pool_cond_.Broadcast();

    // Wait for all transactions to complete.
    if (txns_allocated_.any()) {
      all_transactions_completed_.Reset();
      wait = true;
    }
  }

  if (wait && all_transactions_completed_.Wait(zx::sec(5)) == ZX_ERR_TIMED_OUT) {
    fdf::error("Shutdown timed out waiting for in-flight transactions. Forcing port disable.");
    // Disable the controller (so we don't double-free requests) and cancel in-flight requests.
    controller_->port(port_)->Disable();
    fbl::AutoLock lock(&lock_);
    for (size_t i = 0; i < txns_.size(); ++i) {
      if (txns_allocated_.test(i)) {
        block_server_->SendReply(txns_[i].request_id, zx::error(ZX_ERR_CANCELED));
        FreeSataTransactionLocked(&txns_[i]);
      }
    }
    txns_allocated_.reset();
  }

  fbl::AutoLock lock(&lock_);
  if (block_server_) {
    block_server_->DestroyAsync([this, callback = std::move(callback)]() mutable {
      {
        fbl::AutoLock lock(&lock_);
        block_server_.reset();
      }
      callback();
    });
  } else {
    callback();
  }
}

void SataDevice::Get(GetCompleter::Sync& completer) {
  zx::event token = controller_->node_token();
  if (token.is_valid()) {
    completer.Reply(zx::ok(std::move(token)));
  } else {
    completer.Reply(zx::error(ZX_ERR_NOT_FOUND));
  }
}

}  // namespace ahci

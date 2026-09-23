// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "block_device.h"

#include <fidl/fuchsia.hardware.block.volume/cpp/fidl.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/ddk/driver.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_offers.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/trace/event.h>

namespace ftl {

BlockDevice::BlockDevice() : fdf::DriverBase2("ftl") {}

BlockDevice::~BlockDevice() {
  std::lock_guard<std::mutex> lock(mutex_);
  if (volume_) {
    volume_->Unmount();
  }
}

zx::result<> BlockDevice::Start(fdf::DriverContext context) {
  node_token_ = context.take_node_token();
  exposed_inspector_.emplace(context.CreateInspector(this));
  metrics_.emplace(exposed_inspector_->root().CreateChild("ftl"));

  auto incoming = std::shared_ptr<fdf::Namespace>(context.take_incoming());

  zx::result<ddk::NandProtocolClient> parent_client =
      compat::ConnectBanjo<ddk::NandProtocolClient>(incoming);
  if (parent_client.is_error()) {
    FDF_LOG(ERROR, "Failed to connect to parent nand protocol: %s", parent_client.status_string());
    return parent_client.take_error();
  }
  parent_client->GetProto(&parent_);

  zx::result<ddk::BadBlockProtocolClient> bad_block_client =
      compat::ConnectBanjo<ddk::BadBlockProtocolClient>(incoming);
  if (bad_block_client.is_ok()) {
    bad_block_client->GetProto(&bad_block_);
  } else {
    FDF_LOG(WARNING, "Parent device does not support bad_block protocol");
  }

  if (!InitFtl()) {
    FDF_LOG(ERROR, "Failed to initialize FTL");
    return zx::error(ZX_ERR_INTERNAL);
  }

  block_server::PartitionInfo partition_info = {
      .device_flags = static_cast<uint32_t>(fuchsia_storage_block::wire::DeviceFlag::kTrimSupport),
      .block_count = params_.num_pages,
      .block_size = params_.page_size,
      .name = "ftl",
      .flags = 0,
      .max_transfer_size = 0,
  };

  memcpy(partition_info.type_guid, guid_, sizeof(partition_info.type_guid));

  {
    std::lock_guard<std::mutex> lock(mutex_);
    block_server_.emplace(partition_info, this);
  }

  // Also add the block server service.
  component::ServiceInstanceHandler handler;
  zx::result<> add_member_result = handler.AddMember<fuchsia_storage_block::Block>(
      [this](fidl::ServerEnd<fuchsia_storage_block::Block> server_end) {
        std::lock_guard<std::mutex> lock(mutex_);
        if (block_server_) {
          block_server_->Serve(std::move(server_end));
        }
      },
      "volume");
  ZX_ASSERT(add_member_result.is_ok());
  zx::result<> add_token_result = handler.AddMember<fuchsia_driver_token::NodeToken>(
      [this](fidl::ServerEnd<fuchsia_driver_token::NodeToken> server_end) {
        fidl::BindServer(dispatcher(), std::move(server_end), this);
      },
      "token");
  ZX_ASSERT(add_token_result.is_ok());
  zx::result<> add_block_result =
      outgoing()->AddService<fuchsia_hardware_block_volume::Service>(std::move(handler));
  if (add_block_result.is_error()) {
    FDF_LOG(ERROR, "Failed to add block protocol: %s", add_block_result.status_string());
    return add_block_result.take_error();
  }

  zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>> node_result =
      AddChild("ftl", fuchsia_driver_framework::NodePropertyVector{},
               std::vector{fdf::MakeOffer2<fuchsia_hardware_block_volume::Service>()});
  if (node_result.is_error()) {
    FDF_LOG(ERROR, "Failed to add child node: %s", node_result.status_string());
    return node_result.take_error();
  }

  ScheduleFlush();
  return zx::ok();
}

void BlockDevice::Stop(fdf::StopCompleter completer) {
  std::lock_guard<std::mutex> lock(mutex_);
  shutdown_ = true;
  if (block_server_) {
    block_server_->DestroyAsync([this, completer = std::move(completer)]() mutable {
      {
        std::lock_guard<std::mutex> lock(mutex_);
        block_server_.reset();
      }
      completer(zx::ok());
    });
  } else {
    completer(zx::ok());
  }
}

void BlockDevice::OnRequests(std::span<block_server::Request> requests) {
  for (auto& request : requests) {
    std::lock_guard<std::mutex> lock(mutex_);
    if (shutdown_) {
      block_server_->SendReply(request.request_id, zx::error(ZX_ERR_CANCELED));
      continue;
    }
    zx_status_t status = ZX_OK;
    ftl::BlockOperationProperties* op_stats = nullptr;
    nand_counters_.Reset();

    TRACE_DURATION_BEGIN("block:ftl", "Operation", "opcode",
                         static_cast<uint32_t>(request.operation.tag));

    switch (request.operation.tag) {
      case block_server::Operation::Tag::Read:
        op_stats = &metrics_->read();
        [[fallthrough]];
      case block_server::Operation::Tag::Write: {
        if (request.operation.tag == block_server::Operation::Tag::Write) {
          op_stats = &metrics_->write();
          pending_flush_ = true;
        }

        status = block_server::CheckIoRange(request, params_.num_pages);
        if (status != ZX_OK) {
          break;
        }
        uint64_t vmo_offset = request.operation.tag == block_server::Operation::Tag::Read
                                  ? request.operation.read.vmo_offset
                                  : request.operation.write.vmo_offset;
        uint64_t dev_offset = request.operation.tag == block_server::Operation::Tag::Read
                                  ? request.operation.read.device_block_offset
                                  : request.operation.write.device_block_offset;
        uint32_t length = request.operation.tag == block_server::Operation::Tag::Read
                              ? request.operation.read.block_count
                              : request.operation.write.block_count;
        if (length > std::numeric_limits<int>::max()) {
          // FTLN library takes int length values
          status = ZX_ERR_OUT_OF_RANGE;
          break;
        }

        fzl::VmoMapper mapper;
        status =
            mapper.Map(*request.vmo, vmo_offset, static_cast<uint64_t>(length) * params_.page_size,
                       ZX_VM_FLAG_PERM_READ | ZX_VM_FLAG_PERM_WRITE | ZX_VM_MAP_RANGE);
        if (status != ZX_OK) {
          break;
        }
        if (request.operation.tag == block_server::Operation::Tag::Write) {
          status = volume_->Write(dev_offset, static_cast<int>(length), mapper.start());
        } else {
          status = volume_->Read(dev_offset, static_cast<int>(length), mapper.start());
        }
        break;
      }
      case block_server::Operation::Tag::Trim: {
        op_stats = &metrics_->trim();
        status = block_server::CheckIoRange(request, params_.num_pages);
        if (status != ZX_OK) {
          break;
        }
        status = volume_->Trim(request.operation.trim.device_block_offset,
                               request.operation.trim.block_count);
        break;
      }
      case block_server::Operation::Tag::Flush: {
        op_stats = &metrics_->flush();
        status = volume_->Flush();
        pending_flush_ = false;
        break;
      }
      default:
        status = ZX_ERR_NOT_SUPPORTED;
        break;
    }

    TRACE_DURATION_END("block:ftl", "Operation", "nand_ops", nand_counters_.GetSum());

    if (op_stats != nullptr) {
      op_stats->count.Add(1);
      op_stats->all.count.Add(nand_counters_.GetSum());
      op_stats->all.rate.Add(nand_counters_.GetSum());
      op_stats->block_erase.count.Add(nand_counters_.block_erase);
      op_stats->block_erase.rate.Add(nand_counters_.block_erase);
      op_stats->page_write.count.Add(nand_counters_.page_write);
      op_stats->page_write.rate.Add(nand_counters_.page_write);
      op_stats->page_read.count.Add(nand_counters_.page_read);
      op_stats->page_read.rate.Add(nand_counters_.page_read);
    }

    Volume::Counters counters;
    if (volume_->GetCounters(&counters) == ZX_OK) {
      metrics_->max_wear().Set(counters.wear_count);
      metrics_->initial_bad_blocks().Set(counters.initial_bad_blocks);
      metrics_->running_bad_blocks().Set(counters.running_bad_blocks);
      metrics_->total_bad_blocks().Set(counters.initial_bad_blocks + counters.running_bad_blocks);
      metrics_->worn_blocks_detected().Set(counters.worn_blocks_detected);
      metrics_->projected_bad_blocks().Set(counters.initial_bad_blocks +
                                           counters.running_bad_blocks +
                                           counters.worn_blocks_detected);
    }

    if (block_server_) {
      block_server_->SendReply(request.request_id, zx::make_result(status));
    }
  }
}

void BlockDevice::DoBackgroundFlush() {
  std::lock_guard<std::mutex> lock(mutex_);
  if (shutdown_) {
    return;
  }
  if (pending_flush_) {
    zx_status_t status = volume_->Flush();
    if (status != ZX_OK) {
      FDF_LOG(ERROR, "FTL: background flush failed: %s", zx_status_get_string(status));
    }
    pending_flush_ = false;
  }
  ScheduleFlush();
}

void BlockDevice::ScheduleFlush() {
  async::PostDelayedTask(
      fdf::Dispatcher::GetCurrent()->async_dispatcher(), [this]() { DoBackgroundFlush(); },
      zx::sec(15));
}

bool BlockDevice::OnVolumeAdded(uint32_t page_size, uint32_t num_pages) {
  params_ = {page_size, num_pages};
  FDF_LOG(INFO, "FTL: %d pages of %d bytes", num_pages, page_size);
  return true;
}

zx_status_t BlockDevice::FormatInternal() {
  std::lock_guard<std::mutex> lock(mutex_);
  zx_status_t status = volume_->Format();
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "FTL: format failed: %s", zx_status_get_string(status));
  }
  return status;
}

bool BlockDevice::InitFtl() {
  std::lock_guard<std::mutex> lock(mutex_);
  std::unique_ptr<NandDriver> driver =
      NandDriver::CreateWithCounters(&parent_, &bad_block_, &nand_counters_, 0);
  const char* error = driver->Init();
  if (error) {
    FDF_LOG(ERROR, "Failed to init FTL driver: %s", error);
    return false;
  }
  memcpy(guid_, driver->info().partition_guid, ZBI_PARTITION_GUID_LEN);

  if (!volume_) {
    volume_ = std::make_unique<ftl::VolumeImpl>(this);
  }

  error = volume_->Init(std::move(driver));
  if (error) {
    FDF_LOG(ERROR, "Failed to init FTL volume: %s", error);
    return false;
  }

  Volume::Stats stats;
  if (volume_->GetStats(&stats) == ZX_OK) {
    FDF_LOG(INFO, "FTL: Wear count: %u, Garbage level: %d%%", stats.wear_count,
            stats.garbage_level);
    metrics_->max_wear().Set(stats.wear_count);
    metrics_->initial_bad_blocks().Set(stats.initial_bad_blocks);
    metrics_->running_bad_blocks().Set(stats.running_bad_blocks);
    metrics_->total_bad_blocks().Set(stats.initial_bad_blocks + stats.running_bad_blocks);
    metrics_->worn_blocks_detected().Set(stats.worn_blocks_detected);
    metrics_->projected_bad_blocks().Set(stats.initial_bad_blocks + stats.running_bad_blocks +
                                         stats.worn_blocks_detected);

    static_assert(std::size(stats.map_block_end_page_failure_reasons) == Metrics::kReasonCount);
    for (int i = 0; i < Metrics::kReasonCount; ++i) {
      metrics_->map_block_end_page_failure_reason(i).Set(
          stats.map_block_end_page_failure_reasons[i]);
    }
  }

  FDF_LOG(INFO, "FTL: InitFtl ok");
  return true;
}

void BlockDevice::Get(GetCompleter::Sync& completer) {
  if (node_token_.is_valid()) {
    zx::event token;
    if (zx_status_t status = node_token_.duplicate(ZX_RIGHT_SAME_RIGHTS, &token); status == ZX_OK) {
      completer.Reply(zx::ok(std::move(token)));
      return;
    } else {
      completer.Reply(zx::error(status));
    }
  } else {
    completer.Reply(zx::error(ZX_ERR_NOT_FOUND));
  }
}

}  // namespace ftl

FUCHSIA_DRIVER_EXPORT2(ftl::BlockDevice);

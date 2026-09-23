// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "sdmmc-partition-device.h"

#include <fidl/fuchsia.hardware.block.volume/cpp/wire.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/ddk/binding_driver.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/logging/cpp/logger.h>
#include <string.h>
#include <zircon/hw/gpt.h>

#include <bind/fuchsia/cpp/bind.h>

#include "sdmmc-block-device.h"
#include "sdmmc-root-device.h"
#include "sdmmc-types.h"

namespace sdmmc {

PartitionDevice::PartitionDevice(SdmmcBlockDevice* sdmmc_parent,
                                 const fuchsia_storage_block::wire::BlockInfo& block_info,
                                 EmmcPartition partition)
    : sdmmc_parent_(sdmmc_parent), block_info_(block_info), partition_(partition) {
  block_server::PartitionInfo info{
      .device_flags = static_cast<uint32_t>(block_info.flags),
      .block_count = block_info.block_count,
      .block_size = block_info.block_size,
      .max_transfer_size = block_info.max_transfer_size,
  };
  switch (partition_) {
    case USER_DATA_PARTITION: {
      // For compatibility with the old implementation, don't give 'user' a visible name/guid.
      partition_name_ = "user";
      break;
    }
    case BOOT_PARTITION_1: {
      partition_name_ = "boot1";
      info.name = partition_name_;
      const uint8_t guid[16] = GUID_EMMC_BOOT1_VALUE;
      std::copy_n(guid, sizeof(guid), std::begin(info.type_guid));
      break;
    }
    case BOOT_PARTITION_2: {
      partition_name_ = "boot2";
      info.name = partition_name_;
      const uint8_t guid[16] = GUID_EMMC_BOOT2_VALUE;
      std::copy_n(guid, sizeof(guid), std::begin(info.type_guid));
      break;
    }
    default:
      // partition_name_ is left empty, which causes PartitionDevice::AddDevice() to return an
      // error.
      break;
  }
  block_server_.emplace(info, this);
}

zx_status_t PartitionDevice::AddDevice() {
  if (!partition_name_) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  auto [controller_client_end, controller_server_end] =
      fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();

  controller_.Bind(std::move(controller_client_end));

  fidl::Arena arena;

  const auto args = fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena)
                        .name(arena, partition_name_)
                        .Build();

  auto result = sdmmc_parent_->block_node()->AddChild(args, std::move(controller_server_end), {});
  if (!result.ok()) {
    fdf::error("Failed to add child partition device: {}", result.status_string());
    return result.status();
  }

  auto handlers = fuchsia_hardware_block_volume::Service::InstanceHandler({
      .volume =
          [this](fidl::ServerEnd<fuchsia_storage_block::Block> server_end) {
            fbl::AutoLock lock(&lock_);
            if (block_server_)
              block_server_->Serve(std::move(server_end));
          },
      .token =
          [this](fidl::ServerEnd<fuchsia_driver_token::NodeToken> server_end) {
            fidl::BindServer(fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                             std::move(server_end), this);
          },
  });

  if (sdmmc_parent_->SupportsInlineEncryption()) {
    zx::result result = handlers.add_inline_encryption(ice_bindings_.CreateHandler(
        sdmmc_parent_, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
        fidl::kIgnoreBindingClosure));
    ZX_ASSERT(result.is_ok());
  }

  if (zx::result result = sdmmc_parent_->parent()
                              ->driver_outgoing()
                              ->AddService<fuchsia_hardware_block_volume::Service>(
                                  std::move(handlers), partition_name_);
      result.is_error()) {
    fdf::error("Failed to add service instance for '{}': {}", partition_name_, result);
    return result.status_value();
  }

  return ZX_OK;
}

fdf::Logger& PartitionDevice::logger() const { return sdmmc_parent_->logger(); }

void PartitionDevice::StopBlockServer(fit::callback<void()> callback) {
  fbl::AutoLock lock(&lock_);
  if (block_server_) {
    block_server_->DestroyAsync([this, callback = std::move(callback)]() mutable {
      fbl::AutoLock lock(&lock_);
      block_server_.reset();
      callback();
    });
  } else {
    callback();
  }
}

void PartitionDevice::OnRequests(cpp20::span<block_server::Request> requests) {
  sdmmc_parent_->OnRequests(*this, requests);
}

void PartitionDevice::SendReply(block_server::RequestId request, zx::result<> status) {
  fbl::AutoLock lock(&lock_);
  ZX_DEBUG_ASSERT(block_server_);
  if (block_server_) {
    block_server_->SendReply(request, status);
  }
}

void PartitionDevice::Get(GetCompleter::Sync& completer) {
  zx::event token = sdmmc_parent_->parent()->node_token();
  if (token.is_valid()) {
    completer.Reply(zx::ok(std::move(token)));
  } else {
    completer.Reply(zx::error(ZX_ERR_NOT_FOUND));
  }
}

}  // namespace sdmmc

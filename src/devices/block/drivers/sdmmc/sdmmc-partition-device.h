// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_SDMMC_SDMMC_PARTITION_DEVICE_H_
#define SRC_DEVICES_BLOCK_DRIVERS_SDMMC_SDMMC_PARTITION_DEVICE_H_

#include <fidl/fuchsia.driver.token/cpp/fidl.h>
#include <fidl/fuchsia.hardware.block.volume/cpp/wire.h>
#include <fidl/fuchsia.hardware.inlineencryption/cpp/wire.h>
#include <lib/driver/component/cpp/driver_base.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <zircon/types.h>

#include <fbl/auto_lock.h>

#include "sdmmc-types.h"
#include "src/storage/lib/block_server/block_server.h"

namespace sdmmc {

class SdmmcBlockDevice;

class PartitionDevice : public block_server::DriverInterface,
                        public fidl::Server<fuchsia_driver_token::NodeToken> {
 public:
  PartitionDevice(SdmmcBlockDevice* sdmmc_parent,
                  const fuchsia_storage_block::wire::BlockInfo& block_info,
                  EmmcPartition partition);

  zx_status_t AddDevice();

  EmmcPartition partition() const { return partition_; }
  fuchsia_storage_block::wire::BlockInfo block_info() const { return block_info_; }

  // fuchsia_driver_token::NodeToken implementation.
  void Get(GetCompleter::Sync& completer) override;

  void SendReply(block_server::RequestId, zx::result<>);

  void StopBlockServer(fit::callback<void()> callback);

  // block_server::DriverInterface
  std::string_view SessionSchedulerRole() const final {
    return "fuchsia.devices.block.drivers.sdmmc.worker";
  }
  void OnRequests(cpp20::span<block_server::Request>) final;
  fdf::Logger& logger() const final;

 private:
  SdmmcBlockDevice* const sdmmc_parent_;
  const fuchsia_storage_block::wire::BlockInfo block_info_;
  const EmmcPartition partition_;

  const char* partition_name_ = nullptr;
  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> controller_;
  fidl::ServerBindingGroup<fuchsia_hardware_inlineencryption::Device> ice_bindings_;

  fbl::Mutex lock_;
  std::optional<block_server::BlockServer> block_server_ TA_GUARDED(lock_);
};

}  // namespace sdmmc

#endif  // SRC_DEVICES_BLOCK_DRIVERS_SDMMC_SDMMC_PARTITION_DEVICE_H_

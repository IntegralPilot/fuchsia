// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_LIB_SCSI_INCLUDE_LIB_SCSI_BLOCK_DEVICE_H_
#define SRC_DEVICES_BLOCK_LIB_SCSI_INCLUDE_LIB_SCSI_BLOCK_DEVICE_H_

#include <fidl/fuchsia.driver.token/cpp/fidl.h>
#include <fidl/fuchsia.hardware.block.volume/cpp/wire.h>
#include <lib/driver/compat/cpp/compat.h>
#include <lib/driver/component/cpp/driver_base.h>
#include <lib/fit/function.h>
#include <lib/scsi/controller.h>
#include <lib/sync/cpp/completion.h>
#include <stdint.h>

#include <optional>

#include <fbl/string_printf.h>

#include "src/storage/lib/block_server/block_server.h"

namespace scsi {

struct DeviceOptions {
  static DeviceOptions Default() {
    return DeviceOptions(/*check_unmap_support=*/false, /*use_mode_sense_6*/ true,
                         /*use_read_write_12*/ false);
  }

  explicit DeviceOptions(bool check_unmap_support, bool use_mode_sense_6, bool use_read_write_12)
      : check_unmap_support(check_unmap_support),
        use_mode_sense_6(use_mode_sense_6),
        use_read_write_12(use_read_write_12) {}
  DeviceOptions() = delete;

  bool check_unmap_support;
  bool use_mode_sense_6;
  bool use_read_write_12;
};

// |BlockDevice| represents a single SCSI direct access block device.
// |BlockDevice| bridges between the Zircon block protocol and SCSI commands/responses.
class BlockDevice : public block_server::DriverInterface,
                    public fidl::Server<fuchsia_driver_token::NodeToken> {
 public:
  // Public so that we can use make_unique.
  // Clients should use BlockDevice::Bind().
  BlockDevice(Controller* controller, uint8_t target, uint16_t lun, DeviceOptions device_options)
      : controller_(controller), target_(target), lun_(lun), device_options_(device_options) {}

  // Create a BlockDevice at a specific target/lun.
  // |controller| is a pointer to the scsi::Controller this BlockDevice is attached to.
  // |controller| must outlast BlockDevice.
  // This BlockDevice does not take ownership of or any references on |controller|.
  // A |max_transfer_bytes| value of fuchsia_storage_block::wire::kMaxTransferUnbounded implies
  // there is no limit on the transfer size.
  // Returns a BlockDevice* to allow for removal of removable media disks.
  static zx::result<std::unique_ptr<BlockDevice>> Bind(Controller* controller, uint8_t target,
                                                       uint16_t lun, uint32_t max_transfer_bytes,
                                                       DeviceOptions device_options);

  virtual ~BlockDevice();

  // Asynchronously shut down the block server, invoking `callback` upon completion.
  // This must be called before the `BlockDevice` is deleted.
  virtual void ShutdownAsync(fit::callback<void()> callback);

  // Remove this block device.
  void RemoveDevice() {
    if (auto& outgoing = controller_->driver_outgoing()) {
      (void)outgoing->RemoveService<fuchsia_hardware_block_volume::Service>(DeviceName().c_str());
    }
    if (node_controller_.is_valid()) {
      auto result = node_controller_->Remove();
      if (!result.ok()) {
        FDF_LOGL(ERROR, logger(), "Failed to call Remove on node controller.");
      }
    }
  }

  fbl::String DeviceName() const {
    return fbl::StringPrintf("scsi-block-device-%u-%u", target_, lun_);
  }
  fbl::String LunName() const { return fbl::StringPrintf("lun%u", lun_); }

  // block_server::DriverInterface implementation
  void OnRequests(std::span<block_server::Request> requests) override;
  fdf::Logger& logger() const override;

  // fuchsia_driver_token::NodeToken implementation
  void Get(GetCompleter::Sync& completer) override;

  uint8_t target() const { return target_; }
  uint16_t lun() const { return lun_; }

  bool removable() const { return removable_; }
  bool dpo_fua_available() const { return dpo_fua_available_; }
  bool write_protected() const { return write_protected_; }
  bool write_cache_enabled() const { return write_cache_enabled_; }
  uint64_t block_count() const { return block_count_; }
  uint32_t block_size_bytes() const { return block_size_bytes_; }
  uint32_t max_transfer_bytes() const { return max_transfer_bytes_; }

  BlockDevice(const BlockDevice&) = delete;
  BlockDevice& operator=(const BlockDevice&) = delete;

  // for test
  DeviceOptions& GetDeviceOptions() { return device_options_; }

  // Exposed for testing
  std::optional<block_server::BlockServer>& block_server() { return block_server_; }
  static std::unique_ptr<BlockDevice> CreateForTesting(Controller* controller, uint8_t target,
                                                       uint16_t lun, DeviceOptions device_options,
                                                       uint64_t block_count = 1024,
                                                       uint32_t block_size_bytes = 512) {
    auto dev = std::make_unique<BlockDevice>(controller, target, lun, device_options);
    dev->block_count_ = block_count;
    dev->block_size_bytes_ = block_size_bytes;
    return dev;
  }

 private:
  zx_status_t AddDevice(uint32_t max_transfer_bytes);

  Controller* const controller_;
  const uint8_t target_;
  const uint16_t lun_;
  uint32_t max_transfer_bytes_ = 0;
  uint32_t max_transfer_blocks_ = 0;

  bool removable_ = false;
  bool dpo_fua_available_ = false;
  bool write_protected_ = false;
  bool write_cache_enabled_ = false;

  bool unmap_command_supported_ = false;

  uint64_t block_count_ = 0;
  uint32_t block_size_bytes_ = 0;

  DeviceOptions device_options_;

  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> node_controller_;

  std::optional<block_server::BlockServer> block_server_;

  compat::SyncInitializedDeviceServer compat_server_;
};

}  // namespace scsi

#endif  // SRC_DEVICES_BLOCK_LIB_SCSI_INCLUDE_LIB_SCSI_BLOCK_DEVICE_H_

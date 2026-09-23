// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_FTL_BLOCK_DEVICE_H_
#define SRC_DEVICES_BLOCK_DRIVERS_FTL_BLOCK_DEVICE_H_

#include <fidl/fuchsia.driver.token/cpp/fidl.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <fuchsia/hardware/badblock/c/banjo.h>
#include <fuchsia/hardware/badblock/cpp/banjo.h>
#include <fuchsia/hardware/nand/c/banjo.h>
#include <fuchsia/hardware/nand/cpp/banjo.h>
#include <lib/async/cpp/task.h>
#include <lib/driver/compat/cpp/banjo_client.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/inspect/cpp/vmo/types.h>
#include <lib/zbi-format/partition.h>
#include <lib/zx/event.h>

#include <atomic>
#include <memory>
#include <mutex>

#include "src/devices/block/drivers/ftl/metrics.h"
#include "src/devices/block/drivers/ftl/nand_driver.h"
#include "src/storage/lib/block_server/block_server.h"
#include "src/storage/lib/ftl/ftln/volume.h"

namespace ftl {

struct BlockParams {
  uint64_t GetSize() const { return static_cast<uint64_t>(page_size) * num_pages; }

  uint32_t page_size;
  uint32_t num_pages;
};

class BlockDevice : public fdf::DriverBase2,
                    public block_server::DriverInterface,
                    public ftl::FtlInstance,
                    public fidl::Server<fuchsia_driver_token::NodeToken> {
 public:
  BlockDevice();
  ~BlockDevice() override;

  zx::result<> Start(fdf::DriverContext context) override;
  void Stop(fdf::StopCompleter completer) override;

  // fuchsia_driver_token::NodeToken implementation.
  void Get(GetCompleter::Sync& completer) override;

  // block_server::DriverInterface implementation.
  void OnRequests(std::span<block_server::Request> requests) override;
  std::string_view SessionSchedulerRole() const override {
    return "fuchsia.devices.block.drivers.ftl.device";
  }

  // FtlInstance interface.
  bool OnVolumeAdded(uint32_t page_size, uint32_t num_pages) override;

  // Issues a command to format the FTL (aka, delete all data).
  zx_status_t FormatInternal();

  // Returns a read_only handle to the underlying Inspect VMO.
  zx::vmo DuplicateInspectVmo() { return exposed_inspector_->inspector().DuplicateVmo(); }

  OperationCounters& nand_counters() { return nand_counters_; }

  void SetVolumeForTest(std::unique_ptr<ftl::Volume> volume) {
    std::lock_guard<std::mutex> lock(mutex_);
    volume_ = std::move(volume);
  }

  void SetNandParentForTest(const nand_protocol_t& nand) { parent_ = nand; }

 private:
  bool InitFtl();

  std::mutex mutex_;

  BlockParams params_ = {};

  nand_protocol_t parent_ = {};
  bad_block_protocol_t bad_block_ = {};

  std::unique_ptr<ftl::Volume> volume_ __TA_GUARDED(mutex_);

  uint8_t guid_[ZBI_PARTITION_GUID_LEN] = {};

  std::optional<Metrics> metrics_;
  std::optional<inspect::ComponentInspector> exposed_inspector_;
  zx::event node_token_;

  // Keeps track of the nand operations being issued for each incoming block operation.
  OperationCounters nand_counters_ __TA_GUARDED(mutex_);

  std::optional<block_server::BlockServer> block_server_ __TA_GUARDED(mutex_);
  bool shutdown_ __TA_GUARDED(mutex_) = false;
  void DoBackgroundFlush();
  void ScheduleFlush();
  bool pending_flush_ __TA_GUARDED(mutex_) = false;
};

}  // namespace ftl

#endif  // SRC_DEVICES_BLOCK_DRIVERS_FTL_BLOCK_DEVICE_H_

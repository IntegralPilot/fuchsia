// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_NVME_NAMESPACE_H_
#define SRC_DEVICES_BLOCK_DRIVERS_NVME_NAMESPACE_H_

#include <fidl/fuchsia.driver.token/cpp/fidl.h>
#include <fidl/fuchsia.hardware.block.volume/cpp/wire.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fit/function.h>
#include <lib/zircon-internal/thread_annotations.h>

#include <bitset>
#include <functional>

#include <fbl/auto_lock.h>
#include <fbl/condition_variable.h>
#include <fbl/mutex.h>
#include <fbl/string_printf.h>

#include "src/devices/block/drivers/nvme/io-command.h"
#include "src/storage/lib/block_server/block_server.h"

namespace nvme {

class Nvme;

class Namespace : public block_server::DriverInterface,
                  public fidl::Server<fuchsia_driver_token::NodeToken> {
 public:
  explicit Namespace(Nvme* controller, uint32_t namespace_id)
      : controller_(controller), namespace_id_(namespace_id) {}
  ~Namespace();

  // Create a namespace on |controller| with |namespace_id|.
  static zx::result<std::unique_ptr<Namespace>> Bind(Nvme* controller, uint32_t namespace_id);
  fbl::String NamespaceName() const { return fbl::StringPrintf("namespace-%u", namespace_id_); }

  void OnRequests(std::span<block_server::Request> requests) override;
  fdf::Logger& logger() const override;

  // fuchsia_driver_token::NodeToken implementation.
  void Get(GetCompleter::Sync& completer) override;

  void CompleteIoCommand(IoCommand* io_cmd, zx_status_t status);

  void ServeRequests(fidl::ServerEnd<fuchsia_storage_block::Block> server_end);

  void StopBlockServer(fit::callback<void()> callback);

  bool HasInflightCommands() {
    fbl::AutoLock lock(&lock_);
    return io_command_bitmap_.any();
  }

 private:
  // Invokes AddChild().
  zx_status_t AddNamespace();

  // Main driver initialization.
  zx_status_t Init();

  zx::result<std::reference_wrapper<IoCommand>> AllocateIoCommand() TA_REQ(lock_);
  void FreeIoCommand(IoCommand* io_cmd) TA_REQ(lock_);

  Nvme* const controller_;
  const uint32_t namespace_id_;

  block_server::PartitionInfo block_info_ = {};
  uint32_t max_transfer_blocks_;

  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> node_controller_;

  std::optional<block_server::BlockServer> block_server_ TA_GUARDED(lock_);

  static constexpr size_t kMaxRequests = 64;
  std::array<IoCommand, kMaxRequests> io_command_pool_;
  std::bitset<kMaxRequests> io_command_bitmap_ TA_GUARDED(lock_);
  fbl::Mutex lock_;
  fbl::ConditionVariable pool_cond_ TA_GUARDED(lock_);
  bool shutdown_ TA_GUARDED(lock_) = false;
};

}  // namespace nvme

#endif  // SRC_DEVICES_BLOCK_DRIVERS_NVME_NAMESPACE_H_

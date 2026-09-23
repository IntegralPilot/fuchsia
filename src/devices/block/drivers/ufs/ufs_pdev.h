// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_UFS_UFS_PDEV_H_
#define SRC_DEVICES_BLOCK_DRIVERS_UFS_UFS_PDEV_H_

#include <fidl/fuchsia.hardware.interconnect/cpp/wire.h>
#include <fidl/fuchsia.hardware.platform.device/cpp/wire.h>
#include <fidl/fuchsia.hardware.ufs.phy/cpp/fidl.h>
#include <lib/driver/logging/cpp/logger.h>

#include "src/devices/block/drivers/ufs/ufs.h"

namespace ufs {

class UfsPdev final : public Ufs, public fidl::Server<fuchsia_hardware_ufs_phy::Ufshci> {
 public:
  using Ufs::Ufs;

 protected:
  zx::result<> InitResources() override;
  zx_status_t StopResources() override;

  zx::result<> InitQuirk() override;

  zx::result<> PdevNotifyEventCallback(NotifyEvent event, uint64_t data);
  zx::result<> PreLinkStartup();
  zx::result<> PrePowerModeChange();

  zx::result<fidl::ClientEnd<fuchsia_hardware_ufs_phy::Ufshci>> StartUfshciServer();
  void StopUfshciServer();

  // Ufshci Server methods
  void DmeSet(DmeSetRequest& request, DmeSetCompleter::Sync& completer) override;
  void DmeGet(DmeGetRequest& request, DmeGetCompleter::Sync& completer) override;
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_hardware_ufs_phy::Ufshci> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    fdf::error("Unknown method in Ufshci server: {}", metadata.method_ordinal);
  }

 private:
  fidl::WireSyncClient<fuchsia_hardware_ufs_phy::UfsPhy> ufs_phy_;
  fidl::WireSyncClient<fuchsia_hardware_interconnect::Path> interconnect_client_;

  fdf::Dispatcher ufshci_dispatcher_;
  libsync::Completion ufshci_dispatcher_shutdown_completion_;
};

}  // namespace ufs

#endif  // SRC_DEVICES_BLOCK_DRIVERS_UFS_UFS_PDEV_H_

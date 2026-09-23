// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_UFS_RPMB_H_
#define SRC_DEVICES_BLOCK_DRIVERS_UFS_RPMB_H_

#include <fidl/fuchsia.hardware.rpmb/cpp/wire.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/compat/cpp/compat.h>
#include <lib/driver/component/cpp/driver_base.h>
#include <lib/fidl/cpp/wire/server.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/zircon-internal/thread_annotations.h>

#include <mutex>

namespace ufs {

class Ufs;

class UfsRpmbDevice : public fidl::WireServer<fuchsia_hardware_rpmb::Rpmb> {
 public:
  static constexpr char kDeviceName[] = "ufs.rpmb";
  static constexpr uint64_t kMaxRpmbTransferSize = 8192;  // 8KB max
  static constexpr uint64_t kRpmbFrameSize = 512;
  static constexpr uint64_t kMaxFramesPerTransfer = 2;

  explicit UfsRpmbDevice(Ufs* ufs_parent) : ufs_parent_(ufs_parent) {}

  zx_status_t AddDevice();
  void Serve(fidl::ServerEnd<fuchsia_hardware_rpmb::Rpmb> request);

  // fidl::WireServer<fuchsia_hardware_rpmb::Rpmb>
  void GetDeviceInfo(GetDeviceInfoCompleter::Sync& completer) override;
  void Request(RequestRequestView request, RequestCompleter::Sync& completer) override
      TA_EXCL(lock_);

 private:
  Ufs* const ufs_parent_;
  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> controller_;
  compat::SyncInitializedDeviceServer compat_server_;

  std::mutex lock_;
  zx::vmo dma_vmo_ TA_GUARDED(lock_);            // Pre-allocated DMA VMO
  fzl::VmoMapper dma_mapper_ TA_GUARDED(lock_);  // Mapper for DMA VMO
  fidl::ServerBindingGroup<fuchsia_hardware_rpmb::Rpmb> bindings_;
};

}  // namespace ufs

#endif  // SRC_DEVICES_BLOCK_DRIVERS_UFS_RPMB_H_

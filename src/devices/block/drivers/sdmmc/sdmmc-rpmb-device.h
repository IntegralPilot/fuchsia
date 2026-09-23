// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_DEVICES_BLOCK_DRIVERS_SDMMC_SDMMC_RPMB_DEVICE_H_
#define SRC_DEVICES_BLOCK_DRIVERS_SDMMC_SDMMC_RPMB_DEVICE_H_

#include <fidl/fuchsia.hardware.rpmb/cpp/driver/wire.h>
#include <fidl/fuchsia.hardware.rpmb/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/compat/cpp/compat.h>
#include <lib/driver/component/cpp/driver_base.h>
#include <lib/sdmmc/hw.h>

#include <array>

namespace sdmmc {

class SdmmcBlockDevice;

class RpmbDeviceBase {
 public:
  RpmbDeviceBase(SdmmcBlockDevice* sdmmc_parent, const std::array<uint8_t, SDMMC_CID_SIZE>& cid,
                 const std::array<uint8_t, MMC_EXT_CSD_SIZE>& ext_csd)
      : sdmmc_parent_(sdmmc_parent),
        cid_(cid),
        rpmb_size_(ext_csd[MMC_EXT_CSD_RPMB_SIZE_MULT]),
        reliable_write_sector_count_(ext_csd[MMC_EXT_CSD_REL_WR_SEC_C]) {}

 protected:
  fuchsia_hardware_rpmb::wire::EmmcDeviceInfo GetDeviceInfo();
  void Request(fuchsia_hardware_rpmb::wire::Request request,
               fit::callback<void(zx_status_t)> callback);
  SdmmcBlockDevice* sdmmc_parent() { return sdmmc_parent_; }

 private:
  SdmmcBlockDevice* const sdmmc_parent_;
  const std::array<uint8_t, SDMMC_CID_SIZE> cid_;
  const uint8_t rpmb_size_;
  const uint8_t reliable_write_sector_count_;
};

class RpmbDevice : public RpmbDeviceBase, public fidl::WireServer<fuchsia_hardware_rpmb::Rpmb> {
 public:
  static constexpr char kDeviceName[] = "rpmb";

  RpmbDevice(SdmmcBlockDevice* sdmmc_parent, const std::array<uint8_t, SDMMC_CID_SIZE>& cid,
             const std::array<uint8_t, MMC_EXT_CSD_SIZE>& ext_csd)
      : RpmbDeviceBase(sdmmc_parent, cid, ext_csd) {}

  zx_status_t AddDevice();
  void Serve(fidl::ServerEnd<fuchsia_hardware_rpmb::Rpmb> request);

  void GetDeviceInfo(GetDeviceInfoCompleter::Sync& completer) override;
  void Request(RequestRequestView request, RequestCompleter::Sync& completer) override;

 private:
  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> controller_;
  compat::SyncInitializedDeviceServer compat_server_;
};

class DriverRpmbDevice : public RpmbDeviceBase,
                         public fdf::WireServer<fuchsia_hardware_rpmb::DriverRpmb> {
 public:
  static constexpr char kDeviceName[] = "rpmb";

  DriverRpmbDevice(SdmmcBlockDevice* sdmmc_parent, const std::array<uint8_t, SDMMC_CID_SIZE>& cid,
                   const std::array<uint8_t, MMC_EXT_CSD_SIZE>& ext_csd)
      : RpmbDeviceBase(sdmmc_parent, cid, ext_csd) {}

  void GetDeviceInfo(fdf::Arena& arena, GetDeviceInfoCompleter::Sync& completer) override;
  void Request(RequestRequestView request, fdf::Arena& arena,
               RequestCompleter::Sync& completer) override;
};

}  // namespace sdmmc

#endif  // SRC_DEVICES_BLOCK_DRIVERS_SDMMC_SDMMC_RPMB_DEVICE_H_

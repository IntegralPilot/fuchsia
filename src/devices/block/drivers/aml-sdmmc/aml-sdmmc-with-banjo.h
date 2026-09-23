// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_AML_SDMMC_AML_SDMMC_WITH_BANJO_H_
#define SRC_DEVICES_BLOCK_DRIVERS_AML_SDMMC_AML_SDMMC_WITH_BANJO_H_

#include <fuchsia/hardware/sdmmc/cpp/banjo.h>
#include <zircon/compiler.h>

#include "aml-sdmmc.h"

namespace aml_sdmmc {

class AmlSdmmcWithBanjo : public AmlSdmmc, public ddk::SdmmcProtocol<AmlSdmmcWithBanjo> {
 public:
  explicit AmlSdmmcWithBanjo() : AmlSdmmc() {}

  // ddk::SdmmcProtocol implementation
  zx_status_t SdmmcHostInfo(sdmmc_host_info_t* out_info);
  zx_status_t SdmmcSetSignalVoltage(sdmmc_voltage_t voltage);
  zx_status_t SdmmcSetBusWidth(sdmmc_bus_width_t bus_width) __TA_EXCLUDES(lock_);
  zx_status_t SdmmcSetBusFreq(uint32_t bus_freq) __TA_EXCLUDES(lock_);
  zx_status_t SdmmcSetTiming(sdmmc_timing_t timing) __TA_EXCLUDES(lock_);
  zx_status_t SdmmcHwReset() __TA_EXCLUDES(lock_);
  zx_status_t SdmmcPerformTuning(uint32_t cmd_idx) __TA_EXCLUDES(tuning_lock_);
  zx_status_t SdmmcRegisterVmo(uint32_t vmo_id, uint8_t client_id, zx::vmo vmo, uint64_t offset,
                               uint64_t size, uint32_t vmo_rights) __TA_EXCLUDES(lock_);
  zx_status_t SdmmcUnregisterVmo(uint32_t vmo_id, uint8_t client_id, zx::vmo* out_vmo)
      __TA_EXCLUDES(lock_);
  zx_status_t SdmmcRequest(const sdmmc_req_t* req, uint32_t out_response[4]) __TA_EXCLUDES(lock_);

 private:
  // Translates a Banjo sdmmc request (sdmmc_req_t) into a FIDL one
  // (fuchsia_hardware_sdmmc::wire::SdmmcReq).
  zx::result<fuchsia_hardware_sdmmc::wire::SdmmcReq> BanjoToFidlReq(const sdmmc_req_t& banjo_req,
                                                                    fdf::Arena* arena);

  std::optional<compat::DeviceServer::BanjoConfig> get_banjo_config() override {
    compat::DeviceServer::BanjoConfig config{ZX_PROTOCOL_SDMMC};
    config.callbacks[ZX_PROTOCOL_SDMMC] = banjo_server_.callback();
    return config;
  }

  compat::BanjoServer banjo_server_{ZX_PROTOCOL_SDMMC, this, &sdmmc_protocol_ops_};
};

}  // namespace aml_sdmmc

#endif  // SRC_DEVICES_BLOCK_DRIVERS_AML_SDMMC_AML_SDMMC_WITH_BANJO_H_

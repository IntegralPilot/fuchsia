// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "aml-sdmmc-with-banjo.h"

#include <lib/driver/logging/cpp/logger.h>

namespace aml_sdmmc {

zx::result<fuchsia_hardware_sdmmc::wire::SdmmcReq> AmlSdmmcWithBanjo::BanjoToFidlReq(
    const sdmmc_req_t& banjo_req, fdf::Arena* arena) {
  fuchsia_hardware_sdmmc::wire::SdmmcReq wire_req;

  wire_req.cmd_idx = banjo_req.cmd_idx;
  wire_req.cmd_flags = banjo_req.cmd_flags;
  wire_req.arg = banjo_req.arg;
  wire_req.blocksize = banjo_req.blocksize;
  wire_req.suppress_error_messages = banjo_req.suppress_error_messages;
  wire_req.client_id = banjo_req.client_id;

  wire_req.buffers.Allocate(*arena, banjo_req.buffers_count);
  for (size_t i = 0; i < banjo_req.buffers_count; i++) {
    if (banjo_req.buffers_list[i].type == SDMMC_BUFFER_TYPE_VMO_ID) {
      wire_req.buffers[i].buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmoId(
          banjo_req.buffers_list[i].buffer.vmo_id);
    } else {
      if (banjo_req.buffers_list[i].type != SDMMC_BUFFER_TYPE_VMO_HANDLE) {
        return zx::error(ZX_ERR_INVALID_ARGS);
      }
      zx::vmo dup;
      zx_status_t status = zx_handle_duplicate(banjo_req.buffers_list[i].buffer.vmo,
                                               ZX_RIGHT_SAME_RIGHTS, dup.reset_and_get_address());
      if (status != ZX_OK) {
        fdf::error("Failed to duplicate vmo: {}", zx_status_get_string(status));
        return zx::error(status);
      }
      wire_req.buffers[i].buffer =
          fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmo(std::move(dup));
    }

    wire_req.buffers[i].offset = banjo_req.buffers_list[i].offset;
    wire_req.buffers[i].size = banjo_req.buffers_list[i].size;
  }
  return zx::ok(std::move(wire_req));
}

zx_status_t AmlSdmmcWithBanjo::SdmmcHostInfo(sdmmc_host_info_t* info) {
  info->caps = static_cast<uint64_t>(dev_info_.caps);
  info->max_transfer_size = dev_info_.max_transfer_size;
  info->max_buffer_regions = dev_info_.max_buffer_regions;
  return ZX_OK;
}

zx_status_t AmlSdmmcWithBanjo::SdmmcSetSignalVoltage(sdmmc_voltage_t voltage) {
  // Mirroring AmlSdmmc::SetSignalVoltage().
  return ZX_OK;
}

zx_status_t AmlSdmmcWithBanjo::SdmmcSetBusWidth(sdmmc_bus_width_t bus_width) {
  std::lock_guard<std::mutex> lock(lock_);

  if (power_suspended_) {
    fdf::error("Rejecting SdmmcSetBusWidth (Banjo) while power is suspended.");
    return ZX_ERR_BAD_STATE;
  }

  // Only handling the cases for AmlSdmmc::SdmmcSetBusWidthImpl() to work correctly.
  fuchsia_hardware_sdmmc::wire::SdmmcBusWidth wire_bus_width;
  switch (bus_width) {
    case SDMMC_BUS_WIDTH_EIGHT:
      wire_bus_width = fuchsia_hardware_sdmmc::wire::SdmmcBusWidth::kEight;
      break;
    case SDMMC_BUS_WIDTH_FOUR:
      wire_bus_width = fuchsia_hardware_sdmmc::wire::SdmmcBusWidth::kFour;
      break;
    case SDMMC_BUS_WIDTH_ONE:
      wire_bus_width = fuchsia_hardware_sdmmc::wire::SdmmcBusWidth::kOne;
      break;
    default:
      wire_bus_width = fuchsia_hardware_sdmmc::wire::SdmmcBusWidth::kMax;
      break;
  }

  return SetBusWidthImpl(wire_bus_width);
}

zx_status_t AmlSdmmcWithBanjo::SdmmcSetBusFreq(uint32_t bus_freq) {
  std::lock_guard<std::mutex> lock(lock_);

  if (power_suspended_) {
    fdf::error("Rejecting SdmmcSetBusFreq (Banjo) while power is suspended.");
    return ZX_ERR_BAD_STATE;
  }

  return SetBusFreqImpl(bus_freq);
}

zx_status_t AmlSdmmcWithBanjo::SdmmcSetTiming(sdmmc_timing_t timing) {
  std::lock_guard<std::mutex> lock(lock_);

  if (power_suspended_) {
    fdf::error("Rejecting SdmmcSetTiming (Banjo) while power is suspended.");
    return ZX_ERR_BAD_STATE;
  }

  // Only handling the cases for AmlSdmmc::SdmmcSetTimingImpl() to work correctly.
  fuchsia_hardware_sdmmc::wire::SdmmcTiming wire_timing;
  switch (timing) {
    case SDMMC_TIMING_HS400:
      wire_timing = fuchsia_hardware_sdmmc::wire::SdmmcTiming::kHs400;
      break;
    case SDMMC_TIMING_HSDDR:
      wire_timing = fuchsia_hardware_sdmmc::wire::SdmmcTiming::kHsddr;
      break;
    case SDMMC_TIMING_DDR50:
      wire_timing = fuchsia_hardware_sdmmc::wire::SdmmcTiming::kDdr50;
      break;
    default:
      wire_timing = fuchsia_hardware_sdmmc::wire::SdmmcTiming::kMax;
      break;
  }

  return SetTimingImpl(wire_timing);
}

zx_status_t AmlSdmmcWithBanjo::SdmmcHwReset() {
  std::lock_guard<std::mutex> lock(lock_);

  if (power_suspended_) {
    fdf::error("Rejecting SdmmcHwReset (Banjo) while power is suspended.");
    return ZX_ERR_BAD_STATE;
  }

  return HwResetImpl();
}

zx_status_t AmlSdmmcWithBanjo::SdmmcPerformTuning(uint32_t tuning_cmd_idx) {
  std::lock_guard<std::mutex> tuning_lock(tuning_lock_);

  {
    std::lock_guard<std::mutex> lock(lock_);
    if (power_suspended_) {
      fdf::error("Rejecting SdmmcPerformTuning (Banjo) while power is suspended.");
      return ZX_ERR_BAD_STATE;
    }
  }

  return PerformTuningImpl(tuning_cmd_idx);
}

zx_status_t AmlSdmmcWithBanjo::SdmmcRegisterVmo(uint32_t vmo_id, uint8_t client_id, zx::vmo vmo,
                                                uint64_t offset, uint64_t size,
                                                uint32_t vmo_rights) {
  return RegisterVmoImpl(vmo_id, client_id, std::move(vmo), offset, size,
                         static_cast<fuchsia_hardware_sdmmc::SdmmcVmoRight>(vmo_rights));
}

zx_status_t AmlSdmmcWithBanjo::SdmmcUnregisterVmo(uint32_t vmo_id, uint8_t client_id,
                                                  zx::vmo* out_vmo) {
  return UnregisterVmoImpl(vmo_id, client_id, out_vmo);
}

zx_status_t AmlSdmmcWithBanjo::SdmmcRequest(const sdmmc_req_t* req, uint32_t out_response[4]) {
  std::lock_guard<std::mutex> lock(lock_);
  if (power_suspended_) {
    fdf::error("Rejecting SdmmcRequest (Banjo) while power is suspended.");
    return ZX_ERR_BAD_STATE;
  }

  fdf::Arena arena('AMSD');
  zx::result<fuchsia_hardware_sdmmc::wire::SdmmcReq> wire_req = BanjoToFidlReq(*req, &arena);
  if (wire_req.is_error()) {
    return wire_req.error_value();
  }

  return RequestImpl(*wire_req, out_response);
}

}  // namespace aml_sdmmc

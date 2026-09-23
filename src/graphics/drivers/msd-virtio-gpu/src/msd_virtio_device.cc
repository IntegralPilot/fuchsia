// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "msd_virtio_device.h"

#include <lib/magma/util/macros.h>

#include "src/graphics/drivers/msd-virtio-gpu/include/magma-virtio-gpu-defs.h"
#include "src/graphics/lib/virtio/virtio-abi.h"

MsdVirtioDevice::MsdVirtioDevice(VirtioGpuControl* control) : control_(control) {}

magma_status_t MsdVirtioDevice::MsdGetIcdList(std::vector<msd::MsdIcdInfo>* icd_info_out) {
  MAGMA_DMESSAGE("MsdVirtioDevice::GetIcdList");

  icd_info_out->clear();
  icd_info_out->push_back({
      .component_url = "fuchsia-pkg://fuchsia.com/libvulkan_gfxstream#meta/vulkan.cm",
      .support_flags = msd::ICD_SUPPORT_FLAG_VULKAN,
  });

  return MAGMA_STATUS_OK;
}

zx::result<zx::vmo> MsdVirtioDevice::GetCapset(uint32_t capset_id, uint32_t capset_version) {
  uint64_t capset_count = control_->GetCapabilitySetLimit();
  MAGMA_DMESSAGE("Got capset_count %lu", capset_count);

  std::optional<uint32_t> capset_size;

  for (uint32_t capset_index = 0; capset_index < capset_count; capset_index++) {
    virtio_abi::GetCapsetInfoCommand request = {
        .header = {.type = virtio_abi::ControlType::kGetCapabilitySetInfoCommand},
        .capset_index = capset_index,
    };

    virtio_abi::GetCapsetInfoResponse response;
    auto callback = [&response](cpp20::span<uint8_t> response_bytes) {
      MAGMA_DASSERT(response_bytes.size() == sizeof(response));
      memcpy(&response, response_bytes.data(), sizeof(response));
    };
    auto result = control_->SendHardwareCommand(
        cpp20::span(reinterpret_cast<uint8_t*>(&request), sizeof(request)), callback);
    if (result.is_error()) {
      MAGMA_DMESSAGE("SendHardwareCommand failed: %s", result.status_string());
      return result.take_error();
    }

    if (response.header.type != virtio_abi::ControlType::kCapabilitySetInfoResponse) {
      MAGMA_LOG(ERROR, "Unexpected response: %s (0x%04x)",
                ControlTypeToString(response.header.type),
                static_cast<unsigned int>(response.header.type));
      return zx::error(ZX_ERR_INTERNAL);
    }

    if (response.capset_id == capset_id) {
      capset_size = response.capset_max_size;
      MAGMA_DMESSAGE("Got capset_size %u", *capset_size);
      break;
    }
  }

  if (!capset_size) {
    MAGMA_DMESSAGE("Failed to find capset_id %u", capset_id);
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  virtio_abi::GetCapsetCommand request = {
      .header = {.type = virtio_abi::ControlType::kGetCapabilitySetCommand},
      .capset_id = capset_id,
      .capset_version = capset_version,
  };

  std::vector<uint8_t> response;
  auto callback = [&response](cpp20::span<uint8_t> response_bytes) {
    response.resize(response_bytes.size());
    memcpy(response.data(), response_bytes.data(), response_bytes.size());
  };
  auto result = control_->SendHardwareCommand(
      cpp20::span(reinterpret_cast<uint8_t*>(&request), sizeof(request)), callback);
  if (result.is_error()) {
    MAGMA_DMESSAGE("SendHardwareCommand failed: %s", result.status_string());
    return result.take_error();
  }
  auto response_ptr = reinterpret_cast<virtio_abi::GetCapsetResponse*>(response.data());

  if (response_ptr->header.type != virtio_abi::ControlType::kCapabilitySetResponse) {
    MAGMA_LOG(ERROR, "Unexpected response: %s (0x%04x)",
              ControlTypeToString(response_ptr->header.type),
              static_cast<unsigned int>(response_ptr->header.type));
    return zx::error(ZX_ERR_INTERNAL);
  }

  if (response.size() < capset_size) {
    MAGMA_LOG(ERROR, "Capset response size %zd < expected capset size %u", response.size(),
              *capset_size);
    return zx::error(ZX_ERR_INTERNAL);
  }

  zx::vmo capset_vmo;
  if (zx_status_t status = zx::vmo::create(*capset_size, /*options=*/0, &capset_vmo);
      status != ZX_OK) {
    MAGMA_LOG(ERROR, "zx::vmo::create size %u failed: %d", *capset_size, status);
    return zx::error(status);
  }

  if (zx_status_t status = capset_vmo.write(response_ptr->capset_data, /*offset=*/0, *capset_size);
      status != ZX_OK) {
    MAGMA_LOG(ERROR, "zx::vmo::write size %u failed: %d", *capset_size, status);
    return zx::error(status);
  }

  MAGMA_DMESSAGE("Got capset size %u for id %u version %u", *capset_size, capset_id,
                 capset_version);

  return zx::ok(std::move(capset_vmo));
}

magma_status_t MsdVirtioDevice::MsdQuery(uint64_t id, zx::vmo* result_buffer_out,
                                         uint64_t* result_out) {
  MAGMA_DMESSAGE("MsdVirtioDevice::Query id %lu", id);

  // Upper bits may contain parameters
  uint32_t query_id = id & 0xFFFFFFFF;
  switch (query_id) {
    case MAGMA_QUERY_VENDOR_ID: {
      constexpr uint64_t kVendorIdVirtio = 0x1af4;
      *result_out = kVendorIdVirtio;
      if (result_buffer_out) {
        *result_buffer_out = zx::vmo();
      }
      break;
    }

    case kMagmaVirtioGpuQueryCapset: {
      uint32_t capset_id = (id >> 32) & 0xFFFF;
      uint32_t capset_version = (id >> 24) & 0xFFFF;

      zx::result<zx::vmo> result = GetCapset(capset_id, capset_version);
      if (result.is_error()) {
        MAGMA_DMESSAGE("GetCapset failed: %d", result.status_value());
        return MAGMA_STATUS_INTERNAL_ERROR;
      }
      *result_buffer_out = std::move(result.value());
      if (result_out) {
        *result_out = 0;
      }
      break;
    }

    default:
      return MAGMA_STATUS_INVALID_ARGS;
  }

  return MAGMA_STATUS_OK;
}

// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.driver.compat/cpp/wire.h>
#include <fidl/fuchsia.hardware.vsi/cpp/wire.h>
#include <lib/magma/platform/zircon/zircon_platform_device_dfv2.h>
#include <lib/magma/util/short_macros.h>

#include <optional>

#include "msd_vsi_platform_device.h"
#include "parent_device_dfv2.h"

namespace {
std::optional<uint64_t> ReadSramMetadata(ParentDeviceDfv2* parent_device) {
  auto compat_client = parent_device->incoming_->Connect<fuchsia_driver_compat::Service::Device>();
  if (!compat_client.is_ok()) {
    MAGMA_LOG(ERROR, "Error requesting compat protocol: %s", compat_client.status_string());
    return std::nullopt;
  }
  fidl::WireSyncClient compat(std::move(compat_client.value()));
  auto metadata = compat->GetMetadata();
  if (!metadata.ok()) {
    MAGMA_LOG(ERROR, "Error reading metadata: %s", metadata.lossy_description());
    return std::nullopt;
  }
  if (metadata->is_error()) {
    MAGMA_LOG(ERROR, "Error reading metadata: %d", metadata->error_value());
    return std::nullopt;
  }
  for (auto& data : metadata->value()->metadata) {
    if (data.type == 0) {
      uint64_t external_sram_phys_base;
      zx_status_t status = data.data.read(&external_sram_phys_base, 0, sizeof(uint64_t));
      if (status != ZX_OK) {
        MAGMA_LOG(ERROR, "Error reading metadata: %d", status);
        return std::nullopt;
      }
      return external_sram_phys_base;
    }
  }
  return std::nullopt;
}
}  // namespace

class MsdVsiPlatformDeviceZircon : public MsdVsiPlatformDevice {
 public:
  MsdVsiPlatformDeviceZircon(std::unique_ptr<magma::PlatformDevice> platform_device,
                             std::optional<uint64_t> external_sram_phys_base,
                             fidl::ClientEnd<fuchsia_hardware_vsi::PowerParent> parent)
      : MsdVsiPlatformDevice(std::move(platform_device)),
        external_sram_phys_base_(external_sram_phys_base),
        parent_(std::move(parent)) {}

  std::optional<uint64_t> GetExternalSramPhysicalBase() const override {
    return external_sram_phys_base_;
  }

  magma_status_t ResetPower() override {
    auto result = fidl::WireCall(parent_)->Reset();
    if (!result.ok()) {
      return MAGMA_STATUS_INTERNAL_ERROR;
    }
    return MAGMA_STATUS_OK;
  }

 private:
  std::optional<uint64_t> external_sram_phys_base_;
  fidl::ClientEnd<fuchsia_hardware_vsi::PowerParent> parent_;
};

std::unique_ptr<MsdVsiPlatformDevice> MsdVsiPlatformDevice::Create(void* platform_device_handle) {
  ParentDeviceDfv2* parent_device = reinterpret_cast<ParentDeviceDfv2*>(platform_device_handle);
  zx::result platform_device_client =
      parent_device->incoming_->Connect<fuchsia_hardware_platform_device::Service::Device>();
  if (!platform_device_client.is_ok()) {
    return DRETP(nullptr, "Error requesting platform device protocol: %s",
                 platform_device_client.status_string());
  }

  zx::result nna_client =
      parent_device->incoming_->Connect<fuchsia_hardware_vsi::PowerParentService::Power>();
  if (!nna_client.is_ok()) {
    return DRETP(nullptr, "Error requesting aml nna: %s", nna_client.status_string());
  }

  std::unique_ptr platform_device = std::make_unique<magma::ZirconPlatformDeviceDfv2>(
      fidl::WireSyncClient(std::move(platform_device_client.value())));

  return std::make_unique<MsdVsiPlatformDeviceZircon>(
      std::move(platform_device), ReadSramMetadata(parent_device), std::move(*nna_client));
}

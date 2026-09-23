// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.platform.device/cpp/fidl.h>
#include <fuchsia/hardware/powerimpl/cpp/banjo.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/compat/cpp/compat.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/metadata/cpp/metadata_server.h>
#include <zircon/errors.h>

#include <memory>

namespace power {

class TestPowerDriver : public fdf::DriverBase2, public ddk::PowerImplProtocol<TestPowerDriver> {
 public:
  static constexpr std::string_view kChildNodeName = "test-power";
  static constexpr std::string_view kDriverName = "test-power";

  TestPowerDriver() : fdf::DriverBase2(kDriverName) {}

  // fdf::DriverBase2 implementation.
  zx::result<> Start(fdf::DriverContext context) override;

  zx_status_t PowerImplEnablePowerDomain(uint32_t index);
  zx_status_t PowerImplDisablePowerDomain(uint32_t index);
  zx_status_t PowerImplGetPowerDomainStatus(uint32_t index, power_domain_status_t* out_status);
  zx_status_t PowerImplGetSupportedVoltageRange(uint32_t index, uint32_t* min_voltage,
                                                uint32_t* max_voltage);

  zx_status_t PowerImplRequestVoltage(uint32_t index, uint32_t voltage, uint32_t* actual_voltage);
  zx_status_t PowerImplGetCurrentVoltage(uint32_t index, uint32_t* current_voltage);
  zx_status_t PowerImplReadPmicCtrlReg(uint32_t index, uint32_t addr, uint32_t* value);
  zx_status_t PowerImplWritePmicCtrlReg(uint32_t index, uint32_t addr, uint32_t value);

 private:
  // For testing PMIC register read/write
  uint32_t last_index_ = 0;
  uint32_t last_addr_ = 0;
  uint32_t last_value_ = 0;

  // Values for domains with indexes 0 - 3
  uint32_t min_voltage_[4] = {10, 10, 10, 10};
  uint32_t max_voltage_[4] = {1000, 1000, 1000, 1000};
  uint32_t cur_voltage_[4] = {0, 0, 0, 0};
  bool enabled_[4] = {false, false, false, false};

  compat::SyncInitializedDeviceServer compat_server_;
  fidl::ClientEnd<fuchsia_driver_framework::NodeController> child_;
  compat::BanjoServer banjo_server_{ZX_PROTOCOL_POWER_IMPL, this, &power_impl_protocol_ops_};
  fdf_metadata::MetadataServer<fuchsia_hardware_power::DomainMetadata> metadata_server_;
};

zx::result<> TestPowerDriver::Start(fdf::DriverContext context) {
  compat::DeviceServer::BanjoConfig banjo_config{.default_proto_id = ZX_PROTOCOL_POWER_IMPL};
  banjo_config.callbacks[ZX_PROTOCOL_POWER_IMPL] = banjo_server_.callback();

  auto incoming = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  zx::result<> result =
      compat_server_.Initialize(incoming, outgoing(), context.node_name(), kChildNodeName,
                                compat::ForwardMetadata::None(), std::move(banjo_config));
  if (result.is_error()) {
    fdf::error("Failed to initialize compat server: {}", result);
    return result.take_error();
  }

  zx::result pdev = incoming->Connect<fuchsia_hardware_platform_device::Device>();
  if (pdev.is_error()) {
    fdf::error("Failed to connect to platform device: {}", pdev);
    return pdev.take_error();
  }
  if (zx::result result = metadata_server_.ForwardAndServe(*outgoing(), dispatcher(), pdev.value());
      result.is_error()) {
    fdf::error("Failed to forward metadata: {}", result);
    return result.take_error();
  }

  std::vector offers = compat_server_.CreateOffers2();
  std::optional metadata_offer = metadata_server_.CreateOffer();
  if (metadata_offer.has_value()) {
    offers.push_back(std::move(metadata_offer.value()));
  }
  zx::result child =
      AddChild(kChildNodeName, std::vector<fuchsia_driver_framework::NodeProperty2>{}, offers);
  if (child.is_error()) {
    fdf::error("Failed to create child: {}", child);
    return child.take_error();
  }
  child_ = std::move(child.value());

  return zx::ok();
}

zx_status_t TestPowerDriver::PowerImplEnablePowerDomain(uint32_t index) {
  if (index >= 4) {
    return ZX_ERR_INVALID_ARGS;
  }
  enabled_[index] = true;
  fdf::info("Enabling power domain for index {}", index);
  return ZX_OK;
}

zx_status_t TestPowerDriver::PowerImplDisablePowerDomain(uint32_t index) {
  if (index >= 4) {
    return ZX_ERR_INVALID_ARGS;
  }
  if (!enabled_[index]) {
    fdf::error("Power domain is not enabled for index {}", index);
    return ZX_ERR_UNAVAILABLE;
  }
  enabled_[index] = false;
  return ZX_OK;
}

zx_status_t TestPowerDriver::PowerImplGetPowerDomainStatus(uint32_t index,
                                                           power_domain_status_t* out_status) {
  if (index >= 4) {
    return ZX_ERR_INVALID_ARGS;
  }
  *out_status = POWER_DOMAIN_STATUS_DISABLED;
  if (enabled_[index]) {
    *out_status = POWER_DOMAIN_STATUS_ENABLED;
  }
  return ZX_OK;
}

zx_status_t TestPowerDriver::PowerImplGetSupportedVoltageRange(uint32_t index,
                                                               uint32_t* min_voltage,
                                                               uint32_t* max_voltage) {
  if (index >= 4) {
    return ZX_ERR_INVALID_ARGS;
  }
  *min_voltage = min_voltage_[index];
  *max_voltage = max_voltage_[index];
  return ZX_OK;
}

zx_status_t TestPowerDriver::PowerImplRequestVoltage(uint32_t index, uint32_t voltage,
                                                     uint32_t* actual_voltage) {
  if (index >= 4) {
    return ZX_ERR_INVALID_ARGS;
  }
  if (voltage >= min_voltage_[index] && voltage <= max_voltage_[index]) {
    *actual_voltage = voltage;
    cur_voltage_[index] = voltage;
    return ZX_OK;
  }
  return ZX_ERR_INVALID_ARGS;
}

zx_status_t TestPowerDriver::PowerImplGetCurrentVoltage(uint32_t index, uint32_t* current_voltage) {
  if (index >= 4) {
    return ZX_ERR_INVALID_ARGS;
  }
  *current_voltage = cur_voltage_[index];
  return ZX_OK;
}

zx_status_t TestPowerDriver::PowerImplWritePmicCtrlReg(uint32_t index, uint32_t addr,
                                                       uint32_t value) {
  // Save most recent write for read.
  last_index_ = index;
  last_addr_ = addr;
  last_value_ = value;

  return ZX_OK;
}

zx_status_t TestPowerDriver::PowerImplReadPmicCtrlReg(uint32_t index, uint32_t addr,
                                                      uint32_t* value) {
  if (index == last_index_ && addr == last_addr_) {
    *value = last_value_;
    return ZX_OK;
  }
  return ZX_ERR_NOT_SUPPORTED;
}

}  // namespace power

FUCHSIA_DRIVER_EXPORT2(power::TestPowerDriver);

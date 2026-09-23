// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ti-ina231.h"

#include <endian.h>
#include <fidl/fuchsia.hardware.ti.metadata/cpp/fidl.h>
#include <lib/driver/compat/cpp/metadata.h>
#include <lib/driver/component/cpp/driver_export.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/platform-device/cpp/pdev.h>

#include <bind/fuchsia/cpp/bind.h>

namespace {

// Choose 2048 for the calibration value so that the current and shunt voltage registers are the
// same. This results in a power resolution of 6.25 mW with a shunt resistance of 10 milli-ohms.
constexpr uint16_t kCalibrationValue = 2048;

// From the datasheet:
// Current resolution in A/bit = 0.00512 / (calibration value * shunt resistance in ohms)
// Power resolution in W/bit = current resolution in A/bit * 25
//
// We use shunt resistance in micro-ohms, so this becomes:
// Current resolution in A/bit = 5120.0 / (calibration value * shunt resistance in micro-ohms)
// Multiply by kFixedPointFactor to avoid truncation. To get the power in watts, multiply
// kPowerResolution by the power register value, divide by the shunt resistance in micro-ohms, then
// divide again by kFixedPointFactor.
constexpr uint64_t kFixedPointFactor = 1'000;
constexpr uint64_t kPowerResolution = (25ULL * 5'120 * kFixedPointFactor) / kCalibrationValue;
static_assert((kPowerResolution * kCalibrationValue) == (25ULL * 5'120 * kFixedPointFactor));

// Divide the bus voltage limit by this to get the alert limit register value.
constexpr uint64_t kMicrovoltsPerBit = 1'250;

constexpr float kMicrovoltsToVolts = 1000.0f * 1000.0f;
constexpr float kVoltsPerBit = kMicrovoltsToVolts / kMicrovoltsPerBit;

}  // namespace

namespace power_sensor {

enum class TiIna231::Register : uint8_t {
  kConfigurationReg = 0,
  kBusVoltageReg = 2,
  kPowerReg = 3,
  kCalibrationReg = 5,
  kMaskEnableReg = 6,
  kAlertLimitReg = 7,
};

void TiIna231::GetPowerWatts(GetPowerWattsCompleter::Sync& completer) {
  zx::result<uint16_t> power_reg = Read16(Register::kPowerReg);

  if (power_reg.is_error()) {
    completer.Close(power_reg.error_value());
    return;
  }

  const uint64_t power = (power_reg.value() * kPowerResolution) / shunt_resistor_uohms_;
  completer.ReplySuccess(static_cast<float>(power) / kFixedPointFactor);
}
void TiIna231::GetVoltageVolts(GetVoltageVoltsCompleter::Sync& completer) {
  zx::result<uint16_t> voltage_reg;

  voltage_reg = Read16(Register::kBusVoltageReg);

  if (voltage_reg.is_error()) {
    completer.Close(voltage_reg.error_value());
    return;
  }

  completer.ReplySuccess(static_cast<float>(voltage_reg.value()) / kVoltsPerBit);
}

void TiIna231::GetSensorName(GetSensorNameCompleter::Sync& completer) {
  completer.Reply(fidl::StringView::FromExternal(name_));
}

zx::result<> TiIna231::Start(fdf::DriverContext context) {
  auto incoming = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  zx::result i2c = i2c::I2cChannel::FromIncoming(*incoming, "i2c");
  if (i2c.is_error()) {
    fdf::error("Failed to create i2c channel: {}", i2c);
    return i2c.take_error();
  }

  i2c_ = std::move(i2c.value());
  fidl::WireResult name = i2c_.GetName();
  if (name.ok() && name->is_ok()) {
    name_ = name.value()->name.get();
  }

  zx::result pdev_client =
      incoming->Connect<fuchsia_hardware_platform_device::Service::Device>("pdev");
  if (pdev_client.is_error()) {
    fdf::error("Failed to connect to platform device: {}", pdev_client);
    return pdev_client.take_error();
  }
  fdf::PDev pdev(std::move(pdev_client.value()));

  zx::result metadata_result = pdev.GetFidlMetadata<fuchsia_hardware_ti_metadata::Ina231Metadata>();
  if (metadata_result.is_error()) {
    fdf::error("Failed to get metadata: {}", metadata_result);
    return metadata_result.take_error();
  }
  const fuchsia_hardware_ti_metadata::Ina231Metadata& metadata = metadata_result.value();

  if (!metadata.shunt_resistance_microohm().has_value()) {
    fdf::error("Metadata missing `shunt_resistance_microohm` field");
    return zx::error(ZX_ERR_INTERNAL);
  }
  if (!metadata.shunt_voltage_conversion_time().has_value()) {
    fdf::error("Metadata missing `shunt_voltage_conversion_time` field");
    return zx::error(ZX_ERR_INTERNAL);
  }
  if (!metadata.alert().has_value()) {
    fdf::error("Metadata missing `alert` field");
    return zx::error(ZX_ERR_INTERNAL);
  }
  if (!metadata.power_sensor_domain().has_value()) {
    fdf::error("Metadata missing `power_sensor_domain` field");
    return zx::error(ZX_ERR_INTERNAL);
  }
  if (!metadata.mode().has_value()) {
    fdf::error("Metadata missing `mode` field");
    return zx::error(ZX_ERR_INTERNAL);
  }
  if (!metadata.bus_voltage_conversion_time().has_value()) {
    fdf::error("Metadata missing `bus_voltage_conversion_time` field");
    return zx::error(ZX_ERR_INTERNAL);
  }
  if (!metadata.averages().has_value()) {
    fdf::error("Metadata missing `averages` field");
    return zx::error(ZX_ERR_INTERNAL);
  }

  shunt_resistor_uohms_ = metadata.shunt_resistance_microohm().value();
  if (shunt_resistor_uohms_ == 0) {
    fdf::error("Shunt resistance cannot be zero");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  // Keep only the bits that are not defined in the datasheet, and clear the reset bit.
  constexpr uint16_t kConfigurationRegMask = 0x7000;

  if (zx::result result = Write16(Register::kCalibrationReg, kCalibrationValue);
      result.is_error()) {
    return result.take_error();
  }
  fuchsia_hardware_ti_metadata::Alert alert = metadata.alert().value();
  if (alert == fuchsia_hardware_ti_metadata::Alert::kBusUnderVoltage) {
    if (!metadata.bus_voltage_limit_microvolt().has_value()) {
      fdf::error("Metadata missing `bus_voltage_limit_microvolt` field");
      return zx::error(ZX_ERR_INTERNAL);
    }
    const uint64_t alert_limit_reg_value =
        metadata.bus_voltage_limit_microvolt().value() / kMicrovoltsPerBit;
    if (alert_limit_reg_value > UINT16_MAX) {
      fdf::error("Bus voltage limit is out of range");
      return zx::error(ZX_ERR_OUT_OF_RANGE);
    }

    if (zx::result result =
            Write16(Register::kAlertLimitReg, static_cast<uint16_t>(alert_limit_reg_value));
        result.is_error()) {
      return result.take_error();
    }
  }

  if (zx::result result = Write16(Register::kMaskEnableReg, static_cast<uint16_t>(alert));
      result.is_error()) {
    return result.take_error();
  }

  zx::result<uint16_t> config_status = Read16(Register::kConfigurationReg);
  if (config_status.is_error()) {
    return config_status.take_error();
  }

  const int metadata_value =
      static_cast<uint16_t>(metadata.mode().value()) |
      (static_cast<uint16_t>(metadata.shunt_voltage_conversion_time().value()) << 3) |
      (static_cast<uint16_t>(metadata.bus_voltage_conversion_time().value()) << 6) |
      (static_cast<uint16_t>(metadata.averages().value()) << 9);
  const int configuration_reg_value =
      (config_status.value() & kConfigurationRegMask) | metadata_value;
  if (zx::result result =
          Write16(Register::kConfigurationReg, static_cast<uint16_t>(configuration_reg_value));
      result.is_error()) {
    return result.take_error();
  }

  fuchsia_hardware_power_sensor::Service::InstanceHandler handler({
      .device = bindings_.CreateHandler(this, dispatcher(), fidl::kIgnoreBindingClosure),
  });
  zx::result result =
      outgoing()->AddService<fuchsia_hardware_power_sensor::Service>(std::move(handler));
  if (result.is_error()) {
    fdf::error("Failed to add power-sensor service: {}", result);
    return result.take_error();
  }

  zx::result connector = devfs_connector_.Bind(dispatcher());
  if (connector.is_error()) {
    fdf::error("Failed to bind devfs connector: {}", connector);
    return connector.take_error();
  }

  fuchsia_driver_framework::DevfsAddArgs devfs_args({
      .connector = std::move(connector.value()),
      .class_name = "power-sensor",
      .connector_supports = fuchsia_device_fs::ConnectionType::kDevice,
  });

  const std::vector<fuchsia_driver_framework::Offer> offers = {
      fdf::MakeOffer2<fuchsia_hardware_power_sensor::Service>(component::kDefaultInstance)};

  const std::vector<fuchsia_driver_framework::NodeProperty2> properties = {
      fdf::MakeProperty2(bind_fuchsia::ID, metadata.power_sensor_domain().value()),
  };

  zx::result child = AddChild(kChildNodeName, devfs_args, properties, offers);
  if (child.is_error()) {
    fdf::error("Failed to add child: {}", child);
    return child.take_error();
  }
  child_ = std::move(child.value());

  return zx::ok();
}

zx::result<uint16_t> TiIna231::Read16(Register reg) {
  const uint8_t address = static_cast<uint8_t>(reg);
  const std::array<uint8_t, 1> write_data = {address};
  std::array<uint8_t, 2> read_data;
  zx::result result = i2c_.WriteReadSync(write_data, read_data);
  if (result.is_error()) {
    fdf::error("I2C read failed: {}", result);
    return result.take_error();
  }
  const int value =
      (static_cast<uint16_t>(read_data[1]) << 8) | static_cast<uint16_t>(read_data[0]);
  return zx::ok(betoh16(value));
}

zx::result<> TiIna231::Write16(Register reg, uint16_t value) {
  const std::array<uint8_t, 3> write_data = {static_cast<uint8_t>(reg),
                                             static_cast<uint8_t>(value >> 8),
                                             static_cast<uint8_t>(value & 0xff)};
  zx::result result = i2c_.WriteSync(write_data);
  if (result.is_error()) {
    fdf::error("I2C write failed: {}", result);
    return result.take_error();
  }
  return zx::ok();
}

void TiIna231::DevfsConnect(fidl::ServerEnd<fuchsia_hardware_power_sensor::Device> server) {
  bindings_.AddBinding(dispatcher(), std::move(server), this, fidl::kIgnoreBindingClosure);
}

}  // namespace power_sensor

FUCHSIA_DRIVER_EXPORT2(power_sensor::TiIna231);

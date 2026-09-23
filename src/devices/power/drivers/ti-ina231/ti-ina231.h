// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_POWER_DRIVERS_TI_INA231_TI_INA231_H_
#define SRC_DEVICES_POWER_DRIVERS_TI_INA231_TI_INA231_H_

#include <fidl/fuchsia.hardware.power.sensor/cpp/wire.h>
#include <lib/driver/compat/cpp/compat.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/devfs/cpp/connector.h>
#include <lib/zx/result.h>

#include <string>

#include "src/devices/i2c/lib/i2c-channel/i2c-channel.h"

namespace power_sensor {

class TiIna231 : public fdf::DriverBase2,
                 public fidl::WireServer<fuchsia_hardware_power_sensor::Device> {
 public:
  static constexpr std::string_view kDriverName = "ti_ina231";
  static constexpr std::string_view kChildNodeName = "ti-ina231";

  explicit TiIna231() : fdf::DriverBase2(kDriverName) {}

  // fdf::DriverBase2 implementation.
  zx::result<> Start(fdf::DriverContext context) override;

  // fidl::WireServer<fuchsia_hardware_power_sensor::Device> implementation.
  void GetPowerWatts(GetPowerWattsCompleter::Sync& completer) override;
  void GetVoltageVolts(GetVoltageVoltsCompleter::Sync& completer) override;
  void GetSensorName(GetSensorNameCompleter::Sync& completer) override;

 protected:
 private:
  enum class Register : uint8_t;

  void DevfsConnect(fidl::ServerEnd<fuchsia_hardware_power_sensor::Device> server);

  zx::result<uint16_t> Read16(Register reg);
  zx::result<> Write16(Register reg, uint16_t value);

  uint64_t shunt_resistor_uohms_;
  i2c::I2cChannel i2c_;
  std::string name_;

  driver_devfs::Connector<fuchsia_hardware_power_sensor::Device> devfs_connector_{
      fit::bind_member<&TiIna231::DevfsConnect>(this)};
  compat::SyncInitializedDeviceServer compat_server_;
  fidl::ServerBindingGroup<fuchsia_hardware_power_sensor::Device> bindings_;
  fidl::ClientEnd<fuchsia_driver_framework::NodeController> child_;
};

}  // namespace power_sensor

#endif  // SRC_DEVICES_POWER_DRIVERS_TI_INA231_TI_INA231_H_

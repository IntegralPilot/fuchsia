// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_TEMPERATURE_DRIVERS_SHTV3_SHTV3_H_
#define SRC_DEVICES_TEMPERATURE_DRIVERS_SHTV3_SHTV3_H_

#include <fidl/fuchsia.hardware.temperature/cpp/wire.h>
#include <lib/ddk/device.h>
#include <lib/zx/result.h>

#include <string>

#include <ddktl/device.h>
#include <ddktl/protocol/empty-protocol.h>

#include "src/devices/i2c/lib/i2c-channel-legacy/i2c-channel.h"

namespace temperature {

namespace temperature_fidl = fuchsia_hardware_temperature;
class Shtv3Device;
using DeviceType = ddk::Device<Shtv3Device, ddk::Messageable<temperature_fidl::Device>::Mixin>;

class Shtv3Device : public DeviceType, public ddk::EmptyProtocol<ZX_PROTOCOL_TEMPERATURE> {
 public:
  Shtv3Device(zx_device_t* parent, ddk::I2cChannel i2c, std::string name)
      : DeviceType(parent), i2c_(std::move(i2c)), name_(std::move(name)) {}

  static zx_status_t Create(void* ctx, zx_device_t* parent);

  void DdkRelease();

  void GetTemperatureCelsius(GetTemperatureCelsiusCompleter::Sync& completer) override;
  void GetSensorName(GetSensorNameCompleter::Sync& completer) override;

  // Visible for testing.
  zx_status_t Init();

 private:
  zx::result<float> ReadTemperature();
  zx::result<uint16_t> Read16();
  zx_status_t Write16(uint16_t value);

  ddk::I2cChannel i2c_;
  const std::string name_;
};

}  // namespace temperature

#endif  // SRC_DEVICES_TEMPERATURE_DRIVERS_SHTV3_SHTV3_H_

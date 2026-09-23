// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "temperature_sensor.h"

#include <fidl/fuchsia.hardware.thermal/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/fdio/directory.h>
#include <lib/zx/result.h>
#include <stdio.h>
#include <unistd.h>
#include <zircon/assert.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>

#include <string>
#include <string_view>
#include <utility>

#include <fbl/unique_fd.h>

#include "device.h"
#include "src/lib/fxl/strings/string_printf.h"
#include "status.h"
#include "util.h"

namespace hwstress {

class SystemTemperatureSensor : public TemperatureSensor {
 public:
  explicit SystemTemperatureSensor(fidl::ClientEnd<fuchsia_hardware_thermal::Device> client)
      : client_(std::move(client)) {}

  // |TemperatureSensor| implementation.
  std::optional<double> ReadCelcius() override {
    fidl::WireResult<fuchsia_hardware_thermal::Device::GetTemperatureCelsius> temperature =
        client_->GetTemperatureCelsius();
    if (!temperature.ok() || temperature.value().status != ZX_OK) {
      return std::nullopt;
    }
    return temperature.value().temp;
  }

 private:
  fidl::WireSyncClient<fuchsia_hardware_thermal::Device> client_;
};

std::unique_ptr<TemperatureSensor> CreateSystemTemperatureSensor(std::string_view device_path) {
  zx::result<zx::channel> channel = OpenDeviceChannel(device_path);
  if (channel.is_error()) {
    fprintf(stderr, "Could not open device: %s\n", channel.status_string());
    return nullptr;
  }
  return CreateSystemTemperatureSensor(std::move(channel.value()));
}

std::unique_ptr<TemperatureSensor> CreateSystemTemperatureSensor(zx::channel channel) {
  fidl::ClientEnd<fuchsia_hardware_thermal::Device> client(std::move(channel));
  return std::make_unique<SystemTemperatureSensor>(std::move(client));
}

class NullTemperatureSensor : public TemperatureSensor {
  std::optional<double> ReadCelcius() override { return std::nullopt; }
};

std::unique_ptr<TemperatureSensor> CreateNullTemperatureSensor() {
  return std::make_unique<NullTemperatureSensor>();
}

TemperatureSensor* GetNullTemperatureSensor() {
  static NullTemperatureSensor sensor;
  return &sensor;
}

std::string TemperatureToString(std::optional<double> temperature) {
  return temperature.has_value() ? fxl::StringPrintf("%0.1f°C", temperature.value()) : "unknown";
}

}  // namespace hwstress

// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_POWER_POWER_MANAGER_TESTING_FAKE_DRIVER_TEMPERATURE_SERVER_H_
#define SRC_POWER_POWER_MANAGER_TESTING_FAKE_DRIVER_TEMPERATURE_SERVER_H_

#include <fidl/fuchsia.hardware.temperature/cpp/wire.h>

namespace fake_driver {

// Protocol served to client components over devfs.
class TemperatureDeviceProtocolServer
    : public fidl::WireServer<fuchsia_hardware_temperature::Device> {
 public:
  explicit TemperatureDeviceProtocolServer(std::string sensor_name, float* temperature);

  void GetTemperatureCelsius(GetTemperatureCelsiusCompleter::Sync& completer) override;

  void GetSensorName(GetSensorNameCompleter::Sync& completer) override;

  void Serve(async_dispatcher_t* dispatcher,
             fidl::ServerEnd<fuchsia_hardware_temperature::Device> server);

 private:
  fidl::ServerBindingGroup<fuchsia_hardware_temperature::Device> bindings_;

  float* temperature_;
  std::string sensor_name_;
};

}  // namespace fake_driver

#endif  // SRC_POWER_POWER_MANAGER_TESTING_FAKE_DRIVER_TEMPERATURE_SERVER_H_

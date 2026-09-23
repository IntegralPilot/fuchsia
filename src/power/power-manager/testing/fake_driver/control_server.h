// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_POWER_POWER_MANAGER_TESTING_FAKE_DRIVER_CONTROL_SERVER_H_
#define SRC_POWER_POWER_MANAGER_TESTING_FAKE_DRIVER_CONTROL_SERVER_H_

#include <fidl/fuchsia.powermanager.driver.temperaturecontrol/cpp/wire.h>

namespace fake_driver {

// Protocol served to client components over devfs.
class ControlDeviceProtocolServer
    : public fidl::WireServer<fuchsia_powermanager_driver_temperaturecontrol::Device> {
 public:
  explicit ControlDeviceProtocolServer(float* temperature);

  void SetTemperatureCelsius(SetTemperatureCelsiusRequestView request,
                             SetTemperatureCelsiusCompleter::Sync& completer) override;

  void Serve(async_dispatcher_t* dispatcher,
             fidl::ServerEnd<fuchsia_powermanager_driver_temperaturecontrol::Device> server);

 private:
  fidl::ServerBindingGroup<fuchsia_powermanager_driver_temperaturecontrol::Device> bindings_;

  float* temperature_;
};

}  // namespace fake_driver

#endif  // SRC_POWER_POWER_MANAGER_TESTING_FAKE_DRIVER_CONTROL_SERVER_H_

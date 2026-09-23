// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "control_server.h"

namespace fake_driver {

ControlDeviceProtocolServer::ControlDeviceProtocolServer(float* temperature)
    : temperature_(temperature) {}

void ControlDeviceProtocolServer::SetTemperatureCelsius(
    SetTemperatureCelsiusRequestView request, SetTemperatureCelsiusCompleter::Sync& completer) {
  *temperature_ = request->temperature;
  completer.Reply(ZX_OK);
}

void ControlDeviceProtocolServer::Serve(
    async_dispatcher_t* dispatcher,
    fidl::ServerEnd<fuchsia_powermanager_driver_temperaturecontrol::Device> server) {
  bindings_.AddBinding(dispatcher, std::move(server), this, fidl::kIgnoreBindingClosure);
}

}  // namespace fake_driver

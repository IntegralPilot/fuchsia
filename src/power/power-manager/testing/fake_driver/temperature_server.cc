// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "temperature_server.h"

namespace fake_driver {
TemperatureDeviceProtocolServer::TemperatureDeviceProtocolServer(std::string sensor_name,
                                                                 float* temperature)
    : temperature_(temperature), sensor_name_(sensor_name) {}

void TemperatureDeviceProtocolServer::GetTemperatureCelsius(
    GetTemperatureCelsiusCompleter::Sync& completer) {
  completer.Reply(ZX_OK, *temperature_);
}

void TemperatureDeviceProtocolServer::GetSensorName(GetSensorNameCompleter::Sync& completer) {
  completer.Reply(fidl::StringView::FromExternal(sensor_name_));
}

void TemperatureDeviceProtocolServer::Serve(
    async_dispatcher_t* dispatcher, fidl::ServerEnd<fuchsia_hardware_temperature::Device> server) {
  bindings_.AddBinding(dispatcher, std::move(server), this, fidl::kIgnoreBindingClosure);
}

}  // namespace fake_driver

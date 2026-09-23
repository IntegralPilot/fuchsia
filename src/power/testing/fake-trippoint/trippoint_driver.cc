// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "trippoint_driver.h"

#include <fidl/fuchsia.hardware.trippoint/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>

namespace fake_trippoint {

zx::result<> TrippointDriver::Start(fdf::DriverContext context) {
  fdf::info("Starting fake trippoint driver");
  fuchsia_hardware_temperature::Service::InstanceHandler temperature_handler({
      .device =
          temperature_bindings_.CreateHandler(this, dispatcher(), fidl::kIgnoreBindingClosure),
      .trippoint =
          trippoint_bindings_.CreateHandler(this, dispatcher(), fidl::kIgnoreBindingClosure),
  });
  zx::result<> temperature_result =
      outgoing()->AddService<fuchsia_hardware_temperature::Service>(std::move(temperature_handler));
  if (temperature_result.is_error()) {
    fdf::error("Failed to add service: %s", temperature_result.status_string());
    return temperature_result.take_error();
  }

  test_trippoint::Service::InstanceHandler control_handler({
      .control = control_bindings_.CreateHandler(this, dispatcher(), fidl::kIgnoreBindingClosure),
  });
  zx::result<> control_result =
      outgoing()->AddService<test_trippoint::Service>(std::move(control_handler));
  if (control_result.is_error()) {
    fdf::error("Failed to add service: %s", control_result.status_string());
    return control_result.take_error();
  }

  zx::result child = AddOwnedChild("fake-trippoint-dev");
  if (child.is_error()) {
    fdf::error("Failed to add owned child: %s", child.status_string());
    return child.take_error();
  }
  child_ = std::move(child.value());

  return zx::ok();
}

void TrippointDriver::SetTemperatureCelsius(SetTemperatureCelsiusRequestView request,
                                            SetTemperatureCelsiusCompleter::Sync& completer) {
  temp_celsius_ = request->temp;
  status_ = request->status;
  completer.Reply();
}

void TrippointDriver::GetTemperatureCelsius(GetTemperatureCelsiusCompleter::Sync& completer) {
  completer.Reply(status_, temp_celsius_);
}

void TrippointDriver::GetSensorName(GetSensorNameCompleter::Sync& completer) {
  completer.Reply(fidl::StringView::FromExternal(std::string(name()).c_str()));
}

void TrippointDriver::GetTripPointDescriptors(GetTripPointDescriptorsCompleter::Sync& completer) {
  // TODO(https://fxbug.dev/423948740): Implement this when clients require it.
  completer.Reply(fit::error(ZX_ERR_UNAVAILABLE));
}

void TrippointDriver::SetTripPoints(SetTripPointsRequestView request,
                                    SetTripPointsCompleter::Sync& completer) {
  // TODO(https://fxbug.dev/423948740): Implement this when clients require it.
  completer.Reply(fit::error(ZX_ERR_UNAVAILABLE));
}

void TrippointDriver::WaitForAnyTripPoint(WaitForAnyTripPointCompleter::Sync& completer) {
  // TODO(https://fxbug.dev/423948740): Implement this when clients require it.
  completer.Reply(fit::error(ZX_ERR_UNAVAILABLE));
}

void TrippointDriver::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_trippoint::TripPoint> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::error(
      "Unknown method in fuchsia.hardware.trippoint TripPoint protocol, closing with ZX_ERR_NOT_SUPPORTED");
  completer.Close(ZX_ERR_NOT_SUPPORTED);
}

}  // namespace fake_trippoint

FUCHSIA_DRIVER_EXPORT2(fake_trippoint::TrippointDriver);

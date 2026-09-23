// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sensors/playback/driver_impl.h"

#include <lib/fpromise/bridge.h>

namespace sensors::playback {
namespace {
using fpromise::bridge;
using fpromise::promise;
using fpromise::result;

using fuchsia_hardware_sensors::ActivateSensorError;
using fuchsia_hardware_sensors::ConfigureSensorRateError;
using fuchsia_hardware_sensors::DeactivateSensorError;
using fuchsia_hardware_sensors::Driver;
using fuchsia_sensors_types::SensorEvent;
using fuchsia_sensors_types::SensorInfo;
}  // namespace

bool DriverImpl::client_connected_ = false;

DriverImpl::DriverImpl(async_dispatcher_t* dispatcher, PlaybackController& controller)
    : ActorBase(dispatcher, scope_), controller_(controller) {}

void DriverImpl::GetSensorsList(GetSensorsListCompleter::Sync& completer) {
  ZX_ASSERT(binding_ref_.has_value());

  Schedule(controller_.GetSensorsList().and_then(
      [completer = completer.ToAsync()](const std::vector<SensorInfo>& sensor_list) mutable {
        completer.Reply(sensor_list);
      }));
}

void DriverImpl::ActivateSensor(ActivateSensorRequest& request,
                                ActivateSensorCompleter::Sync& completer) {
  ZX_ASSERT(binding_ref_.has_value());

  Schedule(controller_.ActivateSensor(request.sensor_id())
               .then([completer = completer.ToAsync()](
                         result<void, ActivateSensorError>& result) mutable -> promise<void> {
                 if (result.is_ok()) {
                   completer.Reply(fit::success());
                 } else {
                   completer.Reply(fit::error(result.error()));
                 }
                 return fpromise::make_ok_promise();
               }));
}

void DriverImpl::DeactivateSensor(DeactivateSensorRequest& request,
                                  DeactivateSensorCompleter::Sync& completer) {
  ZX_ASSERT(binding_ref_.has_value());

  Schedule(controller_.DeactivateSensor(request.sensor_id())
               .then([completer = completer.ToAsync()](
                         result<void, DeactivateSensorError>& result) mutable -> promise<void> {
                 if (result.is_ok()) {
                   completer.Reply(fit::success());
                 } else {
                   completer.Reply(fit::error(result.error()));
                 }
                 return fpromise::make_ok_promise();
               }));
}

void DriverImpl::ConfigureSensorRate(ConfigureSensorRateRequest& request,
                                     ConfigureSensorRateCompleter::Sync& completer) {
  ZX_ASSERT(binding_ref_.has_value());

  Schedule(controller_.ConfigureSensorRate(request.sensor_id(), request.sensor_rate_config())
               .then([completer = completer.ToAsync()](
                         result<void, ConfigureSensorRateError>& result) mutable -> promise<void> {
                 if (result.is_ok()) {
                   completer.Reply(fit::success());
                 } else {
                   completer.Reply(fit::error(result.error()));
                 }
                 return fpromise::make_ok_promise();
               }));
}

void DriverImpl::handle_unknown_method(fidl::UnknownMethodMetadata<Driver> metadata,
                                       fidl::UnknownMethodCompleter::Sync& completer) {
  ZX_ASSERT(binding_ref_.has_value());
  completer.Close(ZX_ERR_NOT_SUPPORTED);
}

void DriverImpl::OnUnbound(fidl::UnbindInfo info, fidl::ServerEnd<Driver> server_end,
                           fit::callback<void()> unbind_callback) {
  // |is_user_initiated| returns true if the server code called |Close| on a
  // completer, or |Unbind| / |Close| on the |binding_ref_|, to proactively
  // teardown the connection. These cases are usually part of normal server
  // shutdown, so logging is unnecessary.
  if (info.is_user_initiated()) {
    unbind_callback();
    return;
  }
  if (info.is_peer_closed()) {
    // If the peer (the client) closed their endpoint, log that as INFO.
    FX_LOGS(INFO) << "Client disconnected";
  } else {
    // Treat other unbind causes as errors.
    FX_LOGS(ERROR) << "Server error: " << info;
  }
  controller_.DriverClientDisconnected(std::move(unbind_callback));
}

promise<void> DriverImpl::DisconnectClient(zx_status_t epitaph) {
  bridge<void> bridge;
  Schedule([this, epitaph, completer = std::move(bridge.completer)]() mutable {
    if (binding_ref_.has_value()) {
      FX_LOGS(INFO) << "Disconnecting driver protocol client.";
      binding_ref_->Close(epitaph);
      binding_ref_ = std::nullopt;
    }
    completer.complete_ok();
  });
  return bridge.consumer.promise();
}

void DriverImpl::AttachToController() {
  controller_.SetDisconnectDriverClientCallback(
      [this](zx_status_t epitaph) { return DisconnectClient(epitaph); });

  controller_.SetEventCallback([this](const SensorEvent& event) {
    Schedule([this, event]() {
      fit::result result = fidl::SendEvent(*binding_ref_)->OnSensorEvent(event);
      if (!result.is_ok()) {
        FX_LOGS(ERROR) << "Error sending event: " << result.error_value();
      }
    });
  });
}

}  // namespace sensors::playback

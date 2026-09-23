// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/services/device_registry/observer_server.h"

#include <fidl/fuchsia.audio.device/cpp/fidl.h>
#include <fidl/fuchsia.hardware.audio/cpp/fidl.h>
#include <lib/fit/internal/result.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/clock.h>
#include <zircon/errors.h>

#include "src/media/audio/services/device_registry/audio_device_registry.h"
#include "src/media/audio/services/device_registry/device.h"
#include "src/media/audio/services/device_registry/inspector.h"
#include "src/media/audio/services/device_registry/logging.h"

namespace media_audio {

namespace fad = fuchsia_audio_device;

// static
std::shared_ptr<ObserverServer> ObserverServer::Create(std::shared_ptr<const FidlThread> thread,
                                                       fidl::ServerEnd<fad::Observer> server_end,
                                                       std::shared_ptr<const Device> device) {
  ADR_LOG_STATIC(kLogObserverServerMethods);

  return BaseFidlServer::Create(std::move(thread), std::move(server_end), std::move(device));
}

ObserverServer::ObserverServer(std::shared_ptr<const Device> device) : device_(std::move(device)) {
  ADR_LOG_METHOD(kLogObjectLifetimes);
  SetInspect(Inspector::Singleton()->RecordObserverInstance(zx::clock::get_monotonic()));

  // TODO(https://fxbug.dev/42068381): Consider Health-check if this can change post-initialization.

  ++count_;
  LogObjectCounts();
}

ObserverServer::~ObserverServer() {
  ADR_LOG_METHOD(kLogObjectLifetimes);
  inspect()->RecordDestructionTime(zx::clock::get_monotonic());

  --count_;
  LogObjectCounts();
}

void ObserverServer::DeviceHasError() {
  ADR_LOG_METHOD(kLogObserverServerMethods || kLogNotifyMethods);

  device_has_error_ = true;
  DeviceIsRemoved();
}

// Called when the Device shuts down first.
void ObserverServer::DeviceIsRemoved() {
  ADR_LOG_METHOD(kLogObserverServerMethods || kLogNotifyMethods);

  Shutdown(ZX_ERR_PEER_CLOSED);

  // We don't explicitly clear our shared_ptr<Device> reference, to ensure we destruct first.
}

void ObserverServer::WatchPlugState(WatchPlugStateCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogObserverServerMethods);

  if (device_has_error_) {
    ADR_WARN_METHOD() << "Device encountered an error and will be removed";
    completer.Reply(fit::error<fad::ObserverWatchPlugStateError>(
        fad::ObserverWatchPlugStateError::kDeviceError));
    return;
  }

  FX_CHECK(device_);
  if (!device_->is_codec()) {
    ADR_WARN_METHOD() << "This method is not supported for this device type";
    completer.Reply(fit::error<fad::ObserverWatchPlugStateError>(
        fad::ObserverWatchPlugStateError::kWrongDeviceType));
    return;
  }

  if (watch_plug_state_completer_.has_value()) {
    ADR_WARN_METHOD() << "previous `WatchPlugState` request has not yet completed";
    completer.Reply(fit::error<fad::ObserverWatchPlugStateError>(
        fad::ObserverWatchPlugStateError::kAlreadyPending));
    return;
  }

  watch_plug_state_completer_ = completer.ToAsync();
  MaybeCompleteWatchPlugState();
}

void ObserverServer::PlugStateIsChanged(const fad::PlugState& new_plug_state,
                                        zx::time plug_change_time) {
  ADR_LOG_METHOD(kLogObserverServerMethods || kLogNotifyMethods)
      << new_plug_state << " @ " << plug_change_time.get();

  // plug_state_time can't be any later than "right now"
  auto plug_time = std::min(plug_change_time, zx::clock::get_monotonic());

  new_plug_state_to_notify_ = fad::ObserverWatchPlugStateResponse{{
      .state = new_plug_state,
      .plug_time = plug_time.get(),
  }};
  MaybeCompleteWatchPlugState();
}

void ObserverServer::MaybeCompleteWatchPlugState() {
  if (watch_plug_state_completer_.has_value() && new_plug_state_to_notify_.has_value()) {
    auto completer = std::move(*watch_plug_state_completer_);
    watch_plug_state_completer_.reset();

    auto new_plug_state = std::move(*new_plug_state_to_notify_);
    new_plug_state_to_notify_.reset();

    completer.Reply(fit::success(new_plug_state));
  }
}

void ObserverServer::GetReferenceClock(GetReferenceClockCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogObserverServerMethods);

  if (device_has_error_) {
    ADR_WARN_METHOD() << "Device encountered an error and will be removed";
    completer.Reply(fit::error<fad::ObserverGetReferenceClockError>(
        fad::ObserverGetReferenceClockError::kDeviceError));
    return;
  }

  FX_CHECK(device_);
  if (!device_->is_composite()) {
    ADR_WARN_METHOD() << "This method is not supported for this device type";
    completer.Reply(fit::error<fad::ObserverGetReferenceClockError>(
        fad::ObserverGetReferenceClockError::kWrongDeviceType));
    return;
  }

  auto clock_result = device_->GetReadOnlyClock();
  if (clock_result.is_error()) {
    ADR_WARN_METHOD() << "Device clock could not be created";
    completer.Reply(fit::error<fad::ObserverGetReferenceClockError>(
        fad::ObserverGetReferenceClockError::kDeviceClockUnavailable));
    return;
  }
  fad::ObserverGetReferenceClockResponse response = {{
      .reference_clock = std::move(clock_result.value()),
  }};
  completer.Reply(fit::success(std::move(response)));
}

void ObserverServer::GetElements(GetElementsCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogObserverServerMethods);

  if (device_has_error_) {
    ADR_WARN_METHOD() << "Device has error";
    completer.Reply(fit::error(ZX_ERR_INTERNAL));
    return;
  }

  FX_CHECK(device_);
  if (!device_->is_codec() && !device_->is_composite()) {
    ADR_WARN_METHOD() << "This device_type does not support " << __func__;
    completer.Reply(fit::error(ZX_ERR_WRONG_TYPE));
    return;
  }

  if (!device_->supports_signalprocessing()) {
    ADR_LOG_METHOD(kLogObserverServerMethods) << "This driver does not support signalprocessing";
    completer.Reply(fit::error(ZX_ERR_NOT_SUPPORTED));
    return;
  }

  FX_CHECK(device_->info().has_value() &&
           device_->info()->signal_processing_elements().has_value() &&
           !device_->info()->signal_processing_elements()->empty());
  completer.Reply(fit::success(*device_->info()->signal_processing_elements()));
}

void ObserverServer::GetTopologies(GetTopologiesCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogObserverServerMethods);

  if (device_has_error_) {
    ADR_WARN_METHOD() << "Device has error";
    completer.Reply(fit::error(ZX_ERR_INTERNAL));
    return;
  }

  FX_CHECK(device_);
  if (!device_->is_codec() && !device_->is_composite()) {
    ADR_WARN_METHOD() << "This device_type does not support " << __func__;
    completer.Reply(fit::error(ZX_ERR_WRONG_TYPE));
    return;
  }

  if (!device_->supports_signalprocessing()) {
    ADR_LOG_METHOD(kLogObserverServerMethods) << "This driver does not support signalprocessing";
    completer.Reply(fit::error(ZX_ERR_NOT_SUPPORTED));
    return;
  }

  FX_CHECK(device_->info().has_value() &&
           device_->info()->signal_processing_topologies().has_value() &&
           !device_->info()->signal_processing_topologies()->empty());
  completer.Reply(fit::success(*device_->info()->signal_processing_topologies()));
}

void ObserverServer::WatchTopology(WatchTopologyCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogObserverServerMethods);

  if (device_has_error_) {
    ADR_WARN_METHOD() << "Device has error";
    completer.Close(ZX_ERR_INTERNAL);
    return;
  }

  FX_CHECK(device_);
  if (!device_->is_codec() && !device_->is_composite()) {
    ADR_WARN_METHOD() << "This device_type does not support " << __func__;
    completer.Close(ZX_ERR_WRONG_TYPE);
    return;
  }

  if (!device_->supports_signalprocessing()) {
    ADR_WARN_METHOD() << "This driver does not support signalprocessing";
    completer.Close(ZX_ERR_NOT_SUPPORTED);
    return;
  }

  if (watch_topology_completer_.has_value()) {
    ADR_WARN_METHOD() << "previous `WatchTopology` request has not yet completed";
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }

  watch_topology_completer_ = completer.ToAsync();
  MaybeCompleteWatchTopology();
}

void ObserverServer::TopologyIsChanged(TopologyId topology_id) {
  ADR_LOG_METHOD(kLogObserverServerMethods || kLogNotifyMethods)
      << "(topology_id " << topology_id << ")";

  topology_id_to_notify_ = topology_id;
  MaybeCompleteWatchTopology();
}

void ObserverServer::MaybeCompleteWatchTopology() {
  if (watch_topology_completer_.has_value() && topology_id_to_notify_.has_value()) {
    ADR_LOG_METHOD(kLogObserverServerMethods || kLogNotifyMethods) << " will Reply";

    auto completer = std::move(*watch_topology_completer_);
    watch_topology_completer_.reset();

    auto new_topology_id = *topology_id_to_notify_;
    topology_id_to_notify_.reset();

    completer.Reply(new_topology_id);
  } else {
    ADR_LOG_METHOD(kLogObserverServerMethods || kLogNotifyMethods) << " did not occur";
  }
}

void ObserverServer::WatchElementState(WatchElementStateRequest& request,
                                       WatchElementStateCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogObserverServerMethods);

  if (device_has_error_) {
    ADR_WARN_METHOD() << "Device has error";
    completer.Close(ZX_ERR_INTERNAL);
    return;
  }

  FX_CHECK(device_);
  if (!device_->is_codec() && !device_->is_composite()) {
    ADR_WARN_METHOD() << "This device_type does not support " << __func__;
    completer.Close(ZX_ERR_WRONG_TYPE);
    return;
  }

  if (!device_->supports_signalprocessing()) {
    ADR_WARN_METHOD() << "This driver does not support signalprocessing";
    completer.Close(ZX_ERR_NOT_SUPPORTED);
    return;
  }

  ElementId element_id = request.processing_element_id();
  if (!device_->element_ids().contains(element_id)) {
    ADR_WARN_METHOD() << "unknown element_id " << element_id;
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }

  if (watch_element_state_completers_.contains(element_id)) {
    ADR_WARN_METHOD() << "previous `WatchElementState(" << element_id
                      << ")` request has not yet completed";
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }

  watch_element_state_completers_.insert({element_id, completer.ToAsync()});
  MaybeCompleteWatchElementState(element_id);
}

void ObserverServer::ElementStateIsChanged(
    ElementId element_id, fuchsia_hardware_audio_signalprocessing::ElementState element_state) {
  ADR_LOG_METHOD(kLogObserverServerMethods || kLogNotifyMethods)
      << "(element_id " << element_id << ")";

  element_states_to_notify_.insert_or_assign(element_id, element_state);
  MaybeCompleteWatchElementState(element_id);
}

// If we have an outstanding hanging-get and a state-change, respond with the state change.
void ObserverServer::MaybeCompleteWatchElementState(ElementId element_id) {
  if (watch_element_state_completers_.contains(element_id) &&
      element_states_to_notify_.contains(element_id)) {
    ADR_LOG_METHOD(kLogObserverServerMethods || kLogNotifyMethods)
        << "(" << element_id << ") will Reply";
    auto completer = std::move(watch_element_state_completers_.find(element_id)->second);
    watch_element_state_completers_.erase(element_id);

    auto new_element_state = element_states_to_notify_.find(element_id)->second;
    element_states_to_notify_.erase(element_id);

    completer.Reply(new_element_state);
  } else {
    ADR_LOG_METHOD(kLogObserverServerMethods || kLogNotifyMethods)
        << "(" << element_id << ") did not occur";
  }
}

// We complain but don't close the connection, to accommodate older and newer clients.
void ObserverServer::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_audio_device::Observer> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  ADR_WARN_METHOD() << "(Observer) ordinal " << metadata.method_ordinal;
  if (metadata.unknown_method_type == fidl::UnknownMethodType::kTwoWay) {
    // Pend the completer indefinitely.
    unknown_method_completers_.emplace_back(completer.ToAsync());
  }
}

}  // namespace media_audio

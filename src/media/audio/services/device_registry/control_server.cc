// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/services/device_registry/control_server.h"

#include <fidl/fuchsia.audio.device/cpp/fidl.h>
#include <fidl/fuchsia.audio/cpp/fidl.h>
#include <fidl/fuchsia.hardware.audio.signalprocessing/cpp/fidl.h>
#include <fidl/fuchsia.hardware.audio/cpp/fidl.h>
#include <fidl/fuchsia.mem/cpp/natural_types.h>
#include <lib/fidl/cpp/enum.h>
#include <lib/fidl/cpp/wire/status.h>
#include <lib/fidl/cpp/wire/unknown_interaction_handler.h>
#include <lib/fit/internal/result.h>
#include <lib/fit/result.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/clock.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>

#include <optional>
#include <utility>

#include "src/media/audio/services/device_registry/device.h"
#include "src/media/audio/services/device_registry/inspector.h"
#include "src/media/audio/services/device_registry/logging.h"
#include "src/media/audio/services/device_registry/packet_stream_server.h"
#include "src/media/audio/services/device_registry/ring_buffer_server.h"
#include "src/media/audio/services/device_registry/validate.h"

namespace media_audio {

namespace fad = fuchsia_audio_device;
namespace fha = fuchsia_hardware_audio;

// static
std::shared_ptr<ControlServer> ControlServer::Create(std::shared_ptr<const FidlThread> thread,
                                                     fidl::ServerEnd<fad::Control> server_end,
                                                     std::shared_ptr<AudioDeviceRegistry> parent,
                                                     std::shared_ptr<Device> device) {
  ADR_LOG_STATIC(kLogControlServerMethods);

  return BaseFidlServer::Create(std::move(thread), std::move(server_end), std::move(parent),
                                std::move(device));
}

ControlServer::ControlServer(std::shared_ptr<AudioDeviceRegistry> parent,
                             std::shared_ptr<Device> device)
    : parent_(std::move(parent)), device_(std::move(device)) {
  ADR_LOG_METHOD(kLogObjectLifetimes);
  SetInspect(Inspector::Singleton()->RecordControlInstance(zx::clock::get_monotonic()));

  ++count_;
  LogObjectCounts();
}

ControlServer::~ControlServer() {
  ADR_LOG_METHOD(kLogObjectLifetimes);
  inspect()->RecordDestructionTime(zx::clock::get_monotonic());

  --count_;
  LogObjectCounts();
}

// Called when the client shuts down first.
void ControlServer::OnShutdown(fidl::UnbindInfo info) {
  if (!info.is_peer_closed() && !info.is_user_initiated()) {
    ADR_WARN_METHOD() << "shutdown with unexpected status: " << info;
  } else {
    ADR_LOG_METHOD(kLogRingBufferFidlResponses || kLogObjectLifetimes) << "with status: " << info;
  }

  for (auto& [_, weak_ring_buffer_server] : ring_buffer_servers_) {
    if (auto sh_ring_buffer_server = weak_ring_buffer_server.lock(); sh_ring_buffer_server) {
      sh_ring_buffer_server->ClientDroppedControl();
    }
  }
  ring_buffer_servers_.clear();

  for (auto& [_, weak_packet_stream_server] : packet_stream_servers_) {
    if (auto sh_packet_stream_server = weak_packet_stream_server.lock(); sh_packet_stream_server) {
      sh_packet_stream_server->ClientDroppedControl();
    }
  }
  packet_stream_servers_.clear();
}

// Called when Device drops its RingBuffer FIDL. Tell RingBufferServer and drop our reference.
void ControlServer::DeviceDroppedRingBuffer(ElementId element_id) {
  ADR_LOG_METHOD(kLogControlServerMethods || kLogNotifyMethods);

  auto ring_buffer_server_iter = ring_buffer_servers_.find(element_id);
  if (ring_buffer_server_iter != ring_buffer_servers_.end()) {
    if (auto sh_ring_buffer_server = ring_buffer_server_iter->second.lock();
        sh_ring_buffer_server) {
      sh_ring_buffer_server->DeviceDroppedRingBuffer();
    }
    ring_buffer_servers_.erase(element_id);
  }
}

void ControlServer::DeviceDroppedPacketStream(ElementId element_id) {
  ADR_LOG_METHOD(kLogControlServerMethods || kLogNotifyMethods);

  auto packet_stream_server_iter = packet_stream_servers_.find(element_id);
  if (packet_stream_server_iter != packet_stream_servers_.end()) {
    if (auto sh_packet_stream_server = packet_stream_server_iter->second.lock();
        sh_packet_stream_server) {
      sh_packet_stream_server->DeviceDroppedPacketStream();
    }
    packet_stream_servers_.erase(element_id);
  }
}

void ControlServer::DeviceHasError() {
  ADR_LOG_METHOD(kLogControlServerMethods);

  // Use device_has_error_ as a re-entrancy guard to ensure DeviceIsRemoved() is called only once.
  if (device_has_error_) {
    return;
  }
  device_has_error_ = true;
  DeviceIsRemoved();
}

// Upon exiting this method, we drop our connection to the client.
void ControlServer::DeviceIsRemoved() {
  ADR_LOG_METHOD(kLogControlServerMethods);

  for (auto& [_, weak_ring_buffer_server] : ring_buffer_servers_) {
    if (auto sh_ring_buffer_server = weak_ring_buffer_server.lock(); sh_ring_buffer_server) {
      sh_ring_buffer_server->ClientDroppedControl();
    }
  }
  ring_buffer_servers_.clear();

  for (auto& [_, weak_packet_stream_server] : packet_stream_servers_) {
    if (auto sh_packet_stream_server = weak_packet_stream_server.lock(); sh_packet_stream_server) {
      sh_packet_stream_server->ClientDroppedControl();
    }
  }
  packet_stream_servers_.clear();

  // We don't explicitly clear our shared_ptr<Device> reference, to ensure we destruct first.

  Shutdown(ZX_ERR_PEER_CLOSED);
}

std::shared_ptr<RingBufferServer> ControlServer::TryGetRingBufferServer(ElementId element_id) {
  ADR_LOG_METHOD(kLogControlServerMethods);
  auto ring_buffer_server_iter = ring_buffer_servers_.find(element_id);
  if (ring_buffer_server_iter != ring_buffer_servers_.end()) {
    if (auto sh_ring_buffer_server = ring_buffer_server_iter->second.lock();
        sh_ring_buffer_server) {
      return sh_ring_buffer_server;
    }
    ring_buffer_servers_.erase(element_id);
  }
  return nullptr;
}

// fuchsia.audio.device.Control implementation
void ControlServer::CreateRingBuffer(CreateRingBufferRequest& request,
                                     CreateRingBufferCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogControlServerMethods);

  // Fail if device has error.
  if (device_has_error_) {
    ADR_WARN_METHOD() << "device has an error";
    completer.Reply(fit::error(fad::ControlCreateRingBufferError::kDeviceError));
    return;
  }

  if (!device_->is_composite()) {
    ADR_WARN_METHOD() << "Unsupported method for device_type " << device_->device_type();
    completer.Reply(fit::error(fad::ControlCreateRingBufferError::kWrongDeviceType));
    return;
  }
  // Fail on missing parameters.
  if (!request.element_id()) {
    ADR_WARN_METHOD() << "required field 'element_id' is missing";
    completer.Reply(fit::error(fad::ControlCreateRingBufferError::kInvalidElementId));
    return;
  }
  ElementId element_id = *request.element_id();
  auto& rb_ids = device_->ring_buffer_ids();
  if (!rb_ids.contains(element_id)) {
    ADR_WARN_METHOD() << "required field 'element_id' (" << element_id
                      << ") does not refer to an element of type RING_BUFFER";
    completer.Reply(fit::error(fad::ControlCreateRingBufferError::kInvalidElementId));
    return;
  }

  if (create_ring_buffer_completers_.contains(element_id)) {
    ADR_WARN_METHOD() << "(element_id " << element_id
                      << ") previous `CreateRingBuffer` request has not completed";
    completer.Reply(fit::error(fad::ControlCreateRingBufferError::kAlreadyPending));
    return;
  }

  if (!request.options().has_value()) {
    ADR_WARN_METHOD() << "(element_id " << element_id << ") required field 'options' is missing";
    completer.Reply(fit::error(fad::ControlCreateRingBufferError::kInvalidOptions));
    return;
  }
  if (!request.options()->format().has_value() ||
      !request.options()->format()->sample_type().has_value() ||
      !request.options()->format()->channel_count().has_value() ||
      !request.options()->format()->frames_per_second().has_value()) {
    ADR_WARN_METHOD() << "(element_id " << element_id
                      << ") required 'options.format' (or one of its required members) is missing";
    completer.Reply(fit::error(fad::ControlCreateRingBufferError::kInvalidFormat));
    return;
  }
  if (!request.options()->ring_buffer_min_bytes().has_value()) {
    ADR_WARN_METHOD() << "(element_id " << element_id
                      << ") required field 'options.ring_buffer_min_bytes' is missing";
    completer.Reply(fit::error(fad::ControlCreateRingBufferError::kInvalidMinBytes));
    return;
  }
  if (!request.ring_buffer_server().has_value()) {
    ADR_WARN_METHOD() << "(element_id " << element_id
                      << ") required field 'ring_buffer_server' is missing";
    completer.Reply(fit::error(fad::ControlCreateRingBufferError::kInvalidRingBuffer));
    return;
  }
  if (TryGetRingBufferServer(element_id)) {
    ADR_WARN_METHOD() << "(element_id " << element_id << ") device RingBuffer already exists";
    completer.Reply(fit::error(fad::wire::ControlCreateRingBufferError::kAlreadyAllocated));
    return;
  }

  auto driver_format = device_->SupportedRingBufferDriverFormatForClientFormat(
      element_id, *request.options()->format());
  // Fail if device cannot satisfy the requested format.
  if (!driver_format.has_value()) {
    ADR_WARN_METHOD() << "(element_id " << element_id
                      << ") device does not support the specified options";
    completer.Reply(fit::error(fad::ControlCreateRingBufferError::kFormatMismatch));
    return;
  }

  auto ring_buffer_server = parent_->CreateRingBufferServer(
      std::move(*request.ring_buffer_server()), shared_from_this(), device_, element_id);
  AddChildServer(ring_buffer_server);
  ring_buffer_servers_.insert_or_assign(element_id, ring_buffer_server);

  // The completer is captured so we can respond asynchronously.
  create_ring_buffer_completers_.insert_or_assign(element_id, completer.ToAsync());

  // `Device::CreateRingBuffer` returns false if it fails synchronously. In that case, it has
  // already invoked the callback, which will have replied to the client. Note that
  // `Device::CreateRingBuffer` always invokes the callback irrespective of the return value.
  bool created = device_->CreateRingBuffer(
      element_id, *driver_format, *request.options()->ring_buffer_min_bytes(),
      // Capture a weak_ptr, in case the ControlServer is destroyed while the callback is in flight.
      // Since we do this, it is OK to also pass 'this'.
      [this, self = std::weak_ptr<ControlServer>(shared_from_this()), device = device_,
       element_id](auto result) mutable {
        auto shared_self = self.lock();
        if (!shared_self) {
          ADR_WARN_STATIC() << "(element_id " << element_id
                            << ") ControlServer destroyed before CreateRingBuffer callback ran";
          if (result.is_ok()) {
            device->DropRingBuffer(element_id);
          }
          return;
        }

        // If we have no async completer, maybe we're shutting down and it was cleared. Just exit.
        auto completer_it = shared_self->create_ring_buffer_completers_.find(element_id);
        if (completer_it == shared_self->create_ring_buffer_completers_.end()) {
          ADR_WARN_OBJECT()
              << "(element_id " << element_id
              << ") create_ring_buffer_completer_ gone by the time the CreateRingBuffer callback ran";
          if (result.is_ok()) {
            shared_self->device_->DropRingBuffer(element_id);
          }
          return;
        }

        auto completer = std::move(completer_it->second);
        shared_self->create_ring_buffer_completers_.erase(element_id);

        if (result.is_error()) {
          completer.Reply(fit::error(result.take_error()));
          shared_self->DeviceDroppedRingBuffer(element_id);
          return;
        }

        completer.Reply(fit::success(fad::ControlCreateRingBufferResponse{
            {
                .properties = result.value().properties,
                .ring_buffer = std::move(result.value().ring_buffer),
            },
        }));
      });

  if (!created) {
    // Synchronous failure. The callback was already called, which replied to the completer.
    ADR_WARN_METHOD() << "device cannot create a ring buffer with the specified options";
  }
}

std::shared_ptr<PacketStreamServer> ControlServer::TryGetPacketStreamServer(ElementId element_id) {
  ADR_LOG_METHOD(kLogControlServerMethods);
  auto packet_stream_server_iter = packet_stream_servers_.find(element_id);
  if (packet_stream_server_iter != packet_stream_servers_.end()) {
    if (auto sh_packet_stream_server = packet_stream_server_iter->second.lock();
        sh_packet_stream_server) {
      return sh_packet_stream_server;
    }
    packet_stream_servers_.erase(element_id);
  }
  return nullptr;
}

void ControlServer::CreatePacketStream(CreatePacketStreamRequest& request,
                                       CreatePacketStreamCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogControlServerMethods);

  // Fail if device has error.
  if (device_has_error_) {
    ADR_WARN_METHOD() << "device has an error";
    completer.Reply(fit::error(fad::ControlCreatePacketStreamError::kDeviceError));
    return;
  }

  if (!device_->is_composite()) {
    ADR_WARN_METHOD() << "Unsupported method for device_type " << device_->device_type();
    completer.Reply(fit::error(fad::ControlCreatePacketStreamError::kWrongDeviceType));
    return;
  }
  // Fail on missing parameters.
  if (!request.element_id()) {
    ADR_WARN_METHOD() << "required field 'element_id' is missing";
    completer.Reply(fit::error(fad::ControlCreatePacketStreamError::kInvalidElementId));
    return;
  }
  ElementId element_id = *request.element_id();
  auto& ps_ids = device_->packet_stream_ids();
  if (!ps_ids.contains(element_id)) {
    ADR_WARN_METHOD() << "required field 'element_id' (" << element_id
                      << ") does not refer to an element of type PACKET_STREAM";
    completer.Reply(fit::error(fad::ControlCreatePacketStreamError::kInvalidElementId));
    return;
  }

  if (create_packet_stream_completers_.contains(element_id)) {
    ADR_WARN_METHOD() << "(element_id " << element_id
                      << ") previous `CreatePacketStream` request has not completed";
    completer.Reply(fit::error(fad::ControlCreatePacketStreamError::kAlreadyPending));
    return;
  }

  if (!request.options().has_value()) {
    ADR_WARN_METHOD() << "(element_id " << element_id << ") required field 'options' is missing";
    completer.Reply(fit::error(fad::ControlCreatePacketStreamError::kInvalidOptions));
    return;
  }

  if (!request.options()->format().has_value() ||
      (!request.options()->format()->pcm_format().has_value() &&
       !request.options()->format()->encoding().has_value())) {
    ADR_WARN_METHOD() << "(element_id " << element_id
                      << ") required 'options.format' is missing or is an unknown union variant";
    completer.Reply(fit::error(fad::ControlCreatePacketStreamError::kInvalidFormat));
    return;
  }

  if (request.options()->format()->pcm_format().has_value() &&
      (!request.options()->format()->pcm_format()->sample_type().has_value() ||
       !request.options()->format()->pcm_format()->channel_count().has_value() ||
       !request.options()->format()->pcm_format()->frames_per_second().has_value())) {
    ADR_WARN_METHOD() << "(element_id " << element_id
                      << ") required 'options.format.pcm_format' members are missing";
    completer.Reply(fit::error(fad::ControlCreatePacketStreamError::kInvalidFormat));
    return;
  }

  if (request.options()->format()->encoding().has_value() &&
      (!request.options()->format()->encoding()->encoding_type().has_value() ||
       !request.options()->format()->encoding()->decoded_channel_count().has_value() ||
       !request.options()->format()->encoding()->decoded_frame_rate().has_value())) {
    ADR_WARN_METHOD() << "(element_id " << element_id
                      << ") required 'options.format.encoding' members are missing";
    completer.Reply(fit::error(fad::ControlCreatePacketStreamError::kInvalidFormat));
    return;
  }

  if (!request.packet_stream_server().has_value()) {
    ADR_WARN_METHOD() << "(element_id " << element_id
                      << ") required field 'packet_stream_server' is missing";
    completer.Reply(fit::error(fad::ControlCreatePacketStreamError::kInvalidPacketStream));
    return;
  }
  if (TryGetPacketStreamServer(element_id)) {
    ADR_WARN_METHOD() << "(element_id " << element_id << ") device PacketStream already exists";
    completer.Reply(fit::error(fad::ControlCreatePacketStreamError::kAlreadyAllocated));
    return;
  }

  auto driver_format = device_->SupportedPacketStreamDriverFormatForClientFormat(
      element_id, *request.options()->format());
  // Fail if device cannot satisfy the requested format.
  if (!driver_format.has_value()) {
    ADR_WARN_METHOD() << "(element_id " << element_id
                      << ") device does not support the specified options";
    completer.Reply(fit::error(fad::ControlCreatePacketStreamError::kFormatMismatch));
    return;
  }

  auto packet_stream_server = parent_->CreatePacketStreamServer(
      std::move(*request.packet_stream_server()), shared_from_this(), device_, element_id);
  AddChildServer(packet_stream_server);
  packet_stream_servers_.insert_or_assign(element_id, packet_stream_server);

  // The completer is captured so we can respond asynchronously.
  create_packet_stream_completers_.insert_or_assign(element_id, completer.ToAsync());

  // `Device::CreatePacketStream` returns false if it fails synchronously. In that case, it has
  // already invoked the callback, which will have replied to the client. Note that
  // `Device::CreatePacketStream` always invokes the callback irrespective of the return value.
  bool created = device_->CreatePacketStream(
      element_id, *driver_format,
      // Capture a weak_ptr, in case the ControlServer is destroyed while the callback is in flight.
      // Since we do this, it is OK to also pass 'this'.
      [this, self = std::weak_ptr<ControlServer>(shared_from_this()), device = device_,
       element_id](fit::result<fad::ControlCreatePacketStreamError, Device::PacketStreamInfo>
                       result) mutable {
        auto shared_self = self.lock();
        if (!shared_self) {
          ADR_WARN_STATIC() << "(element_id " << element_id
                            << ") ControlServer destroyed before CreatePacketStream callback ran";
          if (result.is_ok()) {
            device->DropPacketStream(element_id);
          }
          return;
        }

        // If we have no async completer, maybe we're shutting down and it was cleared. Just exit.
        auto completer_it = shared_self->create_packet_stream_completers_.find(element_id);
        if (completer_it == shared_self->create_packet_stream_completers_.end()) {
          ADR_WARN_OBJECT()
              << "(element_id " << element_id
              << ") create_packet_stream_completer_ gone by the time the CreatePacketStream callback ran";
          if (result.is_ok()) {
            shared_self->device_->DropPacketStream(element_id);
          }
          return;
        }

        auto completer = std::move(completer_it->second);
        shared_self->create_packet_stream_completers_.erase(element_id);

        if (result.is_error()) {
          completer.Reply(fit::error(result.take_error()));
          shared_self->DeviceDroppedPacketStream(element_id);
          return;
        }

        fad::ControlCreatePacketStreamResponse response;
        response.properties(std::move(result.value().properties));
        completer.Reply(fit::success(std::move(response)));
      });

  if (!created) {
    // Synchronous failure. The callback was already called, which replied to the completer.
    ADR_WARN_METHOD() << "device cannot create a packet stream with the specified options";
  }
}

// This is only here because ControlNotify includes the methods from ObserverNotify. ControlServer
// doesn't have a role to play in plug state changes, nor a client hanging-get to complete.
void ControlServer::PlugStateIsChanged(const fad::PlugState& new_plug_state,
                                       zx::time plug_change_time) {
  ADR_LOG_METHOD(kLogNotifyMethods);
}

// We receive delay values for the first time during the configuration process. Once we have these
// values, we can calculate the required ring-buffer size and request the VMO.
void ControlServer::DelayInfoIsChanged(ElementId element_id, const fad::DelayInfo& delay_info) {
  ADR_LOG_METHOD(kLogControlServerResponses || kLogNotifyMethods);

  // Initialization is complete, so this represents a delay update.
  // If this is eventually exposed to Observers or any other watcher, notify them.
  if (auto ring_buffer_server = TryGetRingBufferServer(element_id); ring_buffer_server) {
    ring_buffer_server->DelayInfoIsChanged(delay_info);
  }
}

void ControlServer::SetDaiFormat(SetDaiFormatRequest& request,
                                 SetDaiFormatCompleter::Sync& completer) {
  if (device_has_error_) {
    ADR_WARN_METHOD() << "device has an error";
    completer.Reply(fit::error(fad::ControlSetDaiFormatError::kDeviceError));
    return;
  }

  ElementId element_id = fad::kDefaultDaiInterconnectElementId;
  // Fail on missing parameters.
  if (device_->is_composite()) {
    if (!request.element_id().has_value()) {
      ADR_WARN_METHOD() << "required field 'element_id' is missing";
      completer.Reply(fit::error(fad::ControlSetDaiFormatError::kInvalidElementId));
      return;
    }
    element_id = *request.element_id();
  } else if (!device_->is_codec()) {
    ADR_WARN_METHOD() << "Unsupported method for device_type " << device_->device_type();
    completer.Reply(fit::error(fad::ControlSetDaiFormatError::kWrongDeviceType));
    return;
  }
  if (set_dai_format_completers_.contains(element_id)) {
    ADR_WARN_METHOD() << "previous `SetDaiFormat` request has not yet completed";
    completer.Reply(fit::error(fad::ControlSetDaiFormatError::kAlreadyPending));
    return;
  }

  if (!request.dai_format().has_value() || !ValidateDaiFormat(*request.dai_format())) {
    ADR_WARN_METHOD() << "required field 'dai_format' is missing or invalid";
    completer.Reply(fit::error(fad::ControlSetDaiFormatError::kInvalidDaiFormat));
    return;
  }

  set_dai_format_completers_.insert_or_assign(element_id, completer.ToAsync());
  device_->SetDaiFormat(element_id, *request.dai_format());

  // We need the CodecFormatInfo to complete this, so we wait for the Device to notify us. Besides,
  // it's also possible that the underlying driver will reject the request (e.g. format mismatch).
}

// The Device's DaiFormat has changed. If `dai_format` is set, this resulted from `SetDaiFormat`
// being called. Otherwise, the Device is newly-initialized or `Reset` was called, so
// SetDaiFormat must be called again.
void ControlServer::DaiFormatIsChanged(
    ElementId element_id, const std::optional<fha::DaiFormat>& dai_format,
    const std::optional<fha::CodecFormatInfo>& codec_format_info) {
  ADR_LOG_METHOD(kLogControlServerMethods || kLogNotifyMethods) << "(" << element_id << ")";

  auto completer_match = set_dai_format_completers_.find(element_id);

  // Device must be newly-initialized or Reset was called. Either way we don't expect a completion.
  if (!dai_format.has_value()) {
    FX_DCHECK(!codec_format_info.has_value());
    // If there's a completer, it must have been in-progress when Reset was called; cancel it.
    if (completer_match != set_dai_format_completers_.end()) {
      auto completer = std::move(completer_match->second);
      set_dai_format_completers_.erase(element_id);
      completer.Reply(fit::error(fad::ControlSetDaiFormatError::kOther));
    }
    return;
  }

  // SetDaiFormat was called and succeeded, but now we don't have a completer.
  // We could be getting notified of the preexisting DaiFormat, upon establishing our Control.
  if (completer_match == set_dai_format_completers_.end()) {
    ADR_LOG_METHOD(kLogNotifyMethods) << "unsolicited Device notification (no completer).";
    return;
  }

  auto completer = std::move(set_dai_format_completers_.find(element_id)->second);
  set_dai_format_completers_.erase(element_id);
  if (codec_format_info.has_value()) {
    completer.Reply(fit::success(fad::ControlSetDaiFormatResponse{{
        .state = codec_format_info,
    }}));
  } else {
    completer.Reply(fit::success(fad::ControlSetDaiFormatResponse{{
        .state = std::nullopt,
    }}));
  }
}

// SetDaiFormat did NOT result in a change to the controlled device's DaiFormat.
void ControlServer::DaiFormatIsNotChanged(ElementId element_id, const fha::DaiFormat& dai_format,
                                          fad::ControlSetDaiFormatError error) {
  ADR_LOG_METHOD(kLogControlServerMethods || kLogNotifyMethods)
      << "(" << element_id << ", error " << fidl::ToUnderlying(error) << ") for dai_format:";
  if constexpr (kLogControlServerMethods || kLogNotifyMethods) {
    LogDaiFormat(dai_format);
  }

  // SetDaiFormat was called, but now we don't have a completer.
  if (!set_dai_format_completers_.contains(element_id)) {
    ADR_WARN_METHOD()
        << "SetDaiFormat was called, did not result in change, but completer is gone.";
    return;
  }

  auto completer = std::move(set_dai_format_completers_.find(element_id)->second);
  set_dai_format_completers_.erase(element_id);
  // If `error` is 0, SetDaiFormat was not an error but resulted in no change, so succeed that call.
  if (error == fad::ControlSetDaiFormatError(0)) {
    if (device_->dai_format_is_set()) {
      completer.Reply(fit::success(fad::ControlSetDaiFormatResponse{{
          .state = device_->codec_format_info(element_id),
      }}));
    } else {
      completer.Reply(fit::success(fad::ControlSetDaiFormatResponse{{
          .state = std::nullopt,
      }}));
    }
  } else {
    completer.Reply(fit::error(error));
  }
}

void ControlServer::CodecStart(CodecStartCompleter::Sync& completer) {
  if (device_has_error_) {
    ADR_WARN_METHOD() << "device has an error";
    completer.Reply(fit::error(fad::ControlCodecStartError::kDeviceError));
    return;
  }

  if (!device_->is_codec()) {
    ADR_WARN_METHOD() << "Unsupported method for device_type " << device_->device_type();
    completer.Reply(fit::error(fad::ControlCodecStartError::kWrongDeviceType));
    return;
  }

  // Check for already pending
  if (codec_start_completer_.has_value()) {
    ADR_WARN_METHOD() << "previous `CodecStart` request has not yet completed";
    completer.Reply(fit::error(fad::ControlCodecStartError::kAlreadyPending));
    return;
  }

  if (!device_->dai_format_is_set()) {
    ADR_WARN_METHOD() << "CodecStart called before DaiFormat was set";
    completer.Reply(fit::error(fad::ControlCodecStartError::kDaiFormatNotSet));
    return;
  }

  // Check for already started
  if (device_->codec_is_started()) {
    ADR_WARN_METHOD() << "Codec is already started";
    completer.Reply(fit::error(fad::ControlCodecStartError::kAlreadyStarted));
    return;
  }

  codec_start_completer_ = completer.ToAsync();

  // Call into the Device to Start.
  if (!device_->CodecStart()) {
    auto start_completer = std::move(*codec_start_completer_);
    codec_start_completer_.reset();
    start_completer.Reply(fit::error(fad::ControlCodecStartError::kDeviceError));
  }

  // We need `start_time` to complete this, so we wait for the Device to notify us. Besides,
  // it's also possible that the underlying driver will reject the request.
}

void ControlServer::CodecIsStarted(const zx::time& start_time) {
  ADR_LOG_METHOD(kLogNotifyMethods) << "(" << start_time.get() << ")";

  // The codec has been started, but we don't have a completer.
  // We could be getting notified of the preexisting DAI state, upon establishing our Control.
  if (!codec_start_completer_.has_value()) {
    ADR_LOG_METHOD(kLogNotifyMethods) << "unsolicited Device notification (no completer).";
    return;
  }

  auto completer = std::move(*codec_start_completer_);
  codec_start_completer_.reset();
  completer.Reply(fit::success(fad::ControlCodecStartResponse{{
      .start_time = start_time.get(),
  }}));
}

void ControlServer::CodecIsNotStarted() {
  ADR_LOG_METHOD(kLogNotifyMethods);

  if (!codec_start_completer_.has_value()) {
    // If we have no async completer, maybe we're shutting down and it was cleared. Just exit.
    ADR_WARN_METHOD() << "codec_start_completer_ gone by the time CodecIsNotStarted ran";
    return;
  }

  auto completer = std::move(*codec_start_completer_);
  codec_start_completer_.reset();
  completer.Reply(fit::error(fad::ControlCodecStartError::kOther));
}

void ControlServer::CodecStop(CodecStopCompleter::Sync& completer) {
  if (device_has_error_) {
    ADR_WARN_METHOD() << "device has an error";
    completer.Reply(fit::error(fad::ControlCodecStopError::kDeviceError));
    return;
  }

  if (!device_->is_codec()) {
    ADR_WARN_METHOD() << "Unsupported method for device_type " << device_->device_type();
    completer.Reply(fit::error(fad::ControlCodecStopError::kWrongDeviceType));
    return;
  }

  // Check for already pending
  if (codec_stop_completer_.has_value()) {
    ADR_WARN_METHOD() << "previous `CodecStop` request has not yet completed";
    completer.Reply(fit::error(fad::ControlCodecStopError::kAlreadyPending));
    return;
  }

  if (!device_->dai_format_is_set()) {
    ADR_WARN_METHOD() << "CodecStop called before DaiFormat was set";
    completer.Reply(fit::error(fad::ControlCodecStopError::kDaiFormatNotSet));
    return;
  }

  // Check for already stopped
  if (!device_->codec_is_started()) {
    ADR_WARN_METHOD() << "Codec is already stopped";
    completer.Reply(fit::error(fad::ControlCodecStopError::kAlreadyStopped));
    return;
  }

  codec_stop_completer_ = completer.ToAsync();

  // Call into the Device to Stop.
  if (!device_->CodecStop()) {
    auto stop_completer = std::move(*codec_stop_completer_);
    codec_stop_completer_.reset();
    stop_completer.Reply(fit::error(fad::ControlCodecStopError::kDeviceError));
    return;
  }

  // We need `stop_time` to complete this, so we wait for the Device to notify us. Besides,
  // it's also possible that the underlying driver will reject the request.
}

void ControlServer::CodecIsStopped(const zx::time& stop_time) {
  ADR_LOG_METHOD(kLogNotifyMethods) << "(" << stop_time.get() << ")";

  // The codec has been stopped, but we don't have a completer.
  // We could be getting notified of the preexisting DAI state, upon establishing our Control.
  // This could also occur if we were started when the DaiFormat was changed.
  // And finally, this could be triggered by a Reset call.
  if (!codec_stop_completer_.has_value()) {
    ADR_LOG_METHOD(kLogNotifyMethods) << "unsolicited Device notification (no completer).";
    return;
  }

  auto completer = std::move(*codec_stop_completer_);
  codec_stop_completer_.reset();
  completer.Reply(fit::success(fad::ControlCodecStopResponse{{
      .stop_time = stop_time.get(),
  }}));
}

void ControlServer::CodecIsNotStopped() {
  ADR_LOG_METHOD(kLogNotifyMethods);

  if (!codec_stop_completer_.has_value()) {
    // If we have no async completer, maybe we're shutting down and it was cleared. Just exit.
    ADR_WARN_METHOD() << "codec_stop_completer_ gone by the time CodecIsNotStopped ran";
    return;
  }

  auto completer = std::move(*codec_stop_completer_);
  codec_stop_completer_.reset();
  completer.Reply(fit::error(fad::ControlCodecStopError::kOther));
}

void ControlServer::Reset(ResetCompleter::Sync& completer) {
  if (device_has_error_) {
    ADR_WARN_METHOD() << "device has an error";
    completer.Reply(fit::error(fad::ControlResetError::kDeviceError));
    return;
  }

  if (!device_->is_codec() && !device_->is_composite()) {
    ADR_WARN_METHOD() << "Unsupported method for device_type " << device_->device_type();
    completer.Reply(fit::error(fad::ControlResetError::kWrongDeviceType));
    return;
  }

  if (reset_completer_.has_value()) {
    ADR_WARN_METHOD() << "previous `Reset` request has not yet completed";
    completer.Reply(fit::error(fad::ControlResetError::kAlreadyPending));
    return;
  }
  reset_completer_ = completer.ToAsync();

  // If Device::Reset returns false, then it will not subsequently call DeviceIsReset().
  if (!device_->Reset()) {
    ADR_WARN_METHOD() << "device had an error during Device::Reset";
    auto error_completer = std::move(*reset_completer_);
    reset_completer_.reset();
    error_completer.Reply(fit::error(fad::ControlResetError::kDeviceError));
    return;
  }
}

void ControlServer::DeviceIsReset() {
  if (reset_completer_.has_value()) {
    ADR_LOG_METHOD(kLogControlServerMethods || kLogNotifyMethods) << "completing";
    auto completer = std::move(*reset_completer_);
    reset_completer_.reset();
    completer.Reply(fit::success(fad::ControlResetResponse{}));
  } else {
    // If we have no async completer, maybe we're shutting down and it was cleared. Just exit.
    ADR_WARN_METHOD() << "reset_completer_ gone by the time DeviceIsReset ran";
  }
}

// fuchsia.hardware.audio.signalprocessing.SignalProcessing support
//
void ControlServer::GetTopologies(GetTopologiesCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogControlServerMethods);

  if (device_has_error_) {
    ADR_WARN_METHOD() << "Device has error";
    completer.Reply(zx::error(ZX_ERR_INTERNAL));
    return;
  }

  FX_CHECK(device_);
  if (!device_->is_codec() && !device_->is_composite()) {
    ADR_WARN_METHOD() << "This device_type does not support " << __func__;
    completer.Reply(zx::error(ZX_ERR_WRONG_TYPE));
    return;
  }

  if (!device_->supports_signalprocessing()) {
    ADR_LOG_METHOD(kLogControlServerMethods || kLogSignalProcessingFidlCalls)
        << "This driver does not support signalprocessing";
    completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
    return;
  }

  FX_CHECK(device_->info().has_value() &&
           device_->info()->signal_processing_topologies().has_value() &&
           !device_->info()->signal_processing_topologies()->empty());
  completer.Reply(zx::ok(*device_->info()->signal_processing_topologies()));
}

void ControlServer::WatchTopology(WatchTopologyCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogControlServerMethods);

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
    ADR_LOG_METHOD(kLogControlServerMethods || kLogSignalProcessingFidlCalls)
        << "This driver does not support signalprocessing";
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

void ControlServer::SetTopology(SetTopologyRequest& request,
                                SetTopologyCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogControlServerMethods);

  if (device_has_error_) {
    ADR_WARN_METHOD() << "Device has error";
    completer.Reply(zx::error(ZX_ERR_INTERNAL));
    return;
  }

  FX_CHECK(device_);
  if (!device_->is_codec() && !device_->is_composite()) {
    ADR_WARN_METHOD() << "unsupported method for this device_type";
    completer.Reply(zx::error(ZX_ERR_WRONG_TYPE));
    return;
  }

  if (!device_->supports_signalprocessing()) {
    ADR_LOG_METHOD(kLogControlServerMethods || kLogSignalProcessingFidlCalls)
        << "This driver does not support signalprocessing";
    completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
    return;
  }

  if (!device_->topology_ids().contains(request.topology_id())) {
    ADR_WARN_METHOD() << "Unknown topology_id " << request.topology_id();
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  if (auto status = device_->SetTopology(request.topology_id()); status == ZX_OK) {
    ADR_LOG_METHOD(kLogControlServerMethods || kLogSignalProcessingFidlCalls)
        << "SetTopology succeeded";
    completer.Reply(zx::ok());
  } else {
    ADR_LOG_METHOD(kLogControlServerMethods || kLogSignalProcessingFidlCalls)
        << "SetTopology failed: " << status;
    completer.Reply(zx::error(status));
  }
}

void ControlServer::TopologyIsChanged(TopologyId topology_id) {
  ADR_LOG_METHOD(kLogControlServerMethods || kLogNotifyMethods)
      << "(topology_id " << topology_id << ")";

  topology_id_to_notify_ = topology_id;
  MaybeCompleteWatchTopology();
}

void ControlServer::MaybeCompleteWatchTopology() {
  if (watch_topology_completer_.has_value() && topology_id_to_notify_.has_value()) {
    ADR_LOG_METHOD(kLogControlServerMethods || kLogNotifyMethods)
        << "completing(" << *topology_id_to_notify_ << ")";
    auto completer = std::move(*watch_topology_completer_);
    watch_topology_completer_.reset();

    auto new_topology_id = *topology_id_to_notify_;
    topology_id_to_notify_.reset();

    completer.Reply({new_topology_id});
  } else {
    ADR_LOG_METHOD(kLogControlServerMethods || kLogNotifyMethods) << "NOT completing";
  }
}

void ControlServer::GetElements(GetElementsCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogObserverServerMethods);

  if (device_has_error_) {
    ADR_WARN_METHOD() << "Device has error";
    completer.Reply(zx::error(ZX_ERR_INTERNAL));
    return;
  }

  FX_CHECK(device_);
  if (!device_->is_codec() && !device_->is_composite()) {
    ADR_WARN_METHOD() << "This device_type does not support " << __func__;
    completer.Reply(zx::error(ZX_ERR_WRONG_TYPE));
    return;
  }

  if (!device_->supports_signalprocessing()) {
    ADR_LOG_METHOD(kLogControlServerMethods || kLogSignalProcessingFidlCalls)
        << "This driver does not support signalprocessing";
    completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
    return;
  }

  FX_CHECK(device_->info().has_value() &&
           device_->info()->signal_processing_elements().has_value() &&
           !device_->info()->signal_processing_elements()->empty());
  completer.Reply(zx::ok(*device_->info()->signal_processing_elements()));
}

void ControlServer::WatchElementState(WatchElementStateRequest& request,
                                      WatchElementStateCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogControlServerMethods);

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
    ADR_LOG_METHOD(kLogControlServerMethods || kLogSignalProcessingFidlCalls)
        << "This driver does not support signalprocessing";
    completer.Close(ZX_ERR_NOT_SUPPORTED);
    return;
  }

  if (!device_->element_ids().contains(request.processing_element_id())) {
    ADR_WARN_METHOD() << "Unknown element_id " << request.processing_element_id();
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }

  ElementId element_id = request.processing_element_id();
  if (watch_element_state_completers_.contains(element_id)) {
    ADR_WARN_METHOD() << "previous `WatchElementState(" << element_id
                      << ")` request has not yet completed";
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }

  watch_element_state_completers_.insert({element_id, completer.ToAsync()});
  MaybeCompleteWatchElementState(element_id);
}

void ControlServer::SetElementState(SetElementStateRequest& request,
                                    SetElementStateCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogControlServerMethods)
      << "(element_id " << request.processing_element_id() << ")";

  if (device_has_error_) {
    ADR_WARN_METHOD() << "Device has error";
    completer.Reply(zx::error(ZX_ERR_INTERNAL));
    return;
  }

  FX_CHECK(device_);
  if (!device_->is_codec() && !device_->is_composite()) {
    ADR_WARN_METHOD() << "unsupported method for this device_type";
    completer.Reply(zx::error(ZX_ERR_WRONG_TYPE));
    return;
  }

  if (!device_->supports_signalprocessing()) {
    ADR_LOG_METHOD(kLogControlServerMethods || kLogSignalProcessingFidlCalls)
        << "This driver does not support signalprocessing";
    completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
    return;
  }

  if (!device_->element_ids().contains(request.processing_element_id())) {
    ADR_WARN_METHOD() << "Unknown element_id " << request.processing_element_id();
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  if (auto status = device_->SetElementState(request.processing_element_id(), request.state());
      status == ZX_OK) {
    completer.Reply(zx::ok());
  } else {
    completer.Reply(zx::error(status));
  }
}

void ControlServer::ElementStateIsChanged(
    ElementId element_id, fuchsia_hardware_audio_signalprocessing::ElementState element_state) {
  ADR_LOG_METHOD(kLogControlServerMethods || kLogNotifyMethods)
      << "(element_id " << element_id << ")";

  element_states_to_notify_.insert_or_assign(element_id, element_state);
  MaybeCompleteWatchElementState(element_id);
}

// If we have an outstanding hanging-get and a state-change, respond with the state change.
void ControlServer::MaybeCompleteWatchElementState(ElementId element_id) {
  if (watch_element_state_completers_.contains(element_id) &&
      element_states_to_notify_.contains(element_id)) {
    ADR_LOG_METHOD(kLogControlServerMethods || kLogNotifyMethods) << element_id << ": completing";
    auto completer = std::move(watch_element_state_completers_.find(element_id)->second);
    watch_element_state_completers_.erase(element_id);

    auto new_element_state = element_states_to_notify_.find(element_id)->second;
    element_states_to_notify_.erase(element_id);

    completer.Reply({new_element_state});
  } else {
    ADR_LOG_METHOD(kLogControlServerMethods || kLogNotifyMethods)
        << element_id << ": NOT completing";
  }
}

// We complain but don't close the connection, to accommodate older and newer clients.
void ControlServer::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_audio_device::Control> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  ADR_WARN_METHOD() << "(Control) ordinal " << metadata.method_ordinal;
  if (metadata.unknown_method_type == fidl::UnknownMethodType::kTwoWay) {
    // Pend the completer indefinitely.
    unknown_method_completers_.emplace_back(completer.ToAsync());
  }
}

}  // namespace media_audio

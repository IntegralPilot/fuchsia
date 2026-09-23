// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/services/device_registry/packet_stream_server.h"

#include <lib/syslog/cpp/macros.h>

#include "src/media/audio/services/device_registry/logging.h"

namespace media_audio {

namespace fad = fuchsia_audio_device;

std::shared_ptr<PacketStreamServer> PacketStreamServer::Create(
    std::shared_ptr<const FidlThread> thread, fidl::ServerEnd<fad::PacketStream> server_end,
    std::shared_ptr<ControlServer> parent, std::shared_ptr<Device> device, ElementId element_id) {
  ADR_LOG_STATIC(kLogPacketStreamServerMethods);

  return BaseFidlServer::Create(std::move(thread), std::move(server_end), std::move(parent),
                                std::move(device), element_id);
}

PacketStreamServer::PacketStreamServer(std::shared_ptr<ControlServer> parent,
                                       std::shared_ptr<Device> device, ElementId element_id)
    : parent_(std::move(parent)), device_(std::move(device)), element_id_(element_id) {
  ADR_LOG_METHOD(kLogObjectLifetimes);
  SetInspect(Inspector::Singleton()->RecordPacketStreamInstance(zx::clock::get_monotonic()));

  ++count_;
  LogObjectCounts();
}

PacketStreamServer::~PacketStreamServer() {
  ADR_LOG_METHOD(kLogObjectLifetimes);
  inspect()->RecordDestructionTime(zx::clock::get_monotonic());

  --count_;
  LogObjectCounts();
}

void PacketStreamServer::OnShutdown(fidl::UnbindInfo info) {
  if (!info.is_peer_closed() && !info.is_user_initiated()) {
    ADR_WARN_METHOD() << "shutdown with unexpected status: " << info;
  } else {
    ADR_LOG_METHOD(kLogPacketStreamFidlResponses || kLogObjectLifetimes) << "with status: " << info;
  }

  if (!device_dropped_packet_stream_) {
    device_->DropPacketStream(element_id_);
  }
}

void PacketStreamServer::DeviceDroppedPacketStream() {
  ADR_LOG_METHOD(kLogPacketStreamServerMethods || kLogNotifyMethods);
  device_dropped_packet_stream_ = true;
  Shutdown(ZX_ERR_PEER_CLOSED);
}

void PacketStreamServer::ClientDroppedControl() {
  ADR_LOG_METHOD(kLogPacketStreamServerMethods || kLogNotifyMethods);
  Shutdown(ZX_ERR_PEER_CLOSED);
}

// fuchsia.audio.device.PacketStream implementation

void PacketStreamServer::SetBuffers(SetBuffersRequest& request,
                                    SetBuffersCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogPacketStreamServerMethods);

  if (parent_->ControlledDeviceReceivedError()) {
    ADR_WARN_METHOD() << "device has an error";
    completer.Reply(fit::error(fad::PacketStreamSetBufferError::kDeviceError));
    return;
  }

  if (started_) {
    ADR_WARN_METHOD() << "PacketStream is already started";
    completer.Reply(fit::error(fad::PacketStreamSetBufferError::kAlreadyStarted));
    return;
  }

  if (buffers_are_set_) {
    ADR_WARN_METHOD() << "PacketStream already configured";
    completer.Reply(fit::error(fad::PacketStreamSetBufferError::kAlreadyConfigured));
    return;
  }

  if (!request.vmo_info().has_value() || request.vmo_info()->IsUnknown()) {
    ADR_WARN_METHOD() << "required field 'vmo_info' is missing or unknown";
    completer.Reply(fit::error(fad::PacketStreamSetBufferError::kBadVmoConfig));
    return;
  }

  if (request.vmo_info()->Which() == fad::PacketStreamSetupVmoInfo::Tag::kAllocateInfo) {
    const auto& allocate_info = request.vmo_info()->allocate_info().value();
    if (!allocate_info.vmo_count().has_value() || !allocate_info.min_vmo_size().has_value()) {
      ADR_WARN_METHOD() << "AllocateInfo Config missing required fields:"
                        << (!allocate_info.vmo_count().has_value() ? " vmo_count" : "")
                        << (!allocate_info.min_vmo_size().has_value() ? " min_vmo_size" : "");
      completer.Reply(fit::error(fad::PacketStreamSetBufferError::kBadVmoConfig));
      return;
    }
  } else if (request.vmo_info()->Which() == fad::PacketStreamSetupVmoInfo::Tag::kRegisterInfo) {
    const auto& register_info = request.vmo_info()->register_info().value();
    if (!register_info.vmo_infos().has_value() || register_info.vmo_infos()->empty()) {
      ADR_WARN_METHOD() << "RegisterInfo Config "
                        << (register_info.vmo_infos() ? "empty" : "missing") << " vmo_infos";
      completer.Reply(fit::error(fad::PacketStreamSetBufferError::kBadVmoConfig));
      return;
    }
  } else {
    ADR_WARN_METHOD() << "Unknown fad::PacketStreamSetupVmoInfo tag";
    completer.Reply(fit::error(fad::PacketStreamSetBufferError::kBadVmoConfig));
    return;
  }

  device_->SetPacketStreamBuffers(element_id_, std::move(*request.vmo_info()),
                                  [this, completer = completer.ToAsync()](auto result) mutable {
                                    if (result.is_error()) {
                                      ADR_WARN_OBJECT()
                                          << "PacketStream/SetBuffers failed: "
                                          << static_cast<uint32_t>(result.error_value());
                                      completer.Reply(fit::error(result.error_value()));
                                      return;
                                    }
                                    buffers_are_set_ = true;
                                    fad::PacketStreamSetBuffersResponse response;
                                    response.packet_stream(std::move(result.value()));
                                    completer.Reply(fit::success(std::move(response)));
                                  });
}

void PacketStreamServer::Start(StartRequest& request, StartCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogPacketStreamServerMethods);

  if (parent_->ControlledDeviceReceivedError()) {
    ADR_WARN_METHOD() << "device has an error";
    completer.Reply(fit::error(fad::PacketStreamStartError::kDeviceError));
    return;
  }

  if (started_) {
    ADR_WARN_METHOD() << "PacketStream is already started";
    completer.Reply(fit::error(fad::PacketStreamStartError::kAlreadyStarted));
    return;
  }

  if (start_completer_.has_value()) {
    ADR_WARN_METHOD() << "previous `Start` request has not yet completed";
    completer.Reply(fit::error(fad::PacketStreamStartError::kAlreadyPending));
    return;
  }

  start_completer_ = completer.ToAsync();
  device_->StartPacketStream(element_id_, [this](zx::result<> result) {
    ADR_LOG_OBJECT(kLogPacketStreamFidlResponses) << "Device/StartPacketStream response";
    // If we have no async completer, maybe we're shutting down and it was cleared. Just exit.
    if (!start_completer_.has_value()) {
      ADR_WARN_OBJECT() << "start_completer_ gone by the time the StartPacketStream callback ran";
      return;
    }

    auto completer = std::move(start_completer_);
    start_completer_.reset();
    if (result.is_error()) {
      ADR_WARN_OBJECT() << "Start callback: device has an error";
      completer->Reply(fit::error(fad::PacketStreamStartError::kDeviceError));
      return;
    }

    started_ = true;
    completer->Reply(fit::success(fad::PacketStreamStartResponse{}));
  });
}

void PacketStreamServer::Stop(StopRequest& request, StopCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogPacketStreamServerMethods);

  if (parent_->ControlledDeviceReceivedError()) {
    ADR_WARN_METHOD() << "device has an error";
    completer.Reply(fit::error(fad::PacketStreamStopError::kDeviceError));
    return;
  }

  if (!started_) {
    ADR_WARN_METHOD() << "PacketStream is already stopped";
    completer.Reply(fit::error(fad::PacketStreamStopError::kAlreadyStopped));
    return;
  }

  if (stop_completer_.has_value()) {
    ADR_WARN_METHOD() << "previous `Stop` request has not yet completed";
    completer.Reply(fit::error(fad::PacketStreamStopError::kAlreadyPending));
    return;
  }

  stop_completer_ = completer.ToAsync();
  device_->StopPacketStream(element_id_, [this](zx_status_t status) {
    ADR_LOG_OBJECT(kLogPacketStreamFidlResponses) << "Device/StopPacketStream response";
    if (!stop_completer_.has_value()) {
      // If we have no async completer, maybe we're shutting down and it was cleared. Just exit.
      ADR_WARN_OBJECT() << "stop_completer_ gone by the time the StopPacketStream callback ran";
      return;
    }

    auto completer = std::move(stop_completer_);
    stop_completer_.reset();
    if (status != ZX_OK) {
      ADR_WARN_OBJECT() << "Stop callback: device has an error";
      completer->Reply(fit::error(fad::PacketStreamStopError::kDeviceError));
      return;
    }

    started_ = false;
    completer->Reply(fit::success(fad::PacketStreamStopResponse{}));
  });
}

void PacketStreamServer::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_audio_device::PacketStream> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  ADR_WARN_METHOD() << "unknown method (PacketStream) ordinal " << metadata.method_ordinal;
  if (metadata.unknown_method_type == fidl::UnknownMethodType::kTwoWay) {
    // Pend the completer indefinitely.
    unknown_method_completers_.emplace_back(completer.ToAsync());
  }
}

}  // namespace media_audio

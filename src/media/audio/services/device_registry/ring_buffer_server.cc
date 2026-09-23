// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/services/device_registry/ring_buffer_server.h"

#include <fidl/fuchsia.audio.device/cpp/markers.h>
#include <lib/fidl/cpp/enum.h>
#include <lib/fidl/cpp/wire/unknown_interaction_handler.h>
#include <lib/fit/internal/result.h>
#include <lib/trace/event.h>
#include <lib/zx/clock.h>
#include <zircon/errors.h>

#include <utility>

#include "src/media/audio/services/common/base_fidl_server.h"
#include "src/media/audio/services/device_registry/control_server.h"
#include "src/media/audio/services/device_registry/device.h"
#include "src/media/audio/services/device_registry/inspector.h"
#include "src/media/audio/services/device_registry/logging.h"

namespace media_audio {

namespace fad = fuchsia_audio_device;

// static
std::shared_ptr<RingBufferServer> RingBufferServer::Create(
    std::shared_ptr<const FidlThread> thread, fidl::ServerEnd<fad::RingBuffer> server_end,
    std::shared_ptr<ControlServer> parent, std::shared_ptr<Device> device, ElementId element_id) {
  ADR_LOG_STATIC(kLogObjectLifetimes);

  return BaseFidlServer::Create(std::move(thread), std::move(server_end), std::move(parent),
                                std::move(device), element_id);
}

RingBufferServer::RingBufferServer(std::shared_ptr<ControlServer> parent,
                                   std::shared_ptr<Device> device, ElementId element_id)
    : parent_(std::move(parent)), device_(std::move(device)), element_id_(element_id) {
  ADR_LOG_METHOD(kLogObjectLifetimes);
  SetInspect(Inspector::Singleton()->RecordRingBufferInstance(zx::clock::get_monotonic()));

  ++count_;
  LogObjectCounts();
}

RingBufferServer::~RingBufferServer() {
  ADR_LOG_METHOD(kLogObjectLifetimes);
  inspect()->RecordDestructionTime(zx::clock::get_monotonic());

  --count_;
  LogObjectCounts();
}

// Called when the client drops the connection first.
void RingBufferServer::OnShutdown(fidl::UnbindInfo info) {
  if (!info.is_peer_closed() && !info.is_user_initiated()) {
    ADR_WARN_METHOD() << "shutdown with unexpected status: " << info;
  } else {
    ADR_LOG_METHOD(kLogRingBufferServerResponses || kLogObjectLifetimes) << "with status: " << info;
  }

  if (!device_dropped_ring_buffer_) {
    device_->DropRingBuffer(element_id_);

    // We don't explicitly clear our shared_ptr<Device> reference, to ensure we destruct first.
  }
}

void RingBufferServer::ClientDroppedControl() {
  ADR_LOG_METHOD(kLogObjectLifetimes);

  Shutdown(ZX_ERR_PEER_CLOSED);
  // Nothing else is needed: OnShutdown may call DropRingBuffer; our dtor will clear parent_.
}

// Called when the Device drops the RingBuffer FIDL.
void RingBufferServer::DeviceDroppedRingBuffer() {
  ADR_LOG_METHOD(kLogRingBufferServerMethods || kLogNotifyMethods);

  device_dropped_ring_buffer_ = true;
  Shutdown(ZX_ERR_PEER_CLOSED);

  // We don't explicitly clear our shared_ptr<Device> reference, to ensure we destruct first.
  // Same for parent_ -- we want to ensure we destruct before our parent ControlServer.
}

// fuchsia.audio.device.RingBuffer implementation
//
void RingBufferServer::SetActiveChannels(SetActiveChannelsRequest& request,
                                         SetActiveChannelsCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogRingBufferServerMethods);
  TRACE_DURATION("power-audio", "ADR::RingBufferServer::SetActiveChannels", "bitmask",
                 request.channel_bitmask().value_or(-1));

  if (parent_->ControlledDeviceReceivedError()) {
    ADR_WARN_METHOD() << "device has an error";
    TRACE_INSTANT("power-audio", "ADR::RingBufferServer::SetActiveChannels exit",
                  TRACE_SCOPE_PROCESS, "status",
                  fidl::ToUnderlying(fad::RingBufferSetActiveChannelsError::kDeviceError));
    completer.Reply(fit::error(fad::RingBufferSetActiveChannelsError::kDeviceError));
    return;
  }

  if (active_channels_completer_.has_value()) {
    ADR_WARN_METHOD() << "previous `SetActiveChannels` request has not yet completed";
    TRACE_INSTANT("power-audio", "ADR::RingBufferServer::SetActiveChannels exit",
                  TRACE_SCOPE_PROCESS, "status",
                  fidl::ToUnderlying(fad::RingBufferSetActiveChannelsError::kAlreadyPending));
    completer.Reply(fit::error(fad::RingBufferSetActiveChannelsError::kAlreadyPending));
    return;
  }

  // The first time this is called, we may not know whether the driver supports this method.
  // For subsequent calls, we can fast-finish here.
  if (!device_->supports_set_active_channels(element_id_).value_or(true)) {
    ADR_LOG_METHOD(kLogRingBufferServerMethods) << "device does not support SetActiveChannels";
    TRACE_INSTANT("power-audio", "ADR::RingBufferServer::SetActiveChannels exit",
                  TRACE_SCOPE_PROCESS, "status",
                  fidl::ToUnderlying(fad::RingBufferSetActiveChannelsError::kMethodNotSupported));
    completer.Reply(fit::error(fad::RingBufferSetActiveChannelsError::kMethodNotSupported));
    return;
  }

  if (!request.channel_bitmask().has_value()) {
    ADR_WARN_METHOD() << "required field 'channel_bitmask' is missing";
    TRACE_INSTANT(
        "power-audio", "ADR::RingBufferServer::SetActiveChannels exit", TRACE_SCOPE_PROCESS,
        "status",
        fidl::ToUnderlying(fad::RingBufferSetActiveChannelsError::kInvalidChannelBitmask));
    completer.Reply(fit::error(fad::RingBufferSetActiveChannelsError::kInvalidChannelBitmask));
    return;
  }
  auto bitmask = *request.channel_bitmask();
  FX_CHECK(device_->ring_buffer_format(element_id_).channel_count().has_value());
  if (bitmask >= (1u << *device_->ring_buffer_format(element_id_).channel_count())) {
    ADR_WARN_METHOD() << "channel_bitmask (0x0" << std::hex << bitmask << ") too large, for this "
                      << std::dec << *device_->ring_buffer_format(element_id_).channel_count()
                      << "-channel format";
    TRACE_INSTANT("power-audio", "ADR::RingBufferServer::SetActiveChannels exit",
                  TRACE_SCOPE_PROCESS, "status",
                  fidl::ToUnderlying(fad::RingBufferSetActiveChannelsError::kChannelOutOfRange),
                  "bitmask", bitmask);
    completer.Reply(fit::error(fad::RingBufferSetActiveChannelsError::kChannelOutOfRange));
    return;
  }

  active_channels_completer_ = completer.ToAsync();
  auto succeeded = device_->SetActiveChannels(
      element_id_, *request.channel_bitmask(), [this, bitmask](zx::result<zx::time> result) {
        ADR_LOG_OBJECT(kLogRingBufferMethods) << "Device/SetActiveChannels response";
        // If we have no async completer, maybe we're shutting down and it was cleared. Just exit.
        if (!active_channels_completer_.has_value()) {
          ADR_WARN_OBJECT()
              << "active_channels_completer_ gone by the time the StartRingBuffer callback ran";
          TRACE_INSTANT("power-audio", "ADR::RingBufferServer::SetActiveChannels response",
                        TRACE_SCOPE_PROCESS, "status", -1ll, "bitmask", bitmask);
          return;
        }

        auto completer = std::move(active_channels_completer_);
        active_channels_completer_.reset();
        if (result.is_error()) {
          if (result.status_value() == ZX_ERR_NOT_SUPPORTED) {
            ADR_LOG_OBJECT(kLogRingBufferServerMethods)
                << "device does not support SetActiveChannels";
            TRACE_INSTANT(
                "power-audio", "ADR::RingBufferServer::SetActiveChannels response",
                TRACE_SCOPE_PROCESS, "status",
                fidl::ToUnderlying(fad::RingBufferSetActiveChannelsError::kMethodNotSupported));
            completer->Reply(
                fit::error(fad::RingBufferSetActiveChannelsError::kMethodNotSupported));
            return;
          }

          ADR_WARN_OBJECT() << "SetActiveChannels callback: device has an error";
          TRACE_INSTANT("power-audio", "ADR::RingBufferServer::SetActiveChannels response",
                        TRACE_SCOPE_PROCESS, "status",
                        fidl::ToUnderlying(fad::RingBufferSetActiveChannelsError::kDeviceError),
                        "bitmask", bitmask);
          completer->Reply(fit::error(fad::RingBufferSetActiveChannelsError::kDeviceError));
        }

        TRACE_INSTANT("power-audio", "ADR::RingBufferServer::SetActiveChannels response",
                      TRACE_SCOPE_PROCESS, "status", ZX_OK, "bitmask", bitmask);
        completer->Reply(fit::success(fad::RingBufferSetActiveChannelsResponse{{
            .set_time = result.value().get(),
        }}));
      });

  // Should be prevented by the `supports_set_active_channels` check above, but if Device returns
  // false, it's because the element returned NOT_SUPPORTED from a previous SetActiveChannels.
  if (!succeeded) {
    ADR_LOG_METHOD(kLogRingBufferServerMethods) << "device does not support SetActiveChannels";
    auto completer = std::move(active_channels_completer_);
    active_channels_completer_.reset();
    TRACE_INSTANT("power-audio", "ADR::RingBufferServer::SetActiveChannels exit",
                  TRACE_SCOPE_PROCESS, "status",
                  fidl::ToUnderlying(fad::RingBufferSetActiveChannelsError::kMethodNotSupported),
                  "bitmask", bitmask);
    completer->Reply(fit::error(fad::RingBufferSetActiveChannelsError::kMethodNotSupported));
  }

  // Otherwise, `active_channels_completer_` is saved for the future async response.

  TRACE_INSTANT("power-audio", "ADR::RingBufferServer::SetActiveChannels exit", TRACE_SCOPE_PROCESS,
                "reason", "Waiting for async response", "bitmask", bitmask);
}

void RingBufferServer::Start(StartRequest& request, StartCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogRingBufferServerMethods);

  if (parent_->ControlledDeviceReceivedError()) {
    ADR_WARN_METHOD() << "device has an error";
    completer.Reply(fit::error(fad::RingBufferStartError::kDeviceError));
    return;
  }

  if (start_completer_.has_value()) {
    ADR_WARN_METHOD() << "previous `Start` request has not yet completed";
    completer.Reply(fit::error(fad::RingBufferStartError::kAlreadyPending));
    return;
  }

  if (started_) {
    ADR_WARN_METHOD() << "device is already started";
    completer.Reply(fit::error(fad::RingBufferStartError::kAlreadyStarted));
    return;
  }

  start_completer_ = completer.ToAsync();
  device_->StartRingBuffer(element_id_, [this](zx::result<zx::time> result) {
    ADR_LOG_OBJECT(kLogRingBufferMethods) << "Device/StartRingBuffer response";
    // If we have no async completer, maybe we're shutting down and it was cleared. Just exit.
    if (!start_completer_.has_value()) {
      ADR_WARN_OBJECT() << "start_completer_ gone by the time the StartRingBuffer callback ran";
      return;
    }

    auto completer = std::move(start_completer_);
    start_completer_.reset();
    if (result.is_error()) {
      ADR_WARN_OBJECT() << "Start callback: device has an error";
      completer->Reply(fit::error(fad::RingBufferStartError::kDeviceError));
    }

    started_ = true;
    completer->Reply(fit::success(fad::RingBufferStartResponse{{
        .start_time = result.value().get(),
    }}));
  });
}

void RingBufferServer::Stop(StopRequest& request, StopCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogRingBufferServerMethods);

  if (parent_->ControlledDeviceReceivedError()) {
    ADR_WARN_METHOD() << "device has an error";
    completer.Reply(fit::error(fad::RingBufferStopError::kDeviceError));
    return;
  }

  if (stop_completer_.has_value()) {
    ADR_WARN_METHOD() << "previous `Stop` request has not yet completed";
    completer.Reply(fit::error(fad::RingBufferStopError::kAlreadyPending));
    return;
  }

  if (!started_) {
    ADR_WARN_METHOD() << "device is not started";
    completer.Reply(fit::error(fad::RingBufferStopError::kAlreadyStopped));
    return;
  }

  stop_completer_ = completer.ToAsync();
  device_->StopRingBuffer(element_id_, [this](zx_status_t status) {
    ADR_LOG_OBJECT(kLogRingBufferMethods) << "Device/StopRingBuffer response";
    if (!stop_completer_.has_value()) {
      // If we have no async completer, maybe we're shutting down and it was cleared. Just exit.
      ADR_WARN_OBJECT() << "stop_completer_ gone by the time the StopRingBuffer callback ran";
      return;
    }

    auto completer = std::move(stop_completer_);
    stop_completer_.reset();
    if (status != ZX_OK) {
      ADR_WARN_OBJECT() << "Stop callback: device has an error";
      completer->Reply(fit::error(fad::RingBufferStopError::kDeviceError));
      return;
    }

    started_ = false;
    completer->Reply(fit::success(fad::RingBufferStopResponse{}));
  });
}

void RingBufferServer::WatchDelayInfo(WatchDelayInfoCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogRingBufferServerMethods);

  if (parent_->ControlledDeviceReceivedError()) {
    ADR_WARN_METHOD() << "device has an error";
    completer.Reply(fit::error(fad::RingBufferWatchDelayInfoError::kDeviceError));
    return;
  }

  if (delay_info_completer_.has_value()) {
    ADR_WARN_METHOD() << "previous `WatchDelayInfo` request has not yet completed";
    completer.Reply(fit::error(fad::RingBufferWatchDelayInfoError::kAlreadyPending));
    return;
  }

  delay_info_completer_ = completer.ToAsync();
  MaybeCompleteWatchDelayInfo();
}

void RingBufferServer::DelayInfoIsChanged(const fad::DelayInfo& delay_info) {
  ADR_LOG_METHOD(kLogNotifyMethods);

  new_delay_info_to_notify_ = delay_info;
  MaybeCompleteWatchDelayInfo();
}

void RingBufferServer::MaybeCompleteWatchDelayInfo() {
  if (new_delay_info_to_notify_.has_value() && delay_info_completer_.has_value()) {
    auto delay_info = *new_delay_info_to_notify_;
    new_delay_info_to_notify_.reset();

    auto completer = std::move(*delay_info_completer_);
    delay_info_completer_.reset();

    completer.Reply(fit::success(fad::RingBufferWatchDelayInfoResponse{{
        .delay_info = delay_info,
    }}));
  }
}

// We complain but don't close the connection, to accommodate older and newer clients.
void RingBufferServer::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_audio_device::RingBuffer> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  ADR_WARN_METHOD() << "unknown method (RingBuffer) ordinal " << metadata.method_ordinal;
  if (metadata.unknown_method_type == fidl::UnknownMethodType::kTwoWay) {
    // Pend the completer indefinitely.
    unknown_method_completers_.emplace_back(completer.ToAsync());
  }
}

}  // namespace media_audio

// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/consumer/consumer.h"

#include <fidl/fuchsia.media.audio/cpp/markers.h>
#include <fidl/fuchsia.media/cpp/common_types.h>
#include <lib/async/dispatcher.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fit/function.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/errors.h>

#include <cmath>

namespace media::audio {
namespace {

constexpr uint64_t kMinimumLeadTime = ZX_MSEC(30);
constexpr uint64_t kMaximumLeadTime = ZX_MSEC(500);

template <typename Protocol>
void EnsureUnbound(std::optional<fidl::ServerBindingRef<Protocol>>& binding_ref) {
  if (binding_ref) {
    binding_ref->Unbind();
    binding_ref.reset();
  }
}

template <typename Protocol>
void EnsureUnbound(fidl::Client<Protocol>& client) {
  client = fidl::Client<Protocol>();
}

}  // namespace

// static
void Consumer::CreateAndBind(async_dispatcher_t* dispatcher,
                             fidl::ClientEnd<fuchsia_media::AudioCore> audio_core_client_end,
                             fidl::ServerEnd<fuchsia_media::AudioConsumer> server_end) {
  auto impl = std::make_shared<Consumer>(dispatcher, std::move(audio_core_client_end));

  // This is a separate step, because the |Consumer| must be managed by a |shared_ptr| before
  // |Bind| may be called.
  impl->Bind(std::move(server_end));

  // If |Bind| fails, |impl| is the sole reference to the |Consumer| at this point, so the
  // |Consumer| will be deleted here when |impl| goes out of scope.
}

Consumer::Consumer(async_dispatcher_t* dispatcher,
                   fidl::ClientEnd<fuchsia_media::AudioCore> audio_core_client_end)
    : dispatcher_(dispatcher),
      audio_core_event_handler_(fit::bind_member(this, &Consumer::OnAudioCoreFidlError)),
      audio_core_(
          fidl::Client(std::move(audio_core_client_end), dispatcher_, &audio_core_event_handler_)),
      gain_control_event_handler_(fit::bind_member(this, &Consumer::OnGainControlFidlError)) {}

void Consumer::Bind(fidl::ServerEnd<fuchsia_media::AudioConsumer> server_end) {
  zx::result endpoints = fidl::CreateEndpoints<fuchsia_media::AudioRenderer>();
  if (!endpoints.is_ok()) {
    FX_LOGS(ERROR) << "Failed to create endponts: " << endpoints.status_string();
    return;
  }

  fit::result<fidl::OneWayError> result =
      audio_core_->CreateAudioRenderer({{.audio_out_request = std::move(endpoints->server)}});
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to create audio renderer: " << result.error_value().status_string();
    return;
  }

  renderer_ = fidl::Client(std::move(endpoints->client), dispatcher_, this);

  result = renderer_->SetUsage2({{.usage2 = fuchsia_media::AudioRenderUsage2::kMedia}});
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to set usage: " << result.error_value().status_string();
    return;
  }

  // Having performed the preceding operations successfully, we reference this |Consumer| from
  // the server binding here so it won't be deleted when |CreateAndBind| completes.
  consumer_binding_ref_ = fidl::BindServer(
      dispatcher_, std::move(server_end), shared_from_this(),
      [this](fidl::Server<fuchsia_media::AudioConsumer>* impl_unused, fidl::UnbindInfo info,
             fidl::ServerEnd<fuchsia_media::AudioConsumer> server_end) { UnbindAll(); });
}

void Consumer::UnbindAll() {
  pending_stream_sink_completer_.reset();

  EnsureUnbound(consumer_binding_ref_);
  EnsureUnbound(stream_sink_binding_ref_);
  EnsureUnbound(volume_binding_ref_);

  EnsureUnbound(audio_core_);
  EnsureUnbound(renderer_);
  EnsureUnbound(gain_control_);
}

void Consumer::CreateStreamSink(CreateStreamSinkRequest& request,
                                CreateStreamSinkCompleter::Sync& completer) {
  pending_stream_sink_request_ = std::move(request);
  pending_stream_sink_completer_ = completer.ToAsync();

  if (!stream_sink_binding_ref_) {
    MaybeCompletePendingCreateStreamSinkRequest();
  }
}

void Consumer::MaybeCompletePendingCreateStreamSinkRequest() {
  FX_DCHECK(!stream_sink_binding_ref_);

  if (!renderer_) {
    return;
  }

  if (!pending_stream_sink_completer_) {
    // No request pending.
    return;
  }

  if (buffers_previously_added_ != 0) {
    fit::result<fidl::OneWayError> result = renderer_->PauseNoReply();
    if (result.is_error()) {
      FX_LOGS(WARNING) << "Failed to pause: " << result.error_value().status_string();
    }

    result = renderer_->DiscardAllPacketsNoReply();
    if (result.is_error()) {
      FX_LOGS(WARNING) << "Failed to discard packets: " << result.error_value().status_string();
    }

    RemoveAddedPayloadBuffers();
  }

  // Compression isn't currently supported. Close the |StreamSink| server end indicating invalid
  // args.
  if (pending_stream_sink_request_.compression() != nullptr) {
    FX_LOGS(WARNING) << "Compression not supported, compression type "
                     << pending_stream_sink_request_.compression()->type() << " requested";
    pending_stream_sink_request_.stream_sink_request().Close(ZX_ERR_INVALID_ARGS);
    return;
  }

  fit::result<fidl::OneWayError> result = renderer_->SetPcmStreamType(
      {{.type = std::move(pending_stream_sink_request_.stream_type())}});
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to set stream type: " << result.error_value().status_string();
    pending_stream_sink_completer_->Close(result.error_value().status());
    pending_stream_sink_completer_.reset();
    return;
  }

  uint32_t buffers_previously_added_ = 0;
  for (auto& buffer : pending_stream_sink_request_.buffers()) {
    result = renderer_->AddPayloadBuffer({{
        .id = buffers_previously_added_++,
        .payload_buffer = std::move(buffer),
    }});
    if (result.is_error()) {
      FX_LOGS(ERROR) << "Failed to add payload buffer: " << result.error_value().status_string();
      pending_stream_sink_completer_->Close(result.error_value().status());
      pending_stream_sink_completer_.reset();
      RemoveAddedPayloadBuffers();
      return;
    }
  }

  stream_sink_binding_ref_ =
      fidl::BindServer(dispatcher_, std::move(pending_stream_sink_request_.stream_sink_request()),
                       shared_from_this(),
                       [this](fidl::Server<fuchsia_media::StreamSink>* impl, fidl::UnbindInfo info,
                              fidl::ServerEnd<fuchsia_media::StreamSink> server_end) {
                         stream_sink_binding_ref_.reset();
                         MaybeCompletePendingCreateStreamSinkRequest();
                       });

  pending_stream_sink_completer_.reset();
}

void Consumer::RemoveAddedPayloadBuffers() {
  while (buffers_previously_added_ != 0) {
    fit::result<fidl::OneWayError> result =
        renderer_->RemovePayloadBuffer({{.id = --buffers_previously_added_}});
    if (result.is_error()) {
      FX_LOGS(WARNING) << "Failed to remove payload buffer: "
                       << result.error_value().status_string();
      break;
    }
  }
}

void Consumer::Start(StartRequest& request, StartCompleter::Sync& completer) {
  if (!renderer_) {
    return;
  }

  fit::result<fidl::OneWayError> result = renderer_->PlayNoReply({{
      .reference_time = request.reference_time(),
      .media_time = request.media_time(),
  }});
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to PlayNoReply: " << result.error_value().status_string();
    completer.Close(result.error_value().status());
    return;
  }

  timeline_function_ =
      media::TimelineFunction(request.media_time(), request.reference_time(), 1, 1);
  StatusChanged();
}

void Consumer::Stop(StopCompleter::Sync& completer) {
  if (!renderer_) {
    return;
  }

  fit::result<fidl::OneWayError> result = renderer_->PauseNoReply();
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to PauseNoReply: " << result.error_value().status_string();
    completer.Close(result.error_value().status());
    return;
  }

  auto reference_time = zx::clock::get_monotonic().get();
  auto media_time = timeline_function_(reference_time);

  timeline_function_ = media::TimelineFunction(media_time, reference_time, 0, 1);
  StatusChanged();
}

void Consumer::SetRate(SetRateRequest& request, SetRateCompleter::Sync& completer) {
  // Not supported.
  FX_LOGS(WARNING) << "Call to unsupported SetRate method";
  completer.Close(ZX_ERR_NOT_SUPPORTED);
}

void Consumer::BindVolumeControl(BindVolumeControlRequest& request,
                                 BindVolumeControlCompleter::Sync& completer) {
  if (!renderer_) {
    return;
  }

  zx::result endpoints = fidl::CreateEndpoints<fuchsia_media_audio::GainControl>();
  if (!endpoints.is_ok()) {
    FX_LOGS(ERROR) << "Failed to create endponts: " << endpoints.status_string();
    return;
  }

  fit::result<fidl::OneWayError> result = renderer_->BindGainControl(std::move(endpoints->server));
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to BindGainControl: " << result.error_value().status_string();
    return;
  }

  gain_control_ =
      fidl::Client(std::move(endpoints->client), dispatcher_, &gain_control_event_handler_);

  volume_binding_ref_ = fidl::BindServer(
      dispatcher_, std::move(request.volume_control_request()), shared_from_this(),
      [this](fidl::Server<fuchsia_media_audio::VolumeControl>* impl, fidl::UnbindInfo info,
             fidl::ServerEnd<fuchsia_media_audio::VolumeControl> server_end) {
        volume_binding_ref_.reset();
        EnsureUnbound(gain_control_);
      });
}

void Consumer::WatchStatus(WatchStatusCompleter::Sync& completer) {
  watch_status_completer_ = completer.ToAsync();
  MaybeCompleteWatchStatus();
}

void Consumer::SendPacket(SendPacketRequest& request, SendPacketCompleter::Sync& completer) {
  if (!renderer_) {
    return;
  }

  renderer_->SendPacket({{.packet = std::move(request.packet())}})
      .Then([completer = completer.ToAsync()](auto& result) mutable {
        if (result.is_ok()) {
          completer.Reply();
        } else {
          FX_LOGS(ERROR) << "Failed to SendPacket: " << result.error_value().status_string();
          completer.Close(result.error_value().status());
        }
      });
}

void Consumer::SendPacketNoReply(SendPacketNoReplyRequest& request,
                                 SendPacketNoReplyCompleter::Sync& completer) {
  if (!renderer_) {
    return;
  }

  fit::result<fidl::OneWayError> result =
      renderer_->SendPacketNoReply({{.packet = std::move(request.packet())}});
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to SendPacketNoReply: " << result.error_value().status_string();
    completer.Close(result.error_value().status());
    return;
  }
}

void Consumer::EndOfStream(EndOfStreamCompleter::Sync& completer) {
  if (!renderer_) {
    return;
  }

  // TODO(https://fxbug.dev/42077439): Maybe need logic to send OnEndOfStream, if clients need it.
  fit::result<fidl::OneWayError> result = renderer_->EndOfStream();
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to EndOfStream: " << result.error_value().status_string();
    completer.Close(result.error_value().status());
    return;
  }
}

void Consumer::DiscardAllPackets(DiscardAllPacketsCompleter::Sync& completer) {
  if (!renderer_) {
    return;
  }

  renderer_->DiscardAllPackets().Then([completer = completer.ToAsync()](auto& result) mutable {
    if (result.is_ok()) {
      completer.Reply();
    } else {
      FX_LOGS(ERROR) << "Failed to DiscardAllPackets: " << result.error_value().status_string();
      completer.Close(result.error_value().status());
    }
  });
}

void Consumer::DiscardAllPacketsNoReply(DiscardAllPacketsNoReplyCompleter::Sync& completer) {
  if (!renderer_) {
    return;
  }

  fit::result<fidl::OneWayError> result = renderer_->DiscardAllPacketsNoReply();
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to DiscardAllPacketsNoReply: "
                   << result.error_value().status_string();
    completer.Close(result.error_value().status());
    return;
  }
}

void Consumer::SetVolume(SetVolumeRequest& request, SetVolumeCompleter::Sync& completer) {
  if (!gain_control_) {
    return;
  }
  if (!std::isfinite(request.volume())) {
    FX_LOGS(WARNING) << "Consumer::SetVolume(" << request.volume()
                     << ") is indefinite; ignoring this call";
    return;
  }

  audio_core_
      ->GetDbFromVolume2({{
          .usage = fuchsia_media::Usage2::WithRenderUsage(fuchsia_media::AudioRenderUsage2::kMedia),
          .volume = request.volume(),
      }})
      .Then([this](auto& result) {
        if (!gain_control_) {
          FX_LOGS(WARNING) << "Gain control lost during SetVolume processing";
          return;
        }

        fit::result<fidl::OneWayError> set_gain_result =
            gain_control_->SetGain({{.gain_db = result.value().gain_db()}});
        if (result.is_error()) {
          FX_LOGS(WARNING) << "Failed to SetGain to " << result.value().gain_db() << ": "
                           << set_gain_result.error_value().status_string();
          return;
        }
      });
}

void Consumer::SetMute(SetMuteRequest& request, SetMuteCompleter::Sync& completer) {
  if (!gain_control_) {
    return;
  }

  fit::result<fidl::OneWayError> result = gain_control_->SetMute({{.muted = request.mute()}});
  if (result.is_error()) {
    FX_LOGS(WARNING) << "Failed to SetMute: " << result.error_value().status_string();
    return;
  }
}

void Consumer::OnMinLeadTimeChanged(
    fidl::Event<fuchsia_media::AudioRenderer::OnMinLeadTimeChanged>& event) {}

void Consumer::on_fidl_error(fidl::UnbindInfo error) {
  FX_LOGS(WARNING) << "AudioRenderer unbound, " << error.status_string();
  UnbindAll();
}

void Consumer::StatusChanged() {
  status_dirty_ = true;
  MaybeCompleteWatchStatus();
}

void Consumer::MaybeCompleteWatchStatus() {
  if (!status_dirty_) {
    return;
  }

  if (!watch_status_completer_) {
    return;
  }

  // The fixed lead time values are consistent with the previous implementation.
  watch_status_completer_->Reply({{.status = {{
                                       .presentation_timeline = fuchsia_media::TimelineFunction{{
                                           .subject_time = timeline_function_.subject_time(),
                                           .reference_time = timeline_function_.reference_time(),
                                           .subject_delta = timeline_function_.subject_delta(),
                                           .reference_delta = timeline_function_.reference_delta(),
                                       }},
                                       .min_lead_time = kMinimumLeadTime,
                                       .max_lead_time = kMaximumLeadTime,
                                   }}}});

  status_dirty_ = false;
  watch_status_completer_.reset();
}

void Consumer::OnAudioCoreFidlError(fidl::UnbindInfo error) {
  FX_LOGS(WARNING) << "AudioCore unbound, " << error.status_string() << ", closing all channels.";
  UnbindAll();
}

void Consumer::OnGainControlFidlError(fidl::UnbindInfo error) {
  FX_LOGS(WARNING) << "GainControl unbound, " << error.status_string() << ", closing all channels.";
  UnbindAll();
}

}  // namespace media::audio

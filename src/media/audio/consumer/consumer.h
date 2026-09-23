// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_CONSUMER_CONSUMER_H_
#define SRC_MEDIA_AUDIO_CONSUMER_CONSUMER_H_

#include <fidl/fuchsia.media/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/fidl/cpp/unified_messaging_declarations.h>
#include <lib/fidl/cpp/wire/unknown_interaction_handler.h>
#include <lib/media/cpp/timeline_function.h>
#include <lib/syslog/cpp/macros.h>

#include <memory>

namespace media::audio {

class Consumer : public fidl::Server<fuchsia_media::AudioConsumer>,
                 public fidl::Server<fuchsia_media::StreamSink>,
                 public fidl::Server<fuchsia_media_audio::VolumeControl>,
                 public fidl::AsyncEventHandler<fuchsia_media::AudioRenderer>,
                 public std::enable_shared_from_this<Consumer> {
 public:
  // Creates a |Consumer| and binds it to |server_end|. |audio_core_client_end| is supplied here
  // to enable dependency injection.
  static void CreateAndBind(async_dispatcher_t* dispatcher,
                            fidl::ClientEnd<fuchsia_media::AudioCore> audio_core_client_end,
                            fidl::ServerEnd<fuchsia_media::AudioConsumer> server_end);

  Consumer(async_dispatcher_t* dispatcher,
           fidl::ClientEnd<fuchsia_media::AudioCore> audio_core_client_end);

  ~Consumer() override = default;

  // Disallow copy, assign and move.
  Consumer(const Consumer&) = delete;
  Consumer& operator=(const Consumer&) = delete;
  Consumer(Consumer&&) = delete;
  Consumer& operator=(Consumer&&) = delete;

  // fuchsia_media::AudioConsumer implementation.
  void CreateStreamSink(CreateStreamSinkRequest& request,
                        CreateStreamSinkCompleter::Sync& completer) override;

  void Start(StartRequest& request, StartCompleter::Sync& completer) override;

  void Stop(StopCompleter::Sync& completer) override;

  void SetRate(SetRateRequest& request, SetRateCompleter::Sync& completer) override;

  void BindVolumeControl(BindVolumeControlRequest& request,
                         BindVolumeControlCompleter::Sync& completer) override;

  void WatchStatus(WatchStatusCompleter::Sync& completer) override;

  // fuchsia_media::StreamSink implementation.
  void SendPacket(SendPacketRequest& request, SendPacketCompleter::Sync& completer) override;

  void SendPacketNoReply(SendPacketNoReplyRequest& request,
                         SendPacketNoReplyCompleter::Sync& completer) override;

  void EndOfStream(EndOfStreamCompleter::Sync& completer) override;

  void DiscardAllPackets(DiscardAllPacketsCompleter::Sync& completer) override;

  void DiscardAllPacketsNoReply(DiscardAllPacketsNoReplyCompleter::Sync& completer) override;

  // fuchsia_media_audio::VolumeControl implementation.
  void SetVolume(SetVolumeRequest& request, SetVolumeCompleter::Sync& completer) override;

  void SetMute(SetMuteRequest& request, SetMuteCompleter::Sync& completer) override;

  // fidl::AsyncEventHandler<fuchsia_media::AudioRenderer> implementation.
  void OnMinLeadTimeChanged(
      fidl::Event<fuchsia_media::AudioRenderer::OnMinLeadTimeChanged>& event) override;

  // Handles errors concerning the connection to |AudioRenderer|.
  void on_fidl_error(fidl::UnbindInfo error) override;
  void handle_unknown_event(
      fidl::UnknownEventMetadata<fuchsia_media::AudioRenderer> metadata) override {
    FX_LOGS(WARNING) << "Consumer: unknown event (AudioRenderer) ordinal "
                     << metadata.event_ordinal;
  }

 private:
  template <typename Protocol>
  class AsyncEventHandler : public fidl::AsyncEventHandler<Protocol> {
   public:
    explicit AsyncEventHandler(fit::function<void(fidl::UnbindInfo)> fidl_error_callback)
        : fidl_error_callback_(std::move(fidl_error_callback)) {}

    void on_fidl_error(fidl::UnbindInfo error) override { fidl_error_callback_(error); }

   private:
    fit::function<void(fidl::UnbindInfo)> fidl_error_callback_;
  };
  template <typename Protocol>
  class AsyncEventHandlerFlex : public AsyncEventHandler<Protocol> {
   public:
    explicit AsyncEventHandlerFlex(fit::function<void(fidl::UnbindInfo)> fidl_error_callback)
        : AsyncEventHandler<Protocol>(std::move(fidl_error_callback)) {}
    void handle_unknown_event(fidl::UnknownEventMetadata<Protocol> metadata) override {
      FX_LOGS(WARNING) << "Consumer: unknown event (AudioCore) ordinal " << metadata.event_ordinal;
    }
  };

  // Binds to an audio renderer and to |server_end|. If successful, this method calls
  // |shared_from_this|, so this |Consumer| must be managed using a |shared_ptr| prior to calling
  // this method. If the binding to |AudioRenderer| fails synchronously, |server_end| is dropped and
  // |shared_from_this| is not called.
  void Bind(fidl::ServerEnd<fuchsia_media::AudioConsumer> server_end);

  // Unbinds all server and client connections.
  void UnbindAll();

  // Completes a pending |StreamSink| connection request if such a request exists.
  // |stream_sink_binding_ref_| must be empty when this method is called.
  void MaybeCompletePendingCreateStreamSinkRequest();

  // Removes payload buffers previously added to the renderer, if any.
  void RemoveAddedPayloadBuffers();

  // Indicates that the status watched by |WatchStatus| has changed.
  void StatusChanged();

  // Completes a pending |WatchStatus| request if there is one and if the status is dirty.
  void MaybeCompleteWatchStatus();

  // Handles errors concerning the connection to |AudioCore|.
  void OnAudioCoreFidlError(fidl::UnbindInfo error);

  // Handles errors concerning the connection to |GainControl|.
  void OnGainControlFidlError(fidl::UnbindInfo error);

  async_dispatcher_t* dispatcher_;
  std::optional<fidl::ServerBindingRef<fuchsia_media::AudioConsumer>> consumer_binding_ref_;
  CreateStreamSinkRequest pending_stream_sink_request_;
  std::optional<CreateStreamSinkCompleter::Async> pending_stream_sink_completer_;
  std::optional<fidl::ServerBindingRef<fuchsia_media::StreamSink>> stream_sink_binding_ref_;
  std::optional<fidl::ServerBindingRef<fuchsia_media_audio::VolumeControl>> volume_binding_ref_;

  AsyncEventHandlerFlex<fuchsia_media::AudioCore> audio_core_event_handler_;
  fidl::Client<fuchsia_media::AudioCore> audio_core_;

  fidl::Client<fuchsia_media::AudioRenderer> renderer_;
  uint32_t buffers_previously_added_ = 0;

  AsyncEventHandler<fuchsia_media_audio::GainControl> gain_control_event_handler_;
  fidl::Client<fuchsia_media_audio::GainControl> gain_control_;

  std::optional<WatchStatusCompleter::Async> watch_status_completer_;
  bool status_dirty_ = true;
  media::TimelineFunction timeline_function_;
};

}  // namespace media::audio

#endif  // SRC_MEDIA_AUDIO_CONSUMER_CONSUMER_H_

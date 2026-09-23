// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_AUDIO_CORE_AUDIO_CORE_IMPL_H_
#define SRC_MEDIA_AUDIO_AUDIO_CORE_AUDIO_CORE_IMPL_H_

#include <fuchsia/media/cpp/fidl.h>
#include <lib/fidl/cpp/binding_set.h>
#include <lib/trace/event.h>

#include "src/media/audio/audio_core/context.h"

namespace media::audio {

class AudioCoreImpl final : public fuchsia::media::AudioCore {
 public:
  AudioCoreImpl(Context* context);

  // Disallow copy & move.
  AudioCoreImpl(AudioCoreImpl&& o) = delete;
  AudioCoreImpl& operator=(AudioCoreImpl&& o) = delete;
  AudioCoreImpl(const AudioCoreImpl&) = delete;
  AudioCoreImpl& operator=(const AudioCoreImpl&) = delete;

  ~AudioCoreImpl() override;

 private:
  // Forwards requests to the AudioCore service.
  friend class AudioImpl;

  // |fuchsia::media::AudioCore|
  void CreateAudioRenderer(
      fidl::InterfaceRequest<fuchsia::media::AudioRenderer> audio_renderer_request) final;
  void CreateAudioCapturerWithConfiguration(
      fuchsia::media::AudioStreamType stream_type,
      fuchsia::media::AudioCapturerConfiguration configuration,
      fidl::InterfaceRequest<fuchsia::media::AudioCapturer> audio_capturer_request) final;
  void CreateAudioCapturer(
      bool loopback,
      fidl::InterfaceRequest<fuchsia::media::AudioCapturer> audio_capturer_request) final;
  void SetRenderUsageGain(fuchsia::media::AudioRenderUsage usage, float gain_db) final;
  void SetRenderUsageGain2(fuchsia::media::AudioRenderUsage2 usage, float gain_db) final;
  void SetCaptureUsageGain(fuchsia::media::AudioCaptureUsage usage, float gain_db) final;
  void SetCaptureUsageGain2(fuchsia::media::AudioCaptureUsage2 usage, float gain_db) final;
  void BindUsageVolumeControl(
      fuchsia::media::Usage usage,
      fidl::InterfaceRequest<fuchsia::media::audio::VolumeControl> volume_control) final;
  void BindUsageVolumeControl2(
      fuchsia::media::Usage2 usage,
      fidl::InterfaceRequest<fuchsia::media::audio::VolumeControl> volume_control) final;
  void GetDbFromVolume(fuchsia::media::Usage usage, float volume,
                       GetDbFromVolumeCallback callback) final;
  void GetDbFromVolume2(fuchsia::media::Usage2 usage, float volume,
                        GetDbFromVolume2Callback callback) final;
  void GetVolumeFromDb(fuchsia::media::Usage usage, float gain_db,
                       GetVolumeFromDbCallback callback) final;
  void GetVolumeFromDb2(fuchsia::media::Usage2 usage, float gain_db,
                        GetVolumeFromDb2Callback callback) final;
  void SetInteraction(fuchsia::media::Usage active, fuchsia::media::Usage affected,
                      fuchsia::media::Behavior behavior) final;
  void SetInteraction2(fuchsia::media::Usage2 active, fuchsia::media::Usage2 affected,
                       fuchsia::media::Behavior behavior) final;
  void ResetInteractions() final;
  void LoadDefaults() final;
  void handle_unknown_method(uint64_t ordinal, bool method_has_response) final {
    FX_LOGS(ERROR) << "AudioCoreImpl: AudioCore::handle_unknown_method(ordinal " << ordinal
                   << ", method_has_response " << method_has_response << ")";
  }

  float GetVolumeFromDbBase(const fuchsia::media::Usage2& usage, float gain_db);
  float GetDbFromVolumeBase(const fuchsia::media::Usage2& usage, float volume);
  void Shutdown();

  fidl::BindingSet<fuchsia::media::AudioCore> bindings_;

  Context& context_;
};

}  // namespace media::audio

#endif  // SRC_MEDIA_AUDIO_AUDIO_CORE_AUDIO_CORE_IMPL_H_

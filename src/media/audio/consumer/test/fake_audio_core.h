// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_CONSUMER_TEST_FAKE_AUDIO_CORE_H_
#define SRC_MEDIA_AUDIO_CONSUMER_TEST_FAKE_AUDIO_CORE_H_

#include <fidl/fuchsia.media/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>

#include <memory>

#include <gtest/gtest.h>

#include "src/media/audio/consumer/test/fake_audio_renderer.h"

namespace media::audio::tests {

class FakeAudioCore final : public fidl::Server<fuchsia_media::AudioCore> {
 public:
  FakeAudioCore(async_dispatcher_t* dispatcher,
                fidl::ServerEnd<fuchsia_media::AudioCore> server_end)
      : dispatcher_(dispatcher) {
    binding_ref_ = fidl::BindServer(
        dispatcher, std::move(server_end), this,
        [this](fidl::Server<fuchsia_media::AudioCore>* impl, fidl::UnbindInfo info,
               fidl::ServerEnd<fuchsia_media::AudioCore> server_end) { unbind_completed_ = true; });
  }

  ~FakeAudioCore() override = default;

  // Disallow copy, assign and move.
  FakeAudioCore(const FakeAudioCore&) = delete;
  FakeAudioCore& operator=(const FakeAudioCore&) = delete;
  FakeAudioCore(FakeAudioCore&&) = delete;
  FakeAudioCore& operator=(FakeAudioCore&&) = delete;

  void Unbind() {
    if (binding_ref_) {
      binding_ref_->Unbind();
    }
  }

  bool UnbindCompleted() const { return unbind_completed_; }

  // fuchsia_media::AudioCore implementation.
  void CreateAudioRenderer(CreateAudioRendererRequest& request,
                           CreateAudioRendererCompleter::Sync& completer) override {
    EXPECT_FALSE(create_audio_renderer_artifact_);
    create_audio_renderer_artifact_ =
        std::make_unique<FakeAudioRenderer>(dispatcher_, std::move(request.audio_out_request()));
  }

  void CreateAudioCapturer(CreateAudioCapturerRequest&,
                           CreateAudioCapturerCompleter::Sync&) override {
    FX_NOTIMPLEMENTED();
  }

  void CreateAudioCapturerWithConfiguration(
      CreateAudioCapturerWithConfigurationRequest&,
      CreateAudioCapturerWithConfigurationCompleter::Sync&) override {
    FX_NOTIMPLEMENTED();
  }

  void SetRenderUsageGain(SetRenderUsageGainRequest&, SetRenderUsageGainCompleter::Sync&) override {
    FX_NOTIMPLEMENTED();
  }

  void SetRenderUsageGain2(SetRenderUsageGain2Request&,
                           SetRenderUsageGain2Completer::Sync&) override {
    FX_NOTIMPLEMENTED();
  }

  void SetCaptureUsageGain(SetCaptureUsageGainRequest&,
                           SetCaptureUsageGainCompleter::Sync&) override {
    FX_NOTIMPLEMENTED();
  }

  void SetCaptureUsageGain2(SetCaptureUsageGain2Request&,
                            SetCaptureUsageGain2Completer::Sync&) override {
    FX_NOTIMPLEMENTED();
  }

  void BindUsageVolumeControl(BindUsageVolumeControlRequest&,
                              BindUsageVolumeControlCompleter::Sync&) override {
    FX_NOTIMPLEMENTED();
  }

  void BindUsageVolumeControl2(BindUsageVolumeControl2Request&,
                               BindUsageVolumeControl2Completer::Sync&) override {
    FX_NOTIMPLEMENTED();
  }

  void GetDbFromVolume(GetDbFromVolumeRequest&, GetDbFromVolumeCompleter::Sync&) override {
    FX_NOTIMPLEMENTED();
  }

  void GetDbFromVolume2(GetDbFromVolume2Request& request,
                        GetDbFromVolume2Completer::Sync& completer) override {
    EXPECT_FALSE(get_db_from_volume_artifact_);
    get_db_from_volume_artifact_ = std::make_unique<GetDbFromValueArtifact>(
        std::move(request.usage()), request.volume(), completer.ToAsync());
  }

  void GetVolumeFromDb(GetVolumeFromDbRequest&, GetVolumeFromDbCompleter::Sync&) override {
    FX_NOTIMPLEMENTED();
  }

  void GetVolumeFromDb2(GetVolumeFromDb2Request&, GetVolumeFromDb2Completer::Sync&) override {
    FX_NOTIMPLEMENTED();
  }

  void SetInteraction(SetInteractionRequest&, SetInteractionCompleter::Sync&) override {
    FX_NOTIMPLEMENTED();
  }

  void SetInteraction2(SetInteraction2Request&, SetInteraction2Completer::Sync&) override {
    FX_NOTIMPLEMENTED();
  }

  void ResetInteractions(ResetInteractionsCompleter::Sync&) override { FX_NOTIMPLEMENTED(); }

  void LoadDefaults(LoadDefaultsCompleter::Sync&) override { FX_NOTIMPLEMENTED(); }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_media::AudioCore> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    FX_LOGS(ERROR) << "FakeAudioCore: AudioCore::handle_unknown_method(ordinal "
                   << metadata.method_ordinal << ", "
                   << (metadata.unknown_method_type == fidl::UnknownMethodType::kOneWay
                           ? "OneWay)"
                           : "TwoWay)");
  }

  // Checks
  bool WasGetDbFromVolumeCalled(const fuchsia_media::Usage2& expected_usage, float expected_volume,
                                float gain_db_to_return) {
    EXPECT_TRUE(get_db_from_volume_artifact_);
    if (!get_db_from_volume_artifact_) {
      return false;
    }

    EXPECT_EQ(expected_usage, get_db_from_volume_artifact_->usage);
    EXPECT_EQ(expected_volume, get_db_from_volume_artifact_->volume);
    if (expected_usage != get_db_from_volume_artifact_->usage ||
        expected_volume != get_db_from_volume_artifact_->volume) {
      get_db_from_volume_artifact_.reset();
      return false;
    }

    get_db_from_volume_artifact_->completer.Reply({{.gain_db = gain_db_to_return}});

    get_db_from_volume_artifact_.reset();
    return true;
  }

  // Returns a unique pointer to the previously-created audio renderer, if one was created, a null
  // unique pointer otherwise. Note that the caller is responsible for the lifetime of the fake
  // returned fake audio renderer after this call.
  std::unique_ptr<FakeAudioRenderer> WasCreateAudioRendererCalled() {
    return std::move(create_audio_renderer_artifact_);
  }

 private:
  struct GetDbFromValueArtifact {
    GetDbFromValueArtifact(fuchsia_media::Usage2 usage, float volume,
                           GetDbFromVolume2Completer::Async completer)
        : usage(std::move(usage)), volume(volume), completer(std::move(completer)) {}
    fuchsia_media::Usage2 usage;
    float volume;
    GetDbFromVolume2Completer::Async completer;
  };

  bool unbind_completed_ = false;
  async_dispatcher_t* dispatcher_;
  std::optional<fidl::ServerBindingRef<fuchsia_media::AudioCore>> binding_ref_;
  std::unique_ptr<FakeAudioRenderer> create_audio_renderer_artifact_;
  std::unique_ptr<GetDbFromValueArtifact> get_db_from_volume_artifact_;
};

}  // namespace media::audio::tests

#endif  // SRC_MEDIA_AUDIO_CONSUMER_TEST_FAKE_AUDIO_CORE_H_

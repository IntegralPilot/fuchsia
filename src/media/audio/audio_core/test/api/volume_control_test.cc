// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/media/audio/cpp/fidl.h>
#include <fuchsia/media/cpp/fidl.h>

#include "src/media/audio/audio_core/stream_usage.h"
#include "src/media/audio/audio_core/testing/integration/hermetic_audio_test.h"

using fuchsia::media::AudioCaptureUsage2;
using fuchsia::media::AudioRenderUsage2;

namespace media::audio::test {
namespace {

class VolumeControlTest : public HermeticAudioTest {
 protected:
  fuchsia::media::audio::VolumeControlPtr CreateRenderUsageControl(AudioRenderUsage2 usage) {
    fuchsia::media::audio::VolumeControlPtr c;
    audio_core_->BindUsageVolumeControl2(
        fuchsia::media::Usage2::WithRenderUsage(fidl::Clone(usage)), c.NewRequest());
    AddErrorHandler(c, "VolumeControl");
    return c;
  }
};

TEST_F(VolumeControlTest, SetVolumeAndMute) {
  auto client1 = CreateRenderUsageControl(AudioRenderUsage2::MEDIA);
  auto client2 = CreateRenderUsageControl(AudioRenderUsage2::MEDIA);

  float volume = 0.0;
  bool muted = false;
  auto add_callback = [this, &client2, &volume, &muted]() {
    client2.events().OnVolumeMuteChanged =
        AddCallback("OnVolumeMuteChanged", [&volume, &muted](float new_volume, bool new_muted) {
          volume = new_volume;
          muted = new_muted;
        });
  };

  // The initial callback happens immediately.
  add_callback();
  ExpectCallbacks();
  EXPECT_FLOAT_EQ(volume, 1.0);
  EXPECT_EQ(muted, false);

  // Further callbacks happen in response to events.
  add_callback();
  client1->SetVolume(0.5);
  ExpectCallbacks();
  EXPECT_FLOAT_EQ(volume, 0.5);
  EXPECT_EQ(muted, false);

  add_callback();
  client1->SetMute(true);
  ExpectCallbacks();
  EXPECT_EQ(muted, true);

  // Unmute should restore the volume.
  add_callback();
  client1->SetMute(false);
  ExpectCallbacks();
  EXPECT_FLOAT_EQ(volume, 0.5);
  EXPECT_EQ(muted, false);
}

TEST_F(VolumeControlTest, RoutedCorrectly) {
  auto c1 = CreateRenderUsageControl(AudioRenderUsage2::MEDIA);
  auto c2 = CreateRenderUsageControl(AudioRenderUsage2::BACKGROUND);

  // The initial callbacks happen immediately.
  c1.events().OnVolumeMuteChanged = AddCallback("OnVolumeMuteChanged1 InitialCall");
  c2.events().OnVolumeMuteChanged = AddCallback("OnVolumeMuteChanged2 InitialCall");
  ExpectCallbacks();

  // Routing to c1.
  c1.events().OnVolumeMuteChanged = AddCallback("OnVolumeMuteChanged1 RouteTo1");
  c2.events().OnVolumeMuteChanged = AddUnexpectedCallback("OnVolumeMuteChanged2 RouteTo1");
  c1->SetVolume(0);
  ExpectCallbacks();

  // Routing to c2.
  c1.events().OnVolumeMuteChanged = AddUnexpectedCallback("OnVolumeMuteChanged1 RouteTo2");
  c2.events().OnVolumeMuteChanged = AddCallback("OnVolumeMuteChanged2 RouteTo2");
  c2->SetVolume(0);
  ExpectCallbacks();
}

// If BindUsageVolumeControl2 is called with an unknown RenderUsage2, the client should not connect
// (ZX_ERR_INVALID_ARGS) but the preexisting AudioCore connection should not be affected.
TEST_F(VolumeControlTest, FailToConnectToUnknownRenderUsageVolume) {
  fuchsia::media::audio::VolumeControlPtr client;
  audio_core_->BindUsageVolumeControl2(
      fuchsia::media::Usage2::WithRenderUsage(static_cast<AudioRenderUsage2>(68)),
      client.NewRequest());
  AddErrorHandler(client, "VolumeControl");

  ExpectError(client, ZX_ERR_INVALID_ARGS);
  RunLoopUntilIdle();
  EXPECT_TRUE(audio_core_.is_bound());
}

// If BindUsageVolumeControl2 is called with a CaptureUsage, the client should not connect
// (ZX_ERR_NOT_SUPPORTED) but the preexisting AudioCore connection should not be affected.
TEST_F(VolumeControlTest, FailToConnectToCaptureUsageVolume) {
  fuchsia::media::audio::VolumeControlPtr client;
  audio_core_->BindUsageVolumeControl2(ToFidlUsage2(CaptureUsage::SYSTEM_AGENT),
                                       client.NewRequest());
  AddErrorHandler(client, "VolumeControl");

  ExpectError(client, ZX_ERR_NOT_SUPPORTED);
  RunLoopUntilIdle();
  EXPECT_TRUE(audio_core_.is_bound());
}

// If BindUsageVolumeControl2 is called with an unknown CaptureUsage2, the client should not connect
// (ZX_ERR_INVALID_ARGS) but the preexisting AudioCore connection should not be affected.
TEST_F(VolumeControlTest, FailToConnectToUnknownCaptureUsageVolume) {
  fuchsia::media::audio::VolumeControlPtr client;
  audio_core_->BindUsageVolumeControl2(
      fuchsia::media::Usage2::WithCaptureUsage(static_cast<AudioCaptureUsage2>(42)),
      client.NewRequest());
  AddErrorHandler(client, "VolumeControl");

  ExpectError(client, ZX_ERR_INVALID_ARGS);
  RunLoopUntilIdle();
  EXPECT_TRUE(audio_core_.is_bound());
}

TEST_F(VolumeControlTest, VolumeCurveLookups) {
  // Test audio_core instance will have default volume curve, just check the ends.
  float db_lookup = 0.0f;
  float volume_lookup = 0.0f;
  audio_core_->GetDbFromVolume(
      *ToFidlUsageTry(AudioRenderUsage2::MEDIA), 0.0f,
      AddCallback("GetDbFromVolume", [&db_lookup](float db) { db_lookup = db; }));
  ExpectCallbacks();
  EXPECT_EQ(db_lookup, -160.0f);

  audio_core_->GetDbFromVolume(
      *ToFidlUsageTry(AudioRenderUsage2::MEDIA), 1.0f,
      AddCallback("GetDbFromVolume", [&db_lookup](float db) { db_lookup = db; }));
  ExpectCallbacks();
  EXPECT_EQ(db_lookup, 0.0f);

  audio_core_->GetVolumeFromDb(
      *ToFidlUsageTry(AudioRenderUsage2::MEDIA), -160.0f,
      AddCallback("GetVolumeFromDb", [&volume_lookup](float volume) { volume_lookup = volume; }));
  ExpectCallbacks();
  EXPECT_EQ(volume_lookup, 0.0f);

  audio_core_->GetVolumeFromDb(
      *ToFidlUsageTry(AudioRenderUsage2::MEDIA), 0.0f,
      AddCallback("GetVolumeFromDb", [&volume_lookup](float volume) { volume_lookup = volume; }));
  ExpectCallbacks();
  EXPECT_EQ(volume_lookup, 1.0f);
}

TEST_F(VolumeControlTest, Volume2CurveLookups) {
  // Test audio_core instance will have the default volume curve, so just check the ends.
  // Volume 0.0 corresponds to MUTED (-160.0 dB), volume 1.0 corresponds to UNITY (0.0 dB).
  float db_lookup = 0.0f;
  audio_core_->GetDbFromVolume2(
      ToFidlUsage2(RenderUsage::MEDIA), 0.0f,
      AddCallback("GetDbFromVolume2",
                  [&db_lookup](fuchsia::media::AudioCore_GetDbFromVolume2_Result result) {
                    ASSERT_TRUE(result.is_response());
                    db_lookup = result.response().gain_db;
                  }));
  ExpectCallbacks();
  EXPECT_EQ(db_lookup, -160.0f);

  audio_core_->GetDbFromVolume2(
      ToFidlUsage2(RenderUsage::MEDIA), 1.0f,
      AddCallback("GetDbFromVolume2",
                  [&db_lookup](fuchsia::media::AudioCore_GetDbFromVolume2_Result result) {
                    ASSERT_TRUE(result.is_response());
                    db_lookup = result.response().gain_db;
                  }));
  ExpectCallbacks();
  EXPECT_EQ(db_lookup, 0.0f);

  float volume_lookup = 1.0f;
  audio_core_->GetVolumeFromDb2(
      ToFidlUsage2(RenderUsage::MEDIA), -160.0f,
      AddCallback("GetVolumeFromDb2",
                  [&volume_lookup](fuchsia::media::AudioCore_GetVolumeFromDb2_Result result) {
                    ASSERT_TRUE(result.is_response());
                    volume_lookup = result.response().volume;
                  }));
  ExpectCallbacks();
  EXPECT_EQ(volume_lookup, 0.0f);

  audio_core_->GetVolumeFromDb2(
      ToFidlUsage2(RenderUsage::MEDIA), 0.0f,
      AddCallback("GetVolumeFromDb2",
                  [&volume_lookup](fuchsia::media::AudioCore_GetVolumeFromDb2_Result result) {
                    ASSERT_TRUE(result.is_response());
                    volume_lookup = result.response().volume;
                  }));
  ExpectCallbacks();
  EXPECT_EQ(volume_lookup, 1.0f);
}

}  // namespace
}  // namespace media::audio::test

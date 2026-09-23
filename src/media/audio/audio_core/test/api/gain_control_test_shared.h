// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_AUDIO_CORE_TEST_API_GAIN_CONTROL_TEST_SHARED_H_
#define SRC_MEDIA_AUDIO_AUDIO_CORE_TEST_API_GAIN_CONTROL_TEST_SHARED_H_

#include <fuchsia/media/cpp/fidl.h>

#include <gtest/gtest.h>

#include "src/media/audio/audio_core/testing/integration/hermetic_audio_test.h"

// TYPED_TEST_SUITE uses RTTI to print type names, but RTTI is disabled in our build, so
// specialize this function to get nicer test failure messages.
namespace testing::internal {
template <>
std::string GetTypeName<fuchsia::media::AudioRendererPtr>() {
  return "AudioRenderer";
}
template <>
std::string GetTypeName<fuchsia::media::AudioCapturerPtr>() {
  return "AudioCapturer";
}
}  // namespace testing::internal

namespace media::audio::test {

template <typename RendererOrCapturerT>
struct RendererOrCapturerTraits {};

template <>
struct RendererOrCapturerTraits<fuchsia::media::AudioRendererPtr> {
  static std::string Name() { return "AudioRenderer"; }
  static void Create(fuchsia::media::AudioCorePtr& audio_core,
                     fuchsia::media::AudioRendererPtr& p) {
    audio_core->CreateAudioRenderer(p.NewRequest());
  }
};

template <>
struct RendererOrCapturerTraits<fuchsia::media::AudioCapturerPtr> {
  static std::string Name() { return "AudioCapturer"; }
  static void Create(fuchsia::media::AudioCorePtr& audio_core,
                     fuchsia::media::AudioCapturerPtr& p) {
    audio_core->CreateAudioCapturer(false, p.NewRequest());
  }
};

template <typename RendererOrCapturerT>
class GainControlTest : public HermeticAudioTest {
 protected:
  void SetUp() override {
    HermeticAudioTest::SetUp();

    // Create two gain controllers tied to the same parent object. We will manipulate
    // gain_control_1_ while expecting events on both gain controllers.
    RendererOrCapturerTraits<RendererOrCapturerT>::Create(audio_core_, parent_);
    AddErrorHandler(parent_, RendererOrCapturerTraits<RendererOrCapturerT>::Name());

    // Bind gc2 first. If we do this in the opposite order, then commands sent to gc1
    // might happen concurrently with the binding of gc2, meaning gc2 will miss updates.
    parent_->BindGainControl(gain_control_2_.NewRequest());
    parent_->BindGainControl(gain_control_1_.NewRequest());
    AddErrorHandler(gain_control_1_,
                    RendererOrCapturerTraits<RendererOrCapturerT>::Name() + "::GainControl-1");
    AddErrorHandler(gain_control_2_,
                    RendererOrCapturerTraits<RendererOrCapturerT>::Name() + "::GainControl-2");

    // To ensure there is no crosstalk, we create an unused renderer and capturer
    // and a gain control for each, and verify those gain controls are not called.
    audio_core_->CreateAudioRenderer(unused_renderer_.NewRequest());
    audio_core_->CreateAudioCapturer(false, unused_capturer_.NewRequest());
    AddErrorHandler(unused_renderer_, "AudioRenderer (unused)");
    AddErrorHandler(unused_capturer_, "AudioCapturer (unused)");

    SetUpUnusedGainControl(unused_renderer_gain_control_, unused_renderer_);
    SetUpUnusedGainControl(unused_capturer_gain_control_, unused_capturer_);
  }

  template <typename ParentT>
  void SetUpUnusedGainControl(fuchsia::media::audio::GainControlPtr& gc, ParentT& parent) {
    parent->BindGainControl(gc.NewRequest());
    AddErrorHandler(gc, RendererOrCapturerTraits<ParentT>::Name() + "::GainControl (unused)");

    gc.events().OnGainMuteChanged = [](float gain_db, bool muted) {
      ADD_FAILURE() << "Unexpected call to unused " << RendererOrCapturerTraits<ParentT>::Name()
                    << "'s GainControl: OnGainMuteChanged(" << gain_db << ", " << muted << ")";
    };
  }

  void ExpectGainCallback(float expected_gain_db, bool expected_mute) {
    float received_gain_db_1 = NAN;
    float received_gain_db_2 = NAN;
    bool received_mute_1 = false;
    bool received_mute_2 = false;

    // We bound gc2 first, so it gets the event first.
    gain_control_2_.events().OnGainMuteChanged =
        AddCallback("GainControl2::OnGainMuteChanged",
                    [&received_gain_db_2, &received_mute_2](float gain_db, bool muted) {
                      received_gain_db_2 = gain_db;
                      received_mute_2 = muted;
                    });
    gain_control_1_.events().OnGainMuteChanged =
        AddCallback("GainControl1::OnGainMuteChanged",
                    [&received_gain_db_1, &received_mute_1](float gain_db, bool muted) {
                      received_gain_db_1 = gain_db;
                      received_mute_1 = muted;
                    });

    ExpectCallbacks();
    EXPECT_EQ(received_gain_db_1, expected_gain_db);
    EXPECT_EQ(received_gain_db_2, expected_gain_db);
    EXPECT_EQ(received_mute_1, expected_mute);
    EXPECT_EQ(received_mute_2, expected_mute);
  }

  void ExpectNoGainCallback() {
    gain_control_2_.events().OnGainMuteChanged =
        AddCallback("GainControl2::OnGainMuteChanged", [](float gain_db, bool muted) {
          ADD_FAILURE() << "unexpected GainControl2::OnGainMuteChanged callback";
        });
    gain_control_1_.events().OnGainMuteChanged =
        AddCallback("GainControl1::OnGainMuteChanged", [](float gain_db, bool muted) {
          ADD_FAILURE() << "unexpected GainControl1::OnGainMuteChanged callback";
        });
    // If audio_core is not buggy, the above callbacks won't fire.
    // Wait 1s to ensure that does not happen.
    ExpectNoCallbacks(zx::sec(1), "while expecting no OnGainMuteChanged callbacks");
  }

  void ExpectParentDisconnect() {
    // Disconnecting the parent should also disconnect the GainControls.
    ExpectDisconnects({ErrorHandlerFor(parent_), ErrorHandlerFor(gain_control_1_),
                       ErrorHandlerFor(gain_control_2_)});
  }

  void SetGain(float gain_db) { gain_control_1_->SetGain(gain_db); }
  void SetMute(bool mute) { gain_control_1_->SetMute(mute); }
  void SetGainWithRamp(float gain_db, zx::duration ramp_duration) {
    gain_control_1_->SetGainWithRamp(gain_db, ramp_duration.get(),
                                     fuchsia::media::audio::RampType::SCALE_LINEAR);
  }

  fuchsia::media::audio::GainControlPtr& gain_control_1() { return gain_control_1_; }
  fuchsia::media::audio::GainControlPtr& gain_control_2() { return gain_control_2_; }

 private:
  RendererOrCapturerT parent_;
  fuchsia::media::audio::GainControlPtr gain_control_1_;
  fuchsia::media::audio::GainControlPtr gain_control_2_;

  fuchsia::media::AudioRendererPtr unused_renderer_;
  fuchsia::media::AudioCapturerPtr unused_capturer_;
  fuchsia::media::audio::GainControlPtr unused_renderer_gain_control_;
  fuchsia::media::audio::GainControlPtr unused_capturer_gain_control_;
};

using GainControlTestTypes =
    ::testing::Types<fuchsia::media::AudioRendererPtr, fuchsia::media::AudioCapturerPtr>;
class GainControlTypeNames {
 public:
  template <typename T>
  static std::string GetName(int) {
    if (std::is_same<T, fuchsia::media::AudioRendererPtr>())
      return "AudioRenderer";
    if (std::is_same<T, fuchsia::media::AudioCapturerPtr>())
      return "AudioCapturer";
  }
};
TYPED_TEST_SUITE(GainControlTest, GainControlTestTypes, GainControlTypeNames);

// SetGainWithRamp is not implemented for GainControl interfaces created from AudioCapturer
class GainControlRampTest : public GainControlTest<fuchsia::media::AudioRendererPtr> {};

}  // namespace media::audio::test

#endif  // SRC_MEDIA_AUDIO_AUDIO_CORE_TEST_API_GAIN_CONTROL_TEST_SHARED_H_

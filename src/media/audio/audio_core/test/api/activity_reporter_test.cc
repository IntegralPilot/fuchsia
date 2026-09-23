// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/media/cpp/fidl.h>

#include <gmock/gmock.h>

#include "src/media/audio/audio_core/stream_usage.h"
#include "src/media/audio/audio_core/testing/integration/hermetic_audio_test.h"

using fuchsia::media::AudioCaptureUsage;
using fuchsia::media::AudioCaptureUsage2;
using fuchsia::media::AudioRenderUsage;
using fuchsia::media::AudioRenderUsage2;
using fuchsia::media::AudioSampleFormat;
using testing::UnorderedElementsAre;
using testing::UnorderedElementsAreArray;

namespace media::audio::test {

class ActivityReporterTest : public HermeticAudioTest {
 protected:
  void SetUp() override {
    HermeticAudioTest::SetUp();

    realm().Connect(activity_reporter().NewRequest());
    AddErrorHandler(activity_reporter(), "ActivityReporter");
  }

  AudioRendererShim<AudioSampleFormat::SIGNED_16>* CreateAndPlayWithUsage(AudioRenderUsage2 usage) {
    auto format = Format::Create<AudioSampleFormat::SIGNED_16>(1, 8000).value();  // arbitrary
    auto r = CreateAudioRenderer(format, 1024, usage);
    r->fidl()->PlayNoReply(0, 0);
    return r;
  }

  fuchsia::media::ActivityReporterPtr& activity_reporter() { return activity_reporter_; }

 private:
  fuchsia::media::ActivityReporterPtr activity_reporter_;
};

// Test that the user is connected to the activity reporter.
TEST_F(ActivityReporterTest, InitialWatchCaptureActivity) {
  bool received_callback;
  auto add_callback = [this, &received_callback](const std::string& name) {
    received_callback = false;
    activity_reporter()->WatchCaptureActivity(AddCallbackUnordered(
        name, [&received_callback](const auto& usages) { received_callback = true; }));
  };

  // First call should return immediately.
  add_callback("WatchCaptureActivity InitialCall");
  ExpectCallbacks();
  ASSERT_TRUE(received_callback);
}

// Test that the user is connected to the activity reporter.
TEST_F(ActivityReporterTest, AddAndRemoveRenderActivity) {
  bool received_callback;
  std::vector<AudioRenderUsage> active_usages;
  auto add_callback = [this, &received_callback, &active_usages](const std::string& name) {
    received_callback = false;
    active_usages.clear();
    activity_reporter()->WatchRenderActivity(AddCallback(
        name, [&received_callback, &active_usages](std::vector<AudioRenderUsage> usages) {
          received_callback = true;
          active_usages = std::move(usages);
        }));
  };

  // First call should return immediately, others should wait for updates.
  add_callback("WatchRenderActivity InitialCall");
  ExpectCallbacks();
  EXPECT_TRUE(received_callback);
  EXPECT_TRUE(active_usages.empty());

  auto a11y_rend = CreateAndPlayWithUsage(AudioRenderUsage2::ACCESSIBILITY);
  auto bkgd_rend = CreateAndPlayWithUsage(AudioRenderUsage2::BACKGROUND);
  add_callback("WatchRenderActivity AfterPlayBackground");
  ExpectCallbacks();
  EXPECT_THAT(active_usages, UnorderedElementsAreArray({AudioRenderUsage::BACKGROUND}));

  auto media_rend = CreateAndPlayWithUsage(AudioRenderUsage2::MEDIA);
  add_callback("WatchRenderActivity AfterPlayMedia");
  ExpectCallbacks();
  EXPECT_THAT(active_usages,
              UnorderedElementsAreArray({AudioRenderUsage::BACKGROUND, AudioRenderUsage::MEDIA}));

  bkgd_rend->fidl()->PauseNoReply();
  add_callback("WatchRenderActivity AfterPauseBackground");
  ExpectCallbacks();
  EXPECT_THAT(active_usages, UnorderedElementsAreArray({AudioRenderUsage::MEDIA}));

  add_callback("WatchRenderActivity AfterDisconnectMedia");
  Unbind(media_rend);
  ExpectCallbacks();
  EXPECT_TRUE(active_usages.empty());

  Unbind(a11y_rend);
  Unbind(bkgd_rend);
}

// Get the above working, including NOT seeing ACCESSIBILITY, then move it here as-is.
TEST_F(ActivityReporterTest, AddAndRemoveRenderActivity2) {
  bool received_callback;
  std::vector<AudioRenderUsage2> active_usages2;
  auto add_callback2 = [this, &received_callback, &active_usages2](const std::string& name) {
    received_callback = false;
    active_usages2.clear();
    activity_reporter()->WatchRenderActivity2(AddCallbackUnordered(
        name, [&received_callback, &active_usages2](
                  fuchsia::media::ActivityReporter_WatchRenderActivity2_Result result) {
          ASSERT_TRUE(result.is_response()) << "WatchRenderActivity2 returned kFrameworkErr";
          received_callback = true;
          active_usages2 = std::move(result.response().active_usages);
        }));
  };

  add_callback2("WatchRenderActivity2 InitialCall");
  ExpectCallbacks();
  EXPECT_TRUE(received_callback);
  EXPECT_TRUE(active_usages2.empty());

  auto a11y_rend = CreateAndPlayWithUsage(AudioRenderUsage2::ACCESSIBILITY);
  add_callback2("WatchRenderActivity2 AfterPlayAccessibility");
  ExpectCallbacks();
  EXPECT_THAT(active_usages2, UnorderedElementsAreArray({AudioRenderUsage2::ACCESSIBILITY}));

  auto bkgd_rend = CreateAndPlayWithUsage(AudioRenderUsage2::BACKGROUND);
  add_callback2("WatchRenderActivity2 AfterPlayBackground");
  ExpectCallbacks();
  EXPECT_THAT(active_usages2, UnorderedElementsAreArray({AudioRenderUsage2::BACKGROUND,
                                                         AudioRenderUsage2::ACCESSIBILITY}));

  auto media_rend = CreateAndPlayWithUsage(AudioRenderUsage2::MEDIA);
  add_callback2("WatchRenderActivity2 AfterPlayMedia");
  ExpectCallbacks();
  EXPECT_THAT(active_usages2,
              UnorderedElementsAreArray({AudioRenderUsage2::BACKGROUND, AudioRenderUsage2::MEDIA,
                                         AudioRenderUsage2::ACCESSIBILITY}));

  bkgd_rend->fidl()->PauseNoReply();
  add_callback2("WatchRenderActivity2 AfterPauseBackground");
  ExpectCallbacks();
  EXPECT_THAT(active_usages2, UnorderedElementsAreArray(
                                  {AudioRenderUsage2::MEDIA, AudioRenderUsage2::ACCESSIBILITY}));

  add_callback2("WatchRenderActivity2 AfterDisconnectMedia");
  Unbind(media_rend);
  ExpectCallbacks();
  EXPECT_THAT(active_usages2, UnorderedElementsAreArray({AudioRenderUsage2::ACCESSIBILITY}));

  Unbind(a11y_rend);
  Unbind(bkgd_rend);
}

// Test that the user can concurrently interact with both legacy and newer activity reporters.
TEST_F(ActivityReporterTest, AddAndRemoveRenderActivityCombined) {
  bool received_callback, received_callback2;
  std::vector<AudioRenderUsage> active_usages;
  std::vector<AudioRenderUsage2> active_usages2;

  auto add_callback = [this, &received_callback, &active_usages](const std::string& name) {
    received_callback = false;
    active_usages.clear();
    activity_reporter()->WatchRenderActivity(AddCallbackUnordered(
        name, [&received_callback, &active_usages](std::vector<AudioRenderUsage> usages) {
          received_callback = true;
          active_usages = std::move(usages);
        }));
  };
  auto add_callback2 = [this, &received_callback2, &active_usages2](const std::string& name) {
    received_callback2 = false;
    active_usages2.clear();
    activity_reporter()->WatchRenderActivity2(AddCallbackUnordered(
        name, [&received_callback2, &active_usages2](
                  fuchsia::media::ActivityReporter_WatchRenderActivity2_Result result) {
          ASSERT_TRUE(result.is_response()) << "WatchRenderActivity2 returned kFrameworkErr";
          received_callback2 = true;
          active_usages2 = std::move(result.response().active_usages);
        }));
  };

  // First call should return immediately, others should wait for updates.
  add_callback("WatchRenderActivity InitialCall");
  add_callback2("WatchRenderActivity2 InitialCall");
  ExpectCallbacks();
  EXPECT_TRUE(received_callback);
  EXPECT_TRUE(received_callback2);
  EXPECT_TRUE(active_usages.empty());
  EXPECT_TRUE(active_usages2.empty());

  add_callback("WatchRenderActivity AfterPlayInterruption");
  auto inter_rend = CreateAndPlayWithUsage(AudioRenderUsage2::INTERRUPTION);
  add_callback2("WatchRenderActivity2 AfterPlayInterruption");
  ExpectCallbacks();
  EXPECT_THAT(active_usages, UnorderedElementsAreArray({AudioRenderUsage::INTERRUPTION}));
  EXPECT_THAT(active_usages2, UnorderedElementsAreArray({AudioRenderUsage2::INTERRUPTION}));

  auto a11y_rend = CreateAndPlayWithUsage(AudioRenderUsage2::ACCESSIBILITY);
  add_callback2("WatchRenderActivity2 AfterPlayAccessibility");
  ExpectCallbacks();
  EXPECT_THAT(active_usages2, UnorderedElementsAreArray({AudioRenderUsage2::INTERRUPTION,
                                                         AudioRenderUsage2::ACCESSIBILITY}));

  add_callback("WatchRenderActivity AfterPlayAccessibility AfterPlayBackground");
  add_callback2("WatchRenderActivity2 AfterPlayBackground");
  auto bkgd_rend = CreateAndPlayWithUsage(AudioRenderUsage2::BACKGROUND);
  ExpectCallbacks();
  EXPECT_THAT(active_usages, UnorderedElementsAreArray(
                                 {AudioRenderUsage::INTERRUPTION, AudioRenderUsage::BACKGROUND}));
  EXPECT_THAT(active_usages2, UnorderedElementsAreArray({AudioRenderUsage2::INTERRUPTION,
                                                         AudioRenderUsage2::BACKGROUND,
                                                         AudioRenderUsage2::ACCESSIBILITY}));

  a11y_rend->fidl()->PauseNoReply();
  add_callback2("WatchRenderActivity2 AfterPauseAccessibility");
  ExpectCallbacks();
  EXPECT_THAT(active_usages2, UnorderedElementsAreArray({AudioRenderUsage2::INTERRUPTION,
                                                         AudioRenderUsage2::BACKGROUND}));

  add_callback("WatchRenderActivity AfterPauseAccessibility AfterPauseInterruption");
  inter_rend->fidl()->PauseNoReply();
  add_callback2("WatchRenderActivity2 AfterPauseInterruption");
  ExpectCallbacks();
  EXPECT_THAT(active_usages, UnorderedElementsAreArray({AudioRenderUsage::BACKGROUND}));
  EXPECT_THAT(active_usages2, UnorderedElementsAreArray({AudioRenderUsage2::BACKGROUND}));

  Unbind(bkgd_rend);
  add_callback("WatchRenderActivity AfterDisconnectMedia");
  add_callback2("WatchRenderActivity2 AfterDisconnectMedia");
  ExpectCallbacks();
  EXPECT_TRUE(received_callback);
  EXPECT_TRUE(received_callback2);
  EXPECT_TRUE(active_usages.empty());
  EXPECT_TRUE(active_usages2.empty());

  Unbind(a11y_rend);
  Unbind(inter_rend);
}

TEST_F(ActivityReporterTest, DisconnectOnMultipleConcurrentRenderCalls) {
  activity_reporter()->WatchRenderActivity(AddCallback("WatchRenderActivity"));
  ExpectCallbacks();

  activity_reporter()->WatchRenderActivity(
      AddUnexpectedCallback("WatchRenderActivity Unexpected1"));
  activity_reporter()->WatchRenderActivity(
      AddUnexpectedCallback("WatchRenderActivity Unexpected2"));
  ExpectDisconnect(activity_reporter());
}

TEST_F(ActivityReporterTest, DisconnectOnMultipleConcurrentCaptureCalls) {
  activity_reporter()->WatchCaptureActivity(AddCallback("WatchCaptureActivity"));
  ExpectCallbacks();

  activity_reporter()->WatchCaptureActivity(
      AddUnexpectedCallback("WatchCaptureActivity Unexpected1"));
  activity_reporter()->WatchCaptureActivity(
      AddUnexpectedCallback("WatchCaptureActivity Unexpected2"));
  ExpectDisconnect(activity_reporter());
}

TEST_F(ActivityReporterTest, DisconnectOnMultipleConcurrentRender2) {
  activity_reporter()->WatchRenderActivity2(AddCallback("WatchRenderActivity2"));
  ExpectCallbacks();

  activity_reporter()->WatchRenderActivity2(
      AddUnexpectedCallback("WatchRenderActivity2 Unexpected1"));
  activity_reporter()->WatchRenderActivity2(
      AddUnexpectedCallback("WatchRenderActivity2 Unexpected2"));
  ExpectDisconnect(activity_reporter());
}

TEST_F(ActivityReporterTest, DisconnectOnMultipleConcurrentCapture2) {
  activity_reporter()->WatchCaptureActivity2(AddCallback("WatchCaptureActivity2"));
  ExpectCallbacks();

  activity_reporter()->WatchCaptureActivity2(
      AddUnexpectedCallback("WatchCaptureActivity2 Unexpected1"));
  activity_reporter()->WatchCaptureActivity2(
      AddUnexpectedCallback("WatchCaptureActivity2 Unexpected2"));
  ExpectDisconnect(activity_reporter());
}

}  // namespace media::audio::test

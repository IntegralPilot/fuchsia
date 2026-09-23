// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/audio_core/audio_admin.h"

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/media/audio/audio_core/active_stream_count_reporter.h"
#include "src/media/audio/audio_core/stream_usage.h"
#include "src/media/audio/audio_core/stream_volume_manager.h"
#include "src/media/audio/audio_core/testing/null_audio_capturer.h"
#include "src/media/audio/audio_core/testing/null_audio_renderer.h"

namespace media::audio {
namespace {

using CaptureActivity = media::audio::AudioAdmin::ActivityDispatcher::CaptureActivity;
using RenderActivity = media::audio::AudioAdmin::ActivityDispatcher::RenderActivity;
using fuchsia::media::AudioCaptureUsage2;
using fuchsia::media::AudioRenderUsage2;
using fuchsia::media::Usage2;

// Note we purposely use some strange values here to ensure we're not falling back to any default
// or hard-coded logic for values.
constexpr float kMuteGain = -3.0f;
constexpr float kDuckGain = -2.0f;
constexpr float kNoneGain = -1.0f;

constexpr AudioAdmin::BehaviorGain kTestBehaviorGain{
    .none_gain_db = kNoneGain,
    .duck_gain_db = kDuckGain,
    .mute_gain_db = kMuteGain,
};

class MockPolicyActionReporter : public AudioAdmin::PolicyActionReporter {
 public:
  explicit MockPolicyActionReporter(
      fit::function<void(Usage2 usage, fuchsia::media::Behavior policy_action)> receiver)
      : receiver_(std::move(receiver)) {}

  void ReportPolicyAction(Usage2 usage, fuchsia::media::Behavior policy_action) override {
    receiver_(std::move(usage), policy_action);
  }

 private:
  fit::function<void(Usage2 usage, fuchsia::media::Behavior policy_action)> receiver_;
};

class MockActivityDispatcher : public AudioAdmin::ActivityDispatcher {
 public:
  void OnRenderActivityChanged(RenderActivity activity) override {
    last_dispatched_render_activity_ = activity;
  }
  void OnCaptureActivityChanged(CaptureActivity activity) override {
    last_dispatched_capture_activity_ = activity;
  }

  // Access last activity dispatched.
  RenderActivity GetLastRenderActivity() { return last_dispatched_render_activity_; }
  CaptureActivity GetLastCaptureActivity() { return last_dispatched_capture_activity_; }

 private:
  RenderActivity last_dispatched_render_activity_;
  CaptureActivity last_dispatched_capture_activity_;
};

class MockActiveStreamCountReporter : public ActiveStreamCountReporter {
 public:
  MockActiveStreamCountReporter() {
    render_stream_counts_.fill(0);
    capture_stream_counts_.fill(0);
  }

  void OnActiveRenderCountChanged(RenderUsage usage, uint32_t active_count) override {
    auto usage_index = static_cast<std::underlying_type_t<RenderUsage>>(usage);
    render_stream_counts_[usage_index] = active_count;
  }
  void OnActiveCaptureCountChanged(CaptureUsage usage, uint32_t active_count) override {
    auto usage_index = static_cast<std::underlying_type_t<CaptureUsage>>(usage);
    capture_stream_counts_[usage_index] = active_count;
  }

  std::array<uint32_t, kStreamRenderUsageCount>& render_stream_counts() {
    return render_stream_counts_;
  }
  std::array<uint32_t, kStreamCaptureUsageCount>& capture_stream_counts() {
    return capture_stream_counts_;
  }

 private:
  std::array<uint32_t, kStreamRenderUsageCount> render_stream_counts_;
  std::array<uint32_t, kStreamCaptureUsageCount> capture_stream_counts_;
};

class MockStreamVolume : public StreamVolume {
 public:
  explicit MockStreamVolume(AudioRenderUsage2 usage)
      : usage_(Usage2::WithRenderUsage(fidl::Clone(usage))) {}
  explicit MockStreamVolume(AudioCaptureUsage2 usage)
      : usage_(Usage2::WithCaptureUsage(fidl::Clone(usage))) {}

  // |StreamVolume|
  Usage2 GetStreamUsage() const final { return fidl::Clone(usage_); }
  void RealizeVolume(VolumeCommand volume_command) final { ++volume_update_count_; }

  size_t volume_update_count() const { return volume_update_count_; }

 private:
  // Ignore volume update that occurs on renderer/capturer creation.
  size_t volume_update_count_ = -1;
  Usage2 usage_;
};

class AudioAdminTest : public gtest::TestLoopFixture {};

TEST_F(AudioAdminTest, OnlyUpdateVolumeOnPolicyChange) {
  StreamVolumeManager stream_volume_manager(dispatcher());
  MockStreamVolume stream(AudioRenderUsage2::MEDIA);
  stream_volume_manager.AddStream(&stream);

  MockPolicyActionReporter policy_action_reporter([](auto _usage, auto _policy_action) {});
  MockActivityDispatcher mock_activity_dispatcher;
  MockActiveStreamCountReporter mock_active_stream_count_reporter;
  AudioAdmin admin(&stream_volume_manager, &policy_action_reporter, &mock_activity_dispatcher,
                   &mock_active_stream_count_reporter, dispatcher(), kTestBehaviorGain);
  test::NullAudioRenderer r1;
  test::NullAudioCapturer c1;
  test::NullAudioCapturer c2;

  // Media should duck when a Communication stream is active.
  admin.SetInteraction(ToFidlUsage2(CaptureUsage::COMMUNICATION), ToFidlUsage2(RenderUsage::MEDIA),
                       fuchsia::media::Behavior::MUTE);

  // Create active media stream; activation triggers initial policy application (volume update).
  admin.UpdateRendererState(RenderUsage::MEDIA, true, &r1);
  RunLoopUntilIdle();
  EXPECT_EQ(stream.volume_update_count(), 1ul);

  // Create active Communication capturer; media volume should duck.
  admin.UpdateCapturerState(CaptureUsage::COMMUNICATION, true, &c1);
  RunLoopUntilIdle();
  EXPECT_EQ(stream.volume_update_count(), 2ul);

  // Create second active Communication capturer; media volume should remain ducked (no update).
  admin.UpdateCapturerState(CaptureUsage::COMMUNICATION, true, &c2);
  RunLoopUntilIdle();
  EXPECT_EQ(stream.volume_update_count(), 2ul);

  // All Communication streams become inactive; media volume should un-duck.
  admin.UpdateCapturerState(CaptureUsage::COMMUNICATION, false, &c1);
  admin.UpdateCapturerState(CaptureUsage::COMMUNICATION, false, &c2);
  RunLoopUntilIdle();
  EXPECT_EQ(stream.volume_update_count(), 3ul);
}

TEST_F(AudioAdminTest, TwoRenderersWithNoInteractions) {
  MockPolicyActionReporter policy_action_reporter([](auto _usage, auto _policy_action) {});
  MockActivityDispatcher mock_activity_dispatcher;
  StreamVolumeManager stream_volume_manager(dispatcher());
  MockActiveStreamCountReporter mock_active_stream_count_reporter;
  AudioAdmin admin(&stream_volume_manager, &policy_action_reporter, &mock_activity_dispatcher,
                   &mock_active_stream_count_reporter, dispatcher(), kTestBehaviorGain);
  test::NullAudioRenderer r1, r2;

  // Set an inintial stream volume.
  const float kStreamGain = 1.0;
  stream_volume_manager.SetUsageGain(ToFidlUsage2(RenderUsage::MEDIA), kStreamGain);
  stream_volume_manager.SetUsageGain(ToFidlUsage2(RenderUsage::COMMUNICATION), kStreamGain);

  // Start playing a MEDIA stream and check for 'no gain adjustment'.
  admin.UpdateRendererState(RenderUsage::MEDIA, true, &r1);
  RunLoopUntilIdle();
  EXPECT_EQ(kStreamGain + kNoneGain,
            stream_volume_manager.GetUsageGainSettings().GetAdjustedUsageGain(
                ToFidlUsage2(RenderUsage::MEDIA)));

  // Now play a COMMUNICATIONS stream and also check for 'no gain adjustment'.
  admin.UpdateRendererState(RenderUsage::COMMUNICATION, true, &r2);
  RunLoopUntilIdle();
  EXPECT_EQ(kStreamGain + kNoneGain,
            stream_volume_manager.GetUsageGainSettings().GetAdjustedUsageGain(
                ToFidlUsage2(RenderUsage::MEDIA)));
  EXPECT_EQ(kStreamGain + kNoneGain,
            stream_volume_manager.GetUsageGainSettings().GetAdjustedUsageGain(
                ToFidlUsage2(RenderUsage::COMMUNICATION)));
}

TEST_F(AudioAdminTest, TwoRenderersWithDuck) {
  StreamVolumeManager stream_volume_manager(dispatcher());
  MockPolicyActionReporter policy_action_reporter([](auto _usage, auto _policy_action) {});
  MockActivityDispatcher mock_activity_dispatcher;
  MockActiveStreamCountReporter mock_active_stream_count_reporter;
  AudioAdmin admin(&stream_volume_manager, &policy_action_reporter, &mock_activity_dispatcher,
                   &mock_active_stream_count_reporter, dispatcher(), kTestBehaviorGain);
  test::NullAudioRenderer r1, r2;

  // Media should duck when a Communication stream is active.
  admin.SetInteraction(ToFidlUsage2(RenderUsage::COMMUNICATION), ToFidlUsage2(RenderUsage::MEDIA),
                       fuchsia::media::Behavior::DUCK);

  // Set an inintial stream volume.
  const float kStreamGain = 1.0;
  stream_volume_manager.SetUsageGain(ToFidlUsage2(RenderUsage::MEDIA), kStreamGain);
  stream_volume_manager.SetUsageGain(ToFidlUsage2(RenderUsage::COMMUNICATION), kStreamGain);

  // create media active stream.
  admin.UpdateRendererState(RenderUsage::MEDIA, true, &r1);
  RunLoopUntilIdle();
  EXPECT_EQ(kStreamGain + kNoneGain,
            stream_volume_manager.GetUsageGainSettings().GetAdjustedUsageGain(
                ToFidlUsage2(RenderUsage::MEDIA)));

  // communication renderer becomes active; media should duck.
  admin.UpdateRendererState(RenderUsage::COMMUNICATION, true, &r2);
  RunLoopUntilIdle();
  EXPECT_EQ(kStreamGain + kDuckGain,
            stream_volume_manager.GetUsageGainSettings().GetAdjustedUsageGain(
                ToFidlUsage2(RenderUsage::MEDIA)));
  EXPECT_EQ(kStreamGain + kNoneGain,
            stream_volume_manager.GetUsageGainSettings().GetAdjustedUsageGain(
                ToFidlUsage2(RenderUsage::COMMUNICATION)));

  // All Communication streams become inactive; ducking should stop.
  admin.UpdateRendererState(RenderUsage::COMMUNICATION, false, &r2);
  RunLoopUntilIdle();
  EXPECT_EQ(kStreamGain + kNoneGain,
            stream_volume_manager.GetUsageGainSettings().GetAdjustedUsageGain(
                ToFidlUsage2(RenderUsage::MEDIA)));
  EXPECT_EQ(kStreamGain + kNoneGain,
            stream_volume_manager.GetUsageGainSettings().GetAdjustedUsageGain(
                ToFidlUsage2(RenderUsage::COMMUNICATION)));
}

TEST_F(AudioAdminTest, CapturerDucksRenderer) {
  StreamVolumeManager stream_volume_manager(dispatcher());
  MockPolicyActionReporter policy_action_reporter([](auto _usage, auto _policy_action) {});
  MockActivityDispatcher mock_activity_dispatcher;
  MockActiveStreamCountReporter mock_active_stream_count_reporter;
  AudioAdmin admin(&stream_volume_manager, &policy_action_reporter, &mock_activity_dispatcher,
                   &mock_active_stream_count_reporter, dispatcher(), kTestBehaviorGain);
  test::NullAudioRenderer r1;
  test::NullAudioCapturer c1;

  // Set an inintial stream volume.
  const float kStreamGain = 1.0;
  stream_volume_manager.SetUsageGain(ToFidlUsage2(RenderUsage::MEDIA), kStreamGain);
  stream_volume_manager.SetUsageGain(ToFidlUsage2(CaptureUsage::COMMUNICATION), kStreamGain);

  // Media should duck when a Communication stream is active.
  admin.SetInteraction(ToFidlUsage2(CaptureUsage::COMMUNICATION), ToFidlUsage2(RenderUsage::MEDIA),
                       fuchsia::media::Behavior::DUCK);

  // Create active media stream.
  admin.UpdateRendererState(RenderUsage::MEDIA, true, &r1);
  RunLoopUntilIdle();
  EXPECT_EQ(kStreamGain + kNoneGain,
            stream_volume_manager.GetUsageGainSettings().GetAdjustedUsageGain(
                ToFidlUsage2(RenderUsage::MEDIA)));

  // Create active Communication capturer; media output should duck.
  admin.UpdateCapturerState(CaptureUsage::COMMUNICATION, true, &c1);
  RunLoopUntilIdle();
  EXPECT_EQ(kStreamGain + kDuckGain,
            stream_volume_manager.GetUsageGainSettings().GetAdjustedUsageGain(
                ToFidlUsage2(RenderUsage::MEDIA)));
  EXPECT_EQ(kStreamGain + kNoneGain,
            stream_volume_manager.GetUsageGainSettings().GetAdjustedUsageGain(
                ToFidlUsage2(CaptureUsage::COMMUNICATION)));

  // Comm becomes inactive; ducking should stop.
  admin.UpdateCapturerState(CaptureUsage::COMMUNICATION, false, &c1);
  RunLoopUntilIdle();
  EXPECT_EQ(kStreamGain + kNoneGain,
            stream_volume_manager.GetUsageGainSettings().GetAdjustedUsageGain(
                ToFidlUsage2(RenderUsage::MEDIA)));
  EXPECT_EQ(kStreamGain + kNoneGain,
            stream_volume_manager.GetUsageGainSettings().GetAdjustedUsageGain(
                ToFidlUsage2(CaptureUsage::COMMUNICATION)));
}

TEST_F(AudioAdminTest, RendererDucksCapturer) {
  StreamVolumeManager stream_volume_manager(dispatcher());
  MockPolicyActionReporter policy_action_reporter([](auto _usage, auto _policy_action) {});
  MockActivityDispatcher mock_activity_dispatcher;
  MockActiveStreamCountReporter mock_active_stream_count_reporter;
  AudioAdmin admin(&stream_volume_manager, &policy_action_reporter, &mock_activity_dispatcher,
                   &mock_active_stream_count_reporter, dispatcher(), kTestBehaviorGain);
  test::NullAudioRenderer r1;
  test::NullAudioCapturer c1;

  const float kStreamGain = 1.0;
  stream_volume_manager.SetUsageGain(ToFidlUsage2(RenderUsage::COMMUNICATION), kStreamGain);
  stream_volume_manager.SetUsageGain(ToFidlUsage2(CaptureUsage::FOREGROUND), kStreamGain);

  // Foreground capturer should duck when communication renderers are active.
  admin.SetInteraction(ToFidlUsage2(RenderUsage::COMMUNICATION),
                       ToFidlUsage2(CaptureUsage::FOREGROUND), fuchsia::media::Behavior::DUCK);

  // Create active capturer stream.
  admin.UpdateCapturerState(CaptureUsage::FOREGROUND, true, &c1);
  RunLoopUntilIdle();
  EXPECT_EQ(kStreamGain + kNoneGain,
            stream_volume_manager.GetUsageGainSettings().GetAdjustedUsageGain(
                ToFidlUsage2(CaptureUsage::FOREGROUND)));

  // Create active Communication renderer; foreground capturer should duck.
  admin.UpdateRendererState(RenderUsage::COMMUNICATION, true, &r1);
  RunLoopUntilIdle();
  EXPECT_EQ(kStreamGain + kDuckGain,
            stream_volume_manager.GetUsageGainSettings().GetAdjustedUsageGain(
                ToFidlUsage2(CaptureUsage::FOREGROUND)));
  EXPECT_EQ(kStreamGain + kNoneGain,
            stream_volume_manager.GetUsageGainSettings().GetAdjustedUsageGain(
                ToFidlUsage2(RenderUsage::COMMUNICATION)));

  // Comm becomes inactive; ducking should stop.
  admin.UpdateRendererState(RenderUsage::COMMUNICATION, false, &r1);
  RunLoopUntilIdle();
  EXPECT_EQ(kStreamGain + kNoneGain,
            stream_volume_manager.GetUsageGainSettings().GetAdjustedUsageGain(
                ToFidlUsage2(CaptureUsage::FOREGROUND)));
  EXPECT_EQ(kStreamGain + kNoneGain,
            stream_volume_manager.GetUsageGainSettings().GetAdjustedUsageGain(
                ToFidlUsage2(RenderUsage::COMMUNICATION)));
}

TEST_F(AudioAdminTest, PolicyActionsReported) {
  auto test_policy_action = [this](auto expected_action) {
    const auto target_usage = ToFidlUsage2(CaptureUsage::FOREGROUND);
    fuchsia::media::Behavior policy_action_taken;
    // Record any actions taken on our target_usage (AudioCaptureUsage2::FOREGROUND)
    MockPolicyActionReporter policy_action_reporter(
        [&policy_action_taken, &target_usage](auto usage, auto action) {
          if (fidl::Equals(usage, target_usage)) {
            policy_action_taken = action;
          }
        });

    StreamVolumeManager stream_volume_manager(dispatcher());
    MockActivityDispatcher mock_activity_dispatcher;
    MockActiveStreamCountReporter mock_active_stream_count_reporter;
    AudioAdmin admin(&stream_volume_manager, &policy_action_reporter, &mock_activity_dispatcher,
                     &mock_active_stream_count_reporter, dispatcher(), kTestBehaviorGain);
    test::NullAudioRenderer r1;
    test::NullAudioCapturer c1;

    const float kStreamGain = 1.0;
    stream_volume_manager.SetUsageGain(ToFidlUsage2(RenderUsage::COMMUNICATION), kStreamGain);
    stream_volume_manager.SetUsageGain(ToFidlUsage2(CaptureUsage::FOREGROUND), kStreamGain);

    // Foreground capturer should duck when communication renderers are active.
    admin.SetInteraction(ToFidlUsage2(RenderUsage::COMMUNICATION),
                         ToFidlUsage2(CaptureUsage::FOREGROUND), expected_action);

    // Create active capturer stream.
    admin.UpdateCapturerState(CaptureUsage::FOREGROUND, true, &c1);
    // Create active Communication renderer; foreground capturer should receive policy action.
    admin.UpdateRendererState(RenderUsage::COMMUNICATION, true, &r1);
    RunLoopUntilIdle();
    EXPECT_EQ(policy_action_taken, expected_action);

    // Comm becomes inactive; action should stop.
    admin.UpdateRendererState(RenderUsage::COMMUNICATION, false, &r1);
    RunLoopUntilIdle();
    EXPECT_EQ(policy_action_taken, fuchsia::media::Behavior::NONE);
  };

  test_policy_action(fuchsia::media::Behavior::DUCK);
  test_policy_action(fuchsia::media::Behavior::MUTE);
}

TEST_F(AudioAdminTest, RenderActivityDispatched) {
  // Test that a change of usage given an initial activity is correctly dispatched.
  auto test_dispatch_action = [this](RenderActivity initial_activity, RenderUsage changed_usage) {
    StreamVolumeManager stream_volume_manager(dispatcher());
    MockPolicyActionReporter policy_action_reporter([](auto _usage, auto _policy_action) {});
    MockActivityDispatcher mock_activity_dispatcher;
    MockActiveStreamCountReporter mock_active_stream_count_reporter;
    AudioAdmin admin(&stream_volume_manager, &policy_action_reporter, &mock_activity_dispatcher,
                     &mock_active_stream_count_reporter, dispatcher(), kTestBehaviorGain);

    // Trigger the initial activity by registering AudioRenderers.
    std::array<test::NullAudioRenderer, fuchsia::media::RENDER_USAGE2_COUNT> rs;
    for (int i = 0; i < fuchsia::media::RENDER_USAGE2_COUNT; i++) {
      if (initial_activity[i]) {
        admin.UpdateRendererState(static_cast<RenderUsage>(i), true, &rs[i]);
      }
    }

    RunLoopUntilIdle();
    EXPECT_EQ(initial_activity, mock_activity_dispatcher.GetLastRenderActivity());

    int changed_usage_index = static_cast<int>(changed_usage);
    RenderActivity final_activity = initial_activity;
    final_activity.flip(changed_usage_index);

    // Modify the initial activity to reflect the changed usage.
    admin.UpdateRendererState(changed_usage, final_activity[changed_usage_index],
                              &rs[changed_usage_index]);

    RunLoopUntilIdle();
    EXPECT_EQ(final_activity, mock_activity_dispatcher.GetLastRenderActivity());
  };

  // Check all of the possible state transitions from each possible activity.
  int possible_activities_count =
      static_cast<int>(std::pow(2, fuchsia::media::RENDER_USAGE2_COUNT));
  for (int i = 0; i < possible_activities_count; i++) {
    for (int j = 0; j < fuchsia::media::RENDER_USAGE2_COUNT; j++) {
      auto initial_activity = static_cast<RenderActivity>(i);
      auto changed_usage = static_cast<RenderUsage>(j);
      test_dispatch_action(initial_activity, changed_usage);
    }
  }
}

TEST_F(AudioAdminTest, CaptureActivityDispatched) {
  // Test that a change of usage given an initial activity is correctly dispatched.
  auto test_dispatch_action = [this](CaptureActivity initial_activity, CaptureUsage changed_usage) {
    StreamVolumeManager stream_volume_manager(dispatcher());
    MockPolicyActionReporter policy_action_reporter([](auto _usage, auto _policy_action) {});
    MockActivityDispatcher mock_activity_dispatcher;
    MockActiveStreamCountReporter mock_active_stream_count_reporter;
    AudioAdmin admin(&stream_volume_manager, &policy_action_reporter, &mock_activity_dispatcher,
                     &mock_active_stream_count_reporter, dispatcher(), kTestBehaviorGain);

    // Trigger the initial activity by registering AudioCapturers.
    // ActivityReporter covers the FIDL usages, so we test only those
    std::array<test::NullAudioCapturer, fuchsia::media::CAPTURE_USAGE2_COUNT> rs;
    for (int i = 0; i < fuchsia::media::CAPTURE_USAGE2_COUNT; i++) {
      if (initial_activity[i]) {
        admin.UpdateCapturerState(static_cast<CaptureUsage>(i), true, &rs[i]);
      }
    }

    RunLoopUntilIdle();
    EXPECT_EQ(initial_activity, mock_activity_dispatcher.GetLastCaptureActivity());

    int changed_usage_index = static_cast<int>(changed_usage);
    CaptureActivity final_activity = initial_activity;
    final_activity.flip(changed_usage_index);

    // Modify the initial activity to reflect the changed usage.
    admin.UpdateCapturerState(changed_usage, final_activity[changed_usage_index],
                              &rs[changed_usage_index]);

    RunLoopUntilIdle();
    EXPECT_EQ(final_activity, mock_activity_dispatcher.GetLastCaptureActivity());
  };

  // Check all of the possible state transitions from each possible activity.
  int possible_activities_count =
      static_cast<int>(std::pow(2, fuchsia::media::CAPTURE_USAGE2_COUNT));
  for (int i = 0; i < possible_activities_count; i++) {
    for (int j = 0; j < fuchsia::media::CAPTURE_USAGE2_COUNT; j++) {
      auto initial_activity = static_cast<CaptureActivity>(i);
      auto changed_usage = static_cast<CaptureUsage>(j);
      test_dispatch_action(initial_activity, changed_usage);
    }
  }
}

// Test to verify that Mute overrides Duck, and both override None.
TEST_F(AudioAdminTest, PriorityActionsApplied) {
  StreamVolumeManager stream_volume_manager(dispatcher());
  MockPolicyActionReporter policy_action_reporter([](auto _usage, auto _policy_action) {});
  MockActivityDispatcher mock_activity_dispatcher;
  MockActiveStreamCountReporter mock_active_stream_count_reporter;
  AudioAdmin admin(&stream_volume_manager, &policy_action_reporter, &mock_activity_dispatcher,
                   &mock_active_stream_count_reporter, dispatcher(), kTestBehaviorGain);
  test::NullAudioRenderer r1, r2, r3;
  test::NullAudioCapturer c1;

  // Interruption should duck when SystemAgent(render) is active.
  admin.SetInteraction(ToFidlUsage2(RenderUsage::SYSTEM_AGENT),
                       ToFidlUsage2(RenderUsage::INTERRUPTION), fuchsia::media::Behavior::DUCK);

  // Communication(render) should duck when SystemAgent(render) is active.
  admin.SetInteraction(ToFidlUsage2(RenderUsage::SYSTEM_AGENT),
                       ToFidlUsage2(RenderUsage::COMMUNICATION), fuchsia::media::Behavior::DUCK);

  // Communication(render) should mute when SystemAgent(capture) is active.
  admin.SetInteraction(ToFidlUsage2(CaptureUsage::SYSTEM_AGENT),
                       ToFidlUsage2(RenderUsage::COMMUNICATION), fuchsia::media::Behavior::MUTE);

  // Set an initial stream volume.
  const float kStreamGain = 1.0;
  stream_volume_manager.SetUsageGain(ToFidlUsage2(RenderUsage::INTERRUPTION), kStreamGain);
  stream_volume_manager.SetUsageGain(ToFidlUsage2(RenderUsage::COMMUNICATION), kStreamGain);

  // Create Interruption active stream.
  admin.UpdateRendererState(RenderUsage::INTERRUPTION, true, &r1);
  RunLoopUntilIdle();
  EXPECT_EQ(kStreamGain + kNoneGain,
            stream_volume_manager.GetUsageGainSettings().GetAdjustedUsageGain(
                ToFidlUsage2(RenderUsage::INTERRUPTION)));

  // Create Communication active stream.
  admin.UpdateRendererState(RenderUsage::COMMUNICATION, true, &r2);
  RunLoopUntilIdle();
  EXPECT_EQ(kStreamGain + kNoneGain,
            stream_volume_manager.GetUsageGainSettings().GetAdjustedUsageGain(
                ToFidlUsage2(RenderUsage::COMMUNICATION)));

  // SystemAgent capturer becomes active; Interruption should not change, Communication should mute.
  admin.UpdateCapturerState(CaptureUsage::SYSTEM_AGENT, true, &c1);
  RunLoopUntilIdle();
  EXPECT_EQ(kStreamGain + kNoneGain,
            stream_volume_manager.GetUsageGainSettings().GetAdjustedUsageGain(
                ToFidlUsage2(RenderUsage::INTERRUPTION)));
  EXPECT_EQ(kStreamGain + kMuteGain,
            stream_volume_manager.GetUsageGainSettings().GetAdjustedUsageGain(
                ToFidlUsage2(RenderUsage::COMMUNICATION)));

  // SystemAgent renderer becomes active; Interruption should duck, Communication should remain
  // muted.
  admin.UpdateRendererState(RenderUsage::SYSTEM_AGENT, true, &r3);
  RunLoopUntilIdle();
  EXPECT_EQ(kStreamGain + kDuckGain,
            stream_volume_manager.GetUsageGainSettings().GetAdjustedUsageGain(
                ToFidlUsage2(RenderUsage::INTERRUPTION)));
  EXPECT_EQ(kStreamGain + kMuteGain,
            stream_volume_manager.GetUsageGainSettings().GetAdjustedUsageGain(
                ToFidlUsage2(RenderUsage::COMMUNICATION)));
}

class ActiveStreamCountReporterTest : public AudioAdminTest {
 protected:
  void SetUp() override {
    expected_render_counts_.fill(0);
    expected_capture_counts_.fill(0);
  }

  void ValidateActiveRenderStreamCounts(
      std::array<uint32_t, kStreamRenderUsageCount>& expected_counts) {
    auto render_counts = mock_active_stream_count_reporter_.render_stream_counts();
    for (auto usage_index = 0u; usage_index < kStreamRenderUsageCount; ++usage_index) {
      EXPECT_EQ(render_counts[usage_index], expected_counts[usage_index])
          << "Comparison failed for " << ToString(kRenderUsages[usage_index]);
    }
  }

  void ValidateActiveCaptureStreamCounts(
      std::array<uint32_t, kStreamCaptureUsageCount>& expected_counts) {
    auto capture_counts = mock_active_stream_count_reporter_.capture_stream_counts();
    for (auto usage_index = 0u; usage_index < kStreamCaptureUsageCount; ++usage_index) {
      EXPECT_EQ(capture_counts[usage_index], expected_counts[usage_index])
          << "Comparison failed for " << ToString(kCaptureUsages[usage_index]);
    }
  }

  void UpdateExpectedCountsAndVerify(StreamUsage usage, int32_t change_in_count) {
    if (usage.is_render_usage()) {
      auto index = static_cast<std::underlying_type_t<RenderUsage>>(usage.render_usage());
      ASSERT_GE(static_cast<int64_t>(expected_render_counts_[index]) + change_in_count, 0ll)
          << "Usage count cannot be negative; test logic error";
      expected_render_counts_[index] += change_in_count;
    } else {
      auto index = static_cast<std::underlying_type_t<CaptureUsage>>(usage.capture_usage());
      ASSERT_GE(static_cast<int64_t>(expected_capture_counts_[index]) + change_in_count, 0ll)
          << "Usage count cannot be negative; test logic error";
      expected_capture_counts_[index] += change_in_count;
    }

    RunLoopUntilIdle();
    ValidateActiveRenderStreamCounts(expected_render_counts_);
    ValidateActiveCaptureStreamCounts(expected_capture_counts_);
  }

  MockActiveStreamCountReporter mock_active_stream_count_reporter_;

  std::array<uint32_t, kStreamRenderUsageCount> expected_render_counts_;
  std::array<uint32_t, kStreamCaptureUsageCount> expected_capture_counts_;
};

// Test to verify the ActiveStreamCountReporter interface
TEST_F(ActiveStreamCountReporterTest, ConcurrentCounts) {
  StreamVolumeManager stream_volume_manager(dispatcher());
  MockPolicyActionReporter policy_action_reporter([](auto _usage, auto _policy_action) {});
  MockActivityDispatcher mock_activity_dispatcher;
  AudioAdmin admin(&stream_volume_manager, &policy_action_reporter, &mock_activity_dispatcher,
                   &mock_active_stream_count_reporter_, dispatcher(), kTestBehaviorGain);

  test::NullAudioRenderer r1, r2, r3, r4;
  test::NullAudioCapturer c1, c2, c3, c4;

  // Add a number of renderers and capturers, verifying active stream counts.
  //
  // Interruption renderer becomes active.
  admin.UpdateRendererState(RenderUsage::INTERRUPTION, true, &r1);
  UpdateExpectedCountsAndVerify(StreamUsage::WithRenderUsage(RenderUsage::INTERRUPTION), 1);

  // SystemAgent capturer becomes active.
  admin.UpdateCapturerState(CaptureUsage::SYSTEM_AGENT, true, &c1);
  UpdateExpectedCountsAndVerify(StreamUsage::WithCaptureUsage(CaptureUsage::SYSTEM_AGENT), 1);

  // Ultrasound renderer becomes active.
  admin.UpdateRendererState(RenderUsage::ULTRASOUND, true, &r2);
  UpdateExpectedCountsAndVerify(StreamUsage::WithRenderUsage(RenderUsage::ULTRASOUND), 1);

  // Foreground capturer becomes active.
  admin.UpdateCapturerState(CaptureUsage::FOREGROUND, true, &c2);
  UpdateExpectedCountsAndVerify(StreamUsage::WithCaptureUsage(CaptureUsage::FOREGROUND), 1);

  // Interruption renderer becomes active.
  admin.UpdateRendererState(RenderUsage::INTERRUPTION, true, &r3);
  UpdateExpectedCountsAndVerify(StreamUsage::WithRenderUsage(RenderUsage::INTERRUPTION), 1);

  // Loopback capturer becomes active.
  admin.UpdateCapturerState(CaptureUsage::LOOPBACK, true, &c3);
  UpdateExpectedCountsAndVerify(StreamUsage::WithCaptureUsage(CaptureUsage::LOOPBACK), 1);

  // Media renderer becomes active.
  admin.UpdateRendererState(RenderUsage::MEDIA, true, &r4);
  UpdateExpectedCountsAndVerify(StreamUsage::WithRenderUsage(RenderUsage::MEDIA), 1);

  // Communication capturer becomes active.
  admin.UpdateCapturerState(CaptureUsage::COMMUNICATION, true, &c4);
  UpdateExpectedCountsAndVerify(StreamUsage::WithCaptureUsage(CaptureUsage::COMMUNICATION), 1);

  // Now unwind those same renderers and capturers, verifying active stream counts.
  //
  // SystemAgent capturer becomes inactive.
  admin.UpdateCapturerState(CaptureUsage::SYSTEM_AGENT, false, &c1);
  UpdateExpectedCountsAndVerify(StreamUsage::WithCaptureUsage(CaptureUsage::SYSTEM_AGENT), -1);

  // Both Interruption renderers become inactive.
  admin.UpdateRendererState(RenderUsage::INTERRUPTION, false, &r1);
  admin.UpdateRendererState(RenderUsage::INTERRUPTION, false, &r3);
  UpdateExpectedCountsAndVerify(StreamUsage::WithRenderUsage(RenderUsage::INTERRUPTION), -2);

  // Foreground capturer becomes inactive.
  admin.UpdateCapturerState(CaptureUsage::FOREGROUND, false, &c2);
  UpdateExpectedCountsAndVerify(StreamUsage::WithCaptureUsage(CaptureUsage::FOREGROUND), -1);

  // Ultrasound renderer becomes inactive.
  admin.UpdateRendererState(RenderUsage::ULTRASOUND, false, &r2);
  UpdateExpectedCountsAndVerify(StreamUsage::WithRenderUsage(RenderUsage::ULTRASOUND), -1);

  // Loopback capturer becomes inactive.
  admin.UpdateCapturerState(CaptureUsage::LOOPBACK, false, &c3);
  UpdateExpectedCountsAndVerify(StreamUsage::WithCaptureUsage(CaptureUsage::LOOPBACK), -1);

  // Media renderer becomes inactive.
  admin.UpdateRendererState(RenderUsage::MEDIA, false, &r4);
  UpdateExpectedCountsAndVerify(StreamUsage::WithRenderUsage(RenderUsage::MEDIA), -1);

  // Communication capturer becomes inactive.
  admin.UpdateCapturerState(CaptureUsage::COMMUNICATION, false, &c4);
  UpdateExpectedCountsAndVerify(StreamUsage::WithCaptureUsage(CaptureUsage::COMMUNICATION), -1);
}

}  // namespace
}  // namespace media::audio

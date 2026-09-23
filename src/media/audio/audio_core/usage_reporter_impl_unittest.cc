// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/audio_core/usage_reporter_impl.h"

#include <lib/fidl/cpp/binding.h>

#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"

namespace media::audio {

using fuchsia::media::AudioRenderUsage2;

const auto kMediaUsage = ToFidlUsage2(RenderUsage::MEDIA);
const auto kMutedState = fuchsia::media::UsageState::WithMuted({});
const auto kUnadjustedState = fuchsia::media::UsageState::WithUnadjusted({});
const auto kActivateCallback = true;
const auto kDeactivateCallback = false;

class FakeUsageWatcher : public fuchsia::media::UsageWatcher {
 public:
  explicit FakeUsageWatcher(bool activate_callback) : activate_callback_(activate_callback) {}

  const fuchsia::media::Usage2& last_usage() const { return last_usage_; }

  const fuchsia::media::UsageState& last_usage_state() const { return last_usage_state_; }

 private:
  void OnStateChanged(fuchsia::media::Usage usage, fuchsia::media::UsageState usage_state,
                      OnStateChangedCallback callback) override {
    last_usage_ = ToFidlUsage2(usage);
    last_usage_state_ = std::move(usage_state);

    if (activate_callback_) {
      callback();
    }
  }

  bool activate_callback_;
  fuchsia::media::Usage2 last_usage_;
  fuchsia::media::UsageState last_usage_state_;
};

class FakeUsageWatcher2 : public fuchsia::media::UsageWatcher2 {
 public:
  explicit FakeUsageWatcher2(bool activate_callback) : activate_callback_(activate_callback) {}

  const fuchsia::media::Usage2& last_usage() const { return last_usage_; }

  const fuchsia::media::UsageState& last_usage_state() const { return last_usage_state_; }

 private:
  void OnStateChanged(fuchsia::media::Usage2 usage, fuchsia::media::UsageState usage_state,
                      OnStateChangedCallback callback) override {
    FX_LOGS(INFO) << __func__ << "(usage " << usage << ", state " << usage_state << ")";
    last_usage_ = std::move(usage);
    last_usage_state_ = std::move(usage_state);

    if (activate_callback_) {
      callback();
    }
  }

  bool activate_callback_;
  fuchsia::media::Usage2 last_usage_;
  fuchsia::media::UsageState last_usage_state_;
};

class UsageReporterImplTest : public gtest::TestLoopFixture {
 protected:
  static constexpr int kMaxStates = UsageReporterImpl::MAX_STATES;

  fidl::Binding<fuchsia::media::UsageWatcher, std::unique_ptr<FakeUsageWatcher>> Watch(
      fuchsia::media::Usage2 usage, bool activate_callback) {
    fidl::InterfaceHandle<fuchsia::media::UsageWatcher> state_watcher_handle;
    auto request = state_watcher_handle.NewRequest();
    auto usage1 = ToFidlUsageTry(usage);
    if (usage1.has_value()) {
      usage_reporter_->Watch(std::move(usage1.value()), std::move(state_watcher_handle));
    }
    return fidl::Binding(std::make_unique<FakeUsageWatcher>(activate_callback), std::move(request));
  }
  fidl::Binding<fuchsia::media::UsageWatcher2, std::unique_ptr<FakeUsageWatcher2>> Watch2(
      fuchsia::media::Usage2 usage, bool activate_callback) {
    fidl::InterfaceHandle<fuchsia::media::UsageWatcher2> state_watcher_handle;
    auto request = state_watcher_handle.NewRequest();
    usage_reporter_->Watch2(std::move(usage), std::move(state_watcher_handle));
    return fidl::Binding(std::make_unique<FakeUsageWatcher2>(activate_callback),
                         std::move(request));
  }

  bool AcksComplete(const fuchsia::media::Usage2& usage) {
    auto& set = usage_reporter_impl_.watcher_set(usage);
    for (auto& watcher : set.watchers) {
      if (watcher.second.outstanding_ack_count != 0) {
        return false;
      }
    }
    auto& set2 = usage_reporter_impl_.watcher_set2(usage);
    for (auto& watcher2 : set2.watchers) {
      if (watcher2.second.outstanding_ack_count != 0) {
        return false;
      }
    }
    return true;
  }

  bool WatchersDisconnected(const fuchsia::media::Usage2& usage) {
    auto& set = usage_reporter_impl_.watcher_set(usage);
    auto& set2 = usage_reporter_impl_.watcher_set2(usage);
    return set.watchers.empty() && set2.watchers.empty();
  }

  UsageReporterImpl usage_reporter_impl_;
  AudioAdmin::PolicyActionReporter* policy_action_reporter_ = &usage_reporter_impl_;
  fuchsia::media::UsageReporter* usage_reporter_ = &usage_reporter_impl_;
};

TEST_F(UsageReporterImplTest, StateIsEmittedToWatcher) {
  auto watcher = Watch(fidl::Clone(kMediaUsage), kActivateCallback);

  policy_action_reporter_->ReportPolicyAction(fidl::Clone(kMediaUsage),
                                              fuchsia::media::Behavior::MUTE);

  RunLoopUntilIdle();
  EXPECT_TRUE(fidl::Equals(watcher.impl()->last_usage(), kMediaUsage));
  EXPECT_TRUE(fidl::Equals(watcher.impl()->last_usage_state(), kMutedState));
  EXPECT_TRUE(AcksComplete(kMediaUsage));
}

TEST_F(UsageReporterImplTest, StateIsEmittedToWatcher2) {
  auto watcher2 = Watch2(fidl::Clone(kMediaUsage), kActivateCallback);

  policy_action_reporter_->ReportPolicyAction(fidl::Clone(kMediaUsage),
                                              fuchsia::media::Behavior::MUTE);

  RunLoopUntilIdle();
  EXPECT_TRUE(fidl::Equals(watcher2.impl()->last_usage(), kMediaUsage));
  EXPECT_TRUE(fidl::Equals(watcher2.impl()->last_usage_state(), kMutedState));
  EXPECT_TRUE(AcksComplete(kMediaUsage));
}

TEST_F(UsageReporterImplTest, StatesAreEmittedToWatcher) {
  auto watcher = Watch(fidl::Clone(kMediaUsage), kActivateCallback);

  policy_action_reporter_->ReportPolicyAction(fidl::Clone(kMediaUsage),
                                              fuchsia::media::Behavior::MUTE);
  policy_action_reporter_->ReportPolicyAction(fidl::Clone(kMediaUsage),
                                              fuchsia::media::Behavior::MUTE);

  RunLoopUntilIdle();
  EXPECT_TRUE(fidl::Equals(watcher.impl()->last_usage(), kMediaUsage));
  EXPECT_TRUE(fidl::Equals(watcher.impl()->last_usage_state(), kMutedState));
  EXPECT_TRUE(AcksComplete(kMediaUsage));
}

TEST_F(UsageReporterImplTest, StatesAreEmittedToWatcher2) {
  auto watcher2 = Watch2(fidl::Clone(kMediaUsage), kActivateCallback);

  policy_action_reporter_->ReportPolicyAction(fidl::Clone(kMediaUsage),
                                              fuchsia::media::Behavior::MUTE);
  policy_action_reporter_->ReportPolicyAction(fidl::Clone(kMediaUsage),
                                              fuchsia::media::Behavior::MUTE);

  RunLoopUntilIdle();
  EXPECT_TRUE(fidl::Equals(watcher2.impl()->last_usage(), kMediaUsage));
  EXPECT_TRUE(fidl::Equals(watcher2.impl()->last_usage_state(), kMutedState));
  EXPECT_TRUE(AcksComplete(kMediaUsage));
}

TEST_F(UsageReporterImplTest, ErrorHandlerDisconnectsWatcher) {
  // Watcher dropped after block scope to trigger error handler.
  {
    auto watcher = Watch(fidl::Clone(kMediaUsage), kDeactivateCallback);
    auto watcher2 = Watch2(fidl::Clone(kMediaUsage), kDeactivateCallback);

    policy_action_reporter_->ReportPolicyAction(fidl::Clone(kMediaUsage),
                                                fuchsia::media::Behavior::MUTE);

    EXPECT_FALSE(WatchersDisconnected(kMediaUsage));
  }
  RunLoopUntilIdle();
  EXPECT_TRUE(WatchersDisconnected(kMediaUsage));
}

TEST_F(UsageReporterImplTest, StateIsEmittedToMultipleWatchers) {
  auto watcher1 = Watch(fidl::Clone(kMediaUsage), kActivateCallback);
  auto watcher2 = Watch(fidl::Clone(kMediaUsage), kActivateCallback);
  auto watcher3 = Watch2(fidl::Clone(kMediaUsage), kActivateCallback);
  auto watcher4 = Watch2(fidl::Clone(kMediaUsage), kActivateCallback);

  policy_action_reporter_->ReportPolicyAction(fidl::Clone(kMediaUsage),
                                              fuchsia::media::Behavior::MUTE);

  RunLoopUntilIdle();
  EXPECT_TRUE(fidl::Equals(watcher1.impl()->last_usage(), kMediaUsage));
  EXPECT_TRUE(fidl::Equals(watcher1.impl()->last_usage_state(), kMutedState));
  EXPECT_TRUE(fidl::Equals(watcher2.impl()->last_usage(), kMediaUsage));
  EXPECT_TRUE(fidl::Equals(watcher2.impl()->last_usage_state(), kMutedState));
  EXPECT_TRUE(fidl::Equals(watcher3.impl()->last_usage(), kMediaUsage));
  EXPECT_TRUE(fidl::Equals(watcher3.impl()->last_usage_state(), kMutedState));
  EXPECT_TRUE(fidl::Equals(watcher4.impl()->last_usage(), kMediaUsage));
  EXPECT_TRUE(fidl::Equals(watcher4.impl()->last_usage_state(), kMutedState));
  EXPECT_TRUE(AcksComplete(kMediaUsage));
}

TEST_F(UsageReporterImplTest, StatesAreEmittedToAllWatchers) {
  auto watcher1 = Watch(fidl::Clone(kMediaUsage), kActivateCallback);
  auto watcher2 = Watch(fidl::Clone(kMediaUsage), kActivateCallback);
  auto watcher3 = Watch2(fidl::Clone(kMediaUsage), kActivateCallback);
  auto watcher4 = Watch2(fidl::Clone(kMediaUsage), kActivateCallback);

  policy_action_reporter_->ReportPolicyAction(fidl::Clone(kMediaUsage),
                                              fuchsia::media::Behavior::MUTE);
  policy_action_reporter_->ReportPolicyAction(fidl::Clone(kMediaUsage),
                                              fuchsia::media::Behavior::MUTE);

  RunLoopUntilIdle();
  EXPECT_TRUE(fidl::Equals(watcher1.impl()->last_usage(), kMediaUsage));
  EXPECT_TRUE(fidl::Equals(watcher1.impl()->last_usage_state(), kMutedState));
  EXPECT_TRUE(fidl::Equals(watcher2.impl()->last_usage(), kMediaUsage));
  EXPECT_TRUE(fidl::Equals(watcher2.impl()->last_usage_state(), kMutedState));
  EXPECT_TRUE(fidl::Equals(watcher3.impl()->last_usage(), kMediaUsage));
  EXPECT_TRUE(fidl::Equals(watcher3.impl()->last_usage_state(), kMutedState));
  EXPECT_TRUE(fidl::Equals(watcher4.impl()->last_usage(), kMediaUsage));
  EXPECT_TRUE(fidl::Equals(watcher4.impl()->last_usage_state(), kMutedState));
  EXPECT_TRUE(AcksComplete(kMediaUsage));
}

TEST_F(UsageReporterImplTest, WatchersThatDontReplyAreDisconnected) {
  auto watcher1 = Watch(fidl::Clone(kMediaUsage), kDeactivateCallback);
  auto watcher2 = Watch(fidl::Clone(kMediaUsage), kDeactivateCallback);
  auto watcher3 = Watch2(fidl::Clone(kMediaUsage), kDeactivateCallback);
  auto watcher4 = Watch2(fidl::Clone(kMediaUsage), kDeactivateCallback);

  // Report up to kMaxStates and allow watchers to ack if enabled.
  for (int i = 0; i < kMaxStates; ++i) {
    policy_action_reporter_->ReportPolicyAction(fidl::Clone(kMediaUsage),
                                                fuchsia::media::Behavior::MUTE);
  }
  RunLoopUntilIdle();
  EXPECT_FALSE(WatchersDisconnected(kMediaUsage));

  // Report additional state to reach kMaxStates and cause disconnect of un-acking watchers.
  policy_action_reporter_->ReportPolicyAction(fidl::Clone(kMediaUsage),
                                              fuchsia::media::Behavior::MUTE);
  RunLoopUntilIdle();
  EXPECT_TRUE(WatchersDisconnected(kMediaUsage));
}

TEST_F(UsageReporterImplTest, WatcherReceivesCachedState) {
  // The watcher should receive the current state on connection.
  auto watcher1 = Watch(fidl::Clone(kMediaUsage), kActivateCallback);
  RunLoopUntilIdle();
  EXPECT_TRUE(fidl::Equals(watcher1.impl()->last_usage(), kMediaUsage));
  EXPECT_TRUE(fidl::Equals(watcher1.impl()->last_usage_state(), kUnadjustedState));

  policy_action_reporter_->ReportPolicyAction(fidl::Clone(kMediaUsage),
                                              fuchsia::media::Behavior::MUTE);

  // The new watcher should receive the current state when it connects, and that state should be
  // updated by the policy action.
  auto watcher2 = Watch(fidl::Clone(kMediaUsage), kActivateCallback);
  RunLoopUntilIdle();
  EXPECT_TRUE(fidl::Equals(watcher2.impl()->last_usage(), kMediaUsage));
  EXPECT_TRUE(fidl::Equals(watcher2.impl()->last_usage_state(), kMutedState));
  EXPECT_TRUE(AcksComplete(kMediaUsage));
}

TEST_F(UsageReporterImplTest, Watcher2ReceivesCachedState) {
  // The watcher should receive the current state on connection.
  auto watcher1 = Watch2(fidl::Clone(kMediaUsage), kActivateCallback);
  RunLoopUntilIdle();
  EXPECT_TRUE(fidl::Equals(watcher1.impl()->last_usage(), kMediaUsage));
  EXPECT_TRUE(fidl::Equals(watcher1.impl()->last_usage_state(), kUnadjustedState));

  policy_action_reporter_->ReportPolicyAction(fidl::Clone(kMediaUsage),
                                              fuchsia::media::Behavior::MUTE);

  // The new watcher should receive the current state when it connects, and that state should be
  // updated by the policy action.
  auto watcher2 = Watch2(fidl::Clone(kMediaUsage), kActivateCallback);
  RunLoopUntilIdle();
  EXPECT_TRUE(fidl::Equals(watcher2.impl()->last_usage(), kMediaUsage));
  EXPECT_TRUE(fidl::Equals(watcher2.impl()->last_usage_state(), kMutedState));
  EXPECT_TRUE(AcksComplete(kMediaUsage));
}

}  // namespace media::audio

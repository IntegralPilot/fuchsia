// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.ui.pointer/cpp/fidl.h>
#include <lib/async-testing/test_loop.h>
#include <lib/sys/cpp/testing/component_context_provider.h>
#include <lib/syslog/cpp/macros.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/ui/scenic/lib/input/touch_source.h"
#include "src/ui/scenic/lib/input/touch_system.h"
#include "src/ui/scenic/lib/utils/check_is_on_thread.h"

// These tests exercise the full gesture disambiguation implementation of InputSystem for
// clients of the  fuchsia.ui.pointer.TouchSource protocol.

namespace input::test {

using fup_EventPhase = fuchsia_ui_pointer::EventPhase;
using fup_TouchEvent = fuchsia_ui_pointer::TouchEvent;
using fup_TouchResponse = fuchsia_ui_pointer::TouchResponse;
using fup_TouchResponseType = fuchsia_ui_pointer::TouchResponseType;
using fup_TouchInteractionStatus = fuchsia_ui_pointer::TouchInteractionStatus;

using scenic_impl::input::InternalTouchEvent;
using scenic_impl::input::Phase;
using scenic_impl::input::StreamId;
using scenic_impl::input::TouchSource;

constexpr zx_koid_t kContextKoid = 100u;
constexpr zx_koid_t kClient1Koid = 1u;
constexpr zx_koid_t kClient2Koid = 2u;

constexpr StreamId kStream1Id = 11u;
constexpr StreamId kStream2Id = 22u;

namespace {

InternalTouchEvent PointerEventTemplate(zx_koid_t target) {
  InternalTouchEvent event;
  event.timestamp = 0;
  event.device_id = 1u;
  event.pointer_id = 1u;
  event.phase = Phase::kAdd;
  event.context = kContextKoid;
  event.target = target;
  event.position_in_viewport = glm::vec2(5, 5);
  event.buttons = 0;

  event.viewport.extents.min = {0, 0};
  event.viewport.extents.max = {10, 10};

  return event;
}

fup_TouchResponse MakeTouchResponse(fup_TouchResponseType response_type) {
  fup_TouchResponse response;
  response.response_type(response_type);
  return response;
}

}  // namespace

class GestureDisambiguationTest : public gtest::TestLoopFixture {
 public:
  GestureDisambiguationTest()
      : dispatcher_setter_(dispatcher(), dispatcher()),
        hit_tester_(inspect_node_),
        touch_system_(dispatcher(), hit_tester_, inspect_node_) {}

  void SetUp() override {
    ::testing::Test::SetUp();
    auto endpoints1 = fidl::Endpoints<fuchsia_ui_pointer::TouchSource>::Create();
    auto endpoints2 = fidl::Endpoints<fuchsia_ui_pointer::TouchSource>::Create();

    client1_ptr_.Bind(std::move(endpoints1.client), dispatcher(), &client1_event_handler_);
    client2_ptr_.Bind(std::move(endpoints2.client), dispatcher(), &client2_event_handler_);

    OnNewViewTreeSnapshot(NewSnapshot(
        /*hits*/ {}, /*hierarchy*/ {kContextKoid, kClient1Koid, kClient2Koid}));
    touch_system_.RegisterTouchSource(std::move(endpoints1.server), kClient1Koid);
    touch_system_.RegisterTouchSource(std::move(endpoints2.server), kClient2Koid);
  }

  void OnNewViewTreeSnapshot(std::shared_ptr<const view_tree::Snapshot> snapshot) {
    current_snapshot_ = std::move(snapshot);
  }

 protected:
  // Creates a new snapshot with a hit test that returns |hits|, and a ViewTree with a straight
  // hierarchy matching |hierarchy|.
  std::shared_ptr<view_tree::Snapshot> NewSnapshot(std::vector<zx_koid_t> hits,
                                                   std::vector<zx_koid_t> hierarchy) {
    auto snapshot = std::make_shared<view_tree::Snapshot>();
    snapshot->sequence_number = next_sequence_number_++;

    if (!hierarchy.empty()) {
      snapshot->root = hierarchy[0];
      const auto [_, success] = snapshot->view_tree.try_emplace(hierarchy[0]);
      FX_DCHECK(success);
      if (hierarchy.size() > 1) {
        snapshot->view_tree[hierarchy[0]].children = {hierarchy[1]};
        for (size_t i = 1; i < hierarchy.size() - 1; ++i) {
          snapshot->view_tree[hierarchy[i]].parent = hierarchy[i - 1];
          snapshot->view_tree[hierarchy[i]].children = {hierarchy[i + 1]};
        }
        snapshot->view_tree[hierarchy.back()].parent = hierarchy[hierarchy.size() - 2];
      }
    }

    snapshot->hit_testers.emplace_back([hits = std::move(hits)](auto...) {
      return view_tree::SubtreeHitTestResult{.hits = hits};
    });

    return snapshot;
  }

 private:
  utils::ScopedThreadDispatcherSetter dispatcher_setter_;
  // Must be initialized before |touch_system_|.
  sys::testing::ComponentContextProvider context_provider_;
  uint64_t next_sequence_number_ = 1;

 protected:
  class Client1EventHandler : public fidl::AsyncEventHandler<fuchsia_ui_pointer::TouchSource> {
   public:
    bool channel_closed = false;
    void on_fidl_error(fidl::UnbindInfo info) override { channel_closed = true; }
  };
  class Client2EventHandler : public fidl::AsyncEventHandler<fuchsia_ui_pointer::TouchSource> {
   public:
    bool channel_closed = false;
    void on_fidl_error(fidl::UnbindInfo info) override { channel_closed = true; }
  };

  Client1EventHandler client1_event_handler_;
  Client2EventHandler client2_event_handler_;

  inspect::Node inspect_node_;
  std::shared_ptr<const view_tree::Snapshot> current_snapshot_;
  scenic_impl::input::HitTester hit_tester_;
  scenic_impl::input::TouchSystem touch_system_;
  fidl::Client<fuchsia_ui_pointer::TouchSource> client1_ptr_;
  fidl::Client<fuchsia_ui_pointer::TouchSource> client2_ptr_;
};

TEST_F(GestureDisambiguationTest, Watch_WithNoInjectedEvents_ShouldNeverReturn) {
  bool callback_triggered = false;
  client1_ptr_->Watch({{.responses = {}}}).Then([&callback_triggered](auto& result) {
    callback_triggered = result.is_ok();
  });

  RunLoopUntilIdle();
  EXPECT_FALSE(callback_triggered);
}

TEST_F(GestureDisambiguationTest, IllegalOperation_ShouldCloseChannel) {
  // Illegal operation: calling Watch() twice without getting an event.
  bool callback_triggered = false;
  client1_ptr_->Watch({{.responses = {}}}).Then([&callback_triggered](auto& result) {
    callback_triggered = result.is_ok();
  });
  client1_ptr_->Watch({{.responses = {}}}).Then([&callback_triggered](auto& result) {
    callback_triggered = result.is_ok();
  });
  RunLoopUntilIdle();
  EXPECT_TRUE(client1_event_handler_.channel_closed);
  EXPECT_FALSE(callback_triggered);
}

TEST_F(GestureDisambiguationTest, ExclusiveInjection_ShouldBeDeliveredOnlyToTarget_AndBeGranted) {
  std::vector<fup_TouchEvent> received_events1;
  client1_ptr_->Watch({{.responses = {}}})
      .Then([&received_events1](fidl::Result<fuchsia_ui_pointer::TouchSource::Watch>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        auto events = std::move(result->events());
        std::move(events.begin(), events.end(), std::back_inserter(received_events1));
      });
  std::vector<fup_TouchEvent> received_events2;
  client2_ptr_->Watch({{.responses = {}}})
      .Then([&received_events2](fidl::Result<fuchsia_ui_pointer::TouchSource::Watch>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        auto events = std::move(result->events());
        std::move(events.begin(), events.end(), std::back_inserter(received_events2));
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_events1.empty());
  EXPECT_TRUE(received_events2.empty());

  touch_system_.InjectTouchEventExclusive(PointerEventTemplate(kClient1Koid), kStream1Id,
                                          *current_snapshot_);
  RunLoopUntilIdle();
  EXPECT_EQ(received_events1.size(), 1u);
  ASSERT_TRUE(received_events1[0].interaction_result().has_value());
  EXPECT_EQ(received_events1[0].interaction_result()->status(),
            fup_TouchInteractionStatus::kGranted);
  EXPECT_TRUE(received_events2.empty());

  received_events1.clear();
  touch_system_.InjectTouchEventExclusive(PointerEventTemplate(kClient2Koid), kStream2Id,
                                          *current_snapshot_);
  RunLoopUntilIdle();
  ASSERT_EQ(received_events2.size(), 1u);
  ASSERT_TRUE(received_events2[0].interaction_result().has_value());
  EXPECT_EQ(received_events2[0].interaction_result()->status(),
            fup_TouchInteractionStatus::kGranted);
  EXPECT_TRUE(received_events1.empty());
}

TEST_F(GestureDisambiguationTest,
       InjectionThatHitsClientWithoutValidAncestors_ShouldBeDeliveredAndBeGranted) {
  std::vector<fup_TouchEvent> received_events1;
  client1_ptr_->Watch({{.responses = {}}})
      .Then([&received_events1](fidl::Result<fuchsia_ui_pointer::TouchSource::Watch>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        auto events = std::move(result->events());
        std::move(events.begin(), events.end(), std::back_inserter(received_events1));
      });
  std::vector<fup_TouchEvent> received_events2;
  client2_ptr_->Watch({{.responses = {}}})
      .Then([&received_events2](fidl::Result<fuchsia_ui_pointer::TouchSource::Watch>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        auto events = std::move(result->events());
        std::move(events.begin(), events.end(), std::back_inserter(received_events2));
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_events1.empty());
  EXPECT_TRUE(received_events2.empty());

  OnNewViewTreeSnapshot(NewSnapshot(
      /*hits*/ {kClient1Koid}, /*hierarchy*/ {kContextKoid, kClient1Koid, kClient2Koid}));

  touch_system_.InjectTouchEventHitTested(PointerEventTemplate(kClient1Koid), kStream1Id,
                                          *current_snapshot_);
  RunLoopUntilIdle();
  ASSERT_EQ(received_events1.size(), 1u);
  ASSERT_TRUE(received_events1[0].interaction_result().has_value());
  EXPECT_EQ(received_events1[0].interaction_result()->status(),
            fup_TouchInteractionStatus::kGranted);
  EXPECT_TRUE(received_events2.empty());

  OnNewViewTreeSnapshot(
      NewSnapshot(/*hits*/ {kClient2Koid}, /*hierarchy*/ {kContextKoid, kClient2Koid}));

  touch_system_.InjectTouchEventHitTested(PointerEventTemplate(kClient2Koid), kStream2Id,
                                          *current_snapshot_);
  RunLoopUntilIdle();
  ASSERT_EQ(received_events2.size(), 1u);
  ASSERT_TRUE(received_events2[0].interaction_result().has_value());
  EXPECT_EQ(received_events2[0].interaction_result()->status(),
            fup_TouchInteractionStatus::kGranted);
}

TEST_F(GestureDisambiguationTest,
       InjectionThatHitsClientWithValidAncestor_ShouldBeDeliveredToBoth) {
  std::vector<fup_TouchEvent> received_events1;
  client1_ptr_->Watch({{.responses = {}}})
      .Then([&received_events1](fidl::Result<fuchsia_ui_pointer::TouchSource::Watch>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        auto events = std::move(result->events());
        std::move(events.begin(), events.end(), std::back_inserter(received_events1));
      });
  std::vector<fup_TouchEvent> received_events2;
  client2_ptr_->Watch({{.responses = {}}})
      .Then([&received_events2](fidl::Result<fuchsia_ui_pointer::TouchSource::Watch>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        auto events = std::move(result->events());
        std::move(events.begin(), events.end(), std::back_inserter(received_events2));
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_events1.empty());
  EXPECT_TRUE(received_events2.empty());

  OnNewViewTreeSnapshot(NewSnapshot(
      /*hits*/ {kClient2Koid}, /*hierarchy*/ {kContextKoid, kClient1Koid, kClient2Koid}));
  touch_system_.InjectTouchEventHitTested(PointerEventTemplate(kClient1Koid), kStream1Id,
                                          *current_snapshot_);

  RunLoopUntilIdle();
  ASSERT_EQ(received_events1.size(), 1u);
  EXPECT_FALSE(received_events1.front().interaction_result().has_value());
  ASSERT_EQ(received_events2.size(), 1u);
  EXPECT_FALSE(received_events2.front().interaction_result().has_value());

  {
    std::vector<fup_TouchResponse> responses;
    responses.emplace_back(MakeTouchResponse(fup_TouchResponseType::kMaybe));
    client1_ptr_->Watch({{.responses = std::move(responses)}})
        .Then([&received_events1](fidl::Result<fuchsia_ui_pointer::TouchSource::Watch>& result) {
          ASSERT_TRUE(result.is_ok()) << result.error_value();
          auto events = std::move(result->events());
          std::move(events.begin(), events.end(), std::back_inserter(received_events1));
        });
  }
  {
    std::vector<fup_TouchResponse> responses;
    responses.emplace_back(MakeTouchResponse(fup_TouchResponseType::kMaybe));
    client2_ptr_->Watch({{.responses = std::move(responses)}})
        .Then([&received_events2](fidl::Result<fuchsia_ui_pointer::TouchSource::Watch>& result) {
          ASSERT_TRUE(result.is_ok()) << result.error_value();
          auto events = std::move(result->events());
          std::move(events.begin(), events.end(), std::back_inserter(received_events2));
        });
  }

  // No one should be granted the win yet, so expect no more events.
  RunLoopUntilIdle();
  EXPECT_EQ(received_events1.size(), 1u);
  EXPECT_EQ(received_events2.size(), 1u);
}

TEST_F(GestureDisambiguationTest, Contest_ShouldNotIncludeContext) {
  std::vector<fup_TouchEvent> received_events1;
  client1_ptr_->Watch({{.responses = {}}})
      .Then([&received_events1](fidl::Result<fuchsia_ui_pointer::TouchSource::Watch>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        auto events = std::move(result->events());
        std::move(events.begin(), events.end(), std::back_inserter(received_events1));
      });
  std::vector<fup_TouchEvent> received_events2;
  client2_ptr_->Watch({{.responses = {}}})
      .Then([&received_events2](fidl::Result<fuchsia_ui_pointer::TouchSource::Watch>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        auto events = std::move(result->events());
        std::move(events.begin(), events.end(), std::back_inserter(received_events2));
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_events1.empty());
  EXPECT_TRUE(received_events2.empty());

  // Inject an event with kClient1Koid as the context and kClient2Koid as the target.
  OnNewViewTreeSnapshot(NewSnapshot(
      /*hits*/ {kClient2Koid}, /*hierarchy*/ {kContextKoid, kClient1Koid, kClient2Koid}));
  auto event = PointerEventTemplate(kClient2Koid);
  event.context = kClient1Koid;
  touch_system_.InjectTouchEventHitTested(std::move(event), kStream1Id, *current_snapshot_);

  RunLoopUntilIdle();
  EXPECT_TRUE(received_events1.empty()) << "The context should not receive any events.";
  EXPECT_EQ(received_events2.size(), 1u);
}

TEST_F(GestureDisambiguationTest, EveryoneRespondsYesPrioritize_ShouldResolveToHighestPriority) {
  std::vector<fup_TouchEvent> received_events1;
  client1_ptr_->Watch({{.responses = {}}})
      .Then([&received_events1](fidl::Result<fuchsia_ui_pointer::TouchSource::Watch>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        auto events = std::move(result->events());
        std::move(events.begin(), events.end(), std::back_inserter(received_events1));
      });
  std::vector<fup_TouchEvent> received_events2;
  client2_ptr_->Watch({{.responses = {}}})
      .Then([&received_events2](fidl::Result<fuchsia_ui_pointer::TouchSource::Watch>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        auto events = std::move(result->events());
        std::move(events.begin(), events.end(), std::back_inserter(received_events2));
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_events1.empty());
  EXPECT_TRUE(received_events2.empty());

  OnNewViewTreeSnapshot(NewSnapshot(
      /*hits*/ {kClient2Koid}, /*hierarchy*/ {kContextKoid, kClient1Koid, kClient2Koid}));
  touch_system_.InjectTouchEventHitTested(PointerEventTemplate(kClient1Koid), kStream1Id,
                                          *current_snapshot_);

  RunLoopUntilIdle();
  ASSERT_EQ(received_events1.size(), 1u);
  EXPECT_FALSE(received_events1.front().interaction_result().has_value());
  ASSERT_EQ(received_events2.size(), 1u);
  EXPECT_FALSE(received_events2.front().interaction_result().has_value());

  // Both try to claim the stream, but client1 has higher priority and should win.
  {
    std::vector<fup_TouchResponse> responses;
    responses.emplace_back(MakeTouchResponse(fup_TouchResponseType::kYesPrioritize));
    client1_ptr_->Watch({{.responses = std::move(responses)}})
        .Then([&received_events1](fidl::Result<fuchsia_ui_pointer::TouchSource::Watch>& result) {
          ASSERT_TRUE(result.is_ok()) << result.error_value();
          auto events = std::move(result->events());
          std::move(events.begin(), events.end(), std::back_inserter(received_events1));
        });
  }
  {
    std::vector<fup_TouchResponse> responses;
    responses.emplace_back(MakeTouchResponse(fup_TouchResponseType::kYesPrioritize));
    client2_ptr_->Watch({{.responses = std::move(responses)}})
        .Then([&received_events2](fidl::Result<fuchsia_ui_pointer::TouchSource::Watch>& result) {
          ASSERT_TRUE(result.is_ok()) << result.error_value();
          auto events = std::move(result->events());
          std::move(events.begin(), events.end(), std::back_inserter(received_events2));
        });
  }

  // Both should have received an event with a TouchInteractionStatus.
  RunLoopUntilIdle();
  ASSERT_EQ(received_events1.size(), 2u);
  ASSERT_TRUE(received_events1[1].interaction_result().has_value());
  EXPECT_EQ(received_events1[1].interaction_result()->status(),
            fup_TouchInteractionStatus::kGranted);
  ASSERT_EQ(received_events2.size(), 2u);
  ASSERT_TRUE(received_events2[1].interaction_result().has_value());
  EXPECT_EQ(received_events2[1].interaction_result()->status(),
            fup_TouchInteractionStatus::kDenied);

  // Subsequent events should only go to the winner.
  {
    std::vector<fup_TouchResponse> responses;
    responses.emplace_back();
    client1_ptr_->Watch({{.responses = std::move(responses)}})
        .Then([&received_events1](fidl::Result<fuchsia_ui_pointer::TouchSource::Watch>& result) {
          ASSERT_TRUE(result.is_ok()) << result.error_value();
          auto events = std::move(result->events());
          std::move(events.begin(), events.end(), std::back_inserter(received_events1));
        });
  }
  {
    std::vector<fup_TouchResponse> responses;
    responses.emplace_back();
    client2_ptr_->Watch({{.responses = std::move(responses)}})
        .Then([&received_events2](fidl::Result<fuchsia_ui_pointer::TouchSource::Watch>& result) {
          ASSERT_TRUE(result.is_ok()) << result.error_value();
          auto events = std::move(result->events());
          std::move(events.begin(), events.end(), std::back_inserter(received_events2));
        });
  }

  auto event = PointerEventTemplate(kClient1Koid);
  event.phase = Phase::kChange;
  touch_system_.InjectTouchEventHitTested(std::move(event), kStream1Id, *current_snapshot_);

  RunLoopUntilIdle();
  EXPECT_EQ(received_events1.size(), 3u);
  ASSERT_EQ(received_events2.size(), 2u);
}

TEST_F(GestureDisambiguationTest, EveryonRespondsMaybe_ShouldResolveAtStreamEnd) {
  OnNewViewTreeSnapshot(NewSnapshot(
      /*hits*/ {kClient2Koid}, /*hierarchy*/ {kContextKoid, kClient1Koid, kClient2Koid}));

  // Inject one event for each phase:
  {
    auto event = PointerEventTemplate(kClient1Koid);
    event.phase = Phase::kAdd;
    touch_system_.InjectTouchEventHitTested(event.ShallowClone(), kStream1Id, *current_snapshot_);
    event.phase = Phase::kChange;
    touch_system_.InjectTouchEventHitTested(event.ShallowClone(), kStream1Id, *current_snapshot_);
    event.phase = Phase::kRemove;
    touch_system_.InjectTouchEventHitTested(std::move(event), kStream1Id, *current_snapshot_);
  }

  std::vector<fup_TouchEvent> received_events1;
  client1_ptr_->Watch({{.responses = {}}})
      .Then([&received_events1](fidl::Result<fuchsia_ui_pointer::TouchSource::Watch>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        auto events = std::move(result->events());
        std::move(events.begin(), events.end(), std::back_inserter(received_events1));
      });
  std::vector<fup_TouchEvent> received_events2;
  client2_ptr_->Watch({{.responses = {}}})
      .Then([&received_events2](fidl::Result<fuchsia_ui_pointer::TouchSource::Watch>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        auto events = std::move(result->events());
        std::move(events.begin(), events.end(), std::back_inserter(received_events2));
      });

  RunLoopUntilIdle();
  EXPECT_EQ(received_events1.size(), 3u);
  EXPECT_EQ(received_events2.size(), 3u);

  // Both respond MAYBE for the entire stream. Client2 has lower priority and should win at stream
  // end.
  {
    std::vector<fup_TouchResponse> responses(received_events1.size());
    std::generate(responses.begin(), responses.end(),
                  [] { return MakeTouchResponse(fup_TouchResponseType::kMaybe); });
    client1_ptr_->Watch({{.responses = std::move(responses)}})
        .Then([&received_events1](fidl::Result<fuchsia_ui_pointer::TouchSource::Watch>& result) {
          ASSERT_TRUE(result.is_ok()) << result.error_value();
          auto events = std::move(result->events());
          std::move(events.begin(), events.end(), std::back_inserter(received_events1));
        });
  }
  {
    std::vector<fup_TouchResponse> responses(received_events2.size());
    std::generate(responses.begin(), responses.end(),
                  [] { return MakeTouchResponse(fup_TouchResponseType::kMaybe); });
    client2_ptr_->Watch({{.responses = std::move(responses)}})
        .Then([&received_events2](fidl::Result<fuchsia_ui_pointer::TouchSource::Watch>& result) {
          ASSERT_TRUE(result.is_ok()) << result.error_value();
          auto events = std::move(result->events());
          std::move(events.begin(), events.end(), std::back_inserter(received_events2));
        });
  }

  // Both should have received an event with a TouchInteractionStatus.
  RunLoopUntilIdle();
  ASSERT_TRUE(received_events1.back().interaction_result().has_value());
  EXPECT_EQ(received_events1.back().interaction_result()->status(),
            fup_TouchInteractionStatus::kDenied);
  ASSERT_TRUE(received_events2.back().interaction_result().has_value());
  EXPECT_EQ(received_events2.back().interaction_result()->status(),
            fup_TouchInteractionStatus::kGranted);
}

TEST_F(GestureDisambiguationTest, MidStreamChannelClose_ShouldGrantStreamToCompetitor) {
  OnNewViewTreeSnapshot(NewSnapshot(/*hits*/ {kClient2Koid},
                                    /*hierarchy*/ {kContextKoid, kClient1Koid, kClient2Koid}));
  touch_system_.InjectTouchEventHitTested(PointerEventTemplate(kClient1Koid), kStream1Id,
                                          *current_snapshot_);

  {
    std::vector<fup_TouchEvent> received_events;
    client1_ptr_->Watch({{.responses = {}}})
        .Then([&received_events](fidl::Result<fuchsia_ui_pointer::TouchSource::Watch>& result) {
          ASSERT_TRUE(result.is_ok()) << result.error_value();
          auto events = std::move(result->events());
          std::move(events.begin(), events.end(), std::back_inserter(received_events));
        });

    RunLoopUntilIdle();
    EXPECT_EQ(received_events.size(), 1u);
  }
  {
    std::vector<fup_TouchEvent> received_events;
    client2_ptr_->Watch({{.responses = {}}})
        .Then([&received_events](fidl::Result<fuchsia_ui_pointer::TouchSource::Watch>& result) {
          ASSERT_TRUE(result.is_ok()) << result.error_value();
          auto events = std::move(result->events());
          std::move(events.begin(), events.end(), std::back_inserter(received_events));
        });
    RunLoopUntilIdle();
    EXPECT_EQ(received_events.size(), 1u);
  }

  // Close client1's channel.
  client1_ptr_ = {};

  {  // Observe client2 winning the contest.
    std::vector<fup_TouchEvent> received_events;
    std::vector<fup_TouchResponse> responses;
    responses.emplace_back(MakeTouchResponse(fup_TouchResponseType::kMaybe));
    client2_ptr_->Watch({{.responses = std::move(responses)}})
        .Then([&received_events](fidl::Result<fuchsia_ui_pointer::TouchSource::Watch>& result) {
          ASSERT_TRUE(result.is_ok()) << result.error_value();
          auto events = std::move(result->events());
          std::move(events.begin(), events.end(), std::back_inserter(received_events));
        });

    RunLoopUntilIdle();
    ASSERT_EQ(received_events.size(), 1u);
    ASSERT_TRUE(received_events.front().interaction_result().has_value());
    EXPECT_EQ(received_events.front().interaction_result()->status(),
              fup_TouchInteractionStatus::kGranted);
  }
}

TEST_F(GestureDisambiguationTest, MidStreamChannelForcedClose_ShouldGrantStreamToCompetitor) {
  OnNewViewTreeSnapshot(NewSnapshot(/*hits*/ {kClient2Koid},
                                    /*hierarchy*/ {kContextKoid, kClient1Koid, kClient2Koid}));
  touch_system_.InjectTouchEventHitTested(PointerEventTemplate(kClient1Koid), kStream1Id,
                                          *current_snapshot_);

  {
    std::vector<fup_TouchEvent> received_events;
    client1_ptr_->Watch({{.responses = {}}})
        .Then([&received_events](fidl::Result<fuchsia_ui_pointer::TouchSource::Watch>& result) {
          ASSERT_TRUE(result.is_ok()) << result.error_value();
          auto events = std::move(result->events());
          std::move(events.begin(), events.end(), std::back_inserter(received_events));
        });

    RunLoopUntilIdle();
    EXPECT_EQ(received_events.size(), 1u);
  }
  {
    std::vector<fup_TouchEvent> received_events;
    client2_ptr_->Watch({{.responses = {}}})
        .Then([&received_events](fidl::Result<fuchsia_ui_pointer::TouchSource::Watch>& result) {
          ASSERT_TRUE(result.is_ok()) << result.error_value();
          auto events = std::move(result->events());
          std::move(events.begin(), events.end(), std::back_inserter(received_events));
        });
    RunLoopUntilIdle();
    EXPECT_EQ(received_events.size(), 1u);
  }

  {  // Illegal operation: empty response vector after first call. Observe channel close.
    EXPECT_FALSE(client1_event_handler_.channel_closed);
    bool callback_triggered = false;
    client1_ptr_->Watch({{.responses = {}}}).Then([&callback_triggered](auto& result) {
      callback_triggered = result.is_ok();
    });
    RunLoopUntilIdle();
    EXPECT_TRUE(client1_event_handler_.channel_closed);
    EXPECT_FALSE(callback_triggered);
  }

  {  // Observe client2 winning the contest.
    std::vector<fup_TouchEvent> received_events;
    std::vector<fup_TouchResponse> responses;
    responses.emplace_back(MakeTouchResponse(fup_TouchResponseType::kMaybe));
    client2_ptr_->Watch({{.responses = std::move(responses)}})
        .Then([&received_events](fidl::Result<fuchsia_ui_pointer::TouchSource::Watch>& result) {
          ASSERT_TRUE(result.is_ok()) << result.error_value();
          auto events = std::move(result->events());
          std::move(events.begin(), events.end(), std::back_inserter(received_events));
        });
    RunLoopUntilIdle();
    ASSERT_EQ(received_events.size(), 1u);
    ASSERT_TRUE(received_events.front().interaction_result().has_value());
    EXPECT_EQ(received_events.front().interaction_result()->status(),
              fup_TouchInteractionStatus::kGranted);
  }
}

TEST_F(GestureDisambiguationTest,
       LoserDisconnectDuringContestResolution_MultipleStreams_ShouldNotCrash) {
  OnNewViewTreeSnapshot(NewSnapshot(/*hits*/ {kClient2Koid},
                                    /*hierarchy*/ {kContextKoid, kClient1Koid, kClient2Koid}));

  // Inject two concurrent streams.
  touch_system_.InjectTouchEventHitTested(PointerEventTemplate(kClient1Koid), kStream1Id,
                                          *current_snapshot_);
  touch_system_.InjectTouchEventHitTested(PointerEventTemplate(kClient1Koid), kStream2Id,
                                          *current_snapshot_);

  std::vector<fup_TouchEvent> received_events1;
  std::vector<fup_TouchEvent> received_events2;

  client1_ptr_->Watch({{.responses = {}}})
      .Then([&received_events1](fidl::Result<fuchsia_ui_pointer::TouchSource::Watch>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        auto events = std::move(result->events());
        std::move(events.begin(), events.end(), std::back_inserter(received_events1));
      });
  client2_ptr_->Watch({{.responses = {}}})
      .Then([&received_events2](fidl::Result<fuchsia_ui_pointer::TouchSource::Watch>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        auto events = std::move(result->events());
        std::move(events.begin(), events.end(), std::back_inserter(received_events2));
      });
  RunLoopUntilIdle();

  ASSERT_EQ(received_events1.size(), 2u);
  ASSERT_EQ(received_events2.size(), 2u);

  // Client 2 unbinds its channel. When contest is resolved for stream 1, Client 2's EndContest
  // will encounter unbind/error and trigger EraseContender, which synchronously resolves stream 2.
  client2_ptr_ = {};

  std::vector<fup_TouchResponse> responses1;
  responses1.emplace_back(MakeTouchResponse(fup_TouchResponseType::kYesPrioritize));
  responses1.emplace_back(MakeTouchResponse(fup_TouchResponseType::kYesPrioritize));

  std::vector<fup_TouchEvent> win_events1;
  client1_ptr_->Watch({{.responses = std::move(responses1)}})
      .Then([&win_events1](fidl::Result<fuchsia_ui_pointer::TouchSource::Watch>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        auto events = std::move(result->events());
        std::move(events.begin(), events.end(), std::back_inserter(win_events1));
      });
  RunLoopUntilIdle();

  ASSERT_EQ(win_events1.size(), 2u);
  ASSERT_TRUE(win_events1[0].interaction_result().has_value());
  EXPECT_EQ(win_events1[0].interaction_result()->status(), fup_TouchInteractionStatus::kGranted);
  ASSERT_TRUE(win_events1[1].interaction_result().has_value());
  EXPECT_EQ(win_events1[1].interaction_result()->status(), fup_TouchInteractionStatus::kGranted);
}

}  // namespace input::test

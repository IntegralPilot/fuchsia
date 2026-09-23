// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/input/touch_source_v2.h"

#include <fidl/fuchsia.ui.pointer/cpp/fidl.h>
#include <lib/async-testing/test_loop.h>
#include <lib/syslog/cpp/macros.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"

namespace input::test {

using scenic_impl::input::GestureContenderInspector;
using scenic_impl::input::GestureResponse;
using scenic_impl::input::InternalTouchEvent;
using scenic_impl::input::Phase;
using scenic_impl::input::StreamId;
using scenic_impl::input::TouchSourceV2;

constexpr zx_koid_t kViewRefKoid = 25;
constexpr StreamId kStreamId = 1;
constexpr uint32_t kDeviceId = 2;
constexpr uint32_t kPointerId = 3;
const view_tree::Snapshot kSnapshot{};

namespace {

InternalTouchEvent CreateTouchEvent(Phase phase) {
  InternalTouchEvent event;
  event.device_id = kDeviceId;
  event.pointer_id = kPointerId;
  event.phase = phase;
  event.viewport = {
      .receiver_from_viewport_transform = std::array<float, 9>({1, 0, 0, 0, 1, 0, 0, 0, 1}),
  };
  return event;
}

class TestTouchSourceV2Client : public fidl::AsyncEventHandler<fuchsia_ui_pointer::TouchSourceV2> {
 public:
  void OnTouchEvents(
      fidl::Event<fuchsia_ui_pointer::TouchSourceV2::OnTouchEvents>& event) override {
    received_events_.insert(received_events_.end(), std::make_move_iterator(event.events().begin()),
                            std::make_move_iterator(event.events().end()));
    last_event_stamp_ = event.last_event_stamp();
  }

  void on_fidl_error(fidl::UnbindInfo info) override { unbind_info_ = info; }
  void handle_unknown_event(
      fidl::UnknownEventMetadata<fuchsia_ui_pointer::TouchSourceV2> metadata) override {}

  std::vector<fuchsia_ui_pointer::TouchEvent> received_events_;
  uint64_t last_event_stamp_ = 0;
  std::optional<fidl::UnbindInfo> unbind_info_;
};

}  // namespace

class TouchSourceV2Test : public gtest::TestLoopFixture {
 protected:
  void SetUp() override {
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_pointer::TouchSourceV2>::Create();

    touch_source_.emplace(
        dispatcher(), kViewRefKoid, std::move(server_end),
        /*respond*/
        [this](StreamId stream_id, const std::vector<GestureResponse>& responses) {
          std::copy(responses.begin(), responses.end(),
                    std::back_inserter(received_responses_[stream_id]));
        },
        /*error_handler*/ [this] { internal_error_handler_fired_ = true; }, inspector_);

    client_.emplace(std::move(client_end), dispatcher(), &event_handler_);
  }

  bool internal_error_handler_fired_ = false;
  std::unordered_map<StreamId, std::vector<GestureResponse>> received_responses_;

  GestureContenderInspector inspector_{inspect::Node()};
  TestTouchSourceV2Client event_handler_;
  std::optional<fidl::Client<fuchsia_ui_pointer::TouchSourceV2>> client_;
  std::optional<TouchSourceV2> touch_source_;
};

TEST_F(TouchSourceV2Test, UpdateStream_AlwaysRespondsYes) {
  InternalTouchEvent add_event = CreateTouchEvent(Phase::kAdd);
  touch_source_->UpdateStream(kSnapshot, kStreamId, std::move(add_event),
                              /*is_end_of_stream*/ false, {});

  EXPECT_THAT(received_responses_[kStreamId], testing::ElementsAre(GestureResponse::kYes));
}

TEST_F(TouchSourceV2Test, UpdateStream_EndOfStream_RespondsYes) {
  InternalTouchEvent add_event = CreateTouchEvent(Phase::kAdd);
  touch_source_->UpdateStream(kSnapshot, kStreamId, std::move(add_event),
                              /*is_end_of_stream*/ false, {});
  received_responses_.clear();

  InternalTouchEvent remove_event = CreateTouchEvent(Phase::kRemove);
  touch_source_->UpdateStream(kSnapshot, kStreamId, std::move(remove_event),
                              /*is_end_of_stream*/ true, {});

  EXPECT_THAT(received_responses_[kStreamId], testing::ElementsAre(GestureResponse::kYes));
}

TEST_F(TouchSourceV2Test, PushEventsAndAck) {
  InternalTouchEvent add_event = CreateTouchEvent(Phase::kAdd);
  touch_source_->UpdateStream(kSnapshot, kStreamId, std::move(add_event),
                              /*is_end_of_stream*/ false, {});

  RunLoopUntilIdle();

  EXPECT_EQ(event_handler_.received_events_.size(), 1u);
  EXPECT_EQ(event_handler_.last_event_stamp_, 1u);

  // Send ACK to server.
  auto ack_result = (*client_)->AcknowledgeEvents({1});
  EXPECT_TRUE(ack_result.is_ok());
  RunLoopUntilIdle();

  EXPECT_FALSE(internal_error_handler_fired_);
}

TEST_F(TouchSourceV2Test, FlowControl_BuffersAndFlushesOnAck) {
  // Push kMaxUnacknowledgedEvents (150) events.
  for (uint64_t i = 1; i <= TouchSourceV2::kMaxUnacknowledgedEvents; ++i) {
    Phase phase = (i == 1) ? Phase::kAdd : Phase::kChange;
    InternalTouchEvent event = CreateTouchEvent(phase);
    touch_source_->UpdateStream(kSnapshot, kStreamId, std::move(event), /*is_end_of_stream*/ false,
                                {});
  }
  RunLoopUntilIdle();

  EXPECT_EQ(event_handler_.received_events_.size(), TouchSourceV2::kMaxUnacknowledgedEvents);
  EXPECT_EQ(event_handler_.last_event_stamp_, TouchSourceV2::kMaxUnacknowledgedEvents);

  // Push 151st event. Limit reached (150 unacked), so it should be buffered.
  InternalTouchEvent extra_event = CreateTouchEvent(Phase::kChange);
  touch_source_->UpdateStream(kSnapshot, kStreamId, std::move(extra_event),
                              /*is_end_of_stream*/ false, {});
  RunLoopUntilIdle();

  // No new events received by client yet.
  EXPECT_EQ(event_handler_.received_events_.size(), TouchSourceV2::kMaxUnacknowledgedEvents);

  // Acknowledge the 150 events.
  auto ack_result = (*client_)->AcknowledgeEvents({TouchSourceV2::kMaxUnacknowledgedEvents});
  EXPECT_TRUE(ack_result.is_ok());
  RunLoopUntilIdle();

  // Now the buffered 151st event is sent.
  EXPECT_EQ(event_handler_.received_events_.size(), TouchSourceV2::kMaxUnacknowledgedEvents + 1);
  EXPECT_EQ(event_handler_.last_event_stamp_, TouchSourceV2::kMaxUnacknowledgedEvents + 1);
}

TEST_F(TouchSourceV2Test, FlowControl_PartialBatchAck) {
  // Push 10 events (all sent in 1 batch, stamps 1..10).
  for (uint64_t i = 1; i <= 10; ++i) {
    Phase phase = (i == 1) ? Phase::kAdd : Phase::kChange;
    InternalTouchEvent event = CreateTouchEvent(phase);
    touch_source_->UpdateStream(kSnapshot, kStreamId, std::move(event), /*is_end_of_stream*/ false,
                                {});
  }
  RunLoopUntilIdle();
  EXPECT_EQ(event_handler_.received_events_.size(), 10u);
  EXPECT_EQ(event_handler_.last_event_stamp_, 10u);

  // Partially acknowledge stamp 5.
  auto ack_result = (*client_)->AcknowledgeEvents({5});
  EXPECT_TRUE(ack_result.is_ok());
  RunLoopUntilIdle();
  EXPECT_FALSE(internal_error_handler_fired_);

  // Fully acknowledge stamp 10.
  ack_result = (*client_)->AcknowledgeEvents({10});
  EXPECT_TRUE(ack_result.is_ok());
  RunLoopUntilIdle();
  EXPECT_FALSE(internal_error_handler_fired_);
}

TEST_F(TouchSourceV2Test, InvalidAck_ClosesChannel) {
  InternalTouchEvent add_event = CreateTouchEvent(Phase::kAdd);
  touch_source_->UpdateStream(kSnapshot, kStreamId, std::move(add_event),
                              /*is_end_of_stream*/ false, {});
  RunLoopUntilIdle();

  // Acknowledge stamp 999 (which hasn't been sent yet).
  auto ack_result = (*client_)->AcknowledgeEvents({999});
  EXPECT_TRUE(ack_result.is_ok());
  RunLoopUntilIdle();

  EXPECT_TRUE(internal_error_handler_fired_);
}

TEST_F(TouchSourceV2Test, DuplicateOrEarlierAck_ClosesChannel) {
  for (uint64_t i = 1; i <= 5; ++i) {
    Phase phase = (i == 1) ? Phase::kAdd : Phase::kChange;
    InternalTouchEvent event = CreateTouchEvent(phase);
    touch_source_->UpdateStream(kSnapshot, kStreamId, std::move(event), /*is_end_of_stream*/ false,
                                {});
  }
  RunLoopUntilIdle();

  EXPECT_EQ(event_handler_.received_events_.size(), 5u);
  EXPECT_EQ(event_handler_.last_event_stamp_, 5u);

  // Send ACK for stamp 5.
  auto ack_result = (*client_)->AcknowledgeEvents({5});
  EXPECT_TRUE(ack_result.is_ok());
  RunLoopUntilIdle();
  EXPECT_FALSE(internal_error_handler_fired_);

  // Send duplicate ACK for stamp 5.
  ack_result = (*client_)->AcknowledgeEvents({5});
  EXPECT_TRUE(ack_result.is_ok());
  RunLoopUntilIdle();
  EXPECT_TRUE(internal_error_handler_fired_);
}

TEST_F(TouchSourceV2Test, EarlierAck_ClosesChannel) {
  for (uint64_t i = 1; i <= 5; ++i) {
    Phase phase = (i == 1) ? Phase::kAdd : Phase::kChange;
    InternalTouchEvent event = CreateTouchEvent(phase);
    touch_source_->UpdateStream(kSnapshot, kStreamId, std::move(event), /*is_end_of_stream*/ false,
                                {});
  }
  RunLoopUntilIdle();

  EXPECT_EQ(event_handler_.received_events_.size(), 5u);
  EXPECT_EQ(event_handler_.last_event_stamp_, 5u);

  // Send ACK for stamp 4.
  auto ack_result = (*client_)->AcknowledgeEvents({4});
  EXPECT_TRUE(ack_result.is_ok());
  RunLoopUntilIdle();
  EXPECT_FALSE(internal_error_handler_fired_);

  // Send earlier ACK for stamp 3.
  ack_result = (*client_)->AcknowledgeEvents({3});
  EXPECT_TRUE(ack_result.is_ok());
  RunLoopUntilIdle();
  EXPECT_TRUE(internal_error_handler_fired_);
}

TEST_F(TouchSourceV2Test, EndContest_AwardedWin_EmitsGranted) {
  InternalTouchEvent add_event = CreateTouchEvent(Phase::kAdd);
  touch_source_->UpdateStream(kSnapshot, kStreamId, std::move(add_event),
                              /*is_end_of_stream*/ false, {});
  RunLoopUntilIdle();

  ASSERT_EQ(event_handler_.received_events_.size(), 1u);
  EXPECT_FALSE(event_handler_.received_events_[0].interaction_result().has_value());

  touch_source_->EndContest(kStreamId, /*awarded_win=*/true);
  RunLoopUntilIdle();

  ASSERT_EQ(event_handler_.received_events_.size(), 2u);
  ASSERT_TRUE(event_handler_.received_events_[1].interaction_result().has_value());
  EXPECT_EQ(event_handler_.received_events_[1].interaction_result()->status(),
            fuchsia_ui_pointer::TouchInteractionStatus::kGranted);
}

TEST_F(TouchSourceV2Test, EndContest_Denied_EmitsDeniedAndCleansUp) {
  InternalTouchEvent add_event = CreateTouchEvent(Phase::kAdd);
  touch_source_->UpdateStream(kSnapshot, kStreamId, std::move(add_event),
                              /*is_end_of_stream*/ false, {});
  RunLoopUntilIdle();

  ASSERT_EQ(event_handler_.received_events_.size(), 1u);
  EXPECT_FALSE(event_handler_.received_events_[0].interaction_result().has_value());

  touch_source_->EndContest(kStreamId, /*awarded_win=*/false);
  RunLoopUntilIdle();

  ASSERT_EQ(event_handler_.received_events_.size(), 2u);
  ASSERT_TRUE(event_handler_.received_events_[1].interaction_result().has_value());
  EXPECT_EQ(event_handler_.received_events_[1].interaction_result()->status(),
            fuchsia_ui_pointer::TouchInteractionStatus::kDenied);
}

TEST_F(TouchSourceV2Test, EndContest_WinBeforeFirstUpdateStream) {
  touch_source_->EndContest(kStreamId, /*awarded_win=*/true);

  InternalTouchEvent add_event = CreateTouchEvent(Phase::kAdd);
  touch_source_->UpdateStream(kSnapshot, kStreamId, std::move(add_event),
                              /*is_end_of_stream*/ false, {});
  RunLoopUntilIdle();

  ASSERT_EQ(event_handler_.received_events_.size(), 1u);
  ASSERT_TRUE(event_handler_.received_events_[0].interaction_result().has_value());
  EXPECT_EQ(event_handler_.received_events_[0].interaction_result()->status(),
            fuchsia_ui_pointer::TouchInteractionStatus::kGranted);
}

TEST_F(TouchSourceV2Test, FlowControl_BufferOverflow_ClosesChannel) {
  // Push kMaxUnacknowledgedEvents (150) events to fill available credit.
  for (uint64_t i = 1; i <= TouchSourceV2::kMaxUnacknowledgedEvents; ++i) {
    Phase phase = (i == 1) ? Phase::kAdd : Phase::kChange;
    InternalTouchEvent event = CreateTouchEvent(phase);
    touch_source_->UpdateStream(kSnapshot, kStreamId, std::move(event), /*is_end_of_stream*/ false,
                                {});
  }
  RunLoopUntilIdle();
  EXPECT_EQ(event_handler_.received_events_.size(), TouchSourceV2::kMaxUnacknowledgedEvents);
  EXPECT_FALSE(internal_error_handler_fired_);

  // Now push kMaxBufferedEvents (1000) more events while unacknowledged.
  // Channel should remain open.
  for (size_t i = 0; i < TouchSourceV2::kMaxBufferedEvents; ++i) {
    InternalTouchEvent event = CreateTouchEvent(Phase::kChange);
    touch_source_->UpdateStream(kSnapshot, kStreamId, std::move(event), /*is_end_of_stream*/ false,
                                {});
  }
  RunLoopUntilIdle();
  EXPECT_FALSE(internal_error_handler_fired_);

  // Push 1 more event exceeding the buffer capacity. Channel should close.
  InternalTouchEvent event = CreateTouchEvent(Phase::kChange);
  touch_source_->UpdateStream(kSnapshot, kStreamId, std::move(event), /*is_end_of_stream*/ false,
                              {});
  RunLoopUntilIdle();
  EXPECT_TRUE(internal_error_handler_fired_);
}

TEST_F(TouchSourceV2Test, TouchDeviceInfo_SentOncePerDevice) {
  constexpr uint32_t kDevice1 = 1;
  constexpr uint32_t kDevice2 = 2;

  // Stream 1 from Device 1 (ADD)
  InternalTouchEvent event1 = CreateTouchEvent(Phase::kAdd);
  event1.device_id = kDevice1;
  touch_source_->UpdateStream(kSnapshot, 1, std::move(event1), /*is_end_of_stream*/ false, {});

  // Stream 1 from Device 1 (CHANGE)
  InternalTouchEvent event2 = CreateTouchEvent(Phase::kChange);
  event2.device_id = kDevice1;
  touch_source_->UpdateStream(kSnapshot, 1, std::move(event2), /*is_end_of_stream*/ false, {});

  // Stream 2 from Device 2 (ADD)
  InternalTouchEvent event3 = CreateTouchEvent(Phase::kAdd);
  event3.device_id = kDevice2;
  touch_source_->UpdateStream(kSnapshot, 2, std::move(event3), /*is_end_of_stream*/ false, {});

  RunLoopUntilIdle();

  ASSERT_EQ(event_handler_.received_events_.size(), 3u);
  // Event 1 has device_info for Device 1
  ASSERT_TRUE(event_handler_.received_events_[0].device_info().has_value());
  EXPECT_EQ(event_handler_.received_events_[0].device_info()->id(), kDevice1);

  // Event 2 does NOT have device_info
  EXPECT_FALSE(event_handler_.received_events_[1].device_info().has_value());

  // Event 3 has device_info for Device 2
  ASSERT_TRUE(event_handler_.received_events_[2].device_info().has_value());
  EXPECT_EQ(event_handler_.received_events_[2].device_info()->id(), kDevice2);
}

TEST_F(TouchSourceV2Test, ViewParameters_DeliveredOnFirstEventAndOnBoundsChange) {
  view_tree::BoundingBox bounds1{.min = {0, 0}, .max = {100, 100}};
  view_tree::BoundingBox bounds2{.min = {0, 0}, .max = {200, 200}};

  // Event 1: First event should include view_parameters.
  InternalTouchEvent event1 = CreateTouchEvent(Phase::kAdd);
  touch_source_->UpdateStream(kSnapshot, kStreamId, std::move(event1), /*is_end_of_stream*/ false,
                              bounds1);

  // Event 2: Same bounds, should NOT include view_parameters.
  InternalTouchEvent event2 = CreateTouchEvent(Phase::kChange);
  touch_source_->UpdateStream(kSnapshot, kStreamId, std::move(event2), /*is_end_of_stream*/ false,
                              bounds1);

  // Event 3: Changed bounds, should include view_parameters.
  InternalTouchEvent event3 = CreateTouchEvent(Phase::kChange);
  touch_source_->UpdateStream(kSnapshot, kStreamId, std::move(event3), /*is_end_of_stream*/ false,
                              bounds2);

  RunLoopUntilIdle();

  ASSERT_EQ(event_handler_.received_events_.size(), 3u);
  EXPECT_TRUE(event_handler_.received_events_[0].view_parameters().has_value());
  EXPECT_FALSE(event_handler_.received_events_[1].view_parameters().has_value());
  EXPECT_TRUE(event_handler_.received_events_[2].view_parameters().has_value());
}

TEST_F(TouchSourceV2Test, WakeLease_Forwarded) {
  zx::eventpair lease_token, peer_token;
  ASSERT_EQ(zx::eventpair::create(0, &lease_token, &peer_token), ZX_OK);

  InternalTouchEvent event = CreateTouchEvent(Phase::kAdd);
  event.wake_lease = std::move(lease_token);
  touch_source_->UpdateStream(kSnapshot, kStreamId, std::move(event), /*is_end_of_stream*/ false,
                              {});

  RunLoopUntilIdle();

  ASSERT_EQ(event_handler_.received_events_.size(), 1u);
  EXPECT_TRUE(event_handler_.received_events_[0].wake_lease().has_value());
  EXPECT_TRUE(event_handler_.received_events_[0].wake_lease()->is_valid());
}

TEST_F(TouchSourceV2Test, EventsAfterClose_DoNotCrashOrDoubleClose) {
  // Trigger close with invalid ack.
  auto ack_result = (*client_)->AcknowledgeEvents({999});
  EXPECT_TRUE(ack_result.is_ok());
  RunLoopUntilIdle();
  EXPECT_TRUE(internal_error_handler_fired_);
  received_responses_.clear();

  // Now push many events to verify no double close / crash occurs.
  for (size_t i = 0; i < TouchSourceV2::kMaxBufferedEvents + 10; ++i) {
    InternalTouchEvent event = CreateTouchEvent(Phase::kChange);
    touch_source_->UpdateStream(kSnapshot, kStreamId, std::move(event), /*is_end_of_stream*/ false,
                                {});
  }
  RunLoopUntilIdle();

  // Should have responded kNo for each event to satisfy Gesture Arena contract.
  EXPECT_EQ(received_responses_[kStreamId].size(), TouchSourceV2::kMaxBufferedEvents + 10);
  for (const auto& response : received_responses_[kStreamId]) {
    EXPECT_EQ(response, GestureResponse::kNo);
  }
}

TEST_F(TouchSourceV2Test, SynchronousLoseContestDuringUpdateStream) {
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_pointer::TouchSourceV2>::Create();
  TestTouchSourceV2Client client_event_handler;
  fidl::Client<fuchsia_ui_pointer::TouchSourceV2> client(std::move(client_end), dispatcher(),
                                                         &client_event_handler);

  std::optional<TouchSourceV2> ts;
  ts.emplace(
      dispatcher(), kViewRefKoid, std::move(server_end),
      /*respond*/
      [&ts](StreamId stream_id, const std::vector<GestureResponse>& responses) {
        // Synchronously simulate losing the contest during respond callback.
        ts->EndContest(stream_id, /*awarded_win=*/false);
      },
      /*error_handler*/ [] {}, inspector_);

  InternalTouchEvent add_event = CreateTouchEvent(Phase::kAdd);
  ts->UpdateStream(kSnapshot, kStreamId, std::move(add_event), /*is_end_of_stream=*/false, {});

  RunLoopUntilIdle();

  // The client should receive the ADD event followed by the DENIED interaction result.
  ASSERT_EQ(client_event_handler.received_events_.size(), 2u);
  EXPECT_TRUE(client_event_handler.received_events_[0].pointer_sample().has_value());
  EXPECT_EQ(client_event_handler.received_events_[0].pointer_sample()->phase(),
            fuchsia_ui_pointer::EventPhase::kAdd);
  ASSERT_TRUE(client_event_handler.received_events_[1].interaction_result().has_value());
  EXPECT_EQ(client_event_handler.received_events_[1].interaction_result()->status(),
            fuchsia_ui_pointer::TouchInteractionStatus::kDenied);
}

TEST_F(TouchSourceV2Test, SynchronousLoseContestDuringChangeUpdateStream) {
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_pointer::TouchSourceV2>::Create();
  TestTouchSourceV2Client client_event_handler;
  fidl::Client<fuchsia_ui_pointer::TouchSourceV2> client(std::move(client_end), dispatcher(),
                                                         &client_event_handler);

  bool should_lose_on_change = false;
  std::optional<TouchSourceV2> ts;
  ts.emplace(
      dispatcher(), kViewRefKoid, std::move(server_end),
      /*respond*/
      [&ts, &should_lose_on_change](StreamId stream_id,
                                    const std::vector<GestureResponse>& responses) {
        if (should_lose_on_change) {
          ts->EndContest(stream_id, /*awarded_win=*/false);
        }
      },
      /*error_handler*/ [] {}, inspector_);

  InternalTouchEvent add_event = CreateTouchEvent(Phase::kAdd);
  ts->UpdateStream(kSnapshot, kStreamId, std::move(add_event), /*is_end_of_stream=*/false, {});
  RunLoopUntilIdle();
  ASSERT_EQ(client_event_handler.received_events_.size(), 1u);

  should_lose_on_change = true;
  InternalTouchEvent change_event = CreateTouchEvent(Phase::kChange);
  ts->UpdateStream(kSnapshot, kStreamId, std::move(change_event), /*is_end_of_stream=*/false, {});
  RunLoopUntilIdle();

  // The client should receive the CHANGE event followed by the DENIED interaction result.
  ASSERT_EQ(client_event_handler.received_events_.size(), 3u);
  EXPECT_TRUE(client_event_handler.received_events_[1].pointer_sample().has_value());
  EXPECT_EQ(client_event_handler.received_events_[1].pointer_sample()->phase(),
            fuchsia_ui_pointer::EventPhase::kChange);
  ASSERT_TRUE(client_event_handler.received_events_[2].interaction_result().has_value());
  EXPECT_EQ(client_event_handler.received_events_[2].interaction_result()->status(),
            fuchsia_ui_pointer::TouchInteractionStatus::kDenied);
}

}  // namespace input::test

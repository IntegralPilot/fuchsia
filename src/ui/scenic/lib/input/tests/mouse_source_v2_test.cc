// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/input/mouse_source_v2.h"

#include <fidl/fuchsia.ui.pointer/cpp/fidl.h>
#include <lib/async-testing/test_loop.h>
#include <lib/syslog/cpp/macros.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"

namespace input::test {

using scenic_impl::input::InternalMouseEvent;
using scenic_impl::input::MouseSourceV2;
using scenic_impl::input::StreamId;

constexpr StreamId kStreamId = 1;
constexpr uint32_t kDeviceId = 2;

namespace {

InternalMouseEvent CreateMouseEvent() {
  InternalMouseEvent event;
  event.device_id = kDeviceId;
  event.position_in_viewport = {10.0f, 20.0f};
  event.viewport = {
      .receiver_from_viewport_transform = std::array<float, 9>({1, 0, 0, 0, 1, 0, 0, 0, 1}),
  };
  return event;
}

class TestMouseSourceV2Client : public fidl::AsyncEventHandler<fuchsia_ui_pointer::MouseSourceV2> {
 public:
  void OnMouseEvents(
      fidl::Event<fuchsia_ui_pointer::MouseSourceV2::OnMouseEvents>& event) override {
    received_events_.insert(received_events_.end(), std::make_move_iterator(event.events().begin()),
                            std::make_move_iterator(event.events().end()));
    last_event_stamp_ = event.last_event_stamp();
  }

  void on_fidl_error(fidl::UnbindInfo info) override { unbind_info_ = info; }
  void handle_unknown_event(
      fidl::UnknownEventMetadata<fuchsia_ui_pointer::MouseSourceV2> metadata) override {}

  std::vector<fuchsia_ui_pointer::MouseEvent> received_events_;
  uint64_t last_event_stamp_ = 0;
  std::optional<fidl::UnbindInfo> unbind_info_;
};

}  // namespace

class MouseSourceV2Test : public gtest::TestLoopFixture {
 protected:
  void SetUp() override {
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_pointer::MouseSourceV2>::Create();

    mouse_source_.emplace(
        std::move(server_end),
        /*error_handler*/ [this] { internal_error_handler_fired_ = true; }, dispatcher());

    client_.emplace(std::move(client_end), dispatcher(), &event_handler_);
  }

  bool internal_error_handler_fired_ = false;

  TestMouseSourceV2Client event_handler_;
  std::optional<fidl::Client<fuchsia_ui_pointer::MouseSourceV2>> client_;
  std::optional<MouseSourceV2> mouse_source_;
};

TEST_F(MouseSourceV2Test, PushEventsAndAck) {
  InternalMouseEvent event = CreateMouseEvent();
  mouse_source_->UpdateStream(kStreamId, std::move(event), {}, /*view_exit*/ false);
  RunLoopUntilIdle();

  EXPECT_EQ(event_handler_.received_events_.size(), 1u);
  EXPECT_EQ(event_handler_.last_event_stamp_, 1u);

  auto ack_result = (*client_)->AcknowledgeEvents({1});
  EXPECT_TRUE(ack_result.is_ok());
  RunLoopUntilIdle();

  EXPECT_FALSE(internal_error_handler_fired_);
}

TEST_F(MouseSourceV2Test, FlowControl_BuffersAndFlushesOnAck) {
  for (uint64_t i = 1; i <= MouseSourceV2::kMaxUnacknowledgedEvents; ++i) {
    InternalMouseEvent event = CreateMouseEvent();
    mouse_source_->UpdateStream(kStreamId, std::move(event), {}, /*view_exit*/ false);
  }
  RunLoopUntilIdle();

  EXPECT_EQ(event_handler_.received_events_.size(), MouseSourceV2::kMaxUnacknowledgedEvents);
  EXPECT_EQ(event_handler_.last_event_stamp_, MouseSourceV2::kMaxUnacknowledgedEvents);

  // 151st event is buffered.
  InternalMouseEvent extra_event = CreateMouseEvent();
  mouse_source_->UpdateStream(kStreamId, std::move(extra_event), {}, /*view_exit*/ false);
  RunLoopUntilIdle();

  EXPECT_EQ(event_handler_.received_events_.size(), MouseSourceV2::kMaxUnacknowledgedEvents);

  auto ack_result = (*client_)->AcknowledgeEvents({MouseSourceV2::kMaxUnacknowledgedEvents});
  EXPECT_TRUE(ack_result.is_ok());
  RunLoopUntilIdle();

  EXPECT_EQ(event_handler_.received_events_.size(), MouseSourceV2::kMaxUnacknowledgedEvents + 1);
  EXPECT_EQ(event_handler_.last_event_stamp_, MouseSourceV2::kMaxUnacknowledgedEvents + 1);
}

TEST_F(MouseSourceV2Test, FlowControl_PartialBatchAck) {
  // Push 10 events (all sent in 1 batch, stamps 1..10).
  for (uint64_t i = 1; i <= 10; ++i) {
    InternalMouseEvent event = CreateMouseEvent();
    mouse_source_->UpdateStream(kStreamId, std::move(event), {}, /*view_exit*/ false);
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

TEST_F(MouseSourceV2Test, ViewExitEvent_RequiresAck) {
  InternalMouseEvent event = CreateMouseEvent();
  mouse_source_->UpdateStream(kStreamId, std::move(event), {}, /*view_exit*/ false);
  RunLoopUntilIdle();
  EXPECT_EQ(event_handler_.received_events_.size(), 1u);

  // Send view exit.
  InternalMouseEvent exit_event = CreateMouseEvent();
  mouse_source_->UpdateStream(kStreamId, std::move(exit_event), {}, /*view_exit*/ true);
  RunLoopUntilIdle();
  EXPECT_EQ(event_handler_.received_events_.size(), 2u);
  ASSERT_TRUE(event_handler_.received_events_.back().stream_info().has_value());
  EXPECT_EQ(event_handler_.received_events_.back().stream_info()->status(),
            fuchsia_ui_pointer::MouseViewStatus::kExited);

  // Client acknowledges events.
  auto ack_result = (*client_)->AcknowledgeEvents({2});
  EXPECT_TRUE(ack_result.is_ok());
  RunLoopUntilIdle();

  EXPECT_FALSE(internal_error_handler_fired_);
}

TEST_F(MouseSourceV2Test, InvalidAck_ClosesChannel) {
  InternalMouseEvent event = CreateMouseEvent();
  mouse_source_->UpdateStream(kStreamId, std::move(event), {}, /*view_exit*/ false);
  RunLoopUntilIdle();

  auto ack_result = (*client_)->AcknowledgeEvents({999});
  EXPECT_TRUE(ack_result.is_ok());
  RunLoopUntilIdle();

  EXPECT_TRUE(internal_error_handler_fired_);
}

TEST_F(MouseSourceV2Test, DuplicateOrEarlierAck_ClosesChannel) {
  for (uint64_t i = 1; i <= 5; ++i) {
    InternalMouseEvent event = CreateMouseEvent();
    mouse_source_->UpdateStream(kStreamId, std::move(event), {}, /*view_exit*/ false);
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

TEST_F(MouseSourceV2Test, EarlierAck_ClosesChannel) {
  for (uint64_t i = 1; i <= 5; ++i) {
    InternalMouseEvent event = CreateMouseEvent();
    mouse_source_->UpdateStream(kStreamId, std::move(event), {}, /*view_exit*/ false);
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

TEST_F(MouseSourceV2Test, FlowControl_BufferOverflow_ClosesChannel) {
  // Push kMaxUnacknowledgedEvents (150) events to fill available credit.
  for (uint64_t i = 1; i <= MouseSourceV2::kMaxUnacknowledgedEvents; ++i) {
    InternalMouseEvent event = CreateMouseEvent();
    mouse_source_->UpdateStream(kStreamId, std::move(event), {}, /*view_exit*/ false);
  }
  RunLoopUntilIdle();
  EXPECT_EQ(event_handler_.received_events_.size(), MouseSourceV2::kMaxUnacknowledgedEvents);
  EXPECT_FALSE(internal_error_handler_fired_);

  // Now push kMaxBufferedEvents (1000) more events while unacknowledged.
  // Channel should remain open.
  for (size_t i = 0; i < MouseSourceV2::kMaxBufferedEvents; ++i) {
    InternalMouseEvent event = CreateMouseEvent();
    mouse_source_->UpdateStream(kStreamId, std::move(event), {}, /*view_exit*/ false);
  }
  RunLoopUntilIdle();
  EXPECT_FALSE(internal_error_handler_fired_);

  // Push 1 more event exceeding the buffer capacity. Channel should close.
  InternalMouseEvent event = CreateMouseEvent();
  mouse_source_->UpdateStream(kStreamId, std::move(event), {}, /*view_exit*/ false);
  RunLoopUntilIdle();
  EXPECT_TRUE(internal_error_handler_fired_);
}

TEST_F(MouseSourceV2Test, MouseDeviceInfo_SentOncePerDevice) {
  constexpr uint32_t kDevice1 = 1;
  constexpr uint32_t kDevice2 = 2;

  // Stream 1 from Device 1
  InternalMouseEvent event1 = CreateMouseEvent();
  event1.device_id = kDevice1;
  mouse_source_->UpdateStream(1, std::move(event1), {}, /*view_exit*/ false);

  // Stream 1 second event from Device 1
  InternalMouseEvent event2 = CreateMouseEvent();
  event2.device_id = kDevice1;
  mouse_source_->UpdateStream(1, std::move(event2), {}, /*view_exit*/ false);

  // Stream 2 from Device 2
  InternalMouseEvent event3 = CreateMouseEvent();
  event3.device_id = kDevice2;
  mouse_source_->UpdateStream(2, std::move(event3), {}, /*view_exit*/ false);

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

TEST_F(MouseSourceV2Test, ViewParameters_DeliveredOnFirstEventAndOnBoundsChange) {
  view_tree::BoundingBox bounds1{.min = {0, 0}, .max = {100, 100}};
  view_tree::BoundingBox bounds2{.min = {0, 0}, .max = {200, 200}};

  // Event 1: First event should include view_parameters.
  InternalMouseEvent event1 = CreateMouseEvent();
  mouse_source_->UpdateStream(kStreamId, std::move(event1), bounds1, /*view_exit*/ false);

  // Event 2: Same bounds, should NOT include view_parameters.
  InternalMouseEvent event2 = CreateMouseEvent();
  mouse_source_->UpdateStream(kStreamId, std::move(event2), bounds1, /*view_exit*/ false);

  // Event 3: Changed bounds, should include view_parameters.
  InternalMouseEvent event3 = CreateMouseEvent();
  mouse_source_->UpdateStream(kStreamId, std::move(event3), bounds2, /*view_exit*/ false);

  RunLoopUntilIdle();

  ASSERT_EQ(event_handler_.received_events_.size(), 3u);
  EXPECT_TRUE(event_handler_.received_events_[0].view_parameters().has_value());
  EXPECT_FALSE(event_handler_.received_events_[1].view_parameters().has_value());
  EXPECT_TRUE(event_handler_.received_events_[2].view_parameters().has_value());
}

TEST_F(MouseSourceV2Test, StreamInfo_EnteredAndExited) {
  // First event in stream should have stream_info ENTERED.
  InternalMouseEvent event1 = CreateMouseEvent();
  mouse_source_->UpdateStream(kStreamId, std::move(event1), {}, /*view_exit*/ false);

  // Second event in same stream should NOT have stream_info.
  InternalMouseEvent event2 = CreateMouseEvent();
  mouse_source_->UpdateStream(kStreamId, std::move(event2), {}, /*view_exit*/ false);

  // View exit event should have stream_info EXITED.
  InternalMouseEvent event3 = CreateMouseEvent();
  mouse_source_->UpdateStream(kStreamId, std::move(event3), {}, /*view_exit*/ true);

  RunLoopUntilIdle();

  ASSERT_EQ(event_handler_.received_events_.size(), 3u);
  ASSERT_TRUE(event_handler_.received_events_[0].stream_info().has_value());
  EXPECT_EQ(event_handler_.received_events_[0].stream_info()->status(),
            fuchsia_ui_pointer::MouseViewStatus::kEntered);

  EXPECT_FALSE(event_handler_.received_events_[1].stream_info().has_value());

  ASSERT_TRUE(event_handler_.received_events_[2].stream_info().has_value());
  EXPECT_EQ(event_handler_.received_events_[2].stream_info()->status(),
            fuchsia_ui_pointer::MouseViewStatus::kExited);
}

TEST_F(MouseSourceV2Test, WakeLease_Forwarded) {
  zx::eventpair lease_token, peer_token;
  ASSERT_EQ(zx::eventpair::create(0, &lease_token, &peer_token), ZX_OK);

  InternalMouseEvent event = CreateMouseEvent();
  event.wake_lease = std::move(lease_token);
  mouse_source_->UpdateStream(kStreamId, std::move(event), {}, /*view_exit*/ false);

  RunLoopUntilIdle();

  ASSERT_EQ(event_handler_.received_events_.size(), 1u);
  EXPECT_TRUE(event_handler_.received_events_[0].wake_lease().has_value());
  EXPECT_TRUE(event_handler_.received_events_[0].wake_lease()->is_valid());
}

TEST_F(MouseSourceV2Test, WakeLease_ForwardedOnViewExit) {
  // First enter the stream.
  mouse_source_->UpdateStream(kStreamId, CreateMouseEvent(), {}, /*view_exit*/ false);
  RunLoopUntilIdle();
  event_handler_.received_events_.clear();

  zx::eventpair lease_token, peer_token;
  ASSERT_EQ(zx::eventpair::create(0, &lease_token, &peer_token), ZX_OK);

  InternalMouseEvent event = CreateMouseEvent();
  event.wake_lease = std::move(lease_token);
  mouse_source_->UpdateStream(kStreamId, std::move(event), {}, /*view_exit*/ true);

  RunLoopUntilIdle();

  ASSERT_EQ(event_handler_.received_events_.size(), 1u);
  ASSERT_TRUE(event_handler_.received_events_[0].stream_info().has_value());
  EXPECT_EQ(event_handler_.received_events_[0].stream_info()->status(),
            fuchsia_ui_pointer::MouseViewStatus::kExited);
  EXPECT_TRUE(event_handler_.received_events_[0].wake_lease().has_value());
  EXPECT_TRUE(event_handler_.received_events_[0].wake_lease()->is_valid());
}

TEST_F(MouseSourceV2Test, EventsAfterClose_DoNotCrashOrDoubleClose) {
  // Trigger close with invalid ack.
  auto ack_result = (*client_)->AcknowledgeEvents({999});
  EXPECT_TRUE(ack_result.is_ok());
  RunLoopUntilIdle();
  EXPECT_TRUE(internal_error_handler_fired_);

  // Now push many events to verify no double close / crash occurs.
  for (size_t i = 0; i < MouseSourceV2::kMaxBufferedEvents + 10; ++i) {
    InternalMouseEvent event = CreateMouseEvent();
    mouse_source_->UpdateStream(kStreamId, std::move(event), {}, /*view_exit*/ false);
  }
  RunLoopUntilIdle();
}

}  // namespace input::test

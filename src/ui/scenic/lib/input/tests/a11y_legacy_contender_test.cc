// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/input/a11y_legacy_contender.h"

#include <gmock/gmock.h>
#include <gtest/gtest.h>

namespace lib_ui_input_tests {
namespace {

using fuchsia_ui_input_accessibility::EventHandling;
using scenic_impl::input::A11yLegacyContender;
using scenic_impl::input::GestureResponse;
using scenic_impl::input::InternalTouchEvent;
using scenic_impl::input::StreamId;

constexpr view_tree::BoundingBox kViewBoundsEmpty{};
const view_tree::Snapshot kSnapshot{};
constexpr bool kStreamOngoing = false;
constexpr bool kStreamEnding = true;

TEST(A11yLegacyContenderTest, SingleStream_ConsumedAtSweep) {
  constexpr StreamId kId1 = 1;
  constexpr uint32_t kPointerId1 = 4;
  std::vector<GestureResponse> responses;
  std::vector<InternalTouchEvent> events_sent_to_client;
  scenic_impl::input::GestureContenderInspector inspector(inspect::Node{});
  auto contender = A11yLegacyContender(
      /*respond*/
      [&responses](StreamId id, GestureResponse response) { responses.push_back(response); },
      /*deliver_events_to_client*/
      [&events_sent_to_client](const view_tree::Snapshot&, const InternalTouchEvent& event) {
        events_sent_to_client.emplace_back(event.ShallowClone());
      },
      inspector);

  // Start a stream. No events shuld get responses until the client makes a decision,
  // and all events should be forwarded to client.
  EXPECT_EQ(events_sent_to_client.size(), 0u);
  {
    InternalTouchEvent event;
    event.pointer_id = kPointerId1;
    contender.UpdateStream(kSnapshot, kId1, std::move(event), kStreamOngoing, kViewBoundsEmpty);
  }
  EXPECT_EQ(events_sent_to_client.size(), 1u);
  EXPECT_TRUE(responses.empty());
  {
    InternalTouchEvent event;
    event.pointer_id = kPointerId1;
    contender.UpdateStream(kSnapshot, kId1, std::move(event), kStreamOngoing, kViewBoundsEmpty);
  }
  EXPECT_EQ(events_sent_to_client.size(), 2u);
  EXPECT_TRUE(responses.empty());
  {
    InternalTouchEvent event;
    event.pointer_id = kPointerId1;
    contender.UpdateStream(kSnapshot, kId1, std::move(event), kStreamEnding, kViewBoundsEmpty);
  }
  EXPECT_EQ(events_sent_to_client.size(), 3u);
  EXPECT_TRUE(responses.empty());

  contender.OnStreamHandled(kPointerId1, EventHandling::kConsumed);
  ASSERT_EQ(responses.size(), 3u);
  EXPECT_THAT(responses, testing::Each(GestureResponse::kYesPrioritize));

  // Award the win. Expect no more responses.
  responses.clear();
  events_sent_to_client.clear();
  contender.EndContest(kId1, /*awarded_win*/ true);
  EXPECT_TRUE(events_sent_to_client.empty());
  EXPECT_TRUE(responses.empty());
}

TEST(A11yLegacyContenderTest, SingleStream_ConsumedMidContest) {
  constexpr StreamId kId1 = 1;
  constexpr uint32_t kPointerId1 = 4;
  std::vector<GestureResponse> responses;
  std::vector<InternalTouchEvent> events_sent_to_client;
  scenic_impl::input::GestureContenderInspector inspector(inspect::Node{});
  auto contender = A11yLegacyContender(
      /*respond*/
      [&responses](StreamId id, GestureResponse response) { responses.push_back(response); },
      /*deliver_events_to_client*/
      [&events_sent_to_client](const view_tree::Snapshot&, const InternalTouchEvent& event) {
        events_sent_to_client.emplace_back(event.ShallowClone());
      },
      inspector);

  // Start a stream. No events should get responses until the client makes a decision,
  // and all events should be forwarded to client.
  EXPECT_EQ(events_sent_to_client.size(), 0u);
  {
    InternalTouchEvent event;
    event.pointer_id = kPointerId1;
    contender.UpdateStream(kSnapshot, kId1, std::move(event), kStreamOngoing, kViewBoundsEmpty);
  }
  {
    InternalTouchEvent event;
    event.pointer_id = kPointerId1;
    contender.UpdateStream(kSnapshot, kId1, std::move(event), kStreamOngoing, kViewBoundsEmpty);
  }
  EXPECT_EQ(events_sent_to_client.size(), 2u);
  EXPECT_TRUE(responses.empty());

  // Since the stream hasn't ended yet we're not at sweep, but the YES_PRIORITIZE response is sent
  // immediately.
  contender.OnStreamHandled(kPointerId1, EventHandling::kConsumed);
  ASSERT_EQ(responses.size(), 2u);
  EXPECT_THAT(responses, testing::Each(GestureResponse::kYesPrioritize));

  // Subsequent events should have a YES response.
  {
    InternalTouchEvent event;
    event.pointer_id = kPointerId1;
    contender.UpdateStream(kSnapshot, kId1, std::move(event), kStreamOngoing, kViewBoundsEmpty);
  }
  ASSERT_EQ(responses.size(), 3u);
  EXPECT_EQ(responses[2], GestureResponse::kYesPrioritize);

  // Award the win. Expect no responses on subsequent events.
  responses.clear();
  events_sent_to_client.clear();
  contender.EndContest(kId1, /*awarded_win*/ true);
  {
    InternalTouchEvent event;
    event.pointer_id = kPointerId1;
    contender.UpdateStream(kSnapshot, kId1, std::move(event), kStreamOngoing, kViewBoundsEmpty);
  }
  {
    InternalTouchEvent event;
    event.pointer_id = kPointerId1;
    contender.UpdateStream(kSnapshot, kId1, std::move(event), kStreamEnding, kViewBoundsEmpty);
  }
  EXPECT_EQ(events_sent_to_client.size(), 2u);
  EXPECT_TRUE(responses.empty());
}

TEST(A11yLegacyContenderTest, SingleStream_Rejected) {
  constexpr StreamId kId1 = 1;
  constexpr uint32_t kPointerId1 = 4;
  std::vector<GestureResponse> responses;
  std::vector<InternalTouchEvent> events_sent_to_client;
  scenic_impl::input::GestureContenderInspector inspector(inspect::Node{});
  auto contender = A11yLegacyContender(
      /*respond*/
      [&responses](StreamId id, GestureResponse response) { responses.push_back(response); },
      /*deliver_events_to_client*/
      [&events_sent_to_client](const view_tree::Snapshot&, const InternalTouchEvent& event) {
        events_sent_to_client.emplace_back(event.ShallowClone());
      },
      inspector);

  {
    InternalTouchEvent event;
    event.pointer_id = kPointerId1;
    contender.UpdateStream(kSnapshot, kId1, std::move(event), kStreamOngoing, kViewBoundsEmpty);
  }
  {
    InternalTouchEvent event;
    event.pointer_id = kPointerId1;
    contender.UpdateStream(kSnapshot, kId1, std::move(event), kStreamOngoing, kViewBoundsEmpty);
  }
  EXPECT_EQ(events_sent_to_client.size(), 2u);
  EXPECT_TRUE(responses.empty());

  // On rejection we should get single NO response.
  contender.OnStreamHandled(kPointerId1, EventHandling::kRejected);
  ASSERT_EQ(responses.size(), 1u);
  EXPECT_EQ(responses[0], GestureResponse::kNo);
}

// Tests that no further responses are sent after the contest ends.
TEST(A11yLegacyContenderTest, ContestEndedOnResponse) {
  constexpr StreamId kId1 = 1;
  constexpr uint32_t kPointerId1 = 4;
  std::vector<GestureResponse> responses;
  std::vector<InternalTouchEvent> events_sent_to_client;
  A11yLegacyContender* contender_ptr;
  scenic_impl::input::GestureContenderInspector inspector(inspect::Node{});
  auto contender = A11yLegacyContender(
      /*respond*/
      [&responses, &contender_ptr](StreamId id, GestureResponse response) {
        responses.push_back(response);
        contender_ptr->EndContest(id, /*awarded_win*/ true);
      },
      /*deliver_events_to_client*/
      [&events_sent_to_client](const view_tree::Snapshot&, const InternalTouchEvent& event) {
        events_sent_to_client.emplace_back(event.ShallowClone());
      },
      inspector);
  contender_ptr = &contender;

  {
    InternalTouchEvent event;
    event.pointer_id = kPointerId1;
    contender.UpdateStream(kSnapshot, kId1, std::move(event), kStreamOngoing, kViewBoundsEmpty);
  }
  {
    InternalTouchEvent event;
    event.pointer_id = kPointerId1;
    contender.UpdateStream(kSnapshot, kId1, std::move(event), kStreamOngoing, kViewBoundsEmpty);
  }
  {
    InternalTouchEvent event;
    event.pointer_id = kPointerId1;
    contender.UpdateStream(kSnapshot, kId1, std::move(event), kStreamOngoing, kViewBoundsEmpty);
  }
  EXPECT_EQ(events_sent_to_client.size(), 3u);
  EXPECT_TRUE(responses.empty());

  // Consume the stream. The win is awarded on the first response, and no further responses
  // should be seen.
  contender.OnStreamHandled(kPointerId1, EventHandling::kConsumed);
  ASSERT_EQ(responses.size(), 1u);
  EXPECT_EQ(responses[0], GestureResponse::kYesPrioritize);

  // Check that events are delivered after contest end.
  events_sent_to_client.clear();
  {
    InternalTouchEvent event;
    event.pointer_id = kPointerId1;
    contender.UpdateStream(kSnapshot, kId1, std::move(event), kStreamOngoing, kViewBoundsEmpty);
  }
  EXPECT_EQ(events_sent_to_client.size(), 1u);
}

TEST(A11yLegacyContenderTest, MultipleStreams) {
  constexpr StreamId kId1 = 1, kId2 = 2, kId3 = 3;
  constexpr uint32_t kPointerId1 = 4, kPointerId2 = 5, kPointerId3 = 6;
  std::unordered_map<StreamId, std::vector<GestureResponse>> responses;
  std::vector<InternalTouchEvent> events_sent_to_client;
  scenic_impl::input::GestureContenderInspector inspector(inspect::Node{});
  auto contender = A11yLegacyContender(
      /*respond*/
      [&responses](StreamId id, GestureResponse response) { responses[id].push_back(response); },
      /*deliver_events_to_client*/
      [&events_sent_to_client](const view_tree::Snapshot&, const InternalTouchEvent& event) {
        events_sent_to_client.emplace_back(event.ShallowClone());
      },
      inspector);

  // Start three streams and make sure they're all handled correctly individually.
  EXPECT_EQ(events_sent_to_client.size(), 0u);
  {
    InternalTouchEvent event;
    event.pointer_id = kPointerId1;
    contender.UpdateStream(kSnapshot, kId1, std::move(event), kStreamOngoing, kViewBoundsEmpty);
  }
  {
    InternalTouchEvent event;
    event.pointer_id = kPointerId1;
    contender.UpdateStream(kSnapshot, kId1, std::move(event), kStreamOngoing, kViewBoundsEmpty);
  }
  EXPECT_EQ(events_sent_to_client.size(), 2u);
  EXPECT_TRUE(responses.empty());

  {
    InternalTouchEvent event;
    event.pointer_id = kPointerId2;
    contender.UpdateStream(kSnapshot, kId2, std::move(event), kStreamOngoing, kViewBoundsEmpty);
  }
  {
    InternalTouchEvent event;
    event.pointer_id = kPointerId2;
    contender.UpdateStream(kSnapshot, kId2, std::move(event), kStreamEnding, kViewBoundsEmpty);
  }
  EXPECT_EQ(events_sent_to_client.size(), 4u);
  EXPECT_TRUE(responses.empty());

  {
    InternalTouchEvent event;
    event.pointer_id = kPointerId3;
    contender.UpdateStream(kSnapshot, kId3, std::move(event), kStreamOngoing, kViewBoundsEmpty);
  }
  {
    InternalTouchEvent event;
    event.pointer_id = kPointerId3;
    contender.UpdateStream(kSnapshot, kId3, std::move(event), kStreamOngoing, kViewBoundsEmpty);
  }
  EXPECT_EQ(events_sent_to_client.size(), 6u);
  EXPECT_TRUE(responses.empty());

  // Now the client decides on all three streams and observe the expected responses.
  events_sent_to_client.clear();
  contender.OnStreamHandled(kPointerId1, EventHandling::kConsumed);
  EXPECT_EQ(responses.size(), 1u);
  ASSERT_EQ(responses[kId1].size(), 2u);
  EXPECT_THAT(responses[kId1], testing::Each(GestureResponse::kYesPrioritize));
  contender.OnStreamHandled(kPointerId2, EventHandling::kConsumed);
  EXPECT_EQ(responses.size(), 2u);
  ASSERT_EQ(responses[kId2].size(), 2u);
  EXPECT_THAT(responses[kId2], testing::Each(GestureResponse::kYesPrioritize));
  contender.OnStreamHandled(kPointerId3, EventHandling::kRejected);
  EXPECT_EQ(responses.size(), 3u);
  ASSERT_EQ(responses[kId3].size(), 1u);
  EXPECT_EQ(responses[kId3][0], GestureResponse::kNo);

  EXPECT_EQ(events_sent_to_client.size(), 0u);

  // End contests 2 and 3.
  contender.EndContest(kId2, /*awarded_win*/ true);
  contender.EndContest(kId3, /*awarded_win*/ false);
  responses.clear();

  // Since streams 2 and 3 ended and lost respectively they should count as new streams if used
  // again.
  {
    InternalTouchEvent event;
    event.pointer_id = kPointerId2;
    contender.UpdateStream(kSnapshot, kId2, std::move(event), kStreamOngoing, kViewBoundsEmpty);
  }
  EXPECT_EQ(events_sent_to_client.size(), 1u);
  EXPECT_TRUE(responses.empty());
  {
    InternalTouchEvent event;
    event.pointer_id = kPointerId3;
    contender.UpdateStream(kSnapshot, kId3, std::move(event), kStreamOngoing, kViewBoundsEmpty);
  }
  EXPECT_EQ(events_sent_to_client.size(), 2u);
  EXPECT_TRUE(responses.empty());

  // Stream 1 should continue to receive YES_PRIORITIZE on each new message, since that stream is
  // still ongoing.
  {
    InternalTouchEvent event;
    event.pointer_id = kPointerId1;
    contender.UpdateStream(kSnapshot, kId1, std::move(event), kStreamOngoing, kViewBoundsEmpty);
  }
  EXPECT_EQ(events_sent_to_client.size(), 3u);
  EXPECT_EQ(responses.size(), 1u);
  EXPECT_EQ(responses[kId1][0], GestureResponse::kYesPrioritize);
}

// This test ensures that the contender can handle receiving multiple streams with the same
// pointer_id before a11y has time to respond.
TEST(A11yLegacyContenderTest, MultipleStreams_WithSamePointer) {
  constexpr StreamId kId1 = 1, kId2 = 2, kId3 = 3;
  constexpr uint32_t kPointerId = 4;
  std::unordered_map<StreamId, std::vector<GestureResponse>> responses;
  scenic_impl::input::GestureContenderInspector inspector(inspect::Node{});
  auto contender = A11yLegacyContender(
      /*respond*/
      [&responses](StreamId id, GestureResponse response) { responses[id].push_back(response); },
      /*deliver_events_to_client*/
      [](const view_tree::Snapshot&, const InternalTouchEvent&) {}, inspector);

  // Create three streams and end them.
  {
    InternalTouchEvent event;
    event.pointer_id = kPointerId;
    contender.UpdateStream(kSnapshot, kId1, std::move(event), kStreamEnding, {});
  }
  {
    InternalTouchEvent event;
    event.pointer_id = kPointerId;
    contender.UpdateStream(kSnapshot, kId2, std::move(event), kStreamEnding, {});
  }
  {
    InternalTouchEvent event;
    event.pointer_id = kPointerId;
    contender.UpdateStream(kSnapshot, kId3, std::move(event), kStreamEnding, {});
  }
  EXPECT_TRUE(responses.empty());

  // Return OnStreamHandled messages for all ongoing streams, but always reuse kPointerId.
  // Observe that each stream gets the correct message.
  contender.OnStreamHandled(kPointerId, EventHandling::kConsumed);
  EXPECT_EQ(responses.size(), 1u);
  ASSERT_EQ(responses[kId1].size(), 1u);
  EXPECT_EQ(responses[kId1][0], GestureResponse::kYesPrioritize);
  contender.OnStreamHandled(kPointerId, EventHandling::kRejected);
  EXPECT_EQ(responses.size(), 2u);
  ASSERT_EQ(responses[kId2].size(), 1u);
  EXPECT_EQ(responses[kId2][0], GestureResponse::kNo);
  contender.OnStreamHandled(kPointerId, EventHandling::kConsumed);
  EXPECT_EQ(responses.size(), 3u);
  ASSERT_EQ(responses[kId3].size(), 1u);
  EXPECT_EQ(responses[kId3][0], GestureResponse::kYesPrioritize);
}

}  // namespace
}  // namespace lib_ui_input_tests

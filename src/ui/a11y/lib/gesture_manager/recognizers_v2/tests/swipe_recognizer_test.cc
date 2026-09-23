// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/ui/pointer/augment/cpp/fidl.h>

#include <memory>

#include <gtest/gtest.h>

#include "fuchsia/ui/pointer/cpp/fidl.h"
#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/ui/a11y/lib/gesture_manager/gesture_util_v2/util.h"
#include "src/ui/a11y/lib/gesture_manager/recognizers_v2/directional_swipe_recognizers.h"
#include "src/ui/a11y/lib/gesture_manager/recognizers_v2/tests/mocks/mock_participation_token.h"
#include "src/ui/a11y/lib/gesture_manager/recognizers_v2/timing_constants.h"
#include "src/ui/a11y/lib/testing/input_v2.h"

#include <glm/glm.hpp>

namespace accessibility_test {
namespace {

using fuchsia::ui::pointer::EventPhase;
using input_v2::AddEvent;
using input_v2::ChangeEvents;

constexpr char kSwipeRecognizerName[] = "test_swipe_recognizer";

class TestSwipeRecognizer : public a11y::recognizers_v2::SwipeRecognizerBase {
 public:
  TestSwipeRecognizer(SwipeGestureCallback callback, uint32_t number_of_fingers)
      : SwipeRecognizerBase(std::move(callback), number_of_fingers,
                            a11y::recognizers_v2::kMaxSwipeDuration, kSwipeRecognizerName) {}

  void set_valid(bool valid) { valid_ = valid; }

  std::string DebugName() const override { return kSwipeRecognizerName; }

 private:
  bool SwipeHasValidSlopeAndDirection(float x_displacement, float y_displacement) const override {
    return valid_;
  }

  bool valid_ = true;
};

template <typename Recognizer>
class SwipeRecognizerTest : public gtest::TestLoopFixture,
                            public testing::WithParamInterface<uint32_t> {
 public:
  SwipeRecognizerTest()
      : recognizer_(
            [this](a11y::gesture_util_v2::GestureContext context) {
              gesture_won_ = true;
              gesture_context_ = std::move(context);
            },
            GetParam()) {}

  bool gesture_won() const { return gesture_won_; }
  const a11y::gesture_util_v2::GestureContext& gesture_context() const { return gesture_context_; }
  Recognizer* recognizer() { return &recognizer_; }
  MockParticipationTokenHandle* token() { return &token_; }

  void SendPointerEvents(const std::vector<input_v2::PointerParams>& events) {
    for (const auto& event : events) {
      SendPointerEvent(event);
    }
  }

  void SendPointerEvent(const input_v2::PointerParams& event) {
    recognizer_.HandleEvent(input_v2::ToTouchEvent(event, 0));
  }

 private:
  MockParticipationTokenHandle token_;

  // recognizer_ must follow token_, since recognizer_ may internally hold a raw pointer
  // to token_.
  Recognizer recognizer_;

  bool gesture_won_ = false;

  a11y::gesture_util_v2::GestureContext gesture_context_;
};

class SwipeRecognizerBaseTest : public SwipeRecognizerTest<TestSwipeRecognizer> {};
class UpSwipeRecognizerTest
    : public SwipeRecognizerTest<a11y::recognizers_v2::UpSwipeGestureRecognizer> {};
class DownSwipeRecognizerTest
    : public SwipeRecognizerTest<a11y::recognizers_v2::DownSwipeGestureRecognizer> {};
class LeftSwipeRecognizerTest
    : public SwipeRecognizerTest<a11y::recognizers_v2::LeftSwipeGestureRecognizer> {};
class RightSwipeRecognizerTest
    : public SwipeRecognizerTest<a11y::recognizers_v2::RightSwipeGestureRecognizer> {};

INSTANTIATE_TEST_SUITE_P(SwipeRecognizerBaseTestWithParams, SwipeRecognizerBaseTest,
                         ::testing::Values(1, 2, 3));
INSTANTIATE_TEST_SUITE_P(UpSwipeRecognizerTestWithParams, UpSwipeRecognizerTest,
                         ::testing::Values(1, 2, 3));
INSTANTIATE_TEST_SUITE_P(DownSwipeRecognizerTestWithParams, DownSwipeRecognizerTest,
                         ::testing::Values(1, 2, 3));
INSTANTIATE_TEST_SUITE_P(LeftSwipeRecognizerTestWithParams, LeftSwipeRecognizerTest,
                         ::testing::Values(1, 2, 3));
INSTANTIATE_TEST_SUITE_P(RightSwipeRecognizerTestWithParams, RightSwipeRecognizerTest,
                         ::testing::Values(1, 2, 3));

TEST_P(SwipeRecognizerBaseTest, Win) {
  recognizer()->OnWin();
  EXPECT_TRUE(gesture_won());
}

TEST_P(SwipeRecognizerBaseTest, Defeat) {
  recognizer()->OnDefeat();
  EXPECT_FALSE(gesture_won());
}

// Tests Gesture Detection failure when less fingers are detected than expected.
// Also covers the case, when REMOVE event is detected before all the ADD events are detected.
// This test case applies only to cases where the number of fingers is more than 1.
TEST_P(SwipeRecognizerBaseTest, RejectLessThanExpectedFinger) {
  if (GetParam() == 1) {
    return;
  }
  recognizer()->OnContestStarted(token()->TakeToken());
  for (uint32_t finger = 0; finger < GetParam() - 1; finger++) {
    SendPointerEvents(AddEvent(finger, {}));
  }
  ASSERT_TRUE(token()->is_held());
  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kUndecided);

  // REMOVE event must be between .375 and .75 NDC from DOWN event for gesture to be considered
  // a swipe.
  for (uint32_t finger = 0; finger < GetParam() - 1; finger++) {
    SendPointerEvent({finger, EventPhase::REMOVE, {0, .7f}});
  }

  EXPECT_FALSE(token()->is_held());
  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kReject);
}

// Tests Gesture Detection failure when more fingers are detected than expected.
TEST_P(SwipeRecognizerBaseTest, RejectMoreThanExpectedFinger) {
  recognizer()->OnContestStarted(token()->TakeToken());
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(AddEvent(finger, {}));
  }

  SendPointerEvent({GetParam() + 1, EventPhase::ADD, {}});
  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kReject);
}

// Test Gesture detection failure when an ADD event for a finger is detected after REMOVE event was
// detected for any other finger.
// This doesn't apply when the number of fingers is 1.
TEST_P(SwipeRecognizerBaseTest, RejectDownEventAfterFirstUp) {
  if (GetParam() == 1) {
    return;
  }
  recognizer()->OnContestStarted(token()->TakeToken());

  // Send ADD events for all but 1 finger.
  for (uint32_t finger = 0; finger < GetParam() - 1; finger++) {
    SendPointerEvents(AddEvent(finger, {}));
  }

  ASSERT_TRUE(token()->is_held());
  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kUndecided);

  // REMOVE event must be between .375 and .75 NDC from DOWN event for gesture to be considered
  // a swipe.
  // Send REMOVE event for the first finger.
  SendPointerEvent({0, EventPhase::REMOVE, {0, .7f}});

  // Send the last ADD event.
  SendPointerEvents(AddEvent(GetParam(), {}));

  EXPECT_FALSE(token()->is_held());
  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kReject);
}

// Test Gesture detection failure when a CHANGE event for a finger is detected before ADD event.
// This doesn't apply when the number of fingers is 1.
TEST_P(SwipeRecognizerBaseTest, RejectMoveEventBeforeDown) {
  if (GetParam() == 1) {
    return;
  }
  recognizer()->OnContestStarted(token()->TakeToken());

  // Send the first ADD event.
  SendPointerEvents(AddEvent(0, {}));

  // Send CHANGE event for the next finger.
  SendPointerEvent({1, EventPhase::CHANGE, {}});

  EXPECT_FALSE(token()->is_held());
  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kReject);
}

TEST_P(SwipeRecognizerBaseTest, Timeout) {
  recognizer()->OnContestStarted(token()->TakeToken());
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(AddEvent(finger, {}));
  }

  RunLoopFor(a11y::recognizers_v2::kMaxSwipeDuration);
  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kReject);
}

TEST_P(SwipeRecognizerBaseTest, NoTimeoutAfterDetected) {
  recognizer()->OnContestStarted(token()->TakeToken());

  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(AddEvent(finger, {}));
  }
  // REMOVE event must be between .375 and .75 NDC from ADD event for gesture to be considered
  // a swipe.
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvent({finger, EventPhase::REMOVE, {0, .7f}});
  }

  // By now, the token has been released (verified in the |Accept| test), so state can no longer
  // change. Wait for the timeout, to make sure the scheduled task doesn't execute and crash us.
  RunLoopFor(a11y::recognizers_v2::kMaxSwipeDuration);
}

// Tests rejection case in which the swipe gesture does not cover long enough distance.
TEST_P(SwipeRecognizerBaseTest, RejectWhenDistanceTooSmall) {
  recognizer()->OnContestStarted(token()->TakeToken());
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(AddEvent(finger, {}));
  }
  // REMOVE event must be between .375 and .75 NDC from ADD event for gesture to be considered
  // a swipe.
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvent({finger, EventPhase::REMOVE, {0, .2f}});
  }

  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kReject);
}

// Ensures that the test recognizer, which considers all swipe paths valid by default, calls
// |Accept| on |REMOVE|. The base recognizer still validates swipe distance.
TEST_P(SwipeRecognizerBaseTest, Accept) {
  recognizer()->OnContestStarted(token()->TakeToken());

  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(AddEvent(finger, {}));
  }

  ASSERT_TRUE(token()->is_held());
  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kUndecided);

  // REMOVE event must be between .375 and .75 NDC from ADD event for gesture to be considered
  // a swipe.
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvent({finger, EventPhase::REMOVE, {0, .7f}});
  }

  EXPECT_FALSE(token()->is_held());
  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kAccept);
}

// Tests case in which swipe gesture covers a large distance. We are using the entire upper range,
// so there is no case where the distance between REMOVE and ADD is more than 1NDC.
TEST_P(SwipeRecognizerBaseTest, AcceptWhenDistanceIsLarge) {
  recognizer()->OnContestStarted(token()->TakeToken());
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(AddEvent(finger, {}));
  }
  // REMOVE event must be between .25 and 1 NDC from ADD event for gesture to be considered
  // a swipe.
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvent({finger, EventPhase::REMOVE, {0, 1}});
  }

  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kAccept);
}

// Tests case in which swipe gesture covers a large distance. We are using the entire upper range,
// so there is no case where the distance between REMOVE and ADD is more than 1NDC.
TEST_P(SwipeRecognizerBaseTest, AcceptWhenOnlyCentroidDistanceIsLarge) {
  recognizer()->OnContestStarted(token()->TakeToken());
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(AddEvent(finger, {}));
  }
  // REMOVE event must be between .25 and 1 NDC from ADD event for gesture to be considered
  // a swipe.
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    // If this is a multi-finger swipe, leave one finger in its original
    // location. In this case, the centroid will have moved by more than the
    // swipe displacement threshold, but one of the fingers will not. In this
    // case, we should still accept.
    if (GetParam() > 1 && finger == 0) {
      SendPointerEvent({finger, EventPhase::REMOVE, {0, 0}});
      continue;
    }

    SendPointerEvent({finger, EventPhase::REMOVE, {0, 1}});
  }

  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kAccept);
}

TEST_P(UpSwipeRecognizerTest, MoveEventAtSameLocationAsDown) {
  recognizer()->OnContestStarted(token()->TakeToken());

  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(AddEvent(finger, {}));
    SendPointerEvent({finger, EventPhase::CHANGE, {}});
  }
  ASSERT_TRUE(token()->is_held());
  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kUndecided);
}

TEST_P(UpSwipeRecognizerTest, GestureDetected) {
  recognizer()->OnContestStarted(token()->TakeToken());
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(AddEvent(finger, {}));
  }
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(ChangeEvents(finger, {}, {0, -.7f}));
  }
  // REMOVE event must be between .375 and .75 NDC from ADD event for gesture to be considered
  // a swipe.
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvent({finger, EventPhase::REMOVE, {0, -.7f}});
  }

  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kAccept);
}

// Test Gesture detection case when a long CHANGE event is detected for a finger after first REMOVE
// event is detected.
// This test is applicable only when number of finger is more than 1.
TEST_P(UpSwipeRecognizerTest, RejectLongMoveEventAfterFirstUp) {
  if (GetParam() == 1) {
    return;
  }

  recognizer()->OnContestStarted(token()->TakeToken());

  // Send all the ADD events.
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(AddEvent(finger, {}));
  }

  // Send all the CHANGE events.
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(ChangeEvents(finger, {}, {0, -.7f}));
  }

  ASSERT_TRUE(token()->is_held());
  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kUndecided);

  // Send first REMOVE event.
  SendPointerEvent({0, EventPhase::REMOVE, {0, -.7f}});

  // Move finger over a larger distance.
  SendPointerEvent({1, EventPhase::CHANGE, {0, -.9f}});

  // Send remaining REMOVE events.
  // REMOVE event must be between .375 and .75 NDC from ADD event for gesture to be considered
  // a swipe.
  for (uint32_t finger = 1; finger < GetParam(); finger++) {
    SendPointerEvent({finger, EventPhase::REMOVE, {0, -.9f}});
  }

  EXPECT_FALSE(token()->is_held());
  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kAccept);
}

TEST_P(DownSwipeRecognizerTest, MoveEventAtSameLocationAsDown) {
  recognizer()->OnContestStarted(token()->TakeToken());
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(AddEvent(finger, {}));
    SendPointerEvent({finger, EventPhase::CHANGE, {}});
  }
  ASSERT_TRUE(token()->is_held());
  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kUndecided);
}

TEST_P(DownSwipeRecognizerTest, GestureDetected) {
  recognizer()->OnContestStarted(token()->TakeToken());
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(AddEvent(finger, {}));
  }
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(ChangeEvents(finger, {}, {0, .7f}));
  }
  // REMOVE event must be between .375 and .75 NDC from ADD event for gesture to be considered
  // a swipe.
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvent({finger, EventPhase::REMOVE, {0, .7f}});
  }

  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kAccept);
}

// Test Gesture detection case when a long CHANGE event is detected for a finger after first REMOVE
// event is detected.
// This test is applicable only when number of finger is more than 1.
TEST_P(DownSwipeRecognizerTest, RejectLongMoveEventAfterFirstUp) {
  if (GetParam() == 1) {
    return;
  }

  recognizer()->OnContestStarted(token()->TakeToken());

  // Send all the ADD events.
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(AddEvent(finger, {}));
  }

  // Send all the CHANGE events.
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(ChangeEvents(finger, {}, {0, .7f}));
  }

  ASSERT_TRUE(token()->is_held());
  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kUndecided);

  // Send first REMOVE event.
  SendPointerEvent({0, EventPhase::REMOVE, {0, .7f}});

  // Move finger over a larger distance.
  SendPointerEvent({1, EventPhase::CHANGE, {0, .9f}});

  // Send remaining REMOVE events.
  // REMOVE event must be between .375 and .75 NDC from ADD event for gesture to be considered
  // a swipe.
  for (uint32_t finger = 1; finger < GetParam(); finger++) {
    SendPointerEvent({finger, EventPhase::REMOVE, {0, .9f}});
  }

  EXPECT_FALSE(token()->is_held());
  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kAccept);
}

TEST_P(RightSwipeRecognizerTest, MoveEventAtSameLocationAsDown) {
  recognizer()->OnContestStarted(token()->TakeToken());
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(AddEvent(finger, {}));
    SendPointerEvent({finger, EventPhase::CHANGE, {}});
  }
  ASSERT_TRUE(token()->is_held());
  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kUndecided);
}

TEST_P(RightSwipeRecognizerTest, GestureDetected) {
  recognizer()->OnContestStarted(token()->TakeToken());
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(AddEvent(finger, {}));
  }
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(ChangeEvents(finger, {}, {.7f, 0}));
  }
  // REMOVE event must be between .375 and .75 NDC from ADD event for gesture to be considered
  // a swipe.
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvent({finger, EventPhase::REMOVE, {.7f, 0}});
  }

  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kAccept);
}

// Test Gesture detection case when a long CHANGE event is detected for a finger after first REMOVE
// event is detected.
// This test is applicable only when number of finger is more than 1.
TEST_P(RightSwipeRecognizerTest, RejectLongMoveEventAfterFirstUp) {
  if (GetParam() == 1) {
    return;
  }

  recognizer()->OnContestStarted(token()->TakeToken());

  // Send all the ADD events.
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(AddEvent(finger, {}));
  }

  // Send all the CHANGE events.
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(ChangeEvents(finger, {}, {.7f, 0}));
  }

  ASSERT_TRUE(token()->is_held());
  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kUndecided);

  // Send first REMOVE event.
  SendPointerEvent({0, EventPhase::REMOVE, {.7f, 0}});

  // Move finger over a larger distance.
  SendPointerEvent({1, EventPhase::CHANGE, {.9f, 0}});

  // Send remaining REMOVE events.
  // REMOVE event must be between .375 and .75 NDC from ADD event for gesture to be considered
  // a swipe.
  for (uint32_t finger = 1; finger < GetParam(); finger++) {
    SendPointerEvent({finger, EventPhase::REMOVE, {.9f, 0}});
  }

  EXPECT_FALSE(token()->is_held());
  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kAccept);
}

TEST_P(LeftSwipeRecognizerTest, MoveEventAtSameLocationAsDown) {
  recognizer()->OnContestStarted(token()->TakeToken());
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(AddEvent(finger, {}));
    SendPointerEvent({finger, EventPhase::CHANGE, {}});
  }
  ASSERT_TRUE(token()->is_held());
  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kUndecided);
}

TEST_P(LeftSwipeRecognizerTest, GestureDetected) {
  recognizer()->OnContestStarted(token()->TakeToken());
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(AddEvent(finger, {}));
  }
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(ChangeEvents(finger, {}, {-.7f, 0}));
  }
  // REMOVE event must be between .375 and .75 NDC from ADD event for gesture to be considered
  // a swipe.
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvent({finger, EventPhase::REMOVE, {-.7f, 0}});
  }

  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kAccept);
}

// Test Gesture detection case when a long CHANGE event is detected for a finger after first REMOVE
// event is detected.
// This test is applicable only when number of finger is more than 1.
TEST_P(LeftSwipeRecognizerTest, RejectLongMoveEventAfterFirstUp) {
  if (GetParam() == 1) {
    return;
  }

  recognizer()->OnContestStarted(token()->TakeToken());

  // Send all the ADD events.
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(AddEvent(finger, {}));
  }

  // Send all the CHANGE events.
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(ChangeEvents(finger, {}, {-.7f, 0}));
  }

  ASSERT_TRUE(token()->is_held());
  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kUndecided);

  // Send first REMOVE event.
  SendPointerEvent({0, EventPhase::REMOVE, {-.7f, 0}});

  // Move finger over a larger distance.
  SendPointerEvent({1, EventPhase::CHANGE, {-.9f, 0}});

  // Send remaining REMOVE events.
  // REMOVE event must be between .375 and .75 NDC from ADD event for gesture to be considered
  // a swipe.
  for (uint32_t finger = 1; finger < GetParam(); finger++) {
    SendPointerEvent({finger, EventPhase::REMOVE, {-.9f, 0}});
  }

  EXPECT_FALSE(token()->is_held());
  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kAccept);
}

// Tests rejection case for upward swipe in which up gesture ends too far from vertical.
TEST_P(UpSwipeRecognizerTest, RejectSwipeOnInvalidEndLocation) {
  recognizer()->OnContestStarted(token()->TakeToken());
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(AddEvent(finger, {}));
  }
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvent({finger, EventPhase::REMOVE, {.5f, -.5f}});
  }

  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kReject);
}

// Tests rejection case for upward swipe in which gesture takes invalid path. Every swipe has cone
// like area in which the gesture is valid. This test is checking that if swipe falls outside of
// this cone then its rejected.
TEST_P(UpSwipeRecognizerTest, RejectSwipeOnInvalidPath) {
  recognizer()->OnContestStarted(token()->TakeToken());
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(AddEvent(finger, {}));
  }
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvent({finger, EventPhase::CHANGE, {0, .3f}});
  }

  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kReject);
}

// Tests rejection case for downward swipe in which gesture ends in an invalid location.
TEST_P(DownSwipeRecognizerTest, RejectSwipeOnInvalidEndLocation) {
  recognizer()->OnContestStarted(token()->TakeToken());
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(AddEvent(finger, {}));
  }
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvent({finger, EventPhase::REMOVE, {-.5f, .5f}});
  }

  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kReject);
}

// Tests rejection case for downward swipe in which gesture takes invalid path. Every swipe has cone
// like area in which the gesture is valid. This test is checking that if swipe falls outside of
// this cone then its rejected.
TEST_P(DownSwipeRecognizerTest, RejectSwipeOnInvalidPath) {
  recognizer()->OnContestStarted(token()->TakeToken());
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(AddEvent(finger, {}));
  }
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvent({finger, EventPhase::CHANGE, {0, -.3f}});
  }

  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kReject);
}

// Tests rejection case for right swipe in which gesture ends in an invalid location.
TEST_P(RightSwipeRecognizerTest, RejectSwipeOnInvalidEndLocation) {
  recognizer()->OnContestStarted(token()->TakeToken());
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(AddEvent(finger, {}));
  }
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvent({finger, EventPhase::REMOVE, {.5f, .5f}});
  }

  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kReject);
}

// Tests rejection case for right swipe in which gesture takes invalid path. Every swipe has cone
// like area in which the gesture is valid. This test is checking that if swipe falls outside of
// this cone then its rejected.
TEST_P(RightSwipeRecognizerTest, RejectSwipeOnInvalidPath) {
  recognizer()->OnContestStarted(token()->TakeToken());
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(AddEvent(finger, {}));
  }
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvent({finger, EventPhase::CHANGE, {-.3f, 0}});
  }

  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kReject);
}

// Tests rejection case for left swipe in which gesture ends in an invalid location.
TEST_P(LeftSwipeRecognizerTest, RejectSwipeOnInvalidEndLocation) {
  recognizer()->OnContestStarted(token()->TakeToken());
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(AddEvent(finger, {}));
  }
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvent({finger, EventPhase::REMOVE, {-.5f, -.5f}});
  }

  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kReject);
}

// Tests rejection case for left swipe in which gesture takes invalid path. Every swipe has cone
// like area in which the gesture is valid. This test is checking that if swipe falls outside of
// this cone then its rejected.
TEST_P(LeftSwipeRecognizerTest, RejectSwipeOnInvalidPath) {
  recognizer()->OnContestStarted(token()->TakeToken());
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvents(AddEvent(finger, {}));
  }
  for (uint32_t finger = 0; finger < GetParam(); finger++) {
    SendPointerEvent({finger, EventPhase::CHANGE, {.3f, 0}});
  }

  EXPECT_EQ(token()->status(), MockParticipationTokenHandle::Status::kReject);
}

}  // namespace
}  // namespace accessibility_test

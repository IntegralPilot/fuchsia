// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/a11y/lib/gesture_manager/recognizers_v2/one_finger_n_tap_recognizer.h"

#include <fuchsia/ui/pointer/augment/cpp/fidl.h>

#include <memory>

#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/ui/a11y/lib/gesture_manager/gesture_util_v2/util.h"
#include "src/ui/a11y/lib/gesture_manager/recognizers_v2/tests/mocks/mock_participation_token.h"
#include "src/ui/a11y/lib/gesture_manager/recognizers_v2/timing_constants.h"
#include "src/ui/a11y/lib/testing/input_v2.h"

namespace accessibility_test {

namespace {

using fuchsia::ui::pointer::EventPhase;
using input_v2::AddEvent;
using input_v2::DragEvents;
using input_v2::TapEvents;

class OneFingerNTapRecognizerTest : public gtest::TestLoopFixture {
 public:
  OneFingerNTapRecognizerTest() = default;

  void SendPointerEvents(const std::vector<input_v2::PointerParams>& events) const {
    for (const auto& event : events) {
      SendPointerEvent(event);
    }
  }

  // Constraints to keep in mind when simulating |GestureArena| behavior:
  // * Only send pointer events while a participation token is held.
  void SendPointerEvent(const input_v2::PointerParams& event) const {
    if (token_.is_held()) {
      recognizer_->HandleEvent(input_v2::ToTouchEvent(event, 0));
    }
  }

  void CreateGestureRecognizer(int number_of_taps) {
    recognizer_ = std::make_unique<a11y::recognizers_v2::OneFingerNTapRecognizer>(
        [this](a11y::gesture_util_v2::GestureContext context) {
          gesture_won_ = true;
          gesture_context_ = std::move(context);
        },
        number_of_taps);
  }

  MockParticipationTokenHandle token_;
  std::unique_ptr<a11y::recognizers_v2::OneFingerNTapRecognizer> recognizer_;
  bool gesture_won_ = false;
  a11y::gesture_util_v2::GestureContext gesture_context_;
};

// Tests Single tap Gesture Detection case.
TEST_F(OneFingerNTapRecognizerTest, SingleTapWonAfterGestureDetected) {
  CreateGestureRecognizer(1);

  recognizer_->OnContestStarted(token_.TakeToken());

  // Send Tap event.
  SendPointerEvents(TapEvents(1, {}));

  EXPECT_FALSE(token_.is_held());
  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kAccept);
}

// Tests Double tap Gesture Detection case.
TEST_F(OneFingerNTapRecognizerTest, DoubleTapWonAfterGestureDetected) {
  CreateGestureRecognizer(2);

  recognizer_->OnContestStarted(token_.TakeToken());

  // Send events for first tap.
  SendPointerEvents(TapEvents(1, {}));

  // Send event for the second tap.
  SendPointerEvents(TapEvents(1, {}));

  EXPECT_FALSE(token_.is_held());
  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kAccept);
}

// Tests Single tap gesture detection case where gesture is declared a winner.
TEST_F(OneFingerNTapRecognizerTest, SingleTapGestureDetectedWin) {
  CreateGestureRecognizer(1);
  recognizer_->OnContestStarted(token_.TakeToken());

  SendPointerEvents(TapEvents(1, {}));
  recognizer_->OnWin();

  EXPECT_TRUE(gesture_won_);

  // Wait for the timeout, to make sure the scheduled task doesn't execute and crash us.
  RunLoopFor(a11y::recognizers_v2::kMaxTapDuration);
}

// Tests Double tap gesture detection case where gesture is declared a winner.
TEST_F(OneFingerNTapRecognizerTest, DoubleTapGestureDetectedWin) {
  CreateGestureRecognizer(2);
  recognizer_->OnContestStarted(token_.TakeToken());

  // Send events for first tap.
  SendPointerEvents(TapEvents(1, {}));

  // Send events for second tap.
  SendPointerEvents(TapEvents(1, {}));

  recognizer_->OnWin();

  EXPECT_TRUE(gesture_won_);

  // Wait for the timeout, to make sure the scheduled task doesn't execute and crash us.
  RunLoopFor(a11y::recognizers_v2::kMaxTapDuration);
}

// Tests Single tap gesture detection case where gesture is declared defeated.
TEST_F(OneFingerNTapRecognizerTest, SingleTapGestureDetectedLoss) {
  CreateGestureRecognizer(1);
  recognizer_->OnContestStarted(token_.TakeToken());

  SendPointerEvents(TapEvents(1, {}));
  recognizer_->OnDefeat();

  EXPECT_FALSE(gesture_won_);

  // Wait for the timeout, to make sure the scheduled task doesn't execute and crash us.
  RunLoopFor(a11y::recognizers_v2::kMaxTapDuration);
}

// Tests Double tap gesture detection case where gesture is declared defeated.
TEST_F(OneFingerNTapRecognizerTest, DoubleTapGestureDetectedLoss) {
  CreateGestureRecognizer(2);
  recognizer_->OnContestStarted(token_.TakeToken());

  // Send events for first tap.
  SendPointerEvents(TapEvents(1, {}));

  // Send events for second tap.
  SendPointerEvents(TapEvents(1, {}));

  recognizer_->OnDefeat();

  EXPECT_FALSE(gesture_won_);

  // Wait for the timeout, to make sure the scheduled task doesn't execute and crash us.
  RunLoopFor(a11y::recognizers_v2::kMaxTapDuration);
}

// Tests Single tap gesture detection failure, where gesture detection times out because of long
// press.
TEST_F(OneFingerNTapRecognizerTest, SingleTapGestureTimeout) {
  CreateGestureRecognizer(1);
  recognizer_->OnContestStarted(token_.TakeToken());

  SendPointerEvents(AddEvent(1, {}));

  // Wait until the timeout, after which the gesture should abandon.
  RunLoopFor(a11y::recognizers_v2::kMaxTapDuration);

  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kReject);
}

// Tests Double tap gesture detection failure, where gesture detection times out because second tap
// doesn't start under timeout_between_taps_.
TEST_F(OneFingerNTapRecognizerTest, DoubleTapGestureTimeoutBetweenTaps) {
  CreateGestureRecognizer(2);
  recognizer_->OnContestStarted(token_.TakeToken());

  // Send events for first tap.
  SendPointerEvents(TapEvents(1, {}));

  // Wait until the timeout, after which the gesture should abandon.
  RunLoopFor(a11y::recognizers_v2::kMaxTapDuration);

  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kReject);
}

// Tests Single tap gesture detection failure when multiple fingers are detected.
TEST_F(OneFingerNTapRecognizerTest, SingleTapMultiFingerDetected) {
  CreateGestureRecognizer(1);
  recognizer_->OnContestStarted(token_.TakeToken());

  SendPointerEvents(AddEvent(1, {}));

  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kUndecided);

  SendPointerEvents(AddEvent(2, {}));

  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kReject);
}

// Tests Double tap gesture detection failure when multiple fingers are detected.
TEST_F(OneFingerNTapRecognizerTest, DoubleTapMultiFingerDetected) {
  CreateGestureRecognizer(2);
  recognizer_->OnContestStarted(token_.TakeToken());

  SendPointerEvents(TapEvents(1, {}));

  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kUndecided);

  SendPointerEvents(AddEvent(2, {}));

  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kReject);
}

// Tests Single tap gesture detection when gesture is preformed with move under the allowed limit.
TEST_F(OneFingerNTapRecognizerTest, SingleTapGestureWithMoveUnderThreshold) {
  CreateGestureRecognizer(1);
  recognizer_->OnContestStarted(token_.TakeToken());

  SendPointerEvents(DragEvents(1, {}, {a11y::gesture_util_v2::kGestureMoveThreshold - .1f, 0}));

  EXPECT_FALSE(token_.is_held());
  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kAccept);
}

// Tests Double tap gesture detection when gesture is preformed with move under the allowed limit.
TEST_F(OneFingerNTapRecognizerTest, DoubleTapGestureWithMoveUnderThreshold) {
  CreateGestureRecognizer(2);
  recognizer_->OnContestStarted(token_.TakeToken());

  SendPointerEvents(TapEvents(1, {}));
  SendPointerEvents(DragEvents(1, {}, {a11y::gesture_util_v2::kGestureMoveThreshold - .1f, 0}));

  EXPECT_FALSE(token_.is_held());
  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kAccept);
}

// Tests Single tap gesture detection failure when gesture is performed over a larger area(something
// like swipe).
TEST_F(OneFingerNTapRecognizerTest, SingleTapGesturePerformedOverLargerArea) {
  CreateGestureRecognizer(1);
  recognizer_->OnContestStarted(token_.TakeToken());

  SendPointerEvents(DragEvents(1, {}, {a11y::gesture_util_v2::kGestureMoveThreshold + .1f, 0}));

  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kReject);
}

// Tests Double tap gesture detection failure when gesture is performed over a larger area(something
// like swipe).
TEST_F(OneFingerNTapRecognizerTest, DoubleTapGesturePerformedOverLargerArea) {
  CreateGestureRecognizer(2);
  recognizer_->OnContestStarted(token_.TakeToken());

  // Send events for first tap.
  SendPointerEvents(TapEvents(1, {}));

  SendPointerEvents(DragEvents(1, {}, {a11y::gesture_util_v2::kGestureMoveThreshold + .1f, 0}));

  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kReject);
}

// Tests Double tap gesture detection case where individual taps are performed at significant
// distance from each other.
TEST_F(OneFingerNTapRecognizerTest, DoubleTapPerformedWithDistantTapsFromEachOther) {
  CreateGestureRecognizer(2);
  recognizer_->OnContestStarted(token_.TakeToken());

  // Send events for first tap.
  SendPointerEvents(TapEvents(1, {0, 0}));

  // Send events for second tap.
  SendPointerEvents(TapEvents(1, {1, 1}));

  EXPECT_FALSE(token_.is_held());
  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kAccept);
}

// This test makes sure that local coordinates are passed correctly through the gesture context to
// the callback.
TEST_F(OneFingerNTapRecognizerTest, RecognizersPassesLocalCoordinatesToCallback) {
  CreateGestureRecognizer(1);
  recognizer_->OnContestStarted(token_.TakeToken());

  auto event = input_v2::ToTouchEvent({1, EventPhase::ADD, {}}, 0);
  event.local_viewref_koid = 100;
  event.local_point = {2, 2};
  recognizer_->HandleEvent(event);
  event.touch_event.mutable_pointer_sample()->set_phase(EventPhase::REMOVE);
  recognizer_->HandleEvent(event);

  recognizer_->OnWin();

  EXPECT_TRUE(gesture_won_);
  EXPECT_EQ(gesture_context_.view_ref_koid, 100u);
  auto& location = gesture_context_.current_pointer_locations[1].local_point;
  EXPECT_EQ(location.x, 2.);
  EXPECT_EQ(location.y, 2.);
}

}  // namespace

}  // namespace accessibility_test

// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/a11y/lib/gesture_manager/recognizers_v2/m_finger_n_tap_recognizer.h"

#include <fuchsia/ui/pointer/augment/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>

#include <memory>

#include <gtest/gtest.h>

#include "fuchsia/ui/pointer/cpp/fidl.h"
#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/ui/a11y/lib/gesture_manager/gesture_util_v2/util.h"
#include "src/ui/a11y/lib/gesture_manager/recognizers_v2/tests/mocks/mock_participation_token.h"
#include "src/ui/a11y/lib/gesture_manager/recognizers_v2/timing_constants.h"
#include "src/ui/a11y/lib/testing/input_v2.h"

namespace accessibility_test {
namespace {

constexpr uint32_t kDefaultFingers = 2;

using fuchsia::ui::pointer::EventPhase;
using input_v2::AddEvent;
using input_v2::DragEvents;
using input_v2::RemoveEvent;
using input_v2::TapEvents;

class MFingerNTapRecognizerTest : public gtest::TestLoopFixture {
 public:
  MFingerNTapRecognizerTest() = default;

  void SendPointerEvents(const std::vector<input_v2::PointerParams>& events) const {
    for (const auto& event : events) {
      SendPointerEvent(event);
    }
  }

  void SendPointerEvent(const input_v2::PointerParams& event) const {
    if (token_.is_held()) {
      recognizer_->HandleEvent(input_v2::ToTouchEvent(event, 0));
    }
  }

  void CreateGestureRecognizer(uint32_t number_of_fingers, uint32_t number_of_taps) {
    recognizer_ = std::make_unique<a11y::recognizers_v2::MFingerNTapRecognizer>(
        [this](a11y::gesture_util_v2::GestureContext context) {
          gesture_won_ = true;
          gesture_context_ = std::move(context);
        },
        number_of_fingers, number_of_taps);
  }

  MockParticipationTokenHandle token_;
  std::unique_ptr<a11y::recognizers_v2::MFingerNTapRecognizer> recognizer_;
  bool gesture_won_ = false;
  a11y::gesture_util_v2::GestureContext gesture_context_;
};

// Tests Single tap Gesture Detection case.
TEST_F(MFingerNTapRecognizerTest, SingleTapWonAfterGestureDetected) {
  CreateGestureRecognizer(kDefaultFingers, 1);

  recognizer_->OnContestStarted(token_.TakeToken());

  // Send two-finger-tap event.
  SendPointerEvents(AddEvent(1, {}) + AddEvent(2, {}) + RemoveEvent(1, {}) + RemoveEvent(2, {}));

  EXPECT_FALSE(token_.is_held());
  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kAccept);
}

// Tests Double tap Gesture Detection case.
TEST_F(MFingerNTapRecognizerTest, DoubleTapWonAfterGestureDetected) {
  CreateGestureRecognizer(kDefaultFingers, 2);

  recognizer_->OnContestStarted(token_.TakeToken());

  // Send events for first tap.
  SendPointerEvents(AddEvent(1, {}) + AddEvent(2, {}) + RemoveEvent(1, {}) + RemoveEvent(2, {}));

  // Send event for the second tap.
  SendPointerEvents(AddEvent(1, {}) + AddEvent(2, {}) + RemoveEvent(1, {}) + RemoveEvent(2, {}));

  EXPECT_FALSE(token_.is_held());
  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kAccept);
}

// Tests Single tap gesture detection case where gesture is declared a winner.
TEST_F(MFingerNTapRecognizerTest, SingleTapGestureDetectedWin) {
  CreateGestureRecognizer(kDefaultFingers, 1);
  recognizer_->OnContestStarted(token_.TakeToken());

  SendPointerEvents(AddEvent(1, {}) + AddEvent(2, {}) + RemoveEvent(1, {}) + RemoveEvent(2, {}));
  recognizer_->OnWin();

  EXPECT_TRUE(gesture_won_);

  // Wait for the timeout, to make sure the scheduled task doesn't execute and crash us.
  RunLoopFor(a11y::recognizers_v2::kMaxTapDuration);
}

// Tests Double tap gesture detection case where gesture is declared a winner.
TEST_F(MFingerNTapRecognizerTest, DoubleTapGestureDetectedWin) {
  CreateGestureRecognizer(kDefaultFingers, 2);
  recognizer_->OnContestStarted(token_.TakeToken());

  // Send events for first tap.
  SendPointerEvents(AddEvent(1, {}) + AddEvent(2, {}) + RemoveEvent(1, {}) + RemoveEvent(2, {}));

  // Send event for the second tap.
  SendPointerEvents(AddEvent(1, {}) + AddEvent(2, {}) + RemoveEvent(1, {}) + RemoveEvent(2, {}));

  recognizer_->OnWin();

  EXPECT_TRUE(gesture_won_);

  // Wait for the timeout, to make sure the scheduled task doesn't execute and crash us.
  RunLoopFor(a11y::recognizers_v2::kMaxTapDuration);
}

// Tests Single tap gesture detection case where gesture is declared defeated.
TEST_F(MFingerNTapRecognizerTest, SingleTapGestureDetectedLoss) {
  CreateGestureRecognizer(kDefaultFingers, 1);
  recognizer_->OnContestStarted(token_.TakeToken());

  // Send events for first tap.
  SendPointerEvents(AddEvent(1, {}) + AddEvent(2, {}) + RemoveEvent(1, {}) + RemoveEvent(2, {}));

  recognizer_->OnDefeat();

  EXPECT_FALSE(gesture_won_);

  // Wait for the timeout, to make sure the scheduled task doesn't execute and crash us.
  RunLoopFor(a11y::recognizers_v2::kMaxTapDuration);
}

// Tests Double tap gesture detection case where gesture is declared defeated.
TEST_F(MFingerNTapRecognizerTest, DoubleTapGestureDetectedLoss) {
  CreateGestureRecognizer(kDefaultFingers, 2);
  recognizer_->OnContestStarted(token_.TakeToken());

  // Send events for first tap.
  SendPointerEvents(AddEvent(1, {}) + AddEvent(2, {}) + RemoveEvent(1, {}) + RemoveEvent(2, {}));

  // Send event for the second tap.
  SendPointerEvents(AddEvent(1, {}) + AddEvent(2, {}) + RemoveEvent(1, {}) + RemoveEvent(2, {}));

  recognizer_->OnDefeat();

  EXPECT_FALSE(gesture_won_);

  // Wait for the timeout, to make sure the scheduled task doesn't execute and crash us.
  RunLoopFor(a11y::recognizers_v2::kMaxTapDuration);
}

// Tests Single tap gesture detection failure, where gesture detection times out because of long
// press.
TEST_F(MFingerNTapRecognizerTest, SingleTapGestureTimeout) {
  CreateGestureRecognizer(kDefaultFingers, 1);
  recognizer_->OnContestStarted(token_.TakeToken());

  SendPointerEvents((AddEvent(1, {}) + AddEvent(2, {})));

  // Wait until the timeout, after which the gesture should abandon.
  RunLoopFor(a11y::recognizers_v2::kMaxTapDuration);

  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kReject);
}

// Tests Double tap gesture detection failure, where gesture detection times out because second tap
// doesn't start under timeout_between_taps_.
TEST_F(MFingerNTapRecognizerTest, DoubleTapGestureTimeoutBetweenTaps) {
  CreateGestureRecognizer(kDefaultFingers, 2);
  recognizer_->OnContestStarted(token_.TakeToken());

  // Send events for first tap.
  SendPointerEvents((AddEvent(1, {}) + AddEvent(2, {}) + RemoveEvent(1, {}) + RemoveEvent(2, {})));

  // Wait until the timeout, after which the gesture should abandon.
  RunLoopFor(a11y::recognizers_v2::kMaxTapDuration);

  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kReject);
}

// Tests Single tap gesture detection failure when too many fingers are detected.
TEST_F(MFingerNTapRecognizerTest, SingleTapThirdFingerDetected) {
  CreateGestureRecognizer(kDefaultFingers, 1);
  recognizer_->OnContestStarted(token_.TakeToken());

  SendPointerEvents(AddEvent(1, {}) + AddEvent(2, {}));

  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kUndecided);

  // Sends a down event with a third pointer ID, causing the gesture to be rejected.
  SendPointerEvent({3, EventPhase::ADD, {}});

  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kReject);
}

// Tests Double tap gesture detection failure when too many fingers are detected.
TEST_F(MFingerNTapRecognizerTest, DoubleTapThirdFingerDetected) {
  CreateGestureRecognizer(kDefaultFingers, 2);
  recognizer_->OnContestStarted(token_.TakeToken());

  // Send events for first tap.
  SendPointerEvents((AddEvent(1, {}) + AddEvent(2, {}) + RemoveEvent(1, {}) + RemoveEvent(2, {})));

  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kUndecided);

  // Sends a down event with a third pointer ID, causing the gesture to be rejected.
  SendPointerEvent({3, EventPhase::ADD, {}});

  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kReject);
}

// Tests Single tap gesture detection when gesture is preformed with move under the allowed limit.
TEST_F(MFingerNTapRecognizerTest, SingleTapGestureWithMoveUnderThreshold) {
  CreateGestureRecognizer(kDefaultFingers, 1);
  recognizer_->OnContestStarted(token_.TakeToken());

  SendPointerEvents(AddEvent(1, {}) +
                    DragEvents(2, {}, {a11y::gesture_util_v2::kGestureMoveThreshold - .1f, 0}) +
                    RemoveEvent(1, {}));

  EXPECT_FALSE(token_.is_held());
  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kAccept);
}

// Tests Single tap gesture detection failure when gesture is performed over a larger area(something
// like swipe).
TEST_F(MFingerNTapRecognizerTest, SingleTapGesturePerformedOverLargerArea) {
  CreateGestureRecognizer(kDefaultFingers, 1);
  recognizer_->OnContestStarted(token_.TakeToken());

  SendPointerEvents(AddEvent(1, {}) +
                    DragEvents(2, {}, {a11y::gesture_util_v2::kGestureMoveThreshold + .1f, 0}));

  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kReject);
}

// Tests Double tap gesture detection case where individual taps are performed at significant
// distance from each other.
TEST_F(MFingerNTapRecognizerTest, DoubleTapPerformedWithDistantTapsFromEachOther) {
  CreateGestureRecognizer(kDefaultFingers, 2);
  recognizer_->OnContestStarted(token_.TakeToken());

  // Send events for first tap.
  SendPointerEvents(AddEvent(1, {0, 0}) + AddEvent(2, {0, 0}) + RemoveEvent(1, {0, 0}) +
                    RemoveEvent(2, {0, 0}));

  // Send event for the second tap.
  SendPointerEvents(AddEvent(1, {1, 1}) + AddEvent(2, {1, 1}) + RemoveEvent(1, {1, 1}) +
                    RemoveEvent(2, {1, 1}));

  EXPECT_FALSE(token_.is_held());
  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kAccept);
}

// This test makes sure that local coordinates are passed correctly through the gesture context to
// the callback.
TEST_F(MFingerNTapRecognizerTest, RecognizersPassesLocalCoordinatesToCallback) {
  constexpr uint64_t koid = 100;

  CreateGestureRecognizer(kDefaultFingers, 1);
  recognizer_->OnContestStarted(token_.TakeToken());

  // Send first finger down event with location specified.
  {
    auto event = input_v2::ToTouchEvent({1, EventPhase::ADD, {0, 0.01}}, 0, koid);
    event.local_point = {1, 2};
    recognizer_->HandleEvent(event);
  }

  // Send second finger down event with a different location than the first.
  // The recognizer should pass the location from this event through to the
  // callback.
  {
    auto event = input_v2::ToTouchEvent({2, EventPhase::ADD, {0.02, 0.03}}, 0, koid);
    event.local_point = {3, 4};
    recognizer_->HandleEvent(event);
  }

  // Send up events.
  {
    auto event = input_v2::ToTouchEvent({1, EventPhase::REMOVE, {0.04, 0.05}}, 0, koid);
    event.local_point = {4, 5};
    recognizer_->HandleEvent(event);
  }
  {
    auto event = input_v2::ToTouchEvent({2, EventPhase::REMOVE, {0.06, 0.07}}, 0, koid);
    event.local_point = {6, 7};
    recognizer_->HandleEvent(event);
  }

  recognizer_->OnWin();

  EXPECT_TRUE(gesture_won_);
  EXPECT_EQ(gesture_context_.view_ref_koid, koid);

  constexpr float e = 0.001f;
  EXPECT_NEAR(gesture_context_.starting_pointer_locations[1].ndc_point.x, 0., e);
  EXPECT_NEAR(gesture_context_.starting_pointer_locations[1].ndc_point.y, 0.01, e);
  EXPECT_NEAR(gesture_context_.starting_pointer_locations[2].ndc_point.x, 0.02, e);
  EXPECT_NEAR(gesture_context_.starting_pointer_locations[2].ndc_point.y, 0.03, e);
  EXPECT_NEAR(gesture_context_.current_pointer_locations[1].ndc_point.x, 0.04, e);
  EXPECT_NEAR(gesture_context_.current_pointer_locations[1].ndc_point.y, 0.05, e);
  EXPECT_NEAR(gesture_context_.current_pointer_locations[2].ndc_point.x, 0.06, e);
  EXPECT_NEAR(gesture_context_.current_pointer_locations[2].ndc_point.y, 0.07, e);
}

TEST_F(MFingerNTapRecognizerTest, LiftAndReplaceSecondFingerIsNotRecognized) {
  CreateGestureRecognizer(kDefaultFingers, 1);
  recognizer_->OnContestStarted(token_.TakeToken());

  // Send events for holding one finger down, and double-tapping with the
  // other finger.
  SendPointerEvents(AddEvent(1, {}) + AddEvent(2, {}) + RemoveEvent(2, {}) + AddEvent(2, {}) +
                    RemoveEvent(1, {}) + RemoveEvent(2, {}));

  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kReject);
}

TEST_F(MFingerNTapRecognizerTest, LiftFingerBeforePlacingSecondFingerOnScreen) {
  CreateGestureRecognizer(kDefaultFingers, 1);
  recognizer_->OnContestStarted(token_.TakeToken());

  // Send one-finger double tap.
  SendPointerEvents(2 * TapEvents(1, {}));

  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kReject);
}

// Tests successful one-finger triple tap gesture detection.
TEST_F(MFingerNTapRecognizerTest, OneFingerTripleTapDetected) {
  CreateGestureRecognizer(1 /*number of fingers*/, 3 /*number of taps*/);
  recognizer_->OnContestStarted(token_.TakeToken());

  // Send events for first tap.
  SendPointerEvents((AddEvent(1, {}) + RemoveEvent(1, {}) + AddEvent(1, {}) + RemoveEvent(1, {}) +
                     AddEvent(1, {}) + RemoveEvent(1, {})));

  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kAccept);
}

// Tests successful three-finger double tap gesture detection.
TEST_F(MFingerNTapRecognizerTest, ThreeFingerDoubleTapDetected) {
  CreateGestureRecognizer(3 /*number of fingers*/, 2 /*number of fingers*/);
  recognizer_->OnContestStarted(token_.TakeToken());

  // Send events for first tap.
  SendPointerEvents((AddEvent(1, {}) + AddEvent(2, {}) + AddEvent(3, {}) + RemoveEvent(1, {}) +
                     RemoveEvent(2, {}) + RemoveEvent(3, {})));

  // Send events for second tap.
  SendPointerEvents((AddEvent(1, {}) + AddEvent(2, {}) + AddEvent(3, {}) + RemoveEvent(1, {}) +
                     RemoveEvent(2, {}) + RemoveEvent(3, {})));

  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kAccept);
}

// Tests tap length timeout.
TEST_F(MFingerNTapRecognizerTest, ThreeFingerDoubleTapRejected) {
  CreateGestureRecognizer(3 /*number of fingers*/, 2 /*number of fingers*/);
  recognizer_->OnContestStarted(token_.TakeToken());

  SendPointerEvents(AddEvent(1, {}));
  RunLoopFor(a11y::recognizers_v2::kMaxTapDuration);

  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kReject);
}

}  // namespace
}  // namespace accessibility_test

// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/a11y/lib/gesture_manager/recognizers_v2/m_finger_n_tap_drag_recognizer.h"

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

using input_v2::AddEvent;
using input_v2::ChangeEvents;
using input_v2::RemoveEvent;

class MFingerNTapDragRecognizerTest : public gtest::TestLoopFixture {
 public:
  MFingerNTapDragRecognizerTest() = default;

  void SendPointerEvents(const std::vector<input_v2::PointerParams>& events) const {
    for (const auto& event : events) {
      SendPointerEvent(event);
    }
  }

  // Constraints to keep in mind when simulating |GestureArena| behavior:
  // * Only send pointer events while a participation token is held.
  void SendPointerEvent(const input_v2::PointerParams& event) const {
    if (token_.is_held()) {
      recognizer_->HandleEvent(ToTouchEvent(event, 0));
    }
  }

  void CreateGestureRecognizer(
      uint32_t number_of_fingers, uint32_t number_of_taps,
      float drag_displacement_threshold =
          a11y::recognizers_v2::MFingerNTapDragRecognizer::kDefaultDragDisplacementThreshold,
      float update_displacement_threshold =
          a11y::recognizers_v2::MFingerNTapDragRecognizer::kDefaultUpdateDisplacementThreshold) {
    recognizer_ = std::make_unique<a11y::recognizers_v2::MFingerNTapDragRecognizer>(
        [this](a11y::gesture_util_v2::GestureContext context) {
          gesture_won_ = true;
          gesture_context_ = std::move(context);
        },
        [this](a11y::gesture_util_v2::GestureContext context) {
          gesture_updates_.push_back(std::move(context));
        },
        [this](a11y::gesture_util_v2::GestureContext context) { gesture_complete_called_ = true; },
        number_of_fingers, number_of_taps, drag_displacement_threshold,
        update_displacement_threshold);
  }

  MockParticipationTokenHandle token_;
  std::unique_ptr<a11y::recognizers_v2::MFingerNTapDragRecognizer> recognizer_;
  bool gesture_won_ = false;
  bool gesture_complete_called_ = false;
  a11y::gesture_util_v2::GestureContext gesture_context_;
  std::vector<a11y::gesture_util_v2::GestureContext> gesture_updates_;
};

// Tests successful three-finger double-tap with drag detection.
TEST_F(MFingerNTapDragRecognizerTest, ThreeFingerDoubleTapWithDragDetected) {
  CreateGestureRecognizer(3 /*number of fingers*/, 2 /*number of fingers*/);
  recognizer_->OnContestStarted(token_.TakeToken());

  // Send events for first tap.
  SendPointerEvents((AddEvent(1, {}) + AddEvent(2, {}) + AddEvent(3, {}) + RemoveEvent(1, {}) +
                     RemoveEvent(2, {}) + RemoveEvent(3, {})));

  // Send events for second tap.
  SendPointerEvents((AddEvent(1, {}) + AddEvent(2, {}) + AddEvent(3, {})));

  RunLoopFor(a11y::recognizers_v2::kMinDragDuration);

  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kAccept);
  recognizer_->OnWin();

  EXPECT_TRUE(gesture_won_);
  EXPECT_FALSE(gesture_complete_called_);

  SendPointerEvents(ChangeEvents(1, {}, {0, .5f}));
  EXPECT_EQ(gesture_updates_.size(), 10u);
  EXPECT_EQ(gesture_updates_[9].current_pointer_locations[1].ndc_point.x, .0f);
  EXPECT_GT(gesture_updates_[9].current_pointer_locations[1].ndc_point.y, .49f);
  EXPECT_LT(gesture_updates_[9].current_pointer_locations[1].ndc_point.y, .51f);

  // We should call on_complete_ after the first REMOVE event received after the
  // gesture was accepted.
  SendPointerEvents(RemoveEvent(1, {}));

  EXPECT_TRUE(gesture_complete_called_);
}

// Tests successful three-finger double-tap indecision with drag detection.
TEST_F(MFingerNTapDragRecognizerTest,
       ThreeFingerDoubleTapWithDragUndecidedNonDefaultDragThreshold) {
  CreateGestureRecognizer(3 /*number of fingers*/, 2 /*number of fingers*/,
                          0.2f /*drag displacement threshold*/);
  recognizer_->OnContestStarted(token_.TakeToken());

  // Send events for first tap.
  SendPointerEvents((AddEvent(1, {}) + AddEvent(2, {}) + AddEvent(3, {}) + RemoveEvent(1, {}) +
                     RemoveEvent(2, {}) + RemoveEvent(3, {})));

  // Send events for second tap. The centroid's displacement should be between
  // the default drag displacement threshold of 0.1f and the specified threshold
  // of 0.2f.
  SendPointerEvents(
      (AddEvent(1, {}) + AddEvent(2, {}) + AddEvent(3, {}) + ChangeEvents(1, {}, {0.45f, 0})));

  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kUndecided);
}

// Tests the case in which a three-finger-double-tap is detected, but the update
// threshold is not met.
TEST_F(MFingerNTapDragRecognizerTest, ThreeFingerDoubleTapWithDragNoUpdatesUntilThresholdExceeded) {
  CreateGestureRecognizer(3 /*number of fingers*/, 2 /*number of fingers*/,
                          0.1f /*drag displacement threshold*/,
                          0.5f /*update displacement threshold*/);
  recognizer_->OnContestStarted(token_.TakeToken());

  // Send events for first tap.
  SendPointerEvents((AddEvent(1, {}) + AddEvent(2, {}) + AddEvent(3, {}) + RemoveEvent(1, {}) +
                     RemoveEvent(2, {}) + RemoveEvent(3, {})));

  // Send events for second tap.
  SendPointerEvents(
      (AddEvent(1, {}) + AddEvent(2, {}) + AddEvent(3, {}) + ChangeEvents(1, {}, {0.5f, 0})));

  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kAccept);
  recognizer_->OnWin();

  EXPECT_TRUE(gesture_won_);
  EXPECT_FALSE(gesture_complete_called_);

  // Move across a displacement that does NOT exceed the update threshold.
  SendPointerEvents(ChangeEvents(2, {}, {0.1f, 0}));

  // No updates should have been received.
  EXPECT_TRUE(gesture_updates_.empty());
}

// Tests rejection of drag that doesn't last long enough.
TEST_F(MFingerNTapDragRecognizerTest, ThreeFingerDoubleTapWithDragRejected) {
  CreateGestureRecognizer(3 /*number of fingers*/, 2 /*number of fingers*/);
  recognizer_->OnContestStarted(token_.TakeToken());

  // Send events for first tap.
  SendPointerEvents((AddEvent(1, {}) + AddEvent(2, {}) + AddEvent(3, {}) + RemoveEvent(1, {}) +
                     RemoveEvent(2, {}) + RemoveEvent(3, {})));

  // Send events for second tap.
  SendPointerEvents((AddEvent(1, {}) + AddEvent(2, {}) + AddEvent(3, {}) + RemoveEvent(1, {}) +
                     RemoveEvent(2, {}) + RemoveEvent(3, {})));

  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kReject);
  EXPECT_FALSE(gesture_won_);
  EXPECT_TRUE(gesture_updates_.empty());
  EXPECT_FALSE(gesture_complete_called_);
}

// Tests successful one-finger triple-tap with drag detection.
TEST_F(MFingerNTapDragRecognizerTest, OneFingerTripleTapWithDragDetected) {
  CreateGestureRecognizer(1 /*number of fingers*/, 3 /*number of fingers*/);
  recognizer_->OnContestStarted(token_.TakeToken());

  SendPointerEvents((AddEvent(1, {}) + RemoveEvent(1, {}) + AddEvent(1, {}) + RemoveEvent(1, {}) +
                     AddEvent(1, {}) + ChangeEvents(1, {}, {})));

  RunLoopFor(a11y::recognizers_v2::kMinDragDuration);

  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kAccept);
  recognizer_->OnWin();

  EXPECT_TRUE(gesture_won_);
  EXPECT_FALSE(gesture_complete_called_);
  // We should NOT have received any updates during the MOVE events prior to
  // accepting.
  EXPECT_TRUE(gesture_updates_.empty());

  SendPointerEvents(ChangeEvents(1, {}, {0, .5f}));
  EXPECT_EQ(gesture_updates_.size(), 10u);

  EXPECT_FALSE(gesture_complete_called_);

  SendPointerEvents(RemoveEvent(1, {}));

  EXPECT_TRUE(gesture_complete_called_);
}

// Tests successful one-finger triple-tap with drag indecision with non-default
// drag displacement threshold.
TEST_F(MFingerNTapDragRecognizerTest, OneFingerTripleTapWithDragUndecidedNonDefaultDragThreshold) {
  CreateGestureRecognizer(1 /*number of fingers*/, 3 /*number of fingers*/,
                          0.2f /*drag displacement threshold*/);
  recognizer_->OnContestStarted(token_.TakeToken());

  // CHANGE events should cover a displacement between the default drag threshold
  // of 0.1f and the specified threshold of 0.2f.
  SendPointerEvents((AddEvent(1, {}) + RemoveEvent(1, {}) + AddEvent(1, {}) + RemoveEvent(1, {}) +
                     AddEvent(1, {}) + ChangeEvents(1, {}, {0.15f, 0})));

  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kUndecided);
}

// Tests the case in which a drag is detected, but the update threshold is not
// met.
TEST_F(MFingerNTapDragRecognizerTest, OneFingerTripleTapDragNoUpdatesUntilThresholdExceeded) {
  CreateGestureRecognizer(1 /*number of fingers*/, 3 /*number of fingers*/,
                          0.1f /*drag displacement threshold*/,
                          0.5f /*update displacement threshold*/);
  recognizer_->OnContestStarted(token_.TakeToken());

  SendPointerEvents((AddEvent(1, {}) + RemoveEvent(1, {}) + AddEvent(1, {}) + RemoveEvent(1, {}) +
                     AddEvent(1, {}) + ChangeEvents(1, {}, {0.5f, 0.5f})));

  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kAccept);
  recognizer_->OnWin();

  EXPECT_TRUE(gesture_won_);
  EXPECT_FALSE(gesture_complete_called_);
  // We should NOT have received any updates during the CHANGE events prior to
  // accepting.
  EXPECT_TRUE(gesture_updates_.empty());

  // Move across a displacement that does NOT exceed the update threshold.
  SendPointerEvents(ChangeEvents(1, {0.5f, 0.5f}, {0.6f, .5f}));

  // No updates should have been received.
  EXPECT_TRUE(gesture_updates_.empty());
}

// Tests the case in which a drag is detected, but then an extra finger is
// placed on screen.
TEST_F(MFingerNTapDragRecognizerTest, ThreeFingerDoubleTapWithDragDetectedExtraFinger) {
  CreateGestureRecognizer(1 /*number of fingers*/, 3 /*number of fingers*/);
  recognizer_->OnContestStarted(token_.TakeToken());

  SendPointerEvents((AddEvent(1, {}) + RemoveEvent(1, {}) + AddEvent(1, {}) + RemoveEvent(1, {}) +
                     AddEvent(1, {}) + ChangeEvents(1, {}, {})));

  RunLoopFor(a11y::recognizers_v2::kMinDragDuration);

  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kAccept);
  recognizer_->OnWin();

  EXPECT_TRUE(gesture_won_);
  EXPECT_FALSE(gesture_complete_called_);
  // We should NOT have received any updates during the CHANGE events prior to
  // accepting.
  EXPECT_TRUE(gesture_updates_.empty());

  SendPointerEvents(ChangeEvents(1, {}, {0, .5f}));
  EXPECT_EQ(gesture_updates_.size(), 10u);

  EXPECT_FALSE(gesture_complete_called_);

  SendPointerEvents(AddEvent(2, {}));

  EXPECT_TRUE(gesture_complete_called_);
}

// Tests the case in which the finger moves too far from its starting location
// during one of the non-drag taps.
TEST_F(MFingerNTapDragRecognizerTest, OneFingerTripleTapWithDragRejectedInvalidTap) {
  CreateGestureRecognizer(1 /*number of fingers*/, 3 /*number of fingers*/);
  recognizer_->OnContestStarted(token_.TakeToken());

  SendPointerEvents((AddEvent(1, {}) + ChangeEvents(1, {}, {1, 1})));

  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kReject);
  EXPECT_FALSE(gesture_won_);
  EXPECT_FALSE(gesture_complete_called_);
  EXPECT_TRUE(gesture_updates_.empty());
}

// Tests the case in which the gesture is accepted after the finger moves far from its starting
// position on the last tap.
TEST_F(MFingerNTapDragRecognizerTest, OneFingerTripleTapWithDragAggressiveAccept) {
  CreateGestureRecognizer(1 /*number of fingers*/, 3 /*number of fingers*/);
  recognizer_->OnContestStarted(token_.TakeToken());

  SendPointerEvents((AddEvent(1, {}) + RemoveEvent(1, {}) + AddEvent(1, {}) + RemoveEvent(1, {}) +
                     AddEvent(1, {}) + ChangeEvents(1, {}, {0, 0.6})));

  // Once the finger has a displacement of more than .1f from its initial
  // location during the third tap, we should accept.
  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kAccept);
}

// Tests the case in which the gesture is rejected for a timeout on one of the taps that is NOT the
// last.
TEST_F(MFingerNTapDragRecognizerTest, ThreeFingerDoubleTapRejectedEarlyTapLengthTimeout) {
  CreateGestureRecognizer(3 /*number of fingers*/, 2 /*number of fingers*/);
  recognizer_->OnContestStarted(token_.TakeToken());

  SendPointerEvents(AddEvent(1, {}));
  RunLoopFor(a11y::recognizers_v2::kMaxTapDuration);

  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kReject);
}

// Tests the case in which the gesture is rejected for a timeout on the last tap.
TEST_F(MFingerNTapDragRecognizerTest, ThreeFingerDoubleTapRejectedLastTapLengthTimeout) {
  CreateGestureRecognizer(3 /*number of fingers*/, 2 /*number of fingers*/);
  recognizer_->OnContestStarted(token_.TakeToken());

  SendPointerEvents(AddEvent(1, {}) + AddEvent(2, {}) + AddEvent(3, {}) + RemoveEvent(1, {}) +
                    RemoveEvent(2, {}) + RemoveEvent(3, {}) + AddEvent(1, {}));
  RunLoopFor(a11y::recognizers_v2::kMaxTapDuration);

  EXPECT_EQ(token_.status(), MockParticipationTokenHandle::Status::kReject);
}

}  // namespace
}  // namespace accessibility_test

// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/a11y/lib/gesture_manager/recognizers_v2/two_finger_drag_recognizer.h"

#include <fuchsia/ui/pointer/augment/cpp/fidl.h>

#include <memory>

#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/ui/a11y/lib/gesture_manager/gesture_util_v2/util.h"
#include "src/ui/a11y/lib/gesture_manager/recognizers_v2/tests/mocks/mock_participation_token.h"
#include "src/ui/a11y/lib/gesture_manager/recognizers_v2/timing_constants.h"
#include "src/ui/a11y/lib/testing/input_v2.h"

#include <glm/glm.hpp>

namespace accessibility_test {
namespace {

using input_v2::AddEvent;
using input_v2::ChangeEvents;
using input_v2::DragEvents;
using input_v2::RemoveEvent;

class TwoFingerDragRecognizerTest : public gtest::TestLoopFixture {
 public:
  TwoFingerDragRecognizerTest() = default;

  void SendPointerEvents(const std::vector<input_v2::PointerParams>& events) {
    for (const auto& event : events) {
      SendPointerEvent(event);
    }
  }

  void SendPointerEvent(const input_v2::PointerParams& event) {
    if (token_.is_held()) {
      recognizer_->HandleEvent(ToTouchEvent(event, 0));
    }
  }

  void CreateGestureRecognizer() {
    recognizer_ = std::make_unique<a11y::recognizers_v2::TwoFingerDragRecognizer>(
        [this](a11y::gesture_util_v2::GestureContext context) {
          gesture_start_callback_called_ = true;
        },
        [this](a11y::gesture_util_v2::GestureContext context) {
          gesture_updates_.push_back(std::move(context));
        },
        [this](a11y::gesture_util_v2::GestureContext context) {
          gesture_complete_callback_called_ = true;
        },
        a11y::recognizers_v2::kMinDragDuration);

    recognizer_->OnContestStarted(token_.TakeToken());
  }

 protected:
  MockParticipationTokenHandle token_;
  std::unique_ptr<a11y::recognizers_v2::TwoFingerDragRecognizer> recognizer_;
  std::vector<a11y::gesture_util_v2::GestureContext> gesture_updates_;
  bool gesture_start_callback_called_ = false;
  bool gesture_complete_callback_called_ = false;
};

// Tests successful drag detection case where time threshold is exceeded.
TEST_F(TwoFingerDragRecognizerTest, WonAfterGestureDetectedTimeThreshold) {
  CreateGestureRecognizer();

  glm::vec2 first_update_ndc_position = {0, .01f};
  auto first_update_local_coordinates = input_v2::ToLocalCoordinates(first_update_ndc_position);

  SendPointerEvents(AddEvent(1, {0, 0.01f}) + AddEvent(2, {}) + ChangeEvents(1, {}, {0, .01f}));

  // Wait for the drag delay to elapse, at which point the recognizer should claim the win and
  // invoke the update callback.
  RunLoopFor(a11y::recognizers_v2::kMinDragDuration);

  ASSERT_EQ(token_.status(), MockParticipationTokenHandle::Status::kAccept);
  recognizer_->OnWin();

  EXPECT_TRUE(gesture_start_callback_called_);
  EXPECT_FALSE(gesture_complete_callback_called_);

  // We should see an update at location of the last event ingested prior to the delay elapsing.
  EXPECT_EQ(gesture_updates_.size(), 1u);
  {
    auto& location = gesture_updates_[0].current_pointer_locations[1].local_point;
    EXPECT_EQ(location.x, first_update_local_coordinates.x);
    EXPECT_EQ(location.y, first_update_local_coordinates.y);
  }
  {
    auto& location = gesture_updates_[0].current_pointer_locations[2].local_point;
    EXPECT_EQ(location.x, 0.);
    EXPECT_EQ(location.y, 0.);
  }

  SendPointerEvents(ChangeEvents(2, {0, 0}, {0, .1}) + RemoveEvent(2, {0, .1}));

  EXPECT_FALSE(token_.is_held());
  EXPECT_TRUE(gesture_complete_callback_called_);

  // Since ChangeEvents() generates 10 evenly-spaced pointer events between the starting point (0,
  // 0) and ending point (0, .1). We should receive updates for each event.

  EXPECT_EQ(gesture_updates_.size(), 11u);
  {
    auto& location = gesture_updates_[10].current_pointer_locations[2].ndc_point;
    EXPECT_EQ(location.x, 0.f);
    EXPECT_GT(location.y, 0.09f);
    EXPECT_LT(location.y, 0.11f);
  }
}

// Drag detected after separation threshold exceeded.
TEST_F(TwoFingerDragRecognizerTest, WonAfterGestureDetectedSeparationThresholdIncreasing) {
  CreateGestureRecognizer();

  SendPointerEvents(AddEvent(1, {0, 0}) + AddEvent(2, {0, 0.01}) + ChangeEvents(1, {}, {0, .02f}));

  // Once the distance between the two pointers has increased or decreased by
  // more than 20%, we should accept.
  ASSERT_EQ(token_.status(), MockParticipationTokenHandle::Status::kAccept);
}

// Drag detected after separation threshold exceeded.
TEST_F(TwoFingerDragRecognizerTest, WonAfterGestureDetectedSeparationThresholdDecreasing) {
  CreateGestureRecognizer();

  SendPointerEvents(AddEvent(1, {0, 0}) + AddEvent(2, {0, 0.05}) + ChangeEvents(1, {}, {0, .02f}));

  // Once the distance between the two pointers has increased or decreased by
  // more than 20%, we should accept.
  ASSERT_EQ(token_.status(), MockParticipationTokenHandle::Status::kAccept);
}

// Drag detected after displacement threshold exceeded.
TEST_F(TwoFingerDragRecognizerTest, WonAfterGestureDetectedDisplacementThreshold) {
  CreateGestureRecognizer();

  SendPointerEvents(AddEvent(1, {0, 0}) + AddEvent(2, {0, 0.5}) +
                    ChangeEvents(2, {0, 0.5}, {0, .59f}));

  // The cenroid has not yet moved by .1f, so remain undecided.
  ASSERT_EQ(token_.status(), MockParticipationTokenHandle::Status::kUndecided);

  SendPointerEvents(ChangeEvents(1, {0, 0}, {0, .12}));

  ASSERT_EQ(token_.status(), MockParticipationTokenHandle::Status::kAccept);
}

// Drag rejected when third finger comes down.
TEST_F(TwoFingerDragRecognizerTest, RejectTooManyFingers) {
  CreateGestureRecognizer();

  SendPointerEvents(AddEvent(1, {0, 0}) + AddEvent(2, {0, 0.5}) + AddEvent(3, {0, 0}));

  // The cenroid has not yet moved by .1f, so remain undecided.
  ASSERT_EQ(token_.status(), MockParticipationTokenHandle::Status::kReject);
}

// Drag rejected if the second finger doesn't come down soon enough.
TEST_F(TwoFingerDragRecognizerTest, RejectSecondFingerTimeout) {
  CreateGestureRecognizer();

  SendPointerEvents(AddEvent(1, {0, 0}));

  RunLoopFor(a11y::recognizers_v2::kMinDragDuration);

  ASSERT_EQ(token_.status(), MockParticipationTokenHandle::Status::kReject);
}

// Drag rejected if we see a REMOVE event before the second ADD event.
TEST_F(TwoFingerDragRecognizerTest, RejectFirstFingerLiftedBeforeSecondFingerDown) {
  CreateGestureRecognizer();

  SendPointerEvents(AddEvent(1, {0, 0}) + RemoveEvent(1, {0, 0}));

  ASSERT_EQ(token_.status(), MockParticipationTokenHandle::Status::kReject);
}

// Drag accepted only after the second finger comes down, even if the
// displacement for the first finger is large.
TEST_F(TwoFingerDragRecognizerTest, OnlyCheckDisplacementIfTwoFingersDown) {
  CreateGestureRecognizer();

  SendPointerEvents(AddEvent(1, {0, 0}) + ChangeEvents(1, {0, 0}, {0, 1}) + AddEvent(2, {0, 0}));

  ASSERT_EQ(token_.status(), MockParticipationTokenHandle::Status::kUndecided);

  SendPointerEvents(ChangeEvents(2, {0, 0}, {0, 0.6}));

  ASSERT_EQ(token_.status(), MockParticipationTokenHandle::Status::kAccept);
}

}  // namespace
}  // namespace accessibility_test

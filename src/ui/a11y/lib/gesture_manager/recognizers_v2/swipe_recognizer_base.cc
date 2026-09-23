// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/a11y/lib/gesture_manager/recognizers_v2/swipe_recognizer_base.h"

#include <fuchsia/ui/pointer/augment/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>

#include <utility>

#include "fuchsia/ui/pointer/cpp/fidl.h"
#include "src/ui/a11y/lib/gesture_manager/arena_v2/participation_token_interface.h"
#include "src/ui/a11y/lib/gesture_manager/gesture_util/util.h"

namespace a11y::recognizers_v2 {

struct SwipeRecognizerBase::Contest {
  explicit Contest(std::unique_ptr<ParticipationTokenInterface> participation_token)
      : token(std::move(participation_token)), hold_timeout(token.get()) {}

  std::unique_ptr<ParticipationTokenInterface> token;

  // Async task used to schedule hold timeout.
  async::TaskClosureMethod<ParticipationTokenInterface, &ParticipationTokenInterface::Reject>
      hold_timeout;
};

SwipeRecognizerBase::SwipeRecognizerBase(SwipeGestureCallback callback, uint32_t number_of_fingers,
                                         zx::duration swipe_gesture_timeout, std::string debug_name)
    : swipe_gesture_callback_(std::move(callback)),
      swipe_gesture_timeout_(swipe_gesture_timeout),
      number_of_fingers_(number_of_fingers),
      debug_name_(std::move(debug_name)) {}

SwipeRecognizerBase::~SwipeRecognizerBase() = default;

void SwipeRecognizerBase::HandleEvent(
    const fuchsia::ui::pointer::augment::TouchEventWithLocalHit& event) {
  FX_DCHECK(contest_);

  FX_DCHECK(event.touch_event.has_pointer_sample());
  const auto& sample = event.touch_event.pointer_sample();

  FX_DCHECK(sample.has_interaction());
  const auto& pointer_id = sample.interaction().pointer_id;

  FX_DCHECK(sample.has_phase());
  switch (sample.phase()) {
    case fuchsia::ui::pointer::EventPhase::ADD:
      InitializeStartingGestureContext(event, &gesture_context_);

      if (!ValidateTouchEvent(gesture_context_, event)) {
        contest_->token->Reject();
        break;
      }

      if (NumberOfFingersOnScreen(gesture_context_) > number_of_fingers_) {
        contest_->token->Reject();
      } else if (NumberOfFingersOnScreen(gesture_context_) == 1) {
        // Schedule a task to declare defeat with a timeout equal to swipe_gesture_timeout_.
        contest_->hold_timeout.PostDelayed(async_get_default_dispatcher(), swipe_gesture_timeout_);
      }

      break;

    case fuchsia::ui::pointer::EventPhase::CHANGE:
      // Check that gesture info for the pointer_id exists and the touch event is valid.
      if (!ValidateTouchEvent(gesture_context_, event)) {
        contest_->token->Reject();
        break;
      }

      UpdateGestureContext(event, true /* finger is on screen */, &gesture_context_);

      // Check that fingers are moving in the direction of swipe recognizer only when all the
      // fingers are detected, there is no REMOVE event seen so far and length of swipe so far
      // is longer than kMinSwipeDistance.
      if (NumberOfFingersOnScreen(gesture_context_) == number_of_fingers_) {
        if (MinSwipeLengthAchieved(pointer_id, event) && !ValidateSwipePath(pointer_id, event)) {
          contest_->token->Reject();
          break;
        }
      }

      break;

    case fuchsia::ui::pointer::EventPhase::REMOVE:
      // If we receive a REMOVE event before number_of_fingers pointers have been
      // added to the screen, then reject.
      if (gesture_context_.starting_pointer_locations.size() != number_of_fingers_) {
        contest_->token->Reject();
        break;
      }

      // Validate pointer events.
      if (!(ValidateTouchEvent(gesture_context_, event) && ValidateSwipePath(pointer_id, event))) {
        contest_->token->Reject();
        break;
      }

      UpdateGestureContext(event, false /* finger is off screen */, &gesture_context_);

      // If all the REMOVE events are detected then call Accept.
      if (!NumberOfFingersOnScreen(gesture_context_)) {
        if (SquareDistanceBetweenPoints(gesture_context_.CurrentCentroid(false),
                                        gesture_context_.StartingCentroid(false)) >=
            kMinSwipeDistance * kMinSwipeDistance) {
          contest_->token->Accept();
          contest_.reset();
        } else {
          contest_->token->Reject();
          break;
        }
      }

      break;

    default:
      break;
  }
}

void SwipeRecognizerBase::OnWin() { swipe_gesture_callback_(gesture_context_); }

void SwipeRecognizerBase::OnDefeat() { contest_.reset(); }

void SwipeRecognizerBase::ResetRecognizer() {
  contest_.reset();
  ResetGestureContext(&gesture_context_);
}

bool SwipeRecognizerBase::ValidateSwipePath(
    uint32_t pointer_id, const fuchsia::ui::pointer::augment::TouchEventWithLocalHit& event) const {
  FX_DCHECK(event.touch_event.has_pointer_sample());
  const auto& sample = event.touch_event.pointer_sample();
  FX_DCHECK(sample.has_position_in_viewport());
  const auto& [x, y] = sample.position_in_viewport();

  // Verify that slope of line containing gesture start point and current touch event location
  // falls within a pre-specified range.
  auto it = gesture_context_.starting_pointer_locations.find(pointer_id);
  if (it == gesture_context_.starting_pointer_locations.end()) {
    return false;
  }
  auto dx = x - it->second.ndc_point.x;
  auto dy = y - it->second.ndc_point.y;

  return SwipeHasValidSlopeAndDirection(dx, dy);
}

bool SwipeRecognizerBase::MinSwipeLengthAchieved(
    uint32_t pointer_id, const fuchsia::ui::pointer::augment::TouchEventWithLocalHit& event) const {
  FX_DCHECK(event.touch_event.has_pointer_sample());
  const auto& sample = event.touch_event.pointer_sample();
  FX_DCHECK(sample.has_position_in_viewport());
  const auto& [x, y] = sample.position_in_viewport();

  auto it = gesture_context_.starting_pointer_locations.find(pointer_id);
  if (it == gesture_context_.starting_pointer_locations.end()) {
    return false;
  }

  float dx = x - it->second.ndc_point.x;
  float dy = y - it->second.ndc_point.y;

  float d2 = dx * dx + dy * dy;
  return d2 >= kMinSwipeDistance * kMinSwipeDistance;
}

void SwipeRecognizerBase::OnContestStarted(std::unique_ptr<ParticipationTokenInterface> token) {
  ResetGestureContext(&gesture_context_);
  contest_ = std::make_unique<Contest>(std::move(token));
}

}  // namespace a11y::recognizers_v2

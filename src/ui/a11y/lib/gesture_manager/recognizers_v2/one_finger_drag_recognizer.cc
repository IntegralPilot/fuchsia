// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/a11y/lib/gesture_manager/recognizers_v2/one_finger_drag_recognizer.h"

#include <fuchsia/ui/pointer/augment/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>

#include "src/ui/a11y/lib/gesture_manager/arena_v2/participation_token_interface.h"

namespace a11y::recognizers_v2 {

struct OneFingerDragRecognizer::Contest {
  explicit Contest(std::unique_ptr<ParticipationTokenInterface> participation_token)
      : token(std::move(participation_token)), claim_win_task(token.get()) {}

  std::unique_ptr<ParticipationTokenInterface> token;

  bool won = false;

  // Async task that claims a win if the drag gesture lasts longer than a delay.
  async::TaskClosureMethod<ParticipationTokenInterface, &ParticipationTokenInterface::Accept>
      claim_win_task;
};

OneFingerDragRecognizer::OneFingerDragRecognizer(DragGestureCallback on_drag_started,
                                                 DragGestureCallback on_drag_update,
                                                 DragGestureCallback on_drag_complete,
                                                 zx::duration drag_gesture_delay)
    : on_drag_started_(std::move(on_drag_started)),
      on_drag_update_(std::move(on_drag_update)),
      on_drag_complete_(std::move(on_drag_complete)),
      drag_gesture_delay_(drag_gesture_delay) {}

OneFingerDragRecognizer::~OneFingerDragRecognizer() = default;

void OneFingerDragRecognizer::HandleEvent(
    const fuchsia::ui::pointer::augment::TouchEventWithLocalHit& event) {
  FX_DCHECK(contest_);

  FX_DCHECK(event.touch_event.has_pointer_sample());
  const auto& sample = event.touch_event.pointer_sample();

  FX_DCHECK(sample.has_interaction());
  const auto pointer_id = sample.interaction().pointer_id;

  FX_DCHECK(sample.has_phase());
  switch (sample.phase()) {
    case fuchsia::ui::pointer::EventPhase::ADD:
      // If there are any fingers already onscreen, then we should reject if
      // another comes down.
      if (NumberOfFingersOnScreen(gesture_context_)) {
        ResetRecognizer();
        return;
      }

      InitializeStartingGestureContext(event, &gesture_context_);

      if (!ValidateTouchEvent(gesture_context_, event)) {
        ResetRecognizer();
        return;
      }

      previous_update_location_ = gesture_context_.starting_pointer_locations[pointer_id];

      // Schedule a task to attempt to claim win after duration of drag_gesture_delay_.
      contest_->claim_win_task.PostDelayed(async_get_default_dispatcher(), drag_gesture_delay_);

      break;

    case fuchsia::ui::pointer::EventPhase::CHANGE:
      // If there are zero fingers on screen or multiple fingers on screen, then we should reset.
      if (NumberOfFingersOnScreen(gesture_context_) != 1) {
        ResetRecognizer();
        return;
      }

      if (!ValidateTouchEvent(gesture_context_, event)) {
        ResetRecognizer();
        return;
      }

      // Update pointer book-keeping.
      UpdateGestureContext(event, true /*finger is on screen*/, &gesture_context_);

      // IF this recognizer is the contest winner, previous_update_location_info_ reflects the
      // location of the previous update. Otherwise, previous_update_location_info_ reflects the
      // location of the previous pointer event ingested.
      //
      // THEREFORE, IF the recognizer is NOT yet the contest winner, we should update the previous
      // location info, but should NOT call the update callback. IF the recognizer is the contest
      // winner, we only want to update the previous location and invoke the update callback if the
      // distance between the location of the current event and the previous update exceeds the
      // minimum threshold.
      if (!contest_->won) {
        previous_update_location_ = gesture_context_.current_pointer_locations[pointer_id];
      } else if (DragDistanceExceedsUpdateThreshold(event)) {
        previous_update_location_ = gesture_context_.current_pointer_locations[pointer_id];
        on_drag_update_(gesture_context_);
      }

      break;

    case fuchsia::ui::pointer::EventPhase::REMOVE:
      if (!ValidateTouchEvent(gesture_context_, event)) {
        ResetRecognizer();
        return;
      }

      // Update gesture context to reflect REMOVE event info.
      UpdateGestureContext(event, false /*finger is off screen*/, &gesture_context_);

      // If any fingers are left onscreen after a REMOVE event, then this gesture
      // cannot be a valid one-finger drag.
      if (NumberOfFingersOnScreen(gesture_context_)) {
        ResetRecognizer();
        return;
      }

      if (contest_->won) {
        on_drag_complete_(gesture_context_);
      }

      contest_.reset();

      break;

    default:
      break;
  }
}

void OneFingerDragRecognizer::OnWin() {
  if (contest_) {
    contest_->won = true;
    // The gesture has been recognized and we inform about its start.
    on_drag_started_(gesture_context_);
    // We need to call on_drag_update_ immediately after successfully claiming a win, because it's
    // possible that no update will ever occur if no further CHANGE events are ingested, OR if the
    // locations of these events are close to the location of the last event ingested before the win
    // was claimed.
    on_drag_update_(gesture_context_);
  } else {
    // It's possible that we don't get awarded the win until after the gesture has completed, in
    // which case just call the start and complete handler.
    on_drag_started_(gesture_context_);
    on_drag_complete_(gesture_context_);
    ResetRecognizer();
  }
}

void OneFingerDragRecognizer::OnDefeat() { ResetRecognizer(); }

void OneFingerDragRecognizer::ResetRecognizer() {
  ResetGestureContext(&gesture_context_);
  contest_.reset();
}

bool OneFingerDragRecognizer::DragDistanceExceedsUpdateThreshold(
    const fuchsia::ui::pointer::augment::TouchEventWithLocalHit& event) const {
  FX_DCHECK(event.touch_event.has_pointer_sample());
  const auto& sample = event.touch_event.pointer_sample();
  FX_DCHECK(sample.has_position_in_viewport());
  const auto& [x, y] = sample.position_in_viewport();

  // Check if distance between previous update point and current event exceeds specified minimum
  // threshold.
  auto dx = x - previous_update_location_.ndc_point.x;
  auto dy = y - previous_update_location_.ndc_point.y;

  return dx * dx + dy * dy >= kMinDragDistanceForUpdate * kMinDragDistanceForUpdate;
}

void OneFingerDragRecognizer::OnContestStarted(std::unique_ptr<ParticipationTokenInterface> token) {
  ResetRecognizer();
  contest_ = std::make_unique<Contest>(std::move(token));
}

std::string OneFingerDragRecognizer::DebugName() const { return "one_finger_drag_recognizer"; }

}  // namespace a11y::recognizers_v2

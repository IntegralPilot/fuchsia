// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/a11y/lib/gesture_manager/recognizers_v2/m_finger_n_tap_drag_recognizer.h"

#include <fuchsia/ui/pointer/augment/cpp/fidl.h>
#include <fuchsia/ui/pointer/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>

#include <set>

#include "src/lib/fxl/strings/string_printf.h"
#include "src/ui/a11y/lib/gesture_manager/arena_v2/participation_token_interface.h"
#include "src/ui/a11y/lib/gesture_manager/gesture_util_v2/util.h"
#include "src/ui/a11y/lib/gesture_manager/recognizers_v2/timing_constants.h"

namespace a11y::recognizers_v2 {

struct MFingerNTapDragRecognizer::Contest {
  explicit Contest(std::unique_ptr<ParticipationTokenInterface> participation_token)
      : token(std::move(participation_token)),
        tap_length_timeout(token.get()),
        tap_interval_timeout(token.get()),
        accept_task(token.get()) {}

  std::unique_ptr<ParticipationTokenInterface> token;

  // Indicates whether m fingers have been on the screen at the same time
  // during the current tap.
  bool tap_in_progress = false;

  // Keeps the count of the number of taps detected so far, for the gesture.
  uint32_t number_of_taps_detected = 0;

  // Indicates whether the recognizer has successfully accepted the gesture.
  bool won = false;

  // Async task to schedule tap length timeout.
  // This task enforces a timeout between the first ADD event and last REMOVE event
  // of a particular tap.
  async::TaskClosureMethod<ParticipationTokenInterface, &ParticipationTokenInterface::Reject>
      tap_length_timeout;

  // Async task used to schedule tap interval timeout.
  // This task enforces a timeout between the last REMOVE event of a tap and the
  // first ADD event of the next tap.
  async::TaskClosureMethod<ParticipationTokenInterface, &ParticipationTokenInterface::Reject>
      tap_interval_timeout;

  // Async task to schedule delayed win for held tap.
  async::TaskClosureMethod<ParticipationTokenInterface, &ParticipationTokenInterface::Accept>
      accept_task;
};

MFingerNTapDragRecognizer::MFingerNTapDragRecognizer(
    OnMFingerNTapDragCallback on_recognize, OnMFingerNTapDragCallback on_update,
    OnMFingerNTapDragCallback on_complete, uint32_t number_of_fingers, uint32_t number_of_taps,
    float drag_displacement_threshold, float update_displacement_threshold)
    : on_recognize_(std::move(on_recognize)),
      on_update_(std::move(on_update)),
      on_complete_(std::move(on_complete)),
      number_of_fingers_in_gesture_(number_of_fingers),
      number_of_taps_in_gesture_(number_of_taps),
      drag_displacement_threshold_(drag_displacement_threshold),
      update_displacement_threshold_(update_displacement_threshold) {}

MFingerNTapDragRecognizer::~MFingerNTapDragRecognizer() = default;

void MFingerNTapDragRecognizer::OnTapStarted() {
  // If this tap is the last in the gesture, post a task to accept the gesture
  // if the fingers are still on screen after `kMinDragDuration` has elapsed.
  // Otherwise, if this tap is NOT the last in the gesture, post a task to
  // reject the gesture if the fingers have not lifted by the time `kTapDuration`
  // elapses. In this case, we also need to cancel the tap length timeout.
  if (contest_->number_of_taps_detected == number_of_taps_in_gesture_ - 1) {
    contest_->tap_length_timeout.Cancel();
    contest_->accept_task.PostDelayed(async_get_default_dispatcher(), kMinDragDuration);
  }
}

void MFingerNTapDragRecognizer::OnExcessFingers() {
  // If the gesture has already been accepted (i.e. the user has successfully
  // performed n-1 taps, followed by a valid hold, but an (m+1)th finger comes
  // down on screen, we should invoke the on_complete_ callback.
  // In any event, the gesture is no longer valid, so we should reset the
  // recognizer.
  if (contest_->won) {
    on_complete_(gesture_context_);
  }

  ResetRecognizer();
}

void MFingerNTapDragRecognizer::OnChangeEvent(
    const fuchsia::ui::pointer::augment::TouchEventWithLocalHit& event) {
  // If we've accepted the gesture, invoke on_update_. Otherwise, if the current
  // tap is the last (which could become a drag), we should check if the
  // fingers have already moved far enough to constitute a drag.
  // If this tap is not the last, we should verify that the
  // fingers are close enough to their starting locations to constitute a valid
  // tap.
  if (contest_->won) {
    if (DisplacementExceedsThreshold(
            last_update_gesture_context_.CurrentCentroid(/* use_local_coordinates = */ false),
            gesture_context_.CurrentCentroid(/* use_local_coordinates = */ false),
            update_displacement_threshold_)) {
      on_update_(gesture_context_);
      last_update_gesture_context_ = gesture_context_;
    }
  } else if (contest_->number_of_taps_detected == number_of_taps_in_gesture_ - 1) {
    if (DisplacementExceedsThreshold(
            gesture_context_.StartingCentroid(/* use_local_coordinates = */ false),
            gesture_context_.CurrentCentroid(/* use_local_coordinates = */ false),
            drag_displacement_threshold_)) {
      contest_->token->Accept();
      return;
    }
  } else if (!TouchEventIsValidTap(gesture_context_, event)) {
    ResetRecognizer();
  }
}

void MFingerNTapDragRecognizer::OnRemoveEvent() {
  // If we've already accepted the gesture, then we should invoke on_complete_
  // and reset the recognizer once the first REMOVE event is received (at which
  // point, the drag is considered complete).
  if (contest_->won) {
    on_complete_(gesture_context_);
    ResetRecognizer();
    return;
  }

  // If we have counted number_of_taps_in_gesture_ - 1 complete taps, then this
  // REMOVE event must mark the end of the drag. If we have not already accepted the
  // gesture at this point, we should reject.
  if (contest_->number_of_taps_detected == number_of_taps_in_gesture_ - 1) {
    ResetRecognizer();
    return;
  }

  // If this REMOVE event removed the last finger from the screen, then the most
  // recent tap is complete.
  if (!NumberOfFingersOnScreen(gesture_context_)) {
    // If we've made it this far, we know that (1) m fingers were on screen
    // simultaneously during the current single tap, and (2) The m fingers have
    // now been removed, without any interceding finger ADD events.
    // Therefore, we can conclude that a complete m-finger tap has occurred.
    contest_->number_of_taps_detected++;

    // Mark that all m fingers were removed from the screen.
    contest_->tap_in_progress = false;

    // Cancel task which was scheduled for detecting single tap.
    contest_->tap_length_timeout.Cancel();

    // Schedule task with delay of timeout_between_taps_.
    contest_->tap_interval_timeout.PostDelayed(async_get_default_dispatcher(), kMaxTimeBetweenTaps);
  }
}

void MFingerNTapDragRecognizer::HandleEvent(
    const fuchsia::ui::pointer::augment::TouchEventWithLocalHit& event) {
  FX_DCHECK(contest_);

  FX_DCHECK(event.touch_event.has_pointer_sample());
  const auto& sample = event.touch_event.pointer_sample();

  FX_DCHECK(sample.has_interaction()) << DebugName() << ": touch event is missing pointer id.";
  const auto pointer_id = sample.interaction().pointer_id;

  FX_DCHECK(sample.has_phase()) << DebugName() << ": touch event is missing phase information.";
  switch (sample.phase()) {
    case fuchsia::ui::pointer::EventPhase::ADD:
      // If we receive an ADD event when there are already m fingers on the
      // screen, then either we've received a second ADD event for one of the fingers that's
      // already on the screen, or we've received an ADD event for an (m+1)th
      // finger. In either case, we should abandon the current gesture.
      if (NumberOfFingersOnScreen(gesture_context_) >= number_of_fingers_in_gesture_) {
        OnExcessFingers();
        break;
      }

      // If we receive an ADD event when there is a tap in progress, then we
      // should abandon the gesture.
      // NOTE: this is a distinct check from the one above, and is required to
      // ensure that the number of fingers touching the screen decreases
      // monotonically once the first finger is removed.
      // For example,
      // consider the case of finger 1 DOWN, finger 2 DOWN, finger 2 UP, finger
      // 2 DOWN. Clearly, this is not a two-finger tap, but at the time of the
      // second "finger 2 DOWN" event, contest->fingers_on_screen.size() would
      // be 1, so the check above would pass.
      if (contest_->tap_in_progress) {
        ResetRecognizer();
        break;
      }

      // If we receive successive ADD events for the same pointer without a
      // REMOVE event, then we should abandon the current gesture.
      if (FingerIsOnScreen(gesture_context_, pointer_id)) {
        ResetRecognizer();
        break;
      }

      // Initialize starting info for this new tap.
      InitializeStartingGestureContext(event, &gesture_context_);

      // If the total number of fingers involved in the gesture now exceeds
      // number_of_fingers_in_gesture_, reject the gesture.
      if (gesture_context_.starting_pointer_locations.size() > number_of_fingers_in_gesture_) {
        ResetRecognizer();
        break;
      }

      // On the first ADD event of the tap, cancel the tap interval timeout and
      // schedule the tap length timeout.
      if (NumberOfFingersOnScreen(gesture_context_) == 1) {
        contest_->tap_interval_timeout.Cancel();
        contest_->tap_length_timeout.PostDelayed(async_get_default_dispatcher(), kMaxTapDuration);
      }

      contest_->tap_in_progress =
          (NumberOfFingersOnScreen(gesture_context_) == number_of_fingers_in_gesture_);
      // Only start the timeout once all m fingers are on the screen together.
      if (contest_->tap_in_progress) {
        OnTapStarted();
      }

      break;

    case fuchsia::ui::pointer::EventPhase::CHANGE:
      FX_DCHECK(FingerIsOnScreen(gesture_context_, pointer_id))
          << DebugName() << ": CHANGE event received without preceding ADD event.";

      // Validate the event for the gesture being performed.
      if (!ValidateTouchEvent(gesture_context_, event)) {
        ResetRecognizer();
        break;
      }

      UpdateGestureContext(event, true /* finger is on screen */, &gesture_context_);

      OnChangeEvent(event);

      break;

    case fuchsia::ui::pointer::EventPhase::REMOVE:
      FX_DCHECK(FingerIsOnScreen(gesture_context_, pointer_id))
          << DebugName() << ": REMOVE event received without preceding ADD event.";

      // Validate event for the gesture being performed.
      if (!ValidateTouchEvent(gesture_context_, event)) {
        ResetRecognizer();
        break;
      }

      UpdateGestureContext(event, false /* finger is not on screen */, &gesture_context_);

      // The number of fingers on screen during a multi-finger tap should
      // monotonically increase from 0 to m, and
      // then monotonically decrease back to 0. If a finger is removed before
      // number_of_fingers_in_gesture_ fingers are on the screen simultaneously,
      // then we should reject this gesture.
      if (!contest_->tap_in_progress) {
        ResetRecognizer();
        break;
      }

      OnRemoveEvent();

      break;
    default:
      break;
  }
}

bool MFingerNTapDragRecognizer::DisplacementExceedsThreshold(::fuchsia::math::PointF start,
                                                             ::fuchsia::math::PointF end,
                                                             float threshold) const {
  return gesture_util_v2::SquareDistanceBetweenPoints(start, end) >= threshold * threshold;
}

void MFingerNTapDragRecognizer::ResetRecognizer() {
  contest_.reset();
  ResetGestureContext(&gesture_context_);
}

void MFingerNTapDragRecognizer::OnWin() {
  on_recognize_(gesture_context_);
  last_update_gesture_context_ = gesture_context_;
  if (contest_) {
    contest_->won = true;
  } else {
    // It's possible that we don't get awarded the win until after the gesture has
    // completed, in which case we also need to call the complete handler.
    on_complete_(gesture_context_);
    ResetRecognizer();
  }
}

void MFingerNTapDragRecognizer::OnDefeat() { ResetRecognizer(); }

void MFingerNTapDragRecognizer::OnContestStarted(
    std::unique_ptr<ParticipationTokenInterface> token) {
  ResetRecognizer();
  contest_ = std::make_unique<Contest>(std::move(token));
}

std::string MFingerNTapDragRecognizer::DebugName() const {
  return fxl::StringPrintf("MFingerNTapDragRecognizer(m=%d, n=%d)", number_of_fingers_in_gesture_,
                           number_of_taps_in_gesture_);
}

}  // namespace a11y::recognizers_v2

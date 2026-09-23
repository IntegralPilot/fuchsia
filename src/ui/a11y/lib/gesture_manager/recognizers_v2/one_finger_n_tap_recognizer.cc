// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/a11y/lib/gesture_manager/recognizers_v2/one_finger_n_tap_recognizer.h"

#include <fuchsia/ui/pointer/augment/cpp/fidl.h>
#include <fuchsia/ui/pointer/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>

#include "src/lib/fxl/strings/string_printf.h"
#include "src/ui/a11y/lib/gesture_manager/arena_v2/participation_token_interface.h"

namespace a11y::recognizers_v2 {

struct OneFingerNTapRecognizer::Contest {
  explicit Contest(std::unique_ptr<ParticipationTokenInterface> participation_token)
      : token(std::move(participation_token)), reject_task(token.get()) {}

  std::unique_ptr<ParticipationTokenInterface> token;

  // Indicates that an ADD event for the current tap is detected.
  bool tap_in_progress = false;

  // Keeps the count of the number of taps detected so far, for the gesture.
  int number_of_taps_detected = 0;

  // Async task used to schedule long-press timeout.
  async::TaskClosureMethod<ParticipationTokenInterface, &ParticipationTokenInterface::Reject>
      reject_task;
};

OneFingerNTapRecognizer::OneFingerNTapRecognizer(OnFingerTapGesture callback, int number_of_taps,
                                                 zx::duration tap_timeout,
                                                 zx::duration timeout_between_taps)
    : on_finger_tap_callback_(std::move(callback)),
      number_of_taps_in_gesture_(number_of_taps),
      tap_timeout_(tap_timeout),
      timeout_between_taps_(timeout_between_taps) {}

OneFingerNTapRecognizer::~OneFingerNTapRecognizer() = default;

void OneFingerNTapRecognizer::HandleEvent(
    const fuchsia::ui::pointer::augment::TouchEventWithLocalHit& event) {
  FX_DCHECK(contest_);

  FX_DCHECK(event.touch_event.has_pointer_sample());
  const auto& sample = event.touch_event.pointer_sample();

  FX_DCHECK(sample.has_phase()) << DebugName() << ": touch event is missing phase information.";
  switch (sample.phase()) {
    case fuchsia::ui::pointer::EventPhase::ADD:
      // If a tap is already detected, make sure the pointer_id and device_id of the new event,
      // matches with the previous one.
      if (contest_->number_of_taps_detected) {
        if (!ValidateTouchEvent(gesture_context_, event)) {
          FX_LOGS(INFO) << DebugName() << ": touch event is not valid. Dropping current event.";
          ResetRecognizer();
          break;
        }
      }

      // Initialize gesture_start_info and gesture_context.
      InitializeStartingGestureContext(event, &gesture_context_);

      // If the gesture is already in progress then abandon this gesture since an ADD event
      // represents the start of the gesture. Also, check event is valid for one finger tap.
      if (contest_->tap_in_progress || !TouchEventIsValidTap(gesture_context_, event)) {
        FX_LOGS(INFO) << DebugName()
                      << ": touch event is not valid for current gesture. "
                         "Dropping current event.";
        ResetRecognizer();
        break;
      }

      // Cancel task which would be scheduled for timeout between taps.
      contest_->reject_task.Cancel();

      // Schedule a task with timeout `kTapDuration` for the current tap to complete.
      contest_->reject_task.PostDelayed(async_get_default_dispatcher(), tap_timeout_);
      contest_->tap_in_progress = true;
      break;

    case fuchsia::ui::pointer::EventPhase::CHANGE:
      FX_DCHECK(contest_->tap_in_progress)
          << DebugName() << ": CHANGE event received without preceding ADD event.";

      // Validate the event for the gesture being performed.
      if (!ValidateEvent(event)) {
        ResetRecognizer();
      }

      UpdateGestureContext(event, true, &gesture_context_);

      break;

    case fuchsia::ui::pointer::EventPhase::REMOVE:
      FX_DCHECK(contest_->tap_in_progress)
          << DebugName() << ": REMOVE event received without preceding ADD event.";

      // Validate event for the gesture being performed.
      if (!ValidateEvent(event)) {
        ResetRecognizer();
        break;
      }

      UpdateGestureContext(event, false, &gesture_context_);

      // Tap is detected.
      contest_->number_of_taps_detected += 1;

      // Check if this is not the last tap of the gesture.
      if (contest_->number_of_taps_detected < number_of_taps_in_gesture_) {
        contest_->tap_in_progress = false;
        // Cancel task which was scheduled for detecting single tap.
        contest_->reject_task.Cancel();

        // Schedule task with delay of timeout_between_taps_.
        contest_->reject_task.PostDelayed(async_get_default_dispatcher(), timeout_between_taps_);
      } else {
        // Tap gesture is detected.
        contest_->token->Accept();
        contest_.reset();
      }
      break;
    default:
      break;
  }
}

void OneFingerNTapRecognizer::OnWin() {
  on_finger_tap_callback_(gesture_context_);
  ResetGestureContext(&gesture_context_);
}

void OneFingerNTapRecognizer::OnDefeat() { ResetRecognizer(); }

void OneFingerNTapRecognizer::ResetRecognizer() {
  contest_.reset();
  ResetGestureContext(&gesture_context_);
}

bool OneFingerNTapRecognizer::ValidateEvent(
    const fuchsia::ui::pointer::augment::TouchEventWithLocalHit& event) const {
  return ValidateTouchEvent(gesture_context_, event) &&
         TouchEventIsValidTap(gesture_context_, event);
}

void OneFingerNTapRecognizer::OnContestStarted(
    std::unique_ptr<ParticipationTokenInterface> participation_token) {
  ResetRecognizer();
  contest_ = std::make_unique<Contest>(std::move(participation_token));
}

std::string OneFingerNTapRecognizer::DebugName() const {
  return fxl::StringPrintf("OneFingerNTapRecognizer(n=%d)", number_of_taps_in_gesture_);
}

}  // namespace a11y::recognizers_v2

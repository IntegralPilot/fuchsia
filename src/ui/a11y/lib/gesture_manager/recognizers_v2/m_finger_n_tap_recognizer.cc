// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/a11y/lib/gesture_manager/recognizers_v2/m_finger_n_tap_recognizer.h"

#include <fuchsia/ui/pointer/augment/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>

#include <set>

#include "src/lib/fxl/strings/string_printf.h"
#include "src/ui/a11y/lib/gesture_manager/arena_v2/participation_token_interface.h"
#include "src/ui/a11y/lib/gesture_manager/gesture_util_v2/util.h"
#include "src/ui/a11y/lib/gesture_manager/recognizers_v2/timing_constants.h"

namespace a11y::recognizers_v2 {

namespace {

using gesture_util_v2::ValidateTouchEvent;

}  // namespace

struct MFingerNTapRecognizer::Contest {
  explicit Contest(std::unique_ptr<ParticipationTokenInterface> participation_token)
      : token(std::move(participation_token)),
        tap_length_timeout(token.get()),
        tap_interval_timeout(token.get()) {}

  std::unique_ptr<ParticipationTokenInterface> token;
  // Indicates whether m fingers have been on the screen at the same time
  // during the current tap.
  bool tap_in_progress = false;
  // Keeps the count of the number of taps detected so far, for the gesture.
  uint32_t number_of_taps_detected = 0;
  // Async task used to reject taps that are held for too long.
  // This task enforces a time limit between the first finger ADD event and
  // last finger REMOVE event of a particular tap.
  async::TaskClosureMethod<ParticipationTokenInterface, &ParticipationTokenInterface::Reject>
      tap_length_timeout;
  // Async task used to schedule between-tap timeout.
  // This task enforces a time limit between the last finger REMOVE event of one tap
  // and the first finger ADD event of the next tap.
  async::TaskClosureMethod<ParticipationTokenInterface, &ParticipationTokenInterface::Reject>
      tap_interval_timeout;
};

MFingerNTapRecognizer::MFingerNTapRecognizer(OnMFingerNTapCallback callback,
                                             uint32_t number_of_fingers, uint32_t number_of_taps)
    : on_recognize_(std::move(callback)),
      number_of_fingers_in_gesture_(number_of_fingers),
      number_of_taps_in_gesture_(number_of_taps) {}

MFingerNTapRecognizer::~MFingerNTapRecognizer() = default;

void MFingerNTapRecognizer::OnExcessFingers() { ResetRecognizer(); }

void MFingerNTapRecognizer::OnChangeEvent(
    const fuchsia::ui::pointer::augment::TouchEventWithLocalHit& pointer_event) {
  if (!TouchEventIsValidTap(gesture_context_, pointer_event)) {
    ResetRecognizer();
  }
}

void MFingerNTapRecognizer::OnRemoveEvent() {
  // If there are still fingers on the screen, then we haven't yet detected
  // a full tap, so there's no more work to do at this point.
  if (NumberOfFingersOnScreen(gesture_context_)) {
    return;
  }

  // If we've made it this far, we know that (1) m fingers were on screen
  // simultaneously during the current gesture, and (2) The m fingers have
  // now been removed, without any interceding finger REMOVE events.
  // Therefore, we can conclude that a complete m-finger tap has occurred.
  // In this case, we should cancel the tap-length timeout.
  contest_->number_of_taps_detected++;
  contest_->tap_in_progress = false;
  contest_->tap_length_timeout.Cancel();

  // Check if this is not the last tap of the gesture.
  if (contest_->number_of_taps_detected < number_of_taps_in_gesture_) {
    // Schedule task with delay of timeout_between_taps_.
    contest_->tap_interval_timeout.PostDelayed(async_get_default_dispatcher(), kMaxTimeBetweenTaps);
  } else {
    // Tap gesture is detected.
    contest_->token->Accept();
    contest_.reset();
  }
}

void MFingerNTapRecognizer::HandleEvent(
    const fuchsia::ui::pointer::augment::TouchEventWithLocalHit& event) {
  FX_DCHECK(contest_);
  FX_DCHECK(event.touch_event.has_pointer_sample());
  const auto& sample = event.touch_event.pointer_sample();

  FX_DCHECK(sample.has_interaction()) << DebugName() << ": Pointer event is missing pointer id.";
  const auto pointer_id = sample.interaction().pointer_id;

  FX_DCHECK(sample.has_phase()) << DebugName() << ": Pointer event is missing phase information.";
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

      // If we receive successive ADD events for the same pointer without an
      // REMOVE event, then we should abandon the current gesture.
      if (FingerIsOnScreen(gesture_context_, pointer_id)) {
        ResetRecognizer();
        break;
      }

      // Initialize starting info for this new tap.
      gesture_util_v2::InitializeStartingGestureContext(event, &gesture_context_);

      // If the total number of fingers involved in the gesture now exceeds
      // number_of_fingers_in_gesture_, reject the gesture.
      if (gesture_context_.starting_pointer_locations.size() > number_of_fingers_in_gesture_) {
        ResetRecognizer();
        break;
      }

      // Cancel task which would be scheduled for timeout between taps and
      // schedule the timeout for this tap if this is the first ADD event of
      // the new tap.
      if (NumberOfFingersOnScreen(gesture_context_) == 1) {
        contest_->tap_interval_timeout.Cancel();
        contest_->tap_length_timeout.PostDelayed(async_get_default_dispatcher(), kMaxTapDuration);
      }

      contest_->tap_in_progress =
          (NumberOfFingersOnScreen(gesture_context_) == number_of_fingers_in_gesture_);

      break;

    case fuchsia::ui::pointer::EventPhase::CHANGE:
      FX_DCHECK(FingerIsOnScreen(gesture_context_, pointer_id))
          << DebugName() << ": Pointer CHANGE event received without preceding ADD event.";

      // Validate the pointer_event for the gesture being performed.
      if (!ValidateTouchEvent(gesture_context_, event)) {
        ResetRecognizer();
        break;
      }

      gesture_util_v2::UpdateGestureContext(event, true /* finger is on screen */,
                                            &gesture_context_);

      OnChangeEvent(event);

      break;

    case fuchsia::ui::pointer::EventPhase::REMOVE:
      FX_DCHECK(FingerIsOnScreen(gesture_context_, pointer_id))
          << DebugName() << ": Pointer REMOVE event received without preceding DOWN event.";

      // Validate pointer_event for the gesture being performed.
      if (!ValidateTouchEvent(gesture_context_, event)) {
        ResetRecognizer();
        break;
      }

      gesture_util_v2::UpdateGestureContext(event, false /* finger is not on screen */,
                                            &gesture_context_);

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

    case fuchsia::ui::pointer::EventPhase::CANCEL:
      ResetRecognizer();
      break;
  }
}

void MFingerNTapRecognizer::ResetRecognizer() {
  contest_.reset();
  ResetGestureContext(&gesture_context_);
}

void MFingerNTapRecognizer::OnWin() {
  on_recognize_(gesture_context_);
  ResetGestureContext(&gesture_context_);
}

void MFingerNTapRecognizer::OnDefeat() { ResetRecognizer(); }

void MFingerNTapRecognizer::OnContestStarted(
    std::unique_ptr<ParticipationTokenInterface> participation_token) {
  ResetRecognizer();
  contest_ = std::make_unique<Contest>(std::move(participation_token));
}

std::string MFingerNTapRecognizer::DebugName() const {
  return fxl::StringPrintf("MFingerNTapRecognizer(m=%d, n=%d)", number_of_fingers_in_gesture_,
                           number_of_taps_in_gesture_);
}

}  // namespace a11y::recognizers_v2

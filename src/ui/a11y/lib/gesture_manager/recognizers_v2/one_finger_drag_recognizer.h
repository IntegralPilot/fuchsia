// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_A11Y_LIB_GESTURE_MANAGER_RECOGNIZERS_V2_ONE_FINGER_DRAG_RECOGNIZER_H_
#define SRC_UI_A11Y_LIB_GESTURE_MANAGER_RECOGNIZERS_V2_ONE_FINGER_DRAG_RECOGNIZER_H_

#include <fuchsia/ui/pointer/augment/cpp/fidl.h>
#include <lib/fit/function.h>
#include <lib/zx/time.h>

#include "src/ui/a11y/lib/gesture_manager/arena_v2/participation_token_interface.h"
#include "src/ui/a11y/lib/gesture_manager/arena_v2/recognizer_v2.h"
#include "src/ui/a11y/lib/gesture_manager/gesture_util_v2/util.h"
#include "src/ui/a11y/lib/gesture_manager/recognizers_v2/timing_constants.h"

namespace a11y::recognizers_v2 {

// OneFingerDragRecognizer class implements logic to recognize and react to one finger drag
// gestures.
//
// Minimal effort is taken towards ignoring 2-finger gestures. For feature parity, while a second
// finger is down, events will be suppressed. When it is released, the remaining pointer must be the
// original. This requirement should probably be dropped in the future.
class OneFingerDragRecognizer : public GestureRecognizerV2 {
 public:
  // Minimum distance (in NDC) that a drag gesture must cover in order to invoke another update.
  // Value 1.f / 16 is chosen based on one finger tap recognizer maximum displacement.
  static constexpr float kMinDragDistanceForUpdate = 1.f / 16.f;

  // Signature for various drag recognizer callback functions.
  using DragGestureCallback = fit::function<void(gesture_util_v2::GestureContext)>;

  // on_drag_started: Callback invoked at most once when the recognizer has won the arena. Callback
  // only occurs if at least one pointer is on the screen.
  //
  // on_drag_update: Callback invoked as new CHANGE events are handled AFTER the drag gesture is
  // recognized and has won the arena. Callbacks only occur while exactly one pointer is on the
  // screen.
  //
  // on_drag_complete: Callback invoked when the drag gesture is completed (as finger is lifted from
  // screen, or after this recognizer is awarded the win if this occurs after the gesture has
  // ended).
  //
  // drag_gesture_delay: Minimum time a finger can be in contact with the screen to be considered a
  // drag. Once this delay elapses, the recognizer tries to aggressively accept the gesture in the
  // arena.
  OneFingerDragRecognizer(DragGestureCallback on_drag_started, DragGestureCallback on_drag_update,
                          DragGestureCallback on_drag_complete,
                          zx::duration drag_gesture_delay = kMinDragDuration);
  ~OneFingerDragRecognizer() override;

  void HandleEvent(const fuchsia::ui::pointer::augment::TouchEventWithLocalHit& event) override;
  void OnWin() override;
  void OnDefeat() override;
  void OnContestStarted(std::unique_ptr<ParticipationTokenInterface> token) override;
  std::string DebugName() const override;

 private:
  // Represents state internal to a contest, i.e. participation token, accept delay, and pointer
  // state.
  struct Contest;

  // Resets gesture_context_ and contest_.
  void ResetRecognizer();

  // Helper function to handle CHANGE events (for readability to avoid nested switch statements).
  void HandleChangeEvent(const fuchsia::ui::pointer::augment::TouchEventWithLocalHit& event);

  // Returns true if distance between current touch event and event that prompted previous call to
  // update callback exceeds kMinDragDistanceForUpdate.
  bool DragDistanceExceedsUpdateThreshold(
      const fuchsia::ui::pointer::augment::TouchEventWithLocalHit& event) const;

  // Callback invoked once the drag gesture has been recognized.
  DragGestureCallback on_drag_started_;

  // Callback invoked as new CHANGE events are handled AFTER the drag gesture is recognized.
  DragGestureCallback on_drag_update_;

  // Callback invoked when the drag gesture is completed (as finger is lifted from screen).
  DragGestureCallback on_drag_complete_;

  gesture_util_v2::GestureContext gesture_context_;

  // Once a drag is recognized and the recognizer claims the win, it should call update callback
  // whenever the pointer location changes by a distance exceeding kMinDragDistanceForUpdate. In
  // order to enforce this update schedule, the recognizer needs to maintain state on the previous
  // update. This field stores the location of the previous update (if the recognizer is the winner)
  // OR the location of the previous pointer event ingested (if the recognizer is not yet the
  // winner).
  gesture_util_v2::PointerLocation previous_update_location_;

  // Minimum time a finger can be in contact with the screen to be considered a drag.
  zx::duration drag_gesture_delay_;

  std::unique_ptr<Contest> contest_;
};

}  // namespace a11y::recognizers_v2

#endif  // SRC_UI_A11Y_LIB_GESTURE_MANAGER_RECOGNIZERS_V2_ONE_FINGER_DRAG_RECOGNIZER_H_

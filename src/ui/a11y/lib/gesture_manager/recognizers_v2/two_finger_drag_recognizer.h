// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_A11Y_LIB_GESTURE_MANAGER_RECOGNIZERS_V2_TWO_FINGER_DRAG_RECOGNIZER_H_
#define SRC_UI_A11Y_LIB_GESTURE_MANAGER_RECOGNIZERS_V2_TWO_FINGER_DRAG_RECOGNIZER_H_

#include <fuchsia/ui/pointer/augment/cpp/fidl.h>
#include <lib/fit/function.h>
#include <lib/zx/time.h>

#include "src/ui/a11y/lib/gesture_manager/arena_v2/participation_token_interface.h"
#include "src/ui/a11y/lib/gesture_manager/arena_v2/recognizer_v2.h"
#include "src/ui/a11y/lib/gesture_manager/gesture_util_v2/util.h"
#include "src/ui/a11y/lib/gesture_manager/recognizers_v2/timing_constants.h"

namespace a11y::recognizers_v2 {

// TwoFingerDragRecognizer class implements logic to recognize and react to two finger drag
// gestures.
class TwoFingerDragRecognizer : public GestureRecognizerV2 {
 public:
  // Displacements of less than 1/16 are considered valid for taps, so we want
  // to recognize slightly larger gestures as drags.
  static constexpr float kDragDisplacementThreshold = 1.f / 10;

  // If the distance between the two fingers changes by more than 20%, we can
  // accept this gesture as a drag.
  static constexpr float kFingerSeparationThresholdFactor = 6.f / 5;

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
  TwoFingerDragRecognizer(DragGestureCallback on_drag_started, DragGestureCallback on_drag_update,
                          DragGestureCallback on_drag_complete,
                          zx::duration drag_gesture_delay = kMinDragDuration);
  ~TwoFingerDragRecognizer() override;

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

  // Returns true if the displacement between the gesture's starting and current
  // centroids exceeds kDragDisplacementThreshold.
  bool DisplacementExceedsThreshold();

  // Returns true if the distance between the two fingers has changed by
  // kFingerSeparationThresholdFactor relative to the start of the gesture.
  bool SeparationExceedsThreshold();

  // Callback invoked once the drag gesture has been recognized.
  DragGestureCallback on_drag_started_;

  // Callback invoked as new MOVE events are handled AFTER the drag gesture is recognized.
  DragGestureCallback on_drag_update_;

  // Callback invoked when the drag gesture is completed (as finger is lifted from screen).
  DragGestureCallback on_drag_complete_;

  gesture_util_v2::GestureContext gesture_context_;

  // Minimum time a finger can be in contact with the screen to be considered a drag.
  zx::duration drag_gesture_delay_;

  std::unique_ptr<Contest> contest_;
};

}  // namespace a11y::recognizers_v2

#endif  // SRC_UI_A11Y_LIB_GESTURE_MANAGER_RECOGNIZERS_V2_TWO_FINGER_DRAG_RECOGNIZER_H_

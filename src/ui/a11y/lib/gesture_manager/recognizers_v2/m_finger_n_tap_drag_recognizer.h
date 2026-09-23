// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_A11Y_LIB_GESTURE_MANAGER_RECOGNIZERS_V2_M_FINGER_N_TAP_DRAG_RECOGNIZER_H_
#define SRC_UI_A11Y_LIB_GESTURE_MANAGER_RECOGNIZERS_V2_M_FINGER_N_TAP_DRAG_RECOGNIZER_H_

#include <fuchsia/ui/pointer/augment/cpp/fidl.h>

#include "src/ui/a11y/lib/gesture_manager/arena_v2/participation_token_interface.h"
#include "src/ui/a11y/lib/gesture_manager/arena_v2/recognizer_v2.h"
#include "src/ui/a11y/lib/gesture_manager/gesture_util_v2/util.h"

namespace a11y::recognizers_v2 {

class MFingerNTapDragRecognizer : public GestureRecognizerV2 {
 public:
  // Displacements of less than 1/16 NDC are considered valid for taps, so we want
  // to recognize slightly larger gestures as drags.
  static constexpr float kDefaultDragDisplacementThreshold = 1.f / 10;

  // Default value for the minimum displacement between successive updates.
  // Default to updating on every CHANGE event after a win.
  static constexpr float kDefaultUpdateDisplacementThreshold = 0;

  // Callback which will be invoked when gesture has been recognized.
  using OnMFingerNTapDragCallback = fit::function<void(gesture_util_v2::GestureContext)>;

  // Constructor of this class takes in following parameters:
  //  1. on_recognize: Callback will be invoked, when the gesture is detected and the recognizer
  //     is the winner in gesture arena.
  //  2. on_update: Callback invoked on CHANGE events after the gesture has
  //     claimed the win (and only if hold_last_tap = true).
  //  3. on_complete: Callback invoked on the last REMOVE event after the gesture
  //     has claimed the win (and ony if hold_last_tap = true).
  //  4. number_of_fingers: Number of fingers in gesture.
  //  5. number_of_taps: Number of taps gesture recognizer will detect.
  // When the gesture starts, we schedule a timeout on the default dispatcher. If gesture is
  // recognized in this timeout period, then the scheduled task is cancelled. If not recognized,
  // scheduled tasks will get executed which will declare defeat for the current recognizer.
  MFingerNTapDragRecognizer(
      OnMFingerNTapDragCallback on_recognize, OnMFingerNTapDragCallback on_update,
      OnMFingerNTapDragCallback on_complete, uint32_t number_of_fingers, uint32_t number_of_taps,
      float drag_displacement_threshold = kDefaultDragDisplacementThreshold,
      float update_displacement_threshold = kDefaultUpdateDisplacementThreshold);

  ~MFingerNTapDragRecognizer() override;

  // A human-readable string name for the recognizer to be used in logs only.
  std::string DebugName() const override;

  // Processes incoming touch events to detect tap gestures like (Single, double, etc.).
  void HandleEvent(const fuchsia::ui::pointer::augment::TouchEventWithLocalHit& event) override;

  // This method gets called when the recognizer has won the arena.
  void OnWin() override;

  // This method gets called when the recognizer has lost the arena.
  // It resets the state of the participation token.
  void OnDefeat() override;

  // At the start of every arena contest this method will be called.
  // This also resets the state of the recognizer.
  void OnContestStarted(std::unique_ptr<ParticipationTokenInterface> token) override;

 private:
  // Represents state internal to a contest, i.e. participation token,
  // long-press timeout, and tap state.
  struct Contest;

  // Returns true if the displacement from |start| to |end| is at least |threshold|.
  bool DisplacementExceedsThreshold(::fuchsia::math::PointF start, ::fuchsia::math::PointF end,
                                    float threshold) const;

  // Helper method invoked when more than m fingers are in contact with the
  // screen.
  void OnExcessFingers();

  void OnTapStarted();

  void OnChangeEvent(const fuchsia::ui::pointer::augment::TouchEventWithLocalHit& event);

  void OnRemoveEvent();

  void ResetRecognizer();

  // Stores the Gesture Context which is required to execute the callback.
  gesture_util_v2::GestureContext gesture_context_ = {};

  // Stores the Gesture Context at the time of the last update.
  gesture_util_v2::GestureContext last_update_gesture_context_ = {};

  // Callback which will be executed when gesture is detected and is also a winner in the arena.
  OnMFingerNTapDragCallback on_recognize_;

  // Callback which will be executed on CHANGE events after a tap-hold gesture is detected.
  OnMFingerNTapDragCallback on_update_;

  // Callback which will be executed when the last finger is removed after a tap-hold gesture is
  // detected.
  OnMFingerNTapDragCallback on_complete_;

  std::unique_ptr<Contest> contest_;

  // Number of fingers in gesture.
  const uint32_t number_of_fingers_in_gesture_;

  // Number of taps this gesture recognizer will detect.
  const uint32_t number_of_taps_in_gesture_;

  // Minimum displacement from starting point beyond which a drag is
  // automatically accepted.
  const float drag_displacement_threshold_;

  // Minimum displacement between successive updates.
  const float update_displacement_threshold_;
};

}  // namespace a11y::recognizers_v2

#endif  // SRC_UI_A11Y_LIB_GESTURE_MANAGER_RECOGNIZERS_V2_M_FINGER_N_TAP_DRAG_RECOGNIZER_H_

// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_SCHEDULING_FRAME_PREDICTOR_H_
#define SRC_UI_SCENIC_LIB_SCHEDULING_FRAME_PREDICTOR_H_

#include <lib/zx/time.h>

namespace scheduling {

struct PredictedTimes {
  // The point at which a client should begin an update and render a frame,
  // so that it can (with acceptable probability) be displayed by |presentation_time|.
  zx::time latch_point_time;
  // The predicted presentation time. This corresponds to a future VSYNC.
  zx::time presentation_time;

  constexpr bool operator==(PredictedTimes other) const {
    return latch_point_time == other.latch_point_time &&
           presentation_time == other.presentation_time;
  }

  constexpr bool operator!=(PredictedTimes other) const { return !(*this == other); }
};

struct PredictionRequest {
  zx::time now;
  // The minimum presentation time a client would like to hit.
  zx::time requested_presentation_time;
  zx::time last_vsync_time;
  zx::duration vsync_interval;
};

// Interface for performing frame predictions. Predicts viable presentation times
// and corresponding latch-points for a frame, based on previously reported update
// and render durations.
class FramePredictor {
 public:
  virtual ~FramePredictor() = default;

  // Computes the target presentation time for
  // |request.requested_presentation_time|, and a latch-point that is early
  // enough to apply one update and render a frame, in order to hit the
  // predicted presentation time.
  //
  // Both |PredictedTimes.latch_point_time| and
  // |PredictedTimes.presentation_time| are guaranteed to be after
  // |request.now|. |PredictedTimes.presentation_time| is guaranteed to be later
  // than or equal to |request.requested_presentation_time|.
  virtual PredictedTimes GetPrediction(PredictionRequest request) const = 0;

  // Used by the client to report a measured render duration. The render
  // duration is the CPU + GPU time it takes to build and render a frame. This
  // will be considered in subsequent calls to |GetPrediction|.
  virtual void ReportRenderDuration(zx::duration time_to_render) = 0;

  // Used by the client to report a measured update duration. The update
  // duration is the time it takes to apply a batch of updates. This will be
  // considered in subsequent calls to |GetPrediction|.
  virtual void ReportUpdateDuration(zx::duration time_to_update) = 0;

  // ---------------------------------------------------------------------------
  // Utility functions common to all FramePredictors.
  //
  // These functions are intended to be used by specific FramePredictors when
  // calculating a prediction.
  // ---------------------------------------------------------------------------

  // Returns the |PredictionTimes| for a |PredictionRequest|.  |frame_preparation_time| is the
  // a duration long enough to prepare a frame for display (i.e. to update the global scene graph,
  // optionally do GPU composition, and whatever else is necessary).
  static PredictedTimes ComputePredictionFromDuration(PredictionRequest request,
                                                      zx::duration frame_preparation_time);

  // Conceptually, starts at |base_vsync_time| and repeatedly adds |vsync_interval| until arriving
  // at a time that is >= |min_vsync_time|, which is then returned.  The actual implementation is
  // more efficient.
  //
  // |base_vsync_time| Assumed to be close to a past or future vsync time.
  // |vsync_interval| The expected time between vsyncs.
  // |min_vsync_time| The minimum time allowed to return.
  static zx::time ComputeNextVsyncTime(zx::time base_vsync_time, zx::duration vsync_interval,
                                       zx::time min_vsync_time);

  // Snaps the client's `requested_presentation_time` back to the closest past vsync
  // if it overshoots it by less than the threshold.
  static zx::time SnapRequestedPresentationTime(zx::time requested_presentation_time,
                                                zx::time last_vsync_time,
                                                zx::duration vsync_interval);
};

}  // namespace scheduling

#endif  // SRC_UI_SCENIC_LIB_SCHEDULING_FRAME_PREDICTOR_H_

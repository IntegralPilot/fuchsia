// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/scheduling/frame_predictor.h"

#include <lib/syslog/cpp/macros.h>

#include <algorithm>

namespace {

// Allowed snapping threshold as a percentage of vsync_interval.
constexpr int64_t kSnapThresholdPercent = 18;
// LINT.IfChange
// Maximum duration cap for the snapping threshold.
constexpr zx::duration kMaxSnapThreshold = zx::msec(3);
// LINT.ThenChange(//src/ui/scenic/lib/scheduling/tests/default_frame_scheduler_unittest.cc)

struct VsyncProjection {
  zx::time previous_vsync;
  zx::duration overshoot;
};

// Projects a timestamp onto the Vsync grid defined by `base_vsync_time` and `vsync_interval`.
// Returns the previous Vsync boundary (<= time) and the overshoot duration.
// Assumes `time > base_vsync_time` and `vsync_interval > 0`.
VsyncProjection ProjectOntoVsyncGrid(zx::time time, zx::time base_vsync_time,
                                     zx::duration vsync_interval) {
  FX_DCHECK(time > base_vsync_time);
  FX_DCHECK(vsync_interval.get() > 0);

  const zx::duration diff = time - base_vsync_time;
  const int64_t num_intervals = diff.get() / vsync_interval.get();
  const zx::time previous_vsync = base_vsync_time + (vsync_interval * num_intervals);
  const zx::duration overshoot = time - previous_vsync;

  return {.previous_vsync = previous_vsync, .overshoot = overshoot};
}

}  // namespace

namespace scheduling {

// static
zx::time FramePredictor::SnapRequestedPresentationTime(zx::time requested_presentation_time,
                                                       zx::time last_vsync_time,
                                                       zx::duration vsync_interval) {
  if (requested_presentation_time <= last_vsync_time || vsync_interval.get() <= 0) {
    return requested_presentation_time;
  }

  const auto [previous_vsync, overshoot] =
      ProjectOntoVsyncGrid(requested_presentation_time, last_vsync_time, vsync_interval);

  const zx::duration snap_threshold =
      std::min(vsync_interval * kSnapThresholdPercent / 100, kMaxSnapThreshold);

  if (overshoot <= snap_threshold) {
    return previous_vsync;
  }

  return requested_presentation_time;
}

// static
zx::time FramePredictor::ComputeNextVsyncTime(zx::time base_vsync_time, zx::duration vsync_interval,
                                              zx::time min_vsync_time) {
  FX_DCHECK(vsync_interval.get() > 0);
  // If the base sync time is greater than or equal to the minimum acceptable
  // sync time, just return it.
  // Note: in practice, these numbers are unlikely to be identical. The "equal to"
  // comparison is necessary for tests, which have much tighter control on time.
  if (base_vsync_time >= min_vsync_time) {
    return base_vsync_time;
  }

  const auto [previous_vsync, overshoot] =
      ProjectOntoVsyncGrid(min_vsync_time, base_vsync_time, vsync_interval);

  if (overshoot.get() == 0) {
    return previous_vsync;
  }
  return previous_vsync + vsync_interval;
}

// static
PredictedTimes FramePredictor::ComputePredictionFromDuration(
    PredictionRequest request, zx::duration frame_preparation_duration) {
  // Calculate minimum time this would sync to. It is last vsync time plus half
  // a vsync-interval (to allow for jitter for the VSYNC signal), or the current
  // time plus the expected render time, whichever is larger, so we know we have
  // enough time to render for that sync.
  const zx::time min_presentation_time =
      std::max({// Guarantees a time at least one vsync interval greater than the last vsync time.
                (request.last_vsync_time + (request.vsync_interval / 2)),
                // Guarantees a time that (probably) gives enough time to prepare a frame.
                (request.now + frame_preparation_duration),
                // Guarantees a time that isn't earlier than the requested time.
                request.requested_presentation_time});

  // Clamp |min_presentation_time| to the subsequent predicted vsync time.
  const zx::time target_presentation_time =
      ComputeNextVsyncTime(request.last_vsync_time, request.vsync_interval, min_presentation_time);

  // Find time the client should latch and start rendering in order to
  // frame in time for the target present.
  const zx::time latch_point = target_presentation_time - frame_preparation_duration;

  return {.latch_point_time = latch_point, .presentation_time = target_presentation_time};
}

}  // namespace scheduling

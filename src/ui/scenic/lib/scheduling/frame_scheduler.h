// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_SCHEDULING_FRAME_SCHEDULER_H_
#define SRC_UI_SCENIC_LIB_SCHEDULING_FRAME_SCHEDULER_H_

#include <lib/fit/function.h>
#include <lib/zx/time.h>

#include <unordered_set>
#include <vector>

#include "src/ui/scenic/lib/scheduling/id.h"

namespace scheduling {

struct PresentTimestamps {
  zx::time presented_time = zx::time(0);
  zx::duration vsync_interval = zx::duration(0);
};

struct FuturePresentationInfo {
  zx::time latch_point = zx::time(0);
  zx::time presentation_time = zx::time(0);
};

using SessionsWithFailedUpdates = std::unordered_set<SessionId>;

// The timestamp data that is expected to be delivered after rendering and presenting a frame.
// TODO(https://fxbug.dev/42098890): If there are multiple render passes, |render_done_time| is the
// time furthest forward in time. Solving 24669 may involve expanding this struct to support
// multiple passes in data.
// TODO(https://fxbug.dev/42149510): when there are multiple displays, there is no single "actual
// presentation time" that the FrameRenderer can return.
struct Timestamps {
  zx::time render_done_time;
  zx::time actual_presentation_time;
};
// Time value used to signal the time measurement was dropped.
static constexpr zx::time kTimeDropped = zx::time(ZX_TIME_INFINITE);

// Callback passed to the renderer when rendering a frame. It is the responsibility of the renderer
// to trigger the callback once all timestamp data is available.
using FramePresentedCallback = fit::function<void(const Timestamps&)>;

// The FrameScheduler is responsible for scheduling frames to be drawn in response to requests from
// clients.  When a frame is requested, the FrameScheduler will decide at which Vsync the frame
// should be displayed at. This time will be no earlier than the requested time, and will be as
// close as possible to the requested time, subject to various constraints.  For example, if the
// requested time is earlier than the time that rendering would finish, were it started immediately,
// then the frame will be scheduled for a later Vsync.
class FrameScheduler {
 public:
  virtual ~FrameScheduler() = default;

  // If |render_continuously|, we keep scheduling new frames immediately after each presented frame,
  // regardless of whether they're explicitly requested using RequestFrame().
  virtual void SetRenderContinuously(bool render_continuously) = 0;

  // The scheduler will choose a presentation time based on `requested_presentation_time`,
  // according to its internal scheduling algorithm.  `squashable` determines if the update is
  // allowed to be combined with a subsequent one in case of delays.
  //
  // All calls to this method must provide a valid (non-zero) `id_pair.present_id`.
  virtual void ScheduleUpdateForSession(zx::time requested_presentation_time,
                                        SchedulingIdPair id_pair, bool squashable,
                                        bool schedule_asap) = 0;

  // Gets the predicted latch points and presentation times for the frames at or before the next
  // |requested_prediction_span| time span. Uses the FramePredictor to do so.
  virtual std::vector<FuturePresentationInfo> GetFuturePresentationInfos(
      zx::duration requested_prediction_span) = 0;

  // Removes all references to |session_id|.
  virtual void RemoveSession(SessionId session_id) = 0;

  // Clients cannot call Present() anymore when |presents_in_flight_| reaches this value. Scenic
  // uses this to apply backpressure to clients.
  // TODO(https://fxbug.dev/42120601): Move into implementation.
  static constexpr int64_t kMaxPresentsInFlight = 5;
};

}  // namespace scheduling

#endif  // SRC_UI_SCENIC_LIB_SCHEDULING_FRAME_SCHEDULER_H_

// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_FLATLAND_FLATLAND_PRESENTER_H_
#define SRC_UI_SCENIC_LIB_FLATLAND_FLATLAND_PRESENTER_H_

#include <lib/zx/counter.h>
#include <lib/zx/event.h>

#include <vector>

#include "src/ui/scenic/lib/scheduling/frame_scheduler.h"
#include "src/ui/scenic/lib/scheduling/id.h"

namespace flatland {

// Interface for Flatland instances to register user Present calls. Primarily intended to provide
// a thread-safe abstraction around a FrameScheduler.
class FlatlandPresenter {
 public:
  virtual ~FlatlandPresenter() {}

  // From scheduling::FrameScheduler::ScheduleUpdateForSession():
  //
  // Tells the frame scheduler to schedule a frame. This is also used for updates triggered by
  // something other than a Session update i.e. an ImagePipe with a new Image to present.
  //
  // Flatland should not call this function until it has reached the acquire fences and queued an
  // UberStruct for the associated |id_pair|.
  //
  // This function should be called from Flatland instance worker threads.
  virtual void ScheduleUpdateForSession(zx::time requested_presentation_time,
                                        scheduling::SchedulingIdPair id_pair, bool squashable,
                                        std::vector<zx::event> release_fences,
                                        std::vector<zx::counter> release_counters,
                                        std::vector<zx::counter> present_fences,
                                        bool schedule_asap) = 0;

  // From scheduling::FrameScheduler::GetFuturePresentationInfos():
  // Gets the predicted latch points and presentation times for the frames at or before the next
  // |requested_prediction_span| time span. Uses the FramePredictor to do so.
  //
  // The callback is guaranteed to run on the calling thread.
  virtual std::vector<scheduling::FuturePresentationInfo> GetFuturePresentationInfos() = 0;

  // From scheduling::FrameScheduler::RemoveSession():
  //
  // Removes all references to |session_id| and schedules a new frame to clean up any leftovers.
  //
  // This function should be called from Flatland instance worker threads. Final clean-up is posted
  // back on the main thread, so state may still exist after this method returns.
  //
  // `release_fence` provides the caller with a way to know that the effects of removing the session
  // have (roughly) "appeared on screen", so that it is safe to e.g. release any client images that
  // are now known not be be in use by Vulkan or the display controller.
  virtual void RemoveSession(scheduling::SessionId session_id,
                             std::optional<zx::event> release_fence) = 0;
};

}  // namespace flatland

#endif  // SRC_UI_SCENIC_LIB_FLATLAND_FLATLAND_PRESENTER_H_

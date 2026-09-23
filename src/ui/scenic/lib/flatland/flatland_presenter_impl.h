// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_FLATLAND_FLATLAND_PRESENTER_IMPL_H_
#define SRC_UI_SCENIC_LIB_FLATLAND_FLATLAND_PRESENTER_IMPL_H_

#include <lib/async/dispatcher.h>
#include <lib/zx/counter.h>
#include <lib/zx/event.h>

#include <map>
#include <memory>
#include <unordered_map>
#include <vector>

#include "src/ui/scenic/lib/flatland/flatland_presenter.h"
#include "src/ui/scenic/lib/scheduling/frame_scheduler.h"
#include "src/ui/scenic/lib/scheduling/id.h"

namespace flatland {

class FlatlandPresenterImpl final : public FlatlandPresenter,
                                    public std::enable_shared_from_this<FlatlandPresenterImpl> {
 public:
  struct Fences {
    // Fences which are signaled when the resources associated with the fences are safe to reuse,
    // according to the semantics defined by `fuchsia.ui.composition/PresentArgs.release_fences`,
    // which are too complicated to replicate here.
    std::vector<zx::event> release_fences;

    // Fences which are signaled when the resources associated with the fences are safe to reuse.
    // This is similar to `release_fences` but uses counters instead of events.
    std::vector<zx::counter> release_counters;

    // Fences which are signaled when a Vsync event notifies Scenic that the frame corresponding to
    // the fences has been presented.
    std::vector<zx::counter> present_fences;
  };

  // The |main_dispatcher| must be the dispatcher that GFX sessions run and update on. That thread
  // is typically refered to as the "main thread" or "render thread".
  // FrameScheduler is what FlatlandPresenterImpl will use for frame scheduling calls.
  FlatlandPresenterImpl(async_dispatcher_t* main_dispatcher,
                        scheduling::FrameScheduler& frame_scheduler);

  // |FlatlandPresenter|
  void ScheduleUpdateForSession(zx::time requested_presentation_time,
                                scheduling::SchedulingIdPair id_pair, bool unsquashable,
                                std::vector<zx::event> release_fences,
                                std::vector<zx::counter> release_counters,
                                std::vector<zx::counter> present_fences,
                                bool schedule_asap) override;

  // |FlatlandPresenter|.
  std::vector<scheduling::FuturePresentationInfo> GetFuturePresentationInfos() override;

  // |FlatlandPresenter|
  void RemoveSession(scheduling::SessionId session_id,
                     std::optional<zx::event> release_fence) override;

  // Called at FrameScheduler's UpdateSessions() time.
  // Takes the fences up to the corresponding PresentId for each SessionId in |sessions_to_update|
  // and moves them to an internal set of "accumulated" fences.  These fences can be retrieved
  // by the caller at any time via TakeFences(), which clears the accumulated fences.  This scheme
  // allows the accumulation of fences for multiple UpdateSessions() calls per frame.
  void AccumulateFences(
      const std::unordered_map<scheduling::SessionId, scheduling::PresentId>& sessions_to_update);

  // Return all fences that were accumulated during calls to UpdateSessions().  The caller
  // takes responsibility for signaling these fences at the appropriate time (see comments on
  // `struct Fences`).
  Fences TakeFences();

 private:
  async_dispatcher_t* const main_dispatcher_;
  scheduling::FrameScheduler& frame_scheduler_;

  // Fences that correspond to a scheduled frame that hasn't been presented yet.
  std::map<scheduling::SchedulingIdPair, Fences> pending_fences_;

  Fences accumulated_fences_;

  // The number of frames to request from GetFuturePresentationInfos().
  const int64_t kFuturePresentationInfoCount = 2;

  // The default frame interval assumes a 60Hz display.
  const zx::duration kDefaultFrameInterval = zx::usec(16'667);

  const zx::duration kDefaultPredictionSpan = kDefaultFrameInterval * kFuturePresentationInfoCount;
};

}  // namespace flatland

#endif  // SRC_UI_SCENIC_LIB_FLATLAND_FLATLAND_PRESENTER_IMPL_H_

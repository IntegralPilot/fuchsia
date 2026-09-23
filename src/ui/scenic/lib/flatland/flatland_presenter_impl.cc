// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/flatland/flatland_presenter_impl.h"

#include <lib/async/cpp/task.h>
#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>

#include <iterator>

#include "src/ui/scenic/lib/scheduling/id.h"

namespace flatland {

FlatlandPresenterImpl::FlatlandPresenterImpl(async_dispatcher_t* main_dispatcher,
                                             scheduling::FrameScheduler& frame_scheduler)
    : main_dispatcher_(main_dispatcher), frame_scheduler_(frame_scheduler) {}

void FlatlandPresenterImpl::AccumulateFences(
    const std::unordered_map<scheduling::SessionId, scheduling::PresentId>& sessions_to_update) {
  TRACE_DURATION("gfx", "FlatlandPresenterImpl::AccumulateFences");
  FX_DCHECK(main_dispatcher_ == async_get_default_dispatcher());
  for (const auto& [session_id, present_id] : sessions_to_update) {
    auto start_it = pending_fences_.lower_bound({session_id, 0});
    auto end_it = pending_fences_.upper_bound({session_id, present_id});

    for (auto it = start_it; it != end_it; ++it) {
      // Append release fences.
      accumulated_fences_.release_fences.insert(
          accumulated_fences_.release_fences.end(),
          std::make_move_iterator(it->second.release_fences.begin()),
          std::make_move_iterator(it->second.release_fences.end()));
      it->second.release_fences.clear();

      // Append release counters.
      accumulated_fences_.release_counters.insert(
          accumulated_fences_.release_counters.end(),
          std::make_move_iterator(it->second.release_counters.begin()),
          std::make_move_iterator(it->second.release_counters.end()));
      it->second.release_counters.clear();

      // Append present fences.
      accumulated_fences_.present_fences.insert(
          accumulated_fences_.present_fences.end(),
          std::make_move_iterator(it->second.present_fences.begin()),
          std::make_move_iterator(it->second.present_fences.end()));
      it->second.present_fences.clear();
    }
    pending_fences_.erase(start_it, end_it);
  }
}

FlatlandPresenterImpl::Fences FlatlandPresenterImpl::TakeFences() {
  TRACE_DURATION("gfx", "FlatlandPresenterImpl::TakeFences");
  FX_DCHECK(main_dispatcher_ == async_get_default_dispatcher());
  Fences taken_fences = std::move(accumulated_fences_);
  accumulated_fences_ = {};
  return taken_fences;
}

void FlatlandPresenterImpl::ScheduleUpdateForSession(
    zx::time requested_presentation_time, scheduling::SchedulingIdPair id_pair, bool unsquashable,
    std::vector<zx::event> release_fences, std::vector<zx::counter> release_counters,
    std::vector<zx::counter> present_fences, bool schedule_asap) {
  // TODO(https://fxbug.dev/42139440): The FrameScheduler is not thread-safe, but a lock is not
  // sufficient since GFX sessions may access the FrameScheduler without passing through this
  // object. Post a task to the main thread, which is where GFX runs, to account for thread safety.
  async::PostTask(
      main_dispatcher_,
      [thiz = shared_from_this(), requested_presentation_time, id_pair, unsquashable,
       release_fences = std::move(release_fences), release_counters = std::move(release_counters),
       present_fences = std::move(present_fences), schedule_asap]() mutable {
        TRACE_DURATION("gfx", "FlatlandPresenterImpl::ScheduleUpdateForSession[task]");
        FX_DCHECK(!thiz->pending_fences_.contains(id_pair));
        thiz->pending_fences_.emplace(id_pair,
                                      Fences{.release_fences = std::move(release_fences),
                                             .release_counters = std::move(release_counters),
                                             .present_fences = std::move(present_fences)});
        thiz->frame_scheduler_.ScheduleUpdateForSession(requested_presentation_time, id_pair,
                                                        !unsquashable, schedule_asap);
      });
}

std::vector<scheduling::FuturePresentationInfo>
FlatlandPresenterImpl::GetFuturePresentationInfos() {
  FX_DCHECK(main_dispatcher_ == async_get_default_dispatcher());
  return frame_scheduler_.GetFuturePresentationInfos(kDefaultPredictionSpan);
}

void FlatlandPresenterImpl::RemoveSession(scheduling::SessionId session_id,
                                          std::optional<zx::event> release_fence) {
  async::PostTask(main_dispatcher_, [thiz = shared_from_this(), session_id,
                                     release_fence = std::move(release_fence)]() mutable {
    TRACE_DURATION("gfx", "FlatlandPresenterImpl::RemoveSession[task]");
    // Remove any registered fences for the removed session.
    {
      auto start = thiz->pending_fences_.lower_bound({session_id, 0});
      auto end = thiz->pending_fences_.lower_bound({session_id + 1, 0});
      thiz->pending_fences_.erase(start, end);
    }

    scheduling::SchedulingIdPair id_pair{session_id, scheduling::GetNextPresentId()};

    // If provided, add one final release fence for cleanup.
    if (release_fence.has_value()) {
      FX_DCHECK(release_fence.value());
      std::vector<zx::event> release_fences;
      release_fences.emplace_back(std::move(*release_fence));
      thiz->pending_fences_.emplace(
          id_pair, Fences{.release_fences = std::move(release_fences), .present_fences = {}});
    }

    // Ensure that in case no client is currently rendering we'll still produce a new frame to clean
    // up any leftovers from the dead one.
    // The sequencing of RemoveSession() followed by scheduling a new present for the same ID
    // ensures both that there will be no collisions for the |session_id| used and that we'll
    // schedule exactly one frame for the shortest possible timeframe.
    thiz->frame_scheduler_.RemoveSession(session_id);
    thiz->frame_scheduler_.ScheduleUpdateForSession(zx::time(0), id_pair, /*squashable=*/true,
                                                    /*schedule_asap=*/false);
  });
}

}  // namespace flatland
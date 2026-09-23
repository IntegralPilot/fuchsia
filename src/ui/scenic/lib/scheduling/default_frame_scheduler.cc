// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/scheduling/default_frame_scheduler.h"

#include <lib/async/default.h>
#include <lib/async/time.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>

#include "src/ui/scenic/lib/utils/logging.h"

namespace {

static const uint64_t kNumDebugFrames = 3;

template <class T>
static void RemoveSessionIdFromMap(scheduling::SessionId session_id,
                                   std::map<scheduling::SchedulingIdPair, T>* map) {
  auto start = map->lower_bound({session_id, 0});
  auto end = map->lower_bound({session_id + 1, 0});
  map->erase(start, end);
}

}  // namespace

namespace scheduling {

DefaultFrameScheduler::DefaultFrameScheduler(std::unique_ptr<FramePredictor> predictor,
                                             inspect::Node inspect_node,
                                             metrics::Metrics* metrics_logger)
    : dispatcher_(async_get_default_dispatcher()),
      frame_predictor_(std::move(predictor)),
      inspect_node_(std::move(inspect_node)),
      stats_(inspect_node_.CreateChild("Frame Stats"), metrics_logger),
      weak_factory_(this) {
  FX_DCHECK(frame_predictor_);

  inspect_frame_number_ = inspect_node_.CreateUint("most_recent_frame_number", frame_number_);
  inspect_wakeups_without_render_ = inspect_node_.CreateUint("wakeups_without_rendering", 0);
  inspect_last_successful_update_start_time_ =
      inspect_node_.CreateUint("last_successful_update_start_time", 0);
  inspect_last_successful_target_presentation_time_ =
      inspect_node_.CreateUint("last_successful_target_presentation_time", 0);
}

DefaultFrameScheduler::~DefaultFrameScheduler() {}

void DefaultFrameScheduler::Initialize(std::shared_ptr<const VsyncTiming> vsync_timing,
                                       UpdateSessions update_sessions,
                                       OnCpuWorkDone on_cpu_work_done,
                                       OnFramePresented on_frame_presented,
                                       RenderScheduledFrame render_scheduled_frame) {
  FX_CHECK(vsync_timing_ == nullptr) << "Tried to initialize twice";
  vsync_timing_ = vsync_timing;
  update_sessions_ = std::move(update_sessions);
  on_cpu_work_done_ = std::move(on_cpu_work_done);
  on_frame_presented_ = std::move(on_frame_presented);
  render_scheduled_frame_ = std::move(render_scheduled_frame);
}

void DefaultFrameScheduler::SetRenderContinuously(bool render_continuously) {
  render_continuously_ = render_continuously;
  if (render_continuously_) {
    RequestFrame(zx::time(0), /*schedule_asap=*/false);
  }
}

std::pair<zx::time, zx::time> DefaultFrameScheduler::ComputePresentationAndWakeupTimesForTargetTime(
    const zx::time& requested_presentation_time, bool schedule_asap) const {
  const zx::time& last_vsync_time = vsync_timing_->last_vsync_time();
  const zx::duration& vsync_interval = vsync_timing_->vsync_interval();
  FX_DCHECK(vsync_interval.get() >= 0);
  FX_DCHECK(last_vsync_time.get() >= 0);
  const zx::time& now = zx::time(async_now(dispatcher_));

  if (schedule_asap) {
    // If the client requested a future time, we should respect it. We only want to schedule ASAP
    // if the requested time is effectively "now" (i.e. within the current vsync interval), or
    // explicitly 0.
    const zx::time next_vsync_time = last_vsync_time + vsync_interval;
    if (requested_presentation_time <= next_vsync_time) {
      // We target scheduling the frame as soon as possible, i.e. "now". However, kernel CPU
      // scheduling delays might result in new_target_presentation_time being in the past. We pick
      // the earlier time to avoid violating the DCHECK invariant in ApplyUpdates().
      return std::make_pair(next_vsync_time, std::min(now, next_vsync_time));
    }
  }

  const PredictedTimes& times =
      frame_predictor_->GetPrediction({.now = now,
                                       .requested_presentation_time = requested_presentation_time,
                                       .last_vsync_time = last_vsync_time,
                                       .vsync_interval = vsync_interval});

  return std::make_pair(times.presentation_time, times.latch_point_time);
}

void DefaultFrameScheduler::RequestFrame(zx::time requested_presentation_time, bool schedule_asap) {
  FX_DCHECK(HaveUpdatableSessions() || render_continuously_ || !last_frame_is_presented_);

  auto [new_target_presentation_time, new_wakeup_time] =
      ComputePresentationAndWakeupTimesForTargetTime(requested_presentation_time, schedule_asap);

  TRACE_DURATION(
      "gfx", "DefaultFrameScheduler::RequestFrame", "requested presentation time",
      requested_presentation_time.get() / 1'000'000, "target_presentation_time",
      new_target_presentation_time.get() / 1'000'000, "candidate wakeup time",
      new_wakeup_time.get() / 1'000'000, "current wakeup time", wakeup_time_.get() / 1'000'000,
      "now", zx::time(async_now(dispatcher_)).get() / 1'000'000, "schedule_asap", schedule_asap);

  // Output requested presentation time in milliseconds.
  // Logging the first few frames to find common startup bugs.
  if (frame_number_ <= kNumDebugFrames) {
    FX_LOGS(DEBUG) << "FrameScheduler::RequestFrame() times requested="
                   << requested_presentation_time.get()
                   << "  target=" << new_target_presentation_time.get()
                   << "  wakeup=" << new_wakeup_time.get();
  } else {
    FLATLAND_VERBOSE_LOG << "FrameScheduler::RequestFrame() times requested="
                         << requested_presentation_time.get()
                         << "  target=" << new_target_presentation_time.get()
                         << "  wakeup=" << new_wakeup_time.get();
  }

  // If there is no render waiting we should schedule a frame. Likewise, if newly predicted wake up
  // time is earlier than the current one then we need to reschedule the next wake-up.
  if (!frame_render_task_.is_pending() || new_wakeup_time < wakeup_time_) {
    frame_render_task_.Cancel();

    next_target_presentation_time_ = new_target_presentation_time;
    wakeup_time_ = new_wakeup_time;
    frame_render_task_.PostForTime(dispatcher_, wakeup_time_);
  }
}

void DefaultFrameScheduler::HandleNextFrameRequest() {
  // Finds and requests a frame for the lowest requested_presentation_time across all sessions'
  // next update.
  if (!pending_present_requests_.empty()) {
    SessionId last_session = scheduling::kInvalidSessionId;
    zx::time next_min_time = zx::time(std::numeric_limits<zx_time_t>::max());
    bool schedule_asap = false;
    for (const auto& [id_pair, request] : pending_present_requests_) {
      if (id_pair.session_id != last_session &&
          !sessions_with_unsquashable_updates_pending_presentation_.contains(id_pair.session_id)) {
        last_session = id_pair.session_id;
        next_min_time = std::min(next_min_time, request.requested_presentation_time);
        schedule_asap |= request.schedule_asap;
      }
    }

    if (next_min_time.get() != std::numeric_limits<zx_time_t>::max()) {
      RequestFrame(next_min_time, /*schedule_asap=*/schedule_asap);
    }
  }
}

void DefaultFrameScheduler::MaybeRenderFrame(async_dispatcher_t*, async::TaskBase*, zx_status_t) {
  const uint64_t frame_number = frame_number_;

  {
    // Trace event to track the delta between the targeted wakeup_time_ and the actual wakeup
    // time. It is used to detect delays (i.e. if this thread is blocked on the cpu). The intended
    // wakeup_time_ is used to track the canonical "start" of this frame at various points during
    // the frame's execution.
    const zx::duration wakeup_delta = zx::time(async_now(dispatcher_)) - wakeup_time_;
    TRACE_COUNTER("gfx", "Wakeup Time Delta", /* counter_id */ 0, "delta", wakeup_delta.get());
  }

  const auto target_presentation_time = next_target_presentation_time_;
  TRACE_DURATION("gfx", "FrameScheduler::MaybeRenderFrame", "target_presentation_time",
                 target_presentation_time.get() / 1'000'000);

  // Logging the first few frames to find common startup bugs.
  if (frame_number < kNumDebugFrames) {
    FX_LOGS(DEBUG) << "FrameScheduler::MaybeRenderFrame() frame_number=" << frame_number
                   << "  target_presentation_time=" << target_presentation_time.get()
                   << "  wakeup_time=" << wakeup_time_.get();
  } else {
    FLATLAND_VERBOSE_LOG << "FrameScheduler::MaybeRenderFrame() frame_number=" << frame_number
                         << "  target_presentation_time=" << target_presentation_time.get()
                         << "  wakeup_time=" << wakeup_time_.get();
  }

  // Apply all updates
  const zx::time update_start_time = zx::time(async_now(dispatcher_));

  // The second value, |wakeup_time_|, here is important for ensuring our flows stay connected.
  // If you change it please ensure the "request_to_render" flow stays connected.
  const bool needs_render = ApplyUpdates(target_presentation_time, wakeup_time_, frame_number);

  if (needs_render) {
    inspect_last_successful_update_start_time_.Set(update_start_time.get());
    last_successful_update_start_time_ = update_start_time;
  }

  // TODO(https://fxbug.dev/42098890) Revisit how we do this.
  const zx::time update_end_time = zx::time(async_now(dispatcher_));
  const zx::time render_start_time = update_end_time;
  frame_predictor_->ReportUpdateDuration(zx::duration(update_end_time - update_start_time));

  if (!needs_render && last_frame_is_presented_ && !render_continuously_) {
    FLATLAND_VERBOSE_LOG << "FrameScheduler::MaybeRenderFrame() frame_number=" << frame_number
                         << "  target_presentation_time=" << target_presentation_time.get()
                         << "  skipping render because there is nothing to render.";

    inspect_wakeups_without_render_.Set(++wakeups_without_render_);

    TRACE_FLOW_END("gfx", "scenic_frame", frame_number);

    // Nothing to render. Continue with next request in the queue.
    HandleNextFrameRequest();
    return;
  }

  // TODO(https://fxbug.dev/42098738) Remove the presentation check, and pipeline frames within a
  // VSYNC interval.
  FX_DCHECK(last_presented_frame_number_ <= frame_number);

  // Only one frame is allowed "in flight" at any given time.
  // Don't start rendering another frame until the previous frame is on the display.
  if (last_presented_frame_number_ < (frame_number - 1)) {
    TRACE_INSTANT("gfx", "scenic_frame_dropped: too many frames in flight", TRACE_SCOPE_THREAD,
                  "frame_number", frame_number);

    FLATLAND_VERBOSE_LOG << "FrameScheduler::MaybeRenderFrame() frame_number=" << frame_number
                         << "  target_presentation_time=" << target_presentation_time.get()
                         << "  skipping render because frame_number="
                         << (last_presented_frame_number_ + 1) << "  is still in flight";

    TRACE_FLOW_END("gfx", "scenic_frame", frame_number);

    last_frame_is_presented_ = false;
    return;
  }

  last_frame_is_presented_ = true;

  // Logging the first few frames to find common startup bugs.
  if (frame_number < kNumDebugFrames) {
    FX_LOGS(INFO) << "FrameScheduler::MaybeRenderFrame() frame_number=" << frame_number
                  << "  target_presentation_time=" << target_presentation_time.get()
                  << "  ... calling RenderFrame";
  } else {
    FLATLAND_VERBOSE_LOG << "FrameScheduler::MaybeRenderFrame() frame_number=" << frame_number
                         << "  target_presentation_time=" << target_presentation_time.get()
                         << "  ... calling RenderFrame";
  }

  TRACE_INSTANT("gfx", "Render start", TRACE_SCOPE_PROCESS, "Expected presentation time",
                target_presentation_time.get(), "frame_number", frame_number);

  const trace_flow_id_t frame_render_trace_id = TRACE_NONCE();
  TRACE_FLOW_BEGIN("gfx", "render_to_presented", frame_render_trace_id);
  auto on_presented_callback = [=,
                                weak = weak_factory_.GetWeakPtr()](const Timestamps& timestamps) {
    TRACE_FLOW_END("gfx", "render_to_presented", frame_render_trace_id);
    if (weak) {
      weak->HandleFramePresented(frame_number, render_start_time, target_presentation_time,
                                 timestamps);
    } else {
      FX_LOGS(ERROR) << "Error, cannot record presentation time: FrameScheduler does not exist";
    }
  };
  outstanding_latch_points_.push_back(update_end_time);

  inspect_frame_number_.Set(frame_number);

  // Render the frame.
  render_scheduled_frame_(frame_number, target_presentation_time, std::move(on_presented_callback));

  // Let all Session Updaters know of the timing of the end of RenderFrame().
  on_cpu_work_done_();

  // Schedule next frame if any unhandled presents are left.
  ++frame_number_;
  HandleNextFrameRequest();
}

void DefaultFrameScheduler::ScheduleUpdateForSession(zx::time requested_presentation_time,
                                                     SchedulingIdPair id_pair, bool squashable,
                                                     bool schedule_asap) {
  FX_DCHECK(id_pair.present_id != kInvalidPresentId);
  FX_DCHECK(id_pair.session_id != scheduling::kInvalidSessionId);
  FX_DCHECK((presents_.lower_bound(id_pair) == presents_.end()) ||
            (presents_.lower_bound(id_pair)->first.session_id != id_pair.session_id))
      << "PresentIds for a Session must be submitted in order";

  // Reserve a slot so we can track and report the time that this present was latched, even if it is
  // squashed with later ones.
  presents_[id_pair] = std::nullopt;

  TRACE_DURATION("gfx", "DefaultFrameScheduler::ScheduleUpdateForSession",
                 "requested_presentation_time", requested_presentation_time.get() / 1'000'000);

  // Utilized in low-hanging optimizations below.  If desired, we could also optimize TRACE_DURATION
  // calls, although (because we could no longer rely on a RAII scope for duration) we would need
  // to split each into a TRACE_DURATION_BEGIN/TRACE_DURATION_END pair.
  const bool trace_enabled = TRACE_CATEGORY_ENABLED("gfx");

  // Micro-optimize tracing.
  if (trace_enabled) {
    // TODO(https://fxbug.dev/414450649): remove this, since it is a subset of the
    // `scenic_session_present` flow.  This will require updating trace-processing scripts.
    TRACE_FLOW_END("gfx", "ScheduleUpdate", id_pair.present_id);

    TRACE_INSTAFLOW_STEP("gfx", "scenic_session_present", "request_frame",
                         SESSION_TRACE_ID(id_pair.session_id, id_pair.present_id), "session_id",
                         TA_UINT64(id_pair.session_id), "present_id",
                         TA_UINT64(id_pair.present_id));
  }

  const zx::time snapped_presentation_time = FramePredictor::SnapRequestedPresentationTime(
      requested_presentation_time, vsync_timing_->last_vsync_time(),
      vsync_timing_->vsync_interval());

  // Logging the first few frames to find common startup bugs.
  if (frame_number_ < kNumDebugFrames) {
    FX_LOGS(DEBUG) << "FrameScheduler::ScheduleUpdateForSession() session_id=" << id_pair.session_id
                   << "  present_id=" << id_pair.present_id
                   << "  requested_presentation_time=" << requested_presentation_time.get()
                   << "  snapped_presentation_time=" << snapped_presentation_time.get();
  } else {
    FLATLAND_VERBOSE_LOG << "FrameScheduler::ScheduleUpdateForSession() session_id="
                         << id_pair.session_id << "  present_id=" << id_pair.present_id
                         << "  requested_presentation_time=" << requested_presentation_time.get()
                         << "  snapped_presentation_time=" << snapped_presentation_time.get();
  }

  const trace_flow_id_t flow_id = trace_enabled ? TRACE_NONCE() : 0;
  if (trace_enabled) {
    TRACE_FLOW_BEGIN("gfx", "request_to_render", flow_id);
  }
  pending_present_requests_.emplace(std::make_pair(
      id_pair, PresentRequest{.requested_presentation_time = snapped_presentation_time,
                              .flow_id = flow_id,
                              .squashable = squashable,
                              .schedule_asap = schedule_asap}));

  HandleNextFrameRequest();
}

std::vector<FuturePresentationInfo> DefaultFrameScheduler::GetFuturePresentationInfos(
    zx::duration requested_prediction_span) {
  std::vector<FuturePresentationInfo> infos;

  PredictionRequest request;
  request.now = zx::time(async_now(dispatcher_));
  request.last_vsync_time = vsync_timing_->last_vsync_time();

  // We assume this value is constant, at least for the near future.
  request.vsync_interval = vsync_timing_->vsync_interval();
  FX_DCHECK(request.last_vsync_time.get() >= 0);
  FX_DCHECK(request.vsync_interval.get() >= 0);

  constexpr static const uint64_t kMaxPredictionCount = 8;
  uint64_t count = 0;

  zx::time prediction_limit = request.now + requested_prediction_span;
  while (request.now <= prediction_limit && count < kMaxPredictionCount) {
    // We ask for a "0 time" in order to give us the next possible presentation time. It also fits
    // the Present() pattern most Scenic clients currently use.
    request.requested_presentation_time = zx::time(0);

    PredictedTimes times = frame_predictor_->GetPrediction(request);
    infos.push_back(
        {.latch_point = times.latch_point_time, .presentation_time = times.presentation_time});

    // The new now time is one tick after the returned latch point. This ensures uniqueness in the
    // results we give to the client since we know we cannot schedule a frame for a latch point in
    // the past.
    //
    // We also guarantee loop termination by the same token. Latch points are monotonically
    // increasing, which means so is |request.now| so it will eventually reach prediction_limit.
    request.now = times.latch_point_time + zx::duration(1);

    // last_vsync_time should be the greatest value less than request.now where a vsync
    // occurred. We can calculate this inductively by adding vsync_intervals to last_vsync_time.
    // Therefore what we add to last_vsync_time is the difference between now and
    // last_vsync_time, integer divided by vsync_interval, then multipled by vsync_interval.
    //
    // Because now' is the latch_point, and latch points are monotonically increasing, we
    // guarantee that |difference| and therefore last_vsync_time is also monotonically increasing.
    zx::duration difference = request.now - request.last_vsync_time;
    uint64_t num_intervals = difference / request.vsync_interval;
    request.last_vsync_time += request.vsync_interval * num_intervals;

    ++count;
  }

  ZX_DEBUG_ASSERT(infos.size() >= 1);
  return infos;
}

void DefaultFrameScheduler::HandleFramePresented(uint64_t frame_number, zx::time render_start_time,
                                                 zx::time target_presentation_time,
                                                 const Timestamps& timestamps) {
  FX_DCHECK(frame_number == last_presented_frame_number_ + 1);
  FX_DCHECK(vsync_timing_->vsync_interval().get() >= 0);

  if (frame_number < kNumDebugFrames) {
    FX_LOGS(INFO) << "DefaultFrameScheduler::HandleFramePresented() frame_number=" << frame_number;
  } else {
    FLATLAND_VERBOSE_LOG << "FrameScheduler::HandleFramePresented() frame_number=" << frame_number;
  }

  last_presented_frame_number_ = frame_number;

  FrameStats::Timestamps frame_stats = {
      .latch_point_time = outstanding_latch_points_.front(),
      .render_start_time = render_start_time,
      .render_done_time = timestamps.render_done_time,
      .target_presentation_time = target_presentation_time,
      .actual_presentation_time = timestamps.actual_presentation_time,
  };

  stats_.RecordFrame(frame_stats, vsync_timing_->vsync_interval());

  if (timestamps.render_done_time != kTimeDropped) {
    zx::duration duration =
        std::max(timestamps.render_done_time - render_start_time, zx::duration(0));
    frame_predictor_->ReportRenderDuration(zx::duration(duration));
    inspect_last_successful_target_presentation_time_.Set(target_presentation_time.get());
    last_successful_target_presentation_time_ = target_presentation_time;
  }

  if (timestamps.actual_presentation_time == kTimeDropped) {
    TRACE_INSTANT("gfx", "FrameDropped", TRACE_SCOPE_PROCESS, "frame_number", frame_number);
  } else {
    if (TRACE_CATEGORY_ENABLED("gfx")) {
      // Log trace data..
      zx::duration target_vs_actual =
          timestamps.actual_presentation_time - target_presentation_time;

      zx::time now = zx::time(async_now(dispatcher_));
      zx::duration elapsed_since_presentation = now - timestamps.actual_presentation_time;
      FX_DCHECK(elapsed_since_presentation.get() >= 0);

      TRACE_INSTANT("gfx", "FramePresented", TRACE_SCOPE_PROCESS, "frame_number", frame_number,
                    "presentation time", timestamps.actual_presentation_time.get(),
                    "target time missed by", target_vs_actual.get(),
                    "elapsed time since presentation", elapsed_since_presentation.get());
    }

    SignalPresentedUpTo(frame_number,
                        /*actual_presentation_time*/ timestamps.actual_presentation_time,
                        /*presentation_interval*/ vsync_timing_->vsync_interval());
  }
  outstanding_latch_points_.pop_front();

  sessions_with_unsquashable_updates_pending_presentation_.clear();

  if (!last_frame_is_presented_ || render_continuously_) {
    RequestFrame(zx::time(0), /*schedule_asap=*/false);
  } else {
    // Schedule next frame if any unhandled presents are left.
    HandleNextFrameRequest();
  }
}

void DefaultFrameScheduler::RemoveSession(SessionId session_id) {
  RemoveSessionIdFromMap(session_id, &pending_present_requests_);
  const auto begin_it = presents_.lower_bound({session_id, 0});
  const auto end_it = presents_.upper_bound({session_id, std::numeric_limits<PresentId>::max()});
  presents_.erase(begin_it, end_it);
}

std::unordered_map<SessionId, PresentId> DefaultFrameScheduler::CollectUpdatesForThisFrame(
    zx::time target_presentation_time) {
  std::unordered_map<SessionId, PresentId> updates;

  SessionId current_session = scheduling::kInvalidSessionId;
  bool hit_limit = false;
  bool preceding_update_is_squashable = true;
  auto it = pending_present_requests_.begin();
  while (it != pending_present_requests_.end()) {
    auto& [id_pair, present_request] = *it;

    if (current_session != id_pair.session_id) {
      current_session = id_pair.session_id;
      hit_limit = false;
      preceding_update_is_squashable = true;
    }

    if (!hit_limit && present_request.requested_presentation_time <= target_presentation_time &&
        preceding_update_is_squashable &&
        !sessions_with_unsquashable_updates_pending_presentation_.contains(id_pair.session_id)) {
      if (present_request.flow_id) {
        TRACE_FLOW_END("gfx", "request_to_render", present_request.flow_id);
      }
      // Return only the last relevant present id for each session.
      updates[current_session] = id_pair.present_id;
      if (!present_request.squashable) {
        sessions_with_unsquashable_updates_pending_presentation_.emplace(id_pair.session_id);
      }

      preceding_update_is_squashable = present_request.squashable;
      it = pending_present_requests_.erase(it);
    } else {
      hit_limit = true;
      ++it;
    }
  }

#if defined(USE_FLATLAND_VERBOSE_LOGGING)
  if (updates.empty()) {
    FLATLAND_VERBOSE_LOG << "FrameScheduler::CollectUpdatesForThisFrame() frame_number="
                         << frame_number_ << "  no updates for target_presentation_time="
                         << target_presentation_time.get();
  } else {
    std::ostringstream oss;
    for (const auto& [session_id, present_id] : updates) {
      oss << "\n                    session_id=" << session_id << " present_id=" << present_id;
    }
    FLATLAND_VERBOSE_LOG << "FrameScheduler::CollectUpdatesForThisFrame() frame_number="
                         << frame_number_ << "  updates for target_presentation_time="
                         << target_presentation_time.get() << oss.str();
  }
#endif

  return updates;
}

void DefaultFrameScheduler::PrepareUpdates(const std::unordered_map<SessionId, PresentId>& updates,
                                           zx::time latched_time, uint64_t frame_number) {
  TRACE_DURATION("gfx", "FrameScheduler::PrepareUpdates");

  latched_updates_.push(
      {.frame_number = frame_number, .updated_sessions = updates, .latched_time = latched_time});

  for (const auto& [session_id, present_id] : updates) {
    SetLatchedTimeForPresentsUpTo({.session_id = session_id, .present_id = present_id},
                                  latched_time);
  }
}

void DefaultFrameScheduler::SetLatchedTimeForPresentsUpTo(SchedulingIdPair id_pair,
                                                          zx::time latched_time) {
  const auto begin_it = presents_.lower_bound({id_pair.session_id, 0});
  const auto end_it = presents_.upper_bound(id_pair);
  std::for_each(begin_it, end_it,
                [latched_time](std::pair<const SchedulingIdPair, std::optional<zx::time>>& pair) {
                  // Update latched time for Present2Infos that haven't already been latched on
                  // previous frames.
                  if (pair.second == std::nullopt)
                    pair.second = latched_time;
                });
}

bool DefaultFrameScheduler::ApplyUpdates(zx::time target_presentation_time, zx::time latched_time,
                                         uint64_t frame_number) {
  FX_DCHECK(latched_time <= target_presentation_time)
      << "latched_time=" << latched_time.get()
      << " target_presentation_time=" << target_presentation_time.get()
      << " frame number=" << frame_number;

  // Logging the first few frames to find common startup bugs.
  if (frame_number < kNumDebugFrames) {
    FX_LOGS(DEBUG) << "FrameScheduler::ApplyScheduledSessionUpdates() frame_number=" << frame_number
                   << "  target_presentation_time=" << target_presentation_time.get();
  } else {
    FLATLAND_VERBOSE_LOG << "FrameScheduler::ApplyScheduledSessionUpdates() frame_number="
                         << frame_number
                         << "  target_presentation_time=" << target_presentation_time.get();
  }

  // NOTE: this name is used by scenic_frame_stats.dart
  TRACE_DURATION("gfx", "ApplyScheduledSessionUpdates", "target_presentation_time",
                 target_presentation_time.get() / 1'000'000, "frame_number", frame_number);

  TRACE_FLOW_BEGIN("gfx", "scenic_frame", frame_number);

  // TODO(https://fxbug.dev/460278647): together these take significant time.  There are several
  // possibilities for optimization:
  //   - return vector instead of unordered_map
  //   - reuse memory instead of allocating extra frame (more effective with vectors than maps)
  const std::unordered_map<SessionId, PresentId> update_map =
      CollectUpdatesForThisFrame(target_presentation_time);
  PrepareUpdates(update_map, latched_time, frame_number);

  // Micro-optimize tracing.
  if (TRACE_CATEGORY_ENABLED("gfx")) {
    // The straightforward approach would be to use TRACE_INSTAFLOW_STEP for each session-present,
    // but there is a non-negligible cost if there are multiple presents.
    TRACE_DURATION("gfx", "scenic_session_present/prepare_to_render", "frame_number",
                   TA_UINT64(frame_number), "latched_time", TA_INT64(latched_time.get()));
    for (auto [session_id, present_id] : update_map) {
      TRACE_FLOW_STEP("gfx", "scenic_session_present/prepare_to_render",
                      SESSION_TRACE_ID(session_id, present_id));
    }
  }

  update_sessions_(update_map, frame_number);

  // If anything was updated, we need to render.
  return !update_map.empty();
}

void DefaultFrameScheduler::SignalPresentedUpTo(uint64_t frame_number,
                                                zx::time actual_presentation_time,
                                                zx::duration presentation_interval) {
  // Get last present_id up to |frame_number| for each session.
  std::unordered_map<SessionId, PresentId> last_updates;
  std::unordered_map<SessionId, std::map<PresentId, zx::time>> latched_times;
  while (!latched_updates_.empty() && latched_updates_.front().frame_number <= frame_number) {
    const FrameUpdate& latched_update = latched_updates_.front();

    // Micro-optimize tracing.
    if (TRACE_CATEGORY_ENABLED("gfx")) {
      // The straightforward approach would be to use TRACE_INSTAFLOW_STEP for each session-present,
      // but there is a non-negligible cost if there are multiple presents.
      TRACE_DURATION("gfx", "scenic_session_present/frame_presented", "frame_number",
                     TA_UINT64(frame_number), "latched_time",
                     TA_INT64(latched_update.latched_time.get()), "presentation_time",
                     TA_INT64(actual_presentation_time.get()));

      for (auto [session_id, present_id] : latched_update.updated_sessions) {
        TRACE_FLOW_STEP("gfx", "scenic_session_present/frame_presented",
                        SESSION_TRACE_ID(session_id, present_id));
        last_updates[session_id] = present_id;
      }
    } else {
      for (auto [session_id, present_id] : latched_update.updated_sessions) {
        last_updates[session_id] = present_id;
      }
    }

    latched_updates_.pop();
  }

  for (const auto& [session_id, present_id] : last_updates) {
    latched_times[session_id] =
        ExtractLatchTimestampsUpTo({.session_id = session_id, .present_id = present_id});
  }

  on_frame_presented_(latched_times, PresentTimestamps{
                                         .presented_time = zx::time(actual_presentation_time),
                                         .vsync_interval = zx::duration(presentation_interval),
                                     });
}

std::map<PresentId, zx::time> DefaultFrameScheduler::ExtractLatchTimestampsUpTo(
    SchedulingIdPair id_pair) {
  std::map<PresentId, zx::time> timestamps;

  auto begin_it = presents_.lower_bound({id_pair.session_id, 0});
  auto end_it = presents_.upper_bound(id_pair);
  FX_DCHECK(std::distance(begin_it, end_it) >= 0);
  std::for_each(begin_it, end_it,
                [&timestamps](std::pair<const SchedulingIdPair, std::optional<zx::time>>& pair) {
                  FX_DCHECK(pair.second.has_value());
                  timestamps[pair.first.present_id] = pair.second.value();
                });
  presents_.erase(begin_it, end_it);

  return timestamps;
}

void DefaultFrameScheduler::LogPeriodicDebugInfo() {
  FX_LOGS(INFO) << "DefaultFrameScheduler::LogPeriodicDebugInfo()"
                << "\n\t frame number: " << frame_number_
                << "\n\t current time: " << async_now(dispatcher_)
                << "\n\t last successful update start time: "
                << last_successful_update_start_time_.get()
                << "\n\t last successful target presentation time: "
                << last_successful_target_presentation_time_.get();
}

}  // namespace scheduling

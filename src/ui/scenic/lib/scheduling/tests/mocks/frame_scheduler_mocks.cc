// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/scheduling/tests/mocks/frame_scheduler_mocks.h"

#include <lib/async/cpp/time.h>
#include <lib/async/default.h>

namespace scheduling::test {

void MockFrameScheduler::SetRenderContinuously(bool render_continuously) {
  if (set_render_continuously_callback_) {
    set_render_continuously_callback_(render_continuously);
  }
}

void MockFrameScheduler::ScheduleUpdateForSession(zx::time presentation_time,
                                                  SchedulingIdPair id_pair, bool squashable,
                                                  bool schedule_asap) {
  if (schedule_update_for_session_callback_) {
    schedule_update_for_session_callback_(presentation_time, id_pair, squashable, schedule_asap);
  }
}

std::vector<scheduling::FuturePresentationInfo> MockFrameScheduler::GetFuturePresentationInfos(
    zx::duration requested_prediction_span) {
  if (get_future_presentation_infos_callback_) {
    return get_future_presentation_infos_callback_(requested_prediction_span);
  }
  return {};
}

void MockFrameScheduler::RemoveSession(SessionId session_id) {
  if (remove_session_callback_) {
    remove_session_callback_(session_id);
  }
}

}  // namespace scheduling::test

// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-testing/test_loop.h>
#include <lib/async/default.h>

#include <fuzzer/FuzzedDataProvider.h>
#include <gtest/gtest.h>

#include "src/ui/scenic/lib/scheduling/constant_frame_predictor.h"
#include "src/ui/scenic/lib/scheduling/default_frame_scheduler.h"

namespace scheduling::test {

extern "C" int LLVMFuzzerTestOneInput(const uint8_t* Data, size_t Size) {
  // Fuzz FrameScheduler against fuzz timing input. The expectation is that all
  // tasks can be posted to the loop and run, and the FrameScheduler will not crash.
  FuzzedDataProvider fuzzed_data(Data, Size);

  async::TestLoop test_loop;
  async_set_default_dispatcher(test_loop.dispatcher());
  // Sanity check the default dispatcher has been set.
  EXPECT_EQ(async_get_default_dispatcher(), test_loop.dispatcher());

  // Fuzz vsync and prediction times.
  zx::time last_vsync_time = zx::time(fuzzed_data.ConsumeIntegral<zx_time_t>());
  zx::duration vsync_interval = zx::msec(fuzzed_data.ConsumeIntegral<zx_time_t>());

  // Negative values here indicates programming or driver bug and are not interesting to fuzz.
  if (last_vsync_time.get() < 0 || vsync_interval.get() <= 0)
    return 0;

  zx::duration constant_prediction_offset = zx::msec(fuzzed_data.ConsumeIntegral<zx_time_t>());
  zx::time schedule_present_time = zx::time(fuzzed_data.ConsumeIntegral<zx_time_t>());

  // Set up DefaultFrameScheduler.
  auto vsync_timing = std::make_shared<VsyncTiming>();
  vsync_timing->AddVsyncInterval(vsync_interval);
  vsync_timing->set_last_vsync_time(last_vsync_time);

  DefaultFrameScheduler frame_scheduler(
      std::make_unique<ConstantFramePredictor>(constant_prediction_offset));

  uint32_t update_sessions_count = 0;
  uint32_t on_cpu_work_done_count = 0;
  uint32_t on_frame_presented_count = 0;
  uint32_t render_scheduled_frame_count = 0;
  frame_scheduler.Initialize(
      vsync_timing,
      /*update_sessions*/
      [&update_sessions_count](auto& sessions_to_update, auto trace_id) {
        update_sessions_count++;
      },
      /*on_cpu_work_done*/
      [&on_cpu_work_done_count] { on_cpu_work_done_count++; },
      /*on_frame_presented*/
      [&on_frame_presented_count](auto latched_times, auto present_times) {
        on_frame_presented_count++;
      },
      /*render_scheduled_frame*/
      [&render_scheduled_frame_count](auto frame_number, auto presentation_time, auto callback) {
        render_scheduled_frame_count++;
      });

  const bool squashable = fuzzed_data.ConsumeIntegral<bool>();
  const bool schedule_asap = fuzzed_data.ConsumeIntegral<bool>();

  const SessionId client_id = 5;
  frame_scheduler.ScheduleUpdateForSession(schedule_present_time, {client_id, 1}, squashable,
                                           schedule_asap);

  test_loop.RunUntilIdle();

  EXPECT_EQ(update_sessions_count, 1u);
  EXPECT_EQ(on_cpu_work_done_count, 1u);
  EXPECT_EQ(on_frame_presented_count, 1u);
  EXPECT_EQ(render_scheduled_frame_count, 1u);
  return 0;
}

}  // namespace scheduling::test

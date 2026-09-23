// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/flatland/engine/release_fence_manager.h"

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/ui/scenic/lib/scheduling/frame_scheduler.h"
#include "src/ui/scenic/lib/utils/helpers.h"
#include "zircon/system/public/zircon/syscalls.h"

// TEST COVERAGE NOTES
//
// There are quite a few cases to test here, and it is difficult to get an idea of the coverage
// by reading the code.  This is an overview of the cases that are tested below.
//
// 0) It's not useful for the client to pass release fences with frame #1, but we don't disallow it.
//    Since there is no previous frame, these fences are signaled immediately.
//
//    Tests:
//    - FirstFrameSignalsImmediately
//
// 1) Verify that the moment that release fence are signaled depends on whether the *previous* frame
//    is GPU-composited or direct-scanout.  See "Design Requirements" in the ReleaseFenceManager
//    class comment.
//
//    Tests:
//    - SignalingWhenPreviousFrameWasGpuComposited
//    - SignalingWhenPreviousFrameWasDirectScanout
//
// 2) Dropped/Skipped frames.  OnVsync() for later frame causes frame callback of earlier frames to
//    be invoked (assuming that all render_finished_fences are signaled for earlier GPU-composited
//    frames).
//
//    Tests:
//    - OutOfOrderRenderFinished
//
// 3) FrameRecords are removed ASAP, as soon as the frame callback has been invoked and there is at
//    least one subsequent frame registered.
//
//    Tests:
//    - ImmediateErasure
//
// 4) Repeated OnVsync() calls with the same frame number are OK.  This is an expected use case:
//    this is what will be received from the display controller and someone needs to handle it, so
//    might as well be ReleaseFenceManager.
//
//    Tests:
//    - RepeatedOnVsyncFrameNumbers
//
// 5) Edge-case where OnVsync() is received before |render_finished_fence| is signaled (or at least
//    before the signal is handled).
//
//    Tests:
//    - FramePresentedCallbackForGpuCompositedFrame
//
// 6) Properly-set timestamps in frame-presented callback.
//
//    Tests:
//    - OutOfOrderRenderFinished
//    - FramePresentedCallbackForGpuCompositedFrame
//    - FramePresentedCallbackForDirectScanoutFrame

namespace flatland::test {

namespace {

class ReleaseFenceManagerTest : public gtest::TestLoopFixture {};

}  // namespace

TEST_F(ReleaseFenceManagerTest, FirstFrameSignalsImmediately) {
  // Test when first frame is GPU-composited.
  {
    ReleaseFenceManager manager(dispatcher());
    std::vector<zx::event> release_fences = utils::CreateEventArray(2);
    std::vector<zx::counter> release_counters = utils::CreateCounterArray(2);
    zx::event render_finished_fence = utils::CreateEvent();

    bool callback_invoked = false;
    manager.OnGpuCompositedFrame(
        /*frame_number*/ 1, utils::CopyZxHandle(render_finished_fence),
        utils::CopyZxHandleVector(release_fences), utils::CopyZxHandleVector(release_counters), {},
        [&callback_invoked](scheduling::Timestamps) { callback_invoked = true; });

    for (auto& fence : release_fences) {
      EXPECT_TRUE(utils::IsEventSignalled(fence, ZX_EVENT_SIGNALED));
    }
    for (auto& counter : release_counters) {
      EXPECT_TRUE(utils::IsCounterSignalled(counter, ZX_COUNTER_SIGNALED));
      EXPECT_EQ(utils::ReadCounter(counter), Now().get());
    }
    EXPECT_FALSE(utils::IsEventSignalled(render_finished_fence, ZX_EVENT_SIGNALED));
    EXPECT_FALSE(callback_invoked);
  }

  // Same thing, except with a direct-scanout frame.
  {
    ReleaseFenceManager manager(dispatcher());
    std::vector<zx::event> release_fences = utils::CreateEventArray(2);
    std::vector<zx::counter> release_counters = utils::CreateCounterArray(2);

    bool callback_invoked = false;
    manager.OnDirectScanoutFrame(
        /*frame_number*/ 1, utils::CopyZxHandleVector(release_fences),
        utils::CopyZxHandleVector(release_counters), {},
        [&callback_invoked](scheduling::Timestamps) { callback_invoked = true; });

    for (auto& fence : release_fences) {
      EXPECT_TRUE(utils::IsEventSignalled(fence, ZX_EVENT_SIGNALED));
    }
    for (auto& counter : release_counters) {
      EXPECT_TRUE(utils::IsCounterSignalled(counter, ZX_COUNTER_SIGNALED));
      EXPECT_EQ(utils::ReadCounter(counter), Now().get());
    }
    EXPECT_FALSE(callback_invoked);
  }
}

TEST_F(ReleaseFenceManagerTest, SignalingWhenPreviousFrameWasGpuComposited) {
  // For the purposes of this test, it doesn't matter whether the second frame is GPU-composited or
  // direct-scanout.  Test both variants.
  for (auto& second_frame_is_gpu_composited : std::array<bool, 2>{true, false}) {
    ReleaseFenceManager manager(dispatcher());

    zx::event render_finished_fence = utils::CreateEvent();
    manager.OnGpuCompositedFrame(
        /*frame_number*/ 1, utils::CopyZxHandle(render_finished_fence), {}, {}, {},
        [](scheduling::Timestamps) {});

    // These fences will be passed along with the second frame, and signaled when the first frame is
    // finished rendering.
    std::vector<zx::event> release_fences = utils::CreateEventArray(2);
    std::vector<zx::counter> release_counters = utils::CreateCounterArray(2);

    if (second_frame_is_gpu_composited) {
      manager.OnGpuCompositedFrame(
          /*frame_number*/ 2, utils::CreateEvent(), utils::CopyZxHandleVector(release_fences),
          utils::CopyZxHandleVector(release_counters), {}, [](scheduling::Timestamps) {});
    } else {
      manager.OnDirectScanoutFrame(
          /*frame_number*/ 2, utils::CopyZxHandleVector(release_fences),
          utils::CopyZxHandleVector(release_counters), {}, [](scheduling::Timestamps) {});
    }

    // The fences provided with the second frame are not signaled until the first frame
    // is finished rendering.
    for (auto& fence : release_fences) {
      EXPECT_FALSE(utils::IsEventSignalled(fence, ZX_EVENT_SIGNALED));
    }
    for (auto& counter : release_counters) {
      EXPECT_FALSE(utils::IsCounterSignalled(counter, ZX_COUNTER_SIGNALED));
    }

    const zx::time_monotonic kRenderFinishedLowerBoundTime(zx_clock_get_monotonic());
    render_finished_fence.signal(0u, ZX_EVENT_SIGNALED);
    RunLoopUntilIdle();
    const zx::time_monotonic kRenderFinishedUpperBoundTime(zx_clock_get_monotonic());

    for (auto& fence : release_fences) {
      EXPECT_TRUE(utils::IsEventSignalled(fence, ZX_EVENT_SIGNALED));
    }
    for (auto& counter : release_counters) {
      EXPECT_TRUE(utils::IsCounterSignalled(counter, ZX_COUNTER_SIGNALED));
      EXPECT_GE(utils::ReadCounter(counter), kRenderFinishedLowerBoundTime.get());
      EXPECT_LE(utils::ReadCounter(counter), kRenderFinishedUpperBoundTime.get());
    }
  }
}

TEST_F(ReleaseFenceManagerTest, SignalingWhenPreviousFrameWasDirectScanout) {
  // For the purposes of this test, it doesn't matter whether the second frame is GPU-composited or
  // direct-scanout.  Test both variants.
  for (auto& second_frame_is_gpu_composited : std::array<bool, 2>{true, false}) {
    ReleaseFenceManager manager(dispatcher());

    manager.OnDirectScanoutFrame(
        /*frame_number*/ 1, {}, {}, {}, [](scheduling::Timestamps) {});

    // These fences will be passed along with the second frame, and signaled when the second frame
    // is displayed on screen (as evidenced by receiving an OnVsync()).
    std::vector<zx::event> release_fences = utils::CreateEventArray(2);
    std::vector<zx::counter> release_counters = utils::CreateCounterArray(2);

    if (second_frame_is_gpu_composited) {
      zx::event render_finished_fence = utils::CreateEvent();
      manager.OnGpuCompositedFrame(/*frame_number*/ 2, utils::CopyZxHandle(render_finished_fence),
                                   utils::CopyZxHandleVector(release_fences),
                                   utils::CopyZxHandleVector(release_counters), {},
                                   [](scheduling::Timestamps) {});

      // Finishing rendering doesn't signal the release fences, because the frame has not been
      // displayed yet.
      render_finished_fence.signal(0u, ZX_EVENT_SIGNALED);
      RunLoopUntilIdle();
      for (auto& fence : release_fences) {
        EXPECT_FALSE(utils::IsEventSignalled(fence, ZX_EVENT_SIGNALED));
      }
      for (auto& counter : release_counters) {
        EXPECT_FALSE(utils::IsCounterSignalled(counter, ZX_COUNTER_SIGNALED));
      }
    } else {
      manager.OnDirectScanoutFrame(
          /*frame_number*/ 2, utils::CopyZxHandleVector(release_fences),
          utils::CopyZxHandleVector(release_counters), {}, [](scheduling::Timestamps) {});
    }

    // The fences are signaled when the second frame is displayed, not the first.
    manager.OnVsync(/*frame_number*/ 1, zx::time_monotonic(1));
    for (auto& fence : release_fences) {
      EXPECT_FALSE(utils::IsEventSignalled(fence, ZX_EVENT_SIGNALED));
    }
    for (auto& counter : release_counters) {
      EXPECT_FALSE(utils::IsCounterSignalled(counter, ZX_COUNTER_SIGNALED));
    }

    const zx::time_monotonic kVsyncTime(10);
    manager.OnVsync(/*frame_number*/ 2, kVsyncTime);
    for (auto& fence : release_fences) {
      EXPECT_TRUE(utils::IsEventSignalled(fence, ZX_EVENT_SIGNALED));
    }
    for (auto& counter : release_counters) {
      EXPECT_TRUE(utils::IsCounterSignalled(counter, ZX_COUNTER_SIGNALED));
      EXPECT_EQ(utils::ReadCounter(counter), kVsyncTime.get());
    }
  }
}

TEST_F(ReleaseFenceManagerTest, FramePresentedCallbackForGpuCompositedFrame) {
  // Test common case, where render_finished_fence is signaled before the OnVsync() is received.
  {
    ReleaseFenceManager manager(dispatcher());
    zx::event render_finished_fence = utils::CreateEvent();

    bool callback_invoked = false;
    scheduling::Timestamps callback_timestamps;
    manager.OnGpuCompositedFrame(
        /*frame_number*/ 1, utils::CopyZxHandle(render_finished_fence), {}, {}, {},
        [&](scheduling::Timestamps timestamps) {
          callback_invoked = true;
          callback_timestamps = timestamps;
        });

    const zx::time_monotonic kRenderFinishedLowerBoundTime(zx_clock_get_monotonic());
    render_finished_fence.signal(0u, ZX_EVENT_SIGNALED);
    RunLoopUntilIdle();
    EXPECT_FALSE(callback_invoked);

    const zx::time_monotonic kVsyncTime(zx_clock_get_monotonic());
    manager.OnVsync(/*frame_number*/ 1, kVsyncTime);
    EXPECT_TRUE(callback_invoked);
    EXPECT_GE(callback_timestamps.render_done_time, kRenderFinishedLowerBoundTime);
    EXPECT_LE(callback_timestamps.render_done_time, kVsyncTime);
    EXPECT_EQ(callback_timestamps.actual_presentation_time, kVsyncTime);
  }

  // Test rare edge case, where render_finished_fence is signaled before the OnVsync() is received,
  // but we don't process is until afterward (unclear whether this will ever happen in practice).
  {
    ReleaseFenceManager manager(dispatcher());
    zx::event render_finished_fence = utils::CreateEvent();

    bool callback_invoked = false;
    scheduling::Timestamps callback_timestamps;
    manager.OnGpuCompositedFrame(
        /*frame_number*/ 1, utils::CopyZxHandle(render_finished_fence), {}, {}, {},
        [&](scheduling::Timestamps timestamps) {
          callback_invoked = true;
          callback_timestamps = timestamps;
        });

    const zx::time_monotonic kRenderFinishedLowerBoundTime(zx_clock_get_monotonic());
    render_finished_fence.signal(0u, ZX_EVENT_SIGNALED);

    const zx::time_monotonic kVsyncTime(zx_clock_get_monotonic());
    manager.OnVsync(/*frame_number*/ 1, kVsyncTime);
    EXPECT_FALSE(callback_invoked);

    // This is where we process the event's signal.
    RunLoopUntilIdle();
    EXPECT_TRUE(callback_invoked);
    EXPECT_GE(callback_timestamps.render_done_time, kRenderFinishedLowerBoundTime);
    EXPECT_LE(callback_timestamps.render_done_time, kVsyncTime);
    EXPECT_EQ(callback_timestamps.actual_presentation_time, kVsyncTime);
  }
}

TEST_F(ReleaseFenceManagerTest, FramePresentedCallbackForDirectScanoutFrame) {
  ReleaseFenceManager manager(dispatcher());

  const zx::time_monotonic kFrameStartTime(10'000'000);
  const zx::time_monotonic kVsyncTime(12'000'000);
  RunLoopUntil(kFrameStartTime);

  bool callback_invoked = false;
  scheduling::Timestamps callback_timestamps;
  manager.OnDirectScanoutFrame(
      /*frame_number*/ 1, {}, {}, {}, [&](scheduling::Timestamps timestamps) {
        callback_invoked = true;
        callback_timestamps = timestamps;
      });

  manager.OnVsync(/*frame_number*/ 1, kVsyncTime);
  EXPECT_TRUE(callback_invoked);
  // TODO(https://fxbug.dev/42154139): what should the render_done_time be?
  EXPECT_EQ(callback_timestamps.render_done_time, kFrameStartTime);
  EXPECT_EQ(callback_timestamps.actual_presentation_time, kVsyncTime);
}

TEST_F(ReleaseFenceManagerTest, OutOfOrderRenderFinished) {
  ReleaseFenceManager manager(dispatcher());

  bool callback_invoked1 = false;
  bool callback_invoked2 = false;
  bool callback_invoked3 = false;
  bool callback_invoked4 = false;
  scheduling::Timestamps callback_timestamps1;
  scheduling::Timestamps callback_timestamps2;
  scheduling::Timestamps callback_timestamps3;
  scheduling::Timestamps callback_timestamps4;
  zx::event render_finished_fence2 = utils::CreateEvent();
  zx::event render_finished_fence4 = utils::CreateEvent();

  manager.OnDirectScanoutFrame(
      /*frame_number*/ 1, {}, {}, {}, [&](scheduling::Timestamps timestamps) {
        callback_invoked1 = true;
        callback_timestamps1 = timestamps;
        EXPECT_FALSE(callback_invoked2);
        EXPECT_FALSE(callback_invoked3);
        EXPECT_FALSE(callback_invoked4);
      });
  EXPECT_EQ(manager.frame_record_count(), 1u);

  manager.OnGpuCompositedFrame(
      /*frame_number*/ 2, utils::CopyZxHandle(render_finished_fence2), {}, {}, {},
      [&](scheduling::Timestamps timestamps) {
        callback_invoked2 = true;
        callback_timestamps2 = timestamps;
        EXPECT_TRUE(callback_invoked1);
        EXPECT_FALSE(callback_invoked3);
        EXPECT_FALSE(callback_invoked4);
      });
  EXPECT_EQ(manager.frame_record_count(), 2u);

  manager.OnDirectScanoutFrame(
      /*frame_number*/ 3, {}, {}, {}, [&](scheduling::Timestamps timestamps) {
        callback_invoked3 = true;
        callback_timestamps3 = timestamps;
        EXPECT_TRUE(callback_invoked1);
        EXPECT_TRUE(callback_invoked2);
        EXPECT_FALSE(callback_invoked4);
      });
  EXPECT_EQ(manager.frame_record_count(), 3u);

  manager.OnGpuCompositedFrame(
      /*frame_number*/ 4, utils::CopyZxHandle(render_finished_fence4), {}, {}, {},
      [&](scheduling::Timestamps timestamps) {
        callback_invoked4 = true;
        callback_timestamps4 = timestamps;
        EXPECT_TRUE(callback_invoked1);
        EXPECT_TRUE(callback_invoked2);
        EXPECT_TRUE(callback_invoked3);
      });
  EXPECT_EQ(manager.frame_record_count(), 4u);

  EXPECT_FALSE(callback_invoked1);
  EXPECT_FALSE(callback_invoked2);
  EXPECT_FALSE(callback_invoked3);
  EXPECT_FALSE(callback_invoked4);

  // In this scenario, for some reason frame 4's rendering completes before frame 2's.  Although
  // this is unlikely, it's good to have this edge case covered in a reasonable way.  A more likely
  // scenario is that a direct-scanout frame (such as frame 3) is presented before the previous
  // GPU-composited frame is finished rendering; this scenario is also covered here.

  const zx::time_monotonic kRenderFinishedLowerBoundTime4(zx_clock_get_monotonic());
  render_finished_fence4.signal(0u, ZX_EVENT_SIGNALED);
  RunLoopUntilIdle();
  EXPECT_FALSE(callback_invoked4);
  const zx::time_monotonic kVsyncTime(zx_clock_get_monotonic());
  manager.OnVsync(/*frame_number*/ 4, kVsyncTime);

  // Even though frame 4 has been presented, we can only invoke the first callback.  This is because
  // of scheduling::FrameRenderer's requirement that: "Frames must be rendered in the order they are
  // requested, and callbacks must be triggered in the same order."
  EXPECT_TRUE(callback_invoked1);
  EXPECT_FALSE(callback_invoked2);
  EXPECT_FALSE(callback_invoked3);
  EXPECT_FALSE(callback_invoked4);
  EXPECT_EQ(callback_timestamps1.actual_presentation_time, kVsyncTime);
  EXPECT_EQ(manager.frame_record_count(), 3u);

  // Once frame 2's render-finished fence has been signaled, this "unlocks" the rest of the frames.
  const zx::time_monotonic kRenderFinishedLowerBoundTime2(zx_clock_get_monotonic());
  render_finished_fence2.signal(0u, ZX_EVENT_SIGNALED);
  RunLoopUntilIdle();

  EXPECT_TRUE(callback_invoked2);
  EXPECT_TRUE(callback_invoked3);
  EXPECT_TRUE(callback_invoked4);
  EXPECT_EQ(callback_timestamps2.actual_presentation_time, kVsyncTime);
  EXPECT_EQ(callback_timestamps3.actual_presentation_time, kVsyncTime);
  EXPECT_EQ(callback_timestamps4.actual_presentation_time, kVsyncTime);

  // Even though all frame callbacks have been invoked, the frame record for the last frame is kept
  // around, because its type (GPU-composited vs. direct-scanout) affects how the *next* frame's
  // release fences are handled.
  EXPECT_EQ(manager.frame_record_count(), 1u);

  // Adding an additional frame results in the old frame-record being erased, and a new one added.
  manager.OnDirectScanoutFrame(
      /*frame_number*/ 5, {}, {}, {}, [&](scheduling::Timestamps) {});
  EXPECT_EQ(manager.frame_record_count(), 1u);
}

TEST_F(ReleaseFenceManagerTest, ImmediateErasure) {
  // Frame is erased immediately when a subsequent frame is added, after the first frame already has
  // its callback invoked (we don't test the callback explicitly here; this is done in other tests).
  {
    ReleaseFenceManager manager(dispatcher());

    // First frame can't be erased even after presented.
    manager.OnDirectScanoutFrame(/*frame_number*/ 1, {}, {}, {}, [](scheduling::Timestamps) {});
    manager.OnVsync(/*frame_number*/ 1, zx::time_monotonic(100));
    EXPECT_EQ(manager.frame_record_count(), 1u);

    // Adding the next frame causes the first to be erased.
    zx::event render_finished_fence = utils::CreateEvent();
    manager.OnGpuCompositedFrame(
        /*frame_number*/ 2, utils::CopyZxHandle(render_finished_fence), {}, {}, {},
        [](scheduling::Timestamps) {});
    EXPECT_EQ(manager.frame_record_count(), 1u);

    // Second frame can't be erased even after render-finished and presented.
    render_finished_fence.signal(0u, ZX_EVENT_SIGNALED);
    RunLoopUntilIdle();
    manager.OnVsync(/*frame_number*/ 2, zx::time_monotonic(200));
    EXPECT_EQ(manager.frame_record_count(), 1u);

    // Adding the next frame causes the second to be erased.
    manager.OnDirectScanoutFrame(/*frame_number*/ 3, {}, {}, {}, [](scheduling::Timestamps) {});
    EXPECT_EQ(manager.frame_record_count(), 1u);
  }

  // GPU-composited frame is erased immediately when there is already a subsequent frame, rendering
  // has finished, and it has been presented (the last 2 in either order).
  {
    ReleaseFenceManager manager(dispatcher());
    zx::event render_finished_fence1 = utils::CreateEvent();
    zx::event render_finished_fence2 = utils::CreateEvent();

    manager.OnGpuCompositedFrame(
        /*frame_number*/ 1, utils::CopyZxHandle(render_finished_fence1), {}, {}, {},
        [](scheduling::Timestamps) {});

    manager.OnGpuCompositedFrame(
        /*frame_number*/ 2, utils::CopyZxHandle(render_finished_fence2), {}, {}, {},
        [](scheduling::Timestamps) {});

    // First frame has fence signaled before OnVsync().  The other way works too, as we see below.
    render_finished_fence1.signal(0u, ZX_EVENT_SIGNALED);
    RunLoopUntilIdle();
    EXPECT_EQ(manager.frame_record_count(), 2u);
    manager.OnVsync(/*frame_number*/ 1, zx::time_monotonic(100));
    EXPECT_EQ(manager.frame_record_count(), 1u);

    // Add a third frame, so the second can be erased immediately after its callback is invoked.
    manager.OnDirectScanoutFrame(/*frame_number*/ 3, {}, {}, {}, [](scheduling::Timestamps) {});

    // Second frame has OnVsync() before fence signal is received.
    render_finished_fence2.signal(0u, ZX_EVENT_SIGNALED);
    manager.OnVsync(/*frame_number*/ 2, zx::time_monotonic(200));
    EXPECT_EQ(manager.frame_record_count(), 2u);
    RunLoopUntilIdle();  // handle the signaling of |render_finished_fence2|
    EXPECT_EQ(manager.frame_record_count(), 1u);
  }

  // Direct-scanout frame is erased immediately when there is already a subsequent frame, as soon as
  // its callback is invoked.
  {
    ReleaseFenceManager manager(dispatcher());

    manager.OnDirectScanoutFrame(/*frame_number*/ 1, {}, {}, {}, [](scheduling::Timestamps) {});
    manager.OnDirectScanoutFrame(/*frame_number*/ 2, {}, {}, {}, [](scheduling::Timestamps) {});

    EXPECT_EQ(manager.frame_record_count(), 2u);
    manager.OnVsync(/*frame_number*/ 1, zx::time_monotonic(100));
    EXPECT_EQ(manager.frame_record_count(), 1u);
  }
}

TEST_F(ReleaseFenceManagerTest, RepeatedOnVsyncFrameNumbers) {
  ReleaseFenceManager manager(dispatcher());

  uint64_t callback_count1 = 0;
  manager.OnDirectScanoutFrame(/*frame_number*/ 1, {}, {}, {},
                               [&](scheduling::Timestamps) { ++callback_count1; });

  manager.OnVsync(/*frame_number*/ 1, zx::time_monotonic(100));
  EXPECT_EQ(callback_count1, 1u);
  manager.OnVsync(/*frame_number*/ 1, zx::time_monotonic(200));
  manager.OnVsync(/*frame_number*/ 1, zx::time_monotonic(300));
  manager.OnVsync(/*frame_number*/ 1, zx::time_monotonic(400));
  manager.OnVsync(/*frame_number*/ 1, zx::time_monotonic(500));
  EXPECT_EQ(callback_count1, 1u);

  // Register another frame, but have more Vsyncs for the first frame arrive before the second is
  // presented.
  uint64_t callback_count2 = 0;
  manager.OnDirectScanoutFrame(/*frame_number*/ 2, {}, {}, {},
                               [&](scheduling::Timestamps) { ++callback_count2; });

  manager.OnVsync(/*frame_number*/ 1, zx::time_monotonic(600));
  EXPECT_EQ(callback_count1, 1u);
  EXPECT_EQ(callback_count2, 0u);

  manager.OnVsync(/*frame_number*/ 2, zx::time_monotonic(700));
  EXPECT_EQ(callback_count1, 1u);
  EXPECT_EQ(callback_count2, 1u);
}

TEST_F(ReleaseFenceManagerTest, SignalPresentFencesForGpuCompositedFrame) {
  ReleaseFenceManager manager(dispatcher());
  zx::event render_finished_fence = utils::CreateEvent();
  std::vector<zx::counter> present_fences = utils::CreateCounterArray(2);

  bool callback_invoked = false;
  manager.OnGpuCompositedFrame(/*frame_number*/ 1, utils::CopyZxHandle(render_finished_fence), {},
                               {}, utils::CopyZxHandleVector(present_fences),
                               [&](scheduling::Timestamps) { callback_invoked = true; });

  // Not signaled yet.
  for (auto& c : present_fences) {
    EXPECT_FALSE(utils::IsCounterSignalled(c, ZX_COUNTER_SIGNALED));
  }

  // Render finishes. Still not signaled.
  render_finished_fence.signal(0u, ZX_EVENT_SIGNALED);
  RunLoopUntilIdle();
  for (auto& c : present_fences) {
    EXPECT_FALSE(utils::IsCounterSignalled(c, ZX_COUNTER_SIGNALED));
  }

  // Vsync occurs. Now signaled with the vsync timestamp.
  const zx::time_monotonic kVsyncTime(123456789);
  manager.OnVsync(/*frame_number*/ 1, kVsyncTime);
  EXPECT_TRUE(callback_invoked);
  for (auto& c : present_fences) {
    EXPECT_TRUE(utils::IsCounterSignalled(c, ZX_COUNTER_SIGNALED));
    EXPECT_EQ(utils::ReadCounter(c), kVsyncTime.get());
  }
}

TEST_F(ReleaseFenceManagerTest, SignalPresentFencesForDirectScanoutFrame) {
  ReleaseFenceManager manager(dispatcher());
  std::vector<zx::counter> present_fences = utils::CreateCounterArray(2);

  bool callback_invoked = false;
  manager.OnDirectScanoutFrame(/*frame_number*/ 1, {}, {},
                               utils::CopyZxHandleVector(present_fences),
                               [&](scheduling::Timestamps timestamps) { callback_invoked = true; });

  // Not signaled yet.
  for (auto& c : present_fences) {
    EXPECT_FALSE(utils::IsCounterSignalled(c, ZX_COUNTER_SIGNALED));
  }

  // Vsync occurs. Now signaled with the vsync timestamp.
  const zx::time_monotonic kVsyncTime(987654321);
  manager.OnVsync(/*frame_number*/ 1, kVsyncTime);
  EXPECT_TRUE(callback_invoked);
  for (auto& c : present_fences) {
    EXPECT_TRUE(utils::IsCounterSignalled(c, ZX_COUNTER_SIGNALED));
    EXPECT_EQ(utils::ReadCounter(c), kVsyncTime.get());
  }
}

TEST_F(ReleaseFenceManagerTest, SignalPresentFencesForSkippedFrames) {
  ReleaseFenceManager manager(dispatcher());

  std::vector<zx::counter> present_fences1 = utils::CreateCounterArray(1);
  std::vector<zx::counter> present_fences2 = utils::CreateCounterArray(1);

  bool callback_invoked1 = false;
  bool callback_invoked2 = false;

  manager.OnDirectScanoutFrame(
      /*frame_number*/ 1, {}, {}, utils::CopyZxHandleVector(present_fences1),
      [&](scheduling::Timestamps timestamps) { callback_invoked1 = true; });
  manager.OnDirectScanoutFrame(/*frame_number*/ 2, {}, {},
                               utils::CopyZxHandleVector(present_fences2),
                               [&](scheduling::Timestamps) { callback_invoked2 = true; });

  // Vsync for frame 2 arrives. This skips frame 1.
  const zx::time_monotonic kVsyncTime(1000);
  manager.OnVsync(/*frame_number*/ 2, kVsyncTime);

  // Both should be signaled with the vsync time.
  EXPECT_TRUE(callback_invoked1);
  EXPECT_TRUE(callback_invoked2);

  EXPECT_TRUE(utils::IsCounterSignalled(present_fences1[0], ZX_COUNTER_SIGNALED));
  EXPECT_EQ(utils::ReadCounter(present_fences1[0]), kVsyncTime.get());

  EXPECT_TRUE(utils::IsCounterSignalled(present_fences2[0], ZX_COUNTER_SIGNALED));
  EXPECT_EQ(utils::ReadCounter(present_fences2[0]), kVsyncTime.get());
}

TEST_F(ReleaseFenceManagerTest, SignalPresentFencesStrictOrderingWhenGpuFinishesLate) {
  ReleaseFenceManager manager(dispatcher());

  zx::event render_finished_fence1 = utils::CreateEvent();
  std::vector<zx::counter> present_fences1 = utils::CreateCounterArray(1);
  std::vector<zx::counter> present_fences2 = utils::CreateCounterArray(1);

  bool callback_invoked1 = false;
  bool callback_invoked2 = false;

  manager.OnGpuCompositedFrame(/*frame_number*/ 1, utils::CopyZxHandle(render_finished_fence1), {},
                               {}, utils::CopyZxHandleVector(present_fences1),
                               [&](scheduling::Timestamps) { callback_invoked1 = true; });
  manager.OnDirectScanoutFrame(/*frame_number*/ 2, {}, {},
                               utils::CopyZxHandleVector(present_fences2),
                               [&](scheduling::Timestamps) { callback_invoked2 = true; });

  // Vsync for frame 2 arrives.
  const zx::time_monotonic kVsyncTime(2000);
  manager.OnVsync(/*frame_number*/ 2, kVsyncTime);

  // Frame 2 is technically presented, but frame 1 hasn't finished rendering.
  // Neither callback should be invoked yet, and neither fence should be signaled.
  EXPECT_FALSE(callback_invoked1);
  EXPECT_FALSE(callback_invoked2);
  EXPECT_FALSE(utils::IsCounterSignalled(present_fences1[0], ZX_COUNTER_SIGNALED));
  EXPECT_FALSE(utils::IsCounterSignalled(present_fences2[0], ZX_COUNTER_SIGNALED));

  // Frame 1 finished rendering.
  render_finished_fence1.signal(0u, ZX_EVENT_SIGNALED);
  RunLoopUntilIdle();

  // Now both should be invoked and signaled.
  EXPECT_TRUE(callback_invoked1);
  EXPECT_TRUE(callback_invoked2);
  EXPECT_TRUE(utils::IsCounterSignalled(present_fences1[0], ZX_COUNTER_SIGNALED));
  EXPECT_EQ(utils::ReadCounter(present_fences1[0]), kVsyncTime.get());
  EXPECT_TRUE(utils::IsCounterSignalled(present_fences2[0], ZX_COUNTER_SIGNALED));
  EXPECT_EQ(utils::ReadCounter(present_fences2[0]), kVsyncTime.get());
}

// This test verifies that release fences for a new frame are signaled immediately if the
// previous GPU-composited frame has already finished rendering. This exercises a different
// state transition than `SignalPresentFencesStrictOrderingWhenGpuFinishesLate`, which focuses
// on `present_fences` when GPU rendering finishes *late*.
TEST_F(ReleaseFenceManagerTest, SignalReleaseFencesWhenPreviousFrameFinishedEarly) {
  ReleaseFenceManager manager(dispatcher());

  zx::event render_finished_fence1 = utils::CreateEvent();
  manager.OnGpuCompositedFrame(/*frame_number*/ 1, utils::CopyZxHandle(render_finished_fence1), {},
                               {}, {}, [](scheduling::Timestamps) {});

  const zx::time_monotonic kRenderFinishedLowerBoundTime(zx_clock_get_monotonic());
  // GPU finishes rendering immediately.
  render_finished_fence1.signal(0u, ZX_EVENT_SIGNALED);
  // Allow waiter to notice the signaling of |render_finished_fence1|
  RunLoopUntilIdle();
  const zx::time_monotonic kRenderFinishedUpperBoundTime(zx_clock_get_monotonic());

  // Subsequent frame's release fences are signaled immediately because the previous frame's
  // rendering is already finished.
  std::vector<zx::event> release_fences2 = utils::CreateEventArray(1);
  std::vector<zx::counter> release_counters2 = utils::CreateCounterArray(1);
  manager.OnDirectScanoutFrame(/*frame_number*/ 2, utils::CopyZxHandleVector(release_fences2),
                               utils::CopyZxHandleVector(release_counters2), {},
                               [](scheduling::Timestamps) {});

  // Should be signaled immediately.
  EXPECT_TRUE(utils::IsEventSignalled(release_fences2[0], ZX_EVENT_SIGNALED));
  EXPECT_TRUE(utils::IsCounterSignalled(release_counters2[0], ZX_COUNTER_SIGNALED));
  EXPECT_GE(utils::ReadCounter(release_counters2[0]), kRenderFinishedLowerBoundTime.get());
  EXPECT_LE(utils::ReadCounter(release_counters2[0]), kRenderFinishedUpperBoundTime.get());
}

TEST_F(ReleaseFenceManagerTest, ReleaseFenceManagerDestructionWithPendingWait) {
  zx::event render_finished_fence = utils::CreateEvent();
  {
    ReleaseFenceManager manager(dispatcher());
    manager.OnGpuCompositedFrame(/*frame_number*/ 1, utils::CopyZxHandle(render_finished_fence), {},
                                 {}, {}, [](scheduling::Timestamps) {});
    // Destruction here should cancel the WaitOnce.
  }
  // No crash is success.
}

TEST_F(ReleaseFenceManagerTest, SignalPresentFencesWithMultipleCounters) {
  ReleaseFenceManager manager(dispatcher());
  std::vector<zx::counter> present_fences = utils::CreateCounterArray(5);

  const zx::time_monotonic kVsyncTime(55555);
  manager.OnDirectScanoutFrame(/*frame_number*/ 1, {}, {},
                               utils::CopyZxHandleVector(present_fences),
                               [](scheduling::Timestamps) {});
  manager.OnVsync(/*frame_number*/ 1, kVsyncTime);

  for (auto& c : present_fences) {
    EXPECT_TRUE(utils::IsCounterSignalled(c, ZX_COUNTER_SIGNALED));
    EXPECT_EQ(utils::ReadCounter(c), kVsyncTime.get());
  }
}

TEST_F(ReleaseFenceManagerTest, ReleaseCountersOfSameFrameHaveIdenticalTimestamp) {
  for (bool previous_frame_is_gpu_composited : {true, false}) {
    ReleaseFenceManager manager(dispatcher());

    zx::event render_finished_fence = utils::CreateEvent();
    if (previous_frame_is_gpu_composited) {
      manager.OnGpuCompositedFrame(
          /*frame_number*/ 1, utils::CopyZxHandle(render_finished_fence), {}, {}, {},
          [](scheduling::Timestamps) {});
    } else {
      manager.OnDirectScanoutFrame(
          /*frame_number*/ 1, {}, {}, {}, [](scheduling::Timestamps) {});
    }

    std::vector<zx::counter> release_counters = utils::CreateCounterArray(2);
    manager.OnDirectScanoutFrame(
        /*frame_number*/ 2, {}, utils::CopyZxHandleVector(release_counters), {},
        [](scheduling::Timestamps) {});

    EXPECT_FALSE(utils::IsCounterSignalled(release_counters[0], ZX_COUNTER_SIGNALED));
    EXPECT_FALSE(utils::IsCounterSignalled(release_counters[1], ZX_COUNTER_SIGNALED));

    if (previous_frame_is_gpu_composited) {
      render_finished_fence.signal(0u, ZX_EVENT_SIGNALED);
      RunLoopUntilIdle();
    } else {
      const zx::time_monotonic kVsyncTime(55555);
      manager.OnVsync(/*frame_number*/ 2, kVsyncTime);
    }

    EXPECT_TRUE(utils::IsCounterSignalled(release_counters[0], ZX_COUNTER_SIGNALED));
    EXPECT_TRUE(utils::IsCounterSignalled(release_counters[1], ZX_COUNTER_SIGNALED));

    EXPECT_EQ(utils::ReadCounter(release_counters[0]), utils::ReadCounter(release_counters[1]));
  }
}

}  // namespace flatland::test

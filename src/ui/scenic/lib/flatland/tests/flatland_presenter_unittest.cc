// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-testing/test_loop.h>
#include <lib/async/default.h>
#include <lib/async/dispatcher.h>
#include <lib/fit/thread_checker.h>
#include <lib/syslog/cpp/macros.h>

#include <atomic>
#include <chrono>
#include <cstddef>
#include <mutex>
#include <thread>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/ui/scenic/lib/flatland/flatland_presenter_impl.h"
#include "src/ui/scenic/lib/flatland/tests/logging_event_loop.h"
#include "src/ui/scenic/lib/scheduling/tests/mocks/frame_scheduler_mocks.h"
#include "src/ui/scenic/lib/utils/helpers.h"

using flatland::FlatlandPresenterImpl;

namespace flatland::test {

namespace {

template <typename T>
std::vector<zx_koid_t> ExtractKoids(const std::vector<T>& objects) {
  std::vector<zx_koid_t> result;
  result.reserve(objects.size());
  for (const auto& obj : objects) {
    zx_info_handle_basic_t info;
    zx_status_t status = obj.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
    FX_DCHECK(status == ZX_OK);
    result.push_back(info.koid);
  }
  return result;
}

// This harness uses a real loop instead of a test loop since the multithreading test requires the
// tasks posted by the FlatlandPresenterImpl to run without blocking the worker threads.
class FlatlandPresenterTest : public LoggingEventLoop, public ::testing::Test {
 public:
  std::shared_ptr<FlatlandPresenterImpl> CreateFlatlandPresenterImpl(
      scheduling::FrameScheduler& scheduler) {
    return std::make_shared<FlatlandPresenterImpl>(dispatcher(), scheduler);
  }
};

}  // namespace

TEST_F(FlatlandPresenterTest, ScheduleUpdateForSessionForwardsToFrameScheduler) {
  scheduling::test::MockFrameScheduler frame_scheduler;

  // Capture the relevant arguments of the ScheduleUpdateForSession() call.
  zx::time last_presentation_time = zx::time(0);
  auto last_id_pair = scheduling::SchedulingIdPair({
      .session_id = scheduling::kInvalidSessionId,
      .present_id = scheduling::kInvalidPresentId,
  });
  bool last_squashable = false;
  bool last_schedule_asap = false;
  std::vector<zx::event> last_release_fences;
  std::vector<zx::counter> last_present_fences;

  frame_scheduler.set_schedule_update_for_session_callback(
      [&last_presentation_time, &last_id_pair, &last_squashable, &last_schedule_asap](
          zx::time presentation_time, scheduling::SchedulingIdPair id_pair, bool squashable,
          bool schedule_asap) {
        last_presentation_time = presentation_time;
        last_id_pair = id_pair;
        last_squashable = squashable;
        last_schedule_asap = schedule_asap;
      });

  auto presenter = CreateFlatlandPresenterImpl(frame_scheduler);

  const auto kIdPair = scheduling::SchedulingIdPair({
      .session_id = 1,
      .present_id = 2,
  });
  const zx::time kPresentationTime = zx::time(123);
  const bool kUnsquashable = false;
  bool kScheduleAsap = false;

  presenter->ScheduleUpdateForSession(kPresentationTime, kIdPair, kUnsquashable,
                                      /*release_fences=*/{}, /*release_counters=*/{},
                                      /*present_fences=*/{}, kScheduleAsap);
  RunLoopUntilIdle();

  EXPECT_EQ(last_presentation_time, kPresentationTime);
  EXPECT_EQ(last_id_pair, kIdPair);
  EXPECT_EQ(last_squashable, !kUnsquashable);
  EXPECT_EQ(last_schedule_asap, kScheduleAsap);

  // Send another update where schedule_asap is true.
  kScheduleAsap = true;
  const auto kIdPair2 = scheduling::SchedulingIdPair({
      .session_id = 1,
      .present_id = 3,
  });
  presenter->ScheduleUpdateForSession(kPresentationTime, kIdPair2, kUnsquashable,
                                      /*release_fences=*/{}, /*release_counters=*/{},
                                      /*present_fences=*/{},
                                      /*schedule_asap=*/kScheduleAsap);
  RunLoopUntilIdle();

  // zx::time is passed through.
  EXPECT_EQ(last_presentation_time, kPresentationTime);
  EXPECT_EQ(last_id_pair, kIdPair2);
  EXPECT_EQ(last_squashable, !kUnsquashable);
  EXPECT_EQ(last_schedule_asap, kScheduleAsap);
}

TEST_F(FlatlandPresenterTest, ScheduleAsapDoesNotModifyPresentationTime) {
  scheduling::test::MockFrameScheduler frame_scheduler;

  // Capture the relevant arguments of the ScheduleUpdateForSession() call.
  zx::time last_presentation_time = zx::time(0);
  auto last_id_pair = scheduling::SchedulingIdPair({
      .session_id = scheduling::kInvalidSessionId,
      .present_id = scheduling::kInvalidPresentId,
  });
  bool last_squashable = false;
  bool last_schedule_asap = false;

  frame_scheduler.set_schedule_update_for_session_callback(
      [&last_presentation_time, &last_id_pair, &last_squashable, &last_schedule_asap](
          zx::time presentation_time, scheduling::SchedulingIdPair id_pair, bool squashable,
          bool schedule_asap) {
        last_presentation_time = presentation_time;
        last_id_pair = id_pair;
        last_squashable = squashable;
        last_schedule_asap = schedule_asap;
      });

  auto presenter = CreateFlatlandPresenterImpl(frame_scheduler);

  const auto kIdPair = scheduling::SchedulingIdPair({
      .session_id = 1,
      .present_id = 2,
  });
  const zx::time kPresentationTime = zx::time(123);
  const bool kUnsquashable = false;
  const bool kScheduleAsap = true;

  presenter->ScheduleUpdateForSession(kPresentationTime, kIdPair, kUnsquashable,
                                      /*release_fences=*/{}, /*release_counters=*/{},
                                      /*present_fences=*/{}, kScheduleAsap);
  RunLoopUntilIdle();

  // zx::time is passed through.
  EXPECT_EQ(last_presentation_time, kPresentationTime);
  EXPECT_EQ(last_id_pair, kIdPair);
  EXPECT_EQ(last_squashable, !kUnsquashable);
  EXPECT_EQ(last_schedule_asap, kScheduleAsap);
}

TEST_F(FlatlandPresenterTest, RemoveSessionForwardsToFrameScheduler) {
  scheduling::test::MockFrameScheduler frame_scheduler;

  // FlatlandPresenter should first remove the session_id and *then* schedule a new frame for it.
  // Capture both ordering and arguments to confirm.
  std::vector<std::pair<std::string, scheduling::SessionId>> results;
  frame_scheduler.set_remove_session_callback([&results](scheduling::SessionId session_id) {
    results.emplace_back("RemoveSession", session_id);
  });
  frame_scheduler.set_schedule_update_for_session_callback(
      [&results](auto time, auto id_pair, auto squashable, auto schedule_asap) {
        results.emplace_back("Schedule", id_pair.session_id);
      });

  auto presenter = CreateFlatlandPresenterImpl(frame_scheduler);

  const scheduling::SessionId kSessionId = 1;
  presenter->RemoveSession(kSessionId, std::nullopt);

  RunLoopUntilIdle();

  // Since this function runs on the main thread, no RunLoopUntilIdle() is necessary.
  EXPECT_THAT(results, testing::ElementsAre(std::make_pair("RemoveSession", kSessionId),
                                            std::make_pair("Schedule", kSessionId)));
}

TEST_F(FlatlandPresenterTest, GetFuturePresentationInfosForwardsToFrameScheduler) {
  scheduling::test::MockFrameScheduler frame_scheduler;

  // Capture the relevant arguments of the GetFuturePresentationInfos() call.
  zx::duration last_requested_prediction_span;
  const zx::time kLatchPoint = zx::time(15122);
  const zx::time kPresentationTime = zx::time(15410);
  frame_scheduler.set_get_future_presentation_infos_callback(
      [&last_requested_prediction_span, kLatchPoint,
       kPresentationTime](zx::duration requested_prediction_span) {
        last_requested_prediction_span = requested_prediction_span;
        std::vector<scheduling::FuturePresentationInfo> presentation_infos(1);
        presentation_infos[0].latch_point = kLatchPoint;
        presentation_infos[0].presentation_time = kPresentationTime;
        return presentation_infos;
      });

  auto presenter = CreateFlatlandPresenterImpl(frame_scheduler);

  std::vector<scheduling::FuturePresentationInfo> presentation_infos =
      presenter->GetFuturePresentationInfos();
  RunLoopUntilIdle();

  // The requested prediction span should be reasonable - greater than 1 frame's worth of data.
  EXPECT_GT(last_requested_prediction_span, zx::msec(17));
  EXPECT_EQ(presentation_infos.size(), 1u);
  EXPECT_EQ(presentation_infos[0].latch_point, kLatchPoint);
  EXPECT_EQ(presentation_infos[0].presentation_time, kPresentationTime);
}

// Helper function for TakeFences test below.  Encapsulates two calls which always happen
// together in the test: AccumulateFences() and TakeFences().
static FlatlandPresenterImpl::Fences TakeFences(
    const std::shared_ptr<FlatlandPresenterImpl>& presenter,
    const std::unordered_map<scheduling::SessionId, scheduling::PresentId>& sessions_to_update) {
  presenter->AccumulateFences(sessions_to_update);
  return presenter->TakeFences();
}

TEST_F(FlatlandPresenterTest, TakeFences) {
  // The frame scheduler isn't actually used for this test, although it *is* required for the
  // presenter to properly stash the release fences (not inherently, just an implementation detail).
  scheduling::test::MockFrameScheduler frame_scheduler;
  auto presenter = CreateFlatlandPresenterImpl(frame_scheduler);

  const scheduling::SessionId kSessionIdA = 3;
  const scheduling::SessionId kSessionIdB = 7;

  // Create release and present fences.
  std::vector<zx::event> release_fences_A1 = utils::CreateEventArray(2);
  std::vector<zx_koid_t> release_fence_koids_A1 = ExtractKoids(release_fences_A1);
  std::vector<zx::counter> present_fences_A1 = utils::CreateCounterArray(2);
  std::vector<zx_koid_t> present_fence_koids_A1 = ExtractKoids(present_fences_A1);

  std::vector<zx::event> release_fences_A2 = utils::CreateEventArray(2);
  std::vector<zx_koid_t> release_fence_koids_A2 = ExtractKoids(release_fences_A2);
  std::vector<zx::counter> present_fences_A2 = utils::CreateCounterArray(2);
  std::vector<zx_koid_t> present_fence_koids_A2 = ExtractKoids(present_fences_A2);

  std::vector<zx::event> release_fences_B1 = utils::CreateEventArray(2);
  std::vector<zx_koid_t> release_fence_koids_B1 = ExtractKoids(release_fences_B1);
  std::vector<zx::counter> present_fences_B1 = utils::CreateCounterArray(2);
  std::vector<zx_koid_t> present_fence_koids_B1 = ExtractKoids(present_fences_B1);

  std::vector<zx::event> release_fences_B2 = utils::CreateEventArray(2);
  std::vector<zx_koid_t> release_fence_koids_B2 = ExtractKoids(release_fences_B2);
  std::vector<zx::counter> present_fences_B2 = utils::CreateCounterArray(2);
  std::vector<zx_koid_t> present_fence_koids_B2 = ExtractKoids(present_fences_B2);

  std::vector<zx::event> release_fences_B3 = utils::CreateEventArray(2);
  std::vector<zx_koid_t> release_fence_koids_B3 = ExtractKoids(release_fences_B3);
  std::vector<zx::counter> present_fences_B3 = utils::CreateCounterArray(2);
  std::vector<zx_koid_t> present_fence_koids_B3 = ExtractKoids(present_fences_B3);

  const auto present_id_A1 = scheduling::GetNextPresentId();
  presenter->ScheduleUpdateForSession(zx::time(0), {kSessionIdA, present_id_A1},
                                      /*unsquashable=*/true, std::move(release_fences_A1), {},
                                      std::move(present_fences_A1), false);
  const auto present_id_A2 = scheduling::GetNextPresentId();
  presenter->ScheduleUpdateForSession(zx::time(0), {kSessionIdA, present_id_A2},
                                      /*unsquashable=*/true, std::move(release_fences_A2), {},
                                      std::move(present_fences_A2), false);
  const auto present_id_B1 = scheduling::GetNextPresentId();
  presenter->ScheduleUpdateForSession(zx::time(0), {kSessionIdB, present_id_B1},
                                      /*unsquashable=*/true, std::move(release_fences_B1), {},
                                      std::move(present_fences_B1), false);
  const auto present_id_B2 = scheduling::GetNextPresentId();
  presenter->ScheduleUpdateForSession(zx::time(0), {kSessionIdB, present_id_B2},
                                      /*unsquashable=*/true, std::move(release_fences_B2), {},
                                      std::move(present_fences_B2), false);

  // There will be no fences yet, because ScheduleUpdateForSession() stashes the fences in a task
  // dispatched to the main thread, which hasn't run yet.
  auto fences_empty = TakeFences(presenter, {
                                                {kSessionIdA, present_id_A2},
                                                {kSessionIdB, present_id_B1},
                                            });
  EXPECT_TRUE(fences_empty.release_fences.empty());
  EXPECT_TRUE(fences_empty.present_fences.empty());

  // Try to take the same fences.  We should see the fences for A1/A2/B1, but not B2.  Note that we
  // don't explicitly mention A1, but we get the fences for it too, because A2 has a higher present
  // ID for the same session ID.
  RunLoopUntilIdle();
  auto fences_A1A2B1 = TakeFences(presenter, {
                                                 {kSessionIdA, present_id_A2},
                                                 {kSessionIdB, present_id_B1},
                                             });
  EXPECT_EQ(fences_A1A2B1.release_fences.size(), release_fence_koids_A1.size() +
                                                     release_fence_koids_A2.size() +
                                                     release_fence_koids_B1.size());
  auto fences_A1A2B1_koids = ExtractKoids(fences_A1A2B1.release_fences);
  for (auto koid : release_fence_koids_A1) {
    EXPECT_THAT(fences_A1A2B1_koids, testing::Contains(koid));
  }
  for (auto koid : release_fence_koids_A2) {
    EXPECT_THAT(fences_A1A2B1_koids, testing::Contains(koid));
  }
  for (auto koid : release_fence_koids_B1) {
    EXPECT_THAT(fences_A1A2B1_koids, testing::Contains(koid));
  }

  EXPECT_EQ(fences_A1A2B1.present_fences.size(), present_fence_koids_A1.size() +
                                                     present_fence_koids_A2.size() +
                                                     present_fence_koids_B1.size());
  std::vector<zx_koid_t> fences_A1A2B1_present_koids = ExtractKoids(fences_A1A2B1.present_fences);
  for (auto koid : present_fence_koids_A1) {
    EXPECT_THAT(fences_A1A2B1_present_koids, testing::Contains(koid));
  }
  for (auto koid : present_fence_koids_A2) {
    EXPECT_THAT(fences_A1A2B1_present_koids, testing::Contains(koid));
  }
  for (auto koid : present_fence_koids_B1) {
    EXPECT_THAT(fences_A1A2B1_present_koids, testing::Contains(koid));
  }

  // Register one more present.
  const auto present_id_B3 = scheduling::GetNextPresentId();
  presenter->ScheduleUpdateForSession(zx::time(0), {kSessionIdB, present_id_B3},
                                      /*unsquashable=*/true, std::move(release_fences_B3), {},
                                      std::move(present_fences_B3), false);
  RunLoopUntilIdle();
  auto fences_B2B3 = TakeFences(presenter, {
                                               {kSessionIdB, present_id_B3},
                                           });
  EXPECT_EQ(fences_B2B3.release_fences.size(),
            release_fence_koids_B2.size() + release_fence_koids_B3.size());
  auto fences_B2B3_koids = ExtractKoids(fences_B2B3.release_fences);
  for (auto koid : release_fence_koids_B2) {
    EXPECT_THAT(fences_B2B3_koids, testing::Contains(koid));
  }
  for (auto koid : release_fence_koids_B3) {
    EXPECT_THAT(fences_B2B3_koids, testing::Contains(koid));
  }

  EXPECT_EQ(fences_B2B3.present_fences.size(),
            present_fence_koids_B2.size() + present_fence_koids_B3.size());
  std::vector<zx_koid_t> fences_B2B3_present_koids = ExtractKoids(fences_B2B3.present_fences);
  for (auto koid : present_fence_koids_B2) {
    EXPECT_THAT(fences_B2B3_present_koids, testing::Contains(koid));
  }
  for (auto koid : present_fence_koids_B3) {
    EXPECT_THAT(fences_B2B3_present_koids, testing::Contains(koid));
  }
}

TEST_F(FlatlandPresenterTest, MultithreadedAccess) {
  scheduling::test::MockFrameScheduler frame_scheduler;

  // The FrameScheduler will be accessed in a thread-safe way, so the test instead collects the
  // scheduled updates and ensures the function was called the correct number of times with the
  // correct set of ID pairs.
  std::set<scheduling::SchedulingIdPair> scheduled_updates;

  // Also use a generic function call counter to test mutual exclusion between function calls.
  size_t function_count = 0;

  frame_scheduler.set_schedule_update_for_session_callback(
      [&scheduled_updates, &function_count](zx::time presentation_time,
                                            scheduling::SchedulingIdPair id_pair, bool squashable,
                                            bool schedule_asap) {
        scheduled_updates.insert(id_pair);

        ++function_count;
      });

  frame_scheduler.set_get_future_presentation_infos_callback(
      [&function_count](zx::duration requested_prediction_span)
          -> std::vector<scheduling::FuturePresentationInfo> {
        ++function_count;
        std::vector<scheduling::FuturePresentationInfo> infos;
        return infos;
      });

  auto presenter = CreateFlatlandPresenterImpl(frame_scheduler);

  // Start 10 "sessions", each of which registers 100 presents and schedules 100 updates.
  static constexpr uint64_t kNumSessions = 10;
  static constexpr uint64_t kNumPresents = 100;

  std::vector<std::thread> threads;

  std::mutex mutex;
  std::unordered_set<scheduling::PresentId> present_ids;

  const auto now = std::chrono::steady_clock::now();
  const auto then = now + std::chrono::milliseconds(50);

  std::atomic<uint64_t> sessions_posted_all_tasks = 0;
  std::atomic<uint64_t> loop_quits = 0;

  for (uint64_t session_id = 1; session_id <= kNumSessions; ++session_id) {
    std::thread thread([then, session_id, &mutex, &present_ids, &presenter, &loop_quits,
                        &sessions_posted_all_tasks]() {
      // Because each of the threads do a fixed amount of work, they may trigger in succession
      // without overlap. In order to bombard the system with concurrent requests, stall thread
      // execution until a specific time.
      std::this_thread::sleep_until(then);
      std::vector<scheduling::PresentId> presents;
      uint64_t presentation_info_count = 0;

      // Create a thread checker so that we can verify that the GetFuturePresentationInfos()
      // response runs on the correct thread.
      fit::thread_checker checker;
      EXPECT_TRUE(checker.is_thread_valid());

      // Set loop and dispatcher which is needed for GetFuturePresentationInfos().
      async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

      for (uint64_t i = 0; i < kNumPresents; ++i) {
        auto present_id = scheduling::GetNextPresentId();
        presenter->ScheduleUpdateForSession(zx::time(0), {session_id, present_id},
                                            /*unsquashable=*/true, /*release_fences=*/{},
                                            /*release_counters=*/{}, /*present_fences=*/{}, false);
        presents.push_back(present_id);

        // Yield with some randomness so the threads get jumbled up a bit.
        if (std::rand() % 4 == 0) {
          std::this_thread::yield();
        }

        presentation_info_count++;

        EXPECT_TRUE(checker.is_thread_valid());

        // When all kNumPresents tasks are posted back, this thread can safely return.
        if (presentation_info_count == kNumPresents) {
          loop_quits++;
          loop.Quit();
        }
      }

      // Acquire the test mutex and insert all IDs for later evaluation.
      {
        std::scoped_lock lock(mutex);
        for (const auto& present_id : presents) {
          present_ids.insert(present_id);
        }
      }

      sessions_posted_all_tasks++;
      // This thread should run until it receives all replies back from the frame scheduler.
      loop.Run();
    });

    threads.push_back(std::move(thread));
  }

  // Make calls directly to the FrameScheduler to mimic GFX, which runs on the "main" looper, which
  // in this test is just this thread.
  static constexpr scheduling::SessionId kGfxSessionId = kNumSessions + 1;
  static constexpr uint64_t kNumGfxPresents = 500;

  std::vector<scheduling::PresentId> gfx_presents;

  std::this_thread::sleep_until(then);

  for (uint64_t i = 0; i < kNumGfxPresents; ++i) {
    auto present_id = scheduling::GetNextPresentId();
    gfx_presents.push_back(present_id);

    frame_scheduler.ScheduleUpdateForSession(zx::time(0), {kGfxSessionId, present_id},
                                             /*squashable=*/true, /*schedule_asap=*/false);
  }

  {
    std::scoped_lock lock(mutex);
    for (const auto& present_id : gfx_presents) {
      present_ids.insert(present_id);
    }
  }

  // We need to be careful to account for the race where this line can be reached before all t
  // sessions have posted their GetFuturePresentationTimes() messages, leading the test to deadlock.
  //
  // First ensure all t threads have posted all their tasks on the main dispatcher.
  RunLoopUntil([&sessions_posted_all_tasks] { return sessions_posted_all_tasks == kNumSessions; });

  // Then, ensure the main dispatcher can reply to all the tasks, and posts its replies.
  RunLoopUntilIdle();

  // Finally, join all threads that can now process the replies.
  for (auto& t : threads) {
    t.join();
  }

  // Flush all the tasks posted by the presenter.
  RunLoopUntilIdle();

  // Verify that all the PresentIds are unique and that the set has the correct number of ID pairs.
  static constexpr uint64_t kTotalNumPresents = (kNumSessions * kNumPresents) + kNumGfxPresents;

  EXPECT_EQ(present_ids.size(), kTotalNumPresents);
  EXPECT_EQ(scheduled_updates.size(), kTotalNumPresents);

  // Verify that the correct total number of function calls were made.
  EXPECT_EQ(function_count, kTotalNumPresents);

  // Verify that every session received the total number of presentation_infos.
  EXPECT_EQ(loop_quits, kNumSessions);
}

}  // namespace flatland::test

// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/performance/cpu_profiler/targets.h"

#include <lib/zx/job.h>
#include <lib/zx/process.h>
#include <lib/zx/result.h>
#include <lib/zx/thread.h>
#include <zircon/errors.h>
#include <zircon/rights.h>

#include <algorithm>
#include <cstddef>
#include <set>
#include <span>
#include <thread>
#include <unordered_map>
#include <utility>
#include <vector>

#include <gtest/gtest.h>

#include "zircon/types.h"

TEST(TargetsTest, OwnJob) {
  profiler::TargetTree tree;
  zx::job parent;
  ASSERT_EQ(ZX_OK, zx::job::default_job()->duplicate(ZX_RIGHT_SAME_RIGHTS, &parent));
  elf_search::Searcher searcher;

  zx::result<profiler::JobTarget> target = profiler::MakeJobTarget(std::move(parent), searcher);
  ASSERT_TRUE(target.is_ok());

  // We don't expect to have any sub jobs
  EXPECT_TRUE(target->child_jobs.empty());

  // And the only process should be this process
  zx::unowned_process self = zx::process::self();
  zx_info_handle_basic_t info;
  ASSERT_EQ(ZX_OK, self->get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));

  EXPECT_EQ(target->processes.size(), size_t{1});
  EXPECT_EQ(target->processes.begin()->second.pid, info.koid);

  // There might be multiple threads running, but we should be one of them
  zx::unowned_thread thread_self = zx::thread::self();
  zx_info_handle_basic_t thread_info;
  ASSERT_EQ(ZX_OK, thread_self->get_info(ZX_INFO_HANDLE_BASIC, &thread_info, sizeof(thread_info),
                                         nullptr, nullptr));

  ASSERT_GT(target->processes.begin()->second.threads.size(), size_t{0});

  bool found_self = false;
  for (const auto& [_, target] : target->processes.begin()->second.threads) {
    if (target.tid == thread_info.koid) {
      found_self = true;
    }
  }

  EXPECT_TRUE(found_self);
}

// Ensure that if a sub job exits, we are able to get the remaining jobs that still exist
TEST(TargetsTest, JobsDone) {
  elf_search::Searcher searcher;
  std::atomic<bool> keep_going = true;
  // In the background, create and destroy some siblings
  std::thread sibling_maker{[&keep_going]() {
    while (keep_going.load(std::memory_order::relaxed)) {
      zx::job sibling;
      zx_status_t result = zx::job::create(*zx::job::default_job(), 0, &sibling);
      ASSERT_EQ(result, ZX_OK);
    }
  }};

  // Run a bunch of times to give the race condition a chance to occur.
  // Experimentally, the thread above is enough to trigger the race condition about 20% of the time.
  profiler::TargetTree tree;
  for (size_t i = 0; i < 100; i++) {
    zx::job parent;
    ASSERT_EQ(ZX_OK, zx::job::default_job()->duplicate(ZX_RIGHT_SAME_RIGHTS, &parent));

    zx::result<profiler::JobTarget> target = profiler::MakeJobTarget(std::move(parent), searcher);
    // This should succeed regardless of if a job disappeared from under us or not. In the log for
    // the test, we'll likely see some warnings about jobs not being found.
    ASSERT_TRUE(target.is_ok());

    // We should have 1 or zero sub jobs.
    size_t num_subjobs = target->child_jobs.size();
    EXPECT_TRUE(num_subjobs == 0 || num_subjobs == 1);
  }

  keep_going.store(false, std::memory_order_relaxed);
  sibling_maker.join();
}

TEST(TargetsTest, TargetTreeAddProcessTopLevel) {
  profiler::TargetTree tree;
  profiler::ProcessTarget p1{zx::process{ZX_HANDLE_INVALID}, 1, "",
                             std::unordered_map<zx_koid_t, profiler::ThreadTarget>{}};

  EXPECT_TRUE(tree.AddProcess(std::move(p1)).is_ok());

  profiler::ProcessTarget p2{zx::process{ZX_HANDLE_INVALID}, 1, "",
                             std::unordered_map<zx_koid_t, profiler::ThreadTarget>{}};
  zx::result res = tree.AddProcess(std::move(p2));
  ASSERT_TRUE(res.is_error());
  EXPECT_EQ(res.status_value(), ZX_ERR_ALREADY_EXISTS);
}

TEST(TargetsTest, TargetTreeAddJobTopLevel) {
  profiler::TargetTree tree;
  profiler::JobTarget j1{zx::job{ZX_HANDLE_INVALID}, 2, std::span<const zx_koid_t>{}};

  EXPECT_TRUE(tree.AddJob(std::move(j1)).is_ok());

  profiler::JobTarget j2{zx::job{ZX_HANDLE_INVALID}, 2, std::span<const zx_koid_t>{}};
  zx::result res = tree.AddJob(std::move(j2));
  ASSERT_TRUE(res.is_error());
  EXPECT_EQ(res.status_value(), ZX_ERR_ALREADY_EXISTS);
}

TEST(TargetsTest, TargetTreeAddThreadTopLevel) {
  profiler::TargetTree tree;
  profiler::ThreadTarget t1{.handle = zx::thread{ZX_HANDLE_INVALID}, .tid = 3, .name = ""};

  zx::result res = tree.AddThread(4, std::move(t1));
  ASSERT_TRUE(res.is_error());
  EXPECT_EQ(res.status_value(), ZX_ERR_NOT_FOUND);

  profiler::ProcessTarget p1{zx::process{ZX_HANDLE_INVALID}, 4, "",
                             std::unordered_map<zx_koid_t, profiler::ThreadTarget>{}};
  EXPECT_TRUE(tree.AddProcess(std::move(p1)).is_ok());

  profiler::ThreadTarget t2{.handle = zx::thread{ZX_HANDLE_INVALID}, .tid = 3, .name = ""};
  res = tree.AddThread(4, std::move(t2));
  ASSERT_TRUE(res.is_ok());
}

TEST(TargetsTest, TargetTreeAddTargetsNestedNonExist) {
  profiler::TargetTree tree;

  profiler::ThreadTarget t1{.handle = zx::thread{ZX_HANDLE_INVALID}, .tid = 1, .name = ""};
  profiler::ProcessTarget p1{zx::process{ZX_HANDLE_INVALID}, 2, "",
                             std::unordered_map<zx_koid_t, profiler::ThreadTarget>{}};
  profiler::JobTarget j1{zx::job{ZX_HANDLE_INVALID}, 3, std::vector<zx_koid_t>{}};
  profiler::JobTarget j2{zx::job{ZX_HANDLE_INVALID}, 4, std::vector<zx_koid_t>{3}};
  profiler::JobTarget j3{zx::job{ZX_HANDLE_INVALID}, 5, std::vector<zx_koid_t>{3, 4}};

  zx::result res = tree.AddThread(std::vector<zx_koid_t>{3, 4, 5}, 2, std::move(t1));
  EXPECT_TRUE(res.is_error());
  EXPECT_EQ(res.status_value(), ZX_ERR_NOT_FOUND);
  res = tree.AddProcess(std::vector<zx_koid_t>{3, 4, 5}, std::move(p1));
  EXPECT_TRUE(res.is_error());
  EXPECT_EQ(res.status_value(), ZX_ERR_NOT_FOUND);

  res = tree.AddJob(std::vector<zx_koid_t>{3, 4}, std::move(j3));
  EXPECT_TRUE(res.is_error());
  EXPECT_EQ(res.status_value(), ZX_ERR_NOT_FOUND);
  res = tree.AddJob(std::vector<zx_koid_t>{3}, std::move(j2));
  EXPECT_TRUE(res.is_error());
  EXPECT_EQ(res.status_value(), ZX_ERR_NOT_FOUND);
}

TEST(TargetsTest, TargetTreeAddTargetsNested) {
  profiler::TargetTree tree;

  profiler::ThreadTarget t1{.handle = zx::thread{ZX_HANDLE_INVALID}, .tid = 1, .name = ""};
  profiler::ProcessTarget p1{zx::process{ZX_HANDLE_INVALID}, 2, "",
                             std::unordered_map<zx_koid_t, profiler::ThreadTarget>{}};
  profiler::JobTarget j1{zx::job{ZX_HANDLE_INVALID}, 3, std::vector<zx_koid_t>{}};
  profiler::JobTarget j2{zx::job{ZX_HANDLE_INVALID}, 4, std::vector<zx_koid_t>{3}};
  profiler::JobTarget j3{zx::job{ZX_HANDLE_INVALID}, 5, std::vector<zx_koid_t>{3, 4}};

  zx::result res = tree.AddJob(std::vector<zx_koid_t>{}, std::move(j1));
  EXPECT_TRUE(res.is_ok());

  res = tree.AddJob(std::vector<zx_koid_t>{3}, std::move(j2));
  EXPECT_TRUE(res.is_ok());

  res = tree.AddJob(std::vector<zx_koid_t>{3, 4}, std::move(j3));
  EXPECT_TRUE(res.is_ok());

  res = tree.AddProcess(std::vector<zx_koid_t>{3, 4, 5}, std::move(p1));
  EXPECT_TRUE(res.is_ok());

  res = tree.AddThread(std::vector<zx_koid_t>{3, 4, 5}, 2, std::move(t1));
  EXPECT_TRUE(res.is_ok());
}

TEST(TargetsTest, TargetTreeForEachProcess) {
  profiler::TargetTree tree;

  constexpr zx_koid_t top_level_pid1 = 4;
  constexpr zx_koid_t top_level_pid2 = 5;
  constexpr zx_koid_t nested_pid1 = 6;
  constexpr zx_koid_t nested_pid2 = 7;
  constexpr zx_koid_t nested_pid3 = 8;
  constexpr zx_koid_t nested_pid4 = 9;
  constexpr zx_koid_t nested_pid5 = 10;
  constexpr zx_koid_t nested_pid6 = 11;
  profiler::ProcessTarget p1{zx::process{ZX_HANDLE_INVALID}, top_level_pid1, "",
                             std::unordered_map<zx_koid_t, profiler::ThreadTarget>{}};
  profiler::ProcessTarget p2{zx::process{ZX_HANDLE_INVALID}, top_level_pid2, "",
                             std::unordered_map<zx_koid_t, profiler::ThreadTarget>{}};
  profiler::ProcessTarget p3{zx::process{ZX_HANDLE_INVALID}, nested_pid1, "",
                             std::unordered_map<zx_koid_t, profiler::ThreadTarget>{}};
  profiler::ProcessTarget p4{zx::process{ZX_HANDLE_INVALID}, nested_pid2, "",
                             std::unordered_map<zx_koid_t, profiler::ThreadTarget>{}};
  profiler::ProcessTarget p5{zx::process{ZX_HANDLE_INVALID}, nested_pid3, "",
                             std::unordered_map<zx_koid_t, profiler::ThreadTarget>{}};
  profiler::ProcessTarget p6{zx::process{ZX_HANDLE_INVALID}, nested_pid4, "",
                             std::unordered_map<zx_koid_t, profiler::ThreadTarget>{}};
  profiler::ProcessTarget p7{zx::process{ZX_HANDLE_INVALID}, nested_pid5, "",
                             std::unordered_map<zx_koid_t, profiler::ThreadTarget>{}};
  profiler::ProcessTarget p8{zx::process{ZX_HANDLE_INVALID}, nested_pid6, "",
                             std::unordered_map<zx_koid_t, profiler::ThreadTarget>{}};
  profiler::JobTarget j1{zx::job{ZX_HANDLE_INVALID}, 1, std::vector<zx_koid_t>{}};
  profiler::JobTarget j2{zx::job{ZX_HANDLE_INVALID}, 2, std::vector<zx_koid_t>{1}};
  profiler::JobTarget j3{zx::job{ZX_HANDLE_INVALID}, 3, std::vector<zx_koid_t>{1, 2}};

  zx::result res = tree.AddJob(std::move(j1));
  EXPECT_TRUE(res.is_ok());

  res = tree.AddJob(std::vector<zx_koid_t>{1}, std::move(j2));
  EXPECT_TRUE(res.is_ok());

  res = tree.AddJob(std::vector<zx_koid_t>{1, 2}, std::move(j3));
  EXPECT_TRUE(res.is_ok());

  // Add a bunch of processes in the tree at various levels
  res = tree.AddProcess(std::move(p1));
  EXPECT_TRUE(res.is_ok());
  res = tree.AddProcess(std::vector<zx_koid_t>{}, std::move(p2));
  EXPECT_TRUE(res.is_ok());

  res = tree.AddProcess(std::vector<zx_koid_t>{1}, std::move(p3));
  EXPECT_TRUE(res.is_ok());
  res = tree.AddProcess(std::vector<zx_koid_t>{1}, std::move(p4));
  EXPECT_TRUE(res.is_ok());

  res = tree.AddProcess(std::vector<zx_koid_t>{1, 2}, std::move(p5));
  EXPECT_TRUE(res.is_ok());
  res = tree.AddProcess(std::vector<zx_koid_t>{1, 2}, std::move(p6));
  EXPECT_TRUE(res.is_ok());

  res = tree.AddProcess(std::vector<zx_koid_t>{1, 2, 3}, std::move(p7));
  EXPECT_TRUE(res.is_ok());
  res = tree.AddProcess(std::vector<zx_koid_t>{1, 2, 3}, std::move(p8));
  EXPECT_TRUE(res.is_ok());

  std::set<zx_koid_t> seen;
  res = tree.ForEachProcess([&seen](std::span<const zx_koid_t> job_path,
                                    const profiler::ProcessTarget& process) -> zx::result<> {
    EXPECT_EQ(seen.find(process.pid), seen.end());
    seen.insert(process.pid);
    switch (process.pid) {
      case top_level_pid1:
      case top_level_pid2: {
        std::vector<zx_koid_t> expected{};
        EXPECT_TRUE(std::equal(job_path.begin(), job_path.end(), expected.begin()));
        break;
      }
      case nested_pid1:
      case nested_pid2: {
        std::vector<zx_koid_t> expected{1};
        EXPECT_TRUE(std::equal(job_path.begin(), job_path.end(), expected.begin()));
        break;
      }
      case nested_pid3:
      case nested_pid4: {
        std::vector<zx_koid_t> expected{1, 2};
        EXPECT_TRUE(std::equal(job_path.begin(), job_path.end(), expected.begin()));
        break;
      }
      case nested_pid5:
      case nested_pid6: {
        std::vector<zx_koid_t> expected{1, 2, 3};
        EXPECT_TRUE(std::equal(job_path.begin(), job_path.end(), expected.begin()));
        break;
      }
      default:
        return zx::error(ZX_ERR_OUT_OF_RANGE);
    }
    return zx::ok();
  });
  EXPECT_EQ(seen.size(), size_t{8});
  EXPECT_TRUE(res.is_ok());

  // Check that a top level error gets propagated
  res = tree.ForEachProcess([](std::span<const zx_koid_t> job_path,
                               const profiler::ProcessTarget& process) -> zx::result<> {
    return zx::make_result(process.pid == top_level_pid2 ? ZX_ERR_BAD_STATE : ZX_OK);
  });
  EXPECT_TRUE(res.is_error());
  EXPECT_EQ(res.status_value(), ZX_ERR_BAD_STATE);

  // Check that a nested error gets propagated
  res = tree.ForEachProcess([](std::span<const zx_koid_t> job_path,
                               const profiler::ProcessTarget& process) -> zx::result<> {
    return zx::make_result(process.pid == nested_pid6 ? ZX_ERR_BAD_STATE : ZX_OK);
  });
  EXPECT_TRUE(res.is_error());
  EXPECT_EQ(res.status_value(), ZX_ERR_BAD_STATE);
}

TEST(TargetsTest, TargetTreeForEachJob) {
  profiler::TargetTree tree;

  profiler::JobTarget j1{zx::job{ZX_HANDLE_INVALID}, 1, std::vector<zx_koid_t>{}};
  profiler::JobTarget j2{zx::job{ZX_HANDLE_INVALID}, 2, std::vector<zx_koid_t>{1}};
  profiler::JobTarget j3{zx::job{ZX_HANDLE_INVALID}, 3, std::vector<zx_koid_t>{1, 2}};

  zx::result res = tree.AddJob(std::move(j1));
  EXPECT_TRUE(res.is_ok());

  res = tree.AddJob(std::vector<zx_koid_t>{1}, std::move(j2));
  EXPECT_TRUE(res.is_ok());

  res = tree.AddJob(std::vector<zx_koid_t>{1, 2}, std::move(j3));
  EXPECT_TRUE(res.is_ok());

  std::set<zx_koid_t> seen;
  res = tree.ForEachJob([&seen](const profiler::JobTarget& job) -> zx::result<> {
    EXPECT_EQ(seen.find(job.job_id), seen.end());
    seen.insert(job.job_id);
    return zx::ok();
  });
  EXPECT_EQ(seen.size(), size_t{3});
  EXPECT_EQ(size_t{1}, seen.count(1));
  EXPECT_EQ(size_t{1}, seen.count(2));
  EXPECT_EQ(size_t{1}, seen.count(3));
  EXPECT_TRUE(res.is_ok());

  res = tree.ForEachJob([](const profiler::JobTarget& job) -> zx::result<> {
    return zx::make_result(job.job_id == 1 ? ZX_ERR_BAD_STATE : ZX_OK);
  });
  EXPECT_TRUE(res.is_error());
  EXPECT_EQ(res.status_value(), ZX_ERR_BAD_STATE);

  res = tree.ForEachJob([](const profiler::JobTarget& job) -> zx::result<> {
    return zx::make_result(job.job_id == 3 ? ZX_ERR_BAD_STATE : ZX_OK);
  });
  EXPECT_TRUE(res.is_error());
  EXPECT_EQ(res.status_value(), ZX_ERR_BAD_STATE);
}

TEST(TargetsTest, TargetTreeGetProcessTopLevel) {
  profiler::TargetTree tree;
  profiler::ProcessTarget p1{zx::process{ZX_HANDLE_INVALID}, 42, "test_process",
                             std::unordered_map<zx_koid_t, profiler::ThreadTarget>{}};
  EXPECT_TRUE(tree.AddProcess(std::move(p1)).is_ok());

  zx::result<profiler::ProcessTarget*> found = tree.GetProcess(std::span<const zx_koid_t>{}, 42);
  ASSERT_TRUE(found.is_ok());
  ASSERT_NE(*found, nullptr);
  EXPECT_EQ((*found)->pid, zx_koid_t{42});
  EXPECT_EQ((*found)->name, "test_process");

  zx::result<profiler::ProcessTarget*> not_found =
      tree.GetProcess(std::span<const zx_koid_t>{}, 99);
  EXPECT_TRUE(not_found.is_error());
  EXPECT_EQ(not_found.status_value(), ZX_ERR_NOT_FOUND);
}

TEST(TargetsTest, TargetTreeGetProcessNested) {
  profiler::TargetTree tree;
  profiler::JobTarget j1{zx::job{ZX_HANDLE_INVALID}, 1, std::vector<zx_koid_t>{}};
  profiler::JobTarget j2{zx::job{ZX_HANDLE_INVALID}, 2, std::vector<zx_koid_t>{1}};
  profiler::JobTarget j3{zx::job{ZX_HANDLE_INVALID}, 3, std::vector<zx_koid_t>{1, 2}};

  ASSERT_TRUE(tree.AddJob(std::move(j1)).is_ok());
  ASSERT_TRUE(tree.AddJob(std::vector<zx_koid_t>{1}, std::move(j2)).is_ok());
  ASSERT_TRUE(tree.AddJob(std::vector<zx_koid_t>{1, 2}, std::move(j3)).is_ok());

  profiler::ProcessTarget p1{zx::process{ZX_HANDLE_INVALID}, 100, "nested_process",
                             std::unordered_map<zx_koid_t, profiler::ThreadTarget>{}};
  ASSERT_TRUE(tree.AddProcess(std::vector<zx_koid_t>{1, 2, 3}, std::move(p1)).is_ok());

  const std::vector<zx_koid_t> path{1, 2, 3};
  zx::result<profiler::ProcessTarget*> found = tree.GetProcess(path, 100);
  ASSERT_TRUE(found.is_ok());
  ASSERT_NE(*found, nullptr);
  EXPECT_EQ((*found)->pid, zx_koid_t{100});
  EXPECT_EQ((*found)->name, "nested_process");

  zx::result<profiler::ProcessTarget*> not_found_pid = tree.GetProcess(path, 101);
  EXPECT_TRUE(not_found_pid.is_error());
  EXPECT_EQ(not_found_pid.status_value(), ZX_ERR_NOT_FOUND);

  const std::vector<zx_koid_t> invalid_path{1, 2, 99};
  zx::result<profiler::ProcessTarget*> not_found_path = tree.GetProcess(invalid_path, 100);
  EXPECT_TRUE(not_found_path.is_error());
  EXPECT_EQ(not_found_path.status_value(), ZX_ERR_NOT_FOUND);
}

TEST(TargetsTest, PointLookupProcessAndThreadNames) {
  profiler::TargetTree tree;

  // Add top-level process with threads
  std::unordered_map<zx_koid_t, profiler::ThreadTarget> p1_threads;
  p1_threads.try_emplace(
      101,
      profiler::ThreadTarget{.handle = zx::thread{ZX_HANDLE_INVALID}, .tid = 101, .name = "t1"});
  p1_threads.try_emplace(
      102,
      profiler::ThreadTarget{.handle = zx::thread{ZX_HANDLE_INVALID}, .tid = 102, .name = "t2"});
  profiler::ProcessTarget p1{zx::process{ZX_HANDLE_INVALID}, 10, "proc1", std::move(p1_threads)};
  ASSERT_TRUE(tree.AddProcess(std::move(p1)).is_ok());

  // Add nested job with process and threads
  profiler::JobTarget j1{zx::job{ZX_HANDLE_INVALID}, 1, std::vector<zx_koid_t>{}};
  ASSERT_TRUE(tree.AddJob(std::move(j1)).is_ok());

  std::unordered_map<zx_koid_t, profiler::ThreadTarget> p2_threads;
  p2_threads.try_emplace(
      201,
      profiler::ThreadTarget{.handle = zx::thread{ZX_HANDLE_INVALID}, .tid = 201, .name = "t3"});
  profiler::ProcessTarget p2{zx::process{ZX_HANDLE_INVALID}, 20, "proc2", std::move(p2_threads)};
  ASSERT_TRUE(tree.AddProcess(std::vector<zx_koid_t>{1}, std::move(p2)).is_ok());

  // Test GetProcessName
  EXPECT_EQ(tree.GetProcessName(10), "proc1");
  EXPECT_EQ(tree.GetProcessName(20), "proc2");
  EXPECT_EQ(tree.GetProcessName(999), std::nullopt);

  // Test GetThreadName
  EXPECT_EQ(tree.GetThreadName(101), "t1");
  EXPECT_EQ(tree.GetThreadName(102), "t2");
  EXPECT_EQ(tree.GetThreadName(201), "t3");
  EXPECT_EQ(tree.GetThreadName(999), std::nullopt);
}

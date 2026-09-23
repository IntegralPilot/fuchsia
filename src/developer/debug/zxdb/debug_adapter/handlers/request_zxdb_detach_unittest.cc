// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/debug_adapter/handlers/request_zxdb_detach.h"

#include <algorithm>

#include <gtest/gtest.h>

#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/debug_adapter/context_test.h"

namespace zxdb {

namespace {

class RequestZxdbDetachTest : public DebugAdapterContextTest {};

TEST_F(RequestZxdbDetachTest, DetachPid) {
  InitializeDebugging();

  constexpr size_t kProcess2Koid = kProcessKoid + 1;

  InjectProcess(kProcessKoid);
  InjectProcess(kProcess2Koid);
  RunClient();

  dap::ZxdbDetachRequest request = {};
  request.pid = kProcessKoid;
  auto response = client().send(request);

  context().OnStreamReadable();
  loop().RunUntilNoTasks();

  RunPendingClientCalls();
  auto got = response.get();
  EXPECT_FALSE(got.error);

  // Detaching from just one target will delete the process object, but not the target, so we should
  // still have two targets.
  const auto& targets = context().session()->system().GetTargets();
  EXPECT_EQ(targets.size(), 2u);

  // We should have one target without a process object and one with the pid that we did not detach
  // from.
  EXPECT_NE(std::ranges::find_if(
                targets, [](const Target* target) { return target->GetProcess() == nullptr; }),
            targets.end());
  EXPECT_NE(std::ranges::find_if(targets,
                                 [](const Target* target) {
                                   return target->GetProcess()
                                              ? target->GetProcess()->GetKoid() == kProcess2Koid
                                              : false;
                                 }),
            targets.end());
}

TEST_F(RequestZxdbDetachTest, DetachAll) {
  InitializeDebugging();

  constexpr size_t kProcess2Koid = kProcessKoid + 1;
  constexpr size_t kProcess3Koid = kProcessKoid + 2;
  constexpr size_t kProcess4Koid = kProcessKoid + 3;

  InjectProcess(kProcessKoid);
  InjectProcess(kProcess2Koid);
  InjectProcess(kProcess3Koid);
  InjectProcess(kProcess4Koid);

  RunClient();

  dap::ZxdbDetachRequest request = {};
  request.all = true;
  auto response = client().send(request);

  context().OnStreamReadable();
  loop().RunUntilNoTasks();

  RunPendingClientCalls();
  auto got = response.get();
  EXPECT_FALSE(got.error);

  // All targets should be deleted except for the required default target, which should no longer
  // have a process associated with it.
  ASSERT_EQ(context().session()->system().GetTargets().size(), 1u);
  ASSERT_TRUE(
      std::ranges::all_of(context().session()->system().GetTargets(), [](const Target* target) {
        return target->GetState() == Target::State::kNone && target->GetProcess() == nullptr;
      }));
}

TEST_F(RequestZxdbDetachTest, InvalidArgs) {
  InitializeDebugging();

  // Both pid and all
  {
    dap::ZxdbDetachRequest request = {};
    request.pid = kProcessKoid;
    request.all = true;
    auto response = client().send(request);

    context().OnStreamReadable();
    loop().RunUntilNoTasks();

    RunPendingClientCalls();
    auto got = response.get();
    EXPECT_TRUE(got.error);
  }

  // Neither pid nor all
  {
    dap::ZxdbDetachRequest request = {};
    auto response = client().send(request);

    context().OnStreamReadable();
    loop().RunUntilNoTasks();

    RunPendingClientCalls();
    auto got = response.get();
    EXPECT_TRUE(got.error);
  }

  // Negative pid
  {
    dap::ZxdbDetachRequest request = {};
    request.pid = -1;
    auto response = client().send(request);

    context().OnStreamReadable();
    loop().RunUntilNoTasks();

    RunPendingClientCalls();
    auto got = response.get();
    EXPECT_TRUE(got.error);
  }
}

}  // namespace

}  // namespace zxdb

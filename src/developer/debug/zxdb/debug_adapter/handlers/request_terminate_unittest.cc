// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/debug_adapter/handlers/request_terminate.h"

#include <gtest/gtest.h>

#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/client/target_impl.h"
#include "src/developer/debug/zxdb/debug_adapter/context_test.h"

namespace zxdb {

namespace {

class RequestTerminateTest : public DebugAdapterContextTest {};

TEST_F(RequestTerminateTest, TerminateSpecificTargetByKoid) {
  InitializeDebugging();

  auto target_impls = session().system().GetTargetImpls();
  ASSERT_EQ(1u, target_impls.size());
  TargetImpl* target1 = target_impls[0];
  target1->CreateProcess(Process::StartType::kLaunch, kProcessKoid, "process1", 0, {},
                         std::nullopt);

  TargetImpl* target2 = session().system().CreateNewTargetImpl(nullptr);
  target2->CreateProcess(Process::StartType::kAttach, kProcessKoid + 1, "process2", 0, {},
                         std::nullopt);

  ASSERT_EQ(session().system().GetTargets().size(), 2u);

  RunClient();

  // Explicitly terminate target1 (kProcessKoid)
  dap::ZxdbTerminateRequest request = {};
  request.koid = kProcessKoid;
  auto response = client().send(request);

  context().OnStreamReadable();
  loop().RunUntilNoTasks();

  RunPendingClientCalls();
  auto got = response.get();
  EXPECT_FALSE(got.error);

  // target1 should be killed/terminated
  EXPECT_EQ(target1->GetState(), Target::State::kNone);
  EXPECT_EQ(target1->GetProcess(), nullptr);

  // target2 should be left completely untouched and running!
  EXPECT_EQ(target2->GetState(), Target::State::kRunning);
  EXPECT_NE(target2->GetProcess(), nullptr);
}

TEST_F(RequestTerminateTest, TerminateInvalidKoidValue) {
  InitializeDebugging();

  auto target_impls = session().system().GetTargetImpls();
  ASSERT_EQ(1u, target_impls.size());
  TargetImpl* target = target_impls[0];
  target->CreateProcess(Process::StartType::kLaunch, kProcessKoid, "process1", 0, {}, std::nullopt);

  RunClient();

  // Terminate with unknown KOID
  dap::ZxdbTerminateRequest request = {};
  request.koid = kProcessKoid + 999;
  auto response = client().send(request);

  context().OnStreamReadable();
  loop().RunUntilNoTasks();

  RunPendingClientCalls();
  auto got = response.get();
  EXPECT_TRUE(got.error);
}

TEST_F(RequestTerminateTest, TerminateNonPositiveKoid) {
  InitializeDebugging();

  RunClient();

  // Test KOID = 0
  {
    dap::ZxdbTerminateRequest request = {};
    request.koid = 0;
    auto response = client().send(request);

    context().OnStreamReadable();
    loop().RunUntilNoTasks();

    RunPendingClientCalls();
    auto got = response.get();
    EXPECT_TRUE(got.error);
  }

  // Test KOID = -5 (negative values)
  {
    dap::ZxdbTerminateRequest request = {};
    request.koid = -5;
    auto response = client().send(request);

    context().OnStreamReadable();
    loop().RunUntilNoTasks();

    RunPendingClientCalls();
    auto got = response.get();
    EXPECT_TRUE(got.error);
  }
}

TEST_F(RequestTerminateTest, TerminateWithoutKoidFails) {
  InitializeDebugging();

  auto target_impls = session().system().GetTargetImpls();
  ASSERT_EQ(1u, target_impls.size());
  TargetImpl* target = target_impls[0];
  target->CreateProcess(Process::StartType::kLaunch, kProcessKoid, "process1", 0, {}, std::nullopt);

  RunClient();

  dap::ZxdbTerminateRequest request = {};
  auto response = client().send(request);

  context().OnStreamReadable();
  loop().RunUntilNoTasks();

  RunPendingClientCalls();
  auto got = response.get();
  EXPECT_TRUE(got.error);
  EXPECT_EQ(got.error.message, "zxdb does not support terminate without a process KOID.");

  EXPECT_EQ(target->GetState(), Target::State::kRunning);
  EXPECT_NE(target->GetProcess(), nullptr);
}

}  // namespace

}  // namespace zxdb

// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/debug_adapter/handlers/request_process.h"

#include <gtest/gtest.h>

#include "src/developer/debug/zxdb/client/target_impl.h"
#include "src/developer/debug/zxdb/debug_adapter/context_test.h"

namespace zxdb {

namespace {

class RequestProcessTest : public DebugAdapterContextTest {};

TEST_F(RequestProcessTest, GetAllProcessesEmpty) {
  InitializeDebugging();

  auto response = client().send(dap::ZxdbProcessRequest());
  context().OnStreamReadable();
  RunClient();

  auto got = response.get();
  EXPECT_FALSE(got.error);
  EXPECT_EQ(got.response.processes.size(), 0u);
}

TEST_F(RequestProcessTest, GetAllProcesses) {
  InitializeDebugging();

  constexpr uint64_t kProcess2Koid = kProcessKoid + 1;
  constexpr uint64_t kThread2Koid = kThreadKoid + 1;
  constexpr uint64_t kThread3Koid = kThreadKoid + 2;

  InjectProcess(kProcessKoid);
  RunClient();
  InjectThread(kProcessKoid, kThreadKoid);
  RunClient();
  InjectThread(kProcessKoid, kThread2Koid);
  RunClient();

  context().session()->system().CreateNewTargetImpl(nullptr)->CreateProcessForTesting(
      kProcess2Koid, "test_process_2");
  RunClient();
  InjectThread(kProcess2Koid, kThread3Koid);
  RunClient();

  auto response = client().send(dap::ZxdbProcessRequest());
  context().OnStreamReadable();
  RunClient();

  auto got = response.get();
  EXPECT_FALSE(got.error);
  ASSERT_EQ(got.response.processes.size(), 2u);

  EXPECT_EQ(got.response.processes[0].id, static_cast<int64_t>(kProcessKoid));
  ASSERT_EQ(got.response.processes[0].threads.size(), 2u);
  EXPECT_EQ(got.response.processes[0].threads[0].id, static_cast<int64_t>(kThreadKoid));
  EXPECT_EQ(got.response.processes[0].threads[1].id, static_cast<int64_t>(kThread2Koid));

  EXPECT_EQ(got.response.processes[1].id, static_cast<int64_t>(kProcess2Koid));
  EXPECT_EQ(got.response.processes[1].name, "test_process_2");
  ASSERT_EQ(got.response.processes[1].threads.size(), 1u);
  EXPECT_EQ(got.response.processes[1].threads[0].id, static_cast<int64_t>(kThread3Koid));
}

TEST_F(RequestProcessTest, GetSpecificProcessByPid) {
  InitializeDebugging();

  constexpr uint64_t kProcess2Koid = kProcessKoid + 1;
  constexpr uint64_t kThread2Koid = kThreadKoid + 1;

  InjectProcess(kProcessKoid);
  RunClient();
  InjectThread(kProcessKoid, kThreadKoid);
  RunClient();

  context().session()->system().CreateNewTargetImpl(nullptr)->CreateProcessForTesting(
      kProcess2Koid, "test_process_2");
  RunClient();
  InjectThread(kProcess2Koid, kThread2Koid);
  RunClient();

  dap::ZxdbProcessRequest request;
  request.pid = static_cast<int64_t>(kProcess2Koid);
  auto response = client().send(request);

  context().OnStreamReadable();
  RunClient();

  auto got = response.get();
  EXPECT_FALSE(got.error);
  ASSERT_EQ(got.response.processes.size(), 1u);
  EXPECT_EQ(got.response.processes[0].id, static_cast<int64_t>(kProcess2Koid));
  EXPECT_EQ(got.response.processes[0].name, "test_process_2");
  ASSERT_EQ(got.response.processes[0].threads.size(), 1u);
  EXPECT_EQ(got.response.processes[0].threads[0].id, static_cast<int64_t>(kThread2Koid));
}

TEST_F(RequestProcessTest, ProcessNotFound) {
  InitializeDebugging();

  InjectProcess(kProcessKoid);
  RunClient();

  dap::ZxdbProcessRequest request;
  request.pid = 9999999;
  auto response = client().send(request);

  context().OnStreamReadable();
  RunClient();

  auto got = response.get();
  EXPECT_TRUE(got.error);
  EXPECT_EQ(got.error.message, "Process not found");
}

TEST_F(RequestProcessTest, InvalidPid) {
  InitializeDebugging();

  {
    dap::ZxdbProcessRequest request;
    request.pid = 0;
    auto response = client().send(request);

    context().OnStreamReadable();
    RunClient();

    auto got = response.get();
    EXPECT_TRUE(got.error);
    EXPECT_EQ(got.error.message, "PID must be positive");
  }

  {
    dap::ZxdbProcessRequest request;
    request.pid = -10;
    auto response = client().send(request);

    context().OnStreamReadable();
    RunClient();

    auto got = response.get();
    EXPECT_TRUE(got.error);
    EXPECT_EQ(got.error.message, "PID must be positive");
  }
}

}  // namespace

}  // namespace zxdb

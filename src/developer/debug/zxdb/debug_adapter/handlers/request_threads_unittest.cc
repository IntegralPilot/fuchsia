// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/debug_adapter/handlers/request_threads.h"

#include <dap/typeof.h>
#include <gtest/gtest.h>

#include "src/developer/debug/zxdb/client/target_impl.h"
#include "src/developer/debug/zxdb/debug_adapter/context.h"
#include "src/developer/debug/zxdb/debug_adapter/context_test.h"

namespace zxdb {

namespace {

class RequestThreadsTest : public DebugAdapterContextTest {};

}  // namespace

TEST_F(RequestThreadsTest, ListThreads) {
  InitializeDebugging();

  InjectProcess(kProcessKoid);
  // Run client to receive process started event.
  RunClient();
  InjectThread(kProcessKoid, kThreadKoid);
  // Run client to receive threads started event.
  RunClient();

  // Send Threads request from the client.
  auto response = client().send(dap::ThreadsRequest());

  // Read request and process it in server.
  context().OnStreamReadable();

  // Run client to receive threads response.
  RunClient();
  auto got = response.get();
  EXPECT_FALSE(got.error);
  EXPECT_EQ(got.response.threads.size(), 1u);
}

TEST_F(RequestThreadsTest, ListThreadsForMultipleProcess) {
  InitializeDebugging();

  InjectProcess(kProcessKoid);
  // Run client to receive process started event.
  RunClient();
  InjectThread(kProcessKoid, kThreadKoid);
  // Run client to receive threads started event.
  RunClient();

  // Inject another target with a active process.
  context().session()->system().CreateNewTargetImpl(nullptr)->CreateProcessForTesting(
      kProcessKoid + 1, "test2");
  RunClient();
  // Inject a thread in the second process.
  InjectThread(kProcessKoid + 1, kThreadKoid + 1);
  RunClient();

  // Send Threads request from the client.
  auto response = client().send(dap::ThreadsRequest());

  // Read request and process it in server.
  context().OnStreamReadable();

  // Run client to receive threads response.
  RunClient();
  auto got = response.get();
  EXPECT_FALSE(got.error);
  EXPECT_EQ(got.response.threads.size(), 2u);
}

TEST_F(RequestThreadsTest, ListThreadsZxdbProcessId) {
  InitializeDebugging();

  InjectProcess(kProcessKoid);
  RunClient();
  InjectThread(kProcessKoid, kThreadKoid);
  RunClient();

  dap::ThreadsRequest req;
  dap::ResponseOrError<dap::ThreadsResponseZxdb> got;
  bool response_received = false;

  // Use the low-level |send| function since we are expecting to see the zxdb extended
  // ThreadsResponseZxdb, rather than the standard DAP ThreadsResponse that will be returned if we
  // just sent |req| the normal way. See the registration of ThreadsRequest handler in context.cc
  // for details.
  client().send(dap::TypeOf<dap::ThreadsRequest>::type(),
                dap::TypeOf<dap::ThreadsResponseZxdb>::type(), &req,
                [&](const void* res, const dap::Error* err) {
                  if (err) {
                    got.error = *err;
                  } else {
                    got.response = *static_cast<const dap::ThreadsResponseZxdb*>(res);
                  }
                  response_received = true;
                });

  context().OnStreamReadable();
  RunClient();

  ASSERT_TRUE(response_received);
  ASSERT_FALSE(got.error);
  ASSERT_EQ(got.response.threads.size(), 1u);
  EXPECT_EQ(got.response.threads[0].id, static_cast<dap::integer>(kThreadKoid));
  ASSERT_TRUE(got.response.threads[0].processId.has_value());
  EXPECT_EQ(got.response.threads[0].processId.value(), static_cast<dap::integer>(kProcessKoid));
}

}  // namespace zxdb

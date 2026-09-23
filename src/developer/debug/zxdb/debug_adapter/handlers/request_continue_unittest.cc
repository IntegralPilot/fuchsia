// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/client/thread.h"
#include "src/developer/debug/zxdb/debug_adapter/context_test.h"

namespace zxdb {

namespace {

// Create our own fake RemoteAPI to observe and verify thread resumes.
class FakeRemoteAPI : public RemoteAPI {
 public:
  std::vector<debug_ipc::ResumeRequest>& resume_requests() { return resume_requests_; }

  void Resume(const debug_ipc::ResumeRequest& request,
              fit::callback<void(const Err&, debug_ipc::ResumeReply)> cb) override {
    resume_requests_.push_back(request);
    RemoteAPI::Resume(request, std::move(cb));
  }

 private:
  std::vector<debug_ipc::ResumeRequest> resume_requests_;
};

class RequestContinueTest : public DebugAdapterContextTest {
 public:
  FakeRemoteAPI* remote_api() { return remote_api_; }

 protected:
  std::unique_ptr<RemoteAPI> GetRemoteAPIImpl() override {
    auto remote_api = std::make_unique<FakeRemoteAPI>();
    remote_api_ = remote_api.get();
    return remote_api;
  }

 private:
  FakeRemoteAPI* remote_api_;
};

}  // namespace

TEST_F(RequestContinueTest, ContinueSystem) {
  InitializeDebugging();

  InjectProcess(kProcessKoid);
  InjectProcess(kProcessKoid + 1);
  // Run client to receive process started events.
  RunClient();
  RunClient();
  InjectThread(kProcessKoid, kThreadKoid);
  InjectThread(kProcessKoid, kThreadKoid + 1);
  InjectThread(kProcessKoid + 1, kThreadKoid + 2);
  // Run client to receive thread started events.
  RunClient();
  RunClient();
  RunClient();

  // Send continue request from the client.
  dap::ContinueRequest request = {};
  request.threadId = kThreadKoid;
  auto response = client().send(request);

  // Read request and process it in server.
  context().OnStreamReadable();

  // Verify both threads are resumed.
  auto resume_requests = remote_api()->resume_requests();
  EXPECT_EQ(3u, resume_requests.size());
  EXPECT_EQ(debug_ipc::ResumeRequest::How::kResolveAndContinue, resume_requests[0].how);
  EXPECT_EQ(1u, resume_requests[0].ids.size());
  EXPECT_EQ(kProcessKoid, resume_requests[0].ids[0].process);
  EXPECT_EQ(kThreadKoid, resume_requests[0].ids[0].thread);
  EXPECT_EQ(debug_ipc::ResumeRequest::How::kResolveAndContinue, resume_requests[1].how);
  EXPECT_EQ(1u, resume_requests[1].ids.size());
  EXPECT_EQ(kProcessKoid, resume_requests[1].ids[0].process);
  EXPECT_EQ(kThreadKoid + 1, resume_requests[1].ids[0].thread);
  EXPECT_EQ(debug_ipc::ResumeRequest::How::kResolveAndContinue, resume_requests[2].how);
  EXPECT_EQ(1u, resume_requests[2].ids.size());
  EXPECT_EQ(kProcessKoid + 1, resume_requests[2].ids[0].process);
  EXPECT_EQ(kThreadKoid + 2, resume_requests[2].ids[0].thread);

  // Run client to receive continue response.
  RunClient();
  auto got = response.get();
  EXPECT_FALSE(got.error);
  EXPECT_TRUE(got.response.allThreadsContinued.value(true));
}

TEST_F(RequestContinueTest, ContinueSingleThread) {
  InitializeDebugging();

  InjectProcess(kProcessKoid);
  InjectProcess(kProcessKoid + 1);
  // Run client to receive process started events.
  RunClient();
  RunClient();
  InjectThread(kProcessKoid, kThreadKoid);
  InjectThread(kProcessKoid, kThreadKoid + 1);
  InjectThread(kProcessKoid + 1, kThreadKoid + 2);
  // Run client to receive thread started events.
  RunClient();
  RunClient();
  RunClient();

  // Send continue request from the client.
  dap::ContinueRequest request = {};
  request.threadId = kThreadKoid;
  request.singleThread = true;
  auto response = client().send(request);

  // Read request and process it in server.
  context().OnStreamReadable();

  // Verify the single thread is resumed.
  auto resume_requests = remote_api()->resume_requests();
  EXPECT_EQ(1u, resume_requests.size());
  EXPECT_EQ(debug_ipc::ResumeRequest::How::kResolveAndContinue, resume_requests[0].how);
  EXPECT_EQ(1u, resume_requests[0].ids.size());
  EXPECT_EQ(kProcessKoid, resume_requests[0].ids[0].process);
  EXPECT_EQ(kThreadKoid, resume_requests[0].ids[0].thread);

  // Run client to receive continue response.
  RunClient();
  auto got = response.get();
  EXPECT_FALSE(got.error);
  EXPECT_FALSE(got.response.allThreadsContinued.value(true));
}

}  // namespace zxdb

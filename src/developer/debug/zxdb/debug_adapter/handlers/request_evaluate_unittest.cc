// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/debug_adapter/handlers/request_evaluate.h"

#include <gtest/gtest.h>

#include "src/developer/debug/ipc/records.h"
#include "src/developer/debug/zxdb/client/mock_frame.h"
#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/console/console_context.h"
#include "src/developer/debug/zxdb/debug_adapter/context_test.h"
#include "src/developer/debug/zxdb/symbols/function.h"

namespace zxdb {

namespace {

class RequestEvaluateTest : public DebugAdapterContextTest {
 public:
  void SetUp() override {
    DebugAdapterContextTest::SetUp();
    InitializeDebugging();

    process_ = InjectProcessWithModule(kProcessKoid, 0x1000);
    RunClient();
    thread_ = InjectThread(kProcessKoid, kThreadKoid);
    RunClient();
  }

  dap::ResponseOrError<dap::StackTraceResponse> GetStackTrace(
      std::vector<std::unique_ptr<Frame>> frames) {
    InjectExceptionWithStack(kProcessKoid, kThreadKoid, debug_ipc::ExceptionType::kSingleStep,
                             std::move(frames), true);
    RunClient();

    dap::StackTraceRequest stack_request = {};
    stack_request.threadId = kThreadKoid;
    auto stack_response_fut = client().send(stack_request);

    context().OnStreamReadable();
    loop().RunUntilNoTasks();
    RunClient();
    return stack_response_fut.get();
  }

  Thread* thread() { return thread_; }
  Process* process() { return process_; }

 private:
  Thread* thread_ = nullptr;
  Process* process_ = nullptr;
};

}  // namespace

TEST_F(RequestEvaluateTest, ReplFrameId) {
  fxl::RefPtr<Function> function1(fxl::MakeRefCounted<Function>(DwarfTag::kSubprogram));
  function1->set_assigned_name("test_func1");
  auto location1 = Location(0x10010, FileLine("test_file.cc", 23), 10,
                            SymbolContext::ForRelativeAddresses(), function1);

  fxl::RefPtr<Function> function2(fxl::MakeRefCounted<Function>(DwarfTag::kSubprogram));
  function2->set_assigned_name("test_func2");
  auto location2 = Location(0x10040, FileLine("test_file.cc", 50), 10,
                            SymbolContext::ForRelativeAddresses(), function2);

  std::vector<std::unique_ptr<Frame>> frames;
  frames.push_back(std::make_unique<MockFrame>(&session(), thread(), location1, 0x2000));
  frames.push_back(std::make_unique<MockFrame>(&session(), thread(), location2, 0x2020));

  auto stack_response = GetStackTrace(std::move(frames));
  ASSERT_FALSE(stack_response.error);
  ASSERT_EQ(stack_response.response.stackFrames.size(), 2u);

  dap::integer frame_id_1 = stack_response.response.stackFrames[1].id;
  ASSERT_NE(frame_id_1, 0);

  EXPECT_EQ(context().console()->context().GetActiveFrameIdForThread(thread()), 0);

  dap::EvaluateRequest req;
  req.context = "repl";
  req.frameId = frame_id_1;
  req.expression = "frame";
  auto response = client().send(req);

  context().OnStreamReadable();
  loop().RunUntilNoTasks();
  RunClient();

  auto got = response.get();
  EXPECT_FALSE(got.error);
  EXPECT_EQ(context().console()->context().GetActiveFrameIdForThread(thread()), 1);
  EXPECT_EQ(context().console()->context().GetActiveTarget(), process()->GetTarget());
}

TEST_F(RequestEvaluateTest, InvalidFrameId) {
  dap::EvaluateRequest req;
  req.context = "repl";
  req.frameId = 9999;
  req.expression = "frame";
  auto response = client().send(req);

  context().OnStreamReadable();
  loop().RunUntilNoTasks();
  RunClient();

  auto got = response.get();
  EXPECT_TRUE(got.error);
  EXPECT_EQ(got.error.message, "Invalid frame ID");
}

}  // namespace zxdb

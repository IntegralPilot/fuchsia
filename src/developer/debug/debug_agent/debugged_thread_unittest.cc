// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/debugged_thread.h"

#include <memory>

#include <gtest/gtest.h>

#include "src/developer/debug/debug_agent/arch.h"
#include "src/developer/debug/debug_agent/debugged_process.h"
#include "src/developer/debug/debug_agent/mock_debug_agent_harness.h"
#include "src/developer/debug/debug_agent/mock_exception_handle.h"
#include "src/developer/debug/debug_agent/mock_process.h"
#include "src/developer/debug/debug_agent/mock_process_handle.h"
#include "src/developer/debug/debug_agent/mock_thread.h"
#include "src/developer/debug/debug_agent/mock_thread_handle.h"
#include "src/developer/debug/debug_agent/remote_api.h"
#include "src/developer/debug/ipc/protocol.h"
#include "src/developer/debug/ipc/records.h"

namespace debug_agent {

using namespace debug_ipc;

namespace {

TEST(DebuggedThread, Resume) {
  MockDebugAgentHarness harness;

  constexpr zx_koid_t kProcessKoid = 0x8723456;
  MockProcess process(harness.debug_agent(), kProcessKoid);

  constexpr zx_koid_t kThreadKoid = 0x8723457;
  MockThread* thread = process.AddThread(kThreadKoid);
  EXPECT_FALSE(thread->in_exception());

  ExceptionHandle::Resolution resolution = ExceptionHandle::Resolution::kTryNext;
  debug_ipc::ExceptionStrategy exception_strategy = debug_ipc::ExceptionStrategy::kNone;
  auto exception = std::make_unique<MockExceptionHandle>(
      [&resolution](ExceptionHandle::Resolution new_res) { resolution = new_res; },
      [&exception_strategy](debug_ipc::ExceptionStrategy new_strategy) {
        exception_strategy = new_strategy;
      });
  thread->set_exception_handle(std::move(exception));
  EXPECT_TRUE(thread->in_exception());
  thread->ClientResume(
      debug_ipc::ResumeRequest{.how = debug_ipc::ResumeRequest::How::kResolveAndContinue});
  EXPECT_FALSE(thread->in_exception());
  EXPECT_EQ(resolution, ExceptionHandle::Resolution::kHandled);
  EXPECT_EQ(exception_strategy, debug_ipc::ExceptionStrategy::kNone);

  resolution = ExceptionHandle::Resolution::kTryNext;
  exception_strategy = debug_ipc::ExceptionStrategy::kNone;
  exception = std::make_unique<MockExceptionHandle>(
      [&resolution](ExceptionHandle::Resolution new_res) { resolution = new_res; },
      [&exception_strategy](debug_ipc::ExceptionStrategy new_strategy) {
        exception_strategy = new_strategy;
      });
  thread->set_exception_handle(std::move(exception));
  EXPECT_TRUE(thread->in_exception());
  thread->ClientResume(
      debug_ipc::ResumeRequest{.how = debug_ipc::ResumeRequest::How::kForwardAndContinue});
  EXPECT_FALSE(thread->in_exception());
  EXPECT_EQ(resolution, ExceptionHandle::Resolution::kTryNext);
  EXPECT_EQ(exception_strategy, debug_ipc::ExceptionStrategy::kSecondChance);
}

TEST(DebuggedThread, SingleStepForwardsException) {
  MockDebugAgentHarness harness;

  constexpr zx_koid_t kProcessKoid = 0x8723456;
  MockProcess process(harness.debug_agent(), kProcessKoid);

  constexpr zx_koid_t kThreadKoid = 0x8723457;
  MockThread* thread = process.AddThread(kThreadKoid);
  EXPECT_FALSE(thread->in_exception());

  // Set an exception.
  auto resolution = ExceptionHandle::Resolution::kTryNext;
  auto exception_strategy = debug_ipc::ExceptionStrategy::kNone;
  auto exception = std::make_unique<MockExceptionHandle>(
      [&resolution](ExceptionHandle::Resolution new_res) { resolution = new_res; },
      [&exception_strategy](debug_ipc::ExceptionStrategy new_strategy) {
        exception_strategy = new_strategy;
      });
  exception->set_type(debug_ipc::ExceptionType::kUndefinedInstruction);
  exception->SetStrategy(debug_ipc::ExceptionStrategy::kFirstChance);

  thread->OnException(std::move(exception));

  // We shouldn't release the exception.
  EXPECT_TRUE(thread->in_exception());
  EXPECT_EQ(thread->exception_handle()->GetType(thread->thread_handle()),
            debug_ipc::ExceptionType::kUndefinedInstruction);
  EXPECT_EQ(thread->exception_handle()->GetStrategy(), debug_ipc::ExceptionStrategy::kFirstChance);
  EXPECT_TRUE(thread->exception_handle()->GetResolution().is_ok());
  EXPECT_EQ(*thread->exception_handle()->GetResolution(), ExceptionHandle::Resolution::kTryNext);

  // The notification should be sent.
  ASSERT_EQ(harness.stream_backend()->exceptions().size(), 1u);
  EXPECT_EQ(harness.stream_backend()->exceptions()[0].type,
            debug_ipc::ExceptionType::kUndefinedInstruction);
  EXPECT_EQ(harness.stream_backend()->exceptions()[0].exception.strategy,
            debug_ipc::ExceptionStrategy::kFirstChance);

  // Now try to single step over the exception, which will forward the exception and mark it for
  // second chance handling.
  debug_ipc::ResumeRequest step_request;
  step_request.how = debug_ipc::ResumeRequest::How::kStepInstruction;
  thread->ClientResume(step_request);

  // The exception object should be released now, but we should have set the strategy to second
  // chance first.
  EXPECT_EQ(thread->exception_handle(), nullptr);
  EXPECT_EQ(debug_ipc::ExceptionStrategy::kSecondChance, exception_strategy);
  EXPECT_EQ(ExceptionHandle::Resolution::kTryNext, resolution);

  auto second_chance_exception = std::make_unique<MockExceptionHandle>(
      [&resolution](ExceptionHandle::Resolution new_res) { resolution = new_res; },
      [&exception_strategy](debug_ipc::ExceptionStrategy new_strategy) {
        exception_strategy = new_strategy;
      });
  second_chance_exception->set_type(debug_ipc::ExceptionType::kUndefinedInstruction);
  second_chance_exception->SetStrategy(exception_strategy);

  // Now dispatch the second chance exception, simulating a process that doesn't handle its own
  // exceptions.
  thread->OnException(std::move(second_chance_exception));

  // We should have an exception again, but this one should be second chance.
  EXPECT_TRUE(thread->in_exception());
  EXPECT_EQ(thread->exception_handle()->GetType(thread->thread_handle()),
            debug_ipc::ExceptionType::kUndefinedInstruction);
  EXPECT_EQ(thread->exception_handle()->GetStrategy(), debug_ipc::ExceptionStrategy::kSecondChance);
  EXPECT_TRUE(thread->exception_handle()->GetResolution().is_ok());
  EXPECT_EQ(*thread->exception_handle()->GetResolution(), ExceptionHandle::Resolution::kTryNext);

  ASSERT_EQ(harness.stream_backend()->exceptions().size(), 2u);
  EXPECT_EQ(harness.stream_backend()->exceptions()[1].type,
            debug_ipc::ExceptionType::kUndefinedInstruction);
  EXPECT_EQ(harness.stream_backend()->exceptions()[1].exception.strategy,
            debug_ipc::ExceptionStrategy::kSecondChance);

  // Step again.
  thread->ClientResume(step_request);

  // In a real system, this would mean the exception is forwarded up the job tree and would
  // eventually get killed by the root job. For our purposes, we just need to be sure that we
  // released it for the final time.
  EXPECT_FALSE(thread->in_exception());
  // We should not have marked the exception as handled.
  EXPECT_EQ(ExceptionHandle::Resolution::kTryNext, resolution);
}

TEST(DebuggedThread, OnException) {
  MockDebugAgentHarness harness;
  RemoteAPI* remote_api = harness.debug_agent();

  constexpr zx_koid_t kProcessKoid = 0x8723456;
  MockProcess process(harness.debug_agent(), kProcessKoid);

  constexpr zx_koid_t kThreadKoid = 0x8723457;
  MockThread* thread = process.AddThread(kThreadKoid);
  EXPECT_FALSE(thread->in_exception());

  // Policy: general exceptions initially handled as first-chance.
  // Exception: general, first-chance.
  // Expected: no applied strategy.
  {
    debug_ipc::ExceptionStrategy applied_strategy = debug_ipc::ExceptionStrategy::kNone;
    auto exception = std::make_unique<MockExceptionHandle>(
        [](ExceptionHandle::Resolution) {},
        [&applied_strategy](debug_ipc::ExceptionStrategy new_strategy) {
          applied_strategy = new_strategy;
        });
    exception->set_type(debug_ipc::ExceptionType::kGeneral);
    exception->SetStrategy(debug_ipc::ExceptionStrategy::kFirstChance);
    applied_strategy = debug_ipc::ExceptionStrategy::kNone;  // Clear previously set.

    thread->OnException(std::move(exception));

    EXPECT_EQ(debug_ipc::ExceptionStrategy::kNone, applied_strategy);
  }

  // Policy: general exceptions initially handled as first-chance.
  // Exception: general, second-chance.
  // Expected: no applied strategy (as this isn't our initial handling).
  {
    debug_ipc::ExceptionStrategy applied_strategy = debug_ipc::ExceptionStrategy::kNone;
    auto exception = std::make_unique<MockExceptionHandle>(
        [](ExceptionHandle::Resolution) {},
        [&applied_strategy](debug_ipc::ExceptionStrategy new_strategy) {
          applied_strategy = new_strategy;
        });
    exception->set_type(debug_ipc::ExceptionType::kGeneral);
    exception->SetStrategy(debug_ipc::ExceptionStrategy::kSecondChance);
    applied_strategy = debug_ipc::ExceptionStrategy::kNone;  // Clear previously set.

    thread->OnException(std::move(exception));

    EXPECT_EQ(debug_ipc::ExceptionStrategy::kNone, applied_strategy);
  }

  // Update policy so that general exceptions are handled initially as
  // second-chance.
  const debug_ipc::UpdateGlobalSettingsRequest request = {
      .exception_strategies =
          {
              {
                  .type = debug_ipc::ExceptionType::kGeneral,
                  .value = debug_ipc::ExceptionStrategy::kSecondChance,
              },
          },
  };
  debug_ipc::UpdateGlobalSettingsReply reply;
  remote_api->OnUpdateGlobalSettings(request, &reply);
  EXPECT_TRUE(reply.status.ok());

  // Policy: general exceptions initially handled as second-chance.
  // Exception: general, first-chance.
  // Expected: applied strategy of second-chance.
  {
    debug_ipc::ExceptionStrategy applied_strategy = debug_ipc::ExceptionStrategy::kNone;
    auto exception = std::make_unique<MockExceptionHandle>(
        [](ExceptionHandle::Resolution) {},
        [&applied_strategy](debug_ipc::ExceptionStrategy new_strategy) {
          applied_strategy = new_strategy;
        });
    exception->set_type(debug_ipc::ExceptionType::kGeneral);
    exception->SetStrategy(debug_ipc::ExceptionStrategy::kFirstChance);
    applied_strategy = debug_ipc::ExceptionStrategy::kNone;  // Clear previously set.

    thread->OnException(std::move(exception));

    EXPECT_EQ(debug_ipc::ExceptionStrategy::kSecondChance, applied_strategy);
    // Since we didn't handle the exception, we expect it to have been closed.
    EXPECT_EQ(nullptr, thread->exception_handle());
  }

  // Policy: general exceptions initially handled as second-chance.
  // Exception: general, second-chance.
  // Expected: no applied strategy.
  {
    debug_ipc::ExceptionStrategy applied_strategy = debug_ipc::ExceptionStrategy::kNone;
    auto exception = std::make_unique<MockExceptionHandle>(
        [](ExceptionHandle::Resolution) {},
        [&applied_strategy](debug_ipc::ExceptionStrategy new_strategy) {
          applied_strategy = new_strategy;
        });
    exception->set_type(debug_ipc::ExceptionType::kGeneral);
    exception->SetStrategy(debug_ipc::ExceptionStrategy::kSecondChance);
    applied_strategy = debug_ipc::ExceptionStrategy::kNone;  // Clear previously set.

    thread->OnException(std::move(exception));

    EXPECT_EQ(debug_ipc::ExceptionStrategy::kNone, applied_strategy);
  }
}

}  // namespace

}  // namespace debug_agent

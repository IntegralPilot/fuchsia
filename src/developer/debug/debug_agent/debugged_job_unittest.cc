// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/debugged_job.h"

#include <gtest/gtest.h>

#include "src/developer/debug/debug_agent/mock_debug_agent_harness.h"
#include "src/developer/debug/debug_agent/mock_process.h"
#include "src/developer/debug/debug_agent/mock_thread.h"

namespace debug_agent {
TEST(DebuggedJob, ReportUnhandledException) {
  debug_agent::MockDebugAgentHarness harness;
  DebuggedJob job(harness.debug_agent());

  constexpr zx_koid_t kJobKoid = 1234;
  auto mock_job_handle = std::make_unique<MockJobHandle>(kJobKoid);

  constexpr zx_koid_t kProcess1Koid = 1235;
  constexpr zx_koid_t kProcess1Thread1Koid = 1236;
  constexpr zx_koid_t kProcess2Koid = 1237;
  constexpr zx_koid_t kProcess2Thread1Koid = 1238;
  constexpr zx_koid_t kProcess2Thread2Koid = 1239;

  MockProcess* p1 = harness.AddProcess(kProcess1Koid);
  auto p1_handle = p1->mock_process_handle();

  auto p1_t1 = p1->AddThread(kProcess1Thread1Koid);
  p1_handle.set_threads({p1_t1->mock_thread_handle()});

  MockProcess* p2 = harness.AddProcess(kProcess2Koid);
  auto p2_t1 = p2->AddThread(kProcess2Thread1Koid);
  auto p2_t2 = p2->AddThread(kProcess2Thread2Koid);
  auto p2_handle = p2->mock_process_handle();
  p2_handle.set_threads({p2_t1->mock_thread_handle(), p2_t2->mock_thread_handle()});

  mock_job_handle->set_child_processes({p1_handle, p2_handle});

  MockJobHandle* job_handle = mock_job_handle.get();

  DebuggedJobCreateInfo create_info(std::move(mock_job_handle));
  // This tells the MockJobHandle that we are allowed to receive exception events, mimicking a real
  // system.
  create_info.priority = debug_ipc::AttachConfig::Priority::kStrong;
  job.Init(std::move(create_info));

  auto mock_exception = std::make_unique<MockExceptionHandle>(kProcess1Koid, kProcess1Thread1Koid);

  job_handle->OnException(std::move(mock_exception), MockJobExceptionInfo::kException);

  ASSERT_FALSE(harness.stream_backend()->exceptions().empty());
  ASSERT_EQ(harness.stream_backend()->exceptions().size(), 1u);

  // This exception should be marked that it came from the job and there's nothing to do other than
  // display the exception and move on.
  EXPECT_TRUE(harness.stream_backend()->exceptions()[0].job_only);

  // Other fields in the exception notification should always be empty.
  EXPECT_TRUE(harness.stream_backend()->exceptions()[0].hit_breakpoints.empty());
  EXPECT_TRUE(harness.stream_backend()->exceptions()[0].other_affected_threads.empty());

  // The exception should be released after the notification is sent.
  EXPECT_EQ(mock_exception, nullptr);
}

TEST(DebuggedJob, Init_AttachConfig) {
  debug_agent::MockDebugAgentHarness harness;
  DebuggedJob job(harness.debug_agent());

  constexpr zx_koid_t kJobKoid = 1234;
  auto mock_job_handle = std::make_unique<MockJobHandle>(kJobKoid);
  MockJobHandle* handle_ptr = mock_job_handle.get();

  DebuggedJobCreateInfo create_info(std::move(mock_job_handle));
  create_info.priority = debug_ipc::AttachConfig::Priority::kMinimal;
  ASSERT_TRUE(job.Init(std::move(create_info)).ok());

  EXPECT_EQ(job.type(), JobExceptionChannelType::kNone);
  EXPECT_EQ(handle_ptr->observer(), &job);
  EXPECT_EQ(handle_ptr->observer_type(), JobExceptionChannelType::kNone);

  // Test kWeak priority.
  auto mock_job_handle_weak = std::make_unique<MockJobHandle>(kJobKoid);
  MockJobHandle* handle_weak_ptr = mock_job_handle_weak.get();
  DebuggedJob job_weak(harness.debug_agent());
  DebuggedJobCreateInfo create_info_weak(std::move(mock_job_handle_weak));
  create_info_weak.priority = debug_ipc::AttachConfig::Priority::kWeak;
  ASSERT_TRUE(job_weak.Init(std::move(create_info_weak)).ok());
  EXPECT_EQ(job_weak.type(), JobExceptionChannelType::kDebugger);
  EXPECT_EQ(handle_weak_ptr->observer(), &job_weak);
  EXPECT_EQ(handle_weak_ptr->observer_type(), JobExceptionChannelType::kDebugger);

  // Test kStrong priority.
  auto mock_job_handle_strong = std::make_unique<MockJobHandle>(kJobKoid);
  MockJobHandle* handle_strong_ptr = mock_job_handle_strong.get();
  DebuggedJob job_strong(harness.debug_agent());
  DebuggedJobCreateInfo create_info_strong(std::move(mock_job_handle_strong));
  create_info_strong.priority = debug_ipc::AttachConfig::Priority::kStrong;
  ASSERT_TRUE(job_strong.Init(std::move(create_info_strong)).ok());
  EXPECT_EQ(job_strong.type(), JobExceptionChannelType::kException);
  EXPECT_EQ(handle_strong_ptr->observer(), &job_strong);
  EXPECT_EQ(handle_strong_ptr->observer_type(), JobExceptionChannelType::kException);
}

namespace {
class DestructorNotifierExceptionHandle : public MockExceptionHandle {
 public:
  DestructorNotifierExceptionHandle(uint64_t process_koid, uint64_t thread_koid, bool* destroyed)
      : MockExceptionHandle(process_koid, thread_koid), destroyed_(destroyed) {}
  ~DestructorNotifierExceptionHandle() override { *destroyed_ = true; }

 private:
  bool* destroyed_;
};

class LifetimeCheckingProcessHandle : public MockProcessHandle {
 public:
  LifetimeCheckingProcessHandle(zx_koid_t process_koid, const bool* exception_destroyed)
      : MockProcessHandle(process_koid), exception_destroyed_(exception_destroyed) {}

  zx_koid_t GetKoid() const override {
    EXPECT_FALSE(*exception_destroyed_);
    return MockProcessHandle::GetKoid();
  }

 private:
  const bool* exception_destroyed_;
};
}  // namespace

TEST(DebuggedJob, OnProcessStarting_ExceptionLifetime) {
  MockDebugAgentHarness harness;
  DebuggedJob job(harness.debug_agent());

  constexpr zx_koid_t kJobKoid = 1234;
  auto mock_job_handle = std::make_unique<MockJobHandle>(kJobKoid);
  DebuggedJobCreateInfo create_info(std::move(mock_job_handle));
  create_info.priority = debug_ipc::AttachConfig::Priority::kStrong;
  ASSERT_TRUE(job.Init(std::move(create_info)).ok());

  bool exception_destroyed = false;
  constexpr zx_koid_t kProcessKoid = 5678;
  constexpr zx_koid_t kThreadKoid = 5679;

  auto process =
      std::make_unique<LifetimeCheckingProcessHandle>(kProcessKoid, &exception_destroyed);
  auto exception = std::make_unique<DestructorNotifierExceptionHandle>(kProcessKoid, kThreadKoid,
                                                                       &exception_destroyed);

  static_cast<JobExceptionObserver&>(job).OnProcessStarting(std::move(process),
                                                            std::move(exception));

  EXPECT_TRUE(exception_destroyed);
}

TEST(DebuggedJob, OnProcessNameChanged_ExceptionLifetime) {
  MockDebugAgentHarness harness;
  DebuggedJob job(harness.debug_agent());

  constexpr zx_koid_t kJobKoid = 1234;
  auto mock_job_handle = std::make_unique<MockJobHandle>(kJobKoid);
  DebuggedJobCreateInfo create_info(std::move(mock_job_handle));
  create_info.priority = debug_ipc::AttachConfig::Priority::kStrong;
  ASSERT_TRUE(job.Init(std::move(create_info)).ok());

  bool exception_destroyed = false;
  constexpr zx_koid_t kProcessKoid = 5678;
  constexpr zx_koid_t kThreadKoid = 5679;

  auto process =
      std::make_unique<LifetimeCheckingProcessHandle>(kProcessKoid, &exception_destroyed);
  auto exception = std::make_unique<DestructorNotifierExceptionHandle>(kProcessKoid, kThreadKoid,
                                                                       &exception_destroyed);

  static_cast<JobExceptionObserver&>(job).OnProcessNameChanged(std::move(process),
                                                               std::move(exception));

  EXPECT_TRUE(exception_destroyed);
}

}  // namespace debug_agent

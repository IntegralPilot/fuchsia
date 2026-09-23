// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/zircon_job_handle.h"

#include <lib/zx/channel.h>
#include <lib/zx/job.h>

#include <gtest/gtest.h>

#include "src/developer/debug/debug_agent/exception_handle.h"
#include "src/developer/debug/debug_agent/job_exception_observer.h"
#include "src/developer/debug/debug_agent/process_handle.h"
#include "src/developer/debug/shared/test_with_loop.h"

namespace debug_agent {

namespace {

class ZirconJobHandleTest : public debug::TestWithLoop, public JobExceptionObserver {
 public:
  // JobExceptionObserver implementation.
  void OnProcessStarting(std::unique_ptr<ProcessHandle> process,
                         std::unique_ptr<ExceptionHandle> exception) override {}
  void OnProcessNameChanged(std::unique_ptr<ProcessHandle> process,
                            std::unique_ptr<ExceptionHandle> exception) override {}
  void OnUnhandledException(std::unique_ptr<ExceptionHandle> exception) override {}
};

}  // namespace

TEST_F(ZirconJobHandleTest, Attach_DowngradeAndUnregister) {
  zx::job child_job;
  ASSERT_EQ(zx::job::create(*zx::job::default_job(), 0, &child_job), ZX_OK);

  // Duplicate the job handle so we can test exception channel availability directly.
  zx::job child_job_copy;
  ASSERT_EQ(child_job.duplicate(ZX_RIGHT_SAME_RIGHTS, &child_job_copy), ZX_OK);

  ZirconJobHandle job_handle(std::move(child_job));

  JobHandle::AttachConfig config_exc;
  config_exc.exception_channel_type = JobExceptionChannelType::kException;
  EXPECT_TRUE(job_handle.Attach(this, config_exc).ok());

  // Verify that the exception channel is bound (creating another returns ALREADY_BOUND).
  zx::channel test_ch;
  EXPECT_EQ(child_job_copy.create_exception_channel(0, &test_ch), ZX_ERR_ALREADY_BOUND);

  JobHandle::AttachConfig config_none;
  config_none.exception_channel_type = JobExceptionChannelType::kNone;
  EXPECT_TRUE(job_handle.Attach(this, config_none).ok());

  // Verify that the exception channel was released.
  EXPECT_EQ(child_job_copy.create_exception_channel(0, &test_ch), ZX_OK);
  test_ch.reset();

  EXPECT_TRUE(job_handle.Attach(this, config_exc).ok());
  EXPECT_EQ(child_job_copy.create_exception_channel(0, &test_ch), ZX_ERR_ALREADY_BOUND);

  EXPECT_TRUE(job_handle.Attach(nullptr, config_exc).ok());
  EXPECT_EQ(child_job_copy.create_exception_channel(0, &test_ch), ZX_OK);
}

TEST_F(ZirconJobHandleTest, Attach_SwitchChannelType) {
  zx::job child_job;
  ASSERT_EQ(zx::job::create(*zx::job::default_job(), 0, &child_job), ZX_OK);

  // Duplicate the job handle so we can test exception channel availability directly.
  zx::job child_job_copy;
  ASSERT_EQ(child_job.duplicate(ZX_RIGHT_SAME_RIGHTS, &child_job_copy), ZX_OK);

  ZirconJobHandle job_handle(std::move(child_job));

  JobHandle::AttachConfig config_exc;
  config_exc.exception_channel_type = JobExceptionChannelType::kException;
  EXPECT_TRUE(job_handle.Attach(this, config_exc).ok());

  // Verify that the exception channel is bound (creating another returns ALREADY_BOUND).
  zx::channel test_ch;
  EXPECT_EQ(child_job_copy.create_exception_channel(0, &test_ch), ZX_ERR_ALREADY_BOUND);

  JobHandle::AttachConfig config_dbg;
  config_dbg.exception_channel_type = JobExceptionChannelType::kDebugger;
  EXPECT_TRUE(job_handle.Attach(this, config_dbg).ok());

  // Verify that the non-debugger exception channel was released.
  EXPECT_EQ(child_job_copy.create_exception_channel(0, &test_ch), ZX_OK);
  test_ch.reset();

  EXPECT_TRUE(job_handle.Attach(nullptr, config_exc).ok());
}

}  // namespace debug_agent

// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zx/exception.h>
#include <lib/zx/job.h>
#include <lib/zx/process.h>
#include <zircon/errors.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/exception.h>

#include <thread>

#include <zxtest/zxtest.h>

namespace {

TEST(UserExceptionsTest, InvalidArgs) {
  zx_exception_context_t context = {};
  zx_status_t status = zx_thread_raise_exception(0, ZX_EXCP_USER, &context);
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, status);
  status = zx_thread_raise_exception(ZX_EXCEPTION_TARGET_JOB_DEBUGGER, ZX_EXCP_THREAD_STARTING,
                                     &context);
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, status);
  status = zx_thread_raise_exception(ZX_EXCEPTION_TARGET_JOB_DEBUGGER, ZX_EXCP_USER, nullptr);
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, status);
}

TEST(UserExceptionsTest, NonFatal) {
  zx_exception_context_t context = {};
  EXPECT_OK(zx_thread_raise_exception(ZX_EXCEPTION_TARGET_JOB_DEBUGGER, ZX_EXCP_USER, &context));
}

TEST(UserExceptionsTest, Delivered) {
  zx::channel exception_channel;
  ASSERT_OK(zx::job::default_job()->create_exception_channel(ZX_EXCEPTION_CHANNEL_DEBUGGER,
                                                             &exception_channel));

  zx_status_t raise_result = ZX_ERR_INTERNAL;
  auto raise = std::thread([&raise_result]() {
    zx_exception_context_t context = {};
    context.synth_code = ZX_EXCP_USER_CODE_USER0;
    context.synth_data = 42;
    raise_result =
        zx_thread_raise_exception(ZX_EXCEPTION_TARGET_JOB_DEBUGGER, ZX_EXCP_USER, &context);
  });

  zx_signals_t pending;
  ASSERT_OK(exception_channel.wait_one(ZX_CHANNEL_READABLE | ZX_CHANNEL_PEER_CLOSED,
                                       zx::time::infinite(), &pending));
  ASSERT_NE(0, pending & ZX_CHANNEL_READABLE, "exception channel peer closed (pending 0x%08x)",
            pending);

  zx::exception exception;
  zx_exception_info_t exception_info = {};
  uint32_t byte_count = 0;
  uint32_t handle_count = 0;
  ASSERT_OK(exception_channel.read(0, &exception_info, exception.reset_and_get_address(),
                                   sizeof(exception_info), 1, &byte_count, &handle_count));
  ASSERT_EQ(sizeof(exception_info), byte_count);
  ASSERT_EQ(1, handle_count);
  ASSERT_EQ(ZX_EXCP_USER, exception_info.type);

  zx::thread thread;
  ASSERT_OK(exception.get_thread(&thread));

  zx_exception_report_t report = {};
  ASSERT_OK(
      thread.get_info(ZX_INFO_THREAD_EXCEPTION_REPORT, &report, sizeof(report), nullptr, nullptr));

  ASSERT_EQ(ZX_EXCP_USER_CODE_USER0, report.context.synth_code);
  ASSERT_EQ(42, report.context.synth_data);

  uint32_t value = ZX_EXCEPTION_STATE_HANDLED;
  ASSERT_OK(exception.set_property(ZX_PROP_EXCEPTION_STATE, &value, sizeof(value)));

  exception.reset();
  exception_channel.reset();

  raise.join();
  ASSERT_OK(raise_result);
}

}  // namespace

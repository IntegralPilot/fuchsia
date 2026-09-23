// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/test-exceptions/exception-catcher.h>
#include <lib/zx/process.h>
#include <lib/zx/thread.h>
#include <zircon/errors.h>
#include <zircon/syscalls/exception.h>
#include <zircon/syscalls/object.h>

#include <zxtest/zxtest.h>

namespace test_exceptions {

namespace {

// Helper to easily create and kill threads to reduce boilerplate.
class TestThread {
 public:
  TestThread() {
    EXPECT_OK(zx::thread::create(*zx::process::self(), "test", strlen("test"), 0, &thread_));
  }

  const zx::thread& get() const { return thread_; }

  zx_status_t StartAndCrash() {
    // Passing 0 for sp and pc crashes the thread immediately.
    return thread_.start(0, 0);
  }

  // Blocks until the thread is in an exception.
  zx_status_t WaitUntilInException() {
    while (1) {
      zx_info_thread_t info;
      zx_status_t status = thread_.get_info(ZX_INFO_THREAD, &info, sizeof(info), nullptr, nullptr);
      if (status != ZX_OK) {
        return status;
      }

      if (info.wait_exception_channel_type == ZX_EXCEPTION_CHANNEL_TYPE_NONE) {
        zx::nanosleep(zx::deadline_after(zx::msec(1)));
      } else {
        return ZX_OK;
      }
    }
  }

 private:
  zx::thread thread_;
};

void ExitFromException(zx::exception exception) {
  constexpr uint32_t kExit = ZX_EXCEPTION_STATE_THREAD_EXIT;
  ASSERT_OK(exception.set_property(ZX_PROP_EXCEPTION_STATE, &kExit, sizeof(kExit)));
  zx::thread thread;
  ASSERT_OK(exception.get_thread(&thread));
  exception.reset();
  ASSERT_OK(thread.wait_one(ZX_THREAD_TERMINATED, zx::time::infinite(), nullptr));
}

TEST(ExceptionCatcher, NoExceptions) {
  TestThread thread;

  ExceptionCatcher catcher(thread.get());
}

TEST(ExceptionCatcher, NoExceptionsManualStartStop) {
  TestThread thread;

  ExceptionCatcher catcher;
  EXPECT_OK(catcher.Start(thread.get()));
  EXPECT_OK(catcher.Stop());
}

TEST(ExceptionCatcher, MultipleStartFailure) {
  TestThread thread, thread2;

  ExceptionCatcher catcher;
  EXPECT_OK(catcher.Start(thread.get()));
  EXPECT_NOT_OK(catcher.Start(thread2.get()));
}

TEST(ExceptionCatcher, ChannelInUseFailure) {
  TestThread thread;

  ExceptionCatcher catcher, catcher2;
  EXPECT_OK(catcher.Start(thread.get()));
  EXPECT_NOT_OK(catcher2.Start(thread.get()));
}

TEST(ExceptionCatcher, CatchException) {
  TestThread thread;
  ExceptionCatcher catcher(thread.get());

  ASSERT_OK(thread.StartAndCrash());
  zx::result<zx::exception> result = catcher.ExpectException();
  ASSERT_TRUE(result.is_ok());
  ASSERT_NO_FATAL_FAILURE(ExitFromException(*std::move(result)));
}

TEST(ExceptionCatcher, CatchThreadException) {
  TestThread thread;
  ExceptionCatcher catcher(thread.get());

  ASSERT_OK(thread.StartAndCrash());
  zx::result<zx::exception> result = catcher.ExpectException(thread.get());
  ASSERT_TRUE(result.is_ok());
  ASSERT_NO_FATAL_FAILURE(ExitFromException(*std::move(result)));
}

TEST(ExceptionCatcher, CatchProcessException) {
  TestThread thread;
  ExceptionCatcher catcher(thread.get());

  ASSERT_OK(thread.StartAndCrash());
  zx::result<zx::exception> result = catcher.ExpectException(*zx::process::self());
  ASSERT_TRUE(result.is_ok());
  ASSERT_NO_FATAL_FAILURE(ExitFromException(*std::move(result)));
}

TEST(ExceptionCatcher, CatchMultipleExceptions) {
  ExceptionCatcher catcher(*zx::process::self());

  TestThread threads[4];
  for (auto& thread : threads) {
    ASSERT_OK(thread.StartAndCrash());
    ASSERT_OK(thread.WaitUntilInException());
  }

  for ([[maybe_unused]] auto& thread : threads) {
    zx::result<zx::exception> result = catcher.ExpectException();
    ASSERT_TRUE(result.is_ok());
    ASSERT_NO_FATAL_FAILURE(ExitFromException(*std::move(result)));
  }
}

TEST(ExceptionCatcher, CatchMultipleThreadExceptions) {
  ExceptionCatcher catcher(*zx::process::self());

  TestThread threads[4];
  for (auto& thread : threads) {
    ASSERT_OK(thread.StartAndCrash());
    ASSERT_OK(thread.WaitUntilInException());
  }

  for (auto& thread : threads) {
    zx::result<zx::exception> result = catcher.ExpectException(thread.get());
    ASSERT_TRUE(result.is_ok());
    ASSERT_NO_FATAL_FAILURE(ExitFromException(*std::move(result)));
  }
}

TEST(ExceptionCatcher, CatchMultipleProcessExceptions) {
  ExceptionCatcher catcher(*zx::process::self());

  TestThread threads[4];
  for (auto& thread : threads) {
    ASSERT_OK(thread.StartAndCrash());
    ASSERT_OK(thread.WaitUntilInException());
  }

  for ([[maybe_unused]] auto& thread : threads) {
    zx::result<zx::exception> result = catcher.ExpectException(*zx::process::self());
    ASSERT_TRUE(result.is_ok());
    ASSERT_NO_FATAL_FAILURE(ExitFromException(*std::move(result)));
  }
}

TEST(ExceptionCatcher, CatchMultipleThreadExceptionsAnyOrder) {
  ExceptionCatcher catcher(*zx::process::self());

  TestThread threads[4];
  for (auto& thread : threads) {
    ASSERT_OK(thread.StartAndCrash());
    ASSERT_OK(thread.WaitUntilInException());
  }

  {
    zx::result<zx::exception> result = catcher.ExpectException(threads[1].get());
    ASSERT_TRUE(result.is_ok());
    ASSERT_NO_FATAL_FAILURE(ExitFromException(*std::move(result)));
  }

  {
    zx::result<zx::exception> result = catcher.ExpectException(threads[3].get());
    ASSERT_TRUE(result.is_ok());
    ASSERT_NO_FATAL_FAILURE(ExitFromException(*std::move(result)));
  }

  {
    zx::result<zx::exception> result = catcher.ExpectException(threads[0].get());
    ASSERT_TRUE(result.is_ok());
    ASSERT_NO_FATAL_FAILURE(ExitFromException(*std::move(result)));
  }

  {
    zx::result<zx::exception> result = catcher.ExpectException(threads[2].get());
    ASSERT_TRUE(result.is_ok());
    ASSERT_NO_FATAL_FAILURE(ExitFromException(*std::move(result)));
  }
}

TEST(ExceptionCatcher, UncaughtExceptionFailure) {
  // Catch the exception again at the process level so it doesn't filter
  // up to the system crash handler and kill our whole process.
  ExceptionCatcher process_catcher(*zx::process::self());

  TestThread thread;
  ExceptionCatcher catcher(thread.get());
  ASSERT_OK(thread.StartAndCrash());
  ASSERT_OK(thread.WaitUntilInException());

  EXPECT_EQ(ZX_ERR_CANCELED, catcher.Stop());

  zx::result<zx::exception> result = process_catcher.ExpectException(thread.get());
  ASSERT_TRUE(result.is_ok());
  ASSERT_NO_FATAL_FAILURE(ExitFromException(*std::move(result)));
}

TEST(ExceptionCatcher, ThreadTerminatedFailure) {
  TestThread thread;
  ExceptionCatcher catcher(thread.get());
  ASSERT_OK(thread.StartAndCrash());
  {
    zx::result<zx::exception> result = catcher.ExpectException(thread.get());
    ASSERT_TRUE(result.is_ok());
    ASSERT_NO_FATAL_FAILURE(ExitFromException(*std::move(result)));
  }

  zx::result<zx::exception> result = catcher.ExpectException(thread.get());
  ASSERT_EQ(result.status_value(), ZX_ERR_PEER_CLOSED);
}

}  // namespace

}  // namespace test_exceptions

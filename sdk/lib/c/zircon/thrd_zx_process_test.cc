// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fit/defer.h>
#include <pthread.h>
#include <zircon/process.h>
#include <zircon/threads.h>

#include <zxtest/zxtest.h>

TEST(ThrdSetZxProcessTest, SetBasic) {
  EXPECT_EQ(zx_process_self(), thrd_get_zx_process());

  zx_handle_t previous = thrd_set_zx_process(ZX_HANDLE_INVALID);
  auto reset_handle = fit::defer([previous]() { thrd_set_zx_process(previous); });

  EXPECT_EQ(previous, zx_process_self());
  EXPECT_EQ(ZX_HANDLE_INVALID, thrd_get_zx_process());

  previous = thrd_set_zx_process(zx_process_self());
  EXPECT_EQ(previous, ZX_HANDLE_INVALID);
  EXPECT_EQ(zx_process_self(), thrd_get_zx_process());
}

TEST(ThrdSetZxProcessTest, SetInvalidAndCreate) {
  // Create a new thread with the default process handle.
  thrd_t t1;
  ASSERT_EQ(thrd_create(&t1, [](void* arg) { return 0; }, nullptr), thrd_success);

  int result;
  ASSERT_EQ(thrd_join(t1, &result), thrd_success);

  // Create a new thread with an invalid process handle.
  zx_handle_t previous = thrd_set_zx_process(ZX_HANDLE_INVALID);
  auto reset_handle = fit::defer([previous]() { thrd_set_zx_process(previous); });

  constexpr auto nop_thrd = [](void* arg) -> int { return 0; };
  constexpr auto nop_pthread = [](void* arg) -> void* { return nullptr; };

  // ESRCH indicates zx_thread_create failed due to a bad process handle.
  // POSIX does not specify an ESRCH case, but it implicitly allows one for
  // this case that's induced only using a Zircon-specific API.
  pthread_t pth;
  ASSERT_EQ(pthread_create(&pth, nullptr, nop_pthread, nullptr), ESRCH);

  // The C11 API has only thrd_error to return for anything that's not a
  // resource shortage per se where thrd_nomem best applies.
  thrd_t t2;
  ASSERT_EQ(thrd_create(&t2, nop_thrd, nullptr), thrd_error);
}

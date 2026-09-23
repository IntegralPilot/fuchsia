// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "profile_manager.h"

#include <fuchsia/scheduler/cpp/fidl.h>
#include <lib/zx/handle.h>
#include <lib/zx/result.h>
#include <zircon/syscalls.h>

#include <future>
#include <thread>
#include <tuple>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"
#include "testing_util.h"

namespace hwstress {
namespace {

TEST(ProfileManager, ApplyProfiles) {
  std::unique_ptr<ProfileManager> manager = ProfileManager::CreateFromEnvironment();
  ASSERT_TRUE(manager != nullptr);

  // Create a child thread that just blocks on a future.
  std::promise<bool> should_wake;
  auto worker = std::make_unique<std::thread>(
      [wake = should_wake.get_future()]() mutable { std::ignore = wake.get(); });

  // Set thread priority.
  EXPECT_OK(manager->SetThreadPriority(worker.get(), /*priority=*/1));

  // Set thread affinity.
  EXPECT_OK(manager->SetThreadAffinity(worker.get(), /*mask=*/1));

  // Ensure our affinity has been set correctly. (The kernel doesn't provide priority
  // information.)
  zx_info_thread info;
  ASSERT_OK(HandleFromThread(worker.get())
                ->get_info(ZX_INFO_THREAD, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(info.cpu_affinity_mask.mask[0], 0x1ul);

  // Clean up our child thread.
  should_wake.set_value(true);
  worker->join();
}

}  // namespace
}  // namespace hwstress

// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/last_reboot/last_reboot_info_provider.h"

#include <optional>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/forensics/feedback/reboot_log/final_shutdown_info.h"
#include "src/developer/forensics/testing/gpretty_printers.h"  // IWYU pragma: keep

namespace forensics {
namespace last_reboot {
namespace {

using feedback::FinalShutdownReason;
using ::fuchsia::feedback::LastReboot;

LastReboot MakeLastReboot(const FinalShutdownReason reason,
                          const std::optional<zx::duration> uptime = std::nullopt,
                          const std::optional<zx::duration> runtime = std::nullopt) {
  const feedback::FinalShutdownInfo final_shutdown_info(reason, uptime, runtime,
                                                        /*critical_process=*/std::nullopt);

  LastReboot out_last_reboot;

  LastRebootInfoProvider last_reboot_info_provider(final_shutdown_info);
  last_reboot_info_provider.Get(
      [&](LastReboot last_reboot) { out_last_reboot = std::move(last_reboot); });

  return out_last_reboot;
}

TEST(LastRebootInfoProviderTest, Succeed_Graceful) {
  const LastReboot last_reboot = MakeLastReboot(FinalShutdownReason::kGenericGraceful);

  ASSERT_TRUE(last_reboot.has_graceful());
  EXPECT_TRUE(last_reboot.graceful());

  EXPECT_FALSE(last_reboot.has_reason());
}

TEST(LastRebootInfoProviderTest, Succeed_NotGraceful) {
  const LastReboot last_reboot = MakeLastReboot(FinalShutdownReason::kKernelPanic);

  ASSERT_TRUE(last_reboot.has_graceful());
  EXPECT_FALSE(last_reboot.graceful());

  ASSERT_TRUE(last_reboot.has_reason());
  EXPECT_EQ(last_reboot.reason(), ::fuchsia::feedback::RebootReason::KERNEL_PANIC);
}

TEST(LastRebootInfoProviderTest, Succeed_Planned) {
  const LastReboot last_reboot = MakeLastReboot(FinalShutdownReason::kSystemUpdate);

  ASSERT_TRUE(last_reboot.has_planned());
  EXPECT_TRUE(last_reboot.planned());

  ASSERT_TRUE(last_reboot.has_reason());
  EXPECT_EQ(last_reboot.reason(), ::fuchsia::feedback::RebootReason::SYSTEM_UPDATE);
}

TEST(LastRebootInfoProviderTest, Succeed_NotPlanned) {
  const LastReboot last_reboot = MakeLastReboot(FinalShutdownReason::kUserRequest);

  ASSERT_TRUE(last_reboot.has_planned());
  EXPECT_FALSE(last_reboot.planned());

  ASSERT_TRUE(last_reboot.has_reason());
  EXPECT_EQ(last_reboot.reason(), ::fuchsia::feedback::RebootReason::USER_REQUEST);
}

TEST(LastRebootInfoProviderTest, Succeed_HasUptime) {
  const zx::duration uptime = zx::msec(100);

  const LastReboot last_reboot = MakeLastReboot(FinalShutdownReason::kUserRequest, uptime);

  ASSERT_TRUE(last_reboot.has_uptime());
  EXPECT_EQ(last_reboot.uptime(), uptime.to_nsecs());
}

TEST(LastRebootInfoProviderTest, Succeed_DoesNotHaveUptime) {
  const LastReboot last_reboot = MakeLastReboot(FinalShutdownReason::kUserRequest,
                                                /*uptime=*/std::nullopt);

  EXPECT_FALSE(last_reboot.has_uptime());
}

TEST(LastRebootInfoProviderTest, Succeed_HasRuntime) {
  const zx::duration runtime = zx::msec(78);

  const LastReboot last_reboot = MakeLastReboot(FinalShutdownReason::kUserRequest,
                                                /*uptime=*/std::nullopt, runtime);

  ASSERT_TRUE(last_reboot.has_runtime());
  EXPECT_EQ(last_reboot.runtime(), runtime.to_nsecs());
}

TEST(LastRebootInfoProviderTest, Succeed_DoesNotHaveRuntime) {
  const LastReboot last_reboot = MakeLastReboot(FinalShutdownReason::kUserRequest,
                                                /*uptime=*/std::nullopt,
                                                /*runtime=*/std::nullopt);

  EXPECT_FALSE(last_reboot.has_runtime());
}

TEST(LastRebootInfoProviderTest, Succeed_NotParseable) {
  const LastReboot last_reboot = MakeLastReboot(FinalShutdownReason::kNotParseable);

  EXPECT_FALSE(last_reboot.has_graceful());
  EXPECT_FALSE(last_reboot.has_reason());
}

}  // namespace
}  // namespace last_reboot
}  // namespace forensics

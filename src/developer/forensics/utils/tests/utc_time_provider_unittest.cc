// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/utils/utc_time_provider.h"

#include <fuchsia/time/cpp/fidl.h>
#include <lib/zx/clock.h>
#include <lib/zx/time.h>

#include <memory>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/forensics/testing/unit_test_fixture.h"
#include "src/developer/forensics/utils/utc_clock_ready_watcher.h"
#include "src/lib/files/file.h"
#include "src/lib/files/path.h"
#include "src/lib/timekeeper/test_clock.h"

namespace forensics {
namespace {

constexpr timekeeper::time_utc kTime((zx::hour(7) + zx::min(14) + zx::sec(52)).get());

class UtcTimeProviderTest : public UnitTestFixture {
 public:
  UtcTimeProviderTest() {
    clock_.SetUtc(kTime);

    zx_clock_create_args_v1_t clock_args{.backstop_time = 0};
    FX_CHECK(zx::clock::create(0u, &clock_args, &clock_handle_) == ZX_OK);

    utc_clock_ready_watcher_ = std::make_unique<UtcClockReadyWatcher>(
        dispatcher(), zx::unowned_clock(clock_handle_.get_handle()));
    utc_provider_ = std::make_unique<UtcTimeProvider>(utc_clock_ready_watcher_.get(), &clock_);
  }

 protected:
  void SignalLoggingQualityClock() {
    if (const zx_status_t status =
            clock_handle_.signal(/*clear_mask=*/0,
                                 /*set_mask=*/fuchsia::time::SIGNAL_UTC_CLOCK_LOGGING_QUALITY);
        status != ZX_OK) {
      FX_PLOGS(FATAL, status) << "Failed to achieve logging quality clock";
    }
  }

 protected:
  timekeeper::TestClock clock_;
  zx::clock clock_handle_;
  std::unique_ptr<UtcClockReadyWatcher> utc_clock_ready_watcher_;
  std::unique_ptr<UtcTimeProvider> utc_provider_;
};

TEST_F(UtcTimeProviderTest, Check_CurrentUtcBootDifference) {
  clock_.SetBoot(zx::time_boot(5));
  clock_.SetUtc(timekeeper::time_utc(42));
  SignalLoggingQualityClock();
  RunLoopUntilIdle();

  zx::time_boot boot = clock_.BootNow();
  timekeeper::time_utc utc;
  ASSERT_EQ(clock_.UtcNow(&utc), ZX_OK);

  const std::optional<zx::duration> utc_boot_difference = utc_provider_->CurrentUtcBootDifference();
  ASSERT_TRUE(utc_boot_difference.has_value());
  EXPECT_EQ(boot.get() + utc_boot_difference.value().get(), utc.get());
}

TEST_F(UtcTimeProviderTest, Check_ReadsPreviousBootUtcBootDifference) {
  ASSERT_TRUE(files::WriteFile("/cache/current_utc_monotonic_difference.txt", "1234"));

  // |is_first_instance| is true because the previous UTC-boot difference should be read.
  utc_provider_ = std::make_unique<UtcTimeProvider>(
      utc_clock_ready_watcher_.get(), &clock_,
      PreviousBootFile::FromCache(/*is_first_instance=*/true,
                                  "current_utc_monotonic_difference.txt"));

  const std::optional<zx::duration> previous_utc_boot_difference =
      utc_provider_->PreviousBootUtcBootDifference();

  ASSERT_TRUE(previous_utc_boot_difference.has_value());
  EXPECT_EQ(previous_utc_boot_difference.value().get(), 1234);

  ASSERT_TRUE(files::DeletePath("/cache/curren_utc_monotonic_difference.txt", /*recursive=*/true));
  ASSERT_TRUE(files::DeletePath("/tmp/curren_utc_monotonic_difference.txt", /*recursive=*/true));
}

TEST_F(UtcTimeProviderTest, Check_WritesPreviousBootUtcBootDifference) {
  SignalLoggingQualityClock();
  RunLoopUntilIdle();

  // |is_first_instance| is true because the previous UTC-boot difference should be read.
  utc_provider_ = std::make_unique<UtcTimeProvider>(
      utc_clock_ready_watcher_.get(), &clock_,
      PreviousBootFile::FromCache(/*is_first_instance=*/true,
                                  "current_utc_monotonic_difference.txt"));
  RunLoopUntilIdle();

  const std::optional<zx::duration> utc_boot_difference = utc_provider_->CurrentUtcBootDifference();
  ASSERT_TRUE(utc_boot_difference.has_value());

  std::string content;
  ASSERT_TRUE(files::ReadFileToString("/cache/current_utc_monotonic_difference.txt", &content));

  EXPECT_EQ(content, std::to_string(utc_boot_difference.value().get()));

  ASSERT_TRUE(files::DeletePath("/cache/curren_utc_monotonic_difference.txt", /*recursive=*/true));
  ASSERT_TRUE(files::DeletePath("/tmp/curren_utc_monotonic_difference.txt", /*recursive=*/true));
}

TEST_F(UtcTimeProviderTest, Check_NotReadyOnClockStarted) {
  ASSERT_EQ(
      clock_handle_.update(zx::clock::update_args().set_value(zx::time_monotonic(kTime.get()))),
      ZX_OK);
  RunLoopUntilIdle();

  EXPECT_FALSE(utc_provider_->CurrentUtcBootDifference().has_value());
}

TEST_F(UtcTimeProviderTest, Check_NotReadyOnClockSynchronized) {
  ASSERT_EQ(clock_handle_.signal(
                /*clear_mask=*/0, /*set_mask=*/fuchsia::time::SIGNAL_UTC_CLOCK_SYNCHRONIZED),
            ZX_OK);
  RunLoopUntilIdle();

  EXPECT_FALSE(utc_provider_->CurrentUtcBootDifference().has_value());
}

}  // namespace
}  // namespace forensics

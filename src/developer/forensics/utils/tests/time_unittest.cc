// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/utils/time.h"

#include <string>

#include <gtest/gtest.h>

#include "src/lib/timekeeper/clock.h"
#include "src/lib/timekeeper/test_clock.h"

namespace forensics {
namespace {
constexpr zx::duration kZero(zx::sec(0));

constexpr zx::duration kSecsOnly(zx::sec(1));
constexpr zx::duration kMinsOnly(zx::min(2));
constexpr zx::duration kHoursOnly(zx::hour(3));
constexpr zx::duration kDaysOnly(zx::hour(4 * 24));

constexpr zx::duration kSecsAndMins(kSecsOnly + kMinsOnly);
constexpr zx::duration kSecsAndHours(kSecsOnly + kHoursOnly);
constexpr zx::duration kSecsAndDays(kSecsOnly + kDaysOnly);
constexpr zx::duration kMinsAndHours(kMinsOnly + kHoursOnly);
constexpr zx::duration kMinsAndDays(kMinsOnly + kDaysOnly);
constexpr zx::duration kHoursAndDays(kHoursOnly + kDaysOnly);

constexpr zx::duration kSecsAndMinsAndHours(kSecsOnly + kMinsOnly + kHoursOnly);
constexpr zx::duration kSecsAndMinsAndDays(kSecsOnly + kMinsOnly + kDaysOnly);
constexpr zx::duration kSecsAndHoursAndDays(kSecsOnly + kHoursOnly + kDaysOnly);
constexpr zx::duration kMinsAndHoursAndDays(kMinsOnly + kHoursOnly + kDaysOnly);

constexpr zx::duration kAllUnits(kSecsOnly + kMinsOnly + kHoursOnly + kDaysOnly);
constexpr zx::duration kRndmNSecs(zx::nsec(278232000000000));
constexpr zx::duration kNegRndmNSecs(zx::nsec(-278232000000000));

constexpr char kZeroString[] = "000d00h00m00s";
constexpr char kSecsOnlyString[] = "000d00h00m01s";
constexpr char kMinsOnlyString[] = "000d00h02m00s";
constexpr char kHoursOnlyString[] = "000d03h00m00s";
constexpr char kDaysOnlyString[] = "004d00h00m00s";
constexpr char kSecsAndMinsString[] = "000d00h02m01s";
constexpr char kSecsAndHoursString[] = "000d03h00m01s";
constexpr char kSecsAndDaysString[] = "004d00h00m01s";
constexpr char kMinsAndHoursString[] = "000d03h02m00s";
constexpr char kMinsAndDaysString[] = "004d00h02m00s";
constexpr char kHoursAndDaysString[] = "004d03h00m00s";
constexpr char kSecsAndMinsAndHoursString[] = "000d03h02m01s";
constexpr char kSecsAndMinsAndDaysString[] = "004d00h02m01s";
constexpr char kSecsAndHoursAndDaysString[] = "004d03h00m01s";
constexpr char kMinsAndHoursAndDaysString[] = "004d03h02m00s";
constexpr char kAllUnitsString[] = "004d03h02m01s";
constexpr char kRndmNSecsString[] = "003d05h17m12s";

constexpr timekeeper::time_utc kTime1(0);
constexpr timekeeper::time_utc kTime2((zx::hour(7) + zx::min(14) + zx::sec(52)).get());
constexpr timekeeper::time_utc kTime3(
    (zx::hour(3) * 24 + zx::hour(15) + zx::min(33) + zx::sec(17)).get());

constexpr char kTime1Str[] = "1970-01-01 00:00:00 GMT";
constexpr char kTime2Str[] = "1970-01-01 07:14:52 GMT";
constexpr char kTime3Str[] = "1970-01-04 15:33:17 GMT";

TEST(TimeTest, FormatDuration_ZeroDuration) {
  EXPECT_EQ(FormatDuration(kZero).value(), kZeroString);
}

TEST(TimeTest, FormatDuration_SecondOnly) {
  EXPECT_EQ(FormatDuration(kSecsOnly).value(), kSecsOnlyString);
}

TEST(TimeTest, FormatDuration_MinuteOnly) {
  EXPECT_EQ(FormatDuration(kMinsOnly).value(), kMinsOnlyString);
}
TEST(TimeTest, FormatDuration_HourOnly) {
  EXPECT_EQ(FormatDuration(kHoursOnly).value(), kHoursOnlyString);
}

TEST(TimeTest, FormatDuration_DayOnly) {
  EXPECT_EQ(FormatDuration(kDaysOnly).value(), kDaysOnlyString);
}

TEST(TimeTest, FormatDuration_SecondAndMinute) {
  EXPECT_EQ(FormatDuration(kSecsAndMins).value(), kSecsAndMinsString);
}

TEST(TimeTest, FormatDuration_SecondAndHour) {
  EXPECT_EQ(FormatDuration(kSecsAndHours).value(), kSecsAndHoursString);
}

TEST(TimeTest, FormatDuration_SecondAndDay) {
  EXPECT_EQ(FormatDuration(kSecsAndDays).value(), kSecsAndDaysString);
}

TEST(TimeTest, FormatDuration_MinuteAndHour) {
  EXPECT_EQ(FormatDuration(kMinsAndHours).value(), kMinsAndHoursString);
}

TEST(TimeTest, FormatDuration_MinuteAndDay) {
  EXPECT_EQ(FormatDuration(kMinsAndDays).value(), kMinsAndDaysString);
}

TEST(TimeTest, FormatDuration_HourAndDay) {
  EXPECT_EQ(FormatDuration(kHoursAndDays).value(), kHoursAndDaysString);
}

TEST(TimeTest, FormatDuration_SecAndMinAndHour) {
  EXPECT_EQ(FormatDuration(kSecsAndMinsAndHours).value(), kSecsAndMinsAndHoursString);
}

TEST(TimeTest, FormatDuration_SecAndMinAndDay) {
  EXPECT_EQ(FormatDuration(kSecsAndMinsAndDays).value(), kSecsAndMinsAndDaysString);
}

TEST(TimeTest, FormatDuration_SecAndHourAndDay) {
  EXPECT_EQ(FormatDuration(kSecsAndHoursAndDays).value(), kSecsAndHoursAndDaysString);
}

TEST(TimeTest, FormatDuration_MinAndHourAndDay) {
  EXPECT_EQ(FormatDuration(kMinsAndHoursAndDays).value(), kMinsAndHoursAndDaysString);
}

TEST(TimeTest, FormatDuration_AllUnits) {
  EXPECT_EQ(FormatDuration(kAllUnits).value(), kAllUnitsString);
}

TEST(TimeTest, FormatDuration_RandomNSec) {
  EXPECT_EQ(FormatDuration(kRndmNSecs).value(), kRndmNSecsString);
}

TEST(TimeTest, FormatDuration_NegativeRandomNSec) {
  EXPECT_EQ(FormatDuration(kNegRndmNSecs), std::nullopt);
}

TEST(TimeTest, FormatSecondsSinceEpoch_ValidValue) {
  EXPECT_EQ(FormatSecondsSinceEpoch("1748946819"), "2025-06-03T10:33:39+00:00");
}

TEST(TimeTest, FormatSecondsSinceEpoch_EpochValue) {
  EXPECT_EQ(FormatSecondsSinceEpoch("0"), std::nullopt);
}

TEST(TimeTest, FormatSecondsSinceEpoch_NonNumber) {
  EXPECT_EQ(FormatSecondsSinceEpoch("abc"), std::nullopt);
}

TEST(TimeTest, CurrentUtcTimeRaw) {
  timekeeper::TestClock clock;

  clock.SetUtc(kTime1);
  EXPECT_EQ(CurrentUtcTimeRaw(&clock), kTime1);

  clock.SetUtc(kTime2);
  EXPECT_EQ(CurrentUtcTimeRaw(&clock), kTime2);

  clock.SetUtc(kTime3);
  EXPECT_EQ(CurrentUtcTimeRaw(&clock), kTime3);
}

TEST(TimeTest, CurrentUtcTime) {
  timekeeper::TestClock clock;

  clock.SetUtc(kTime1);
  EXPECT_EQ(CurrentUtcTime(&clock), kTime1Str);

  clock.SetUtc(kTime2);
  EXPECT_EQ(CurrentUtcTime(&clock), kTime2Str);

  clock.SetUtc(kTime3);
  EXPECT_EQ(CurrentUtcTime(&clock), kTime3Str);
}

// Returns ZX_ERR_BAD_HANDLE when used to get a UTC time.
class InvalidClock : public timekeeper::Clock {
 private:
  zx_status_t GetUtcTime(zx_time_t* time) const override { return ZX_ERR_BAD_HANDLE; }
  zx_instant_mono_t GetMonotonicTime() const override { return 0; }
  zx_instant_boot_t GetBootTime() const override { return 0; }
};

TEST(TimeTest, CurrentUtcTimeRaw_InvalidStatus) {
  InvalidClock invalid_clock;
  ASSERT_DEATH(CurrentUtcTimeRaw(&invalid_clock), "Failed to get current Utc time");
}

}  // namespace
}  // namespace forensics

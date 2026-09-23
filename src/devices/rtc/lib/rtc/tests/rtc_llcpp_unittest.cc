// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <librtc_llcpp.h>

#include <zxtest/zxtest.h>

#include "src/devices/testing/mock-ddk/mock-device.h"

namespace rtc {

namespace {

FidlRtc::wire::Time MakeRtc(uint16_t year, uint8_t month, uint8_t day, uint8_t hours,
                            uint8_t minutes, uint8_t seconds) {
  return FidlRtc::wire::Time{seconds, minutes, hours, day, month, year};
}

}  // namespace

TEST(RtcLlccpTest, RTCYearsValid) {
  auto t0 = MakeRtc(1999, 1, 1, 0, 0, 0);
  EXPECT_FALSE(IsRtcValid(t0));

  t0.year = 2000;
  EXPECT_TRUE(IsRtcValid(t0));

  t0.year = 2100;
  EXPECT_FALSE(IsRtcValid(t0));
}

TEST(RtcLlccpTest, RTCMonthsValid) {
  auto t0 = MakeRtc(2001, 7, 1, 0, 0, 0);
  EXPECT_TRUE(IsRtcValid(t0));

  t0.month = 13;
  EXPECT_FALSE(IsRtcValid(t0));

  t0.month = 0;
  EXPECT_FALSE(IsRtcValid(t0));
}

TEST(RtcLlccpTest, RTCDaysValid) {
  auto t0 = MakeRtc(2001, 1, 1, 0, 0, 0);
  EXPECT_TRUE(IsRtcValid(t0));

  t0.month = JANUARY;
  t0.day = 0;
  EXPECT_FALSE(IsRtcValid(t0));
  t0.day = 31;
  EXPECT_TRUE(IsRtcValid(t0));
  t0.day = 32;
  EXPECT_FALSE(IsRtcValid(t0));

  t0.month = FEBRUARY;
  t0.day = 28;  // not a leap year
  EXPECT_TRUE(IsRtcValid(t0));
  t0.day = 29;  // not a leap year
  EXPECT_FALSE(IsRtcValid(t0));

  t0.month = MARCH;
  t0.day = 31;
  EXPECT_TRUE(IsRtcValid(t0));
  t0.month = MARCH;
  t0.day = 32;
  EXPECT_FALSE(IsRtcValid(t0));

  t0.month = APRIL;
  t0.day = 30;
  EXPECT_TRUE(IsRtcValid(t0));
  t0.day = 31;
  EXPECT_FALSE(IsRtcValid(t0));

  t0.month = MAY;
  t0.day = 31;
  EXPECT_TRUE(IsRtcValid(t0));
  t0.day = 32;
  EXPECT_FALSE(IsRtcValid(t0));

  t0.month = JUNE;
  t0.day = 30;
  EXPECT_TRUE(IsRtcValid(t0));
  t0.day = 31;
  EXPECT_FALSE(IsRtcValid(t0));

  t0.month = JULY;
  t0.day = 31;
  EXPECT_TRUE(IsRtcValid(t0));
  t0.day = 32;
  EXPECT_FALSE(IsRtcValid(t0));

  t0.month = AUGUST;
  t0.day = 31;
  EXPECT_TRUE(IsRtcValid(t0));
  t0.day = 32;
  EXPECT_FALSE(IsRtcValid(t0));

  t0.month = SEPTEMBER;
  t0.day = 30;
  EXPECT_TRUE(IsRtcValid(t0));
  t0.day = 31;
  EXPECT_FALSE(IsRtcValid(t0));

  t0.month = OCTOBER;
  t0.day = 31;
  EXPECT_TRUE(IsRtcValid(t0));
  t0.day = 32;
  EXPECT_FALSE(IsRtcValid(t0));

  t0.month = NOVEMBER;
  t0.day = 30;
  EXPECT_TRUE(IsRtcValid(t0));
  t0.day = 31;
  EXPECT_FALSE(IsRtcValid(t0));

  t0.month = DECEMBER;
  t0.day = 31;
  EXPECT_TRUE(IsRtcValid(t0));
  t0.day = 32;
  EXPECT_FALSE(IsRtcValid(t0));
  t0.day = 99;
  EXPECT_FALSE(IsRtcValid(t0));
}

TEST(RtcLlccpTest, HoursMinutesSecondsValid) {
  auto t0 = MakeRtc(2001, 1, 1, 0, 0, 0);
  EXPECT_TRUE(IsRtcValid(t0));

  t0.day = 1;
  t0.hours = 0;
  EXPECT_TRUE(IsRtcValid(t0));
  t0.hours = 23;
  EXPECT_TRUE(IsRtcValid(t0));
  t0.hours = 24;
  EXPECT_FALSE(IsRtcValid(t0));
  t0.hours = 25;
  EXPECT_FALSE(IsRtcValid(t0));

  t0.hours = 1;
  t0.minutes = 0;
  EXPECT_TRUE(IsRtcValid(t0));
  t0.minutes = 59;
  EXPECT_TRUE(IsRtcValid(t0));
  t0.minutes = 60;
  EXPECT_FALSE(IsRtcValid(t0));
  t0.minutes = 61;
  EXPECT_FALSE(IsRtcValid(t0));

  t0.minutes = 1;
  t0.seconds = 0;
  EXPECT_TRUE(IsRtcValid(t0));
  t0.seconds = 59;
  EXPECT_TRUE(IsRtcValid(t0));
  t0.seconds = 60;
  EXPECT_FALSE(IsRtcValid(t0));
  t0.seconds = 61;
  EXPECT_FALSE(IsRtcValid(t0));
}

TEST(RtcLlccpTest, LeapYears) {
  auto t0 = MakeRtc(2000, 2, 28, 0, 0, 0);  // Is a leap year
  EXPECT_TRUE(IsRtcValid(t0));

  t0.day = 29;
  EXPECT_TRUE(IsRtcValid(t0));

  t0.year = 2001;  // NOT a leap year
  EXPECT_FALSE(IsRtcValid(t0));

  t0.year = 2004;  // A leap year
  EXPECT_TRUE(IsRtcValid(t0));

  t0.year = 2020;  // A leap year
  EXPECT_TRUE(IsRtcValid(t0));
}

TEST(RtcLlccpTest, SecondsSinceEpoch) {
  auto t0 = MakeRtc(2018, 8, 4, 1, 19, 1);
  EXPECT_EQ(1533345541, SecondsSinceEpoch(t0));

  auto t1 = MakeRtc(2000, 1, 1, 0, 0, 0);
  EXPECT_EQ(946684800, SecondsSinceEpoch(t1));
}

}  // namespace rtc

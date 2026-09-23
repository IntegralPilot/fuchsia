// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/thermal/ntc.h>

#include <gtest/gtest.h>

namespace thermal {

namespace {

constexpr uint32_t kFakePullupOhms = 47000;
constexpr thermal::NtcInfo kFakeNtcInfo = {
    .part = "ncpXXwf104",
    .profile =
        {
            {.temperature_c = -40, .resistance_ohm = 4397119},
            {.temperature_c = -35, .resistance_ohm = 3088599},
            {.temperature_c = -30, .resistance_ohm = 2197225},
            {.temperature_c = -25, .resistance_ohm = 1581881},
            {.temperature_c = -20, .resistance_ohm = 1151037},
            {.temperature_c = -15, .resistance_ohm = 846579},
            {.temperature_c = -10, .resistance_ohm = 628988},
            {.temperature_c = -5, .resistance_ohm = 471632},
            {.temperature_c = 0, .resistance_ohm = 357012},
            {.temperature_c = 5, .resistance_ohm = 272500},
            {.temperature_c = 10, .resistance_ohm = 209710},
            {.temperature_c = 15, .resistance_ohm = 162651},
            {.temperature_c = 20, .resistance_ohm = 127080},
            {.temperature_c = 25, .resistance_ohm = 100000},
            {.temperature_c = 30, .resistance_ohm = 79222},
            {.temperature_c = 35, .resistance_ohm = 63167},
            {.temperature_c = 40, .resistance_ohm = 50677},
            {.temperature_c = 45, .resistance_ohm = 40904},
            {.temperature_c = 50, .resistance_ohm = 33195},
            {.temperature_c = 55, .resistance_ohm = 27091},
            {.temperature_c = 60, .resistance_ohm = 22224},
            {.temperature_c = 65, .resistance_ohm = 18323},
            {.temperature_c = 70, .resistance_ohm = 15184},
            {.temperature_c = 75, .resistance_ohm = 12635},
            {.temperature_c = 80, .resistance_ohm = 10566},
            {.temperature_c = 85, .resistance_ohm = 8873},
            {.temperature_c = 90, .resistance_ohm = 7481},
            {.temperature_c = 95, .resistance_ohm = 6337},
            {.temperature_c = 100, .resistance_ohm = 5384},
            {.temperature_c = 105, .resistance_ohm = 4594},
            {.temperature_c = 110, .resistance_ohm = 3934},
            {.temperature_c = 115, .resistance_ohm = 3380},
            {.temperature_c = 120, .resistance_ohm = 2916},
            {.temperature_c = 125, .resistance_ohm = 2522},
        },
};

}  // namespace

TEST(ThermalNtcTests, GetTemperatureCelsiusInvalidLow) {
  Ntc ntc(kFakeNtcInfo, kFakePullupOhms);
  float temp;
  EXPECT_EQ(ntc.GetTemperatureCelsius(-0.5f, &temp), ZX_ERR_INVALID_ARGS);
}

TEST(ThermalNtcTests, GetTemperatureCelsiusInvalidHigh) {
  Ntc ntc(kFakeNtcInfo, kFakePullupOhms);
  float temp;
  EXPECT_EQ(ntc.GetTemperatureCelsius(1.0f, &temp), ZX_ERR_INVALID_ARGS);
}

TEST(ThermalNtcTests, GetTemperatureCelsiusLow) {
  Ntc ntc(kFakeNtcInfo, kFakePullupOhms);
  float temp;
  EXPECT_EQ(ntc.GetTemperatureCelsius(0.0f, &temp), ZX_OK);
  EXPECT_FLOAT_EQ(temp, 125.0f);
}

TEST(ThermalNtcTests, GetTemperatureCelsiusHigh) {
  Ntc ntc(kFakeNtcInfo, kFakePullupOhms);
  float temp;
  EXPECT_EQ(ntc.GetTemperatureCelsius(0.99f, &temp), ZX_OK);
  EXPECT_FLOAT_EQ(temp, -40.0f);
}

TEST(ThermalNtcTests, GetTemperatureCelsius) {
  Ntc ntc(kFakeNtcInfo, kFakePullupOhms);
  float temp;
  EXPECT_EQ(ntc.GetTemperatureCelsius(0.88f, &temp), ZX_OK);
  EXPECT_FLOAT_EQ(temp, 0.7303901f);
}

TEST(ThermalNtcTests, GetNormalizedSampleLow) {
  Ntc ntc(kFakeNtcInfo, kFakePullupOhms);
  float sample;
  EXPECT_EQ(ntc.GetNormalizedSample(-45.0f, &sample), ZX_OK);
  EXPECT_FLOAT_EQ(sample, 0.9894242f);
}

TEST(ThermalNtcTests, GetNormalizedSampleHigh) {
  Ntc ntc(kFakeNtcInfo, kFakePullupOhms);
  float sample;
  EXPECT_EQ(ntc.GetNormalizedSample(125.0f, &sample), ZX_OK);
  EXPECT_FLOAT_EQ(sample, 0.05092686f);
}

TEST(ThermalNtcTests, GetNormalizedSample) {
  Ntc ntc(kFakeNtcInfo, kFakePullupOhms);
  float sample;
  EXPECT_EQ(ntc.GetNormalizedSample(2.5f, &sample), ZX_OK);
  EXPECT_FLOAT_EQ(sample, 0.8700782f);
}

}  // namespace thermal

// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/audio_core/usage_settings.h"

#include <gtest/gtest.h>

#include "src/media/audio/audio_core/stream_usage.h"
#include "src/media/audio/lib/processing/gain.h"

namespace media::audio {
namespace {

using fuchsia::media::AudioRenderUsage2;

constexpr float kArbitraryGainValue = -45.0f;
constexpr float kArbitraryGainAdjustment = -2.0f;

constexpr float kArbitraryVolumeValue = 0.14f;

TEST(UsageGainSettingsTest, RenderUsageGainPersists) {
  UsageGainSettings under_test;

  const auto test_usage = [&under_test](auto render_usage) {
    under_test.SetUsageGain(ToFidlUsage2(render_usage), kArbitraryGainValue);
    EXPECT_FLOAT_EQ(under_test.GetAdjustedUsageGain(ToFidlUsage2(render_usage)),
                    kArbitraryGainValue);

    under_test.SetUsageGainAdjustment(ToFidlUsage2(render_usage), kArbitraryGainAdjustment);
    EXPECT_FLOAT_EQ(under_test.GetAdjustedUsageGain(ToFidlUsage2(render_usage)),
                    kArbitraryGainValue + kArbitraryGainAdjustment);
  };

  test_usage(RenderUsage::MEDIA);
  test_usage(RenderUsage::COMMUNICATION);
}

TEST(UsageGainSettingsTest, RenderUsageGainPersistsComponents) {
  UsageGainSettings under_test;

  const auto test_usage = [&under_test](auto render_usage) {
    under_test.SetUsageGain(ToFidlUsage2(render_usage), kArbitraryGainValue);
    EXPECT_FLOAT_EQ(under_test.GetAdjustedUsageGain(ToFidlUsage2(render_usage)),
                    kArbitraryGainValue);

    under_test.SetUsageGainAdjustment(ToFidlUsage2(render_usage), kArbitraryGainAdjustment);
    EXPECT_FLOAT_EQ(under_test.GetUnadjustedUsageGain(ToFidlUsage2(render_usage)),
                    kArbitraryGainValue);
  };

  test_usage(RenderUsage::MEDIA);
  test_usage(RenderUsage::COMMUNICATION);
}

TEST(UsageGainSettingsTest, RenderUsageGainAdjustmentPersists) {
  UsageGainSettings under_test;

  const auto test_usage = [&under_test](auto render_usage) {
    under_test.SetUsageGain(ToFidlUsage2(render_usage), kArbitraryGainValue);
    EXPECT_FLOAT_EQ(under_test.GetAdjustedUsageGain(ToFidlUsage2(render_usage)),
                    kArbitraryGainValue);

    under_test.SetUsageGainAdjustment(ToFidlUsage2(render_usage), kArbitraryGainAdjustment);
    EXPECT_FLOAT_EQ(under_test.GetUsageGainAdjustment(ToFidlUsage2(render_usage)),
                    kArbitraryGainAdjustment);
  };

  test_usage(RenderUsage::MEDIA);
  test_usage(RenderUsage::COMMUNICATION);
}

TEST(UsageGainSettingsTest, CaptureUsageGainPersists) {
  UsageGainSettings under_test;

  const auto test_usage = [&under_test](auto capture_usage) {
    under_test.SetUsageGain(ToFidlUsage2(capture_usage), kArbitraryGainValue);
    EXPECT_FLOAT_EQ(under_test.GetAdjustedUsageGain(ToFidlUsage2(capture_usage)),
                    kArbitraryGainValue);

    under_test.SetUsageGainAdjustment(ToFidlUsage2(capture_usage), kArbitraryGainAdjustment);
    EXPECT_FLOAT_EQ(under_test.GetAdjustedUsageGain(ToFidlUsage2(capture_usage)),
                    kArbitraryGainValue + kArbitraryGainAdjustment);
  };

  test_usage(CaptureUsage::BACKGROUND);
  test_usage(CaptureUsage::SYSTEM_AGENT);
}

TEST(UsageGainSettingsTest, CaptureUsageGainPersistsComponents) {
  UsageGainSettings under_test;

  const auto test_usage = [&under_test](auto capture_usage) {
    under_test.SetUsageGain(ToFidlUsage2(capture_usage), kArbitraryGainValue);
    EXPECT_FLOAT_EQ(under_test.GetAdjustedUsageGain(ToFidlUsage2(capture_usage)),
                    kArbitraryGainValue);

    under_test.SetUsageGainAdjustment(ToFidlUsage2(capture_usage), kArbitraryGainAdjustment);
    EXPECT_FLOAT_EQ(under_test.GetUnadjustedUsageGain(ToFidlUsage2(capture_usage)),
                    kArbitraryGainValue);
  };

  test_usage(CaptureUsage::BACKGROUND);
  test_usage(CaptureUsage::SYSTEM_AGENT);
}

TEST(UsageGainSettingsTest, CaptureUsageGainAdjustmentPersists) {
  UsageGainSettings under_test;

  const auto test_usage = [&under_test](auto capture_usage) {
    under_test.SetUsageGain(ToFidlUsage2(capture_usage), kArbitraryGainValue);
    EXPECT_FLOAT_EQ(under_test.GetAdjustedUsageGain(ToFidlUsage2(capture_usage)),
                    kArbitraryGainValue);

    under_test.SetUsageGainAdjustment(ToFidlUsage2(capture_usage), kArbitraryGainAdjustment);
    EXPECT_FLOAT_EQ(under_test.GetUsageGainAdjustment(ToFidlUsage2(capture_usage)),
                    kArbitraryGainAdjustment);
  };

  test_usage(CaptureUsage::BACKGROUND);
  test_usage(CaptureUsage::SYSTEM_AGENT);
}

TEST(UsageGainSettingsTest, UsageGainCannotExceedUnity) {
  const auto usage = ToFidlUsage2(RenderUsage::SYSTEM_AGENT);
  UsageGainSettings under_test;
  under_test.SetUsageGain(fidl::Clone(usage), 10.0f);

  EXPECT_FLOAT_EQ(under_test.GetAdjustedUsageGain(std::move(usage)), media_audio::kUnityGainDb);
}

TEST(UsageVolumeSettingsTest, RenderUsageVolumePersists) {
  UsageVolumeSettings under_test;

  const auto test_usage = [&under_test](auto render_usage) {
    under_test.SetUsageVolume(ToFidlUsage2(render_usage), kArbitraryVolumeValue);
    EXPECT_FLOAT_EQ(under_test.GetUsageVolume(ToFidlUsage2(render_usage)), kArbitraryVolumeValue);
  };

  test_usage(RenderUsage::ACCESSIBILITY);
  test_usage(RenderUsage::BACKGROUND);
  test_usage(RenderUsage::COMMUNICATION);
  test_usage(RenderUsage::INTERRUPTION);
  test_usage(RenderUsage::MEDIA);
  test_usage(RenderUsage::SYSTEM_AGENT);
}

TEST(UsageVolumeSettingsTest, CaptureUsageVolumePersists) {
  UsageVolumeSettings under_test;

  const auto test_usage = [&under_test](auto capture_usage) {
    under_test.SetUsageVolume(ToFidlUsage2(capture_usage), kArbitraryVolumeValue);
    EXPECT_FLOAT_EQ(under_test.GetUsageVolume(ToFidlUsage2(capture_usage)), kArbitraryVolumeValue);
  };

  test_usage(CaptureUsage::BACKGROUND);
  test_usage(CaptureUsage::SYSTEM_AGENT);
}

TEST(UsageVolumeSettingsTest, DefaultVolumeIsMax) {
  UsageVolumeSettings under_test;

  const auto test_usage = [&under_test](auto capture_usage) {
    EXPECT_FLOAT_EQ(under_test.GetUsageVolume(ToFidlUsage2(capture_usage)),
                    fuchsia::media::audio::MAX_VOLUME);
  };

  test_usage(CaptureUsage::BACKGROUND);
  test_usage(CaptureUsage::SYSTEM_AGENT);
}

}  // namespace
}  // namespace media::audio

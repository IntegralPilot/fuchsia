// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <audio-proto-utils/format-utils.h>
#include <zxtest/zxtest.h>

namespace audio::utils {

TEST(FormatUtil, EnumerationRatesSingleRate) {
  audio_stream_format_range_t range = {
      .sample_formats = AUDIO_SAMPLE_FORMAT_16BIT,
      .min_frames_per_second = 16'000,
      .max_frames_per_second = 16'000,
      .min_channels = 1,
      .max_channels = 8,
      .flags = ASF_RANGE_FLAG_FPS_CONTINUOUS,
  };
  FrameRateEnumerator enumerator(range);
  std::vector<uint32_t> rates(enumerator.begin(), enumerator.end());
  EXPECT_EQ(1, rates.size());
  EXPECT_EQ(16'000, rates[0]);
}

TEST(FormatUtil, EnumerationRatesSingleFamily48kHz) {
  audio_stream_format_range_t range = {
      .sample_formats = AUDIO_SAMPLE_FORMAT_16BIT,
      .min_frames_per_second = 48'000,
      .max_frames_per_second = 192'000,
      .min_channels = 1,
      .max_channels = 1,
      .flags = ASF_RANGE_FLAG_FPS_48000_FAMILY,
  };
  FrameRateEnumerator enumerator(range);
  std::vector<uint32_t> rates(enumerator.begin(), enumerator.end());
  EXPECT_EQ(3, rates.size());
  EXPECT_EQ(48'000, rates[0]);
  EXPECT_EQ(96'000, rates[1]);
  EXPECT_EQ(192'000, rates[2]);
}

TEST(FormatUtil, EnumerationRatesSingleFamily44100Hz) {
  audio_stream_format_range_t range = {
      .sample_formats = AUDIO_SAMPLE_FORMAT_16BIT,
      .min_frames_per_second = 44'100,
      .max_frames_per_second = 88'200,
      .min_channels = 1,
      .max_channels = 1,
      .flags = ASF_RANGE_FLAG_FPS_44100_FAMILY,
  };
  FrameRateEnumerator enumerator(range);
  std::vector<uint32_t> rates(enumerator.begin(), enumerator.end());
  EXPECT_EQ(2, rates.size());
  EXPECT_EQ(44'100, rates[0]);
  EXPECT_EQ(88'200, rates[1]);
}

TEST(FormatUtil, EnumerationRatesMixedFamilies) {
  audio_stream_format_range_t range = {
      .sample_formats = AUDIO_SAMPLE_FORMAT_16BIT,
      .min_frames_per_second = 16'000,
      .max_frames_per_second = 48'000,
      .min_channels = 1,
      .max_channels = 1,
      .flags = ASF_RANGE_FLAG_FPS_48000_FAMILY | ASF_RANGE_FLAG_FPS_44100_FAMILY,
  };
  FrameRateEnumerator enumerator(range);
  std::vector<uint32_t> rates(enumerator.begin(), enumerator.end());
  EXPECT_EQ(5, rates.size());
  EXPECT_EQ(16'000, rates[0]);
  EXPECT_EQ(22'050, rates[1]);
  EXPECT_EQ(32'000, rates[2]);
  EXPECT_EQ(44'100, rates[3]);
  EXPECT_EQ(48'000, rates[4]);
}

}  // namespace audio::utils

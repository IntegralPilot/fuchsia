// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/audio_core/select_best_format.h"

#include <zircon/errors.h>

#include <cstdint>
#include <unordered_map>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"

namespace media::audio {
namespace {

void AddChannelSet(fuchsia::hardware::audio::PcmSupportedFormats& formats,
                   size_t number_of_channels) {
  fuchsia::hardware::audio::ChannelSet channel_set = {};
  std::vector<fuchsia::hardware::audio::ChannelAttributes> attributes(number_of_channels);
  channel_set.set_attributes(std::move(attributes));
  formats.mutable_channel_sets()->push_back(std::move(channel_set));
}

void AddChannelSet(fuchsia_audio_device::PcmFormatSet& formats, size_t number_of_channels) {
  fuchsia_audio_device::ChannelSet channel_set;
  channel_set.attributes(std::vector<fuchsia_audio_device::ChannelAttributes>(number_of_channels));
  if (!formats.channel_sets()) {
    formats.channel_sets({{}});
  }
  formats.channel_sets()->push_back(std::move(channel_set));
}

TEST(SelectBestFormatTest, SelectBestFormatFound) {
  std::vector<fuchsia::hardware::audio::PcmSupportedFormats> fmts;

  fuchsia::hardware::audio::PcmSupportedFormats formats = {};
  AddChannelSet(formats, 1);
  AddChannelSet(formats, 2);
  AddChannelSet(formats, 4);
  AddChannelSet(formats, 8);
  formats.mutable_sample_formats()->push_back(fuchsia::hardware::audio::SampleFormat::PCM_FLOAT);
  formats.mutable_bytes_per_sample()->push_back(4);
  formats.mutable_valid_bits_per_sample()->push_back(32);
  formats.mutable_frame_rates()->push_back(12'000);
  formats.mutable_frame_rates()->push_back(24'000);
  formats.mutable_frame_rates()->push_back(48'000);
  formats.mutable_frame_rates()->push_back(96'000);
  fmts.push_back(std::move(formats));

  fuchsia::media::AudioSampleFormat sample_format_inout = fuchsia::media::AudioSampleFormat::FLOAT;
  uint32_t frames_per_second_inout = 96'000;
  uint32_t channels_inout = 1;

  ASSERT_EQ(SelectBestFormat(fmts, &frames_per_second_inout, &channels_inout, &sample_format_inout),
            ZX_OK);
  ASSERT_EQ(sample_format_inout, fuchsia::media::AudioSampleFormat::FLOAT);
  ASSERT_EQ(frames_per_second_inout, static_cast<uint32_t>(96'000));
  ASSERT_EQ(channels_inout, static_cast<uint32_t>(1));

  // Add a second format range.
  fuchsia::hardware::audio::PcmSupportedFormats formats2 = {};
  AddChannelSet(formats2, 4);
  AddChannelSet(formats2, 5);
  AddChannelSet(formats2, 6);
  AddChannelSet(formats2, 7);
  AddChannelSet(formats2, 8);
  formats2.mutable_sample_formats()->push_back(fuchsia::hardware::audio::SampleFormat::PCM_SIGNED);
  formats2.mutable_bytes_per_sample()->push_back(2);
  formats2.mutable_valid_bits_per_sample()->push_back(16);
  formats2.mutable_frame_rates()->push_back(22'050);
  formats2.mutable_frame_rates()->push_back(44'100);
  formats2.mutable_frame_rates()->push_back(88'200);
  formats2.mutable_frame_rates()->push_back(176'400);
  fmts.push_back(std::move(formats2));

  sample_format_inout = fuchsia::media::AudioSampleFormat::SIGNED_16;
  frames_per_second_inout = 88'200;
  channels_inout = 5;

  ASSERT_EQ(SelectBestFormat(fmts, &frames_per_second_inout, &channels_inout, &sample_format_inout),
            ZX_OK);
  ASSERT_EQ(sample_format_inout, fuchsia::media::AudioSampleFormat::SIGNED_16);
  ASSERT_EQ(frames_per_second_inout, static_cast<uint32_t>(88'200));
  ASSERT_EQ(channels_inout, static_cast<uint32_t>(5));
}

TEST(SelectBestFormatTest, SelectBestFormatFoundNewFidl) {
  std::vector<fuchsia_audio_device::PcmFormatSet> fmts;

  fuchsia_audio_device::PcmFormatSet formats;
  AddChannelSet(formats, 1);
  AddChannelSet(formats, 2);
  AddChannelSet(formats, 4);
  AddChannelSet(formats, 8);
  formats.sample_types({{fuchsia_audio::SampleType::kFloat32}});
  formats.frame_rates({{12'000, 24'000, 48'000, 96'000}});
  fmts.push_back(std::move(formats));

  auto pref = media_audio::Format::CreateOrDie({
      .sample_type = fuchsia_audio::SampleType::kFloat32,
      .channels = 1,
      .frames_per_second = 96'000,
  });

  auto result = SelectBestFormat(fmts, pref);
  ASSERT_TRUE(result.is_ok());
  EXPECT_EQ(result->sample_type(), fuchsia_audio::SampleType::kFloat32);
  EXPECT_EQ(result->frames_per_second(), 96'000);
  EXPECT_EQ(result->channels(), 1);

  // Add a second format range.
  fuchsia_audio_device::PcmFormatSet formats2;
  AddChannelSet(formats2, 4);
  AddChannelSet(formats2, 5);
  AddChannelSet(formats2, 6);
  AddChannelSet(formats2, 7);
  AddChannelSet(formats2, 8);
  formats2.sample_types({{fuchsia_audio::SampleType::kInt16}});
  formats2.frame_rates({{22'050, 44'100, 88'200, 176'400}});
  fmts.push_back(std::move(formats2));

  pref = media_audio::Format::CreateOrDie({
      .sample_type = fuchsia_audio::SampleType::kInt16,
      .channels = 5,
      .frames_per_second = 88'200,
  });

  result = SelectBestFormat(fmts, pref);
  ASSERT_TRUE(result.is_ok());
  EXPECT_EQ(result->sample_type(), fuchsia_audio::SampleType::kInt16);
  EXPECT_EQ(result->frames_per_second(), 88'200);
  EXPECT_EQ(result->channels(), 5);
}

TEST(SelectBestFormatTest, SelectBestFormatOutsideRanges) {
  std::vector<fuchsia::hardware::audio::PcmSupportedFormats> fmts;
  fuchsia::hardware::audio::PcmSupportedFormats formats = {};
  AddChannelSet(formats, 1);
  AddChannelSet(formats, 2);
  AddChannelSet(formats, 4);
  AddChannelSet(formats, 8);
  formats.mutable_sample_formats()->push_back(fuchsia::hardware::audio::SampleFormat::PCM_SIGNED);
  formats.mutable_bytes_per_sample()->push_back(2);
  formats.mutable_valid_bits_per_sample()->push_back(16);
  formats.mutable_frame_rates()->push_back(16'000);
  formats.mutable_frame_rates()->push_back(24'000);
  formats.mutable_frame_rates()->push_back(48'000);
  formats.mutable_frame_rates()->push_back(96'000);
  fmts.push_back(std::move(formats));

  fuchsia::media::AudioSampleFormat sample_format_inout =
      fuchsia::media::AudioSampleFormat::SIGNED_16;
  uint32_t frames_per_second_inout = 0;
  uint32_t channels_inout = 0;

  ASSERT_EQ(SelectBestFormat(fmts, &frames_per_second_inout, &channels_inout, &sample_format_inout),
            ZX_OK);
  ASSERT_EQ(sample_format_inout, fuchsia::media::AudioSampleFormat::SIGNED_16);
  ASSERT_EQ(frames_per_second_inout, static_cast<uint32_t>(16'000));  // Prefer closest available.
  ASSERT_EQ(channels_inout, static_cast<uint32_t>(2));                // Prefer 2 channels.

  sample_format_inout = fuchsia::media::AudioSampleFormat::UNSIGNED_8;
  frames_per_second_inout = 192'000;
  channels_inout = 200;

  ASSERT_EQ(SelectBestFormat(fmts, &frames_per_second_inout, &channels_inout, &sample_format_inout),
            ZX_OK);
  ASSERT_EQ(sample_format_inout, fuchsia::media::AudioSampleFormat::SIGNED_16);
  ASSERT_EQ(frames_per_second_inout, static_cast<uint32_t>(96'000));  // Pick closest available.
  ASSERT_EQ(channels_inout, static_cast<uint32_t>(2));                // Prefer 2 channels.

  // Add a second format range.
  fuchsia::hardware::audio::PcmSupportedFormats formats2 = {};
  AddChannelSet(formats2, 4);
  AddChannelSet(formats2, 5);
  AddChannelSet(formats2, 6);
  AddChannelSet(formats2, 7);
  AddChannelSet(formats2, 8);
  formats2.mutable_sample_formats()->push_back(fuchsia::hardware::audio::SampleFormat::PCM_FLOAT);
  formats2.mutable_bytes_per_sample()->push_back(4);
  formats2.mutable_valid_bits_per_sample()->push_back(32);
  formats2.mutable_frame_rates()->push_back(16'000);
  formats2.mutable_frame_rates()->push_back(24'000);
  fmts.push_back(std::move(formats2));

  frames_per_second_inout = 0;
  channels_inout = 0;
  sample_format_inout = fuchsia::media::AudioSampleFormat::UNSIGNED_8;

  ASSERT_EQ(SelectBestFormat(fmts, &frames_per_second_inout, &channels_inout, &sample_format_inout),
            ZX_OK);
  ASSERT_EQ(sample_format_inout, fuchsia::media::AudioSampleFormat::FLOAT);  // Pick float.
  ASSERT_EQ(frames_per_second_inout, static_cast<uint32_t>(16'000));         // Pick closest.
  ASSERT_EQ(channels_inout, static_cast<uint32_t>(8));                       // Pick highest.
}

TEST(SelectBestFormatTest, SelectBestFormatOutsideRangesNewFidl) {
  std::vector<fuchsia_audio_device::PcmFormatSet> fmts;

  fuchsia_audio_device::PcmFormatSet formats;
  AddChannelSet(formats, 1);
  AddChannelSet(formats, 2);
  AddChannelSet(formats, 4);
  AddChannelSet(formats, 8);
  formats.sample_types({{fuchsia_audio::SampleType::kInt16}});
  formats.frame_rates({{16'000, 24'000, 48'000, 96'000}});
  fmts.push_back(std::move(formats));

  auto pref = media_audio::Format::CreateOrDie({
      .sample_type = fuchsia_audio::SampleType::kInt16,
      .channels = 3,
      .frames_per_second = 1,
  });

  auto result = SelectBestFormat(fmts, pref);
  ASSERT_TRUE(result.is_ok());
  EXPECT_EQ(result->sample_type(), fuchsia_audio::SampleType::kInt16);
  EXPECT_EQ(result->frames_per_second(), 16'000);  // pick closest available
  EXPECT_EQ(result->channels(), 2);                // prefer 2 channels

  pref = media_audio::Format::CreateOrDie({
      .sample_type = fuchsia_audio::SampleType::kUint8,
      .channels = 200,
      .frames_per_second = 192'000,
  });

  result = SelectBestFormat(fmts, pref);
  ASSERT_TRUE(result.is_ok());
  EXPECT_EQ(result->sample_type(), fuchsia_audio::SampleType::kInt16);
  EXPECT_EQ(result->frames_per_second(), 96'000);  // pick closest available
  EXPECT_EQ(result->channels(), 2);                // prefer 2 channels

  // Add a second format range.
  fuchsia_audio_device::PcmFormatSet formats2;
  AddChannelSet(formats2, 4);
  AddChannelSet(formats2, 5);
  AddChannelSet(formats2, 6);
  AddChannelSet(formats2, 7);
  AddChannelSet(formats2, 8);
  formats2.sample_types({{fuchsia_audio::SampleType::kFloat32}});
  formats2.frame_rates({{16'000, 24'000}});
  fmts.push_back(std::move(formats2));

  pref = media_audio::Format::CreateOrDie({
      .sample_type = fuchsia_audio::SampleType::kUint8,
      .channels = 3,
      .frames_per_second = 1,
  });

  result = SelectBestFormat(fmts, pref);
  ASSERT_TRUE(result.is_ok());
  EXPECT_EQ(result->sample_type(), fuchsia_audio::SampleType::kFloat32);  // pick float
  EXPECT_EQ(result->frames_per_second(), 16'000);                         // pick closest
  EXPECT_EQ(result->channels(), 8);                                       // pick highest
}

TEST(SelectBestFormatTest, SelectBestFormatError) {
  std::vector<fuchsia::hardware::audio::PcmSupportedFormats> fmts;

  fuchsia::hardware::audio::PcmSupportedFormats formats = {};
  AddChannelSet(formats, 1);
  AddChannelSet(formats, 2);
  AddChannelSet(formats, 4);
  AddChannelSet(formats, 8);
  formats.mutable_sample_formats()->push_back(fuchsia::hardware::audio::SampleFormat::PCM_FLOAT);
  formats.mutable_bytes_per_sample()->push_back(1);
  formats.mutable_valid_bits_per_sample()->push_back(32);
  formats.mutable_frame_rates()->push_back(8'000);
  formats.mutable_frame_rates()->push_back(16'000);
  formats.mutable_frame_rates()->push_back(24'000);
  formats.mutable_frame_rates()->push_back(48'000);
  formats.mutable_frame_rates()->push_back(96'000);
  formats.mutable_frame_rates()->push_back(192'000);
  formats.mutable_frame_rates()->push_back(384'000);
  formats.mutable_frame_rates()->push_back(768'000);
  fmts.push_back(std::move(formats));

  fuchsia::media::AudioSampleFormat sample_format_inout = {};  // Bad format
  uint32_t frames_per_second_inout = 0;
  uint32_t channels_inout = 0;

  ASSERT_EQ(SelectBestFormat(fmts, &frames_per_second_inout, &channels_inout, &sample_format_inout),
            ZX_ERR_INVALID_ARGS);

  sample_format_inout = fuchsia::media::AudioSampleFormat::SIGNED_16;

  ASSERT_EQ(SelectBestFormat(fmts, &frames_per_second_inout, nullptr,  // Bad pointer.
                             &sample_format_inout),
            ZX_ERR_INVALID_ARGS);
  ASSERT_EQ(SelectBestFormat(fmts, &frames_per_second_inout, &channels_inout, nullptr),
            ZX_ERR_INVALID_ARGS);  // Bad pointer.

  std::vector<fuchsia::hardware::audio::PcmSupportedFormats> empty_fmts;
  ASSERT_EQ(
      SelectBestFormat(empty_fmts, &frames_per_second_inout, &channels_inout, &sample_format_inout),
      ZX_ERR_NOT_SUPPORTED);

  // Test empty channel_sets vector (b/525552666)
  std::vector<fuchsia::hardware::audio::PcmSupportedFormats> empty_channel_sets_fmts;
  fuchsia::hardware::audio::PcmSupportedFormats bad_format = {};
  bad_format.mutable_sample_formats()->push_back(fuchsia::hardware::audio::SampleFormat::PCM_FLOAT);
  bad_format.mutable_bytes_per_sample()->push_back(4);
  bad_format.mutable_valid_bits_per_sample()->push_back(32);
  bad_format.mutable_frame_rates()->push_back(48'000);
  bad_format.mutable_channel_sets();
  empty_channel_sets_fmts.push_back(std::move(bad_format));
  ASSERT_EQ(SelectBestFormat(empty_channel_sets_fmts, &frames_per_second_inout, &channels_inout,
                             &sample_format_inout),
            ZX_ERR_NOT_SUPPORTED);
}

TEST(SelectBestFormatTest, SelectBestFormatErrorNewFidl) {
  // Most of the SelectBestFormatError test cases are impossible by construction. Only this
  // `empty_fmts` case is possible.
  std::vector<fuchsia_audio_device::PcmFormatSet> empty_fmts;
  auto pref = media_audio::Format::CreateOrDie({
      .sample_type = fuchsia_audio::SampleType::kInt16,
      .channels = 3,
      .frames_per_second = 1,
  });
  auto result = SelectBestFormat(empty_fmts, pref);
  ASSERT_FALSE(result.is_ok());
  EXPECT_EQ(result.error_value(), ZX_ERR_NOT_SUPPORTED);

  // Test empty channel_sets vector in PcmFormatSet (b/525552666)
  std::vector<fuchsia_audio_device::PcmFormatSet> empty_channel_sets_fmts;
  fuchsia_audio_device::PcmFormatSet bad_format = {};
  bad_format.sample_types() =
      std::vector<fuchsia_audio::SampleType>{fuchsia_audio::SampleType::kFloat32};
  bad_format.frame_rates() = std::vector<uint32_t>{48'000};
  bad_format.channel_sets() = std::vector<fuchsia_audio_device::ChannelSet>{};
  empty_channel_sets_fmts.push_back(std::move(bad_format));
  result = SelectBestFormat(empty_channel_sets_fmts, pref);
  ASSERT_FALSE(result.is_ok());
  EXPECT_EQ(result.error_value(), ZX_ERR_NOT_SUPPORTED);
}

}  // namespace
}  // namespace media::audio

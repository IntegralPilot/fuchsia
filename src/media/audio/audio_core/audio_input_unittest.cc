// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/audio_core/audio_input.h"

#include "src/media/audio/audio_core/audio_device_manager.h"
#include "src/media/audio/audio_core/audio_driver.h"
#include "src/media/audio/audio_core/loudness_transform.h"
#include "src/media/audio/audio_core/testing/fake_audio_driver.h"
#include "src/media/audio/audio_core/testing/threading_model_fixture.h"

namespace media::audio {
namespace {

constexpr int64_t kRingBufferSizePages = 8;

class AudioInputTest : public testing::ThreadingModelFixture,
                       public ::testing::WithParamInterface<int32_t> {
 protected:
  AudioInputTest()
      : ThreadingModelFixture(
            ProcessConfig::Builder()
                .AddDeviceProfile(
                    {std::nullopt, DeviceConfig::InputDeviceProfile(
                                       GetParam(), /*driver_gain_db=*/0, /*software_gain_db=*/0)})
                .SetDefaultVolumeCurve(
                    VolumeCurve::DefaultForMinGain(VolumeCurve::kDefaultGainForMinVolume))
                .Build()) {}

  void SetUp() override {
    ThreadingModelFixture::SetUp();
    zx::channel c1, c2;
    ASSERT_EQ(ZX_OK, zx::channel::create(0, &c1, &c2));

    remote_driver_ = std::make_unique<testing::FakeAudioDriver>(
        std::move(c1), threading_model().FidlDomain().dispatcher());
    ASSERT_NE(remote_driver_, nullptr);

    fidl::InterfaceHandle<fuchsia::hardware::audio::StreamConfig> stream_config = {};
    stream_config.set_channel(std::move(c2));
    input_ =
        AudioInput::Create("", context().process_config().device_config(), std::move(stream_config),
                           &threading_model(), &context().device_manager(),
                           &context().link_matrix(), context().clock_factory());
    ASSERT_NE(input_, nullptr);

    ring_buffer_mapper_ =
        remote_driver_->CreateRingBuffer(kRingBufferSizePages * zx_system_get_page_size());
    ASSERT_NE(ring_buffer_mapper_.start(), nullptr);
  }

  void RunLoopUntilFormatConfigured() {
    int run_loop_count = 0;
    while (input_->driver()->GetFormat() == std::nullopt && run_loop_count < 100) {
      RunLoopFor(zx::msec(5));
      ++run_loop_count;
    }
  }

  std::unique_ptr<testing::FakeAudioDriver> remote_driver_;
  std::shared_ptr<AudioInput> input_;
  fzl::VmoMapper ring_buffer_mapper_;
};

TEST_P(AudioInputTest, RequestHardwareRateInConfigIfSupported) {
  // Publish a format that has a matching sample rate, and also formats with double and half the
  // requested rate.
  fuchsia::hardware::audio::PcmSupportedFormats formats = {};
  fuchsia::hardware::audio::ChannelSet channel_set = {};
  constexpr size_t kSupportedNumberOfChannels = 1;
  std::vector<fuchsia::hardware::audio::ChannelAttributes> attributes(kSupportedNumberOfChannels);
  channel_set.set_attributes(std::move(attributes));
  formats.mutable_channel_sets()->push_back(std::move(channel_set));
  formats.mutable_sample_formats()->push_back(fuchsia::hardware::audio::SampleFormat::PCM_SIGNED);
  formats.mutable_bytes_per_sample()->push_back(2);
  formats.mutable_valid_bits_per_sample()->push_back(16);
  formats.mutable_frame_rates()->push_back(GetParam());
  formats.mutable_frame_rates()->push_back(2 * GetParam());
  formats.mutable_frame_rates()->push_back(GetParam() / 2);
  remote_driver_->set_formats(std::move(formats));

  remote_driver_->Start();
  threading_model().FidlDomain().ScheduleTask(input_->Startup());
  RunLoopUntilFormatConfigured();

  auto format = input_->driver()->GetFormat();
  ASSERT_TRUE(format);
  ASSERT_EQ(format->frames_per_second(), GetParam());
}

TEST_P(AudioInputTest, FallBackToAlternativeRateIfPreferredRateIsNotSupported) {
  ASSERT_NE(GetParam(), 0);  // Invalid frame rate passed as test parameter.
  const int32_t kSupportedRate = GetParam() * 2;
  fuchsia::hardware::audio::PcmSupportedFormats formats = {};
  fuchsia::hardware::audio::ChannelSet channel_set = {};
  constexpr size_t kSupportedNumberOfChannels = 1;
  std::vector<fuchsia::hardware::audio::ChannelAttributes> attributes(kSupportedNumberOfChannels);
  channel_set.set_attributes(std::move(attributes));
  formats.mutable_channel_sets()->push_back(std::move(channel_set));
  formats.mutable_sample_formats()->push_back(fuchsia::hardware::audio::SampleFormat::PCM_SIGNED);
  formats.mutable_bytes_per_sample()->push_back(2);
  formats.mutable_valid_bits_per_sample()->push_back(16);
  formats.mutable_frame_rates()->push_back(kSupportedRate);
  remote_driver_->set_formats(std::move(formats));

  remote_driver_->Start();
  threading_model().FidlDomain().ScheduleTask(input_->Startup());
  RunLoopUntilFormatConfigured();

  auto format = input_->driver()->GetFormat();
  ASSERT_TRUE(format);
  ASSERT_EQ(format->frames_per_second(), kSupportedRate);
}

// Verify calling SetGainInfo on an AudioInput after activation but before reporter initialization.
TEST_P(AudioInputTest, SetGainInfoBeforeReporterInitialized) {
  fuchsia::hardware::audio::PcmSupportedFormats formats = {};
  fuchsia::hardware::audio::ChannelSet channel_set = {};
  std::vector<fuchsia::hardware::audio::ChannelAttributes> attributes(1);
  channel_set.set_attributes(std::move(attributes));
  formats.mutable_channel_sets()->push_back(std::move(channel_set));
  formats.mutable_sample_formats()->push_back(fuchsia::hardware::audio::SampleFormat::PCM_SIGNED);
  formats.mutable_bytes_per_sample()->push_back(2);
  formats.mutable_valid_bits_per_sample()->push_back(16);
  formats.mutable_frame_rates()->push_back(GetParam());
  remote_driver_->set_formats(std::move(formats));
  remote_driver_->Start();

  // Startup: OnDriverInfoFetched runs, ActivateSelf() initializes device_settings_. Driver ring
  // buffer is never started so OnDriverStartComplete never runs: reporter_ remains uninitialized.
  threading_model().FidlDomain().ScheduleTask(input_->Startup());
  RunLoopUntilFormatConfigured();

  fuchsia::media::AudioGainInfo gain_info;
  gain_info.flags = fuchsia::media::AudioGainInfoFlags::MUTE;
  static_cast<AudioDevice*>(input_.get())
      ->SetGainInfo(gain_info, fuchsia::media::AudioGainValidFlags::MUTE_VALID);
  RunLoopUntilIdle();
}

INSTANTIATE_TEST_SUITE_P(AudioInputTestInstance, AudioInputTest,
                         ::testing::Values(24000, 48000, 96000),
                         [](const ::testing::TestParamInfo<AudioInputTest::ParamType>& info) {
                           return std::to_string(info.param);
                         });

}  // namespace
}  // namespace media::audio

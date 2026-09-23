// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "max98373.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/mock-i2c/mock-i2c-gtest.h>
#include <lib/simple-codec/simple-codec-client.h>
#include <lib/simple-codec/simple-codec-helper.h>

#include <gtest/gtest.h>

#include "src/devices/gpio/testing/fake-gpio/fake-gpio.h"
#include "src/devices/testing/mock-ddk/mock-device.h"
#include "src/lib/testing/predicates/status.h"

namespace audio {

struct Max98373Codec : public Max98373 {
  explicit Max98373Codec(ddk::I2cChannel i2c,
                         fidl::ClientEnd<fuchsia_hardware_gpio::Gpio> codec_reset,
                         zx_device_t* parent)
      : Max98373(parent, std::move(i2c), std::move(codec_reset)) {}
  zx::result<fidl::ClientEnd<fuchsia_hardware_audio::Codec>> GetClient() {
    zx::channel channel_remote;
    fidl::ClientEnd<fuchsia_hardware_audio::Codec> channel_local;
    zx_status_t status = zx::channel::create(0, &channel_local.channel(), &channel_remote);
    if (status != ZX_OK) {
      return zx::error(status);
    }
    status = CodecConnect(std::move(channel_remote));
    if (status != ZX_OK) {
      return zx::error(status);
    }
    return zx::success(std::move(channel_local));
  }
};

class Max98373Test : public ::testing::Test {
 public:
  void SetUp() override {
    // Reset by the TAS driver initialization.
    mock_i2c_
        .ExpectWriteStop({0x20, 0x00, 0x01}, ZX_ERR_INTERNAL)  // Reset, error will retry.
        .ExpectWriteStop({0x20, 0x00, 0x01}, ZX_ERR_INTERNAL)  // Reset, error will retry.
        .ExpectWriteStop({0x20, 0x00, 0x01})                   // Reset.
        .ExpectWrite({0x21, 0xff})
        .ExpectReadStop({0x43})                // Get revision id.
        .ExpectWriteStop({0x20, 0xff, 0x01})   // Global enable.
        .ExpectWriteStop({0x20, 0x43, 0x01})   // Speaker enable.
        .ExpectWriteStop({0x20, 0x3d, 0x28})   // Set digital gain to -20dB.
        .ExpectWriteStop({0x20, 0x3e, 0x05})   // Set analog gain to +13dB.
        .ExpectWriteStop({0x20, 0x2b, 0x01});  // Data in enable.

    fidl_servers_loop_.StartThread("fidl-servers");

    fake_root_ = MockDevice::FakeRootParent();
    fake_gpio_.SyncCall(&fake_gpio::FakeGpio::SetCurrentState,
                        fake_gpio::State{.sub_state = fake_gpio::WriteSubState{.value = 0}});
    fidl::ClientEnd codec_reset = fake_gpio_.SyncCall(&fake_gpio::FakeGpio::Connect);
    ASSERT_OK(SimpleCodecServer::CreateAndAddToDdk<Max98373Codec>(
        GetI2cClient(), std::move(codec_reset), fake_root_.get()));
    ASSERT_EQ(1,
              fake_gpio_.SyncCall(
                  &fake_gpio::FakeGpio::GetWriteValue));  // Reset, set to 0 and then to 1.
    auto* child_dev = fake_root_->GetLatestChild();
    auto codec = child_dev->GetDeviceContext<Max98373Codec>();
    zx::result<fidl::ClientEnd<fuchsia_hardware_audio::Codec>> codec_client = codec->GetClient();
    ASSERT_OK(codec_client.status_value());
    client_.SetCodec(std::move(*codec_client));
  }

  void TearDown() override {
    auto* child_dev = fake_root_->GetLatestChild();
    child_dev->UnbindOp();
    mock_i2c_.VerifyAndClear();
  }

  fidl::ClientEnd<fuchsia_hardware_i2c::Device> GetI2cClient() {
    auto endpoints = fidl::CreateEndpoints<fuchsia_hardware_i2c::Device>();
    if (endpoints.is_error()) {
      return {};
    }

    fidl::BindServer(fidl_servers_loop_.dispatcher(), std::move(endpoints->server), &mock_i2c_);
    return std::move(endpoints->client);
  }

 protected:
  async::Loop fidl_servers_loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
  mock_i2c::MockI2cGtest mock_i2c_;
  async_patterns::TestDispatcherBound<fake_gpio::FakeGpio> fake_gpio_{
      fidl_servers_loop_.dispatcher(), std::in_place};
  SimpleCodecClient client_;
  std::shared_ptr<MockDevice> fake_root_;
};

TEST_F(Max98373Test, GetInfo) {
  auto info = client_.GetInfo();
  EXPECT_FALSE(info.value().unique_id.has_value());
  EXPECT_EQ(info.value().manufacturer.compare("Maxim"), 0);
  EXPECT_EQ(info.value().product_name.compare("MAX98373"), 0);
}

TEST_F(Max98373Test, GetDaiFormats) {
  auto formats = client_.GetDaiFormats();
  EXPECT_EQ(formats.value().number_of_channels.size(), 4u);
  EXPECT_EQ(formats.value().number_of_channels[0], 2u);
  EXPECT_EQ(formats.value().number_of_channels[1], 4u);
  EXPECT_EQ(formats.value().number_of_channels[2], 8u);
  EXPECT_EQ(formats.value().number_of_channels[3], 16u);
  EXPECT_EQ(formats.value().sample_formats.size(), 1u);
  EXPECT_EQ(formats.value().sample_formats[0], SampleFormat::PCM_SIGNED);
  EXPECT_EQ(formats.value().frame_formats.size(), 3u);
  EXPECT_EQ(formats.value().frame_formats[0], FrameFormat::TDM1);
  EXPECT_EQ(formats.value().frame_formats[1], FrameFormat::I2S);
  EXPECT_EQ(formats.value().frame_formats[2], FrameFormat::STEREO_LEFT);
  EXPECT_EQ(formats.value().frame_rates.size(), 8u);
  EXPECT_EQ(formats.value().frame_rates[0], 16'000u);
  EXPECT_EQ(formats.value().frame_rates[1], 22'050u);
  EXPECT_EQ(formats.value().frame_rates[2], 24'000u);
  EXPECT_EQ(formats.value().frame_rates[3], 32'000u);
  EXPECT_EQ(formats.value().frame_rates[4], 44'100u);
  EXPECT_EQ(formats.value().frame_rates[5], 48'000u);
  EXPECT_EQ(formats.value().frame_rates[6], 88'200u);
  EXPECT_EQ(formats.value().frame_rates[7], 96'000u);
  EXPECT_EQ(formats.value().bits_per_slot.size(), 3u);
  EXPECT_EQ(formats.value().bits_per_slot[0], 16u);
  EXPECT_EQ(formats.value().bits_per_slot[1], 24u);
  EXPECT_EQ(formats.value().bits_per_slot[2], 32u);
  EXPECT_EQ(formats.value().bits_per_sample.size(), 3u);
  EXPECT_EQ(formats.value().bits_per_sample[0], 16u);
  EXPECT_EQ(formats.value().bits_per_sample[1], 24u);
  EXPECT_EQ(formats.value().bits_per_sample[2], 32u);
}

TEST_F(Max98373Test, Reset) {
  mock_i2c_
      .ExpectWriteStop({0x20, 0x00, 0x01}, ZX_ERR_INTERNAL)  // Reset, error will retry.
      .ExpectWriteStop({0x20, 0x00, 0x01}, ZX_ERR_INTERNAL)  // Reset, error will retry.
      .ExpectWriteStop({0x20, 0x00, 0x01})                   // Reset.
      .ExpectWrite({0x21, 0xff})
      .ExpectReadStop({0x43})                // Get revision id.
      .ExpectWriteStop({0x20, 0xff, 0x01})   // Global enable.
      .ExpectWriteStop({0x20, 0x43, 0x01})   // Speaker enable.
      .ExpectWriteStop({0x20, 0x3d, 0x28})   // Set digital gain to -20dB.
      .ExpectWriteStop({0x20, 0x3e, 0x05})   // Set analog gain to +13dB.
      .ExpectWriteStop({0x20, 0x2b, 0x01});  // Data in enable.

  ASSERT_OK(client_.Reset());
}

TEST_F(Max98373Test, SetDaiFormat) {
  // Good.
  {
    audio::DaiFormat format = {};
    format.number_of_channels = 2;
    format.channels_to_use_bitmask = 2;
    format.sample_format = SampleFormat::PCM_SIGNED;
    format.frame_format = FrameFormat::I2S;
    format.frame_rate = 16'000;
    format.bits_per_slot = 32;
    format.bits_per_sample = 16;
    mock_i2c_
        .ExpectWriteStop({0x20, 0x29, 0x01})   // Slot 1.
        .ExpectWriteStop({0x20, 0x26, 0x04})   // 64 ratio.
        .ExpectWriteStop({0x20, 0x24, 0x40})   // I2S 16 bits.
        .ExpectWriteStop({0x20, 0x27, 0x03});  // 16KHz
    auto formats = client_.GetDaiFormats();
    ASSERT_TRUE(IsDaiFormatSupported(format, formats.value()));
    ASSERT_OK(client_.SetDaiFormat(std::move(format)));
  }
  // Good.
  {
    audio::DaiFormat format = {};
    format.number_of_channels = 2;
    format.channels_to_use_bitmask = 2;
    format.sample_format = SampleFormat::PCM_SIGNED;
    format.frame_format = FrameFormat::STEREO_LEFT;
    format.frame_rate = 88'200;
    format.bits_per_slot = 32;
    format.bits_per_sample = 24;
    mock_i2c_
        .ExpectWriteStop({0x20, 0x29, 0x01})   // Slot 1.
        .ExpectWriteStop({0x20, 0x26, 0x04})   // 64 ratio.
        .ExpectWriteStop({0x20, 0x24, 0x88})   // Left justification 24 bits.
        .ExpectWriteStop({0x20, 0x27, 0x09});  // 88.1KHz
    auto formats = client_.GetDaiFormats();
    ASSERT_TRUE(IsDaiFormatSupported(format, formats.value()));
    ASSERT_OK(client_.SetDaiFormat(std::move(format)));
  }
  // Bad.
  {
    audio::DaiFormat format = {};
    format.number_of_channels = 2;
    format.channels_to_use_bitmask = 2;
    format.sample_format = SampleFormat::PCM_SIGNED;
    format.frame_format = FrameFormat::STEREO_LEFT;
    format.frame_rate = 88'200;
    format.bits_per_slot = 16;
    format.bits_per_sample = 32;  // 32 bits into 16 is not valid.
    auto formats = client_.GetDaiFormats();
    ASSERT_FALSE(IsDaiFormatSupported(format, formats.value()));
    ASSERT_TRUE(client_.SetDaiFormat(std::move(format)).is_error());
  }
}

TEST_F(Max98373Test, SetDaiFormatTdmSlot) {
  // Slot 0 ok.
  {
    audio::DaiFormat format = {};
    format.number_of_channels = 8;
    format.channels_to_use_bitmask = 1;
    format.sample_format = SampleFormat::PCM_SIGNED;
    format.frame_format = FrameFormat::TDM1;
    format.frame_rate = 32'000;
    format.bits_per_slot = 16;
    format.bits_per_sample = 16;
    mock_i2c_
        .ExpectWriteStop({0x20, 0x29, 0x00})   // Slot 0.
        .ExpectWriteStop({0x20, 0x26, 0x06})   // 128 ratio for 8 x 16 bits.
        .ExpectWriteStop({0x20, 0x24, 0x58})   // TDM 16 bits.
        .ExpectWriteStop({0x20, 0x27, 0x06});  // 32KHz
    auto formats = client_.GetDaiFormats();
    ASSERT_TRUE(IsDaiFormatSupported(format, formats.value()));
    ASSERT_OK(client_.SetDaiFormat(std::move(format)));
  }

  // Slot 1 ok.
  {
    audio::DaiFormat format = {};
    format.number_of_channels = 8;
    format.channels_to_use_bitmask = 2;
    format.sample_format = SampleFormat::PCM_SIGNED;
    format.frame_format = FrameFormat::TDM1;
    format.frame_rate = 44'100;
    format.bits_per_slot = 32;
    format.bits_per_sample = 24;
    mock_i2c_
        .ExpectWriteStop({0x20, 0x29, 0x01})   // Slot 1.
        .ExpectWriteStop({0x20, 0x26, 0x08})   // 256 ratio for 8 x 32 bits.
        .ExpectWriteStop({0x20, 0x24, 0x98})   // TDM 24 bits.
        .ExpectWriteStop({0x20, 0x27, 0x07});  // 44.1KHz
    auto formats = client_.GetDaiFormats();
    ASSERT_TRUE(IsDaiFormatSupported(format, formats.value()));
    ASSERT_OK(client_.SetDaiFormat(std::move(format)));
  }

  // Slot 2 ok.
  {
    audio::DaiFormat format = {};
    format.number_of_channels = 4;
    format.channels_to_use_bitmask = 4;
    format.sample_format = SampleFormat::PCM_SIGNED;
    format.frame_format = FrameFormat::TDM1;
    format.frame_rate = 44'100;
    format.bits_per_slot = 32;
    format.bits_per_sample = 24;
    mock_i2c_
        .ExpectWriteStop({0x20, 0x29, 0x02})   // Slot 2.
        .ExpectWriteStop({0x20, 0x26, 0x06})   // 128 ratio for 4 x 32 bits.
        .ExpectWriteStop({0x20, 0x24, 0x98})   // TDM 24 bits.
        .ExpectWriteStop({0x20, 0x27, 0x07});  // 44.1KHz
    auto formats = client_.GetDaiFormats();
    ASSERT_TRUE(IsDaiFormatSupported(format, formats.value()));
    ASSERT_OK(client_.SetDaiFormat(std::move(format)));
  }

  // Slot 15 ok.
  {
    audio::DaiFormat format = {};
    format.number_of_channels = 16;
    format.channels_to_use_bitmask = 0x8000;
    format.sample_format = SampleFormat::PCM_SIGNED;
    format.frame_format = FrameFormat::TDM1;
    format.frame_rate = 24'000;
    format.bits_per_slot = 16;
    format.bits_per_sample = 16;
    mock_i2c_
        .ExpectWriteStop({0x20, 0x29, 0x0f})   // Slot 15.
        .ExpectWriteStop({0x20, 0x26, 0x08})   // 256 ratio for 16 slots x 16 bits.
        .ExpectWriteStop({0x20, 0x24, 0x58})   // TDM 16 bits.
        .ExpectWriteStop({0x20, 0x27, 0x05});  // 24KHz
    auto formats = client_.GetDaiFormats();
    ASSERT_TRUE(IsDaiFormatSupported(format, formats.value()));
    ASSERT_OK(client_.SetDaiFormat(std::move(format)));
  }

  // Multiple slots not supported.
  {
    audio::DaiFormat format = {};
    format.number_of_channels = 8;
    format.channels_to_use_bitmask = 0x8080;
    format.sample_format = SampleFormat::PCM_SIGNED;
    format.frame_format = FrameFormat::TDM1;
    format.frame_rate = 48'000;
    format.bits_per_slot = 16;
    format.bits_per_sample = 16;
    auto formats = client_.GetDaiFormats();
    ASSERT_TRUE(IsDaiFormatSupported(format, formats.value()));
    zx::result<CodecFormatInfo> format_info = client_.SetDaiFormat(std::move(format));
    EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, format_info.status_value());
  }

  // Slot 16 not supported.
  {
    audio::DaiFormat format = {};
    format.number_of_channels = 8;
    format.channels_to_use_bitmask = 0x1'0000;
    format.sample_format = SampleFormat::PCM_SIGNED;
    format.frame_format = FrameFormat::TDM1;
    format.frame_rate = 48'000;
    format.bits_per_slot = 16;
    format.bits_per_sample = 16;
    auto formats = client_.GetDaiFormats();
    ASSERT_TRUE(IsDaiFormatSupported(format, formats.value()));
    zx::result<CodecFormatInfo> format_info = client_.SetDaiFormat(std::move(format));
    EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, format_info.status_value());
  }
}

TEST_F(Max98373Test, SetGainGood) {
  mock_i2c_.ExpectWriteStop({0x20, 0x3d, 0x40});  // -32dB.

  GainState gain({.gain = -32.f, .muted = false, .agc_enabled = false});
  client_.SetGainState(gain);

  // Make a 2-way call to make sure the server (we know single threaded) completed previous calls.
  auto unused = client_.GetInfo();
  static_cast<void>(unused);
}

TEST_F(Max98373Test, SetGainOurOfRangeLow) {
  mock_i2c_.ExpectWriteStop({0x20, 0x3d, 0x7f});  // -63.5dB.

  GainState gain({.gain = -999.f, .muted = false, .agc_enabled = false});
  client_.SetGainState(gain);

  // Make a 2-way call to make sure the server (we know single threaded) completed previous calls.
  auto unused = client_.GetInfo();
  static_cast<void>(unused);
}

TEST_F(Max98373Test, SetGainOurOfRangeHigh) {
  mock_i2c_.ExpectWriteStop({0x20, 0x3d, 0x00});  // 0dB.

  GainState gain({.gain = 999.f, .muted = false, .agc_enabled = false});
  client_.SetGainState(gain);

  // Make a 2-way call to make sure the server (we know single threaded) completed previous calls.
  auto unused = client_.GetInfo();
  static_cast<void>(unused);
}

}  // namespace audio

// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tas5720.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/ddk/metadata.h>
#include <lib/fpromise/single_threaded_executor.h>
#include <lib/inspect/cpp/hierarchy.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/inspect/cpp/reader.h>
#include <lib/inspect/testing/cpp/inspect.h>
#include <lib/mock-i2c/mock-i2c-gtest.h>
#include <lib/simple-codec/simple-codec-client.h>
#include <lib/simple-codec/simple-codec-helper.h>
#include <lib/sync/completion.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/devices/testing/mock-ddk/mock-device.h"
#include "src/lib/testing/predicates/status.h"

namespace audio {

audio::DaiFormat GetDefaultDaiFormat() {
  return {
      .number_of_channels = 2,
      .channels_to_use_bitmask = 1,  // Use left channel in this mono codec.
      .sample_format = SampleFormat::PCM_SIGNED,
      .frame_format = FrameFormat::STEREO_LEFT,
      .frame_rate = 24'000,
      .bits_per_slot = 32,
      .bits_per_sample = 16,
  };
}

struct Tas5720Test : public ::testing::Test {
  void SetUp() override {
    // Reset by the TAS driver initialization.
    mock_i2c_.ExpectWrite({0x01})
        .ExpectReadStop({0xff})
        .ExpectWriteStop({0x01, 0xfe})  // Enter shutdown (part of reset).
        .ExpectWrite({0x01})
        .ExpectReadStop({0xfe})
        .ExpectWriteStop({0x01, 0xff})  // Exit shutdown (part of reset).
        .ExpectWrite({0x01})
        .ExpectReadStop({0xff})
        .ExpectWriteStop({0x01, 0xfe})  // Enter shutdown (part of stop).
        .ExpectWriteStop({0x02, 0x45})  // Digital control defaults. Left justified.
        .ExpectWriteStop({0x03, 0x90})  // Digital control defaults. Slot 0, muted.
        .ExpectWriteStop({0x06, 0x5d})  // Analog defaults.
        .ExpectWriteStop({0x10, 0xff})  // clippers disabled.
        .ExpectWriteStop({0x11, 0xfc})  // clippers disabled.
        .ExpectWrite({0x01})
        .ExpectReadStop({0xfe})
        .ExpectWriteStop({0x01, 0xff})  // exit shutdown (part of start).
        .ExpectWriteStop({0x06, 0x5d})  // Default gain.
        .ExpectWriteStop({0x04, 0xcf})  // Default gain.
        .ExpectWrite({0x03})
        .ExpectReadStop({0x80})
        .ExpectWriteStop({0x03, 0x90});  // Muted.

    loop_.StartThread();
  }

  fidl::ClientEnd<fuchsia_hardware_i2c::Device> GetI2cClient() {
    auto endpoints = fidl::CreateEndpoints<fuchsia_hardware_i2c::Device>();
    if (endpoints.is_error()) {
      return {};
    }

    fidl::BindServer(loop_.dispatcher(), std::move(endpoints->server), &mock_i2c_);
    return std::move(endpoints->client);
  }

  mock_i2c::MockI2cGtest mock_i2c_;
  async::Loop loop_{&kAsyncLoopConfigNeverAttachToThread};
};

struct Tas5720Codec : public Tas5720 {
  explicit Tas5720Codec(zx_device_t* parent, ddk::I2cChannel i2c)
      : Tas5720(parent, std::move(i2c)) {}
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
  inspect::Inspector& inspect() { return Tas5720::inspect(); }
  void PollFaults(bool is_periodic) { return Tas5720::PollFaults(is_periodic); }
  bool PeriodicFaultPollingDisabledForTests() override { return true; }
};

TEST_F(Tas5720Test, CodecInitGood) {
  auto fake_parent = MockDevice::FakeRootParent();
  uint32_t instance_count = 0;
  fake_parent->SetMetadata(DEVICE_METADATA_PRIVATE, &instance_count, sizeof(instance_count));

  ASSERT_OK(SimpleCodecServer::CreateAndAddToDdk<Tas5720Codec>(fake_parent.get(), GetI2cClient()));
  auto* child_dev = fake_parent->GetLatestChild();
  ASSERT_TRUE(child_dev);

  // Shutdown.
  mock_i2c_.ExpectWrite({0x08}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({0x01}).ExpectReadStop({0xff}).ExpectWriteStop({0x01, 0xfe});

  child_dev->ReleaseOp();
  mock_i2c_.VerifyAndClear();
}

TEST(Tas5720CustomEnvTest, CodecInitBad) {
  auto fake_parent = MockDevice::FakeRootParent();
  uint32_t instance_count = 0;
  fake_parent->SetMetadata(DEVICE_METADATA_PRIVATE, &instance_count, sizeof(instance_count));

  mock_i2c::MockI2cGtest mock_i2c;

  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);

  // Bad replies (2 retries) to enter shutdown (part of reset).
  mock_i2c.ExpectWrite({0x01}).ExpectReadStop({0xff}, ZX_ERR_TIMED_OUT);
  mock_i2c.ExpectWrite({0x01}).ExpectReadStop({0xff}, ZX_ERR_TIMED_OUT);  // Retry 1.
  mock_i2c.ExpectWrite({0x01}).ExpectReadStop({0xff}, ZX_ERR_TIMED_OUT);  // Retry 2.
  // Bad replies (2 retries) to enter shutdown (part of shutdown becuase init failed).
  mock_i2c.ExpectWrite({0x01}).ExpectReadStop({0xff}, ZX_ERR_TIMED_OUT);
  mock_i2c.ExpectWrite({0x01}).ExpectReadStop({0xff}, ZX_ERR_TIMED_OUT);  // Retry 1.
  mock_i2c.ExpectWrite({0x01}).ExpectReadStop({0xff}, ZX_ERR_TIMED_OUT);  // Retry 2.

  auto endpoints = fidl::Endpoints<fuchsia_hardware_i2c::Device>::Create();

  fidl::BindServer(loop.dispatcher(), std::move(endpoints.server), &mock_i2c);
  loop.StartThread();

  ASSERT_EQ(ZX_ERR_TIMED_OUT, SimpleCodecServer::CreateAndAddToDdk<Tas5720Codec>(
                                  fake_parent.get(), std::move(endpoints.client)));

  mock_i2c.VerifyAndClear();
}

TEST_F(Tas5720Test, CodecGetInfo) {
  auto fake_parent = MockDevice::FakeRootParent();
  uint32_t instance_count = 0;
  fake_parent->SetMetadata(DEVICE_METADATA_PRIVATE, &instance_count, sizeof(instance_count));

  ASSERT_OK(SimpleCodecServer::CreateAndAddToDdk<Tas5720Codec>(fake_parent.get(), GetI2cClient()));
  auto* child_dev = fake_parent->GetLatestChild();
  ASSERT_TRUE(child_dev);
  auto codec = child_dev->GetDeviceContext<Tas5720Codec>();
  zx::result<fidl::ClientEnd<fuchsia_hardware_audio::Codec>> codec_client = codec->GetClient();
  ASSERT_OK(codec_client.status_value());
  SimpleCodecClient client;
  client.SetCodec(std::move(*codec_client));

  auto info = client.GetInfo();
  ASSERT_FALSE(info->unique_id.has_value());
  ASSERT_EQ(info->manufacturer.compare("Texas Instruments"), 0);
  ASSERT_EQ(info->product_name.compare("TAS5720"), 0);

  // Shutdown.
  mock_i2c_.ExpectWrite({0x08}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({0x01}).ExpectReadStop({0xff}).ExpectWriteStop({0x01, 0xfe});

  child_dev->ReleaseOp();
  mock_i2c_.VerifyAndClear();
}

TEST_F(Tas5720Test, CodecReset) {
  auto fake_parent = MockDevice::FakeRootParent();
  uint32_t instance_count = 0;
  fake_parent->SetMetadata(DEVICE_METADATA_PRIVATE, &instance_count, sizeof(instance_count));

  // We complete all i2c mock setup before executing server methods in a different thread.
  // Reset by the call to Reset.
  mock_i2c_.ExpectWrite({0x08})
      .ExpectReadStop({0x00})  // Poll for faults (part of reset).
      .ExpectWrite({0x01})
      .ExpectReadStop({0xff})
      .ExpectWriteStop({0x01, 0xfe})  // Enter shutdown (part of reset).
      .ExpectWrite({0x01})
      .ExpectReadStop({0xfe})
      .ExpectWriteStop({0x01, 0xff})  // Exit shutdown (part of reset).
      .ExpectWrite({0x08})
      .ExpectReadStop({0x00})  // Poll for faults (part of stop).
      .ExpectWrite({0x01})
      .ExpectReadStop({0xff})
      .ExpectWriteStop({0x01, 0xfe})  // Enter shutdown (part of stop).
      .ExpectWriteStop({0x02, 0x45})  // Digital control defaults. TODO set I2S.
      .ExpectWriteStop({0x03, 0x90})  // Digital control defaults. Slot 0, muted.
      .ExpectWriteStop({0x06, 0x5d})  // Analog defaults.
      .ExpectWriteStop({0x10, 0xff})  // clippers disabled.
      .ExpectWriteStop({0x11, 0xfc})  // clippers disabled.
      .ExpectWrite({0x01})
      .ExpectReadStop({0xfe})
      .ExpectWriteStop({0x01, 0xff})  // exit shutdown (part of start).
      .ExpectWriteStop({0x06, 0x5d})  // Default gain.
      .ExpectWriteStop({0x04, 0xcf})  // Default gain.
      .ExpectWrite({0x03})
      .ExpectReadStop({0x80})
      .ExpectWriteStop({0x03, 0x90});  // Muted.

  mock_i2c_.ExpectWrite({0x08}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({0x01}).ExpectReadStop({0xff}).ExpectWriteStop({0x01, 0xfe});  // Shutdown.

  ASSERT_OK(SimpleCodecServer::CreateAndAddToDdk<Tas5720Codec>(fake_parent.get(), GetI2cClient()));
  auto* child_dev = fake_parent->GetLatestChild();
  ASSERT_TRUE(child_dev);
  auto codec = child_dev->GetDeviceContext<Tas5720Codec>();
  zx::result<fidl::ClientEnd<fuchsia_hardware_audio::Codec>> codec_client = codec->GetClient();
  ASSERT_OK(codec_client.status_value());
  SimpleCodecClient client;
  client.SetCodec(std::move(*codec_client));
  ASSERT_OK(client.Reset());

  child_dev->ReleaseOp();
  mock_i2c_.VerifyAndClear();
}

TEST_F(Tas5720Test, CodecDaiFormat) {
  auto fake_parent = MockDevice::FakeRootParent();
  uint32_t instance_count = 0;
  fake_parent->SetMetadata(DEVICE_METADATA_PRIVATE, &instance_count, sizeof(instance_count));

  ASSERT_OK(SimpleCodecServer::CreateAndAddToDdk<Tas5720Codec>(fake_parent.get(), GetI2cClient()));
  auto* child_dev = fake_parent->GetLatestChild();
  ASSERT_TRUE(child_dev);
  auto codec = child_dev->GetDeviceContext<Tas5720Codec>();
  zx::result<fidl::ClientEnd<fuchsia_hardware_audio::Codec>> codec_client = codec->GetClient();
  ASSERT_OK(codec_client.status_value());
  SimpleCodecClient client;
  client.SetCodec(std::move(*codec_client));

  // We complete all i2c mock setup before executing server methods in a different thread.
  mock_i2c_.ExpectWrite({0x03}).ExpectReadStop({0xff});
  mock_i2c_.ExpectWriteStop({0x03, 0xfc});  // Set slot to 0.
  mock_i2c_.ExpectWriteStop({0x02, 0x45});  // Set rate to 48kHz.

  mock_i2c_.ExpectWrite({0x03}).ExpectReadStop({0xff});
  mock_i2c_.ExpectWriteStop({0x03, 0xfd});  // Set slot to 1.
  mock_i2c_.ExpectWriteStop({0x02, 0x45});  // Set rate to 48kHz.

  mock_i2c_.ExpectWrite({0x03}).ExpectReadStop({0xff});
  mock_i2c_.ExpectWriteStop({0x03, 0xfc});  // Set slot to 0.
  mock_i2c_.ExpectWriteStop({0x02, 0x4d});  // Set rate to 96kHz.

  mock_i2c_.ExpectWrite({0x03}).ExpectReadStop({0xff});
  mock_i2c_.ExpectWriteStop({0x03, 0xfc});  // Set slot to 0.

  mock_i2c_.ExpectWrite({0x08}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({0x01}).ExpectReadStop({0xff}).ExpectWriteStop({0x01, 0xfe});  // Shutdown.

  // Check getting DAI formats.
  {
    auto formats = client.GetDaiFormats();
    ASSERT_EQ(formats.value().number_of_channels.size(), 1u);
    ASSERT_EQ(formats.value().number_of_channels[0], 2u);
    ASSERT_EQ(formats.value().sample_formats.size(), 1u);
    ASSERT_EQ(formats.value().sample_formats[0], SampleFormat::PCM_SIGNED);
    ASSERT_EQ(formats.value().frame_formats.size(), 2u);
    ASSERT_EQ(formats.value().frame_formats[0], FrameFormat::STEREO_LEFT);
    ASSERT_EQ(formats.value().frame_formats[1], FrameFormat::I2S);
    ASSERT_EQ(formats.value().frame_rates.size(), 2u);
    ASSERT_EQ(formats.value().frame_rates[0], 48000u);
    ASSERT_EQ(formats.value().frame_rates[1], 96000u);
    ASSERT_EQ(formats.value().bits_per_slot.size(), 1u);
    ASSERT_EQ(formats.value().bits_per_slot[0], 32u);
    ASSERT_EQ(formats.value().bits_per_sample.size(), 1u);
    ASSERT_EQ(formats.value().bits_per_sample[0], 16u);
  }

  // Check setting DAI formats.
  {
    DaiFormat format = GetDefaultDaiFormat();
    format.frame_rate = 48'000;
    auto formats = client.GetDaiFormats();
    ASSERT_TRUE(IsDaiFormatSupported(format, formats.value()));
    zx::result<CodecFormatInfo> codec_format_info = client.SetDaiFormat(std::move(format));
    ASSERT_OK(codec_format_info.status_value());
    EXPECT_EQ(zx::msec(25).get() + zx::usec(33'300).get(), codec_format_info->turn_on_delay());
    EXPECT_EQ(zx::msec(25).get() + zx::usec(33'300).get(), codec_format_info->turn_off_delay());
  }
  {
    DaiFormat format = GetDefaultDaiFormat();
    format.frame_rate = 48'000;
    format.channels_to_use_bitmask = 2;  // Use right channel in this mono codec.
    auto formats = client.GetDaiFormats();
    ASSERT_TRUE(IsDaiFormatSupported(format, formats.value()));
    zx::result<CodecFormatInfo> codec_format_info = client.SetDaiFormat(std::move(format));
    ASSERT_OK(codec_format_info.status_value());
    EXPECT_EQ(zx::msec(25).get() + zx::usec(33'300).get(), codec_format_info->turn_on_delay());
    EXPECT_EQ(zx::msec(25).get() + zx::usec(33'300).get(), codec_format_info->turn_off_delay());
  }

  {
    DaiFormat format = GetDefaultDaiFormat();
    format.frame_rate = 96'000;
    auto formats = client.GetDaiFormats();
    ASSERT_TRUE(IsDaiFormatSupported(format, formats.value()));
    zx::result<CodecFormatInfo> codec_format_info = client.SetDaiFormat(std::move(format));
    ASSERT_OK(codec_format_info.status_value());
    EXPECT_EQ(zx::msec(25).get() + zx::usec(16'700).get(), codec_format_info->turn_on_delay());
    EXPECT_EQ(zx::msec(25).get() + zx::usec(16'700).get(), codec_format_info->turn_off_delay());
  }

  {
    DaiFormat format = GetDefaultDaiFormat();
    format.frame_rate = 192'000;
    auto formats = client.GetDaiFormats();
    ASSERT_FALSE(IsDaiFormatSupported(format, formats.value()));
    ASSERT_TRUE(client.SetDaiFormat(std::move(format)).is_error());
  }

  child_dev->ReleaseOp();
  mock_i2c_.VerifyAndClear();
}

TEST_F(Tas5720Test, CodecGain) {
  auto fake_parent = MockDevice::FakeRootParent();
  uint32_t instance_count = 0;
  fake_parent->SetMetadata(DEVICE_METADATA_PRIVATE, &instance_count, sizeof(instance_count));

  ASSERT_OK(SimpleCodecServer::CreateAndAddToDdk<Tas5720Codec>(fake_parent.get(), GetI2cClient()));
  auto* child_dev = fake_parent->GetLatestChild();
  ASSERT_TRUE(child_dev);
  auto codec = child_dev->GetDeviceContext<Tas5720Codec>();
  zx::result<fidl::ClientEnd<fuchsia_hardware_audio::Codec>> codec_client = codec->GetClient();
  ASSERT_OK(codec_client.status_value());
  SimpleCodecClient client;
  client.SetCodec(std::move(*codec_client));

  mock_i2c_
      .ExpectWriteStop({0x06, 0x51})  // Analog 19.2dBV.
      .ExpectWriteStop({0x04, 0x9d})  // Digital -32dB.
      .ExpectWrite({0x03})
      .ExpectReadStop({0x00})
      .ExpectWriteStop({0x03, 0x10});  // Muted.

  // Lower than min gain.
  mock_i2c_
      .ExpectWriteStop({0x06, 0x51})  // Analog 19.2dBV (min)
      .ExpectWriteStop({0x04, 0x00})  // Digital -110.6dB.
      .ExpectWrite({0x03})
      .ExpectReadStop({0x00})
      .ExpectWriteStop({0x03, 0x10});  // Muted.

  // Higher than max gain.
  mock_i2c_
      .ExpectWriteStop({0x06, 0x5d})  // Analog 23.5dBV (max).
      .ExpectWriteStop({0x04, 0xff})  // Digital +24dB.
      .ExpectWrite({0x03})
      .ExpectReadStop({0x00})
      .ExpectWriteStop({0x03, 0x10});  // Muted.

  // Unmute.
  mock_i2c_
      .ExpectWriteStop({0x06, 0x5d})  // Analog 23.5dBV (max).
      .ExpectWriteStop({0x04, 0xff})  // Digital +24dB.
      .ExpectWrite({0x03})
      .ExpectReadStop({0xff})
      .ExpectWriteStop({0x03, 0xef});  // Unmuted.

  mock_i2c_.ExpectWrite({0x08}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({0x01}).ExpectReadStop({0xff}).ExpectWriteStop({0x01, 0xfe});  // Shutdown.

  // Change gain, keep mute and AGC.
  client.SetGainState({
      .gain = -32.f,
      .muted = true,
      .agc_enabled = false,
  });
  // Change gain, keep mute and AGC.
  client.SetGainState({
      .gain = -999.f,
      .muted = true,
      .agc_enabled = false,
  });
  // Change gain, keep mute and AGC.
  client.SetGainState({
      .gain = 111.f,
      .muted = true,
      .agc_enabled = false,
  });
  // Change mute, keep gain and AGC.
  client.SetGainState({
      .gain = 111.f,
      .muted = false,
      .agc_enabled = false,
  });

  // Make a 2-wal call to make sure the server (we know single threaded) completed previous calls.
  auto unused = client.GetInfo();
  static_cast<void>(unused);

  child_dev->ReleaseOp();
  mock_i2c_.VerifyAndClear();
}

TEST_F(Tas5720Test, CodecPlugState) {
  auto fake_parent = MockDevice::FakeRootParent();
  uint32_t instance_count = 0;
  fake_parent->SetMetadata(DEVICE_METADATA_PRIVATE, &instance_count, sizeof(instance_count));

  ASSERT_OK(SimpleCodecServer::CreateAndAddToDdk<Tas5720Codec>(fake_parent.get(), GetI2cClient()));
  auto* child_dev = fake_parent->GetLatestChild();
  ASSERT_TRUE(child_dev);
  auto codec = child_dev->GetDeviceContext<Tas5720Codec>();
  zx::result<fidl::ClientEnd<fuchsia_hardware_audio::Codec>> codec_client = codec->GetClient();
  ASSERT_OK(codec_client.status_value());
  SimpleCodecClient client;
  client.SetCodec(std::move(*codec_client));

  // Shutdown.
  mock_i2c_.ExpectWrite({0x08}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({0x01}).ExpectReadStop({0xff}).ExpectWriteStop({0x01, 0xfe});

  child_dev->ReleaseOp();
  mock_i2c_.VerifyAndClear();
}

TEST(Tas5720CustomEnvTest, InstanceCount) {
  auto fake_parent = MockDevice::FakeRootParent();
  uint32_t instance_count = 2;
  fake_parent->SetMetadata(DEVICE_METADATA_PRIVATE, &instance_count, sizeof(instance_count));

  // Reset by the TAS driver initialization setting slot to 2.
  mock_i2c::MockI2cGtest mock_i2c;

  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);

  mock_i2c.ExpectWrite({0x01})
      .ExpectReadStop({0xff})
      .ExpectWriteStop({0x01, 0xfe})  // Enter shutdown (part of reset).
      .ExpectWrite({0x01})
      .ExpectReadStop({0xfe})
      .ExpectWriteStop({0x01, 0xff})  // Exit shutdown (part of reset).
      .ExpectWrite({0x01})
      .ExpectReadStop({0xff})
      .ExpectWriteStop({0x01, 0xfe})  // Enter shutdown (part of stop).
      .ExpectWriteStop({0x02, 0x45})  // Digital control defaults. TODO set I2S.
      .ExpectWriteStop({0x03, 0x90})  // Digital control defaults. Slot 2, muted.
      .ExpectWriteStop({0x06, 0x5d})  // Analog defaults.
      .ExpectWriteStop({0x10, 0xff})  // clippers disabled.
      .ExpectWriteStop({0x11, 0xfc})  // clippers disabled.
      .ExpectWrite({0x01})
      .ExpectReadStop({0xfe})
      .ExpectWriteStop({0x01, 0xff})  // exit shutdown (part of start).
      .ExpectWriteStop({0x06, 0x5d})  // Default gain.
      .ExpectWriteStop({0x04, 0xcf})  // Default gain.
      .ExpectWrite({0x03})
      .ExpectReadStop({0x80})
      .ExpectWriteStop({0x03, 0x90});  // Muted.

  auto endpoints = fidl::Endpoints<fuchsia_hardware_i2c::Device>::Create();

  fidl::BindServer(loop.dispatcher(), std::move(endpoints.server), &mock_i2c);
  loop.StartThread();

  ASSERT_OK(SimpleCodecServer::CreateAndAddToDdk<Tas5720Codec>(fake_parent.get(),
                                                               std::move(endpoints.client)));
  auto* child_dev = fake_parent->GetLatestChild();
  ASSERT_TRUE(child_dev);

  // Shutdown.
  mock_i2c.ExpectWrite({0x08}).ExpectReadStop({0x00});
  mock_i2c.ExpectWrite({0x01}).ExpectReadStop({0xff}).ExpectWriteStop({0x01, 0xfe});

  child_dev->ReleaseOp();
  mock_i2c.VerifyAndClear();
}

TEST_F(Tas5720Test, FaultNotSeen) {
  auto fake_parent = MockDevice::FakeRootParent();
  uint32_t instance_count = 0;
  fake_parent->SetMetadata(DEVICE_METADATA_PRIVATE, &instance_count, sizeof(instance_count));

  ASSERT_OK(SimpleCodecServer::CreateAndAddToDdk<Tas5720Codec>(fake_parent.get(), GetI2cClient()));
  auto* child_dev = fake_parent->GetLatestChild();
  ASSERT_TRUE(child_dev);
  auto codec = child_dev->GetDeviceContext<Tas5720Codec>();

  mock_i2c_.ExpectWrite({0x08}).ExpectReadStop({0x00});
  codec->PollFaults(/*periodic=*/false);

  fpromise::result<inspect::Hierarchy> hierarchy_result =
      fpromise::run_single_threaded(inspect::ReadFromInspector(codec->inspect()));
  ASSERT_TRUE(hierarchy_result.is_ok());

  inspect::Hierarchy hierarchy = std::move(hierarchy_result.value());
  const inspect::Hierarchy* codec_root = hierarchy.GetByPath({"tas5720"});
  ASSERT_TRUE(codec_root);

  auto& faults = codec_root->children();
  ASSERT_EQ(faults.size(), 1u);

  EXPECT_THAT(faults[0].node(), inspect::testing::PropertyList(testing::Contains(
                                    inspect::testing::StringIs("state", "No fault"))));

  // Shutdown.
  mock_i2c_.ExpectWrite({0x08}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({0x01}).ExpectReadStop({0xff}).ExpectWriteStop({0x01, 0xfe});

  child_dev->ReleaseOp();
  mock_i2c_.VerifyAndClear();
}

TEST_F(Tas5720Test, FaultPollI2CError) {
  auto fake_parent = MockDevice::FakeRootParent();
  uint32_t instance_count = 0;
  fake_parent->SetMetadata(DEVICE_METADATA_PRIVATE, &instance_count, sizeof(instance_count));

  ASSERT_OK(SimpleCodecServer::CreateAndAddToDdk<Tas5720Codec>(fake_parent.get(), GetI2cClient()));
  auto* child_dev = fake_parent->GetLatestChild();
  ASSERT_TRUE(child_dev);
  auto codec = child_dev->GetDeviceContext<Tas5720Codec>();

  // Repeat I2C timeout 3 times.
  mock_i2c_.ExpectWrite({0x08}).ExpectReadStop({0xFF}, ZX_ERR_TIMED_OUT);
  mock_i2c_.ExpectWrite({0x08}).ExpectReadStop({0xFF}, ZX_ERR_TIMED_OUT);
  mock_i2c_.ExpectWrite({0x08}).ExpectReadStop({0xFF}, ZX_ERR_TIMED_OUT);
  codec->PollFaults(/*periodic=*/false);

  fpromise::result<inspect::Hierarchy> hierarchy_result =
      fpromise::run_single_threaded(inspect::ReadFromInspector(codec->inspect()));
  ASSERT_TRUE(hierarchy_result.is_ok());

  inspect::Hierarchy hierarchy = std::move(hierarchy_result.value());
  const inspect::Hierarchy* codec_root = hierarchy.GetByPath({"tas5720"});
  ASSERT_TRUE(codec_root);

  auto& faults = codec_root->children();
  ASSERT_EQ(faults.size(), 1u);

  EXPECT_THAT(faults[0].node(), inspect::testing::PropertyList(testing::Contains(
                                    inspect::testing::StringIs("state", "I2C error"))));

  // Shutdown.
  mock_i2c_.ExpectWrite({0x08}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({0x01}).ExpectReadStop({0xff}).ExpectWriteStop({0x01, 0xfe});

  child_dev->ReleaseOp();
  mock_i2c_.VerifyAndClear();
}

TEST_F(Tas5720Test, FaultPollClockFault) {
  auto fake_parent = MockDevice::FakeRootParent();
  uint32_t instance_count = 0;
  fake_parent->SetMetadata(DEVICE_METADATA_PRIVATE, &instance_count, sizeof(instance_count));

  ASSERT_OK(SimpleCodecServer::CreateAndAddToDdk<Tas5720Codec>(fake_parent.get(), GetI2cClient()));
  auto* child_dev = fake_parent->GetLatestChild();
  ASSERT_TRUE(child_dev);
  auto codec = child_dev->GetDeviceContext<Tas5720Codec>();

  mock_i2c_.ExpectWrite({0x08}).ExpectReadStop({0x08});
  codec->PollFaults(/*periodic=*/false);

  fpromise::result<inspect::Hierarchy> hierarchy_result =
      fpromise::run_single_threaded(inspect::ReadFromInspector(codec->inspect()));
  ASSERT_TRUE(hierarchy_result.is_ok());

  inspect::Hierarchy hierarchy = std::move(hierarchy_result.value());
  const inspect::Hierarchy* codec_root = hierarchy.GetByPath({"tas5720"});
  ASSERT_TRUE(codec_root);

  auto& faults = codec_root->children();
  ASSERT_EQ(faults.size(), 1u);

  EXPECT_THAT(faults[0].node(), inspect::testing::PropertyList(testing::Contains(
                                    inspect::testing::StringIs("state", "SAIF clock error, 08"))));

  // Shutdown.
  mock_i2c_.ExpectWrite({0x08}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({0x01}).ExpectReadStop({0xff}).ExpectWriteStop({0x01, 0xfe});

  child_dev->ReleaseOp();
  mock_i2c_.VerifyAndClear();
}

// Trigger 20 "events" -- ten faults, each of which then goes away.
// This should result in the 10 most recent events being reported,
// and the 10 oldest being dropped.  Don't bother verifying the
// event details, just check the timestamps to verify that
// the first half are dropped.
TEST_F(Tas5720Test, FaultsAgeOut) {
  auto fake_parent = MockDevice::FakeRootParent();
  uint32_t instance_count = 0;
  fake_parent->SetMetadata(DEVICE_METADATA_PRIVATE, &instance_count, sizeof(instance_count));

  ASSERT_OK(SimpleCodecServer::CreateAndAddToDdk<Tas5720Codec>(fake_parent.get(), GetI2cClient()));
  auto* child_dev = fake_parent->GetLatestChild();
  ASSERT_TRUE(child_dev);
  auto codec = child_dev->GetDeviceContext<Tas5720Codec>();

  zx_time_t time_threshold = 0;

  for (int fault_count = 0; fault_count < 10; fault_count++) {
    if (fault_count == 5)
      time_threshold = zx_clock_get_monotonic();

    // Detect fault
    mock_i2c_.ExpectWrite({0x08}).ExpectReadStop({0x08});
    codec->PollFaults(/*periodic=*/false);

    // Fault goes away
    mock_i2c_.ExpectWrite({0x08}).ExpectReadStop({0x00});
    codec->PollFaults(/*periodic=*/false);
  }

  // We should have ten events seen, and all of them should be
  // timestamped after time_threshold.
  fpromise::result<inspect::Hierarchy> hierarchy_result =
      fpromise::run_single_threaded(inspect::ReadFromInspector(codec->inspect()));
  ASSERT_TRUE(hierarchy_result.is_ok());

  inspect::Hierarchy hierarchy = std::move(hierarchy_result.value());
  const inspect::Hierarchy* fault_root = hierarchy.GetByPath({"tas5720"});
  ASSERT_TRUE(fault_root);
  auto& faults = fault_root->children();
  ASSERT_EQ(faults.size(), 10u);
  for (int event_count = 0; event_count < 10; event_count++) {
    const auto* first_seen_property =
        faults[event_count].node().get_property<inspect::IntPropertyValue>("first_seen");
    ASSERT_TRUE(first_seen_property);
    ASSERT_GT(first_seen_property->value(), time_threshold);
  }

  // Shutdown.
  mock_i2c_.ExpectWrite({0x08}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({0x01}).ExpectReadStop({0xff}).ExpectWriteStop({0x01, 0xfe});

  child_dev->ReleaseOp();
  mock_i2c_.VerifyAndClear();
}

}  // namespace audio

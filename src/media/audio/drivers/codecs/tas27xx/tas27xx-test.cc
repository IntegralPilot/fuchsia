// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tas27xx.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/ddk/metadata.h>
#include <lib/fpromise/single_threaded_executor.h>
#include <lib/inspect/cpp/hierarchy.h>
#include <lib/inspect/cpp/reader.h>
#include <lib/inspect/testing/cpp/inspect.h>
#include <lib/mock-i2c/mock-i2c-gtest.h>
#include <lib/simple-codec/simple-codec-client.h>
#include <lib/simple-codec/simple-codec-helper.h>
#include <lib/sync/completion.h>

#include <gtest/gtest.h>

#include "src/devices/gpio/testing/fake-gpio/fake-gpio.h"
#include "src/devices/testing/mock-ddk/mock-device.h"
#include "src/lib/testing/predicates/status.h"

namespace audio {

audio::DaiFormat GetDefaultDaiFormat() {
  return {
      .number_of_channels = 2,
      .channels_to_use_bitmask = 2,  // Use one channel (right) in this mono codec.
      .sample_format = SampleFormat::PCM_SIGNED,
      .frame_format = FrameFormat::I2S,
      .frame_rate = 24'000,
      .bits_per_slot = 32,
      .bits_per_sample = 16,
  };
}

struct Tas27xxCodec : public Tas27xx {
  explicit Tas27xxCodec(zx_device_t* parent, ddk::I2cChannel i2c,
                        fidl::ClientEnd<fuchsia_hardware_gpio::Gpio> fault)
      : Tas27xx(parent, std::move(i2c), std::move(fault), true, true) {}
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
  inspect::Inspector& inspect() { return Tas27xx::inspect(); }
};

class Tas27xxTest : public ::testing::Test {
 public:
  void SetUp() override {
    EXPECT_OK(fidl_servers_loop_.StartThread("fidl-servers"));

    auto endpoints = fidl::Endpoints<fuchsia_hardware_i2c::Device>::Create();

    fidl::BindServer(fidl_servers_loop_.dispatcher(), std::move(endpoints.server), &mock_i2c_);

    mock_i2c_client_ = std::move(endpoints.client);
    fault_gpio_client_ = fault_gpio_.SyncCall(&fake_gpio::FakeGpio::Connect);
  }

 private:
  async::Loop fidl_servers_loop_{&kAsyncLoopConfigNoAttachToCurrentThread};

 protected:
  mock_i2c::MockI2cGtest mock_i2c_;
  async_patterns::TestDispatcherBound<fake_gpio::FakeGpio> fault_gpio_{
      fidl_servers_loop_.dispatcher(), std::in_place};
  fidl::ClientEnd<fuchsia_hardware_i2c::Device> mock_i2c_client_;
  fidl::ClientEnd<fuchsia_hardware_gpio::Gpio> fault_gpio_client_;
};

TEST_F(Tas27xxTest, CodecInitGood) {
  auto fake_parent = MockDevice::FakeRootParent();

  ASSERT_OK(SimpleCodecServer::CreateAndAddToDdk<Tas27xxCodec>(
      fake_parent.get(), std::move(mock_i2c_client_), std::move(fault_gpio_client_)));

  mock_i2c_.VerifyAndClear();
}

TEST_F(Tas27xxTest, CodecInitBad) {
  auto fake_parent = MockDevice::FakeRootParent();

  // Error when getting the interrupt.
  fault_gpio_.SyncCall(&fake_gpio::FakeGpio::SetInterrupt, zx::error(ZX_ERR_INTERNAL));

  ASSERT_EQ(ZX_ERR_INTERNAL,
            SimpleCodecServer::CreateAndAddToDdk<Tas27xxCodec>(
                fake_parent.get(), std::move(mock_i2c_client_), std::move(fault_gpio_client_)));

  mock_i2c_.VerifyAndClear();
}

TEST_F(Tas27xxTest, CodecGetInfo) {
  auto fake_parent = MockDevice::FakeRootParent();

  ASSERT_OK(SimpleCodecServer::CreateAndAddToDdk<Tas27xxCodec>(
      fake_parent.get(), std::move(mock_i2c_client_), std::move(fault_gpio_client_)));
  auto* child_dev = fake_parent->GetLatestChild();
  ASSERT_TRUE(child_dev);
  auto codec = child_dev->GetDeviceContext<Tas27xxCodec>();
  zx::result<fidl::ClientEnd<fuchsia_hardware_audio::Codec>> codec_client = codec->GetClient();
  ASSERT_OK(codec_client.status_value());
  SimpleCodecClient client;
  client.SetCodec(std::move(*codec_client));
  auto info = client.GetInfo();
  ASSERT_FALSE(info->unique_id.has_value());
  ASSERT_EQ(info->manufacturer.compare("Texas Instruments"), 0);
  ASSERT_EQ(info->product_name.compare("TAS2770"), 0);

  mock_i2c_.VerifyAndClear();
}

TEST_F(Tas27xxTest, CodecReset) {
  auto fake_parent = MockDevice::FakeRootParent();

  // Reset by the call to Reset.
  mock_i2c_
      .ExpectWriteStop({0x01, 0x01}, ZX_ERR_INTERNAL)  // SW_RESET error, will retry.
      .ExpectWriteStop({0x01, 0x01}, ZX_OK)            // SW_RESET.
      .ExpectWriteStop({0x02, 0x0e})                   // PWR_CTL stopped.
      .ExpectWriteStop({0x3c, 0x10})                   // CLOCK_CFG.
      .ExpectWriteStop({0x0a, 0x07})                   // SetRate.
      .ExpectWriteStop({0x0c, 0x22})                   // TDM_CFG2.
      .ExpectWriteStop({0x0e, 0x02})                   // TDM_CFG4.
      .ExpectWriteStop({0x0f, 0x44})                   // TDM_CFG5.
      .ExpectWriteStop({0x10, 0x40})                   // TDM_CFG6.
      .ExpectWrite({0x24})
      .ExpectReadStop({0x00})  // INT_LTCH0.
      .ExpectWrite({0x25})
      .ExpectReadStop({0x00})  // INT_LTCH1.
      .ExpectWrite({0x26})
      .ExpectReadStop({0x00})          // INT_LTCH2.
      .ExpectWriteStop({0x20, 0xf8})   // INT_MASK0.
      .ExpectWriteStop({0x21, 0xff})   // INT_MASK1.
      .ExpectWriteStop({0x30, 0x01})   // INT_CFG.
      .ExpectWriteStop({0x05, 0x00})   // 0dB.
      .ExpectWriteStop({0x02, 0x0e});  // PWR_CTL stopped.

  ASSERT_OK(SimpleCodecServer::CreateAndAddToDdk<Tas27xxCodec>(
      fake_parent.get(), std::move(mock_i2c_client_), std::move(fault_gpio_client_)));
  auto* child_dev = fake_parent->GetLatestChild();
  ASSERT_TRUE(child_dev);
  auto codec = child_dev->GetDeviceContext<Tas27xxCodec>();
  zx::result<fidl::ClientEnd<fuchsia_hardware_audio::Codec>> codec_client = codec->GetClient();
  ASSERT_OK(codec_client.status_value());
  SimpleCodecClient client;
  client.SetCodec(std::move(*codec_client));
  ASSERT_OK(client.Reset());

  mock_i2c_.VerifyAndClear();
}

// This test is disabled because it relies on a timeout expectation that would create flakes.
TEST_F(Tas27xxTest, DISABLED_CodecResetDueToErrorState) {
  auto fake_parent = MockDevice::FakeRootParent();

  // Set gain state.
  mock_i2c_
      .ExpectWriteStop({0x05, 0x40})   // -32dB.
      .ExpectWriteStop({0x02, 0x0d});  // PWR_CTL stopped.

  // Set DAI format.
  mock_i2c_
      .ExpectWriteStop({0x0a, 0x07})   // SetRate 48k.
      .ExpectWriteStop({0x0c, 0x22});  // SetTdmSlots right.

  // Start.
  mock_i2c_.ExpectWriteStop({0x02, 0x00});  // PWR_CTL started.

  // Check for error state.
  mock_i2c_.ExpectWrite({0x02}).ExpectReadStop({0x02});  // PRW_CTL in shutdown.

  // Read state to report.
  mock_i2c_.ExpectWrite({0x24})
      .ExpectReadStop({0x00})  // INT_LTCH0.
      .ExpectWrite({0x25})
      .ExpectReadStop({0x00})  // INT_LTCH1.
      .ExpectWrite({0x26})
      .ExpectReadStop({0x00})  // INT_LTCH2.
      .ExpectWrite({0x29})
      .ExpectReadStop({0x00})  // TEMP_MSB.
      .ExpectWrite({0x2a})
      .ExpectReadStop({0x00})  // TEMP_LSB.
      .ExpectWrite({0x27})
      .ExpectReadStop({0x00})  // VBAT_MSB.
      .ExpectWrite({0x28})
      .ExpectReadStop({0x00});  // VBAT_LSB.

  // Reset.
  mock_i2c_
      .ExpectWriteStop({0x01, 0x01}, ZX_OK)  // SW_RESET.
      .ExpectWriteStop({0x02, 0x0d})         // PRW_CTL stopped.
      .ExpectWriteStop({0x3c, 0x10})         // CLOCK_CFG.
      .ExpectWriteStop({0x0a, 0x07})         // SetRate.
      .ExpectWriteStop({0x0c, 0x22})         // TDM_CFG2.
      .ExpectWriteStop({0x0e, 0x02})         // TDM_CFG4.
      .ExpectWriteStop({0x0f, 0x44})         // TDM_CFG5.
      .ExpectWriteStop({0x10, 0x40})         // TDM_CFG6.
      .ExpectWrite({0x24})
      .ExpectReadStop({0x00})  // INT_LTCH0.
      .ExpectWrite({0x25})
      .ExpectReadStop({0x00})  // INT_LTCH1.
      .ExpectWrite({0x26})
      .ExpectReadStop({0x00})          // INT_LTCH2.
      .ExpectWriteStop({0x20, 0xf8})   // INT_MASK0.
      .ExpectWriteStop({0x21, 0xff})   // INT_MASK1.
      .ExpectWriteStop({0x30, 0x01})   // INT_CFG.
      .ExpectWriteStop({0x05, 0x00})   // 0dB, default.
      .ExpectWriteStop({0x02, 0x0d});  // PWR_CTL stopped.

  // Set gain state.
  mock_i2c_
      .ExpectWriteStop({0x05, 0x40})   // -32dB, old gain_state_.
      .ExpectWriteStop({0x02, 0x0d});  // PWR_CTL stopped.

  // Set DAI format.
  mock_i2c_
      .ExpectWriteStop({0x0a, 0x07})   // SetRate 48k.
      .ExpectWriteStop({0x0c, 0x22});  // SetTdmSlots right.

  // Start.
  mock_i2c_.ExpectWriteStop({0x02, 0x00});  // PWR_CTL started.

  ASSERT_OK(SimpleCodecServer::CreateAndAddToDdk<Tas27xxCodec>(
      fake_parent.get(), std::move(mock_i2c_client_), std::move(fault_gpio_client_)));
  auto* child_dev = fake_parent->GetLatestChild();
  ASSERT_TRUE(child_dev);
  auto codec = child_dev->GetDeviceContext<Tas27xxCodec>();
  zx::result<fidl::ClientEnd<fuchsia_hardware_audio::Codec>> codec_client = codec->GetClient();
  ASSERT_OK(codec_client.status_value());
  SimpleCodecClient client;
  client.SetCodec(std::move(*codec_client));

  client.SetGainState({
      .gain = -32.f,
      .muted = false,
      .agc_enabled = false,
  });

  DaiFormat format = GetDefaultDaiFormat();
  format.frame_rate = 48'000;
  ASSERT_OK(client.SetDaiFormat(std::move(format)));

  // Get into started state, so we can be in error state after the timeout.
  ASSERT_OK(client.Start());

  // Wait for the timeout to occur.
  constexpr int64_t kTimeoutSeconds = 30;
  zx::nanosleep(zx::deadline_after(zx::sec(kTimeoutSeconds)));

  // Make a 2-way call to make sure the server (we know single threaded) completed previous calls.
  ASSERT_OK(client.GetInfo());

  mock_i2c_.VerifyAndClear();
}

TEST_F(Tas27xxTest, ExternalConfig) {
  auto fake_parent = MockDevice::FakeRootParent();

  static const fuchsia_hardware_ti_metadata::TasMetadata kMetadata(
      {.init_sequence1 =
           {
               fuchsia_hardware_ti_metadata::Register({
                   .address = 0x12,
                   .value = 0x34,
               }),
               fuchsia_hardware_ti_metadata::Register({
                   .address = 0x56,
                   .value = 0x78,
               }),
           },
       .init_sequence2 = {
           fuchsia_hardware_ti_metadata::Register({
               .address = 0x11,
               .value = 0x22,
           }),
           fuchsia_hardware_ti_metadata::Register({
               .address = 0x33,
               .value = 0x44,
           }),
           fuchsia_hardware_ti_metadata::Register({
               .address = 0x55,
               .value = 0x66,
           }),
       }});
  const fit::result persisted_metadata = fidl::Persist(kMetadata);
  ASSERT_TRUE(persisted_metadata.is_ok());

  fake_parent->SetMetadata(DEVICE_METADATA_PRIVATE, persisted_metadata.value().data(),
                           persisted_metadata.value().size());

  // Reset by the call to Reset.
  mock_i2c_
      .ExpectWriteStop({0x01, 0x01}, ZX_ERR_INTERNAL)  // SW_RESET error, will retry.
      .ExpectWriteStop({0x01, 0x01}, ZX_OK)            // SW_RESET.
      .ExpectWriteStop({0x12, 0x34})                   // External config.
      .ExpectWriteStop({0x56, 0x78})                   // External config.
      .ExpectWriteStop({0x11, 0x22})                   // External config.
      .ExpectWriteStop({0x33, 0x44})                   // External config.
      .ExpectWriteStop({0x55, 0x66})                   // External config.
      .ExpectWriteStop({0x02, 0x0e})                   // PWR_CTL stopped.
      .ExpectWriteStop({0x3c, 0x10})                   // CLOCK_CFG.
      .ExpectWriteStop({0x0a, 0x07})                   // SetRate.
      .ExpectWriteStop({0x0c, 0x22})                   // TDM_CFG2.
      .ExpectWriteStop({0x0e, 0x02})                   // TDM_CFG4.
      .ExpectWriteStop({0x0f, 0x44})                   // TDM_CFG5.
      .ExpectWriteStop({0x10, 0x40})                   // TDM_CFG6.
      .ExpectWrite({0x24})
      .ExpectReadStop({0x00})  // INT_LTCH0.
      .ExpectWrite({0x25})
      .ExpectReadStop({0x00})  // INT_LTCH1.
      .ExpectWrite({0x26})
      .ExpectReadStop({0x00})          // INT_LTCH2.
      .ExpectWriteStop({0x20, 0xf8})   // INT_MASK0.
      .ExpectWriteStop({0x21, 0xff})   // INT_MASK1.
      .ExpectWriteStop({0x30, 0x01})   // INT_CFG.
      .ExpectWriteStop({0x05, 0x00})   // 0dB.
      .ExpectWriteStop({0x02, 0x0e});  // PWR_CTL stopped.

  ASSERT_OK(SimpleCodecServer::CreateAndAddToDdk<Tas27xxCodec>(
      fake_parent.get(), std::move(mock_i2c_client_), std::move(fault_gpio_client_)));
  auto* child_dev = fake_parent->GetLatestChild();
  ASSERT_TRUE(child_dev);
  auto codec = child_dev->GetDeviceContext<Tas27xxCodec>();
  zx::result<fidl::ClientEnd<fuchsia_hardware_audio::Codec>> codec_client = codec->GetClient();
  ASSERT_OK(codec_client.status_value());
  SimpleCodecClient client;
  client.SetCodec(std::move(*codec_client));
  ASSERT_OK(client.Reset());

  mock_i2c_.VerifyAndClear();
}

TEST_F(Tas27xxTest, CodecDaiFormat) {
  auto fake_parent = MockDevice::FakeRootParent();

  ASSERT_OK(SimpleCodecServer::CreateAndAddToDdk<Tas27xxCodec>(
      fake_parent.get(), std::move(mock_i2c_client_), std::move(fault_gpio_client_)));
  auto* child_dev = fake_parent->GetLatestChild();
  ASSERT_TRUE(child_dev);
  auto codec = child_dev->GetDeviceContext<Tas27xxCodec>();
  zx::result<fidl::ClientEnd<fuchsia_hardware_audio::Codec>> codec_client = codec->GetClient();
  ASSERT_OK(codec_client.status_value());
  SimpleCodecClient client;
  client.SetCodec(std::move(*codec_client));

  // We complete all i2c mock setup before executing server methods in a different thread.
  mock_i2c_
      .ExpectWriteStop({0x0a, 0x07})   // SetRate 48k.
      .ExpectWriteStop({0x0c, 0x22})   // SetTdmSlots right.
      .ExpectWriteStop({0x0a, 0x09})   // SetRate 96k.
      .ExpectWriteStop({0x0c, 0x12});  // SetTdmSlots left.

  // Check getting DAI formats.
  {
    auto formats = client.GetDaiFormats();
    ASSERT_EQ(formats.value().number_of_channels.size(), 1u);
    ASSERT_EQ(formats.value().number_of_channels[0], 2u);
    ASSERT_EQ(formats.value().sample_formats.size(), 1u);
    ASSERT_EQ(formats.value().sample_formats[0], SampleFormat::PCM_SIGNED);
    ASSERT_EQ(formats.value().frame_formats.size(), 1u);
    ASSERT_EQ(formats.value().frame_formats[0], FrameFormat::I2S);
    ASSERT_EQ(formats.value().frame_rates.size(), 2u);
    ASSERT_EQ(formats.value().frame_rates[0], 48000u);
    ASSERT_EQ(formats.value().frame_rates[1], 96000u);
    ASSERT_EQ(formats.value().bits_per_slot.size(), 1u);
    ASSERT_EQ(formats.value().bits_per_slot[0], 32u);
    ASSERT_EQ(formats.value().bits_per_sample.size(), 1u);
    ASSERT_EQ(formats.value().bits_per_sample[0], 16u);
  }

  // Check inspect state.
  fpromise::result<inspect::Hierarchy> hierarchy_result =
      fpromise::run_single_threaded(inspect::ReadFromInspector(codec->inspect()));
  ASSERT_TRUE(hierarchy_result.is_ok());

  inspect::Hierarchy hierarchy = std::move(hierarchy_result.value());
  const inspect::Hierarchy* simple_codec_root = hierarchy.GetByPath({"simple_codec"});
  ASSERT_TRUE(simple_codec_root);

  EXPECT_THAT(simple_codec_root->node(), inspect::testing::PropertyList(testing::Contains(
                                             inspect::testing::StringIs("state", "created"))));
  EXPECT_THAT(
      simple_codec_root->node(),
      inspect::testing::PropertyList(testing::Contains(inspect::testing::IntIs("start_time", 0))));
  EXPECT_THAT(simple_codec_root->node(),
              inspect::testing::PropertyList(testing::Contains(
                  inspect::testing::StringIs("manufacturer", "Texas Instruments"))));
  EXPECT_THAT(simple_codec_root->node(), inspect::testing::PropertyList(testing::Contains(
                                             inspect::testing::StringIs("product", "TAS2770"))));

  // Check setting DAI formats.
  {
    DaiFormat format = GetDefaultDaiFormat();
    format.frame_rate = 48'000;
    auto formats = client.GetDaiFormats();
    ASSERT_TRUE(IsDaiFormatSupported(format, formats.value()));
    zx::result<CodecFormatInfo> codec_format_info = client.SetDaiFormat(std::move(format));
    ASSERT_OK(codec_format_info.status_value());
    EXPECT_EQ(zx::usec(5'300).get(), codec_format_info->turn_on_delay());
    EXPECT_EQ(zx::usec(4'700).get(), codec_format_info->turn_off_delay());
  }

  {
    DaiFormat format = GetDefaultDaiFormat();
    format.frame_rate = 96'000;
    format.channels_to_use_bitmask = 1;  // Use one channel (left) in this mono codec.
    auto formats = client.GetDaiFormats();
    ASSERT_TRUE(IsDaiFormatSupported(format, formats.value()));
    zx::result<CodecFormatInfo> codec_format_info = client.SetDaiFormat(std::move(format));
    ASSERT_OK(codec_format_info.status_value());
    EXPECT_EQ(zx::usec(5'300).get(), codec_format_info->turn_on_delay());
    EXPECT_EQ(zx::usec(4'700).get(), codec_format_info->turn_off_delay());
  }

  {
    DaiFormat format = GetDefaultDaiFormat();
    format.frame_rate = 192'000;
    auto formats = client.GetDaiFormats();
    ASSERT_FALSE(IsDaiFormatSupported(format, formats.value()));
    ASSERT_TRUE(client.SetDaiFormat(std::move(format)).is_error());
  }

  mock_i2c_.VerifyAndClear();
}

TEST_F(Tas27xxTest, CodecGain) {
  auto fake_parent = MockDevice::FakeRootParent();

  ASSERT_OK(SimpleCodecServer::CreateAndAddToDdk<Tas27xxCodec>(
      fake_parent.get(), std::move(mock_i2c_client_), std::move(fault_gpio_client_)));
  auto* child_dev = fake_parent->GetLatestChild();
  ASSERT_TRUE(child_dev);
  auto codec = child_dev->GetDeviceContext<Tas27xxCodec>();
  zx::result<fidl::ClientEnd<fuchsia_hardware_audio::Codec>> codec_client = codec->GetClient();
  ASSERT_OK(codec_client.status_value());
  SimpleCodecClient client;
  client.SetCodec(std::move(*codec_client));

  // We complete all i2c mock setup before executing server methods in a different thread.
  mock_i2c_
      .ExpectWriteStop({0x05, 0x40})   // -32dB.
      .ExpectWriteStop({0x02, 0x0e});  // PWR_CTL stopped.

  // Lower than min gain.
  mock_i2c_
      .ExpectWriteStop({0x05, 0xc8})   // -100dB.
      .ExpectWriteStop({0x02, 0x0e});  // PWR_CTL stopped.

  // Higher than max gain.
  mock_i2c_
      .ExpectWriteStop({0x05, 0x0})    // 0dB.
      .ExpectWriteStop({0x02, 0x0e});  // PWR_CTL stopped.

  // Reset and start so the codec is powered down by stop when muted.
  mock_i2c_
      .ExpectWriteStop({0x01, 0x01}, ZX_ERR_INTERNAL)  // SW_RESET error, will retry.
      .ExpectWriteStop({0x01, 0x01}, ZX_OK)            // SW_RESET.
      .ExpectWriteStop({0x02, 0x0e})                   // PWR_CTL stopped.
      .ExpectWriteStop({0x3c, 0x10})                   // CLOCK_CFG.
      .ExpectWriteStop({0x0a, 0x07})                   // SetRate.
      .ExpectWriteStop({0x0c, 0x22})                   // TDM_CFG2.
      .ExpectWriteStop({0x0e, 0x02})                   // TDM_CFG4.
      .ExpectWriteStop({0x0f, 0x44})                   // TDM_CFG5.
      .ExpectWriteStop({0x10, 0x40})                   // TDM_CFG6.
      .ExpectWrite({0x24})
      .ExpectReadStop({0x00})  // INT_LTCH0.
      .ExpectWrite({0x25})
      .ExpectReadStop({0x00})  // INT_LTCH1.
      .ExpectWrite({0x26})
      .ExpectReadStop({0x00})          // INT_LTCH2.
      .ExpectWriteStop({0x20, 0xf8})   // INT_MASK0.
      .ExpectWriteStop({0x21, 0xff})   // INT_MASK1.
      .ExpectWriteStop({0x30, 0x01})   // INT_CFG.
      .ExpectWriteStop({0x05, 0x00})   // 0dB.
      .ExpectWriteStop({0x02, 0x0e});  // PWR_CTL stopped.

  // Start but muted.
  mock_i2c_.ExpectWriteStop({0x02, 0x01});  // PWR_CTL stopped due to mute state.

  // Unmute.
  mock_i2c_
      .ExpectWriteStop({0x05, 0x0})    // 0dB.
      .ExpectWriteStop({0x02, 0x00});  // PWR_CTL started.

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

  // Get into reset ad started state, so mute powers the codec down.
  ASSERT_OK(client.Reset());
  ASSERT_OK(client.Start());
  // Change mute, keep gain and AGC.
  client.SetGainState({
      .gain = 111.f,
      .muted = false,
      .agc_enabled = false,
  });

  // Make a 2-wal call to make sure the server (we know single threaded) completed previous calls.
  ASSERT_OK(client.GetInfo());

  mock_i2c_.VerifyAndClear();
}

}  // namespace audio

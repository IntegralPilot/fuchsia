// Copyright 2022 The Fuchsia Authors. All rights reserved.  Use of
// this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "intel-dsp-stream.h"

#include <fuchsia/hardware/intelhda/codec/cpp/banjo.h>
#include <lib/fidl/cpp/wire/connect_service.h>

#include <optional>
#include <thread>
#include <vector>

#include <ddktl/device.h>
#include <intel-hda/codec-utils/codec-driver-base.h>
#include <intel-hda/codec-utils/streamconfig-base.h>
#include <zxtest/zxtest.h>

#include "src/devices/testing/mock-ddk/mock-device.h"

namespace {

static constexpr char kTestProductName[] = "Builtin Headphone Jack";

fidl::WireSyncClient<fuchsia_hardware_audio::Dai> GetDaiClient(
    fidl::WireSyncClient<fuchsia_hardware_audio::DaiConnector>& client) {
  auto [stream_channel_local, stream_channel_remote] =
      fidl::Endpoints<fuchsia_hardware_audio::Dai>::Create();
  ZX_ASSERT(client->Connect(std::move(stream_channel_remote)).ok());
  return fidl::WireSyncClient<fuchsia_hardware_audio::Dai>(std::move(stream_channel_local));
}

}  // namespace

namespace audio::intel_hda {

class TestCodec : public codecs::IntelHDACodecDriverBase {
 public:
  explicit TestCodec() = default;

  zx_status_t ActivateStream(const fbl::RefPtr<codecs::IntelHDAStreamBase>& stream) {
    return codecs::IntelHDACodecDriverBase::ActivateStream(stream);
  }

  zx::result<> Bind(zx_device_t* codec_dev, const char* name) {
    return codecs::IntelHDACodecDriverBase::Bind(codec_dev, name);
  }
  void DeviceRelease() { codecs::IntelHDACodecDriverBase::DeviceRelease(); }
};

class TestStream : public IntelDspStream {
 public:
  TestStream()
      : IntelDspStream(DspStream{.id = DspPipelineId{1},
                                 .host_format = kTestHostFormat,
                                 .dai_format = kTestDaiFormat,
                                 .is_i2s = true,
                                 .stream_id = 3,
                                 .is_input = false,
                                 .uid = AUDIO_STREAM_UNIQUE_ID_BUILTIN_HEADPHONE_JACK,
                                 .name = kTestProductName}) {}
  zx_status_t Bind() {
    fbl::AutoLock lock(obj_lock());
    return PublishDeviceLocked();
  }

 private:
  static constexpr AudioDataFormat kTestHostFormat = {
      .sampling_frequency = SamplingFrequency::FS_48000HZ,
      .bit_depth = BitDepth::DEPTH_16BIT,
      .channel_map = 0xFFFFFF10,
      .channel_config = ChannelConfig::CONFIG_STEREO,
      .interleaving_style = InterleavingStyle::PER_CHANNEL,
      .number_of_channels = 2,
      .valid_bit_depth = 16,
      .sample_type = SampleType::INT_MSB,
      .reserved = 0,
  };
  static constexpr AudioDataFormat kTestDaiFormat = {
      .sampling_frequency = SamplingFrequency::FS_48000HZ,
      .bit_depth = BitDepth::DEPTH_32BIT,
      .channel_map = 0xFFFFFF10,
      .channel_config = ChannelConfig::CONFIG_STEREO,
      .interleaving_style = InterleavingStyle::PER_CHANNEL,
      .number_of_channels = 2,
      .valid_bit_depth = 24,
      .sample_type = SampleType::INT_MSB,
      .reserved = 0,
  };
};

class FakeController;
using FakeControllerType = ddk::Device<FakeController>;

class FakeController : public FakeControllerType, public ddk::IhdaCodecProtocol<FakeController> {
 public:
  explicit FakeController(zx_device_t* parent)
      : FakeControllerType(parent), loop_(&kAsyncLoopConfigNeverAttachToThread) {}
  ~FakeController() { loop_.Shutdown(); }
  zx_device_t* dev() { return reinterpret_cast<zx_device_t*>(this); }
  zx_status_t Bind() { return DdkAdd("fake-controller-device-test"); }
  void DdkRelease() { delete this; }

  ihda_codec_protocol_t proto() const {
    ihda_codec_protocol_t proto;
    proto.ctx = const_cast<FakeController*>(this);
    proto.ops = const_cast<ihda_codec_protocol_ops_t*>(&ihda_codec_protocol_ops_);
    return proto;
  }

  // ZX_PROTOCOL_IHDA_CODEC Interface
  zx_status_t IhdaCodecGetDriverChannel(zx::channel* out_channel) {
    zx::channel channel_local;
    zx::channel channel_remote;
    zx_status_t status = zx::channel::create(0, &channel_local, &channel_remote);
    if (status != ZX_OK) {
      return status;
    }

    fbl::RefPtr<Channel> channel = Channel::Create(std::move(channel_local));
    if (channel == nullptr) {
      return ZX_ERR_NO_MEMORY;
    }
    codec_driver_channel_ = channel;
    codec_driver_channel_->SetHandler([](async_dispatcher_t* dispatcher, async::WaitBase* wait,
                                         zx_status_t status, const zx_packet_signal_t* signal) {});
    status = codec_driver_channel_->BeginWait(loop_.dispatcher());
    if (status != ZX_OK) {
      codec_driver_channel_.reset();
      return status;
    }

    if (status == ZX_OK) {
      *out_channel = std::move(channel_remote);
    }

    return status;
  }

 private:
  async::Loop loop_;
  fbl::RefPtr<Channel> codec_driver_channel_;
};

class SstStreamTest : public zxtest::Test {
 public:
  void SetUp() override {
    fake_controller_ = new FakeController(root_.get());
    ASSERT_OK(fake_controller_->Bind());  // fake_controller_ is owned by mock ddk.
    auto fake_controller_mock_device = root_->GetLatestChild();
    fake_controller_mock_device->AddProtocol(ZX_PROTOCOL_IHDA_CODEC, fake_controller_->proto().ops,
                                             fake_controller_->proto().ctx);

    codec_ = fbl::AdoptRef(new TestCodec);
    auto ret = codec_->Bind(fake_controller_mock_device, "test");
    ASSERT_OK(ret.status_value());
    stream_ = fbl::AdoptRef(new TestStream);
    ASSERT_OK(codec_->ActivateStream(stream_));
    ASSERT_OK(stream_->Bind());

    loop_.StartThread();
    auto endpoints = fidl::CreateEndpoints<fuchsia_hardware_audio::DaiConnector>();
    fidl::BindServer(loop_.dispatcher(), std::move(endpoints->server), stream_.get());
    client_.Bind(std::move(endpoints->client));
  }

  void TearDown() override {
    loop_.Shutdown();
    fake_controller_->DdkAsyncRemove();
    mock_ddk::ReleaseFlaggedDevices(root_.get());
  }

 protected:
  std::shared_ptr<MockDevice> root_ = MockDevice::FakeRootParent();
  FakeController* fake_controller_;
  fbl::RefPtr<TestCodec> codec_;
  fbl::RefPtr<TestStream> stream_;
  fidl::WireSyncClient<fuchsia_hardware_audio::DaiConnector> client_;
  async::Loop loop_{&kAsyncLoopConfigNeverAttachToThread};
};

TEST_F(SstStreamTest, GetStreamProperties) {
  auto stream_client = GetDaiClient(client_);
  ASSERT_TRUE(stream_client.is_valid());

  auto result = stream_client->GetProperties();
  ASSERT_OK(result.status());

  auto& properties = result->properties;

  const char* kManufacturer = "Intel";
  ASSERT_BYTES_EQ(properties.manufacturer().data(), kManufacturer, strlen(kManufacturer));
  ASSERT_BYTES_EQ(properties.product_name().data(), kTestProductName, strlen(kTestProductName));
  EXPECT_EQ(properties.is_input(), false);

  // Must report a clock domain. Reports monotonic, i.e. 0.
  ASSERT_TRUE(properties.has_clock_domain());
  ASSERT_EQ(properties.clock_domain(), fuchsia_hardware_audio::wire::kClockDomainMonotonic);
}

TEST_F(SstStreamTest, Reset) {
  auto stream_client = GetDaiClient(client_);
  ASSERT_TRUE(stream_client.is_valid());

  auto result = stream_client->Reset();
  ASSERT_OK(result.status());
}

TEST_F(SstStreamTest, GetRingBufferFormats) {
  auto stream_client = GetDaiClient(client_);
  ASSERT_TRUE(stream_client.is_valid());

  auto result = stream_client->GetRingBufferFormats();
  ASSERT_OK(result.status());
  auto& formats = result.value().value()->ring_buffer_formats;
  EXPECT_EQ(formats.size(), 1);
  EXPECT_EQ(formats[0].pcm_supported_formats().channel_sets().size(), 1);
  EXPECT_EQ(formats[0].pcm_supported_formats().channel_sets()[0].attributes().size(), 2);
  EXPECT_EQ(formats[0].pcm_supported_formats().sample_formats().size(), 1);
  EXPECT_EQ(formats[0].pcm_supported_formats().sample_formats()[0],
            fuchsia_hardware_audio::wire::SampleFormat::kPcmSigned);
  EXPECT_EQ(formats[0].pcm_supported_formats().bytes_per_sample().size(), 1);
  EXPECT_EQ(formats[0].pcm_supported_formats().bytes_per_sample()[0], 2);
  EXPECT_EQ(formats[0].pcm_supported_formats().valid_bits_per_sample().size(), 1);
  EXPECT_EQ(formats[0].pcm_supported_formats().valid_bits_per_sample()[0], 16);
  EXPECT_EQ(formats[0].pcm_supported_formats().frame_rates().size(), 1);
  EXPECT_EQ(formats[0].pcm_supported_formats().frame_rates()[0], 48'000);
}

TEST_F(SstStreamTest, GetDaiFormats) {
  auto stream_client = GetDaiClient(client_);
  ASSERT_TRUE(stream_client.is_valid());

  auto result = stream_client->GetDaiFormats();
  ASSERT_OK(result.status());
  auto& formats = result.value().value()->dai_formats;
  EXPECT_EQ(formats.size(), 1);
  EXPECT_EQ(formats[0].number_of_channels.size(), 1);
  EXPECT_EQ(formats[0].number_of_channels[0], 2);  // I2S.
  EXPECT_EQ(formats[0].sample_formats.size(), 1);
  EXPECT_EQ(formats[0].sample_formats[0],
            fuchsia_hardware_audio::wire::DaiSampleFormat::kPcmSigned);
  EXPECT_EQ(formats[0].frame_formats.size(), 1);
  EXPECT_EQ(formats[0].frame_formats[0].frame_format_standard(),
            fuchsia_hardware_audio::wire::DaiFrameFormatStandard::kI2S);
  EXPECT_EQ(formats[0].frame_rates.size(), 1);
  EXPECT_EQ(formats[0].frame_rates[0], 48'000);
  EXPECT_EQ(formats[0].bits_per_slot.size(), 1);
  EXPECT_EQ(formats[0].bits_per_slot[0], 32);
  EXPECT_EQ(formats[0].bits_per_sample.size(), 1);
  EXPECT_EQ(formats[0].bits_per_sample[0], 24);
}

class TestStream2 : public IntelDspStream {
 public:
  TestStream2()
      : IntelDspStream(DspStream{.id = DspPipelineId{4},
                                 .host_format = kTestHostFormat,
                                 .dai_format = kTestDaiFormat,
                                 .is_i2s = false,
                                 .stream_id = 1,
                                 .is_input = true,
                                 .uid = AUDIO_STREAM_UNIQUE_ID_BUILTIN_HEADPHONE_JACK,
                                 .name = kTestProductName}) {}

  zx_status_t Bind() {
    fbl::AutoLock lock(obj_lock());
    return PublishDeviceLocked();
  }

 private:
  static constexpr AudioDataFormat kTestHostFormat = {
      .sampling_frequency = SamplingFrequency::FS_96000HZ,
      .bit_depth = BitDepth::DEPTH_32BIT,
      .channel_map = 0xFFFF3210,
      .channel_config = ChannelConfig::CONFIG_QUATRO,
      .interleaving_style = InterleavingStyle::PER_CHANNEL,
      .number_of_channels = 4,
      .valid_bit_depth = 24,
      .sample_type = SampleType::INT_MSB,
      .reserved = 0,
  };
  static constexpr AudioDataFormat kTestDaiFormat = {
      .sampling_frequency = SamplingFrequency::FS_96000HZ,
      .bit_depth = BitDepth::DEPTH_16BIT,
      .channel_map = 0xFFFF3210,
      .channel_config = ChannelConfig::CONFIG_QUATRO,
      .interleaving_style = InterleavingStyle::PER_CHANNEL,
      .number_of_channels = 4,
      .valid_bit_depth = 16,
      .sample_type = SampleType::INT_MSB,
      .reserved = 0,
  };
};

TEST(SstStream, GetDaiFormats2) {
  std::shared_ptr<MockDevice> root = MockDevice::FakeRootParent();
  FakeController* fake_controller = new FakeController(root.get());
  ASSERT_OK(fake_controller->Bind());
  auto fake_controller_mock_device = root->GetLatestChild();
  fake_controller_mock_device->AddProtocol(ZX_PROTOCOL_IHDA_CODEC, fake_controller->proto().ops,
                                           fake_controller->proto().ctx);
  fbl::RefPtr<TestCodec> codec = fbl::AdoptRef(new TestCodec);
  auto ret = codec->Bind(fake_controller_mock_device, "test");
  ASSERT_OK(ret.status_value());
  auto stream = fbl::AdoptRef(new TestStream2);
  ASSERT_OK(codec->ActivateStream(stream));
  ASSERT_OK(stream->Bind());

  async::Loop loop{&kAsyncLoopConfigNeverAttachToThread};
  loop.StartThread();
  auto endpoints = fidl::CreateEndpoints<fuchsia_hardware_audio::DaiConnector>();
  fidl::BindServer(loop.dispatcher(), std::move(endpoints->server), stream.get());

  fidl::WireSyncClient<fuchsia_hardware_audio::DaiConnector> client;
  client.Bind(std::move(endpoints->client));

  auto stream_client = GetDaiClient(client);
  ASSERT_TRUE(stream_client.is_valid());

  auto result = stream_client->GetDaiFormats();
  ASSERT_OK(result.status());
  auto& formats = result.value().value()->dai_formats;
  EXPECT_EQ(formats.size(), 1);
  EXPECT_EQ(formats[0].number_of_channels.size(), 1);
  EXPECT_EQ(formats[0].number_of_channels[0], 8);  // TDM.
  EXPECT_EQ(formats[0].sample_formats.size(), 1);
  EXPECT_EQ(formats[0].sample_formats[0],
            fuchsia_hardware_audio::wire::DaiSampleFormat::kPcmSigned);
  EXPECT_EQ(formats[0].frame_formats.size(), 1);
  EXPECT_EQ(formats[0].frame_formats[0].frame_format_standard(),
            fuchsia_hardware_audio::wire::DaiFrameFormatStandard::kTdm1);
  EXPECT_EQ(formats[0].frame_rates.size(), 1);
  EXPECT_EQ(formats[0].frame_rates[0], 96'000);
  EXPECT_EQ(formats[0].bits_per_slot.size(), 1);
  EXPECT_EQ(formats[0].bits_per_slot[0], 16);
  EXPECT_EQ(formats[0].bits_per_sample.size(), 1);
  EXPECT_EQ(formats[0].bits_per_sample[0], 16);

  loop.Shutdown();
  fake_controller->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root.get());
}

}  // namespace audio::intel_hda

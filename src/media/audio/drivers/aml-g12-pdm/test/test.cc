// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/default.h>
#include <lib/async-loop/loop.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/fake-mmio-reg/cpp/fake-mmio-reg.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>
#include <lib/fidl/cpp/wire/connect_service.h>
#include <lib/inspect/testing/cpp/zxtest/inspect.h>
#include <lib/sync/completion.h>

#include <soc/aml-s905d2/s905d2-hw.h>
#include <zxtest/zxtest.h>

#include "../audio-stream-in.h"
#include "src/devices/testing/mock-ddk/mock-device.h"

namespace audio::aml_g12 {

namespace audio_fidl = fuchsia_hardware_audio;

fidl::WireSyncClient<audio_fidl::StreamConfig> GetStreamClient(
    fidl::ClientEnd<audio_fidl::StreamConfigConnector> client) {
  fidl::WireSyncClient client_wrap{std::move(client)};
  if (!client_wrap.is_valid()) {
    return {};
  }
  auto [stream_channel_local, stream_channel_remote] =
      fidl::Endpoints<audio_fidl::StreamConfig>::Create();
  auto result = client_wrap->Connect(std::move(stream_channel_remote));
  if (!result.ok()) {
    return {};
  }
  return fidl::WireSyncClient<audio_fidl::StreamConfig>(std::move(stream_channel_local));
}

class FakeMmio {
 public:
  FakeMmio() : mmio_(sizeof(uint32_t), kRegCount) {}

  fdf::MmioBuffer mmio() { return mmio_.GetMmioBuffer(); }
  fake_mmio::FakeMmioReg& reg(size_t ix) { return mmio_[ix]; }

 private:
  static constexpr size_t kRegCount =
      S905D2_EE_AUDIO_LENGTH / sizeof(uint32_t);  // in 32 bits chunks.
  fake_mmio::FakeMmioRegRegion mmio_;
};

metadata::AmlPdmConfig GetDefaultMetadata() {
  metadata::AmlPdmConfig metadata = {};
  snprintf(metadata.manufacturer, sizeof(metadata.manufacturer), "Test");
  snprintf(metadata.product_name, sizeof(metadata.product_name), "Test");
  metadata.number_of_channels = 2;
  metadata.version = metadata::AmlVersion::kS905D3G;
  metadata.sysClockDivFactor = 4;
  metadata.dClockDivFactor = 250;
  return metadata;
}

class TestAudioStreamIn : public AudioStreamIn {
 public:
  explicit TestAudioStreamIn(zx_device_t* parent) : AudioStreamIn(parent) {}
  bool AllowNonContiguousRingBuffer() override { return true; }
  inspect::Inspector& inspect() { return AudioStreamIn::inspect(); }
};

audio_fidl::wire::PcmFormat GetDefaultPcmFormat() {
  audio_fidl::wire::PcmFormat format;
  format.number_of_channels = 2;
  format.sample_format = audio_fidl::wire::SampleFormat::kPcmSigned;
  format.frame_rate = 48'000;
  format.bytes_per_sample = 2;
  format.valid_bits_per_sample = 16;
  return format;
}

struct IncomingNamespace {
  fdf_fake::FakePDev pdev_server;
  component::OutgoingDirectory outgoing{async_get_default_dispatcher()};
};

struct AudioStreamInTest : public inspect::InspectTestHelper, public zxtest::Test {
  void SetUp() override {
    fake_parent_ = MockDevice::FakeRootParent();

    fdf_fake::FakePDev::Config config;

    config.mmios[0] = mmio_.mmio();
    config.mmios[1] = mmio_.mmio();
    config.use_fake_bti = true;
    zx::interrupt irq;
    ASSERT_OK(zx::interrupt::create(zx::resource(), 0, ZX_INTERRUPT_VIRTUAL, &irq));
    config.irqs[0] = std::move(irq);

    auto outgoing_endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
    ASSERT_OK(incoming_loop_.StartThread("incoming-ns-thread"));
    incoming_.SyncCall([config = std::move(config), server = std::move(outgoing_endpoints.server)](
                           IncomingNamespace* infra) mutable {
      infra->pdev_server.SetConfig(std::move(config));
      ASSERT_OK(infra->outgoing.AddService<fuchsia_hardware_platform_device::Service>(
          infra->pdev_server.GetInstanceHandler()));

      ASSERT_OK(infra->outgoing.Serve(std::move(server)));
    });
    ASSERT_NO_FATAL_FAILURE();
    fake_parent_->AddFidlService(fuchsia_hardware_platform_device::Service::Name,
                                 std::move(outgoing_endpoints.client), "pdev");
  }

  void TestRingBufferSize(uint8_t number_of_channels, uint32_t frames_req,
                          uint32_t frames_expected) {
    auto metadata = GetDefaultMetadata();
    metadata.number_of_channels = number_of_channels;
    fake_parent_->SetMetadata(DEVICE_METADATA_PRIVATE, &metadata, sizeof(metadata));

    auto stream = audio::SimpleAudioStream::Create<TestAudioStreamIn>(fake_parent_.get());

    auto connector_endpoints = fidl::Endpoints<audio_fidl::StreamConfigConnector>::Create();

    loop_.StartThread("fidl-thread");
    fidl::BindServer(loop_.dispatcher(), std::move(connector_endpoints.server), stream.get());

    auto stream_client = GetStreamClient(std::move(connector_endpoints.client));
    ASSERT_TRUE(stream_client.is_valid());
    auto [local, remote] = fidl::Endpoints<audio_fidl::RingBuffer>::Create();

    audio_fidl::wire::PcmFormat pcm_format = GetDefaultPcmFormat();
    pcm_format.number_of_channels = number_of_channels;

    fidl::Arena allocator;
    audio_fidl::wire::Format format(allocator);
    format.set_pcm_format(allocator, std::move(pcm_format));

    auto result = stream_client->CreateRingBuffer(std::move(format), std::move(remote));
    ASSERT_OK(result.status());

    auto vmo = fidl::WireCall(local)->GetVmo(frames_req, 0);
    ASSERT_OK(vmo.status());
    ASSERT_EQ(vmo->value()->num_frames, frames_expected);

    stream->DdkAsyncRemove();
    mock_ddk::ReleaseFlaggedDevices(fake_parent_.get());
  }

  async::Loop incoming_loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
  async_patterns::TestDispatcherBound<IncomingNamespace> incoming_{incoming_loop_.dispatcher(),
                                                                   std::in_place};
  FakeMmio mmio_;
  std::shared_ptr<MockDevice> fake_parent_;
  async::Loop loop_{&kAsyncLoopConfigNeverAttachToThread};
};

// With 16 bits samples, frame size is 2 x number of channels bytes. FIFO is 1024 bytes.
// Frames returned depend on client requested min_frames, HW buffer alignment (8 bytes)
// and FIFO size.

// min_frames + FIFO size. With number of channels = 2 (frame size = 4):
// num_frames = min_frames + (FIFO / frame_size) = 2 + (1024 / 4) = 258.
TEST_F(AudioStreamInTest, RingBufferSize1) { TestRingBufferSize(2, 2, 258); }

// Rounded to HW buffer alignment (8). With number of channels = 2 (frame size = 4),
// num_frames = min_frames + (FIFO / frame_size) = 257 + (1024 / 4) = 513, but then
// it is rounded to 2 frames alignmend (8 bytes) to 514.
TEST_F(AudioStreamInTest, RingBufferSize2) { TestRingBufferSize(2, 257, 514); }

// Rounded to HW buffer alignment (8). With number of channels = 3 (frame size = 6),
// num_frames = min_frames + (FIFO / frame_size) = 1 + (1024 / 6) = 171, but then
// it is rounded to 2 frames alignmend (8 bytes) to 172.
TEST_F(AudioStreamInTest, RingBufferSize3) { TestRingBufferSize(3, 1, 172); }

// Rounded to HW buffer alignment (8). With number of channels = 3 (frame size = 6),
// num_frames = min_frames + (FIFO / frame_size) = 341 + (1024 / 6) = 511, but then
// it is rounded to 2 frames alignmend (8 bytes) to 512.
TEST_F(AudioStreamInTest, RingBufferSize4) { TestRingBufferSize(3, 341, 512); }

TEST_F(AudioStreamInTest, Inspect) {
  auto metadata = GetDefaultMetadata();
  fake_parent_->SetMetadata(DEVICE_METADATA_PRIVATE, &metadata, sizeof(metadata));

  auto server = audio::SimpleAudioStream::Create<TestAudioStreamIn>(fake_parent_.get());
  ASSERT_NOT_NULL(server);

  auto connector_endpoints = fidl::Endpoints<audio_fidl::StreamConfigConnector>::Create();

  loop_.StartThread("fidl-thread");
  fidl::BindServer(loop_.dispatcher(), std::move(connector_endpoints.server), server.get());

  auto stream_client = GetStreamClient(std::move(connector_endpoints.client));
  ASSERT_TRUE(stream_client.is_valid());

  audio_fidl::wire::PcmFormat pcm_format = GetDefaultPcmFormat();

  fidl::Arena allocator;
  audio_fidl::wire::Format format(allocator);
  format.set_pcm_format(allocator, std::move(pcm_format));

  auto [local, remote] = fidl::Endpoints<audio_fidl::RingBuffer>::Create();

  auto result = stream_client->CreateRingBuffer(std::move(format), std::move(remote));
  ASSERT_OK(result.status());

  auto props = fidl::WireCall(local)->GetProperties();
  ASSERT_OK(props.status());

  // Check inspect state.
  ASSERT_NO_FATAL_FAILURE(ReadInspect(server->inspect().DuplicateVmo()));
  auto* simple_audio = hierarchy().GetByPath({"simple_audio_stream"});
  ASSERT_TRUE(simple_audio);
  ASSERT_NO_FATAL_FAILURE(
      CheckProperty(simple_audio->node(), "state", inspect::StringPropertyValue("created")));
  ASSERT_NO_FATAL_FAILURE(
      CheckProperty(hierarchy().node(), "status_time", inspect::IntPropertyValue(0)));
  ASSERT_NO_FATAL_FAILURE(
      CheckProperty(hierarchy().node(), "dma_status", inspect::UintPropertyValue(0)));
  ASSERT_NO_FATAL_FAILURE(
      CheckProperty(hierarchy().node(), "pdm_status", inspect::UintPropertyValue(0)));

  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(fake_parent_.get());
}

}  // namespace audio::aml_g12

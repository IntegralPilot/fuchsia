// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.amlogiccanvas/cpp/wire_test_base.h>
#include <fidl/fuchsia.hardware.clock/cpp/wire_test_base.h>
#include <fidl/fuchsia.sysmem2/cpp/wire_test_base.h>
#include <lib/async-loop/default.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/fake-clock/cpp/fake-clock.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>
#include <lib/media/codec_impl/codec_buffer.h>
#include <lib/media/codec_impl/codec_packet.h>
#include <zircon/types.h>

#include <gtest/gtest.h>

#include "src/devices/testing/mock-ddk/mock-device.h"
#include "src/media/drivers/amlogic_decoder/codec_adapter_h264_multi.h"
#include "src/media/drivers/amlogic_decoder/device_ctx.h"

namespace amlogic_decoder {
namespace test {

class FakeSysmem : public fidl::testing::WireTestBase<fuchsia_sysmem2::Allocator> {
 public:
  FakeSysmem() {}

  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) final {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }
};

class FakeCanvas : public fidl::testing::WireTestBase<fuchsia_hardware_amlogiccanvas::Device> {
 public:
  FakeCanvas() {}

  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) final {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }
};

struct IncomingNamespace {
  fdf_fake::FakePDev pdev_server;
  component::OutgoingDirectory outgoing{async_get_default_dispatcher()};
  FakeSysmem fake_sysmem;
  component::OutgoingDirectory outgoing_sysmem{async_get_default_dispatcher()};
  FakeCanvas fake_canvas;
  component::OutgoingDirectory outgoing_canvas{async_get_default_dispatcher()};
  fdf_fake::FakeClock fake_gclk_vdec{async_get_default_dispatcher()};
  component::OutgoingDirectory outgoing_gclk_vdec{async_get_default_dispatcher()};
  fdf_fake::FakeClock fake_clk_dos{async_get_default_dispatcher()};
  component::OutgoingDirectory outgoing_clk_dos{async_get_default_dispatcher()};
};

constexpr uint32_t kBufferLifetimeOrdinal = 1;

static CodecVmoRange VmoRangeOfSize(size_t size) {
  zx::vmo vmo_handle;
  zx_status_t status = zx::vmo::create(size, 0, &vmo_handle);
  ZX_ASSERT(status == ZX_OK);
  return CodecVmoRange(std::move(vmo_handle), 0, size);
}

class CodecBufferForTest : public CodecBuffer {
 public:
  CodecBufferForTest(size_t size, uint32_t index, bool is_secure)
      : CodecBuffer(/*parent=*/nullptr,
                    Info{.port = kOutputPort,
                         .lifetime_ordinal = kBufferLifetimeOrdinal,
                         .index = index,
                         .is_secure = is_secure},
                    VmoRangeOfSize(size)) {
    if (!Map()) {
      ZX_PANIC("CodecBufferForTest() failed to Map()");
    }
  }
};

class CodecPacketForTest : public CodecPacket {
 public:
  CodecPacketForTest(uint32_t index) : CodecPacket(kBufferLifetimeOrdinal, index) {}
};

class DummyEvents : public CodecAdapterEvents {
 public:
  void onCoreCodecFailCodec(const char* format, ...) override {}
  void onCoreCodecFailStream(fuchsia::media::StreamError error) override {}
  void onCoreCodecResetStreamAfterCurrentFrame() override {}
  void onCoreCodecMidStreamOutputConstraintsChange(
      bool buffer_constraints_action_required) override {}
  void onCoreCodecOutputFormatChange() override {}
  void onCoreCodecOutputPacket(CodecPacket* packet, bool error_detected_before,
                               bool error_detected_during) override {}
  void onCoreCodecOutputEndOfStream(bool error_detected_before) override {}
  void onCoreCodecInputPacketDone(CodecPacket* packet) override {}
  void onCoreCodecOutputTimestampHasNoOutput(uint64_t timestamp_ish) override {}
  void onCoreCodecLogEvent(
      media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent event_code) override {}
};
class BindingTest : public testing::Test {
 protected:
  void InitPdev() {
    fdf_fake::FakePDev::Config config;
    config.use_fake_bti = true;
    config.use_fake_irq = true;

    config.device_info = {
        .mmio_count = 5,
        .irq_count = 4,
    };
    for (uint32_t i = 0; i < config.device_info->mmio_count; i++) {
      // Large enough for any memory range, including AOBUS and CBUS.
      constexpr uint64_t kMmioSize = 0x100000;
      zx::vmo vmo;
      ASSERT_EQ(ZX_OK, zx::vmo::create(kMmioSize, 0, &vmo));
      auto mmio_buffer =
          fdf::MmioBuffer::Create(0, kMmioSize, std::move(vmo), ZX_CACHE_POLICY_CACHED);
      ASSERT_EQ(ZX_OK, mmio_buffer.status_value());

      config.mmios[i] = std::move(*mmio_buffer);
    }

    auto outgoing_endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
    ASSERT_EQ(ZX_OK, incoming_loop_.StartThread("incoming-ns-thread"));
    incoming_.SyncCall([config = std::move(config), server = std::move(outgoing_endpoints.server)](
                           IncomingNamespace* infra) mutable {
      infra->pdev_server.SetConfig(std::move(config));
      ASSERT_EQ(ZX_OK, infra->outgoing
                           .AddService<fuchsia_hardware_platform_device::Service>(
                               infra->pdev_server.GetInstanceHandler())
                           .status_value());

      ASSERT_EQ(ZX_OK, infra->outgoing.Serve(std::move(server)).status_value());
    });
    ASSERT_NO_FATAL_FAILURE();
    root_->AddFidlService(fuchsia_hardware_platform_device::Service::Name,
                          std::move(outgoing_endpoints.client), "pdev");
  }
  void InitSysmem() {
    root_->AddNsProtocol<fuchsia_sysmem2::Allocator>(
        incoming_.SyncCall([](IncomingNamespace* infra) mutable {
          return infra->fake_sysmem.bind_handler(async_get_default_dispatcher());
        }));
  }

  void InitCanvas() {
    auto outgoing_endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
    incoming_.SyncCall([server = std::move(outgoing_endpoints.server)](
                           IncomingNamespace* infra) mutable {
      ASSERT_EQ(
          ZX_OK,
          infra->outgoing_canvas
              .AddService<fuchsia_hardware_platform_device::Service>(
                  fuchsia_hardware_amlogiccanvas::Service::InstanceHandler(
                      {.device = infra->fake_canvas.bind_handler(async_get_default_dispatcher())}))
              .status_value());

      ASSERT_EQ(ZX_OK, infra->outgoing_canvas.Serve(std::move(server)).status_value());
    });
    root_->AddFidlService(fuchsia_hardware_amlogiccanvas::Service::Name,
                          std::move(outgoing_endpoints.client), "canvas");
  }

  void InitGclkVdec() {
    auto outgoing_endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
    incoming_.SyncCall(
        [server = std::move(outgoing_endpoints.server)](IncomingNamespace* infra) mutable {
          ASSERT_EQ(ZX_OK, infra->outgoing_gclk_vdec
                               .AddService<fuchsia_hardware_clock::Service>(
                                   infra->fake_gclk_vdec.CreateInstanceHandler())
                               .status_value());

          ASSERT_EQ(ZX_OK, infra->outgoing_gclk_vdec.Serve(std::move(server)).status_value());
        });
    root_->AddFidlService(fuchsia_hardware_clock::Service::Name,
                          std::move(outgoing_endpoints.client), "clock-dos-vdec");
  }

  void InitClkDos() {
    auto outgoing_endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
    incoming_.SyncCall(
        [server = std::move(outgoing_endpoints.server)](IncomingNamespace* infra) mutable {
          ASSERT_EQ(ZX_OK, infra->outgoing_clk_dos
                               .AddService<fuchsia_hardware_clock::Service>(
                                   infra->fake_clk_dos.CreateInstanceHandler())
                               .status_value());

          ASSERT_EQ(ZX_OK, infra->outgoing_clk_dos.Serve(std::move(server)).status_value());
        });
    root_->AddFidlService(fuchsia_hardware_clock::Service::Name,
                          std::move(outgoing_endpoints.client), "clock-dos");
  }

  void InitFirmware() {
    // Firmware that's smaller than the header size will be ignored.
    root_->SetFirmware(std::vector<uint8_t>{0}, "amlogic_video_ucode.bin");
  }

  DeviceCtx* Init() {
    InitPdev();
    InitSysmem();
    InitCanvas();
    InitGclkVdec();
    InitClkDos();
    InitFirmware();
    auto device = std::make_unique<DeviceCtx>(&driver_ctx_, root_.get());
    amlogic_decoder::AmlogicVideo* video = device->video();
    video->SetDeviceType(DeviceType::kSM1);
    EXPECT_EQ(ZX_OK, video->InitRegisters(root_.get()));
    EXPECT_EQ(ZX_OK, video->InitDecoder());

    EXPECT_EQ(ZX_OK, device->Bind());

    // The root device has taken ownership of the device.
    return device.release();
  }

  async::Loop incoming_loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
  async_patterns::TestDispatcherBound<IncomingNamespace> incoming_{incoming_loop_.dispatcher(),
                                                                   std::in_place};
  DriverCtx driver_ctx_;
  std::shared_ptr<MockDevice> root_ = MockDevice::FakeRootParent();
};

TEST_F(BindingTest, Destruction) {
  auto device = Init();
  device->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
  root_.reset();
}

TEST_F(BindingTest, Suspend) {
  auto device = Init();
  ASSERT_EQ(1u, root_->child_count());
  auto* child = root_->GetLatestChild();
  ddk::SuspendTxn txn(device->zxdev(), 0, false, DEVICE_SUSPEND_REASON_REBOOT);
  device->DdkSuspend(std::move(txn));
  child->WaitUntilSuspendReplyCalled();

  root_.reset();
}

TEST_F(BindingTest, H264AdapterAvccParsing) {
  auto device = Init();

  DummyEvents events;
  std::mutex lock;

  std::unique_ptr<CodecAdapterH264Multi> adapter;
  {
    libsync::Completion adapter_created;
    async::PostTask(device->driver()->shared_fidl_loop()->dispatcher(), [&] {
      adapter = std::make_unique<CodecAdapterH264Multi>(lock, &events, device);
      adapter_created.Signal();
    });
    adapter_created.Wait();
  }

  // Prepare OOB bytes in AVCC format.
  std::vector<uint8_t> oob_bytes = {
      1,                     // version
      66,   0,  31,          // profile/compatibility/level
      0xFF,                  // pseudo_nal_length_field_bytes - 1 = 3 (so 4 bytes)
      0xE1,                  // SPS count = 1 (upper bits reserved)
      0,    5,               // SPS length
      10,   11, 12, 13, 14,  // SPS
      1,                     // PPS count
      0,    4,               // PPS length
      20,   21, 22, 23       // PPS
  };

  fuchsia::media::FormatDetails format_details;
  format_details.set_mime_type("video/h264");
  format_details.set_oob_bytes(oob_bytes);

  adapter->CoreCodecInit(format_details);
  adapter->CoreCodecQueueInputFormatDetails(format_details);

  // Prepare the AVCC input stream data:
  // Length prefix = 5 (4 bytes: {0, 0, 0, 5})
  // NAL payload = {100, 101, 102, 103, 104}
  std::vector<uint8_t> avcc_payload = {0, 0, 0, 5, 100, 101, 102, 103, 104};

  auto input_buffer = std::make_unique<CodecBufferForTest>(avcc_payload.size(), 0, false);
  memcpy(input_buffer->base(), avcc_payload.data(), avcc_payload.size());

  auto input_packet = std::make_unique<CodecPacketForTest>(0);
  input_packet->SetBuffer(input_buffer.get());
  input_packet->SetStartOffset(0);
  input_packet->SetValidLengthBytes(static_cast<uint32_t>(avcc_payload.size()));

  adapter->CoreCodecQueueInputPacket(input_packet.get());

  // First ReadMoreInputData call processes the OOB bytes
  auto oob_result = adapter->ReadMoreInputData();
  ASSERT_TRUE(oob_result.has_value());

  // Second ReadMoreInputData call processes the packet using ParseVideoAvcc
  auto result = adapter->ReadMoreInputData();
  ASSERT_TRUE(result.has_value());
  EXPECT_EQ(9u, result->length);  // 4 bytes AnnexB start code + 5 bytes payload = 9

  // Verify that the converted Annex B output matches: {0, 0, 0, 1, 100, 101, 102, 103, 104}
  std::vector<uint8_t> expected_annex_b = {0, 0, 0, 1, 100, 101, 102, 103, 104};
  EXPECT_EQ(expected_annex_b, result->data);

  // Done, clean up.
  {
    libsync::Completion adapter_deleted;
    async::PostTask(device->driver()->shared_fidl_loop()->dispatcher(), [&] {
      adapter.reset();
      adapter_deleted.Signal();
    });
    adapter_deleted.Wait();
  }
  device->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
  root_.reset();
}

TEST_F(BindingTest, H264AdapterAvccToctouRace) {
  auto device = Init();

  DummyEvents events;
  std::mutex lock;

  std::unique_ptr<CodecAdapterH264Multi> adapter;
  {
    libsync::Completion adapter_created;
    async::PostTask(device->driver()->shared_fidl_loop()->dispatcher(), [&] {
      adapter = std::make_unique<CodecAdapterH264Multi>(lock, &events, device);
      adapter_created.Signal();
    });
    adapter_created.Wait();
  }

  // Configure oob_bytes for pseudo_nal_length_field_bytes_ = 1
  std::vector<uint8_t> oob_bytes = {
      1,                     // version
      66,   0,  31,          // profile/compatibility/level
      0xFC,                  // pseudo_nal_length_field_bytes - 1 = 0 (1 byte)
      0xE1,                  // SPS count = 1 (upper bits reserved)
      0,    5,               // SPS length
      10,   11, 12, 13, 14,  // SPS
      1,                     // PPS count
      0,    4,               // PPS length
      20,   21, 22, 23       // PPS
  };

  fuchsia::media::FormatDetails format_details;
  format_details.set_mime_type("video/h264");
  format_details.set_oob_bytes(oob_bytes);

  adapter->CoreCodecInit(format_details);
  adapter->CoreCodecQueueInputFormatDetails(format_details);

  // Prepare the input buffer of size 10040
  auto input_buffer = std::make_unique<CodecBufferForTest>(10040, 0, false);

  auto input_packet = std::make_unique<CodecPacketForTest>(0);
  input_packet->SetBuffer(input_buffer.get());
  input_packet->SetStartOffset(0);
  input_packet->SetValidLengthBytes(10040);

  // First ReadMoreInputData call processes the OOB bytes
  adapter->CoreCodecQueueInputPacket(input_packet.get());
  auto oob_result = adapter->ReadMoreInputData();
  ASSERT_TRUE(oob_result.has_value());

  // Background thread to constantly mutate the buffer content
  std::atomic<bool> running = true;
  std::atomic<bool> signal_mutate = false;
  std::thread race_thread([&running, &signal_mutate, base = input_buffer->base()]() {
    std::vector<uint8_t> state_a(10040);
    for (int i = 0; i < 40; ++i) {
      state_a[i * 251] = 250;
    }
    while (running) {
      if (signal_mutate) {
        // Wait for Pass 1 to finish reading State A, then switch to State B (0s)
        zx::nanosleep(zx::deadline_after(zx::usec(5)));
        memset(base, 0, 10040);
        signal_mutate = false;
      } else {
        memcpy(base, state_a.data(), 10040);
        zx::nanosleep(zx::deadline_after(zx::usec(1)));
      }
    }
  });

  // Perform the race attempts
  for (int i = 0; i < 500; ++i) {
    // Reset to State A
    std::vector<uint8_t> state_a(10040);
    for (int j = 0; j < 40; ++j) {
      state_a[j * 251] = 250;
    }
    memcpy(input_buffer->base(), state_a.data(), 10040);

    adapter->CoreCodecQueueInputPacket(input_packet.get());

    // Signal background thread to mutate to State B in 5 microseconds
    signal_mutate = true;

    auto result = adapter->ReadMoreInputData();

    signal_mutate = false;
  }

  running = false;
  race_thread.join();

  // Done, clean up.
  {
    libsync::Completion adapter_deleted;
    async::PostTask(device->driver()->shared_fidl_loop()->dispatcher(), [&] {
      adapter.reset();
      adapter_deleted.Signal();
    });
    adapter_deleted.Wait();
  }
  device->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
  root_.reset();
}

}  // namespace test
}  // namespace amlogic_decoder

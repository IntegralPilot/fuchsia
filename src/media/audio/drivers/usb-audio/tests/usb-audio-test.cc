// Copyright 2021 The Fuchsia Authors. All rights reserved.  Use of
// this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.midi/cpp/wire.h>
#include <fuchsia/hardware/usb/composite/cpp/banjo.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/default.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/inspect/testing/cpp/zxtest/inspect.h>
#include <lib/sync/completion.h>

#include <iterator>
#include <memory>
#include <mutex>
#include <optional>
#include <vector>

#include <ddktl/device.h>
#include <usb/request-cpp.h>
#include <zxtest/zxtest.h>

#include "src/devices/testing/mock-ddk/mock-device.h"
#include "src/devices/usb/lib/usb-endpoint/testing/fake-usb-endpoint-server.h"
#include "src/media/audio/drivers/usb-audio/usb-audio-device.h"
#include "src/media/audio/drivers/usb-audio/usb-audio-stream.h"
#include "src/media/audio/drivers/usb-audio/usb-midi-source.h"

namespace {
namespace audio_fidl = fuchsia_hardware_audio;
using Request = usb::Request<void>;
using UnownedRequest = usb::BorrowedRequest<void>;
using UnownedRequestQueue = usb::BorrowedRequestQueue<void>;

constexpr uint32_t kTestFrameRate = 48'000;
constexpr uint32_t MAX_OUTSTANDING_REQ = 6;  // Matches usb-audio-stream.cc

audio_fidl::wire::PcmFormat GetDefaultPcmFormat() {
  audio_fidl::wire::PcmFormat format;
  format.number_of_channels = 2;
  format.sample_format = audio_fidl::wire::SampleFormat::kPcmSigned;
  format.frame_rate = kTestFrameRate;
  format.bytes_per_sample = 2;
  format.valid_bits_per_sample = 16;
  return format;
}

fidl::WireSyncClient<audio_fidl::StreamConfig> GetStreamClient(
    fidl::ClientEnd<audio_fidl::StreamConfigConnector> client) {
  fidl::WireSyncClient client_wrap{std::move(client)};
  if (!client_wrap.is_valid()) {
    return {};
  }
  auto [stream_channel_local, stream_channel_remote] =
      fidl::Endpoints<audio_fidl::StreamConfig>::Create();
  const fidl::OneWayStatus status = client_wrap->Connect(std::move(stream_channel_remote));
  EXPECT_OK(status);
  return fidl::WireSyncClient<audio_fidl::StreamConfig>(std::move(stream_channel_local));
}

class FakeDevice;
using FakeDeviceType = ddk::Device<FakeDevice>;

class FakeDevice : public FakeDeviceType,
                   public ddk::UsbProtocol<FakeDevice>,
                   public ddk::UsbCompositeProtocol<FakeDevice>,
                   public fake_usb_endpoint::FakeUsbFidlProvider<fuchsia_hardware_usb::Usb> {
 public:
  explicit FakeDevice(zx_device_t* parent)
      : FakeDeviceType(parent),
        fake_usb_endpoint::FakeUsbFidlProvider<fuchsia_hardware_usb::Usb>(
            async_get_default_dispatcher()) {}
  virtual ~FakeDevice() = default;
  // dev() is used in Binder::DeviceGetProtocol below.
  zx_device_t* dev() { return reinterpret_cast<zx_device_t*>(this); }
  zx_status_t Bind() { return DdkAdd("usb-fake-device-test"); }
  void DdkRelease() {}

  usb_protocol_t proto() const {
    usb_protocol_t proto;
    proto.ctx = const_cast<FakeDevice*>(this);
    proto.ops = const_cast<usb_protocol_ops_t*>(&usb_protocol_ops_);
    return proto;
  }
  usb_composite_protocol_t proto_composite() const {
    usb_composite_protocol_t proto;
    proto.ctx = const_cast<FakeDevice*>(this);
    proto.ops = const_cast<usb_composite_protocol_ops_t*>(&usb_composite_protocol_ops_);
    return proto;
  }

  // USB protocol implementation.
  zx_status_t UsbControlOut(uint8_t request_type, uint8_t request, uint16_t value, uint16_t index,
                            int64_t timeout, const uint8_t* write_buffer, size_t write_size) {
    return ZX_OK;
  }
  zx_status_t UsbControlIn(uint8_t request_type, uint8_t request, uint16_t value, uint16_t index,
                           int64_t timeout, uint8_t* out_read_buffer, size_t read_size,
                           size_t* out_read_actual) {
    struct {
      uint8_t request_type;
      uint8_t request;
      uint16_t value;
      uint16_t index;
      uint8_t data0;
      std::optional<uint8_t> data1;
    } canned_replies[] = {
        // clang-format off
        {.request_type=0xA1, .request=0x82, .value=0x201, .index=0x900, .data0=0x00, .data1=0xdb},
        {.request_type=0xA1, .request=0x83, .value=0x201, .index=0x900, .data0=0x00, .data1=0x00},
        {.request_type=0xA1, .request=0x84, .value=0x201, .index=0x900, .data0=0x00, .data1=0x01},
        {.request_type=0xA1, .request=0x82, .value=0x202, .index=0x900, .data0=0x00, .data1=0xdb},
        {.request_type=0xA1, .request=0x83, .value=0x202, .index=0x900, .data0=0x00, .data1=0x00},
        {.request_type=0xA1, .request=0x84, .value=0x202, .index=0x900, .data0=0x00, .data1=0x01},
        {.request_type=0xA1, .request=0x81, .value=0x201, .index=0x900, .data0=0x00, .data1=0xf6},
        {.request_type=0xA1, .request=0x81, .value=0x100, .index=0x900, .data0=0x00, .data1=std::nullopt},
        {.request_type=0xA1, .request=0x82, .value=0x200, .index=0xA00, .data0=0x00, .data1=0xf4},
        {.request_type=0xA1, .request=0x83, .value=0x200, .index=0xA00, .data0=0x00, .data1=0x17},
        {.request_type=0xA1, .request=0x84, .value=0x200, .index=0xA00, .data0=0x00, .data1=0x01},
        {.request_type=0xA1, .request=0x81, .value=0x200, .index=0xA00, .data0=0x00, .data1=0x08},
        {.request_type=0xA1, .request=0x81, .value=0x100, .index=0xA00, .data0=0x00, .data1=std::nullopt},
        {.request_type=0xA1, .request=0x81, .value=0x700, .index=0xA00, .data0=0x01, .data1=std::nullopt},
        {.request_type=0xA1, .request=0x82, .value=0x200, .index=0xD00, .data0=0x00, .data1=0xe9},
        {.request_type=0xA1, .request=0x83, .value=0x200, .index=0xD00, .data0=0x00, .data1=0x08},
        {.request_type=0xA1, .request=0x84, .value=0x200, .index=0xD00, .data0=0x00, .data1=0x01},
        {.request_type=0xA1, .request=0x81, .value=0x200, .index=0xD00, .data0=0x00, .data1=0xf9},
        {.request_type=0xA1, .request=0x81, .value=0x100, .index=0xD00, .data0=0x01, .data1=std::nullopt},
        // clang-format on
    };
    for (auto& canned_reply : canned_replies) {
      if (request_type == canned_reply.request_type && request == canned_reply.request &&
          value == canned_reply.value && index == canned_reply.index) {
        uint8_t* p = out_read_buffer;
        *p++ = canned_reply.data0;
        if (canned_reply.data1.has_value()) {
          *p++ = canned_reply.data1.value();
          *out_read_actual = 2;
        } else {
          *out_read_actual = 1;
        }
        return ZX_OK;
      }
    }
    return ZX_ERR_INTERNAL;
  }

  virtual void UsbRequestQueue(usb_request_t* usb_request,
                               const usb_request_complete_callback_t* complete_cb) {}

  usb_speed_t UsbGetSpeed() { return USB_SPEED_FULL; }

  zx_status_t UsbSetInterface(uint8_t interface_number, uint8_t alt_setting) { return ZX_OK; }
  uint8_t UsbGetConfiguration() { return 0; }
  zx_status_t UsbSetConfiguration(uint8_t configuration) { return ZX_ERR_NOT_SUPPORTED; }
  zx_status_t UsbEnableEndpoint(const usb_endpoint_descriptor_t* ep_desc,
                                const usb_ss_ep_comp_descriptor_t* ss_com_desc, bool enable) {
    return ZX_ERR_NOT_SUPPORTED;
  }
  zx_status_t UsbResetEndpoint(uint8_t ep_address) { return ZX_ERR_NOT_SUPPORTED; }

  zx_status_t UsbResetDevice() { return ZX_ERR_NOT_SUPPORTED; }
  size_t UsbGetMaxTransferSize(uint8_t ep_address) { return 0; }
  uint32_t UsbGetDeviceId() { return 0; }
  void UsbGetDeviceDescriptor(usb_device_descriptor_t* out_desc) {
    constexpr uint8_t descriptor[] = {
        0x12, 0x01, 0x00, 0x02, 0xe0, 0x01, 0x01, 0x40, 0x87,
        0x80, 0xaa, 0x0a, 0x02, 0x00, 0x00, 0x00, 0x00, 0x01,
    };
    memcpy(out_desc, descriptor, sizeof(descriptor));
  }
  zx_status_t UsbGetConfigurationDescriptorLength(uint8_t configuration, size_t* out_length) {
    return ZX_ERR_NOT_SUPPORTED;
  }
  zx_status_t UsbGetConfigurationDescriptor(uint8_t configuration, uint8_t* out_desc_buffer,
                                            size_t desc_size, size_t* out_desc_actual) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  size_t UsbGetDescriptorsLength() { return sizeof(usb_descriptor_); }
  virtual void UsbGetDescriptors(uint8_t* out_descs_buffer, size_t descs_size,
                                 size_t* out_descs_actual) {
    memcpy(out_descs_buffer, usb_descriptor_, sizeof(usb_descriptor_));
    *out_descs_actual = sizeof(usb_descriptor_);
  }
  zx_status_t UsbGetStringDescriptor(uint8_t desc_id, uint16_t lang_id, uint16_t* out_lang_id,
                                     uint8_t* out_string_buffer, size_t string_size,
                                     size_t* out_string_actual) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  zx_status_t UsbCancelAll(uint8_t ep_address) { return ZX_ERR_NOT_SUPPORTED; }
  uint64_t UsbGetCurrentFrame() { return 0; }
  size_t UsbGetRequestSize() {
    return usb::BorrowedRequest<void>::RequestSize(sizeof(usb_request_t));
  }

  // USB composite protocol implementation.
  size_t UsbCompositeGetAdditionalDescriptorLength() { return 0; }
  zx_status_t UsbCompositeGetAdditionalDescriptorList(uint8_t* out_desc_list, size_t desc_count,
                                                      size_t* out_desc_actual) {
    *out_desc_actual = 0;
    return ZX_OK;
  }
  zx_status_t UsbCompositeClaimInterface(const usb_interface_descriptor_t* desc, uint32_t length) {
    return ZX_ERR_NOT_SUPPORTED;
  }

 private:
  static constexpr uint8_t usb_descriptor_[] = {
      0x09, 0x04, 0x00, 0x00, 0x00, 0x01, 0x01, 0x00, 0x00, 0x0a, 0x24, 0x01, 0x00, 0x01, 0x64,
      0x00, 0x02, 0x01, 0x02, 0x0c, 0x24, 0x02, 0x01, 0x01, 0x01, 0x00, 0x02, 0x03, 0x00, 0x00,
      0x00, 0x0c, 0x24, 0x02, 0x02, 0x01, 0x02, 0x00, 0x01, 0x01, 0x00, 0x00, 0x00, 0x09, 0x24,
      0x03, 0x06, 0x01, 0x03, 0x00, 0x09, 0x00, 0x09, 0x24, 0x03, 0x07, 0x01, 0x01, 0x00, 0x08,
      0x00, 0x07, 0x24, 0x05, 0x08, 0x01, 0x0a, 0x00, 0x0a, 0x24, 0x06, 0x09, 0x0f, 0x01, 0x01,
      0x02, 0x02, 0x00, 0x09, 0x24, 0x06, 0x0a, 0x02, 0x01, 0x43, 0x00, 0x00, 0x09, 0x24, 0x06,
      0x0d, 0x02, 0x01, 0x03, 0x00, 0x00, 0x0d, 0x24, 0x04, 0x0f, 0x02, 0x01, 0x0d, 0x02, 0x03,
      0x00, 0x00, 0x00, 0x00, 0x09, 0x04, 0x01, 0x00, 0x00, 0x01, 0x02, 0x00, 0x00, 0x09, 0x04,
      0x01, 0x01, 0x01, 0x01, 0x02, 0x00, 0x00, 0x07, 0x24, 0x01, 0x01, 0x01, 0x01, 0x00, 0x0e,
      0x24, 0x02, 0x01, 0x02, 0x02, 0x10, 0x02, 0x80, 0xbb, 0x00, 0x44, 0xac, 0x00, 0x09, 0x05,
      0x01, 0x09, 0xc8, 0x00, 0x01, 0x00, 0x00, 0x07, 0x25, 0x01, 0x01, 0x01, 0x01, 0x00, 0x09,
      0x04, 0x02, 0x00, 0x00, 0x01, 0x02, 0x00, 0x00, 0x09, 0x04, 0x02, 0x01, 0x01, 0x01, 0x02,
      0x00, 0x00, 0x07, 0x24, 0x01, 0x07, 0x01, 0x01, 0x00, 0x0e, 0x24, 0x02, 0x01, 0x01, 0x02,
      0x10, 0x02, 0x80, 0xbb, 0x00, 0x44, 0xac, 0x00, 0x09, 0x05, 0x82, 0x0d, 0x64, 0x00, 0x01,
      0x00, 0x00, 0x07, 0x25, 0x01, 0x01, 0x00, 0x00, 0x00,
  };
};
}  // namespace

namespace audio::usb {

template <class FakeDevType>
struct IncomingNamespace {
  std::shared_ptr<FakeDevType> fake_dev;
  component::OutgoingDirectory outgoing{async_get_default_dispatcher()};
};

// The class here is templated on the type of device which the UsbAudioDevice should be a child of.
template <class FakeDevType>
class BaseUsbAudioTest : public inspect::InspectTestHelper, public zxtest::Test {
 public:
  void SetUp() override {
    ASSERT_OK(loop_.StartThread("usb-audio-test-thread"));

    root_ = MockDevice::FakeRootParent();

    auto endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
    incoming_.SyncCall([server = std::move(endpoints.server),
                        this](IncomingNamespace<FakeDevType>* infra) mutable {
      infra->fake_dev = std::make_shared<FakeDevType>(root_.get());
      ASSERT_OK(infra->fake_dev->Bind());

      ASSERT_OK(infra->outgoing.template AddService<fuchsia_hardware_usb::UsbService>(
          fuchsia_hardware_usb::UsbService::InstanceHandler({
              .device = infra->fake_dev->bind_handler(async_get_default_dispatcher()),
          })));

      ASSERT_OK(infra->outgoing.Serve(std::move(server)));
    });
    ASSERT_EQ(root_->child_count(), 1);
    auto* mock_dev = root_->GetLatestChild();
    mock_dev->AddFidlService(fuchsia_hardware_usb::UsbService::Name, std::move(endpoints.client));

    incoming_.SyncCall([&](IncomingNamespace<FakeDevType>* infra) {
      mock_dev->AddProtocol(ZX_PROTOCOL_USB, infra->fake_dev->proto().ops,
                            infra->fake_dev->proto().ctx);
      mock_dev->AddProtocol(ZX_PROTOCOL_USB_COMPOSITE, infra->fake_dev->proto_composite().ops,
                            infra->fake_dev->proto_composite().ctx);
    });
  }

  void TearDown() override {
    incoming_.SyncCall(
        [](IncomingNamespace<FakeDevType>* infra) { infra->fake_dev->DdkAsyncRemove(); });
    mock_ddk::ReleaseFlaggedDevices(root_.get());
  }

 protected:
  std::shared_ptr<MockDevice> root_;
  async::Loop loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
  async_patterns::TestDispatcherBound<IncomingNamespace<FakeDevType>> incoming_{loop_.dispatcher(),
                                                                                std::in_place};
};

using UsbAudioTest = BaseUsbAudioTest<FakeDevice>;

TEST_F(UsbAudioTest, Inspect) {
  incoming_.SyncCall([&](IncomingNamespace<FakeDevice>* infra) {
    zx::result<UsbAudioDevice*> ret = UsbAudioDevice::DriverBind(infra->fake_dev->zxdev());
    ASSERT_TRUE(ret.is_ok());
    ASSERT_NO_FATAL_FAILURE(ReadInspect(ret.value()->streams().front().inspect().DuplicateVmo()));
  });

  auto* inspect = hierarchy().GetByPath({"usb_audio_stream"});
  ASSERT_TRUE(inspect);
  ASSERT_NO_FATAL_FAILURE(
      CheckProperty(inspect->node(), "state", inspect::StringPropertyValue("created")));
  ASSERT_NO_FATAL_FAILURE(
      CheckProperty(inspect->node(), "start_time", inspect::IntPropertyValue(0)));

  ASSERT_NO_FATAL_FAILURE(
      CheckProperty(inspect->node(), "supported_min_number_of_channels",
                    inspect::UintArrayValue({2, 2}, inspect::ArrayDisplayFormat::kFlat)));
  ASSERT_NO_FATAL_FAILURE(
      CheckProperty(inspect->node(), "supported_max_number_of_channels",
                    inspect::UintArrayValue({2, 2}, inspect::ArrayDisplayFormat::kFlat)));
  ASSERT_NO_FATAL_FAILURE(
      CheckProperty(inspect->node(), "supported_min_frame_rates",
                    inspect::UintArrayValue({48'000, 44'100}, inspect::ArrayDisplayFormat::kFlat)));
  ASSERT_NO_FATAL_FAILURE(
      CheckProperty(inspect->node(), "supported_max_frame_rates",
                    inspect::UintArrayValue({48'000, 44'100}, inspect::ArrayDisplayFormat::kFlat)));
  ASSERT_NO_FATAL_FAILURE(
      CheckProperty(inspect->node(), "supported_bits_per_slot",
                    inspect::UintArrayValue({16, 16}, inspect::ArrayDisplayFormat::kFlat)));
  ASSERT_NO_FATAL_FAILURE(
      CheckProperty(inspect->node(), "supported_bits_per_sample",
                    inspect::UintArrayValue({16, 16}, inspect::ArrayDisplayFormat::kFlat)));
  ASSERT_NO_FATAL_FAILURE(CheckProperty(
      inspect->node(), "supported_sample_formats",
      inspect::StringArrayValue({"PCM_signed", "PCM_signed"}, inspect::ArrayDisplayFormat::kFlat)));
}

TEST_F(UsbAudioTest, GetStreamProperties) {
  incoming_.SyncCall([](IncomingNamespace<FakeDevice>* infra) {
    ASSERT_OK(UsbAudioDevice::DriverBind(infra->fake_dev->zxdev()));
  });
  ASSERT_EQ(root_->GetLatestChild()->child_count(), 1);

  ASSERT_EQ(root_->GetLatestChild()->GetLatestChild()->child_count(), 2);
  auto itr = root_->GetLatestChild()->GetLatestChild()->children().begin();
  std::advance(itr, 0);
  UsbAudioStream* dut = (*itr)->GetDeviceContext<UsbAudioStream>();

  auto endpoints = fidl::Endpoints<audio_fidl::StreamConfigConnector>::Create();
  fidl::BindServer(loop_.dispatcher(), std::move(endpoints.server), dut);
  auto stream_client = GetStreamClient(std::move(endpoints.client));

  auto result = stream_client->GetProperties();
  ASSERT_OK(result.status());

  ASSERT_EQ(result.value().properties.clock_domain(), audio_fidl::wire::kClockDomainMonotonic);
  ASSERT_EQ(result.value().properties.min_gain_db(), -37.);
  ASSERT_EQ(result.value().properties.max_gain_db(), 0.);
  ASSERT_EQ(result.value().properties.gain_step_db(), 1.);
  ASSERT_EQ(result.value().properties.can_mute(), true);
  ASSERT_EQ(result.value().properties.can_agc(), false);
  ASSERT_EQ(result.value().properties.plug_detect_capabilities(),
            audio_fidl::wire::PlugDetectCapabilities::kHardwired);
}

TEST_F(UsbAudioTest, MultipleStreamConfigClients) {
  incoming_.SyncCall([](IncomingNamespace<FakeDevice>* infra) {
    ASSERT_OK(UsbAudioDevice::DriverBind(infra->fake_dev->zxdev()));
  });
  ASSERT_EQ(root_->GetLatestChild()->child_count(), 1);

  ASSERT_EQ(root_->GetLatestChild()->GetLatestChild()->child_count(), 2);
  auto itr = root_->GetLatestChild()->GetLatestChild()->children().begin();
  std::advance(itr, 0);
  UsbAudioStream* dut = (*itr)->GetDeviceContext<UsbAudioStream>();

  auto endpoints = fidl::Endpoints<audio_fidl::StreamConfigConnector>::Create();
  fidl::BindServer(loop_.dispatcher(), std::move(endpoints.server), dut);
  fidl::WireSyncClient client_wrap{
      fidl::ClientEnd<audio_fidl::StreamConfigConnector>{std::move(endpoints.client)}};
  {
    auto [stream_channel_local, stream_channel_remote] =
        fidl::Endpoints<audio_fidl::StreamConfig>::Create();
    ASSERT_OK(client_wrap->Connect(std::move(stream_channel_remote)));
    // To make sure the 1-way Connect call is completed in the StreamConfigConnector server,
    // make a 2-way call. Since StreamConfigConnector does not have a 2-way call, we use
    // StreamConfig synchronously.
    auto stream_client =
        fidl::WireSyncClient<audio_fidl::StreamConfig>(std::move(stream_channel_local));
    ASSERT_TRUE(stream_client.is_valid());
    auto result = stream_client->GetProperties();
    ASSERT_OK(result.status());
  }
  {
    auto [stream_channel_local, stream_channel_remote] =
        fidl::Endpoints<audio_fidl::StreamConfig>::Create();
    ASSERT_OK(client_wrap->Connect(std::move(stream_channel_remote)));
    // To make sure the 1-way Connect call is completed in the StreamConfigConnector server,
    // make a 2-way call. Since StreamConfigConnector does not have a 2-way call, we use
    // StreamConfig synchronously.
    auto stream_client =
        fidl::WireSyncClient<audio_fidl::StreamConfig>(std::move(stream_channel_local));
    ASSERT_TRUE(stream_client.is_valid());
    auto result = stream_client->GetProperties();
    ASSERT_OK(result.status());
  }
}

TEST_F(UsbAudioTest, SetAndGetGain) {
  incoming_.SyncCall([](IncomingNamespace<FakeDevice>* infra) {
    ASSERT_OK(UsbAudioDevice::DriverBind(infra->fake_dev->zxdev()));
  });
  ASSERT_EQ(root_->GetLatestChild()->child_count(), 1);

  ASSERT_EQ(root_->GetLatestChild()->GetLatestChild()->child_count(), 2);
  auto itr = root_->GetLatestChild()->GetLatestChild()->children().begin();
  std::advance(itr, 0);
  UsbAudioStream* dut = (*itr)->GetDeviceContext<UsbAudioStream>();

  auto endpoints = fidl::Endpoints<audio_fidl::StreamConfigConnector>::Create();
  fidl::BindServer(loop_.dispatcher(), std::move(endpoints.server), dut);
  auto stream_client = GetStreamClient(std::move(endpoints.client));
  ASSERT_TRUE(stream_client.is_valid());

  constexpr float kTestGain = -12.f;
  {
    fidl::Arena allocator;
    audio_fidl::wire::GainState gain_state(allocator);
    gain_state.set_gain_db(kTestGain);
    auto status = stream_client->SetGain(std::move(gain_state));
    ASSERT_OK(status.status());
  }

  auto gain_state = stream_client->WatchGainState();
  ASSERT_OK(gain_state.status());

  ASSERT_EQ(kTestGain, gain_state.value().gain_state.gain_db());
}

TEST_F(UsbAudioTest, Enumerate) {
  incoming_.SyncCall([](IncomingNamespace<FakeDevice>* infra) {
    ASSERT_OK(UsbAudioDevice::DriverBind(infra->fake_dev->zxdev()));
  });
  ASSERT_EQ(root_->GetLatestChild()->child_count(), 1);

  ASSERT_EQ(root_->GetLatestChild()->GetLatestChild()->child_count(), 2);
  auto itr = root_->GetLatestChild()->GetLatestChild()->children().begin();
  std::advance(itr, 0);
  UsbAudioStream* dut = (*itr)->GetDeviceContext<UsbAudioStream>();

  auto endpoints = fidl::Endpoints<audio_fidl::StreamConfigConnector>::Create();
  fidl::BindServer(loop_.dispatcher(), std::move(endpoints.server), dut);
  auto stream_client = GetStreamClient(std::move(endpoints.client));
  ASSERT_TRUE(stream_client.is_valid());

  auto ret = stream_client->GetSupportedFormats();
  auto& supported_formats = ret.value().supported_formats;
  ASSERT_EQ(2, supported_formats.size());

  auto& formats1 = supported_formats[0].pcm_supported_formats();
  ASSERT_EQ(1, formats1.channel_sets().size());
  ASSERT_EQ(2, formats1.channel_sets()[0].attributes().size());
  ASSERT_EQ(1, formats1.sample_formats().size());
  ASSERT_EQ(audio_fidl::wire::SampleFormat::kPcmSigned, formats1.sample_formats()[0]);
  ASSERT_EQ(1, formats1.frame_rates().size());
  ASSERT_EQ(48'000, formats1.frame_rates()[0]);
  ASSERT_EQ(1, formats1.bytes_per_sample().size());
  ASSERT_EQ(2, formats1.bytes_per_sample()[0]);
  ASSERT_EQ(1, formats1.valid_bits_per_sample().size());
  ASSERT_EQ(16, formats1.valid_bits_per_sample()[0]);

  auto& formats2 = supported_formats[1].pcm_supported_formats();
  ASSERT_EQ(1, formats2.channel_sets().size());
  ASSERT_EQ(2, formats2.channel_sets()[0].attributes().size());
  ASSERT_EQ(1, formats2.sample_formats().size());
  ASSERT_EQ(audio_fidl::wire::SampleFormat::kPcmSigned, formats2.sample_formats()[0]);
  ASSERT_EQ(1, formats2.frame_rates().size());
  ASSERT_EQ(44'100, formats2.frame_rates()[0]);
  ASSERT_EQ(1, formats2.bytes_per_sample().size());
  ASSERT_EQ(2, formats2.bytes_per_sample()[0]);
  ASSERT_EQ(1, formats2.valid_bits_per_sample().size());
  ASSERT_EQ(16, formats2.valid_bits_per_sample()[0]);
}

class FakeDeviceContinuousFrameRatesRange : public FakeDevice {
 public:
  FakeDeviceContinuousFrameRatesRange(zx_device_t* parent) : FakeDevice(parent) {}
  void UsbGetDescriptors(uint8_t* out_descs_buffer, size_t descs_size,
                         size_t* out_descs_actual) override {
    memcpy(out_descs_buffer, usb_descriptor2_, sizeof(usb_descriptor2_));
    *out_descs_actual = sizeof(usb_descriptor2_);
  }

 private:
  static constexpr uint8_t usb_descriptor2_[] = {
      // clang-format off
      0x09, 0x04, 0x00, 0x00, 0x00, 0x01, 0x01, 0x00, 0x00, 0x0a, 0x24, 0x01, 0x00, 0x01, 0x64,
      0x00, 0x02, 0x01, 0x02, 0x0c, 0x24, 0x02, 0x01, 0x01, 0x01, 0x00, 0x02, 0x03, 0x00, 0x00,
      0x00, 0x0c, 0x24, 0x02, 0x02, 0x01, 0x02, 0x00, 0x01, 0x01, 0x00, 0x00, 0x00, 0x09, 0x24,
      0x03, 0x06, 0x01, 0x03, 0x00, 0x09, 0x00, 0x09, 0x24, 0x03, 0x07, 0x01, 0x01, 0x00, 0x08,
      0x00, 0x07, 0x24, 0x05, 0x08, 0x01, 0x0a, 0x00, 0x0a, 0x24, 0x06, 0x09, 0x0f, 0x01, 0x01,
      0x02, 0x02, 0x00, 0x09, 0x24, 0x06, 0x0a, 0x02, 0x01, 0x43, 0x00, 0x00, 0x09, 0x24, 0x06,
      0x0d, 0x02, 0x01, 0x03, 0x00, 0x00, 0x0d, 0x24, 0x04, 0x0f, 0x02, 0x01, 0x0d, 0x02, 0x03,
      0x00, 0x00, 0x00, 0x00, 0x09, 0x04, 0x01, 0x00, 0x00, 0x01, 0x02, 0x00, 0x00, 0x09, 0x04,
      0x01, 0x01, 0x01, 0x01, 0x02, 0x00, 0x00, 0x07, 0x24, 0x01, 0x01, 0x01, 0x01, 0x00,
      // Type I Format Type Descriptor.
      0x0e,              // Length.
      0x24,              // Type CS_INTERFACE.
      0x02,              // Subtype FORMAT_TYPE.
      0x01,              // FormatType FORMAT_TYPE_I.
      0x02,              // 2 channels.
      0x02,              // SubFrameSize.
      0x10,              // 16 bits resolution.
      0x00,              // bSamFreqType = Continuous sampling frequency.
      0x40, 0x1F, 0x00,  // 8kHz.
      0x80, 0xbb, 0x00,  // 48kHz, this range specifies the valid 8, 16, 32 and 48kHz.
      // End of Type I Format Type Descriptor.
      0x09, 0x05, 0x01, 0x09, 0xc8, 0x00, 0x01, 0x00, 0x00, 0x07, 0x25, 0x01, 0x01, 0x01, 0x01,
      0x00, 0x09, 0x04, 0x02, 0x00, 0x00, 0x01, 0x02, 0x00, 0x00, 0x09, 0x04, 0x02, 0x01, 0x01,
      0x01, 0x02, 0x00, 0x00, 0x07, 0x24, 0x01, 0x07, 0x01, 0x01, 0x00,
      0x0e,              // Length.
      0x24,              // Type CS_INTERFACE.
      0x02,              // Subtype FORMAT_TYPE.
      0x01,              // FormatType FORMAT_TYPE_I.
      0x01,              // 1 channel.
      0x02,              // SubFrameSize.
      0x10,              // 16 bits resolution.
      0x00,              // bSamFreqType = Continuous sampling frequency.
      0x44, 0xac, 0x00,  // 44.1kHz.
      0x88, 0x58, 0x01,  // 88.2kHz, this range specifies the valid 44.1 and 88.2kHz.
      // End of Type I Format Type Descriptor.
      0x09, 0x05, 0x82, 0x0d, 0x64, 0x00, 0x01, 0x00, 0x00, 0x07, 0x25, 0x01, 0x01, 0x00, 0x00,
      0x00,
      // clang-format on
  };
};

using UsbAudioContinuousFrameRatesTest = BaseUsbAudioTest<FakeDeviceContinuousFrameRatesRange>;

TEST_F(UsbAudioContinuousFrameRatesTest,
       EnumerateWithDescriptorIncludingContinuousFrameRatesRange) {
  incoming_.SyncCall([](IncomingNamespace<FakeDeviceContinuousFrameRatesRange>* infra) {
    zx::result<UsbAudioDevice*> ret = UsbAudioDevice::DriverBind(infra->fake_dev->zxdev());
    ASSERT_TRUE(ret.is_ok());
  });
  ASSERT_EQ(root_->GetLatestChild()->child_count(), 1);

  ASSERT_EQ(root_->GetLatestChild()->GetLatestChild()->child_count(), 2);
  auto itr = root_->GetLatestChild()->GetLatestChild()->children().begin();
  std::advance(itr, 0);
  UsbAudioStream* dut = (*itr)->GetDeviceContext<UsbAudioStream>();

  auto endpoints = fidl::Endpoints<audio_fidl::StreamConfigConnector>::Create();
  fidl::BindServer(loop_.dispatcher(), std::move(endpoints.server), dut);
  auto stream_client = GetStreamClient(std::move(endpoints.client));
  ASSERT_TRUE(stream_client.is_valid());

  // We get the formats from the first interface in the descriptor.
  auto ret2 = stream_client->GetSupportedFormats();
  ASSERT_TRUE(ret2.ok());
  auto& supported_formats = ret2.value().supported_formats;
  ASSERT_EQ(4, supported_formats.size());

  ASSERT_EQ(1, supported_formats[0].pcm_supported_formats().frame_rates().size());
  ASSERT_EQ(1, supported_formats[1].pcm_supported_formats().frame_rates().size());
  ASSERT_EQ(1, supported_formats[2].pcm_supported_formats().frame_rates().size());
  ASSERT_EQ(1, supported_formats[3].pcm_supported_formats().frame_rates().size());

  ASSERT_EQ(8'000, supported_formats[0].pcm_supported_formats().frame_rates()[0]);
  ASSERT_EQ(16'000, supported_formats[1].pcm_supported_formats().frame_rates()[0]);
  ASSERT_EQ(32'000, supported_formats[2].pcm_supported_formats().frame_rates()[0]);
  ASSERT_EQ(48'000, supported_formats[3].pcm_supported_formats().frame_rates()[0]);
}

class FakeDeviceBadContinuousFrameRatesRange : public FakeDevice {
 public:
  FakeDeviceBadContinuousFrameRatesRange(zx_device_t* parent) : FakeDevice(parent) {}
  void UsbGetDescriptors(uint8_t* out_descs_buffer, size_t descs_size,
                         size_t* out_descs_actual) override {
    memcpy(out_descs_buffer, usb_descriptor2_, sizeof(usb_descriptor2_));
    *out_descs_actual = sizeof(usb_descriptor2_);
  }

 private:
  static inline constexpr uint8_t usb_descriptor2_[] = {
      // clang-format off
      0x09, 0x04, 0x00, 0x00, 0x00, 0x01, 0x01, 0x00, 0x00, 0x0a, 0x24, 0x01, 0x00, 0x01, 0x64,
      0x00, 0x02, 0x01, 0x02, 0x0c, 0x24, 0x02, 0x01, 0x01, 0x01, 0x00, 0x02, 0x03, 0x00, 0x00,
      0x00, 0x0c, 0x24, 0x02, 0x02, 0x01, 0x02, 0x00, 0x01, 0x01, 0x00, 0x00, 0x00, 0x09, 0x24,
      0x03, 0x06, 0x01, 0x03, 0x00, 0x09, 0x00, 0x09, 0x24, 0x03, 0x07, 0x01, 0x01, 0x00, 0x08,
      0x00, 0x07, 0x24, 0x05, 0x08, 0x01, 0x0a, 0x00, 0x0a, 0x24, 0x06, 0x09, 0x0f, 0x01, 0x01,
      0x02, 0x02, 0x00, 0x09, 0x24, 0x06, 0x0a, 0x02, 0x01, 0x43, 0x00, 0x00, 0x09, 0x24, 0x06,
      0x0d, 0x02, 0x01, 0x03, 0x00, 0x00, 0x0d, 0x24, 0x04, 0x0f, 0x02, 0x01, 0x0d, 0x02, 0x03,
      0x00, 0x00, 0x00, 0x00, 0x09, 0x04, 0x01, 0x00, 0x00, 0x01, 0x02, 0x00, 0x00, 0x09, 0x04,
      0x01, 0x01, 0x01, 0x01, 0x02, 0x00, 0x00, 0x07, 0x24, 0x01, 0x01, 0x01, 0x01, 0x00,
      // Type I Format Type Descriptor.
      0x0e,              // Length.
      0x24,              // Type CS_INTERFACE.
      0x02,              // Subtype FORMAT_TYPE.
      0x01,              // FormatType FORMAT_TYPE_I.
      0x02,              // 2 channels.
      0x02,              // SubFrameSize.
      0x10,              // 16 bits resolution.
      0x00,              // bSamFreqType = Continuous sampling frequency.
      0x80, 0xbb, 0x00,  // 48kHz.
      0x40, 0x1F, 0x00,  // 8kHz this is incorrect, the max frequency in the range is lower.
      // End of Type I Format Type Descriptor.
      0x09, 0x05, 0x01, 0x09, 0xc8, 0x00, 0x01, 0x00, 0x00, 0x07, 0x25, 0x01, 0x01, 0x01, 0x01,
      0x00, 0x09, 0x04, 0x02, 0x00, 0x00, 0x01, 0x02, 0x00, 0x00, 0x09, 0x04, 0x02, 0x01, 0x01,
      0x01, 0x02, 0x00, 0x00, 0x07, 0x24, 0x01, 0x07, 0x01, 0x01, 0x00,
      0x0e,              // Length.
      0x24,              // Type CS_INTERFACE.
      0x02,              // Subtype FORMAT_TYPE.
      0x01,              // FormatType FORMAT_TYPE_I.
      0x01,              // 1 channel.
      0x02,              // SubFrameSize.
      0x10,              // 16 bits resolution.
      0x00,              // bSamFreqType = Continuous sampling frequency.
      0xd2, 0x04, 0x00,  // 1234Hz.
      0xd3, 0x04, 0x00,  // 1235Hz incorrect continuous range, can't generate a family rate.
      // End of Type I Format Type Descriptor.
      0x09, 0x05, 0x82, 0x0d, 0x64, 0x00, 0x01, 0x00, 0x00, 0x07, 0x25, 0x01, 0x01, 0x00, 0x00,
      0x00,
      // clang-format on
  };
};

using UsbAudioBadContinuousFrameRatesTest =
    BaseUsbAudioTest<FakeDeviceBadContinuousFrameRatesRange>;

TEST_F(UsbAudioBadContinuousFrameRatesTest, EnumerateBadContinuousFrameRatesRange) {
  incoming_.SyncCall([](IncomingNamespace<FakeDeviceBadContinuousFrameRatesRange>* infra) {
    zx::result<UsbAudioDevice*> ret = UsbAudioDevice::DriverBind(infra->fake_dev->zxdev());
    ASSERT_TRUE(ret.is_ok());
  });
  ASSERT_EQ(root_->GetLatestChild()->child_count(), 1);
  // Both interfaces in the descriptor failed to produce valid formats.
  ASSERT_EQ(root_->GetLatestChild()->GetLatestChild()->child_count(), 0);
}

class FakeMidiDevice : public FakeDevice {
 public:
  explicit FakeMidiDevice(zx_device_t* parent) : FakeDevice(parent) {}
  void UsbGetDescriptors(uint8_t* out_descs_buffer, size_t descs_size,
                         size_t* out_descs_actual) override {
    memcpy(out_descs_buffer, usb_descriptor_midi_, sizeof(usb_descriptor_midi_));
    *out_descs_actual = sizeof(usb_descriptor_midi_);
  }
  void UsbRequestQueue(usb_request_t* usb_request,
                       const usb_request_complete_callback_t* complete_cb) override {
    std::scoped_lock lock(mutex_);
    queued_reqs_.push_back({usb_request, *complete_cb});
  }

  struct QueuedReq {
    usb_request_t* req;
    usb_request_complete_callback_t cb;
  };

  std::optional<QueuedReq> PopQueuedReq() {
    std::scoped_lock lock(mutex_);
    if (queued_reqs_.empty()) {
      return std::nullopt;
    }
    QueuedReq req = queued_reqs_.back();
    queued_reqs_.pop_back();
    return req;
  }

  ~FakeMidiDevice() override {
    std::scoped_lock lock(mutex_);
    for (auto& qreq : queued_reqs_) {
      usb_request_release(qreq.req);
    }
    queued_reqs_.clear();
  }

 private:
  std::mutex mutex_;
  std::vector<QueuedReq> queued_reqs_;
  static constexpr uint8_t usb_descriptor_midi_[] = {
      // clang-format off
      // Interface 0: MIDI Streaming
      0x09, 0x04, 0x00, 0x00, 0x01, 0x01, 0x03, 0x00, 0x00,
      // Bulk IN Endpoint (0x83)
      0x07, 0x05, 0x83, 0x02, 0x02, 0x00, 0x00,
      // clang-format on
  };
};

using UsbMidiTest = BaseUsbAudioTest<FakeMidiDevice>;

TEST_F(UsbMidiTest, ReadClampsToCopied) {
  incoming_.SyncCall([](IncomingNamespace<FakeMidiDevice>* infra) {
    ASSERT_OK(UsbAudioDevice::DriverBind(infra->fake_dev->zxdev()));
  });
  ASSERT_EQ(root_->GetLatestChild()->child_count(), 1);
  ASSERT_EQ(root_->GetLatestChild()->GetLatestChild()->child_count(), 1);
  auto itr = root_->GetLatestChild()->GetLatestChild()->children().begin();
  UsbMidiSource* dut = (*itr)->GetDeviceContext<UsbMidiSource>();
  ASSERT_NE(dut, nullptr);

  std::shared_ptr<libsync::Completion> dispatcher_shutdown =
      std::make_shared<libsync::Completion>();
  zx::result fdf_dispatcher = fdf::SynchronizedDispatcher::Create(
      fdf::SynchronizedDispatcher::Options::kAllowSyncCalls, "midi-test",
      [dispatcher_shutdown](fdf_dispatcher_t* dispatcher) { dispatcher_shutdown->Signal(); });
  ASSERT_OK(fdf_dispatcher.status_value());

  auto [ctrl_local, ctrl_remote] = fidl::Endpoints<fuchsia_hardware_midi::Controller>::Create();
  fidl::BindServer(fdf_dispatcher->async_dispatcher(), std::move(ctrl_remote), dut);
  auto [local, remote] = fidl::Endpoints<fuchsia_hardware_midi::Device>::Create();
  auto open_res = fidl::WireCall(ctrl_local)->OpenSession(std::move(remote));
  ASSERT_OK(open_res.status());

  std::atomic<bool> done = {};
  auto& client = local;
  auto th = std::thread([&client, &done] {
    auto read_res = fidl::WireCall(client)->Read();
    ASSERT_OK(read_res.status());
    ASSERT_EQ(read_res->value()->event.size(), 1);
    ASSERT_EQ(read_res->value()->event[0], 0x90);
    done.store(true);
  });

  while (!done.load()) {
    incoming_.SyncCall([](IncomingNamespace<FakeMidiDevice>* infra) {
      if (auto qreq = infra->fake_dev->PopQueuedReq()) {
        UnownedRequest req(qreq->req, qreq->cb, sizeof(usb_request_t));
        uint8_t data[] = {0x00, 0x90};
        (void)req.CopyTo(data, sizeof(data), 0);
        req.Complete(ZX_OK, 2);
      }
    });
    zx::nanosleep(zx::deadline_after(zx::msec(10)));
  }
  th.join();
  fdf_dispatcher->ShutdownAsync();
  dispatcher_shutdown->Wait();
}

TEST_F(UsbAudioTest, CreateRingBuffer) {
  incoming_.SyncCall([](IncomingNamespace<FakeDevice>* infra) {
    ASSERT_OK(UsbAudioDevice::DriverBind(infra->fake_dev->zxdev()));
  });
  ASSERT_EQ(root_->GetLatestChild()->child_count(), 1);

  ASSERT_EQ(root_->GetLatestChild()->GetLatestChild()->child_count(), 2);
  auto itr = root_->GetLatestChild()->GetLatestChild()->children().begin();
  std::advance(itr, 0);
  UsbAudioStream* dut = (*itr)->GetDeviceContext<UsbAudioStream>();

  auto endpoints = fidl::Endpoints<audio_fidl::StreamConfigConnector>::Create();
  fidl::BindServer(loop_.dispatcher(), std::move(endpoints.server), dut);
  auto stream_client = GetStreamClient(std::move(endpoints.client));
  ASSERT_TRUE(stream_client.is_valid());

  {
    auto [local, remote] = fidl::Endpoints<audio_fidl::RingBuffer>::Create();

    fidl::Arena allocator;
    audio_fidl::wire::Format format(allocator);
    format.set_pcm_format(allocator, GetDefaultPcmFormat());
    auto result1 = stream_client->CreateRingBuffer(format, std::move(remote));
    ASSERT_OK(result1.status());

    // To make sure the 1-way Connect call is completed in the StreamConfigConnector server,
    // make a 2-way call. Since StreamConfigConnector does not have a 2-way call, we use
    // StreamConfig synchronously.
    auto result2 = stream_client->GetProperties();
    ASSERT_OK(result2.status());
  }
}

TEST_F(UsbAudioTest, DelayInfo) {
  incoming_.SyncCall([](IncomingNamespace<FakeDevice>* infra) {
    ASSERT_OK(UsbAudioDevice::DriverBind(infra->fake_dev->zxdev()));
  });
  ASSERT_EQ(root_->GetLatestChild()->child_count(), 1);

  ASSERT_EQ(root_->GetLatestChild()->GetLatestChild()->child_count(), 2);
  auto itr = root_->GetLatestChild()->GetLatestChild()->children().begin();
  std::advance(itr, 0);
  UsbAudioStream* dut = (*itr)->GetDeviceContext<UsbAudioStream>();

  auto endpoints = fidl::Endpoints<audio_fidl::StreamConfigConnector>::Create();
  fidl::BindServer(loop_.dispatcher(), std::move(endpoints.server), dut);
  auto stream_client = GetStreamClient(std::move(endpoints.client));
  ASSERT_TRUE(stream_client.is_valid());

  {
    auto [local, remote] = fidl::Endpoints<audio_fidl::RingBuffer>::Create();

    fidl::Arena allocator;
    audio_fidl::wire::Format format(allocator);
    format.set_pcm_format(allocator, GetDefaultPcmFormat());
    auto result = stream_client->CreateRingBuffer(format, std::move(remote));
    ASSERT_OK(result.status());

    auto delay_info = fidl::WireCall<audio_fidl::RingBuffer>(local)->WatchDelayInfo();
    ASSERT_OK(delay_info.status());
    ASSERT_EQ(delay_info.value().delay_info.internal_delay(), 0);
    ASSERT_FALSE(delay_info.value().delay_info.has_external_delay());
  }
}

// TODO(https://fxbug.dev/42165345): Fix flakes caused by this test.
TEST_F(UsbAudioTest, DISABLED_RingBufferPropertiesAndStartOk) {
  incoming_.SyncCall([](IncomingNamespace<FakeDevice>* infra) {
    ASSERT_OK(UsbAudioDevice::DriverBind(infra->fake_dev->zxdev()));
  });
  ASSERT_EQ(root_->GetLatestChild()->child_count(), 1);

  ASSERT_EQ(root_->GetLatestChild()->GetLatestChild()->child_count(), 2);
  auto itr = root_->GetLatestChild()->GetLatestChild()->children().begin();
  std::advance(itr, 0);
  UsbAudioStream* dut = (*itr)->GetDeviceContext<UsbAudioStream>();

  auto endpoints = fidl::Endpoints<audio_fidl::StreamConfigConnector>::Create();
  fidl::BindServer(loop_.dispatcher(), std::move(endpoints.server), dut);
  auto stream_client = GetStreamClient(std::move(endpoints.client));
  ASSERT_TRUE(stream_client.is_valid());

  auto [local, remote] = fidl::Endpoints<audio_fidl::RingBuffer>::Create();

  fidl::Arena allocator;
  audio_fidl::wire::Format format(allocator);
  format.set_pcm_format(allocator, GetDefaultPcmFormat());
  auto rb = stream_client->CreateRingBuffer(format, std::move(remote));
  ASSERT_OK(rb.status());

  auto result = fidl::WireCall<audio_fidl::RingBuffer>(local)->GetProperties();
  ASSERT_OK(result.status());
  // We don't know what the reported driver_transfer_bytes (the minimum required lead time)
  // is going to be as it will depend on hardware details, but we do know that
  // it will need to be greater than 0.
  ASSERT_GT(result.value().properties.driver_transfer_bytes(), 0);
  ASSERT_EQ(result.value().properties.needs_cache_flush_or_invalidate(), true);

  constexpr uint32_t kNumberOfPositionNotifications = 5;
  constexpr uint32_t kMinFrames = 10;
  auto vmo = fidl::WireCall<audio_fidl::RingBuffer>(local)->GetVmo(kMinFrames,
                                                                   kNumberOfPositionNotifications);
  ASSERT_OK(vmo.status());

  std::atomic<bool> done = {};
  auto& ring_buffer = local;
  incoming_.SyncCall(
      [](IncomingNamespace<FakeDevice>* infra) { infra->fake_dev->ExpectConnectToEndpoint(1); });
  auto th = std::thread([&ring_buffer, &done] {
    auto start = fidl::WireCall<audio_fidl::RingBuffer>(ring_buffer)->Start();
    ASSERT_OK(start.status());
    auto stop = fidl::WireCall<audio_fidl::RingBuffer>(ring_buffer)->Stop();
    ASSERT_OK(stop.status());
    done.store(true);
  });

  // Reply until done.
  while (!done.load()) {
    incoming_.SyncCall([](IncomingNamespace<FakeDevice>* infra) {
      infra->fake_dev->fake_endpoint(1).RequestComplete(ZX_OK, 0);
    });
    // Delay a bit, so there is time for non-data handling, e.g. Stop().
    zx::nanosleep(zx::deadline_after(zx::msec(10)));
  }
  th.join();
}

// TODO(https://fxbug.dev/42166029): Disabled until flakes are fixed.
TEST_F(UsbAudioTest, DISABLED_RingBufferStartBeforeGetVmo) {
  incoming_.SyncCall([](IncomingNamespace<FakeDevice>* infra) {
    ASSERT_OK(UsbAudioDevice::DriverBind(infra->fake_dev->zxdev()));
  });
  ASSERT_EQ(root_->GetLatestChild()->child_count(), 1);

  ASSERT_EQ(root_->GetLatestChild()->GetLatestChild()->child_count(), 2);
  auto itr = root_->GetLatestChild()->GetLatestChild()->children().begin();
  std::advance(itr, 0);
  UsbAudioStream* dut = (*itr)->GetDeviceContext<UsbAudioStream>();

  auto endpoints = fidl::Endpoints<audio_fidl::StreamConfigConnector>::Create();
  fidl::BindServer(loop_.dispatcher(), std::move(endpoints.server), dut);
  auto stream_client = GetStreamClient(std::move(endpoints.client));
  ASSERT_TRUE(stream_client.is_valid());

  auto [local, remote] = fidl::Endpoints<audio_fidl::RingBuffer>::Create();

  fidl::Arena allocator;
  audio_fidl::wire::Format format(allocator);
  format.set_pcm_format(allocator, GetDefaultPcmFormat());
  auto rb = stream_client->CreateRingBuffer(format, std::move(remote));
  ASSERT_OK(rb.status());

  // Start() before GetVmo() must result in channel closure
  auto start = fidl::WireCall<audio_fidl::RingBuffer>(local)->Start();
  ASSERT_EQ(ZX_ERR_PEER_CLOSED, start.status());  // We get a channel close.
}

//// TODO(https://fxbug.dev/42166029): Disabled until flakes are fixed.
TEST_F(UsbAudioTest, DISABLED_RingBufferStartWhileStarted) {
  incoming_.SyncCall([](IncomingNamespace<FakeDevice>* infra) {
    ASSERT_OK(UsbAudioDevice::DriverBind(infra->fake_dev->zxdev()));
  });
  ASSERT_EQ(root_->GetLatestChild()->child_count(), 1);

  ASSERT_EQ(root_->GetLatestChild()->GetLatestChild()->child_count(), 2);
  auto itr = root_->GetLatestChild()->GetLatestChild()->children().begin();
  std::advance(itr, 0);
  UsbAudioStream* dut = (*itr)->GetDeviceContext<UsbAudioStream>();

  auto endpoints = fidl::Endpoints<audio_fidl::StreamConfigConnector>::Create();
  fidl::BindServer(loop_.dispatcher(), std::move(endpoints.server), dut);
  auto stream_client = GetStreamClient(std::move(endpoints.client));
  ASSERT_TRUE(stream_client.is_valid());

  auto [local, remote] = fidl::Endpoints<audio_fidl::RingBuffer>::Create();

  fidl::Arena allocator;
  audio_fidl::wire::Format format(allocator);
  format.set_pcm_format(allocator, GetDefaultPcmFormat());
  auto rb = stream_client->CreateRingBuffer(format, std::move(remote));
  ASSERT_OK(rb.status());

  auto vmo = fidl::WireCall<audio_fidl::RingBuffer>(local)->GetVmo(kTestFrameRate, 0);
  ASSERT_OK(vmo.status());

  std::atomic<bool> done = {};
  auto& ring_buffer = local;
  incoming_.SyncCall(
      [](IncomingNamespace<FakeDevice>* infra) { infra->fake_dev->ExpectConnectToEndpoint(1); });
  auto th = std::thread([&ring_buffer, &done] {
    auto start = fidl::WireCall<audio_fidl::RingBuffer>(ring_buffer)->Start();
    ASSERT_OK(start.status());
    auto restart = fidl::WireCall<audio_fidl::RingBuffer>(ring_buffer)->Start();
    ASSERT_EQ(ZX_ERR_PEER_CLOSED, restart.status());  // We get a channel close.
    auto stop = fidl::WireCall<audio_fidl::RingBuffer>(ring_buffer)->Stop();
    ASSERT_EQ(ZX_ERR_PEER_CLOSED, stop.status());  // We already got a channel closed.
    done.store(true);
  });

  // Reply until done.
  while (!done.load()) {
    incoming_.SyncCall([](IncomingNamespace<FakeDevice>* infra) {
      infra->fake_dev->fake_endpoint(1).RequestComplete(ZX_OK, 0);
    });
    // Delay a bit, so there is time for non-data handling, e.g. Stop().
    zx::nanosleep(zx::deadline_after(zx::msec(10)));
  }
  th.join();
  // Drain until no more requests are pending.
  incoming_.SyncCall([](IncomingNamespace<FakeDevice>* infra) {
    for (size_t i = 0; i < MAX_OUTSTANDING_REQ; i++) {
      infra->fake_dev->fake_endpoint(1).RequestComplete(ZX_OK, 0);
    }
  });
}

// TODO(https://fxbug.dev/42166029): Disabled until flakes are fixed.
TEST_F(UsbAudioTest, DISABLED_RingBufferStopBeforeGetVmo) {
  incoming_.SyncCall([](IncomingNamespace<FakeDevice>* infra) {
    ASSERT_OK(UsbAudioDevice::DriverBind(infra->fake_dev->zxdev()));
  });
  ASSERT_EQ(root_->GetLatestChild()->child_count(), 1);

  ASSERT_EQ(root_->GetLatestChild()->GetLatestChild()->child_count(), 2);
  auto itr = root_->GetLatestChild()->GetLatestChild()->children().begin();
  std::advance(itr, 0);
  UsbAudioStream* dut = (*itr)->GetDeviceContext<UsbAudioStream>();

  auto endpoints = fidl::Endpoints<audio_fidl::StreamConfigConnector>::Create();
  fidl::BindServer(loop_.dispatcher(), std::move(endpoints.server), dut);
  auto stream_client = GetStreamClient(std::move(endpoints.client));
  ASSERT_TRUE(stream_client.is_valid());

  auto [local, remote] = fidl::Endpoints<audio_fidl::RingBuffer>::Create();

  fidl::Arena allocator;
  audio_fidl::wire::Format format(allocator);
  format.set_pcm_format(allocator, GetDefaultPcmFormat());
  auto rb = stream_client->CreateRingBuffer(format, std::move(remote));
  ASSERT_OK(rb.status());

  // Stop() before GetVmo() must result in channel closure
  auto stop = fidl::WireCall<audio_fidl::RingBuffer>(local)->Stop();
  ASSERT_EQ(ZX_ERR_PEER_CLOSED, stop.status());  // We get a channel close.
}

TEST_F(UsbAudioTest, RingBufferStopWhileStopped) {
  incoming_.SyncCall([](IncomingNamespace<FakeDevice>* infra) {
    ASSERT_OK(UsbAudioDevice::DriverBind(infra->fake_dev->zxdev()));
  });
  ASSERT_EQ(root_->GetLatestChild()->child_count(), 1);

  ASSERT_EQ(root_->GetLatestChild()->GetLatestChild()->child_count(), 2);
  auto itr = root_->GetLatestChild()->GetLatestChild()->children().begin();
  std::advance(itr, 0);
  UsbAudioStream* dut = (*itr)->GetDeviceContext<UsbAudioStream>();

  auto endpoints = fidl::Endpoints<audio_fidl::StreamConfigConnector>::Create();
  fidl::BindServer(loop_.dispatcher(), std::move(endpoints.server), dut);
  auto stream_client = GetStreamClient(std::move(endpoints.client));
  ASSERT_TRUE(stream_client.is_valid());

  auto [local, remote] = fidl::Endpoints<audio_fidl::RingBuffer>::Create();

  fidl::Arena allocator;
  audio_fidl::wire::Format format(allocator);
  format.set_pcm_format(allocator, GetDefaultPcmFormat());
  auto rb = stream_client->CreateRingBuffer(format, std::move(remote));
  ASSERT_OK(rb.status());

  auto vmo = fidl::WireCall<audio_fidl::RingBuffer>(local)->GetVmo(kTestFrameRate, 0);
  ASSERT_OK(vmo.status());

  // We are already stopped, but this should be harmless
  auto stop = fidl::WireCall<audio_fidl::RingBuffer>(local)->Stop();
  ASSERT_OK(stop.status());
  // Another stop immediately afterward should also be harmless
  auto restop = fidl::WireCall<audio_fidl::RingBuffer>(local)->Stop();
  ASSERT_OK(restop.status());
}

TEST_F(UsbAudioTest, Unplug) {
  incoming_.SyncCall([](IncomingNamespace<FakeDevice>* infra) {
    ASSERT_OK(UsbAudioDevice::DriverBind(infra->fake_dev->zxdev()));
  });
  ASSERT_EQ(root_->GetLatestChild()->child_count(), 1);

  ASSERT_EQ(root_->GetLatestChild()->GetLatestChild()->child_count(), 2);
  auto itr = root_->GetLatestChild()->GetLatestChild()->children().begin();
  std::advance(itr, 0);
  UsbAudioStream* dut = (*itr)->GetDeviceContext<UsbAudioStream>();

  auto endpoints = fidl::Endpoints<audio_fidl::StreamConfigConnector>::Create();
  fidl::BindServer(loop_.dispatcher(), std::move(endpoints.server), dut);
  auto stream_client = GetStreamClient(std::move(endpoints.client));
  ASSERT_TRUE(stream_client.is_valid());

  auto [local, remote] = fidl::Endpoints<audio_fidl::RingBuffer>::Create();

  fidl::Arena allocator;
  audio_fidl::wire::Format format(allocator);
  format.set_pcm_format(allocator, GetDefaultPcmFormat());
  auto rb = stream_client->CreateRingBuffer(format, std::move(remote));
  ASSERT_OK(rb.status());

  auto result = fidl::WireCall<audio_fidl::RingBuffer>(local)->GetProperties();
  ASSERT_OK(result.status());
  // We don't know what the reported driver_transfer_bytes (the minimum required lead time)
  // is going to be as it will depend on hardware details, but we do know that
  // it will need to be greater than 0.
  ASSERT_GT(result.value().properties.driver_transfer_bytes(), 0);
  ASSERT_EQ(result.value().properties.needs_cache_flush_or_invalidate(), true);

  constexpr uint32_t kNumberOfPositionNotifications = 5;
  constexpr uint32_t kMinFrames = 10;
  auto vmo = fidl::WireCall<audio_fidl::RingBuffer>(local)->GetVmo(kMinFrames,
                                                                   kNumberOfPositionNotifications);
  ASSERT_OK(vmo.status());

  auto& ring_buffer = local;
  incoming_.SyncCall(
      [](IncomingNamespace<FakeDevice>* infra) { infra->fake_dev->ExpectConnectToEndpoint(1); });
  auto th = std::thread([&ring_buffer] {
    auto start = fidl::WireCall<audio_fidl::RingBuffer>(ring_buffer)->Start();
    ASSERT_EQ(ZX_ERR_PEER_CLOSED, start.status());
  });

  incoming_.SyncCall([](IncomingNamespace<FakeDevice>* infra) {
    for (size_t i = 0; i < MAX_OUTSTANDING_REQ; i++) {
      infra->fake_dev->fake_endpoint(1).RequestComplete(ZX_ERR_IO_NOT_PRESENT, 0);
    }
  });
  th.join();

  auto properties = stream_client->GetProperties();
  ASSERT_EQ(ZX_ERR_PEER_CLOSED, properties.status());
}

TEST_F(UsbAudioTest, GetDriverTransferBytes) {
  incoming_.SyncCall([](IncomingNamespace<FakeDevice>* infra) {
    ASSERT_OK(UsbAudioDevice::DriverBind(infra->fake_dev->zxdev()));
  });
  ASSERT_EQ(root_->GetLatestChild()->child_count(), 1);

  ASSERT_EQ(root_->GetLatestChild()->GetLatestChild()->child_count(), 2);
  auto itr = root_->GetLatestChild()->GetLatestChild()->children().begin();
  std::advance(itr, 0);
  UsbAudioStream* dut = (*itr)->GetDeviceContext<UsbAudioStream>();

  auto endpoints = fidl::Endpoints<audio_fidl::StreamConfigConnector>::Create();
  fidl::BindServer(loop_.dispatcher(), std::move(endpoints.server), dut);
  auto stream_client = GetStreamClient(std::move(endpoints.client));

  auto [local, remote] = fidl::Endpoints<audio_fidl::RingBuffer>::Create();

  fidl::Arena allocator;
  audio_fidl::wire::Format format(allocator);
  format.set_pcm_format(allocator, GetDefaultPcmFormat());
  auto rb = stream_client->CreateRingBuffer(format, std::move(remote));
  ASSERT_OK(rb.status());

  auto result = fidl::WireCall(local)->GetProperties();
  ASSERT_OK(result.status());
  // transfer bytes is 1152 = 6 ms x 48'000 Hz x 4 frame size.
  ASSERT_EQ(result.value().properties.driver_transfer_bytes(), 1'152);
}

TEST_F(UsbAudioTest, VmoSize) {
  incoming_.SyncCall([](IncomingNamespace<FakeDevice>* infra) {
    ASSERT_OK(UsbAudioDevice::DriverBind(infra->fake_dev->zxdev()));
  });
  ASSERT_EQ(root_->GetLatestChild()->child_count(), 1);

  ASSERT_EQ(root_->GetLatestChild()->GetLatestChild()->child_count(), 2);
  auto itr = root_->GetLatestChild()->GetLatestChild()->children().begin();
  std::advance(itr, 0);
  UsbAudioStream* dut = (*itr)->GetDeviceContext<UsbAudioStream>();

  auto endpoints = fidl::Endpoints<audio_fidl::StreamConfigConnector>::Create();
  fidl::BindServer(loop_.dispatcher(), std::move(endpoints.server), dut);
  auto stream_client = GetStreamClient(std::move(endpoints.client));
  ASSERT_TRUE(stream_client.is_valid());

  auto [local, remote] = fidl::Endpoints<audio_fidl::RingBuffer>::Create();

  fidl::Arena allocator;
  audio_fidl::wire::Format format(allocator);
  auto default_format = GetDefaultPcmFormat();
  format.set_pcm_format(allocator, default_format);
  uint32_t frame_size = default_format.number_of_channels * default_format.bytes_per_sample;
  auto rb = stream_client->CreateRingBuffer(format, std::move(remote));
  ASSERT_OK(rb.status());

  constexpr uint32_t kNumberOfPositionNotifications = 5;
  constexpr uint32_t kMinFrames = 10;
  auto vmo = fidl::WireCall<audio_fidl::RingBuffer>(local)->GetVmo(kMinFrames,
                                                                   kNumberOfPositionNotifications);
  ASSERT_OK(vmo.status());
  // transfer bytes is 6 ms x frame rate x frame size.
  static_assert(kTestFrameRate % 1'000 == 0);
  uint32_t transfer_bytes = 6 * kTestFrameRate * frame_size / 1'000;
  uint32_t frames_expected = kMinFrames + ((transfer_bytes + frame_size - 1) / frame_size);
  ASSERT_EQ(vmo->value()->num_frames, frames_expected);
}

TEST_F(UsbAudioTest, VmoSizeOverflow) {
  incoming_.SyncCall([](IncomingNamespace<FakeDevice>* infra) {
    ASSERT_OK(UsbAudioDevice::DriverBind(infra->fake_dev->zxdev()));
  });
  ASSERT_EQ(root_->GetLatestChild()->child_count(), 1);

  ASSERT_EQ(root_->GetLatestChild()->GetLatestChild()->child_count(), 2);
  auto itr = root_->GetLatestChild()->GetLatestChild()->children().begin();
  std::advance(itr, 0);
  UsbAudioStream* dut = (*itr)->GetDeviceContext<UsbAudioStream>();

  auto endpoints = fidl::Endpoints<audio_fidl::StreamConfigConnector>::Create();
  fidl::BindServer(loop_.dispatcher(), std::move(endpoints.server), dut);
  auto stream_client = GetStreamClient(std::move(endpoints.client));
  ASSERT_TRUE(stream_client.is_valid());

  auto [local, remote] = fidl::Endpoints<audio_fidl::RingBuffer>::Create();

  fidl::Arena allocator;
  audio_fidl::wire::Format format(allocator);
  auto default_format = GetDefaultPcmFormat();
  format.set_pcm_format(allocator, default_format);
  auto rb = stream_client->CreateRingBuffer(format, std::move(remote));
  ASSERT_OK(rb.status());

  constexpr uint32_t kNumberOfPositionNotifications = 5;
  constexpr uint32_t kMinFrames = 0x3FFFFFA1;
  auto vmo = fidl::WireCall<audio_fidl::RingBuffer>(local)->GetVmo(kMinFrames,
                                                                   kNumberOfPositionNotifications);
  ASSERT_OK(vmo.status());
  ASSERT_TRUE(vmo->is_error());
  ASSERT_EQ(vmo->error_value(), audio_fidl::wire::GetVmoError::kInternalError);
}

TEST_F(UsbAudioTest, LargeTransferSizeOob) {
  incoming_.SyncCall([](IncomingNamespace<FakeDevice>* infra) {
    ASSERT_OK(UsbAudioDevice::DriverBind(infra->fake_dev->zxdev()));
  });
  ASSERT_EQ(root_->GetLatestChild()->child_count(), 1);

  ASSERT_EQ(root_->GetLatestChild()->GetLatestChild()->child_count(), 2);
  auto itr = root_->GetLatestChild()->GetLatestChild()->children().begin();
  std::advance(itr, 0);
  UsbAudioStream* dut = (*itr)->GetDeviceContext<UsbAudioStream>();

  auto endpoints = fidl::Endpoints<audio_fidl::StreamConfigConnector>::Create();
  fidl::BindServer(loop_.dispatcher(), std::move(endpoints.server), dut);
  auto stream_client = GetStreamClient(std::move(endpoints.client));
  ASSERT_TRUE(stream_client.is_valid());

  auto [local, remote] = fidl::Endpoints<audio_fidl::RingBuffer>::Create();

  fidl::Arena allocator;
  audio_fidl::wire::Format format(allocator);
  format.set_pcm_format(allocator, GetDefaultPcmFormat());
  auto rb = stream_client->CreateRingBuffer(format, std::move(remote));
  ASSERT_OK(rb.status());

  constexpr uint32_t kNumberOfPositionNotifications = 5;
  constexpr uint32_t kMinFrames = 10;
  auto vmo = fidl::WireCall<audio_fidl::RingBuffer>(local)->GetVmo(kMinFrames,
                                                                   kNumberOfPositionNotifications);
  ASSERT_OK(vmo.status());

  std::atomic<bool> done = {};
  auto& ring_buffer = local;
  incoming_.SyncCall(
      [](IncomingNamespace<FakeDevice>* infra) { infra->fake_dev->ExpectConnectToEndpoint(1); });
  auto th = std::thread([&ring_buffer, &done] {
    auto start = fidl::WireCall<audio_fidl::RingBuffer>(ring_buffer)->Start();
    ASSERT_OK(start.status());
    auto stop = fidl::WireCall<audio_fidl::RingBuffer>(ring_buffer)->Stop();
    ASSERT_OK(stop.status());
    done.store(true);
  });

  while (!done.load()) {
    incoming_.SyncCall([](IncomingNamespace<FakeDevice>* infra) {
      infra->fake_dev->fake_endpoint(1).RequestComplete(ZX_OK, 100000);
    });
    zx::nanosleep(zx::deadline_after(zx::msec(10)));
  }
  th.join();
}

}  // namespace audio::usb

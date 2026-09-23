// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/hardware/usb/cpp/banjo-mock.h>
#include <lib/zx/channel.h>
#include <lib/zx/vmo.h>

#include <cstring>

#include <zxtest/zxtest.h>

#include "src/camera/drivers/usb_video/usb_video_stream.h"
#include "src/devices/testing/mock-ddk/mock-device.h"

bool operator==(const usb_endpoint_descriptor_t& lhs, const usb_endpoint_descriptor_t& rhs) {
  return lhs.b_length == rhs.b_length && lhs.b_descriptor_type == rhs.b_descriptor_type &&
         lhs.b_endpoint_address == rhs.b_endpoint_address &&
         lhs.bm_attributes == rhs.bm_attributes && lhs.w_max_packet_size == rhs.w_max_packet_size &&
         lhs.b_interval == rhs.b_interval;
}

bool operator==(const usb_ss_ep_comp_descriptor_t& lhs, const usb_ss_ep_comp_descriptor_t& rhs) {
  return lhs.b_length == rhs.b_length && lhs.b_descriptor_type == rhs.b_descriptor_type &&
         lhs.b_max_burst == rhs.b_max_burst && lhs.bm_attributes == rhs.bm_attributes &&
         lhs.w_bytes_per_interval == rhs.w_bytes_per_interval;
}

bool operator==(const usb_request_t& lhs, const usb_request_t& rhs) { return true; }

bool operator==(const usb_request_complete_callback_t& lhs,
                const usb_request_complete_callback_t& rhs) {
  return true;
}

namespace camera::usb_video {

namespace {

// Creates a `BufferCollectionInfo` with one VMO of size `actual_vmo_size`.
// `BufferCollectionInfo::vmo_size` will be set to `declared_vmo_size`.
fuchsia::sysmem::BufferCollectionInfo CreateBufferCollection(uint64_t declared_vmo_size,
                                                             uint64_t actual_vmo_size) {
  fuchsia::sysmem::BufferCollectionInfo buffer_collection;
  buffer_collection.buffer_count = 1;
  buffer_collection.vmo_size = declared_vmo_size;
  buffer_collection.format.image.width = 640;
  buffer_collection.format.image.height = 480;
  buffer_collection.format.image.layers = 2;
  buffer_collection.format.image.pixel_format.type = fuchsia::sysmem::PixelFormatType::NV12;
  buffer_collection.format.image.planes[0].bytes_per_row = 640;
  buffer_collection.format.image.planes[1].bytes_per_row = 640;

  zx::vmo vmo;
  ZX_ASSERT(zx::vmo::create(actual_vmo_size, 0, &vmo) == ZX_OK);
  buffer_collection.vmos[0] = std::move(vmo);
  return buffer_collection;
}

class UsbVideoTest : public zxtest::Test {
 public:
  void SetUp() override { fake_parent_ = MockDevice::FakeRootParent(); }

  void TearDown() override { ASSERT_NO_FATAL_FAILURE(usb_.VerifyAndClear()); }

 protected:
  // Returns whether or not `channel` is closed.
  static bool IsChannelClosed(const zx::channel& channel, zx::duration timeout = zx::msec(10)) {
    zx_signals_t observed = 0;
    zx_status_t status =
        channel.wait_one(ZX_CHANNEL_PEER_CLOSED, zx::deadline_after(timeout), &observed);
    return status == ZX_OK && (observed & ZX_CHANNEL_PEER_CLOSED);
  }

  // Sends a CreateStream FIDL request to `stream` containing `buffer_collection`.
  static fidl::InterfaceHandle<fuchsia::camera::Stream> CallCreateStream(
      UsbVideoStream* stream, fuchsia::sysmem::BufferCollectionInfo buffer_collection,
      zx::eventpair stream_token) {
    fuchsia::camera::FrameRate frame_rate{
        .frames_per_sec_numerator = 10000000,
        .frames_per_sec_denominator = 333333,
    };
    fidl::InterfaceHandle<fuchsia::camera::Stream> stream_handle;
    auto stream_req = stream_handle.NewRequest();

    static_cast<fuchsia::camera::Control*>(stream)->CreateStream(
        std::move(buffer_collection), frame_rate, std::move(stream_req), std::move(stream_token));
    return stream_handle;
  }

  // Mocks a successful UVC SetFormat sequence (Probe and Commit controls).
  // Configures the mock to expect:
  // * A SET_CUR probe request containing the proposed format/frame settings.
  // * A GET_CUR probe request returning the negotiated frame size (10KB) and payload size.
  // * A SET_CUR commit request executing the negotiated format.
  // * 8 calls to get the USB request size (for allocation of the request pool).
  void SetupSuccessfulSetFormat() {
    usb_video_vc_probe_and_commit_controls proposal;
    memset(&proposal, 0, sizeof(proposal));
    proposal.bmHint = USB_VIDEO_BM_HINT_FRAME_INTERVAL;
    proposal.bFormatIndex = 1;
    proposal.bFrameIndex = 1;
    proposal.dwFrameInterval = 333333;

    std::vector<uint8_t> proposal_bytes(reinterpret_cast<uint8_t*>(&proposal),
                                        reinterpret_cast<uint8_t*>(&proposal) + sizeof(proposal));

    usb_.ExpectControlOut(ZX_OK, USB_DIR_OUT | USB_TYPE_CLASS | USB_RECIP_INTERFACE,
                          USB_VIDEO_SET_CUR, USB_VIDEO_VS_PROBE_CONTROL << 8, 1, ZX_TIME_INFINITE,
                          proposal_bytes);

    usb_video_vc_probe_and_commit_controls response;
    memset(&response, 0, sizeof(response));
    response.bmHint = USB_VIDEO_BM_HINT_FRAME_INTERVAL;
    response.bFormatIndex = 1;
    response.bFrameIndex = 1;
    response.dwFrameInterval = 333333;
    response.dwMaxVideoFrameSize = 10240;  // 10KB max frame size
    response.dwMaxPayloadTransferSize = 1024;

    std::vector<uint8_t> response_bytes(reinterpret_cast<uint8_t*>(&response),
                                        reinterpret_cast<uint8_t*>(&response) + sizeof(response));

    usb_.ExpectControlIn(ZX_OK, USB_DIR_IN | USB_TYPE_CLASS | USB_RECIP_INTERFACE,
                         USB_VIDEO_GET_CUR, USB_VIDEO_VS_PROBE_CONTROL << 8, 1, ZX_TIME_INFINITE,
                         response_bytes);

    usb_.ExpectControlOut(ZX_OK, USB_DIR_OUT | USB_TYPE_CLASS | USB_RECIP_INTERFACE,
                          USB_VIDEO_SET_CUR, USB_VIDEO_VS_COMMIT_CONTROL << 8, 1, ZX_TIME_INFINITE,
                          response_bytes);

    for (int i = 0; i < 8; ++i) {
      usb_.ExpectGetRequestSize(sizeof(usb_request_t));
    }
  }

  // Mocks the calls that occur when the driver activates streaming.
  // Configures the mock to expect:
  // * A set-interface call to activate the alternate streaming setting.
  // * 8 calls to queue the allocated USB requests to start receiving frames.
  void SetupSuccessfulStartStreaming() {
    usb_.ExpectSetInterface(ZX_OK, 1, 1);
    usb_request_t dummy_req{};
    usb_request_complete_callback_t dummy_cb{};
    for (int i = 0; i < 8; ++i) {
      usb_.ExpectRequestQueue(dummy_req, dummy_cb);
    }
  }

  // Mocks the calls that occur when the driver deactivates streaming.
  // Configures the mock to expect:
  // * A set interface call to reset the interface to setting 0 (no streaming/no bandwidth).
  void SetupSuccessfulStopStreaming() {
    usb_.ExpectCancelAll(ZX_OK, 0x81);
    usb_.ExpectSetInterface(ZX_OK, 1, 0);
  }

  // Creates a `UsbVideoStream` with a default set of UVC streaming settings.
  std::unique_ptr<UsbVideoStream> CreateStream() {
    StreamingSetting settings;
    settings.hw_clock_frequency = 1000000;
    settings.vs_interface.b_interface_number = 1;
    settings.input_header.bEndpointAddress = 0x81;

    UvcFormat format{
        .format_index = 1,
        .frame_index = 1,
        .pixel_format = UvcPixelFormat::NV12,
        .bits_per_pixel = 12,
        .default_frame_interval = 333333,
        .width = 640,
        .height = 480,
        .stride = 640,
        .default_frame_index = 1,
    };
    settings.formats.push_back(std::move(format));

    StreamingEndpointSetting ep_setting{
        .address = 0x81,
        .alt_setting = 1,
        .isoc_bandwidth = 1024,
        .ep_type = USB_ENDPOINT_ISOCHRONOUS,
    };
    settings.endpoint_settings.push_back(ep_setting);

    return std::make_unique<UsbVideoStream>(fake_parent_.get(), *usb_.GetProto(),
                                            std::move(settings));
  }

  ddk::MockUsb& usb() { return usb_; }

 private:
  std::shared_ptr<MockDevice> fake_parent_;
  ddk::MockUsb usb_;
};

// Verifies that a stream is successfully created and initialized when a valid buffer collection is
// provided.
TEST_F(UsbVideoTest, CreateStream_Success) {
  SetupSuccessfulSetFormat();
  SetupSuccessfulStartStreaming();
  SetupSuccessfulStopStreaming();
  auto stream = CreateStream();

  zx::eventpair client_token, server_token;
  ASSERT_OK(zx::eventpair::create(0, &client_token, &server_token));
  auto buffer_collection = CreateBufferCollection(10240, 10240);
  auto stream_handle =
      CallCreateStream(stream.get(), std::move(buffer_collection), std::move(server_token));

  // Verify the channel is operational.
  EXPECT_FALSE(UsbVideoTest::IsChannelClosed(stream_handle.channel()));

  // Explicitly close the channel to trigger shutdown.
  stream_handle = {};

  // Wait for the stream to be fully stopped. The driver will close its end of the stream_token
  // eventpair when shutdown is complete.
  zx_signals_t observed = 0;
  zx_status_t status =
      client_token.wait_one(ZX_EVENTPAIR_PEER_CLOSED, zx::deadline_after(zx::sec(20)), &observed);
  EXPECT_OK(status);
  EXPECT_TRUE(observed & ZX_EVENTPAIR_PEER_CLOSED);
}

// Verifies that stream creation fails if the physical size of the VMOs in the buffer collection is
// less than the negotiated max frame size.
TEST_F(UsbVideoTest, CreateStream_FailPhysicalSizeTooSmall) {
  SetupSuccessfulSetFormat();
  auto stream = CreateStream();

  const uint64_t kVmoSize = 10240;
  const uint64_t kPhysicalSize = 4096;
  ASSERT_LT(kPhysicalSize, kVmoSize);

  // VMO size is declared as 10KB, but physical VMO size is only 4KB. This should fail during
  // physical size check.
  auto buffer_collection = CreateBufferCollection(kVmoSize, kPhysicalSize);
  auto stream_handle =
      CallCreateStream(stream.get(), std::move(buffer_collection), zx::eventpair{});

  // Verify the channel failed to be created.
  EXPECT_TRUE(UsbVideoTest::IsChannelClosed(stream_handle.channel(), zx::sec(1)));
}

}  // namespace

}  // namespace camera::usb_video

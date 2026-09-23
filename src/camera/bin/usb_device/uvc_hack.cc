// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/camera/bin/usb_device/uvc_hack.h"

namespace camera {

// Client buffers are supporting NV12
void UvcHackGetClientBufferImageFormatConstraints(
    fuchsia::sysmem2::ImageFormatConstraints* image_format_constraints) {
  auto& ifc = *image_format_constraints;
  ifc.set_pixel_format(kUvcHackClientPixelFormat);
  ifc.mutable_color_spaces()->emplace_back(kUvcHackClientColorSpace);
  ifc.set_min_size(
      fuchsia::math::SizeU{.width = kUvcHackClientCodedWidth, .height = kUvcHackClientCodedHeight});
  ifc.set_max_size(
      fuchsia::math::SizeU{.width = kUvcHackClientCodedWidth, .height = kUvcHackClientCodedHeight});
  ifc.set_min_bytes_per_row(kUvcHackClientBytesPerRow);
  ifc.set_max_bytes_per_row(kUvcHackClientBytesPerRow);
  ifc.set_max_width_times_height(kUvcHackClientCodedWidth * kUvcHackClientCodedHeight);
  ifc.set_size_alignment(fuchsia::math::SizeU{.width = kUvcHackClientCodedWidthDivisor,
                                              .height = kUvcHackClientCodedHeightDivisor});
  ifc.set_bytes_per_row_divisor(kUvcHackClientBytesPerRowDivisor);
  ifc.set_start_offset_divisor(kUvcHackClientStartOffsetDivisor);
  ifc.set_display_rect_alignment(fuchsia::math::SizeU{
      .width = kUvcHackClientDisplayWidthDivisor, .height = kUvcHackClientDisplayHeightDivisor});
  ifc.set_required_min_size(
      fuchsia::math::SizeU{.width = kUvcHackClientCodedWidth, .height = kUvcHackClientCodedHeight});
  ifc.set_required_max_size(
      fuchsia::math::SizeU{.width = kUvcHackClientCodedWidth, .height = kUvcHackClientCodedHeight});
}

// Client buffers are supporting NV12
void UvcHackGetClientBufferCollectionConstraints(
    fuchsia::sysmem2::BufferCollectionConstraints* buffer_collection_constraints) {
  auto& constraints = *buffer_collection_constraints;
  constraints.mutable_usage()->set_cpu(fuchsia::sysmem2::CPU_USAGE_READ |
                                       fuchsia::sysmem2::CPU_USAGE_WRITE);
  constraints.set_min_buffer_count_for_camping(kUvcHackClientMinBufferCountForCamping);
  constraints.set_min_buffer_count_for_dedicated_slack(
      kUvcHackClientMinBufferCountForDedicatedSlack);
  constraints.set_min_buffer_count_for_shared_slack(kUvcHackClientMinBufferCountForSharedSlack);
  constraints.set_min_buffer_count(kUvcHackClientMinBufferCount);
  constraints.set_max_buffer_count(kUvcHackClientMaxBufferCount);
  auto& bmc = *constraints.mutable_buffer_memory_constraints();
  bmc.set_ram_domain_supported(true);
  bmc.set_cpu_domain_supported(true);
  auto& ifc = constraints.mutable_image_format_constraints()->emplace_back();
  UvcHackGetClientBufferImageFormatConstraints(&ifc);
}

// Client buffers are supporting NV12
void UvcHackGetClientStreamProperties(fuchsia::camera3::StreamProperties* stream_properties) {
  stream_properties->image_format.pixel_format.type = kUvcHackClientPixelFormatType;
  stream_properties->image_format.coded_width = kUvcHackClientCodedWidth;
  stream_properties->image_format.coded_height = kUvcHackClientCodedHeight;
  stream_properties->image_format.bytes_per_row = kUvcHackClientBytesPerRow;
  stream_properties->image_format.display_width = kUvcHackClientDisplayWidth;
  stream_properties->image_format.display_height = kUvcHackClientDisplayHeight;
  stream_properties->image_format.layers = kUvcHackClientLayers;
  stream_properties->image_format.color_space.type = kUvcHackClientColorSpaceType;
  stream_properties->frame_rate.numerator = kUvcHackFrameRateNumerator;
  stream_properties->frame_rate.denominator = kUvcHackFrameRateDenominator;
  stream_properties->supports_crop_region = false;
}

// Client buffers are supporting NV12
void UvcHackGetClientStreamProperties2(fuchsia::camera3::StreamProperties2* stream_properties) {
  stream_properties->mutable_image_format()->pixel_format.type = kUvcHackClientPixelFormatType;
  stream_properties->mutable_image_format()->coded_width = kUvcHackClientCodedWidth;
  stream_properties->mutable_image_format()->coded_height = kUvcHackClientCodedHeight;
  stream_properties->mutable_image_format()->bytes_per_row = kUvcHackClientBytesPerRow;
  stream_properties->mutable_image_format()->display_width = kUvcHackClientDisplayWidth;
  stream_properties->mutable_image_format()->display_height = kUvcHackClientDisplayHeight;
  stream_properties->mutable_image_format()->layers = kUvcHackClientLayers;
  stream_properties->mutable_image_format()->color_space.type = kUvcHackClientColorSpaceType;
  stream_properties->mutable_frame_rate()->numerator = kUvcHackFrameRateNumerator;
  stream_properties->mutable_frame_rate()->denominator = kUvcHackFrameRateDenominator;
  *(stream_properties->mutable_supports_crop_region()) = false;
  {
    fuchsia::math::Size resolution;
    resolution.width = kUvcHackClientCodedWidth;
    resolution.height = kUvcHackClientCodedHeight;
    stream_properties->mutable_supported_resolutions()->push_back(std::move(resolution));
  }
}

// Server buffers are supporting YUY2 (a.k.a. YUYV).
void UvcHackGetServerFrameRate(fuchsia::camera::FrameRate* frame_rate) {
  frame_rate->frames_per_sec_numerator = kUvcHackFrameRateNumerator;
  frame_rate->frames_per_sec_denominator = kUvcHackFrameRateDenominator;
}

// Server buffers are supporting YUY2 (a.k.a. YUYV).
void UvcHackGetServerBufferVideoFormat(fuchsia::camera::VideoFormat* video_format) {
  video_format->format.width = kUvcHackDriverWidth;
  video_format->format.height = kUvcHackDriverHeight;
  video_format->format.layers = kUvcHackDriverLayers;
  video_format->format.pixel_format.type = kUvcHackDriverPixelFormatType;
  video_format->format.pixel_format.has_format_modifier = false;
  video_format->format.color_space.type = kUvcHackDriverColorSpaceType;
  video_format->format.planes[0].byte_offset = 0;
  video_format->format.planes[0].bytes_per_row = kUvcHackDriverBytesPerRow;
  UvcHackGetServerFrameRate(&video_format->rate);
}

// Warning! Grotesquely hard coded for YUY2 server side & NV12 client side.
//
// TODO(ernesthua) - Replace this with libyuv!
void UvcHackConvertYUY2ToNV12(uint8_t* client_frame, const uint8_t* driver_frame) {
  ZX_ASSERT(kUvcHackClientPixelFormatType == fuchsia::sysmem::PixelFormatType::NV12);
  ZX_ASSERT(kUvcHackDriverPixelFormat == fuchsia::images2::PixelFormat::YUY2);

  ZX_ASSERT(kUvcHackClientCodedWidth == kUvcHackWidth);
  ZX_ASSERT(kUvcHackClientCodedHeight == kUvcHackHeight);
  ZX_ASSERT(kUvcHackDriverWidth == kUvcHackWidth);
  ZX_ASSERT(kUvcHackDriverHeight == kUvcHackHeight);

  ZX_ASSERT((kUvcHackClientBytesPerRow * 2) == kUvcHackDriverBytesPerRow);

  constexpr uint32_t kClientBufferLimit =
      kUvcHackClientBytesPerRow * kUvcHackClientCodedHeight * 3 / 2;
  constexpr uint32_t kClientUVPlaneOffset = kUvcHackClientBytesPerRow * kUvcHackClientCodedHeight;

  constexpr uint32_t kDriverBufferLimit = kUvcHackDriverBytesPerRow * kUvcHackDriverHeight;

  for (uint32_t y = 0; y < kUvcHackHeight; y++) {
    for (uint32_t x = 0; x < kUvcHackWidth; x += 2) {
      uint32_t driver_offset = (y * kUvcHackDriverBytesPerRow) + (x * 2);
      uint32_t client_offset = (y * kUvcHackClientBytesPerRow) + x;
      ZX_ASSERT(driver_offset < kDriverBufferLimit);
      ZX_ASSERT(client_offset < kClientBufferLimit);
      client_frame[client_offset + 0] = driver_frame[driver_offset + 0];
      client_frame[client_offset + 1] = driver_frame[driver_offset + 2];
    }
  }

  for (uint32_t y = 0; y < kUvcHackHeight; y += 2) {
    for (uint32_t x = 0; x < kUvcHackWidth; x += 2) {
      uint32_t driver_offset = (y * kUvcHackDriverBytesPerRow) + (x * 2);
      uint32_t client_offset = kClientUVPlaneOffset + ((y / 2) * kUvcHackClientBytesPerRow) + x;
      ZX_ASSERT(driver_offset < kDriverBufferLimit);
      ZX_ASSERT(client_offset < kClientBufferLimit);
      client_frame[client_offset + 0] = driver_frame[driver_offset + 1];
    }
  }

  for (uint32_t y = 0; y < kUvcHackHeight; y += 2) {
    for (uint32_t x = 0; x < kUvcHackWidth; x += 2) {
      uint32_t driver_offset = (y * kUvcHackDriverBytesPerRow) + (x * 2);
      uint32_t client_offset = kClientUVPlaneOffset + ((y / 2) * kUvcHackClientBytesPerRow) + x;
      ZX_ASSERT(driver_offset < kDriverBufferLimit);
      ZX_ASSERT(client_offset < kClientBufferLimit);
      client_frame[client_offset + 1] = driver_frame[driver_offset + 3];
    }
  }
}

}  // namespace camera

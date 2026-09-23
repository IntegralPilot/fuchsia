// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/camera/drivers/controller/sherlock/isp_debug_config.h"

// This file contains static information for the ISP Debug Configuration.
// There is one stream in one configuration.
// FR --> OutputStreamML (Directly from ISP)

namespace camera {

/*****************************
 * Output Stream parameters  *
 *****************************
 */

static std::vector<fuchsia::images2::ImageFormat> FRDebugStreamImageFormats() {
  std::vector<fuchsia::images2::ImageFormat> result;
  result.emplace_back(camera::StreamConstraints::MakeImageFormat(kFRStreamWidth, kFRStreamHeight,
                                                                 kStreamPixelFormat));
  return result;
}

static fuchsia::camera2::hal::StreamConfig FRDebugStreamConfig() {
  StreamConstraints stream(fuchsia::camera2::CameraStreamType::FULL_RESOLUTION);
  stream.AddImageFormat(kFRStreamWidth, kFRStreamHeight, kStreamPixelFormat);
  stream.set_bytes_per_row_divisor(kIspBytesPerRowDivisor);
  stream.set_contiguous(true);
  stream.set_frames_per_second(kFRStreamFrameRate);
  stream.set_buffer_count_for_camping(kStreamMinBufferForCamping);
  return stream.ConvertToStreamConfig();
}

static std::vector<fuchsia::images2::ImageFormat> DSDebugStreamImageFormats() {
  std::vector<fuchsia::images2::ImageFormat> result;
  result.emplace_back(camera::StreamConstraints::MakeImageFormat(kDSStreamWidth, kDSStreamHeight,
                                                                 kStreamPixelFormat));
  return result;
}

static fuchsia::camera2::hal::StreamConfig DSDebugStreamConfig() {
  StreamConstraints stream(fuchsia::camera2::CameraStreamType::DOWNSCALED_RESOLUTION);
  stream.AddImageFormat(kDSStreamWidth, kDSStreamHeight, kStreamPixelFormat);
  stream.set_bytes_per_row_divisor(kIspBytesPerRowDivisor);
  stream.set_contiguous(true);
  stream.set_frames_per_second(kDSStreamFrameRate);
  stream.set_buffer_count_for_camping(kStreamMinBufferForCamping);
  return stream.ConvertToStreamConfig();
}

/*****************************
 *  EXTERNAL CONFIGURATIONS  *
 *****************************
 */

fuchsia::camera2::hal::Config DebugConfig() {
  fuchsia::camera2::hal::Config config;
  config.stream_configs.push_back(FRDebugStreamConfig());
  config.stream_configs.push_back(DSDebugStreamConfig());
  return config;
}

/*****************************
 *  INTERNAL CONFIGURATIONS  *
 *****************************
 */

static InternalConfigNode OutputFRStream() {
  InternalConfigNode result;
  result.type = kOutputStream, result.output_frame_rate = {
                                   .frames_per_sec_numerator = kFRStreamFrameRate,
                                   .frames_per_sec_denominator = 1,
                               };
  result.supported_streams = {
      {
          .type = fuchsia::camera2::CameraStreamType::FULL_RESOLUTION,
          .supports_dynamic_resolution = false,
          .supports_crop_region = false,
      },
  };
  result.image_formats = FRDebugStreamImageFormats();
  return result;
}

InternalConfigNode DebugConfigFullRes() {
  InternalConfigNode result;

  result.type = kInputStream;
  // For node type |kInputStream| we will be ignoring the
  // frame rate divisor.
  result.output_frame_rate = {
      .frames_per_sec_numerator = kFRStreamFrameRate,
      .frames_per_sec_denominator = 1,
  };
  result.input_stream_type = fuchsia::camera2::CameraStreamType::FULL_RESOLUTION;
  result.supported_streams = {
      {
          .type = fuchsia::camera2::CameraStreamType::FULL_RESOLUTION,
          .supports_dynamic_resolution = false,
          .supports_crop_region = false,
      },
  };
  result.child_nodes = MakeVec(OutputFRStream());
  result.image_formats = FRDebugStreamImageFormats();
  return result;
}

static InternalConfigNode OutputDSStream() {
  InternalConfigNode result;
  result.type = kOutputStream;
  result.output_frame_rate = {
      .frames_per_sec_numerator = kDSStreamFrameRate,
      .frames_per_sec_denominator = 1,
  };
  result.supported_streams = {
      {
          .type = fuchsia::camera2::CameraStreamType::DOWNSCALED_RESOLUTION,
          .supports_dynamic_resolution = false,
          .supports_crop_region = false,
      },
  };
  result.image_formats = DSDebugStreamImageFormats();
  return result;
}

InternalConfigNode DebugConfigDownScaledRes() {
  InternalConfigNode result;
  result.type = kInputStream;
  // For node type |kInputStream| we will be ignoring the
  // frame rate divisor.
  result.output_frame_rate = {
      .frames_per_sec_numerator = kFRStreamFrameRate,
      .frames_per_sec_denominator = 1,
  };
  result.input_stream_type = fuchsia::camera2::CameraStreamType::DOWNSCALED_RESOLUTION;
  result.supported_streams = {
      {
          .type = fuchsia::camera2::CameraStreamType::DOWNSCALED_RESOLUTION,
          .supports_dynamic_resolution = false,
          .supports_crop_region = false,
      },
  };
  result.child_nodes = MakeVec(OutputDSStream());
  result.image_formats = DSDebugStreamImageFormats();
  return result;
}

}  // namespace camera

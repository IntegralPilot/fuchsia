// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/camera/drivers/controller/sherlock/video_conferencing_config.h"

#include <fidl/fuchsia.sysmem/cpp/hlcpp_conversion.h>
#include <lib/sysmem-version/sysmem-version.h>

// This file contains static information for the Video conferencing configuration.
// There are two streams in one configuration.
// FR --> (30fps) GDC1 --> (5fps) GDC2 --> ML(640x512)
//                 :
//                 :
//                 ----> (30fps) GE2D --> Video conferencing client

namespace camera {

fuchsia::sysmem2::BufferCollectionConstraints V2CopyFromV1BufferCollectionConstraints(
    const fuchsia::sysmem::BufferCollectionConstraints& v1) {
  auto v1_natural = fidl::HLCPPToNatural(fidl::Clone(v1));
  auto v2_natural_result = sysmem::V2CopyFromV1BufferCollectionConstraints(&v1_natural);
  ZX_ASSERT(v2_natural_result.is_ok());
  return fidl::NaturalToHLCPP(std::move(v2_natural_result.value()));
}

/**********************************
 *  ML Video FR parameters        *
 **********************************
 */

static std::vector<fuchsia::images2::ImageFormat> MLFRImageFormats() {
  return MakeVec(StreamConstraints::MakeImageFormat(kMlFRWidth, kMlFRHeight, kFramePixelFormat));
}

static fuchsia::camera2::hal::StreamConfig MLVideoFRConfig(bool extended_fov) {
  auto stream_properties = extended_fov
                               ? kMlStreamType | fuchsia::camera2::CameraStreamType::EXTENDED_FOV
                               : kMlStreamType;

  StreamConstraints stream(stream_properties);
  stream.AddImageFormat(kMlFRWidth, kMlFRHeight, kFramePixelFormat);
  stream.set_bytes_per_row_divisor(kGdcBytesPerRowDivisor);
  stream.set_contiguous(true);
  stream.set_frames_per_second(kMlFRFrameRate);
  stream.set_buffer_count_for_camping(kVideoConferencingGDC2OutputBuffers);
  stream.set_min_buffer_count(kVideoConferencingGDC2OutputBuffers + kMlFRMaxClientBuffers);
  return stream.ConvertToStreamConfig();
}

/******************************************
 * Video Conferencing FR Parameters       *
 ******************************************
 */

static std::vector<fuchsia::images2::ImageFormat> VideoImageFormats() {
  return MakeVec(
      StreamConstraints::MakeImageFormat(kVideoWidth, kVideoHeight, kFramePixelFormat),
      StreamConstraints::MakeImageFormat(kVideoWidth1, kVideoHeight1, kFramePixelFormat),
      StreamConstraints::MakeImageFormat(kVideoWidth2, kVideoHeight2, kFramePixelFormat));
}

static fuchsia::camera2::hal::StreamConfig VideoConfig(bool extended_fov) {
  auto stream_properties = extended_fov
                               ? kVideoStreamType | fuchsia::camera2::CameraStreamType::EXTENDED_FOV
                               : kVideoStreamType;

  StreamConstraints stream(stream_properties);
  stream.AddImageFormat(kVideoWidth, kVideoHeight, kFramePixelFormat, kGdcFRWidth, kGdcFRHeight);
  stream.AddImageFormat(kVideoWidth1, kVideoHeight1, kFramePixelFormat, kGdcFRWidth, kGdcFRHeight);
  stream.AddImageFormat(kVideoWidth2, kVideoHeight2, kFramePixelFormat, kGdcFRWidth, kGdcFRHeight);
  stream.set_bytes_per_row_divisor(kGdcBytesPerRowDivisor);
  stream.set_contiguous(true);
  stream.set_frames_per_second(kVideoThrottledOutputFrameRate);
  stream.set_buffer_count_for_camping(kVideoConferencingGE2DOutputBuffers);
  stream.set_min_buffer_count(kVideoConferencingGE2DOutputBuffers + kVideoMaxClientBuffers);
  return stream.ConvertToStreamConfig();
}

/*****************************
 *  EXTERNAL CONFIGURATIONS  *
 *****************************
 */

fuchsia::camera2::hal::Config VideoConferencingConfig(bool extended_fov) {
  fuchsia::camera2::hal::Config config;
  config.stream_configs.push_back(MLVideoFRConfig(extended_fov));
  config.stream_configs.push_back(VideoConfig(extended_fov));
  return config;
}

// ================== INTERNAL CONFIGURATION ======================== //

static InternalConfigNode OutputMLFR(bool extended_fov) {
  InternalConfigNode result;
  result.type = kOutputStream;
  result.output_frame_rate = {
      .frames_per_sec_numerator = kMlFRFrameRate,
      .frames_per_sec_denominator = 1,
  };
  result.supported_streams = {
      {
          .type = extended_fov ? kMlStreamType | fuchsia::camera2::CameraStreamType::EXTENDED_FOV
                               : kMlStreamType,
          .supports_dynamic_resolution = false,
          .supports_crop_region = false,
      },
  };
  result.input_constraints = CopyConstraintsWithOverrides(
      VideoConferencingConfig(extended_fov).stream_configs[0].constraints,
      {.min_buffer_count_for_camping = 0});
  result.image_formats = MLFRImageFormats();
  return result;
}

static InternalConfigNode OutputVideoConferencing(bool extended_fov) {
  InternalConfigNode result;
  result.type = kOutputStream;
  result.output_frame_rate = {
      .frames_per_sec_numerator = kVideoThrottledOutputFrameRate,
      .frames_per_sec_denominator = 1,
  };
  result.supported_streams = {
      {
          .type = extended_fov ? kVideoStreamType | fuchsia::camera2::CameraStreamType::EXTENDED_FOV
                               : kVideoStreamType,
          .supports_dynamic_resolution = false,
          .supports_crop_region = false,
      },
  };
  result.input_constraints = CopyConstraintsWithOverrides(
      VideoConferencingConfig(extended_fov).stream_configs[1].constraints,
      {.min_buffer_count_for_camping = 0});
  result.image_formats = VideoImageFormats();
  return result;
}

fuchsia::sysmem2::BufferCollectionConstraints GdcVideo2Constraints() {
  StreamConstraints stream_constraints;
  stream_constraints.set_bytes_per_row_divisor(kGdcBytesPerRowDivisor);
  stream_constraints.set_contiguous(true);
  stream_constraints.AddImageFormat(kGdcFRWidth, kGdcFRHeight, kFramePixelFormat);
  stream_constraints.set_buffer_count_for_camping(kVideoConferencingGDC2InputBuffers);
  return stream_constraints.MakeBufferCollectionConstraints();
}

static InternalConfigNode GdcVideo2(bool extended_fov) {
  InternalConfigNode result;
  result.type = kGdc;
  result.output_frame_rate = {
      .frames_per_sec_numerator = kMlFRFrameRate,
      .frames_per_sec_denominator = 1,
  };
  result.supported_streams = {
      {
          .type = extended_fov ? kMlStreamType | fuchsia::camera2::CameraStreamType::EXTENDED_FOV
                               : kMlStreamType,
          .supports_dynamic_resolution = false,
          .supports_crop_region = false,
      },
  };
  result.child_nodes = MakeVec(OutputMLFR(extended_fov));
  result.gdc_info = {
      .config_type =
          {
              GdcConfig::VIDEO_CONFERENCE_ML,
          },
  };
  result.input_constraints = GdcVideo2Constraints();
  result.output_constraints =
      V2CopyFromV1BufferCollectionConstraints(MLVideoFRConfig(extended_fov).constraints);
  result.image_formats = MLFRImageFormats();
  return result;
}

fuchsia::sysmem2::BufferCollectionConstraints Ge2dConstraints() {
  StreamConstraints stream_constraints;
  stream_constraints.set_bytes_per_row_divisor(kGe2dBytesPerRowDivisor);
  stream_constraints.set_contiguous(true);
  stream_constraints.AddImageFormat(kGdcFRWidth, kGdcFRHeight, kFramePixelFormat);
  stream_constraints.set_buffer_count_for_camping(kVideoConferencingGE2DInputBuffers);
  return stream_constraints.MakeBufferCollectionConstraints();
}

static InternalConfigNode Ge2d(bool extended_fov) {
  InternalConfigNode result;
  result.type = kGe2d;
  result.output_frame_rate = {
      .frames_per_sec_numerator = kVideoThrottledOutputFrameRate,
      .frames_per_sec_denominator = 1,
  };
  result.supported_streams = {
      {
          .type = extended_fov ? kVideoStreamType | fuchsia::camera2::CameraStreamType::EXTENDED_FOV
                               : kVideoStreamType,
          .supports_dynamic_resolution = true,
          .supports_crop_region = true,
      },
  };
  result.child_nodes = MakeVec(OutputVideoConferencing(extended_fov));
  result.ge2d_info = [] {
    Ge2DInfo result;
    result.config_type = Ge2DConfig::GE2D_RESIZE;
    result.resize = {
        .crop =
            {
                .x = 0,
                .y = 0,
                .width = kGdcFRWidth,
                .height = kGdcFRHeight,
            },
        .output_rotation = GE2D_ROTATION_ROTATION_0,
    };
    return result;
  }();
  result.input_constraints = Ge2dConstraints();
  result.output_constraints =
      V2CopyFromV1BufferCollectionConstraints(VideoConfig(extended_fov).constraints);
  result.image_formats = VideoImageFormats();
  return result;
}

fuchsia::sysmem2::BufferCollectionConstraints GdcVideo1InputConstraints() {
  StreamConstraints stream_constraints;
  stream_constraints.set_bytes_per_row_divisor(kGdcBytesPerRowDivisor);
  stream_constraints.set_contiguous(true);
  stream_constraints.AddImageFormat(kIspFRWidth, kIspFRHeight, kFramePixelFormat);
  stream_constraints.set_buffer_count_for_camping(kVideoConferencingGDC1InputBuffers);
  return stream_constraints.MakeBufferCollectionConstraints();
}

fuchsia::sysmem2::BufferCollectionConstraints GdcVideo1OutputConstraints() {
  StreamConstraints stream_constraints;
  stream_constraints.set_bytes_per_row_divisor(kGdcBytesPerRowDivisor);
  stream_constraints.set_contiguous(true);
  stream_constraints.AddImageFormat(kGdcFRWidth, kGdcFRHeight, kFramePixelFormat);
  stream_constraints.set_buffer_count_for_camping(kVideoConferencingGDC1OutputBuffers);
  return stream_constraints.MakeBufferCollectionConstraints();
}

static std::vector<fuchsia::images2::ImageFormat> GdcVideo1ImageFormats() {
  return MakeVec(StreamConstraints::MakeImageFormat(kGdcFRWidth, kGdcFRHeight, kFramePixelFormat));
}

static InternalConfigNode GdcVideo1(bool extended_fov) {
  InternalConfigNode result;

  auto gdc_config = GdcConfig::VIDEO_CONFERENCE;
  if (extended_fov) {
    gdc_config = GdcConfig::VIDEO_CONFERENCE_EXTENDED_FOV;
  }

  result.type = kGdc;
  result.output_frame_rate = {
      .frames_per_sec_numerator = kVideoThrottledOutputFrameRate,
      .frames_per_sec_denominator = 1,
  };
  result.supported_streams = {
      {
          .type = extended_fov ? kMlStreamType | fuchsia::camera2::CameraStreamType::EXTENDED_FOV
                               : kMlStreamType,
          .supports_dynamic_resolution = false,
          .supports_crop_region = false,

      },
      {
          .type = extended_fov ? kVideoStreamType | fuchsia::camera2::CameraStreamType::EXTENDED_FOV
                               : kVideoStreamType,
          .supports_dynamic_resolution = false,
          .supports_crop_region = false,
      },
  };
  result.child_nodes = MakeVec(GdcVideo2(extended_fov), Ge2d(extended_fov));
  result.gdc_info = {
      .config_type =
          {
              gdc_config,
          },
  };
  result.input_constraints = GdcVideo1InputConstraints();
  result.output_constraints = GdcVideo1OutputConstraints();
  result.image_formats = GdcVideo1ImageFormats();

  return result;
}

fuchsia::sysmem2::BufferCollectionConstraints VideoConfigFullResConstraints() {
  StreamConstraints stream_constraints;
  stream_constraints.set_bytes_per_row_divisor(kIspBytesPerRowDivisor);
  stream_constraints.set_contiguous(true);
  stream_constraints.AddImageFormat(kIspFRWidth, kIspFRHeight, kFramePixelFormat);
  stream_constraints.set_buffer_count_for_camping(kVideoConferencingIspFROutputBuffers);
  return stream_constraints.MakeBufferCollectionConstraints();
}

static std::vector<fuchsia::images2::ImageFormat> IspImageFormats() {
  return MakeVec(StreamConstraints::MakeImageFormat(kIspFRWidth, kIspFRHeight, kFramePixelFormat));
}

InternalConfigNode VideoConfigFullRes(bool extended_fov) {
  InternalConfigNode result;
  result.type = kInputStream;
  result.output_frame_rate = {
      .frames_per_sec_numerator = kSensorMaxFramesPerSecond,
      .frames_per_sec_denominator = 1,
  };
  result.input_stream_type = fuchsia::camera2::CameraStreamType::FULL_RESOLUTION;
  result.supported_streams = {
      {
          .type = extended_fov ? kMlStreamType | fuchsia::camera2::CameraStreamType::EXTENDED_FOV
                               : kMlStreamType,
          .supports_dynamic_resolution = false,
          .supports_crop_region = false,
      },
      {
          .type = extended_fov ? kVideoStreamType | fuchsia::camera2::CameraStreamType::EXTENDED_FOV
                               : kVideoStreamType,
          .supports_dynamic_resolution = false,
          .supports_crop_region = false,
      },
  };
  result.child_nodes = MakeVec(GdcVideo1(extended_fov));
  result.output_constraints = VideoConfigFullResConstraints();
  result.image_formats = IspImageFormats();
  return result;
}

}  // namespace camera

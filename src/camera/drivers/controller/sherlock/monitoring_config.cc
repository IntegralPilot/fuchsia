// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/camera/drivers/controller/sherlock/monitoring_config.h"

// This file contains static information for the Monitor Configuration
// There are three streams in one configuration
// FR --> OutputStreamMLFR (Directly from ISP) (10fps)
// FR --> GDC1 --> OutputStreamMLDS
// DS --> GDC2 --> GE2D --> OutputStreamMonitoring

namespace camera {

static InternalConfigNode Gdc1();
static InternalConfigNode Ge2dMonitoring();
static InternalConfigNode MLFRFrameSkipper();

/**********************************
 * Output Stream ML FR parameters *
 **********************************
 */

static std::vector<fuchsia::images2::ImageFormat> OutputStreamMLFRImageFormats() {
  std::vector<fuchsia::images2::ImageFormat> result;
  result.emplace_back(StreamConstraints::MakeImageFormat(
      kOutputStreamMlFRWidth, kOutputStreamMlFRHeight, kOutputStreamMlFRPixelFormat));
  return result;
}

static fuchsia::camera2::hal::StreamConfig OutputStreamMLFRConfig() {
  StreamConstraints stream(fuchsia::camera2::CameraStreamType::FULL_RESOLUTION |
                           fuchsia::camera2::CameraStreamType::MACHINE_LEARNING);
  stream.AddImageFormat(kOutputStreamMlFRWidth, kOutputStreamMlFRHeight,
                        kOutputStreamMlFRPixelFormat);
  stream.set_bytes_per_row_divisor(kIspBytesPerRowDivisor);
  stream.set_contiguous(true);
  stream.set_frames_per_second(kOutputStreamMlFRFrameRate);
  // TODO(https://fxbug.dev/42182036): Support releasing initial participants' camping buffer
  // counts.
  stream.set_buffer_count_for_camping(kMonitoringIspFROutputBuffers + kMonitoringGDC1InputBuffers);
  stream.set_min_buffer_count(kMonitoringIspFROutputBuffers + kMonitoringGDC1InputBuffers +
                              kOutputStreamMlFRMaxClientBuffers);
  return stream.ConvertToStreamConfig();
}

/***********************************
 * Output Stream ML DS parameters  *
 ***********************************
 */

static std::vector<fuchsia::images2::ImageFormat> OutputStreamMLDSImageFormats() {
  std::vector<fuchsia::images2::ImageFormat> result;
  result.emplace_back(StreamConstraints::MakeImageFormat(
      kOutputStreamMlDSWidth, kOutputStreamMlDSHeight, kOutputStreamMlDSPixelFormat));
  return result;
}

static fuchsia::camera2::hal::StreamConfig OutputStreamMLDSConfig() {
  StreamConstraints stream(fuchsia::camera2::CameraStreamType::DOWNSCALED_RESOLUTION |
                           fuchsia::camera2::CameraStreamType::MACHINE_LEARNING);
  stream.AddImageFormat(kOutputStreamMlDSWidth, kOutputStreamMlDSHeight,
                        kOutputStreamMlFRPixelFormat);
  stream.set_bytes_per_row_divisor(kGdcBytesPerRowDivisor);
  stream.set_contiguous(true);
  stream.set_frames_per_second(kOutputStreamMlDSFrameRate);
  stream.set_buffer_count_for_camping(kMonitoringGDC1OutputBuffers);
  stream.set_min_buffer_count(kMonitoringGDC1OutputBuffers + kOutputStreamMlDSMaxClientBuffers);
  return stream.ConvertToStreamConfig();
}

/******************************************
 * Output Stream DS Monitoring parameters *
 ******************************************
 */

static std::vector<fuchsia::images2::ImageFormat> MonitorConfigDownScaledResImageFormats() {
  std::vector<fuchsia::images2::ImageFormat> result;
  result.emplace_back(StreamConstraints::MakeImageFormat(
      kOutputStreamDSWidth, kOutputStreamDSHeight, kOutputStreamMonitoringPixelFormat));
  return result;
}

static std::vector<fuchsia::images2::ImageFormat> OutputStreamMonitoringImageFormats() {
  std::vector<fuchsia::images2::ImageFormat> result;
  result.emplace_back(StreamConstraints::MakeImageFormat(kOutputStreamMonitoringWidth,
                                                         kOutputStreamMonitoringHeight,
                                                         kOutputStreamMonitoringPixelFormat));
  result.emplace_back(StreamConstraints::MakeImageFormat(kOutputStreamMonitoringWidth1,
                                                         kOutputStreamMonitoringHeight1,
                                                         kOutputStreamMonitoringPixelFormat));
  result.emplace_back(StreamConstraints::MakeImageFormat(kOutputStreamMonitoringWidth2,
                                                         kOutputStreamMonitoringHeight2,
                                                         kOutputStreamMonitoringPixelFormat));
  return result;
}

static fuchsia::camera2::hal::StreamConfig OutputStreamMonitoringConfig() {
  StreamConstraints stream(fuchsia::camera2::CameraStreamType::MONITORING);
  stream.AddImageFormat(kOutputStreamMonitoringWidth, kOutputStreamMonitoringHeight,
                        kOutputStreamMonitoringPixelFormat);
  stream.AddImageFormat(kOutputStreamMonitoringWidth1, kOutputStreamMonitoringHeight1,
                        kOutputStreamMonitoringPixelFormat);
  stream.AddImageFormat(kOutputStreamMonitoringWidth2, kOutputStreamMonitoringHeight2,
                        kOutputStreamMonitoringPixelFormat);
  stream.set_bytes_per_row_divisor(kGe2dBytesPerRowDivisor);
  stream.set_contiguous(true);
  stream.set_frames_per_second(kMonitoringThrottledOutputFrameRate);
  stream.set_buffer_count_for_camping(kMonitoringGDC2OutputBuffers + kMonitoringGE2DInputBuffers);
  stream.set_min_buffer_count(kMonitoringGDC2OutputBuffers + kMonitoringGE2DInputBuffers +
                              kOutputStreamMonitoringMaxClientBuffers);
  return stream.ConvertToStreamConfig();
}

/*****************************
 *  EXTERNAL CONFIGURATIONS  *
 *****************************
 */

fuchsia::camera2::hal::Config MonitoringConfig() {
  fuchsia::camera2::hal::Config config;
  config.stream_configs.push_back(OutputStreamMLFRConfig());
  config.stream_configs.push_back(OutputStreamMLDSConfig());
  config.stream_configs.push_back(OutputStreamMonitoringConfig());
  return config;
}

// ================== INTERNAL CONFIGURATION ======================== //
// FR --> OutputStreamMLFR (Directly from ISP) (10fps)
// FR --> GDC1 --> OutputStreamMLDS (10fps)

static InternalConfigNode OutputStreamMLFR() {
  InternalConfigNode result;
  result.type = kOutputStream;
  result.output_frame_rate = {
      .frames_per_sec_numerator = kOutputStreamMlFRFrameRate,
      .frames_per_sec_denominator = 1,
  };
  result.supported_streams = {
      {
          .type = fuchsia::camera2::CameraStreamType::FULL_RESOLUTION |
                  fuchsia::camera2::CameraStreamType::MACHINE_LEARNING,
          .supports_dynamic_resolution = false,
          .supports_crop_region = false,
      },
  };
  result.input_constraints = CopyConstraintsWithOverrides(
      MonitoringConfig().stream_configs[0].constraints, {.min_buffer_count_for_camping = 0});
  result.image_formats = OutputStreamMLFRImageFormats();
  return result;
}

static InternalConfigNode OutputStreamMLDS() {
  InternalConfigNode result;
  result.type = kOutputStream;
  result.output_frame_rate = {
      .frames_per_sec_numerator = kOutputStreamMlDSFrameRate,
      .frames_per_sec_denominator = 1,
  };
  result.supported_streams = {
      {
          .type = fuchsia::camera2::CameraStreamType::DOWNSCALED_RESOLUTION |
                  fuchsia::camera2::CameraStreamType::MACHINE_LEARNING,
          .supports_dynamic_resolution = false,
          .supports_crop_region = false,
      },
  };
  result.input_constraints = CopyConstraintsWithOverrides(
      MonitoringConfig().stream_configs[1].constraints, {.min_buffer_count_for_camping = 0});
  result.image_formats = OutputStreamMLDSImageFormats();
  return result;
}

fuchsia::sysmem2::BufferCollectionConstraints Gdc1Constraints() {
  StreamConstraints stream_constraints;
  stream_constraints.set_bytes_per_row_divisor(kGdcBytesPerRowDivisor);
  stream_constraints.set_contiguous(true);
  stream_constraints.AddImageFormat(kOutputStreamMlFRWidth, kOutputStreamMlFRHeight,
                                    kOutputStreamMlFRPixelFormat);
  stream_constraints.set_buffer_count_for_camping(kMonitoringGDC1InputBuffers);
  return stream_constraints.MakeBufferCollectionConstraints();
}

static InternalConfigNode Gdc1() {
  InternalConfigNode result;
  result.type = kGdc;
  result.output_frame_rate = {
      .frames_per_sec_numerator = kOutputStreamMlDSFrameRate,
      .frames_per_sec_denominator = 1,
  };
  result.supported_streams = {
      {
          .type = fuchsia::camera2::CameraStreamType::DOWNSCALED_RESOLUTION |
                  fuchsia::camera2::CameraStreamType::MACHINE_LEARNING,
          .supports_dynamic_resolution = false,
          .supports_crop_region = false,
      },
  };
  result.child_nodes = MakeVec(OutputStreamMLDS());
  result.gdc_info = {
      .config_type =
          {
              GdcConfig::MONITORING_ML,
          },
  };
  result.input_constraints = Gdc1Constraints();
  result.output_constraints =
      CopyConstraintsWithOverrides(OutputStreamMLDSConfig().constraints,
                                   {.min_buffer_count_for_camping = kMonitoringGDC1OutputBuffers});
  result.image_formats = OutputStreamMLDSImageFormats();
  return result;
}

fuchsia::sysmem2::BufferCollectionConstraints MonitorConfigFullResConstraints() {
  StreamConstraints stream_constraints;
  stream_constraints.set_bytes_per_row_divisor(kIspBytesPerRowDivisor);
  stream_constraints.set_contiguous(true);
  stream_constraints.AddImageFormat(kOutputStreamMlFRWidth, kOutputStreamMlFRHeight,
                                    kOutputStreamMlFRPixelFormat);
  stream_constraints.set_buffer_count_for_camping(kMonitoringIspFROutputBuffers);
  return stream_constraints.MakeBufferCollectionConstraints();
}

InternalConfigNode MonitorConfigFullRes() {
  InternalConfigNode result;
  result.type = kInputStream;
  result.output_frame_rate = {
      .frames_per_sec_numerator = kSensorMaxFramesPerSecond,
      .frames_per_sec_denominator = 1,
  };
  result.input_stream_type = fuchsia::camera2::CameraStreamType::FULL_RESOLUTION;
  result.supported_streams = {
      {
          .type = fuchsia::camera2::CameraStreamType::FULL_RESOLUTION |
                  fuchsia::camera2::CameraStreamType::MACHINE_LEARNING,
          .supports_dynamic_resolution = false,
          .supports_crop_region = false,
      },
      {
          .type = fuchsia::camera2::CameraStreamType::DOWNSCALED_RESOLUTION |
                  fuchsia::camera2::CameraStreamType::MACHINE_LEARNING,
          .supports_dynamic_resolution = false,
          .supports_crop_region = false,
      },
  };
  result.child_nodes = MakeVec(MLFRFrameSkipper());
  // input constrains not applicable
  result.output_constraints = MonitorConfigFullResConstraints();
  result.image_formats = OutputStreamMLFRImageFormats();
  return result;
}

static InternalConfigNode MLFRFrameSkipper() {
  // This node serves to synchronize the skipped frames between the two reduced-fps downstream
  // nodes.
  static_assert(kOutputStreamMlFRFrameRate == kOutputStreamMlDSFrameRate);
  InternalConfigNode result;
  result.type = kPassthrough;
  result.output_frame_rate = {
      .frames_per_sec_numerator = kOutputStreamMlFRFrameRate,
      .frames_per_sec_denominator = 1,
  };
  result.input_stream_type = fuchsia::camera2::CameraStreamType::FULL_RESOLUTION;
  result.supported_streams = {
      {
          .type = fuchsia::camera2::CameraStreamType::FULL_RESOLUTION |
                  fuchsia::camera2::CameraStreamType::MACHINE_LEARNING,
          .supports_dynamic_resolution = false,
          .supports_crop_region = false,
      },
      {
          .type = fuchsia::camera2::CameraStreamType::DOWNSCALED_RESOLUTION |
                  fuchsia::camera2::CameraStreamType::MACHINE_LEARNING,
          .supports_dynamic_resolution = false,
          .supports_crop_region = false,
      },
  };
  result.child_nodes = MakeVec(OutputStreamMLFR(), Gdc1());
  return result;
}

// DS --> GDC2 --> GE2D --> OutputStreamMonitoring

static InternalConfigNode OutputStreamMonitoring() {
  InternalConfigNode result;
  result.type = kOutputStream;
  result.output_frame_rate = {
      .frames_per_sec_numerator = kMonitoringThrottledOutputFrameRate,
      .frames_per_sec_denominator = 1,
  };
  result.supported_streams = {
      {
          .type = fuchsia::camera2::CameraStreamType::MONITORING,
          .supports_dynamic_resolution = false,
          .supports_crop_region = false,
      },
  };
  result.input_constraints = CopyConstraintsWithOverrides(
      MonitoringConfig().stream_configs[2].constraints, {.min_buffer_count_for_camping = 0});
  result.image_formats = OutputStreamMonitoringImageFormats();
  return result;
}

fuchsia::sysmem2::BufferCollectionConstraints Gdc2Constraints() {
  StreamConstraints stream_constraints;
  stream_constraints.set_bytes_per_row_divisor(kGdcBytesPerRowDivisor);
  stream_constraints.set_contiguous(true);
  stream_constraints.AddImageFormat(kOutputStreamDSWidth, kOutputStreamDSHeight,
                                    kOutputStreamMonitoringPixelFormat);
  stream_constraints.set_buffer_count_for_camping(kMonitoringGDC2InputBuffers);
  return stream_constraints.MakeBufferCollectionConstraints();
}

fuchsia::sysmem2::BufferCollectionConstraints Ge2dMonitoringConstraints() {
  StreamConstraints stream_constraints;
  stream_constraints.set_bytes_per_row_divisor(kGe2dBytesPerRowDivisor);
  stream_constraints.set_contiguous(true);
  stream_constraints.AddImageFormat(kOutputStreamMonitoringWidth, kOutputStreamMonitoringHeight,
                                    kOutputStreamMonitoringPixelFormat);
  stream_constraints.AddImageFormat(kOutputStreamMonitoringWidth1, kOutputStreamMonitoringHeight1,
                                    kOutputStreamMonitoringPixelFormat);
  stream_constraints.AddImageFormat(kOutputStreamMonitoringWidth2, kOutputStreamMonitoringHeight2,
                                    kOutputStreamMonitoringPixelFormat);
  stream_constraints.set_buffer_count_for_camping(kMonitoringGE2DInputBuffers);
  return stream_constraints.MakeBufferCollectionConstraints();
}

static InternalConfigNode Ge2dMonitoring() {
  InternalConfigNode result;
  result.type = kGe2d;
  result.output_frame_rate = {
      .frames_per_sec_numerator = kMonitoringThrottledOutputFrameRate,
      .frames_per_sec_denominator = 1,
  };
  result.supported_streams = {
      {
          .type = fuchsia::camera2::CameraStreamType::MONITORING,
          .supports_dynamic_resolution = false,
          .supports_crop_region = false,
      },
  };
  result.child_nodes = MakeVec(OutputStreamMonitoring());
  result.ge2d_info = [] {
    Ge2DInfo result;
    result.config_type = Ge2DConfig::GE2D_WATERMARK,
    result.watermark = MakeVec(
        [] {
          WatermarkInfo result;
          result.filename = "watermark-720p.rgba";
          result.image_format = StreamConstraints::MakeImageFormat(
              kWatermark720pWidth, kWatermark720pHeight, kWatermarkPixelFormat);
          result.loc_x = kOutputStreamMonitoringWidth - kWatermark720pWidth;
          result.loc_y = 0;
          return result;
        }(),
        [] {
          WatermarkInfo result;
          result.filename = "watermark-480p.rgba";
          result.image_format = StreamConstraints::MakeImageFormat(
              kWatermark480pWidth, kWatermark480pHeight, kWatermarkPixelFormat);
          result.loc_x = kOutputStreamMonitoringWidth1 - kWatermark480pWidth;
          result.loc_y = 0;
          return result;
        }(),
        [] {
          WatermarkInfo result;
          result.filename = "watermark-360p.rgba";
          result.image_format = StreamConstraints::MakeImageFormat(
              kWatermark360pWidth, kWatermark360pHeight, kWatermarkPixelFormat);
          result.loc_x = kOutputStreamMonitoringWidth2 - kWatermark360pWidth;
          result.loc_y = 0;
          return result;
        }());
    return result;
  }();
  // Ge2dMonitoringConstraints uses kMonitoringGE2DInputBuffers, so modification is necessary to
  // reflect the output constraints.
  result.input_constraints = Ge2dMonitoringConstraints();
  result.image_formats = OutputStreamMonitoringImageFormats();
  return result;
}

fuchsia::sysmem2::BufferCollectionConstraints Gdc2OutputConstraints() {
  StreamConstraints stream_constraints;
  stream_constraints.set_bytes_per_row_divisor(kGdcBytesPerRowDivisor);
  stream_constraints.set_contiguous(true);
  stream_constraints.AddImageFormat(kOutputStreamMonitoringWidth, kOutputStreamMonitoringHeight,
                                    kOutputStreamMonitoringPixelFormat);
  stream_constraints.AddImageFormat(kOutputStreamMonitoringWidth1, kOutputStreamMonitoringHeight1,
                                    kOutputStreamMonitoringPixelFormat);
  stream_constraints.AddImageFormat(kOutputStreamMonitoringWidth2, kOutputStreamMonitoringHeight2,
                                    kOutputStreamMonitoringPixelFormat);
  stream_constraints.set_buffer_count_for_camping(kMonitoringGDC2OutputBuffers);
  return stream_constraints.MakeBufferCollectionConstraints();
}

static InternalConfigNode Gdc2() {
  InternalConfigNode result;
  result.type = kGdc;
  result.output_frame_rate = {
      .frames_per_sec_numerator = kMonitoringThrottledOutputFrameRate,
      .frames_per_sec_denominator = 1,
  };
  result.supported_streams = {
      {
          .type = fuchsia::camera2::CameraStreamType::MONITORING,
          .supports_dynamic_resolution = true,
          .supports_crop_region = false,
      },
  };
  result.child_nodes = MakeVec(Ge2dMonitoring());
  result.gdc_info = {
      .config_type =
          {
              GdcConfig::MONITORING_720p,
              GdcConfig::MONITORING_480p,
              GdcConfig::MONITORING_360p,
          },
  };
  result.input_constraints = Gdc2Constraints();
  result.output_constraints = Gdc2OutputConstraints();
  result.image_formats = OutputStreamMonitoringImageFormats();
  return result;
}

fuchsia::sysmem2::BufferCollectionConstraints MonitorConfigDownScaledResConstraints() {
  StreamConstraints stream_constraints;
  stream_constraints.set_bytes_per_row_divisor(kIspBytesPerRowDivisor);
  stream_constraints.set_contiguous(true);
  stream_constraints.AddImageFormat(kOutputStreamDSWidth, kOutputStreamDSHeight,
                                    kOutputStreamMonitoringPixelFormat);
  stream_constraints.set_buffer_count_for_camping(kMonitoringIspDSOutputBuffers);
  return stream_constraints.MakeBufferCollectionConstraints();
}

InternalConfigNode MonitorConfigDownScaledRes() {
  InternalConfigNode result;
  result.type = kInputStream;
  result.output_frame_rate = {
      .frames_per_sec_numerator = kSensorMaxFramesPerSecond,
      .frames_per_sec_denominator = 1,
  };
  result.input_stream_type = fuchsia::camera2::CameraStreamType::DOWNSCALED_RESOLUTION;
  result.supported_streams = {
      {
          .type = fuchsia::camera2::CameraStreamType::MONITORING,
          .supports_dynamic_resolution = false,
          .supports_crop_region = false,
      },
  };
  result.child_nodes = MakeVec(Gdc2());
  // input constraints not applicable
  result.output_constraints = MonitorConfigDownScaledResConstraints();
  result.image_formats = MonitorConfigDownScaledResImageFormats();
  return result;
}

}  // namespace camera

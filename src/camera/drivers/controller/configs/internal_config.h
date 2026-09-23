// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CAMERA_DRIVERS_CONTROLLER_CONFIGS_INTERNAL_CONFIG_H_
#define SRC_CAMERA_DRIVERS_CONTROLLER_CONFIGS_INTERNAL_CONFIG_H_

#include <fuchsia/camera2/hal/cpp/fidl.h>
#include <fuchsia/hardware/ge2d/cpp/banjo.h>
#include <fuchsia/sysmem/cpp/fidl.h>

#include <vector>

namespace camera {

enum GdcConfig {
  MONITORING_360p = 0,
  MONITORING_480p = 1,
  MONITORING_720p = 2,
  MONITORING_ML = 3,
  VIDEO_CONFERENCE = 4,
  VIDEO_CONFERENCE_EXTENDED_FOV = 5,
  VIDEO_CONFERENCE_ML = 6,
  INVALID = 7,
};

enum Ge2DConfig {
  GE2D_WATERMARK = 0,
  GE2D_RESIZE = 1,
  GE2D_COUNT = 2,
};
struct GdcInfo {
  std::vector<GdcConfig> config_type{};
};

struct WatermarkInfo {
  WatermarkInfo() = default;
  WatermarkInfo(const WatermarkInfo& to_copy) {
    filename = to_copy.filename;
    image_format = fidl::Clone(to_copy.image_format);
    loc_x = to_copy.loc_x;
    loc_y = to_copy.loc_y;
  }
  WatermarkInfo& operator=(const WatermarkInfo& to_copy) {
    filename = to_copy.filename;
    image_format = fidl::Clone(to_copy.image_format);
    loc_x = to_copy.loc_x;
    loc_y = to_copy.loc_y;
    return *this;
  }
  WatermarkInfo(WatermarkInfo&& to_move) = default;
  WatermarkInfo& operator=(WatermarkInfo&& to_move) = default;

  const char* filename{};
  fuchsia::images2::ImageFormat image_format{};
  uint32_t loc_x{};
  uint32_t loc_y{};
};

struct StreamInfo {
  fuchsia::camera2::CameraStreamType type{};
  bool supports_dynamic_resolution{};
  bool supports_crop_region{};
};

struct Ge2DInfo {
  Ge2DInfo() = default;
  Ge2DInfo(const Ge2DInfo& to_copy) {
    config_type = to_copy.config_type;
    watermark = to_copy.watermark;
    resize = to_copy.resize;
  }
  Ge2DInfo& operator=(const Ge2DInfo& to_copy) {
    config_type = to_copy.config_type;
    watermark = to_copy.watermark;
    resize = to_copy.resize;
    return *this;
  }
  Ge2DInfo(Ge2DInfo&& to_move) = default;
  Ge2DInfo& operator=(Ge2DInfo&& to_move) = default;

  Ge2DConfig config_type{};
  std::vector<WatermarkInfo> watermark{};
  resize_info resize{};
};

enum NodeType {
  kInputStream,
  kGdc,
  kGe2d,
  kOutputStream,
  kPassthrough,
};
struct InternalConfigNode {
  InternalConfigNode() = default;
  InternalConfigNode(const InternalConfigNode& to_copy) {
    type = to_copy.type;
    output_frame_rate = to_copy.output_frame_rate;
    input_stream_type = to_copy.input_stream_type;
    supported_streams = to_copy.supported_streams;
    child_nodes = to_copy.child_nodes;
    ge2d_info = to_copy.ge2d_info;
    gdc_info = to_copy.gdc_info;
    if (to_copy.input_constraints.has_value()) {
      input_constraints = fidl::Clone(*to_copy.input_constraints);
    }
    if (to_copy.output_constraints.has_value()) {
      output_constraints = fidl::Clone(*to_copy.output_constraints);
    }
    image_formats = fidl::Clone(to_copy.image_formats);
  }
  InternalConfigNode& operator=(const InternalConfigNode& to_copy) {
    type = to_copy.type;
    output_frame_rate = to_copy.output_frame_rate;
    input_stream_type = to_copy.input_stream_type;
    supported_streams = to_copy.supported_streams;
    child_nodes = to_copy.child_nodes;
    ge2d_info = to_copy.ge2d_info;
    gdc_info = to_copy.gdc_info;
    if (to_copy.input_constraints.has_value()) {
      input_constraints = fidl::Clone(*to_copy.input_constraints);
    }
    if (to_copy.output_constraints.has_value()) {
      output_constraints = fidl::Clone(*to_copy.output_constraints);
    }
    image_formats = fidl::Clone(to_copy.image_formats);
    return *this;
  }
  InternalConfigNode(InternalConfigNode&& to_move) = default;
  InternalConfigNode& operator=(InternalConfigNode&& to_move) = default;

  // To identify the type of the node this is.
  NodeType type{};
  // To identify the input frame rate at this node.
  fuchsia::camera2::FrameRate output_frame_rate{};
  // This is only valid for Input Stream Type to differentiate
  // between ISP FR/DS/Scalar streams.
  fuchsia::camera2::CameraStreamType input_stream_type{};
  // Types of |stream_types| supported by this node.
  std::vector<StreamInfo> supported_streams{};
  // Child nodes
  std::vector<InternalConfigNode> child_nodes{};
  // HWAccelerator Info if applicable.
  Ge2DInfo ge2d_info{};
  GdcInfo gdc_info{};
  // Specifies constraints for the node's attachments. Input constraints are required for all nodes
  // except Input nodes. If specified, output constraints indicate the node owns a buffer collection
  // and populates it by processing content from the input collection. Otherwise, the node only uses
  // the input collection, and does not have a collection of its own.
  std::optional<fuchsia::sysmem2::BufferCollectionConstraints> input_constraints{};
  std::optional<fuchsia::sysmem2::BufferCollectionConstraints> output_constraints{};
  // Image formats supported
  std::vector<fuchsia::images2::ImageFormat> image_formats{};
};

struct FrameRateRange {
  fuchsia::camera2::FrameRate min{};
  fuchsia::camera2::FrameRate max{};
};

struct InternalConfigInfo {
  // List of all the streams part of this configuration
  // These streams are high level streams which are coming in from
  // the ISP and not the output streams which are provided to the clients.
  // That information is part of this |InternalConfigNode| in |supported_streams|
  std::vector<InternalConfigNode> streams_info{};
  // The frame rate range supported by this configuration.
  FrameRateRange frame_rate_range{};
};

struct InternalConfigs {
  // List of all the configurations supported on a particular platform
  // Order of the configuration needs to match with the external configuration
  // data.
  std::vector<InternalConfigInfo> configs_info{};
};

}  // namespace camera

#endif  // SRC_CAMERA_DRIVERS_CONTROLLER_CONFIGS_INTERNAL_CONFIG_H_

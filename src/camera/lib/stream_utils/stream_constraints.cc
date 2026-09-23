// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "stream_constraints.h"

#include <fidl/fuchsia.sysmem/cpp/hlcpp_conversion.h>
#include <fidl/fuchsia.sysmem2/cpp/hlcpp_conversion.h>
#include <fuchsia/camera2/cpp/fidl.h>
#include <fuchsia/camera2/hal/cpp/fidl.h>
#include <lib/affine/ratio.h>
#include <lib/image-format/image_format.h>
#include <lib/sysmem-version/sysmem-version.h>

#include <fbl/algorithm.h>

namespace camera {

// Make an ImageFormat_2 struct with default values except for width, height and format.
fuchsia::images2::ImageFormat StreamConstraints::MakeImageFormat(
    uint32_t width, uint32_t height, fuchsia::images2::PixelFormat pixel_format,
    uint32_t original_width, uint32_t original_height) {
  PixelFormatAndModifier pixel_format_and_modifier(fidl::HLCPPToNatural(pixel_format),
                                                   fuchsia_images2::PixelFormatModifier::kLinear);
  uint32_t bytes_per_row = ImageFormatStrideBytesPerWidthPixel(pixel_format_and_modifier) * width;

  // All four dimensions must be non-zero to generate a valid aspect ratio.
  bool has_pixel_aspect_ratio = width && height && original_width && original_height;
  uint64_t pixel_aspect_ratio_width = 1;
  uint64_t pixel_aspect_ratio_height = 1;
  if (width == 0 || height == 0) {
    width = 1;
    height = 1;
  }
  if (has_pixel_aspect_ratio) {
    // Calculate the reduced fraction for the rational value (original_ratio / format_ratio).
    // Equivalent to (original_width / original_height) / (width / height)
    //             = (original_width * height) / (original_height * width)
    pixel_aspect_ratio_width = static_cast<uint64_t>(original_width) * height;
    pixel_aspect_ratio_height = static_cast<uint64_t>(original_height) * width;
    affine::Ratio::Reduce(&pixel_aspect_ratio_width, &pixel_aspect_ratio_height);
    // Round and truncate values that are still too large to fit in the format struct.
    while (pixel_aspect_ratio_width > std::numeric_limits<uint32_t>::max() ||
           pixel_aspect_ratio_height > std::numeric_limits<uint32_t>::max()) {
      pixel_aspect_ratio_width = (pixel_aspect_ratio_width + (1u << 31)) >> 1;
      pixel_aspect_ratio_height = (pixel_aspect_ratio_height + (1u << 31)) >> 1;
    }
  }
  fuchsia::images2::ImageFormat format;
  format.set_pixel_format(pixel_format);
  format.set_size(fuchsia::math::SizeU{.width = width, .height = height});
  format.set_bytes_per_row(bytes_per_row);
  format.set_display_rect(fuchsia::math::RectU{.x = 0, .y = 0, .width = width, .height = height});
  format.set_color_space(fuchsia::images2::ColorSpace::REC601_PAL);
  format.set_pixel_aspect_ratio(
      fuchsia::math::SizeU{.width = static_cast<uint32_t>(pixel_aspect_ratio_width),
                           .height = static_cast<uint32_t>(pixel_aspect_ratio_height)});
  return format;
}

void StreamConstraints::AddImageFormat(uint32_t width, uint32_t height,
                                       fuchsia::images2::PixelFormat format,
                                       uint32_t original_width, uint32_t original_height) {
  formats_.push_back(MakeImageFormat(width, height, format, original_width, original_height));
}

fuchsia::sysmem2::BufferCollectionConstraints StreamConstraints::MakeBufferCollectionConstraints()
    const {
  // Don't make a stream config if AddImageFormats has not been called.
  ZX_ASSERT(!formats_.empty());
  fuchsia::sysmem2::BufferCollectionConstraints constraints;
  constraints.set_min_buffer_count_for_camping(buffer_count_for_camping_);
  constraints.set_min_buffer_count(min_buffer_count_);
  auto& bmc = *constraints.mutable_buffer_memory_constraints();
  bmc.set_cpu_domain_supported(false);
  bmc.set_ram_domain_supported(true);
  if (contiguous_) {
    bmc.set_physically_contiguous_required(true);
  }
  // To allow for pinning
  auto& usage = *constraints.mutable_usage();
  usage.set_cpu(fuchsia::sysmem2::CPU_USAGE_WRITE);
  usage.set_video(fuchsia::sysmem2::VIDEO_USAGE_CAPTURE);

  // Fully constrain the image dimensions. This will make it more likely that a client's constraints
  // won't conflict with the cameras. Unspecified dimensions default to MIN/MAX which doesn't let
  // AttachToken clients resolve to the same constraints.
  uint32_t max_width = 0, max_height = 0;
  uint32_t min_width = std::numeric_limits<uint32_t>::max();
  uint32_t min_height = std::numeric_limits<uint32_t>::max();
  constexpr uint32_t kCodedHeightDivisor = 2;
  constexpr uint32_t kStartOffsetDivisor = 2;

  for (auto& format : formats_) {
    max_width = std::max(max_width, format.size().width);
    max_height = std::max(max_height, format.size().height);
    min_width = std::min(min_width, format.size().width);
    min_height = std::min(min_height, format.size().height);
  }
  auto& ifc = constraints.mutable_image_format_constraints()->emplace_back();
  ifc.set_pixel_format(fuchsia::images2::PixelFormat::NV12);
  ifc.mutable_color_spaces()->emplace_back(fuchsia::images2::ColorSpace::REC601_PAL);
  ifc.set_min_size(fuchsia::math::SizeU{.width = min_width, .height = min_height});
  ifc.set_max_size(fuchsia::math::SizeU{.width = max_width, .height = max_height});
  ifc.set_min_bytes_per_row(min_width);
  ifc.set_max_width_times_height(max_width * max_height);
  ifc.set_size_alignment(
      ::fuchsia::math::SizeU{.width = bytes_per_row_divisor_, .height = kCodedHeightDivisor});
  ifc.set_bytes_per_row_divisor(bytes_per_row_divisor_);
  ifc.set_start_offset_divisor(kStartOffsetDivisor);
  ifc.set_required_min_size(fuchsia::math::SizeU{.width = max_width, .height = max_height});
  ifc.set_required_max_size(fuchsia::math::SizeU{.width = max_width, .height = max_height});

  return constraints;
}

fuchsia::camera2::hal::StreamConfig StreamConstraints::ConvertToStreamConfig() {
  // Don't make a stream config if AddImageFormats has not been called.
  ZX_ASSERT(!formats_.empty());
  fuchsia::camera2::StreamProperties stream_properties{};
  stream_properties.set_stream_type(stream_type_);

  for (auto& format : formats_) {
    format.set_bytes_per_row(fbl::round_up(format.size().width, bytes_per_row_divisor_));
  }

  auto v1_constraints_natural_result = sysmem::V1CopyFromV2BufferCollectionConstraints(
      fidl::HLCPPToNatural(MakeBufferCollectionConstraints()));
  ZX_ASSERT(v1_constraints_natural_result.is_ok());
  auto v1_constraints_hlcpp =
      fidl::NaturalToHLCPP(std::move(*v1_constraints_natural_result.value()));

  std::vector<::fuchsia::sysmem::ImageFormat_2> v1_image_formats;
  for (auto& format : formats_) {
    auto v2_natural_format = fidl::HLCPPToNatural(format);
    auto v1_natural_format_result = sysmem::V1CopyFromV2ImageFormat(v2_natural_format);
    ZX_ASSERT(v1_natural_format_result.is_ok());
    auto v1_hlcpp_format = fidl::NaturalToHLCPP(std::move(v1_natural_format_result.value()));
    v1_image_formats.emplace_back(std::move(v1_hlcpp_format));
  }

  return {
      .frame_rate = {.frames_per_sec_numerator = frames_per_second_,
                     .frames_per_sec_denominator = 1},
      .constraints = std::move(v1_constraints_hlcpp),
      .properties = std::move(stream_properties),
      .image_formats = std::move(v1_image_formats),
  };
}

}  // namespace camera

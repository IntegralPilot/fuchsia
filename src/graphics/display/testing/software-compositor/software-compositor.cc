// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/testing/software-compositor/software-compositor.h"

#include <zircon/assert.h>

#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <span>

#include "src/graphics/display/testing/software-compositor/pixel.h"

namespace software_compositor {

namespace {

inline std::pair<std::span<uint8_t>::iterator, std::span<const uint8_t>::iterator> CopyPixel(
    std::span<uint8_t>::iterator canvas_pixel_it, PixelFormat canvas_pixel_format,
    std::span<const uint8_t>::iterator source_pixel_it, PixelFormat source_pixel_format) {
  if (canvas_pixel_format == source_pixel_format) {
    std::tie(canvas_pixel_it[0], canvas_pixel_it[1], canvas_pixel_it[2], canvas_pixel_it[3]) =
        std::tie(source_pixel_it[0], source_pixel_it[1], source_pixel_it[2], source_pixel_it[3]);
  } else if ((canvas_pixel_format == PixelFormat::kBgra8888 &&
              source_pixel_format == PixelFormat::kRgba8888) ||
             (canvas_pixel_format == PixelFormat::kRgba8888 &&
              source_pixel_format == PixelFormat::kBgra8888)) {
    std::tie(canvas_pixel_it[0], canvas_pixel_it[1], canvas_pixel_it[2], canvas_pixel_it[3]) =
        std::tie(source_pixel_it[2], source_pixel_it[1], source_pixel_it[0], source_pixel_it[3]);
  } else {
    // This should never happen.
    ZX_DEBUG_ASSERT_MSG(false, "not implemented");
  }
  return {canvas_pixel_it + GetBytesPerPixel(canvas_pixel_format),
          source_pixel_it + GetBytesPerPixel(source_pixel_format)};
}

}  // namespace

PixelData OutputImage::At(const Offset2D& offset) const {
  std::span<const uint8_t> data{reinterpret_cast<const uint8_t*>(buffer.data()),
                                static_cast<size_t>(properties.height) * properties.stride_bytes};
  const int bytes_per_pixel = GetBytesPerPixel(properties.pixel_format);
  return PixelData::FromRaw(buffer.subspan(
      offset.y * properties.stride_bytes + offset.x * bytes_per_pixel, bytes_per_pixel));
}

void OutputImage::SetPixelData(const Offset2D& offset, const PixelData& pixel) const {
  const int bytes_per_pixel = GetBytesPerPixel(properties.pixel_format);
  const auto color_to_write = buffer.subspan(
      offset.y * properties.stride_bytes + offset.x * bytes_per_pixel, bytes_per_pixel);
  std::copy(pixel.data.begin(), pixel.data.end(), color_to_write.begin());
}

PixelData InputImage::At(const Offset2D& offset) const {
  const int bytes_per_pixel = GetBytesPerPixel(properties.pixel_format);
  return PixelData::FromRaw(buffer.subspan(
      offset.y * properties.stride_bytes + offset.x * bytes_per_pixel, bytes_per_pixel));
}

SoftwareCompositor::SoftwareCompositor(const OutputImage& canvas) : canvas_(canvas) {}

void SoftwareCompositor::ClearCanvas(const PixelData& color, PixelFormat pixel_format) const {
  const PixelData color_to_fill = color.Convert(pixel_format, canvas_.properties.pixel_format);
  for (int row = 0; row < canvas_.properties.height; ++row) {
    std::span<uint8_t> bytes_to_fill = canvas_.buffer.subspan(row * canvas_.properties.stride_bytes,
                                                              canvas_.properties.stride_bytes);
    auto it = bytes_to_fill.begin();
    for (int column = 0; column < canvas_.properties.width; ++column) {
      it = std::copy(color_to_fill.data.begin(), color_to_fill.data.end(), it);
    }
  }
}

void SoftwareCompositor::CompositeImage(const InputImage& input_image,
                                        const CompositionProperties& composition_properties) const {
  ZX_ASSERT(!input_image.buffer.empty());

  // TODO(https://fxbug.dev/42075534): Currently alpha compositing is not supported.
  // Callers must guarantee that alpha blending is disabled.
  ZX_ASSERT(composition_properties.alpha_mode == ::display::AlphaMode::kDisable);

  // TODO(https://fxbug.dev/42075534): Currently image transformation is not supported.
  // Callers must guarantee that the image to draw is not rotated nor flipped.
  ZX_ASSERT(composition_properties.transform == display::CoordinateTransformation::kIdentity);

  const ::display::Rectangle& image_source = composition_properties.image_source;
  const ::display::Rectangle& canvas_destination = composition_properties.canvas_destination;

  // TODO(https://fxbug.dev/42075534): Currently this doesn't support clipping of the
  // input image. Callers must guarantee that the source frame starts at (0, 0)
  // and has the same size as the input image.
  ZX_ASSERT(image_source.x() == 0);
  ZX_ASSERT(image_source.y() == 0);
  ZX_ASSERT(image_source.width() == input_image.properties.width);
  ZX_ASSERT(image_source.height() == input_image.properties.height);

  // TODO(https://fxbug.dev/42075534): Currently this doesn't support scaling.
  // Callers must guarantee that the destination frame size is the same as the
  // original image size.
  ZX_ASSERT(canvas_destination.width() == input_image.properties.width);
  ZX_ASSERT(canvas_destination.height() == input_image.properties.height);

  // TODO(https://fxbug.dev/42075534): Currently this doesn't support clipping.
  // Callers must guarantee that the destination frame falls within the canvas.
  ZX_ASSERT(canvas_destination.x() + canvas_destination.width() <= canvas_.properties.width);
  ZX_ASSERT(canvas_destination.y() + canvas_destination.height() <= canvas_.properties.height);

  const int src_bytes_per_pixel = GetBytesPerPixel(input_image.properties.pixel_format);
  const int dst_bytes_per_pixel = GetBytesPerPixel(canvas_.properties.pixel_format);

  for (int row = 0; row < canvas_destination.height(); row++) {
    std::span<const uint8_t> source_row =
        input_image.buffer.subspan((row + image_source.y()) * input_image.properties.stride_bytes +
                                       image_source.x() * src_bytes_per_pixel,
                                   /*count=*/image_source.width() * src_bytes_per_pixel);
    std::span<uint8_t> canvas_row =
        canvas_.buffer.subspan((row + canvas_destination.y()) * canvas_.properties.stride_bytes +
                                   canvas_destination.x() * dst_bytes_per_pixel,
                               /*count=*/canvas_destination.width() * dst_bytes_per_pixel);

    auto source_row_it = source_row.begin();
    auto canvas_row_it = canvas_row.begin();
    for (int column = 0; column < canvas_destination.width(); column++) {
      std::tie(canvas_row_it, source_row_it) =
          CopyPixel(canvas_row_it, canvas_.properties.pixel_format, source_row_it,
                    input_image.properties.pixel_format);
    }
  }
}

void SoftwareCompositor::CompositeFallbackColor(
    const CompositionProperties& composition_properties) const {
  // TODO(https://fxbug.dev/42075534): Currently alpha compositing is not supported.
  // Callers must guarantee that alpha blending is disabled.
  ZX_ASSERT(composition_properties.alpha_mode == ::display::AlphaMode::kDisable);

  // TODO(https://fxbug.dev/42075534): Currently image transformation is not supported.
  // Callers must guarantee that the image to draw is not rotated nor flipped.
  ZX_ASSERT(composition_properties.transform == display::CoordinateTransformation::kIdentity);

  const ::display::Rectangle& canvas_destination = composition_properties.canvas_destination;

  // TODO(https://fxbug.dev/42075534): Currently this doesn't support clipping.
  // Callers must guarantee that the destination frame falls within the canvas.
  ZX_ASSERT(canvas_destination.y() + canvas_destination.height() <= canvas_.properties.height);
  ZX_ASSERT(canvas_destination.x() + canvas_destination.width() <= canvas_.properties.width);

  const PixelData color_to_fill =
      PixelData::FromRaw(composition_properties.fallback_color.bytes().subspan(0, 4))
          .Convert(ToPixelFormat(composition_properties.fallback_color.format().ToFidl()),
                   canvas_.properties.pixel_format);

  const int dst_bytes_per_pixel = GetBytesPerPixel(canvas_.properties.pixel_format);

  for (int row = 0; row < canvas_destination.height(); row++) {
    std::span<uint8_t> canvas_row =
        canvas_.buffer.subspan((row + canvas_destination.y()) * canvas_.properties.stride_bytes +
                                   canvas_destination.x() * dst_bytes_per_pixel,
                               /*count=*/canvas_destination.width() * dst_bytes_per_pixel);

    auto canvas_row_it = canvas_row.begin();
    for (int column = 0; column < canvas_destination.width(); column++) {
      canvas_row_it =
          std::copy(color_to_fill.data.begin(), color_to_fill.data.end(), canvas_row_it);
    }
  }
}

void SoftwareCompositor::CompositeLayers(std::span<const LayerForComposition> layers) const {
  constexpr PixelData kBlack = {0x00, 0x00, 0x00, 0xff};
  ClearCanvas(kBlack, PixelFormat::kRgba8888);
  for (const LayerForComposition& image_layer : layers) {
    if (!image_layer.image.buffer.empty()) {
      CompositeImage(image_layer.image, image_layer.properties);
    } else {
      CompositeFallbackColor(image_layer.properties);
    }
  }
}

}  // namespace software_compositor

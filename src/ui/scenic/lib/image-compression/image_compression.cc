// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/image-compression/image_compression.h"

#include <lib/async/cpp/task.h>
#include <lib/async/dispatcher.h>
#include <lib/syslog/cpp/macros.h>

#include <iostream>

// PNG Imports
#include <lib/async/default.h>
#include <png.h>

#include "src/lib/fsl/vmo/sized_vmo.h"
#include "src/lib/fsl/vmo/vector.h"

namespace image_compression {

void ImageCompression::EncodePng(EncodePngRequest& request, EncodePngCompleter::Sync& completer) {
  // This is an async protocol.
  auto async_completer = completer.ToAsync();

  // Ensure all required input fields exist.
  if (!request.raw_vmo() || !request.image_dimensions() || !request.png_vmo()) {
    async_completer.Reply(
        fit::as_error(fuchsia_ui_compression_internal::ImageCompressionError::kMissingArgs));
    return;
  }

  // Ensure both vmo sizes are consistent with the client-given width and height.
  const uint32_t width = request.image_dimensions()->width();
  const uint32_t height = request.image_dimensions()->height();

  // We are assuming BGRA_8 format for the input.
  const uint32_t pixel_size = 4;
  const uint32_t stride = width * pixel_size;
  const uint64_t raw_image_size = static_cast<uint64_t>(width) * height * pixel_size;

  // Convert in_vmo to a usable format.
  uint64_t in_vmo_size;
  request.raw_vmo()->get_size(&in_vmo_size);

  // Convert png_vmo to a usable format.
  uint64_t png_vmo_size;
  request.png_vmo()->get_size(&png_vmo_size);

  // Do some size checks.

  // Check that the stated width and height is compatible with |in_vmo_size|.
  if (raw_image_size > in_vmo_size) {
    FX_LOGS(WARNING) << "ImageCompression::EncodePng(): in_vmo is too small";
    async_completer.Reply(
        fit::as_error(fuchsia_ui_compression_internal::ImageCompressionError::kInvalidArgs));
    return;
  }

  // Check that the png_vmo_size is large enough to hold any potential PNG encoding of the raw
  // image.
  if (png_vmo_size < raw_image_size + zx_system_get_page_size()) {
    FX_LOGS(WARNING) << "ImageCompression::EncodePng(): png_vmo is too small";
    async_completer.Reply(
        fit::as_error(fuchsia_ui_compression_internal::ImageCompressionError::kInvalidArgs));
    return;
  }

  fsl::SizedVmo raw_image = fsl::SizedVmo(std::move(*request.raw_vmo()), raw_image_size);

  // Start libpng specific operations.
  png_structp png_ptr = png_create_write_struct(PNG_LIBPNG_VER_STRING, nullptr, nullptr, nullptr);
  if (!png_ptr) {
    async_completer.Reply(
        fit::as_error(fuchsia_ui_compression_internal::ImageCompressionError::kBadOperation));
    return;
  }

  png_infop info_ptr = png_create_info_struct(png_ptr);
  if (!info_ptr) {
    png_destroy_write_struct(&png_ptr, nullptr);

    async_completer.Reply(
        fit::as_error(fuchsia_ui_compression_internal::ImageCompressionError::kBadOperation));
    return;
  }

  // This is libpng obscure syntax for setting up the error handler.
  if (setjmp(png_jmpbuf(png_ptr))) {
    FX_LOGS(WARNING) << "ImageCompression::EncodePng(): Cannot set libpng error handler";
    async_completer.Reply(
        fit::as_error(fuchsia_ui_compression_internal::ImageCompressionError::kBadOperation));
    return;
  }

  static constexpr int bit_depth = 8;

  // Set the headers: output is 8-bit depth, RGBA format.
  png_set_IHDR(png_ptr, info_ptr, (uint32_t)width, (uint32_t)height, bit_depth, PNG_COLOR_TYPE_RGBA,
               PNG_INTERLACE_NONE, PNG_COMPRESSION_TYPE_DEFAULT, PNG_FILTER_TYPE_DEFAULT);

  std::vector<uint8_t> imgdata;
  if (!fsl::VectorFromVmo(raw_image, &imgdata)) {
    FX_LOGS(WARNING) << "ImageCompression::EncodePng(): Cannot extract data from raw image VMO";
    async_completer.Reply(
        fit::as_error(fuchsia_ui_compression_internal::ImageCompressionError::kBadOperation));
    return;
  }

  // Give libpng a pointer to each pixel at the beginning of each row.
  std::vector<uint8_t*> rows(height);
  for (size_t y = 0; y < height; ++y) {
    rows[y] = imgdata.data() + y * stride;
  }
  png_set_rows(png_ptr, info_ptr, rows.data());

  // Tell libpng how to process each row.
  std::vector<uint8_t> pixels;
  png_set_write_fn(
      png_ptr, &pixels,
      [](png_structp png_ptr, png_bytep data, png_size_t length) {
        auto p = reinterpret_cast<std::vector<uint8_t>*>(png_get_io_ptr(png_ptr));
        p->insert(p->end(), data, data + length);
      },
      nullptr);

  // This is actually the blocking call. At the end, the info and image will be written to |pixels|.
  // Note the swizzle flag, which instructs the library to read from BGRA data.
  png_write_png(png_ptr, info_ptr, PNG_TRANSFORM_BGR, nullptr);

  // This may fail if the client does not allow resizing - but that's okay as it's not necessary.
  request.png_vmo()->set_size(pixels.size());
  request.png_vmo()->set_prop_content_size(pixels.size());

  // Success!
  request.png_vmo()->write(pixels.data(), 0, pixels.size() * sizeof(uint8_t));
  async_completer.Reply(fit::ok());
}

}  // namespace image_compression

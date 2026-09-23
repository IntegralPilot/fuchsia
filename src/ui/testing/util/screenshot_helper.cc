// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "screenshot_helper.h"

#include <fuchsia/io/cpp/fidl.h>
#include <lib/zx/vmar.h>
#include <png.h>
#include <zircon/status.h>

#include <cstdint>
#include <cstdio>
#include <fstream>
#include <iostream>
#include <iterator>
#include <memory>
#include <unordered_map>
#include <vector>

#include "src/ui/scenic/lib/utils/pixel.h"

namespace ui_testing {
namespace {

using fuchsia::ui::composition::ScreenshotFormat;

constexpr uint64_t kBytesPerPixel = 4;
constexpr uint8_t kPNGHeaderBytes = 8;

// Needed for |png_set_read_fn| so that libpng can read from a zx::vmo in a stream-like fashion.
struct libpng_vmo {
  zx::vmo* vmo;
  size_t offset;
};

}  // namespace

Screenshot::Screenshot(const zx::vmo& screenshot_vmo, uint64_t width, uint64_t height, int rotation,
                       fuchsia_ui_composition::ScreenshotFormat format)
    : Screenshot(
          screenshot_vmo, width, height, rotation,
          static_cast<fuchsia::ui::composition::ScreenshotFormat>(fidl::ToUnderlying(format))) {}

Screenshot::Screenshot(const zx::vmo& screenshot_vmo, uint64_t width, uint64_t height, int rotation,
                       ScreenshotFormat format)
    : width_(width), height_(height) {
  if (rotation == 90 || rotation == 270) {
    std::swap(width_, height_);
  }

  // Populate |screenshot_| from |screenshot_vmo|.
  uint64_t vmo_size;
  screenshot_vmo.get_prop_content_size(&vmo_size);

  FX_CHECK(vmo_size == kBytesPerPixel * width_ * height_);
  size_ = vmo_size;

  uint8_t* vmo_host = nullptr;
  auto status = zx::vmar::root_self()->map(ZX_VM_PERM_READ, /*vmar_offset*/ 0, screenshot_vmo,
                                           /*vmo_offset*/ 0, vmo_size,
                                           reinterpret_cast<uintptr_t*>(&vmo_host));

  FX_CHECK(status == ZX_OK);

  ExtractScreenshotFromVMO(vmo_host, format);

  // Unmap the pointer.
  uintptr_t address = reinterpret_cast<uintptr_t>(vmo_host);
  status = zx::vmar::root_self()->unmap(address, vmo_size);
  FX_CHECK(status == ZX_OK);
}

Screenshot::Screenshot() : width_(0), height_(0) {}

Screenshot::Screenshot(const zx::vmo& png_vmo) {
  size_t vmo_size;
  png_vmo.get_size(&vmo_size);
  size_ = vmo_size;

  zx::vmo vmo_copy;
  FX_CHECK(png_vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo_copy) == ZX_OK);
  ExtractScreenshotFromPngVMO(vmo_copy);
}

Pixel Screenshot::GetPixelAt(uint64_t x, uint64_t y) const {
  FX_CHECK(x >= 0 && x < width_ && y >= 0 && y < height_) << "Index out of bounds";
  return screenshot_[y][x];
}

std::map<Pixel, uint32_t> Screenshot::Histogram() const {
  std::map<Pixel, uint32_t> histogram;
  FX_CHECK(screenshot_.size() == height_ && screenshot_[0].size() == width_);

  for (size_t i = 0; i < height_; i++) {
    for (size_t j = 0; j < width_; j++) {
      histogram[screenshot_[i][j]]++;
    }
  }

  return histogram;
}

float Screenshot::ComputeSimilarity(const Screenshot& other) const {
  if (width() != other.width() || height() != other.height())
    return 0;

  uint64_t num_matching_pixels = 0;
  for (uint64_t x = 0; x < width(); ++x) {
    for (uint64_t y = 0; y < height(); ++y) {
      if (GetPixelAt(x, y) == other.GetPixelAt(x, y))
        ++num_matching_pixels;
    }
  }
  return 100.f * static_cast<float>(num_matching_pixels) / static_cast<float>((width() * height()));
}

float Screenshot::ComputeHistogramSimilarity(const Screenshot& other) const {
  if (width() != other.width() || height() != other.height())
    return 0;

  auto histogram = this->Histogram();
  auto other_histogram = other.Histogram();

  uint64_t num_matching_pixels = 0;

  for (auto it = histogram.begin(); it != histogram.end(); ++it) {
    if (other_histogram.find(it->first) != other_histogram.end()) {
      num_matching_pixels += std::min(it->second, other_histogram[it->first]);
    }
  }
  return 100.f * static_cast<float>(num_matching_pixels) / static_cast<float>((width() * height()));
}

bool Screenshot::DumpToCustomArtifacts(const std::string& filename) const {
  const std::string file_path = "/custom_artifacts/" + filename;
  std::ofstream file(file_path);
  if (!file.is_open()) {
    FX_LOGS(ERROR)
        << "Artifact cannot be opened. Follow https://fuchsia.dev/fuchsia-src/development/testing/components/test_runner_framework?hl=en#custom-artifacts.";
  } else {
    for (size_t y = 0; y < height(); ++y) {
      for (size_t x = 0; x < width(); ++x) {
        const auto pixel = GetPixelAt(x, y);
        file << pixel.blue << pixel.green << pixel.red << pixel.alpha;
      }
    }
    file.close();
    FX_LOGS(INFO) << "Screenshot artifact dumped.";
  }
  return true;
}

bool Screenshot::DumpPngToCustomArtifacts(const std::string& filename) const {
  const std::string file_path = "/custom_artifacts/" + filename;
  FILE* fp = fopen(file_path.c_str(), "wb");
  if (!fp) {
    FX_LOGS(ERROR)
        << file_path
        << " cannot be written. Follow https://fuchsia.dev/fuchsia-src/development/testing/components/test_runner_framework?hl=en#custom-artifacts.";
    return false;
  }

  png_structp png = png_create_write_struct(PNG_LIBPNG_VER_STRING, nullptr /* error_ptr */,
                                            nullptr /* error_fn */, nullptr /* warn_fn */);
  FX_CHECK(png);
  png_infop png_info = png_create_info_struct(png);
  FX_CHECK(png_info);

  // This is libpng's syntax for setting up the error handler
  // by using a jump address(!). All calls to libpng must reside in
  // this function.
  if (setjmp(png_jmpbuf(png))) {
    FX_LOGS(ERROR) << "Something went wrong in libpng during write";
    fclose(fp);
    png_destroy_write_struct(&png, &png_info);
    return false;
  }

  png_init_io(png, fp);

  // Set the headers: output is 8-bit depth, RGBA format.
  png_set_IHDR(png, png_info, (uint32_t)width_, (uint32_t)height_, 8 /* bit depth */,
               PNG_COLOR_TYPE_RGBA, PNG_INTERLACE_NONE, PNG_COMPRESSION_TYPE_DEFAULT,
               PNG_FILTER_TYPE_DEFAULT);
  png_write_info(png, png_info);

  // libpng needs pointers to each row of pixels.
  std::vector<const uint8_t*> row_ptrs(height_);
  for (size_t y = 0; y < height_; ++y) {
    row_ptrs[y] = reinterpret_cast<const uint8_t*>(screenshot_[y].data());
  }

  png_set_rows(png, png_info, const_cast<uint8_t**>(row_ptrs.data()));
  // PNG expected RGBA, but screenshot_ is BGRA, so use PNG_TRANSFORM_BGR
  png_write_png(png, png_info, PNG_TRANSFORM_BGR, nullptr /* unused */);

  fclose(fp);
  png_destroy_write_struct(&png, &png_info);

  FX_LOGS(INFO) << "Screenshot saved to " << file_path;
  return true;
}

bool Screenshot::LoadFromPng(const std::string& png_filename) {
  FILE* fp = fopen(png_filename.c_str(), "rb");
  if (!fp) {
    FX_LOGS(FATAL) << " cannot read " << png_filename;
    return false;
  }

  png_structp png = png_create_read_struct(PNG_LIBPNG_VER_STRING, nullptr /* error_ptr */,
                                           nullptr /* error_fn */, nullptr /* warn_fn */);
  FX_CHECK(png);
  png_infop png_info = png_create_info_struct(png);
  FX_CHECK(png_info);

  // This is libpng's syntax for setting up the error handler
  // by using a jump address(!). All calls to libpng must reside in
  // this function.
  if (setjmp(png_jmpbuf(png))) {
    FX_LOGS(ERROR) << "Something went wrong in libpng during read";
    fclose(fp);
    png_destroy_read_struct(&png, &png_info, nullptr /* end_info_ptr_ptr */);
    return false;
  }

  png_init_io(png, fp);

  // Read header info
  png_read_info(png, png_info);

  width_ = png_get_image_width(png, png_info);
  height_ = png_get_image_height(png, png_info);

  auto color_type = png_get_color_type(png, png_info);
  auto bit_depth = png_get_bit_depth(png, png_info);
  FX_CHECK(color_type == PNG_COLOR_TYPE_RGBA) << "Only RGBA png format is supported";
  FX_CHECK(bit_depth == 8) << "Only 8 bit png format is supported";

  // Initialize screenshot memory and pass row ptrs to libpng.
  screenshot_.clear();
  std::vector<uint8_t*> row_ptrs(height_);
  for (size_t y = 0; y < height_; ++y) {
    // Initialize row memory with empty pixels.
    screenshot_.emplace_back(width_, Pixel(0, 0, 0, 0));
    row_ptrs[y] = reinterpret_cast<uint8_t*>(screenshot_[y].data());
  }
  png_set_bgr(png);
  png_read_image(png, reinterpret_cast<uint8_t**>(row_ptrs.data()));

  fclose(fp);
  png_destroy_read_struct(&png, &png_info, nullptr /* end_info_ptr_ptr */);

  FX_LOGS(INFO) << "Screenshot loaded from " << png_filename;

  return true;
}

std::vector<std::pair<uint32_t, utils::Pixel>> Screenshot::LogHistogramTopPixels(
    int num_top_pixels) const {
  auto histogram = Histogram();
  std::vector<std::pair<uint32_t, utils::Pixel>> vec;
  std::transform(
      histogram.begin(), histogram.end(), std::inserter(vec, vec.begin()),
      [](const std::pair<utils::Pixel, uint32_t> p) { return std::make_pair(p.second, p.first); });
  std::stable_sort(vec.begin(), vec.end(),
                   [](const auto& a, const auto& b) { return a.first > b.first; });

  std::vector<std::pair<uint32_t, utils::Pixel>> top;
  std::copy(vec.begin(), vec.begin() + std::min<ptrdiff_t>(vec.size(), num_top_pixels),
            std::back_inserter(top));

  std::cout << "Histogram top:" << std::endl;
  for (const auto& elems : top) {
    std::cout << "{ " << elems.second << " value: " << elems.first << " }" << std::endl;
  }
  std::cout << "--------------" << std::endl;
  return top;
}

void Screenshot::ExtractScreenshotFromVMO(uint8_t* screenshot_vmo, ScreenshotFormat format) {
  FX_CHECK(screenshot_vmo);

  for (size_t i = 0; i < height_; i++) {
    // The head index of the ith row in the screenshot is |i* width_* KbytesPerPixel|.
    screenshot_.push_back(GetPixelsInRow(screenshot_vmo, i, format));
  }
}

std::vector<Pixel> Screenshot::GetPixelsInRow(uint8_t* screenshot_vmo, size_t row_index,
                                              ScreenshotFormat format) const {
  std::vector<Pixel> row;

  for (size_t col_idx = 0; col_idx < static_cast<size_t>(width_ * kBytesPerPixel);
       col_idx += kBytesPerPixel) {
    // Each row in the screenshot has |kBytesPerPixel * width_| elements. Therefore in order to
    // reach the first pixel of the |row_index| row, we have to jump |row_index * width_ *
    // kBytesPerPixel| positions.
    auto pixel_start_index = row_index * width_ * kBytesPerPixel;

    // Every |kBytesPerPixel| bytes represents the BGRA/RGBA values of a pixel. Skip
    // |kBytesPerPixel| bytes to get to the BGRA/RGBA values of the next pixel. Each row in a
    // screenshot has |kBytesPerPixel * width_| bytes of data. Example:- auto data =
    // TakeScreenshot(); data[0-3] -> RGBA of pixel 0. data[4-7] -> RGBA pf pixel 1.
    if (format == ScreenshotFormat::RGBA_RAW) {
      row.emplace_back(screenshot_vmo[pixel_start_index + col_idx + 2],
                       screenshot_vmo[pixel_start_index + col_idx + 1],
                       screenshot_vmo[pixel_start_index + col_idx],
                       screenshot_vmo[pixel_start_index + col_idx + 3]);
    } else {
      row.emplace_back(screenshot_vmo[pixel_start_index + col_idx],
                       screenshot_vmo[pixel_start_index + col_idx + 1],
                       screenshot_vmo[pixel_start_index + col_idx + 2],
                       screenshot_vmo[pixel_start_index + col_idx + 3]);
    }
  }

  return row;
}

// Decode PNG-encoded VMO back to raw format and populate screenshot_ with raw pixels.
// Done this way for testing purposes so we can compare accuracy of screenshots taken in PNG format.
void Screenshot::ExtractScreenshotFromPngVMO(zx::vmo& png_vmo) {
  png_structp png = png_create_read_struct(PNG_LIBPNG_VER_STRING, nullptr, nullptr, nullptr);
  FX_DCHECK(png) << "png_create_read_struct failed";

  png_infop info = png_create_info_struct(png);
  FX_DCHECK(info) << "png_create_info_struct failed";

  // Tell libpng how to read from a zx::vmo in a stream-like fashion.
  libpng_vmo read_fn_vmo = {.vmo = &png_vmo, .offset = 0u};
  png_set_read_fn(png, &read_fn_vmo,
                  [](png_structp png_ptr, png_bytep out_bytes, size_t length) -> void {
                    // Read |length| bytes into |out_bytes| from the VMO.
                    libpng_vmo* vmo = reinterpret_cast<libpng_vmo*>(png_get_io_ptr(png_ptr));
                    FX_CHECK(vmo->vmo->read(out_bytes, vmo->offset, length) == ZX_OK);
                    vmo->offset += length;
                  });

  png_read_info(png, info);

  width_ = png_get_image_width(png, info);
  height_ = png_get_image_height(png, info);

  uint32_t color_type = png_get_color_type(png, info);
  uint32_t bit_depth = png_get_bit_depth(png, info);

  // Only works with 4 bytes (32-bits) per pixel.
  FX_CHECK(color_type == PNG_COLOR_TYPE_RGBA) << "currently only supports RGBA";
  FX_CHECK(bit_depth == 8) << "currently only supports 8-bit channel";

  int64_t row_bytes = png_get_rowbytes(png, info);
  int64_t expected_row_bytes = kBytesPerPixel * width_;  // We assume each pixel is 4 bytes.
  FX_DCHECK(row_bytes == expected_row_bytes)
      << "unexpected row_bytes: " << row_bytes << " expect: 4 * " << width_;

  const uint64_t bytesPerRow = png_get_rowbytes(png, info);
  std::vector<uint8_t> rowData(bytesPerRow);

  // Read one row at a time. For some reason, this is necessary instead of |png_read_image()|. Maybe
  // because we're reading from memory instead of a file?
  for (uint32_t rowIdx = 0; rowIdx < height_; ++rowIdx) {
    png_read_row(png, static_cast<png_bytep>(rowData.data()), nullptr);

    uint32_t byteIndex = 0;
    std::vector<Pixel> row;
    for (uint32_t colIdx = 0; colIdx < width_; ++colIdx) {
      const uint8_t red = rowData[byteIndex++];
      const uint8_t green = rowData[byteIndex++];
      const uint8_t blue = rowData[byteIndex++];
      const uint8_t alpha = rowData[byteIndex++];

      row.emplace_back(blue, green, red, alpha);
    }
    screenshot_.push_back(row);
  }
  png_destroy_read_struct(&png, &info, nullptr);
}

}  // namespace ui_testing

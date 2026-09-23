// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/tests/utils/utils.h"

#include <fidl/fuchsia.io/cpp/fidl.h>
#include <lib/async/cpp/executor.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/ui/scenic/cpp/id.h>

#include <cmath>
#include <cstdint>

#include "src/ui/testing/util/screenshot_helper.h"

namespace integration_tests {

using ui_testing::Screenshot;

// Used to compare whether two values are nearly equal.
// 1000 times machine limits to account for scaling from [0,1] to viewing volume [0,1000].
constexpr float kEpsilon = std::numeric_limits<float>::epsilon() * 1000;

bool CmpFloatingValues(float num1, float num2) {
  auto diff = fabs(num1 - num2);
  return diff < kEpsilon;
}

zx_koid_t ExtractKoid(const zx::object_base& object) {
  zx_info_handle_basic_t info{};
  if (object.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr) != ZX_OK) {
    return ZX_KOID_INVALID;  // no info
  }

  return info.koid;
}

zx_koid_t ExtractKoid(const fuchsia_ui_views::ViewRef& view_ref) {
  return ExtractKoid(view_ref.reference());
}

Mat3 ArrayToMat3(std::array<float, 9> array) {
  Mat3 mat;
  for (size_t row = 0; row < mat.size(); row++) {
    for (size_t col = 0; col < mat[0].size(); col++) {
      mat[row][col] = array[mat.size() * row + col];
    }
  }
  return mat;
}

Vec3 operator*(const Mat3& mat, const Vec3& vec) {
  Vec3 result = {0, 0, 0};
  for (size_t col = 0; col < mat[0].size(); col++) {
    for (size_t row = 0; row < mat.size(); row++) {
      result[col] += mat[row][col] * vec[row];
    }
  }
  return result;
}

Vec3& operator/(Vec3& vec, float num) {
  for (size_t i = 0; i < vec.size(); i++) {
    vec[i] /= num;
  }
  return vec;
}

Vec4 angleAxis(float angle, const Vec3& vec) {
  Vec4 result;
  result[0] = vec[0] * sin(angle * 0.5f);
  result[1] = vec[1] * sin(angle * 0.5f);
  result[2] = vec[2] * sin(angle * 0.5f);
  result[3] = cos(angle * 0.5f);
  return result;
}

ui_testing::Screenshot TakeScreenshot(
    const fidl::SyncClient<fuchsia_ui_composition::Screenshot>& screenshotter, uint64_t width,
    uint64_t height, fuchsia_ui_composition::ScreenshotFormat format, int display_rotation) {
  fuchsia_ui_composition::ScreenshotTakeRequest request;
  request.format() = format;

  auto result = screenshotter->Take(std::move(request));
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to take screenshot: " << result.error_value().FormatDescription();
    return Screenshot();
  }
  FX_CHECK(result.value().vmo().has_value());
  zx::vmo& vmo = result.value().vmo().value();

  if (format == fuchsia_ui_composition::ScreenshotFormat::kPng) {
    return Screenshot(vmo);
  }
  return Screenshot(vmo, width, height, display_rotation, format);
}

ui_testing::Screenshot TakeFileScreenshot(
    const fidl::SyncClient<fuchsia_ui_composition::Screenshot>& screenshotter, uint64_t width,
    uint64_t height, fuchsia_ui_composition::ScreenshotFormat format, int display_rotation) {
  fuchsia_ui_composition::ScreenshotTakeFileRequest request;
  request.format() = format;

  auto result = screenshotter->TakeFile(std::move(request));
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to take screenshot: " << result.error_value().FormatDescription();
    return Screenshot();
  }
  FX_CHECK(result.value().file().has_value());
  fidl::SyncClient<fuchsia_io::File> file(std::move(result.value().file().value()));

  auto attr_result = file->GetAttributes(fuchsia_io::NodeAttributesQuery::kContentSize);
  FX_CHECK(attr_result.is_ok()) << attr_result.error_value().FormatDescription();
  FX_CHECK(attr_result->immutable_attributes().content_size().has_value());
  auto screenshot_size = *attr_result->immutable_attributes().content_size();

  zx::vmo vmo_from_file;
  FX_CHECK(zx::vmo::create(screenshot_size, 0, &vmo_from_file) == ZX_OK);
  uint64_t offset = 0;
  uint64_t read_response_size = fuchsia_io::kMaxBuf;
  do {
    auto read_result = file->Read(fuchsia_io::kMaxBuf);
    FX_CHECK(read_result.is_ok()) << read_result.error_value().FormatDescription();
    const auto& response_data = read_result->data();
    read_response_size = response_data.size();
    vmo_from_file.write(response_data.data(), offset, read_response_size);
    offset += read_response_size;
  } while (read_response_size == fuchsia_io::kMaxBuf);

  if (format == fuchsia_ui_composition::ScreenshotFormat::kPng) {
    return Screenshot(vmo_from_file);
  }
  return Screenshot(vmo_from_file, width, height, display_rotation, format);
}

}  // namespace integration_tests

// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "color_transform_manager.h"

#include <lib/fidl/cpp/clone.h>
#include <lib/syslog/cpp/macros.h>

#include <src/ui/a11y/lib/util/util.h>

namespace a11y {

// clang-format off
const std::array<float, 9> kIdentityMatrix = {
    1, 0, 0,
    0, 1, 0,
    0, 0, 1};
const std::array<float, 3> kZero3x1Vector = {0, 0, 0};

// To invert a color vector in RGB space, we first convert it to
// YIQ color space, then rotate it along Y axis for 180 degrees,
// convert it back to RGB space, and subtract it by 1.
//
// Formula of inverted color:
//   [R' G' B']' = [1, 1, 1] - inv(T) . diag(1, -1, -1) . T . [R G B]'
//               = [1, 1, 1] + kColorInversionMatrix . [R G B]'
//
// where R, G, B \in [0, 1], and T is the RGB to YIQ conversion
// matrix:
//   T = [[0.299   0.587   0.114]
//        [0.596  -0.274  -0.321]
//        [0.211  -0.523   0.311]]
//
// Thus the color inveresion matrix is
//   kColorInversionMatrix
//    = [[ 0.402  -1.174  -0.228]
//       [-0.598  -0.174  -0.228]
//       [-0.599  -1.177   0.771]]
//
const std::array<float, 9> kColorInversionMatrix = {
    0.402f,  -1.174f,  -0.228f,
   -0.598f,  -0.174f,  -0.228f,
   -0.599f,  -1.177f,   0.771f};

// Post offsets should be strictly less than 1.
const std::array<float, 3> kColorInversionPostOffset = {.999f, .999f, .999f};

const std::array<float, 9> kCorrectProtanomaly = {
    0.622774, 0.377226,  0.000000,
    0.264275, 0.735725,  -0.000000,
    0.216821, -0.216821, 1.000000};
const std::array<float, 9> kCorrectDeuteranomaly = {
    0.288299f, 0.711701f,  0.000000f,
    0.052709f, 0.947291f,  -0.000000f,
    -0.257912f, 0.257912f, 1.000000f};
const std::array<float, 9> kCorrectTritanomaly = {
    1.000000f, -0.805712f, 0.805712f,
     0.000000f, 0.378838f, 0.621162f,
    -0.000000f,  0.104823f, 0.895177f};
// clang-format on

namespace {

struct ColorAdjustmentArgs {
  std::array<float, 9> color_adjustment_matrix;
  std::array<float, 3> color_adjustment_pre_offset;
  std::array<float, 3> color_adjustment_post_offset;
};

ColorAdjustmentArgs GetColorAdjustmentArgs(
    bool color_inversion_enabled,
    fuchsia_accessibility::ColorCorrectionMode color_correction_mode) {
  std::array<float, 9> color_inversion_matrix = kIdentityMatrix;
  std::array<float, 9> color_correction_matrix = kIdentityMatrix;
  std::array<float, 3> color_adjustment_pre_offset = kZero3x1Vector;
  std::array<float, 3> color_adjustment_post_offset = kZero3x1Vector;

  if (color_inversion_enabled) {
    color_inversion_matrix = kColorInversionMatrix;
    color_adjustment_post_offset = kColorInversionPostOffset;
  }

  switch (color_correction_mode) {
    case fuchsia_accessibility::ColorCorrectionMode::kCorrectProtanomaly:
      color_correction_matrix = kCorrectProtanomaly;
      break;
    case fuchsia_accessibility::ColorCorrectionMode::kCorrectDeuteranomaly:
      color_correction_matrix = kCorrectDeuteranomaly;
      break;
    case fuchsia_accessibility::ColorCorrectionMode::kCorrectTritanomaly:
      color_correction_matrix = kCorrectTritanomaly;
      break;
    case fuchsia_accessibility::ColorCorrectionMode::kDisabled:
      // fall through
    default:
      color_correction_matrix = kIdentityMatrix;
      break;
  }

  return ColorAdjustmentArgs{.color_adjustment_matrix = Multiply3x3MatrixRowMajor(
                                 color_inversion_matrix, color_correction_matrix),
                             .color_adjustment_pre_offset = color_adjustment_pre_offset,
                             .color_adjustment_post_offset = color_adjustment_post_offset};
}

}  // namespace

void ColorTransformHandlerErrorHandler::on_fidl_error(fidl::UnbindInfo info) {
  FX_LOGS(ERROR) << "ColorTransformHandler disconnected: " << info;
}

ColorTransformManager::ColorTransformManager(async_dispatcher_t* dispatcher,
                                             sys::ComponentContext* startup_context)
    : dispatcher_(dispatcher) {
  FX_CHECK(dispatcher_);
  FX_CHECK(startup_context);
  startup_context->outgoing()->AddProtocol<fuchsia_accessibility::ColorTransform>(
      bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure));
}

void ColorTransformManager::RegisterColorTransformHandler(
    RegisterColorTransformHandlerRequest& request,
    RegisterColorTransformHandlerCompleter::Sync& completer) {
  color_transform_handler_ = fidl::Client(std::move(request.handler()), dispatcher_,
                                          &color_transform_handler_error_handler_);

  MaybeSetColorTransformConfiguration();
}

void ColorTransformManager::ChangeColorTransform(
    bool color_inversion_enabled,
    fuchsia_accessibility::ColorCorrectionMode color_correction_mode) {
  fuchsia_accessibility::ColorTransformConfiguration color_transform_configuration;
  ColorAdjustmentArgs color_adjustment_args =
      GetColorAdjustmentArgs(color_inversion_enabled, color_correction_mode);
  color_transform_configuration.color_inversion_enabled(color_inversion_enabled);
  color_transform_configuration.color_correction(color_correction_mode);
  color_transform_configuration.color_adjustment_matrix(
      color_adjustment_args.color_adjustment_matrix);
  color_transform_configuration.color_adjustment_post_offset(
      color_adjustment_args.color_adjustment_post_offset);
  color_transform_configuration.color_adjustment_pre_offset(
      color_adjustment_args.color_adjustment_pre_offset);

  cached_color_transform_configuration_ = std::move(color_transform_configuration);
  MaybeSetColorTransformConfiguration();
}

void ColorTransformManager::MaybeSetColorTransformConfiguration() {
  if (!color_transform_handler_ || cached_color_transform_configuration_.IsEmpty()) {
    return;
  }

  // The use of copy here is preferred over std::move to handle the unlikely
  // case in which ColorTransformHandler is re-bound. Color transform changes
  // should be infrequent, so the performance penalty is worth the additional
  // robustness.
  auto cached_color_transform_configuration = cached_color_transform_configuration_;

  color_transform_handler_
      ->SetColorTransformConfiguration({std::move(cached_color_transform_configuration)})
      .Then([](auto& resp) { FX_LOGS(INFO) << "Color transform configuration changed."; });
}

}  // namespace a11y

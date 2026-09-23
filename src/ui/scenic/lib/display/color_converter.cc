// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/display/color_converter.h"

#include <lib/async/default.h>

#include <algorithm>
#include <cmath>
#include <iterator>

#include <sdk/lib/syslog/cpp/macros.h>

#include "src/ui/scenic/lib/utils/helpers.h"

namespace display {

namespace {

template <typename T>
bool AreValid(const T& values) {
  return std::all_of(std::begin(values), std::end(values),
                     [](const auto& value) { return std::isfinite(value); });
}

constexpr fidl::Array<float, 9> kDefaultCoefficients = {1, 0, 0, 0, 1, 0, 0, 0, 1};
constexpr fidl::Array<float, 3> kDefaultOffsets = {0, 0, 0};

}  // namespace

ColorConverter::ColorConverter(sys::ComponentContext* app_context,
                               SetColorConversionFunc set_color_conversion_values,
                               SetMinimumRgbFunc set_minimum_rgb)
    : set_color_conversion_values_(std::move(set_color_conversion_values)),
      set_minimum_rgb_(std::move(set_minimum_rgb)) {
  FX_DCHECK(app_context);
  FX_DCHECK(set_color_conversion_values_);
  FX_DCHECK(set_minimum_rgb_);
  app_context->outgoing()->AddProtocol<fuchsia_ui_display_color::Converter>(
      bindings_.CreateHandler(this, async_get_default_dispatcher(), fidl::kIgnoreBindingClosure));
}

void ColorConverter::SetValues(SetValuesRequestView request, SetValuesCompleter::Sync& completer) {
  const auto& properties = request->properties;
  const fidl::Array<float, 9> coefficients =
      properties.has_coefficients() ? properties.coefficients() : kDefaultCoefficients;
  const fidl::Array<float, 3> preoffsets =
      properties.has_preoffsets() ? properties.preoffsets() : kDefaultOffsets;
  const fidl::Array<float, 3> postoffsets =
      properties.has_postoffsets() ? properties.postoffsets() : kDefaultOffsets;

  if (!AreValid(coefficients) || !AreValid(preoffsets) || !AreValid(postoffsets)) {
    const std::string& coefficients_str = utils::GetArrayString("Coefficients", coefficients);
    const std::string& preoffsets_str = utils::GetArrayString("Preoffsets", preoffsets);
    const std::string& postoffsets_str = utils::GetArrayString("Postoffsets", postoffsets);
    FX_LOGS(ERROR) << "Invalid Color Conversion Parameter Values: \n"
                   << coefficients_str << preoffsets_str << postoffsets_str;
    completer.Reply(ZX_ERR_INVALID_ARGS);
    return;
  }

  set_color_conversion_values_(coefficients, preoffsets, postoffsets);
  completer.Reply(ZX_OK);
}

void ColorConverter::SetMinimumRgb(SetMinimumRgbRequestView request,
                                   SetMinimumRgbCompleter::Sync& completer) {
  completer.Reply(set_minimum_rgb_(request->minimum_rgb));
}

}  // namespace display

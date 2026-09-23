// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_DISPLAY_COLOR_CONVERTER_H_
#define SRC_UI_SCENIC_LIB_DISPLAY_COLOR_CONVERTER_H_

#include <fidl/fuchsia.ui.display.color/cpp/wire.h>
#include <lib/fidl/cpp/wire/server.h>
#include <lib/sys/cpp/component_context.h>

namespace display {

using SetColorConversionFunc = fit::function<void(const fidl::Array<float, 9>& coefficients,
                                                  const fidl::Array<float, 3>& preoffsets,
                                                  const fidl::Array<float, 3>& postoffsets)>;
using SetMinimumRgbFunc = fit::function<bool(uint8_t minimum_rgb)>;

// Backend for the ColorConverter FIDL interface.
class ColorConverter : public fidl::WireServer<fuchsia_ui_display_color::Converter> {
 public:
  ColorConverter(sys::ComponentContext* app_context,
                 SetColorConversionFunc set_color_conversion_values,
                 SetMinimumRgbFunc set_minimum_rgb);

  // |fidl::WireServer<fuchsia_ui_display_color::Converter>|
  void SetValues(SetValuesRequestView request, SetValuesCompleter::Sync& completer) override;

  // |fidl::WireServer<fuchsia_ui_display_color::Converter>|
  void SetMinimumRgb(SetMinimumRgbRequestView request,
                     SetMinimumRgbCompleter::Sync& completer) override;

 private:
  fidl::ServerBindingGroup<fuchsia_ui_display_color::Converter> bindings_;

  const SetColorConversionFunc set_color_conversion_values_;
  const SetMinimumRgbFunc set_minimum_rgb_;
};

}  // namespace display

#endif  // SRC_UI_SCENIC_LIB_DISPLAY_COLOR_CONVERTER_H_

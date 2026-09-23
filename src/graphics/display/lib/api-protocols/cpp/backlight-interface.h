// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_PROTOCOLS_CPP_BACKLIGHT_INTERFACE_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_PROTOCOLS_CPP_BACKLIGHT_INTERFACE_H_

#include <lib/zx/result.h>

#include "src/graphics/display/lib/api-types/cpp/backlight-state.h"

namespace display {

// The methods in the [`fuchsia.hardware.backlight/Device`] FIDL interface.
class BacklightInterface {
 public:
  BacklightInterface() = default;

  BacklightInterface(const BacklightInterface&) = delete;
  BacklightInterface(BacklightInterface&&) = delete;
  BacklightInterface& operator=(const BacklightInterface&) = delete;
  BacklightInterface& operator=(BacklightInterface&&) = delete;

  // Retrieves the maximum device brightness, if known.
  //
  // If successful, the returned value is guaranteed to be positive.
  //
  // Errors with ZX_ERR_NOT_SUPPORTED if the driver does not provide
  // guarantees around the amount of light emitted.
  virtual zx::result<float> GetMaxBrightnessNits() = 0;

  // Retrieves the device state.
  //
  // If successful, the returned value is guaranteed to be valid.
  virtual zx::result<BacklightState> GetBacklightState() = 0;

  // Sets the device state.
  //
  // `Backlight::brightness_nits` has precedence over
  // `Backlight::brightness_fraction`. More precisley, if
  // `Backlight::brightness_nits` is populated (not nullopt), the backlight
  // brightness is derived from it, and `Backlight::brightness_fraction` is
  // ignored.
  //
  // Errors if ZX_ERR_NOT_SUPPORTED if `Backlight::brightness_nits` is populated
  // and the driver does not support absolute brightness.
  virtual zx::result<> SetBacklightState(const BacklightState& state) = 0;

 protected:
  // Destruction via base class pointer is not supported intentionally.
  // Instances are not expected to be owned by pointers to base classes.
  ~BacklightInterface() = default;
};

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_PROTOCOLS_CPP_BACKLIGHT_INTERFACE_H_

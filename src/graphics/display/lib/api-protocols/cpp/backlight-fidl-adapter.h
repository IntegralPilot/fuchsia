// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_PROTOCOLS_CPP_BACKLIGHT_FIDL_ADAPTER_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_PROTOCOLS_CPP_BACKLIGHT_FIDL_ADAPTER_H_

#include <fidl/fuchsia.hardware.backlight/cpp/wire.h>
#include <lib/async/dispatcher.h>

#include "src/graphics/display/lib/api-protocols/cpp/backlight-interface.h"

namespace display {

// Translates FIDL API calls to `BacklightInterface` C++ method calls.
//
// This adapter implements the [`fuchsia.hardware.backlight/Device`] FIDL API.
class BacklightFidlAdapter : public fidl::WireServer<fuchsia_hardware_backlight::Device> {
 public:
  // `backlight` receives translated FIDL calls to the
  // [`fuchsia.hardware.backlight/Device`] interface. It must not be null, and
  // must outlive the newly created instance.
  explicit BacklightFidlAdapter(BacklightInterface* backlight);

  BacklightFidlAdapter(const BacklightFidlAdapter&) = delete;
  BacklightFidlAdapter& operator=(const BacklightFidlAdapter&) = delete;

  ~BacklightFidlAdapter() override;

  fidl::ProtocolHandler<fuchsia_hardware_backlight::Device> CreateHandler(
      async_dispatcher_t& dispatcher);

  // fidl::WireServer<fuchsia_hardware_backlight::Device>:
  void GetStateNormalized(GetStateNormalizedCompleter::Sync& completer) override;
  void SetStateNormalized(
      fuchsia_hardware_backlight::wire::DeviceSetStateNormalizedRequest* request,
      SetStateNormalizedCompleter::Sync& completer) override;
  void GetStateAbsolute(GetStateAbsoluteCompleter::Sync& completer) override;
  void SetStateAbsolute(fuchsia_hardware_backlight::wire::DeviceSetStateAbsoluteRequest* request,
                        SetStateAbsoluteCompleter::Sync& completer) override;
  void GetMaxAbsoluteBrightness(GetMaxAbsoluteBrightnessCompleter::Sync& completer) override;

 private:
  // This data member is thread-safe because it is immutable.
  BacklightInterface& backlight_;

  fidl::ServerBindingGroup<fuchsia_hardware_backlight::Device> bindings_;
};

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_PROTOCOLS_CPP_BACKLIGHT_FIDL_ADAPTER_H_

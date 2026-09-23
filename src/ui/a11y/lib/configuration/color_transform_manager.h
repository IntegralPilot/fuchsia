// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_A11Y_LIB_CONFIGURATION_COLOR_TRANSFORM_MANAGER_H_
#define SRC_UI_A11Y_LIB_CONFIGURATION_COLOR_TRANSFORM_MANAGER_H_

#include <fidl/fuchsia.accessibility/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/fidl/cpp/channel.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/macros.h>
#include <math.h>

namespace a11y {

class ColorTransformHandlerErrorHandler
    : public fidl::AsyncEventHandler<fuchsia_accessibility::ColorTransformHandler> {
  void on_fidl_error(fidl::UnbindInfo info) override;
};

class ColorTransformManager : public fidl::Server<fuchsia_accessibility::ColorTransform> {
 public:
  explicit ColorTransformManager(async_dispatcher_t* dispatcher,
                                 sys::ComponentContext* startup_context);

  ~ColorTransformManager() = default;

  // Registers a color transform handler to receive updates about color correction and inversion
  // settings changes. Only one color transform handler at a time is supported.
  void RegisterColorTransformHandler(
      RegisterColorTransformHandlerRequest& request,
      RegisterColorTransformHandlerCompleter::Sync& completer) override;

  // Called to actually change the color transform settings in the system.
  void ChangeColorTransform(bool color_inversion_enabled,
                            fuchsia_accessibility::ColorCorrectionMode color_correction_mode);

 private:
  void MaybeSetColorTransformConfiguration();

  // not owned
  async_dispatcher_t* dispatcher_;

  fidl::ServerBindingGroup<fuchsia_accessibility::ColorTransform> bindings_;

  fuchsia_accessibility::ColorTransformConfiguration cached_color_transform_configuration_;

  // Note that for now, this class supports exactly one color transform handler.
  fidl::Client<fuchsia_accessibility::ColorTransformHandler> color_transform_handler_;

  // error handler for ColorTransformHandler connection.
  ColorTransformHandlerErrorHandler color_transform_handler_error_handler_;
};
}  // namespace a11y

#endif  // SRC_UI_A11Y_LIB_CONFIGURATION_COLOR_TRANSFORM_MANAGER_H_

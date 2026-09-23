// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_DISPLAY_SINGLETON_DISPLAY_SERVICE_H_
#define SRC_UI_SCENIC_LIB_DISPLAY_SINGLETON_DISPLAY_SERVICE_H_

#include <fidl/fuchsia.ui.composition.internal/cpp/fidl.h>
#include <fidl/fuchsia.ui.display.singleton/cpp/fidl.h>
#include <lib/sys/cpp/outgoing_directory.h>

#include <memory>

#include "src/ui/scenic/lib/display/display.h"

namespace display {

// Implements the fuchsia.ui.display.singleton.Info and
// fuchsia.ui.composition.internal.DisplayOwnership FIDL services.
class SingletonDisplayService
    : public fidl::Server<fuchsia_ui_display_singleton::Info>,
      public fidl::Server<fuchsia_ui_composition_internal::DisplayOwnership> {
 public:
  explicit SingletonDisplayService(std::shared_ptr<display::Display> display);

  // |fuchsia_ui_display_singleton::Info|
  void GetMetrics(GetMetricsCompleter::Sync& completer) override;
  void GetMetrics(
      fit::function<void(fuchsia_ui_display_singleton::InfoGetMetricsResponse)> callback);

  // |fuchsia_ui_composition_internal::DisplayOwnership|
  void GetEvent(GetEventCompleter::Sync& completer) override;
  void GetEvent(
      fit::function<void(fuchsia_ui_composition_internal::DisplayOwnershipGetEventResponse)>
          callback);

  // Registers this service impl in |outgoing_directory|.  This service impl object must then live
  // for as long as it is possible for any service requests to be made.
  void AddPublicService(sys::OutgoingDirectory* outgoing_directory);

 private:
  const std::shared_ptr<display::Display> display_ = nullptr;
  fidl::ServerBindingGroup<fuchsia_ui_display_singleton::Info> info_bindings_;
  fidl::ServerBindingGroup<fuchsia_ui_composition_internal::DisplayOwnership> ownership_bindings_;
};

}  // namespace display

#endif  // SRC_UI_SCENIC_LIB_DISPLAY_SINGLETON_DISPLAY_SERVICE_H_

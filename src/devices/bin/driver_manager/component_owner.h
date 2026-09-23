// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_COMPONENT_OWNER_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_COMPONENT_OWNER_H_

#include <fidl/fuchsia.component.runner/cpp/natural_types.h>
#include <fidl/fuchsia.process/cpp/wire_types.h>
#include <lib/zx/result.h>

namespace driver_manager {

class BootupTracker;

// The started component from the perspective of the Component Framework.
struct StartedComponent {
  fuchsia_component_runner::ComponentStartInfo info;
  fidl::ServerEnd<fuchsia_component_runner::ComponentController> component_controller;
};

// Interface to be inherited by driver framework classes that can interact with a
// component controller.
class ComponentOwner {
 public:
  virtual ~ComponentOwner() = default;

  virtual void SetController(
      fidl::ClientEnd<fuchsia_component::Controller> component_controller) = 0;

  virtual void OnComponentStarted(const std::weak_ptr<BootupTracker>& bootup_tracker,
                                  const std::string& moniker,
                                  zx::result<StartedComponent> component) = 0;

  virtual void RequestStartComponent(fuchsia_process::wire::HandleInfo startup_handle,
                                     const std::string& moniker,
                                     const std::weak_ptr<BootupTracker>& bootup_tracker) = 0;

  virtual bool SkipInjectedOffers() const { return false; }

  virtual std::optional<fuchsia_component_sandbox::DictionaryRef> TakeDictionary() {
    return std::nullopt;
  }
};

}  // namespace driver_manager

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_COMPONENT_OWNER_H_

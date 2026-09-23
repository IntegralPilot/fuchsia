// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_RUNNER_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_RUNNER_H_

#include <fidl/fuchsia.component.decl/cpp/fidl.h>
#include <fidl/fuchsia.component.runner/cpp/fidl.h>
#include <fidl/fuchsia.component/cpp/fidl.h>
#include <fidl/fuchsia.driver.index/cpp/fidl.h>
#include <fidl/fuchsia.process/cpp/wire.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>

#include "src/devices/bin/driver_manager/bootup_tracker.h"
#include "src/devices/bin/driver_manager/node.h"
#include "src/devices/bin/driver_manager/offer_injection.h"

namespace driver_manager {

// This class serves as a runner for "driver" components. It also provides an API
// to allow driver components to be created in the current realm.
class Runner : public fidl::WireServer<fuchsia_component_runner::ComponentRunner> {
 public:
  Runner(async_dispatcher_t* dispatcher, fidl::WireClient<fuchsia_component::Realm> realm,
         fidl::WireClient<fuchsia_component::Introspector> introspector,
         OfferInjector offer_injector)
      : dispatcher_(dispatcher),
        realm_(std::move(realm)),
        introspector_(std::move(introspector)),
        offer_injector_(offer_injector) {}

  zx::result<> Publish(component::OutgoingDirectory& outgoing);

  void CreateDriverComponent(const std::shared_ptr<ComponentOwner>& owner,
                             fidl::ServerEnd<fuchsia_component::Controller> controller_request,
                             std::string_view moniker, std::string_view url,
                             std::string_view collection_name,
                             const std::vector<NodeOffer>& offers);

  void StartDriverComponent(const std::string& moniker);

  const fidl::WireClient<fuchsia_component::Realm>& realm() const { return realm_; }

  void SetBootupTracker(const std::weak_ptr<BootupTracker>& bootup_tracker) {
    bootup_tracker_ = bootup_tracker;
  }

 private:
  // fidl::WireServer<fuchsia_component_runner::ComponentRunner>
  void Start(StartRequestView request, StartCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_component_runner::ComponentRunner> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  zx::result<> CallCallback(const std::string& moniker, zx::result<StartedComponent> component);

  std::unordered_map<std::string, std::weak_ptr<ComponentOwner>> moniker_to_owner_;
  std::unordered_map<zx_koid_t, std::string> start_requests_;
  async_dispatcher_t* dispatcher_;
  fidl::WireClient<fuchsia_component::Realm> realm_;
  fidl::WireClient<fuchsia_component::Introspector> introspector_;
  OfferInjector offer_injector_;
  fidl::ServerBindingGroup<fuchsia_component_runner::ComponentRunner> bindings_;
  std::weak_ptr<BootupTracker> bootup_tracker_;
};
}  // namespace driver_manager

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_RUNNER_H_

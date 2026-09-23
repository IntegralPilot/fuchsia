// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_POWER_POWER_MANAGER_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_POWER_POWER_MANAGER_H_

#include <fidl/fuchsia.power.broker/cpp/fidl.h>
#include <fidl/fuchsia.power.broker/cpp/wire.h>
#include <fidl/fuchsia.power.system/cpp/fidl.h>
#include <fidl/fuchsia.power.system/cpp/wire.h>
#include <lib/async/dispatcher.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/fidl/cpp/client.h>
#include <lib/fit/defer.h>
#include <lib/fit/function.h>
#include <lib/zx/eventpair.h>

#include <memory>
#include <variant>
#include <vector>

#include "src/devices/bin/driver_manager/node_types.h"

namespace driver_manager {

class AllDriversElement;
class Node;

class PowerManager : public fidl::WireServer<fuchsia_power_broker::ElementRunner>,
                     public fidl::WireServer<fuchsia_power_system::CpuElementManager> {
 public:
  using CallbackSet = std::vector<fit::callback<void()>>;
  using PowerDependencyToken = fuchsia_power_broker::DependencyToken;

  PowerManager(
      async_dispatcher_t* dispatcher,
      fidl::ClientEnd<fuchsia_power_broker::Topology> power_topology,
      std::optional<fidl::ClientEnd<fuchsia_power_system::CpuElementManager>> cpu_element_mgr,
      bool wait_for_storage_token);

  // Starts the CPU token fetch process if CPU element manager is available.
  // Must be called after initialization.
  void FetchCpuToken();

  // Binds the CPU element manager to the outgoing directory.
  void PublishCpuElementManager(component::OutgoingDirectory& outgoing);

  // Registers the execution state dependency of all drivers with SAG.
  void AddCpuExecutionStateDependency(fuchsia_power_broker::DependencyToken dependency_token,
                                      fuchsia_power_broker::PowerLevel power_level);

  // Get a duplicate of the storage element token.
  std::optional<fuchsia_power_broker::DependencyToken> StorageElementToken();

  // Creates the storage power element under the given topology.
  // Calls |post_creation| callback once the dependency token is registered.
  void CreateStoragePowerElement(fuchsia_power_broker::DependencyToken driver_token,
                                 fuchsia_power_broker::PowerLevel power_level,
                                 fit::callback<void()> post_creation);

  // Checks if CPU token is available, or registers a callback.
  // Returns a duplicate of CPU token if available, std::nullopt otherwise.
  std::optional<fuchsia_power_broker::DependencyToken> GetCpuToken(
      fit::callback<void()> callback_if_not_available = {});

  // Checks if Storage token is available, or registers a callback.
  // Returns a duplicate of Storage token if available, std::nullopt otherwise.
  std::optional<fuchsia_power_broker::DependencyToken> GetStorageToken(
      fit::callback<void()> callback_if_not_available = {});

  // Checks if Storage token is available.
  // Returns a duplicate of Storage token if available, std::nullopt otherwise.
  std::optional<fuchsia_power_broker::DependencyToken> GetStorageTokenIfAvailable();

  // Get a copy of the CPU token (panics if not available).
  fuchsia_power_broker::DependencyToken GetCpuTokenOrAssert();

  // Get a copy of the Storage token (panics if not available).
  fuchsia_power_broker::DependencyToken GetStorageTokenOrAssert();

  void OnBootupComplete(std::shared_ptr<Node> root_node);
  void OnNodeBound(std::shared_ptr<const Node> node);
  void CreateAllDriversPowerElement(std::shared_ptr<Node> root_node);
  void LeaseAllDrivers(const std::shared_ptr<Node>& root_node, fit::callback<void()> callback);
  void CreatePowerElement(
      std::optional<fidl::ClientEnd<fuchsia_power_broker::Topology>> topology_client,
      std::string_view name, fuchsia_power_broker::DependencyToken element_token,
      std::vector<fuchsia_power_broker::DependencyToken> deps,
      fidl::ServerEnd<fuchsia_power_broker::ElementControl> control,
      fidl::ClientEnd<fuchsia_power_broker::ElementRunner> runner,
      fidl::ServerEnd<fuchsia_power_broker::Lessor> lessor, Collection for_collection,
      std::optional<fuchsia_power_broker::DependencyToken> cpu_token_override,
      std::optional<zx::eventpair> initial_lease_token, bool is_hermetic_power_test,
      fit::callback<void(zx::result<bool>)> cb);

  fidl::Client<fuchsia_power_broker::Topology>& power_topology() { return power_topology_; }
  bool SuspendEnabled() const { return power_topology_.is_valid(); }

  // fidl::WireServer<fuchsia_power_broker::ElementRunner>
  void SetLevel(SetLevelRequestView request, SetLevelCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_power_broker::ElementRunner> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  // fidl::WireServer<fuchsia_power_system::CpuElementManager>
  void GetCpuDependencyToken(GetCpuDependencyTokenCompleter::Sync& completer) override;
  void AddExecutionStateDependency(AddExecutionStateDependencyRequestView request,
                                   AddExecutionStateDependencyCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_power_system::CpuElementManager> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

 private:
  void AcquireRebootLease(const std::shared_ptr<Node>& node, std::string topo_path,
                          std::shared_ptr<fit::deferred_callback> deferred);

  async_dispatcher_t* const dispatcher_;
  fidl::Client<fuchsia_power_broker::Topology> power_topology_;

  fidl::ServerBindingGroup<fuchsia_power_broker::ElementRunner> storage_element_runner_;
  fidl::ServerBindingGroup<fuchsia_power_system::CpuElementManager> cpu_element_server_;

  std::optional<fidl::Client<fuchsia_power_system::CpuElementManager>> cpu_element_client_;
  std::variant<CallbackSet, PowerDependencyToken> cpu_callbacks_or_token_ = CallbackSet();

  std::variant<CallbackSet, PowerDependencyToken> storage_callbacks_or_token_ = CallbackSet();
  fidl::Client<fuchsia_power_broker::ElementControl> storage_control_;
  fidl::ClientEnd<fuchsia_power_broker::Lessor> storage_lessor_;

  bool wait_for_storage_token_from_driver_;

  std::optional<fuchsia_power_broker::DependencyToken> all_drivers_token_;
  std::shared_ptr<AllDriversElement> all_drivers_;
  std::vector<zx::eventpair> reboot_leases_;
};

}  // namespace driver_manager

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_POWER_POWER_MANAGER_H_

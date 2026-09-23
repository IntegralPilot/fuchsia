// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_DRIVER_RUNNER_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_DRIVER_RUNNER_H_

#include <fidl/fuchsia.component/cpp/wire.h>
#include <fidl/fuchsia.driver.crash/cpp/wire.h>
#include <fidl/fuchsia.driver.development/cpp/wire.h>
#include <fidl/fuchsia.driver.host/cpp/wire.h>
#include <fidl/fuchsia.driver.index/cpp/wire.h>
#include <fidl/fuchsia.driver.token/cpp/fidl.h>
#include <fidl/fuchsia.hardware.power.statecontrol/cpp/fidl.h>
#include <fidl/fuchsia.ldsvc/cpp/wire.h>
#include <fidl/fuchsia.power.broker/cpp/fidl.h>
#include <fidl/fuchsia.power.system/cpp/fidl.h>
#include <lib/async/cpp/wait.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/fit/function.h>
#include <lib/fpromise/promise.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/zircon-internal/thread_annotations.h>

#include <memory>
#include <unordered_map>
#include <unordered_set>
#include <vector>

#include <fbl/intrusive_double_list.h>

#include "fidl/fuchsia.power.broker/cpp/natural_types.h"
#include "src/devices/bin/driver_loader/loader.h"
#include "src/devices/bin/driver_manager/bind/bind_manager.h"
#include "src/devices/bin/driver_manager/bootup_tracker.h"
#include "src/devices/bin/driver_manager/composite/composite_manager_bridge.h"
#include "src/devices/bin/driver_manager/composite/composite_node_spec_manager.h"
#include "src/devices/bin/driver_manager/dictionary_util.h"
#include "src/devices/bin/driver_manager/driver_host.h"
#include "src/devices/bin/driver_manager/driver_host_runner.h"
#include "src/devices/bin/driver_manager/memory_attribution.h"
#include "src/devices/bin/driver_manager/node.h"
#include "src/devices/bin/driver_manager/offer_injection.h"
#include "src/devices/bin/driver_manager/pending_node_manager.h"
#include "src/devices/bin/driver_manager/power/power_manager.h"
#include "src/devices/bin/driver_manager/runner.h"
#include "src/devices/bin/driver_manager/shutdown/node_removal_tracker.h"
#include "src/devices/bin/driver_manager/shutdown/node_remover.h"
#include "src/devices/lib/log/log.h"

// Note, all of the logic here assumes we are operating on a single-threaded
// dispatcher. It is not safe to use a multi-threaded dispatcher with this code.

// TODO(https://fxbug.dev/479569256) Refactor DriverRunner to separate out power-related code
// with the goal of making things more maintainable and readable.
namespace driver_manager {
class Resource;

class DriverRunner : public fidl::WireServer<fuchsia_driver_framework::CompositeNodeManager>,
                     public fidl::WireServer<fuchsia_driver_framework::NodeManager>,
                     public fidl::WireServer<fuchsia_driver_index::DriverNotifier>,
                     public fidl::WireServer<fuchsia_driver_crash::CrashIntrospect>,
                     public fidl::Server<fuchsia_driver_token::NodeBusTopology>,
                     public fidl::WireServer<fuchsia_driver_token::Debug>,
                     public BindManagerBridge,
                     public CompositeManagerBridge,
                     public std::enable_shared_from_this<DriverRunner>,
                     public NodeManager,
                     public NodeRemover {
  using LoaderServiceFactory = fit::function<zx::result<fidl::ClientEnd<fuchsia_ldsvc::Loader>>()>;

  using DynamicLinkerServiceFactory =
      fit::function<zx::result<fidl::ClientEnd<fuchsia_driver_loader::DriverHostLauncher>>()>;

  using CallbackSet = std::vector<fit::callback<void()>>;

  using PowerDependencyToken = fuchsia_power_broker::DependencyToken;

 public:
  // Args required to enable dynamic linking
  struct DynamicLinkerArgs {
    DynamicLinkerServiceFactory linker_service_factory;
    std::unique_ptr<DriverHostRunner> driver_host_runner;
  };

  // |Dynamic_linker_args| should be set if dynamic linking is available.
  DriverRunner(fidl::ClientEnd<fuchsia_component::Realm> realm,
               fidl::ClientEnd<fuchsia_component::Introspector> introspector,
               fidl::ClientEnd<fuchsia_component_sandbox::CapabilityStore> capability_store,
               fidl::ClientEnd<fuchsia_driver_index::DriverIndex> driver_index,
               inspect::ComponentInspector& inspect, LoaderServiceFactory loader_service_factory,
               async_dispatcher_t* dispatcher, bool enable_test_shutdown_delays,
               OfferInjector offer_injector,
               fidl::ClientEnd<fuchsia_power_broker::Topology> topology_client,
               std::optional<DynamicLinkerArgs> dynamic_linker_args = std::nullopt,
               std::optional<fidl::ClientEnd<fuchsia_power_system::CpuElementManager>>
                   cpu_element_mgr = std::nullopt,
               bool wait_for_storage_token = false,
               std::optional<fidl::ClientEnd<fuchsia_hardware_power_statecontrol::Admin>>
                   statecontrol_admin = std::nullopt);

  // fidl::WireServer<fuchsia_driver_framework::CompositeNodeManager> interface
  void AddSpec(AddSpecRequestView request, AddSpecCompleter::Sync& completer) override;

  // fidl::WireServer<fuchsia_driver_framework::NodeManager> interface
  void AddNode(AddNodeRequestView request, AddNodeCompleter::Sync& completer) override;

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_driver_framework::NodeManager> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  // fidl::WireServer<fuchsia_driver_index::DriverNotifier>
  void NewDriverAvailable(NewDriverAvailableCompleter::Sync& completer) override;

  // fidl::WireServer<fuchsia_driver_crash::CrashIntrospect>
  void FindDriverCrash(FindDriverCrashRequestView request,
                       FindDriverCrashCompleter::Sync& completer) override;

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_driver_framework::CompositeNodeManager> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  // fidl::WireServer<fuchsia_driver_token::NodeBusTopology>
  void Get(GetRequest& request, GetCompleter::Sync& completer) override;

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_driver_token::NodeBusTopology> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  // fidl::WireServer<fuchsia_driver_token::Debug>
  void LogStackTrace(LogStackTraceRequestView request,
                     LogStackTraceCompleter::Sync& completer) override;
  void GetHostKoid(GetHostKoidRequestView request, GetHostKoidCompleter::Sync& completer) override;
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_driver_token::Debug> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override;

  // CompositeManagerBridge interface
  void BindNodesForCompositeNodeSpec() override;
  void AddSpecToDriverIndex(fuchsia_driver_framework::wire::CompositeNodeSpec group,
                            AddToIndexCallback callback) override;

  // NodeManager interface
  // Create a driver component with `url` against a given `node`.
  zx::result<> StartDriver(Node& node, std::string_view url,
                           fuchsia_driver_framework::DriverPackageType package_type) override;

  // NodeManager interface
  // Waits for boot to complete before invoking the callback.
  void WaitForBootup(fit::callback<void()> callback) override;

  // NodeManager interface
  // Shutdown hooks called by the shutdown manager
  void ShutdownAllDrivers(fit::callback<void()> callback) override {
    fdf_log::info("Driver Runner invokes shutdown all drivers");
    removal_tracker_.set_all_callback(std::move(callback));
    root_node_->Remove(RemovalSet::kAll, &removal_tracker_);
    removal_tracker_.FinishEnumeration();
  }

  void ShutdownPkgDrivers(fit::callback<void()> callback) override {
    removal_tracker_.set_pkg_callback(std::move(callback));
    root_node_->Remove(RemovalSet::kPackage, &removal_tracker_);
    removal_tracker_.FinishEnumeration();
  }

  void SetOnRemovalTimeoutCallback(fit::callback<void()> callback) override {
    removal_tracker_.SetOnRemovalTimeoutCallback(std::move(callback));
  }

  void LeaseAllDriversForShutdown(fit::callback<void()> callback) override;

  void RebindComposite(std::string spec, std::optional<std::string> driver_url,
                       fit::callback<void(zx::result<>)> callback) override;

  void RebindCompositesWithDriver(const std::string& url,
                                  fit::callback<void(size_t)> complete_callback);

  bool IsTestShutdownDelayEnabled() const override { return enable_test_shutdown_delays_; }
  std::weak_ptr<std::mt19937> GetShutdownTestRng() const override {
    return shutdown_test_delay_rng_;
  }

  void PublishComponentRunner(component::OutgoingDirectory& outgoing);
  zx::result<> StartRootDriver(std::string_view url);

  // Register a proxy driver called 'Devfs-Driver' that will advertise services that correspond
  // to the protocols offered by devfs class paths.  This call will start the driver
  // registration, but that registration will not be complete until the component framework
  // calls the AddChild callback.  That callback will then update |devfs| with an outgoing
  // directory and a ComponentController. This function should only be called once when the
  // driver manager is starting, and will no longer be needed when devfs migration is complete.
  void StartDevfsDriver(std::shared_ptr<driver_manager::Devfs>& devfs);

  // Goes through the orphan list and attempts the bind them again. Sends nodes that are still
  // orphaned back to the orphan list. Tracks the result of the bindings and then when finished
  // uses the result_callback to report the results.
  void TryBindAllAvailable(
      NodeBindingInfoResultCallback result_callback =
          [](fidl::VectorView<fuchsia_driver_development::wire::NodeBindingInfo>) {});

  // Restarts all the nodes that are colocated with a driver with the given |url|.
  zx::result<uint32_t> RestartNodesColocatedWithDriverUrl(
      std::string_view url, fuchsia_driver_development::RestartRematchFlags rematch_flags);

  void RestartWithDictionary(fidl::StringView moniker,
                             fuchsia_component_sandbox::wire::DictionaryRef dictionary,
                             zx::eventpair reset_eventpair);

  /// Restarts driver components bound to the node identified by |moniker|, providing
  /// custom power dependencies, CPU token override, and targeted child node power token overrides.
  /// Releasing |release_fence| reverts all temporary overrides and restarts the drivers again.
  void RestartWithDictionaryAndPowerDependencies(
      std::string moniker, fuchsia_component_sandbox::DictionaryRef dictionary,
      std::vector<fuchsia_power_broker::LevelDependency> power_dependencies,
      std::optional<zx::event> cpu_token_override,
      std::vector<fuchsia_driver_development::NodePowerTokenOverride> node_power_token_overrides,
      zx::eventpair release_fence);

  std::unordered_set<DriverHost*> DriverHostsWithDriverUrl(std::string_view url);

  fpromise::promise<inspect::Inspector> Inspect() const;

  bool SuspendEnabled() override { return power_manager_ && power_manager_->SuspendEnabled(); }

  std::vector<fuchsia_driver_development::wire::CompositeNodeInfo> GetCompositeListInfo(
      fidl::AnyArena& arena) const;

  fidl::WireClient<fuchsia_driver_index::DriverIndex>& driver_index() { return driver_index_; }

  std::shared_ptr<Node> root_node() const { return root_node_; }

  PowerManager* power_manager() { return power_manager_.get(); }

  // Only exposed for testing.
  void BootupDoneForTesting() { bootup_tracker_->BootupDoneForTesting(); }
  CompositeNodeSpecManager& composite_node_spec_manager() { return composite_node_spec_manager_; }
  const BindManager& bind_manager() const { return bind_manager_; }
  driver_manager::Runner& runner_for_tests() { return runner_; }

  driver_manager::DriverHostRunner* driver_host_runner_for_tests() {
    return dynamic_linker_args_.has_value() ? dynamic_linker_args_->driver_host_runner.get()
                                            : nullptr;
  }

  const fbl::DoublyLinkedList<std::unique_ptr<DriverHostComponent>>& driver_hosts() const {
    return driver_hosts_;
  }

  std::optional<fuchsia_power_broker::DependencyToken> StorageElementToken() override;

  void RebootSystem() override;

 private:
  // NodeManager interface.
  // Attempt to bind `node`. A nullptr for result_tracker is acceptable if the caller doesn't
  // intend to track the results.
  void Bind(Node& node, std::shared_ptr<BindResultTracker> result_tracker) override;
  void Bind(Resource& resource, std::shared_ptr<BindResultTracker> result_tracker) override;
  void BindToUrl(Node& node, std::string_view driver_url_suffix,
                 std::shared_ptr<BindResultTracker> result_tracker) override;
  DriverHost* GetDriverHost(std::string_view driver_host_name_for_colocation) override;
  zx::result<DriverHost*> CreateDriverHost(
      bool use_next_vdso, std::string_view driver_host_name_for_colocation) override;
  // Creates the driver host component, loads the driver host using dynamic linking,
  // and calls |cb| on completion. |cb| will only be called if the return value is zx::ok.
  void CreateDriverHostDynamicLinker(
      std::string_view driver_host_name_for_colocation,
      fit::callback<void(zx::result<DriverHost*>)> completion_cb) override;
  void DestroyDriverHostComponent(std::string_view driver_host_name_for_colocation,
                                  fit::callback<void(zx::result<>)> completion_cb) override;
  bool IsDriverHostValid(DriverHost* driver_host) const override;
  void TryResolvePendingNodes() override;

  DictionaryUtil& dictionary_util() override { return dictionary_util_; }
  MemoryAttributor& memory_attributor() override { return memory_attributor_; }

  ResourceId GetNextResourceId() override;

  // BindManagerBridge interface.
  zx::result<std::string> StartDriver(
      Node& node, fuchsia_driver_framework::wire::DriverInfo driver_info) override;
  zx::result<BindSpecResult> BindToParentSpec(fidl::AnyArena& arena,
                                              CompositeParents composite_parents,
                                              std::weak_ptr<Resource> resource,
                                              bool enable_multibind) override;
  void RequestMatchFromDriverIndex(
      fuchsia_driver_index::wire::MatchDriverArgs args,
      fit::callback<void(fidl::WireUnownedResult<fuchsia_driver_index::DriverIndex::MatchDriver>&)>
          match_callback) override;
  void RequestMatchPendingNode(
      fidl::VectorView<fuchsia_driver_framework::wire::ParentSpec2> dependencies,
      fit::callback<
          void(fidl::WireUnownedResult<fuchsia_driver_index::DriverIndex::MatchPendingNode>&)>
          match_callback) override;
  void RequestRebindFromDriverIndex(std::string spec, std::optional<std::string> driver_url_suffix,
                                    fit::callback<void(zx::result<>)> callback) override;

  void OnBindingStateChanged() override { bootup_tracker_->NotifyBindingChanged(); }
  void OnNodeBound(std::shared_ptr<const Node> node) override {
    if (power_manager_) {
      power_manager_->OnNodeBound(std::move(node));
    }
  }

  zx::result<> CreateDriverHostComponent(std::string moniker,
                                         fidl::ServerEnd<fuchsia_io::Directory> exposed_dir,
                                         std::shared_ptr<bool> exposed_dir_connected,
                                         bool use_next_vdso);
  void CreatePowerElement(
      std::optional<fidl::ClientEnd<fuchsia_power_broker::Topology>> topology_client,
      std::string_view name, fuchsia_power_broker::DependencyToken element_token,
      std::vector<fuchsia_power_broker::DependencyToken> deps,
      fidl::ServerEnd<fuchsia_power_broker::ElementControl> control,
      fidl::ClientEnd<fuchsia_power_broker::ElementRunner> runner,
      fidl::ServerEnd<fuchsia_power_broker::Lessor> lessor, Collection for_collection,
      std::optional<fuchsia_power_broker::DependencyToken> cpu_token_override,
      std::optional<zx::eventpair> initial_lease_token, bool is_hermetic_power_test,
      fit::callback<void(zx::result<bool>)> cb) override;

  void OnBootupComplete();

  uint64_t next_driver_host_id_ = 0;
  ResourceId next_resource_id_ = 0;
  fidl::WireClient<fuchsia_driver_index::DriverIndex> driver_index_;
  LoaderServiceFactory loader_service_factory_;
  DictionaryUtil dictionary_util_;
  fidl::ServerBindingGroup<fuchsia_driver_framework::CompositeNodeManager> manager_bindings_;
  fidl::ServerBindingGroup<fuchsia_driver_framework::NodeManager> node_manager_bindings_;
  fidl::ServerBindingGroup<fuchsia_driver_crash::CrashIntrospect> crash_introspect_bindings_;
  fidl::ServerBindingGroup<fuchsia_driver_token::NodeBusTopology> bus_topo_bindings_;
  fidl::ServerBindingGroup<fuchsia_driver_index::DriverNotifier> driver_notifier_bindings_;
  async_dispatcher_t* const dispatcher_;
  std::shared_ptr<Node> root_node_;

  // Manages pending nodes.
  PendingNodeManager pending_node_manager_;

  // Manages composite node specs.
  CompositeNodeSpecManager composite_node_spec_manager_;

  // Manages driver binding.
  BindManager bind_manager_;

  driver_manager::Runner runner_;

  NodeRemovalTracker removal_tracker_;

  std::shared_ptr<BootupTracker> bootup_tracker_;

  fbl::DoublyLinkedList<std::unique_ptr<DriverHostComponent>> driver_hosts_;

  // True if the driver manager should inject test delays in the shutdown process. Set by the
  // structured config.
  bool enable_test_shutdown_delays_;

  // RNG engine for the shutdown test delays. For reproducibility reasons, only one engine
  // should be used.
  std::shared_ptr<std::mt19937> shutdown_test_delay_rng_;

  // Set if dynamic linking is available.
  std::optional<DynamicLinkerArgs> dynamic_linker_args_;

  // TODO(https://fxbug.dev/349831408): for now we use the same dynamic linker client
  // channel for each driver host.
  std::optional<fidl::WireSharedClient<fuchsia_driver_loader::DriverHostLauncher>>
      driver_host_launcher_;

  std::unique_ptr<PowerManager> power_manager_;

  MemoryAttributor memory_attributor_;

  fidl::Client<fuchsia_hardware_power_statecontrol::Admin> statecontrol_admin_;
  std::unordered_map<std::string, std::vector<fit::callback<void(zx::result<>)>>>
      pending_driver_host_destructions_;
};

Collection ToCollection(const Node& node, fuchsia_driver_framework::DriverPackageType package_type);

}  // namespace driver_manager

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_DRIVER_RUNNER_H_

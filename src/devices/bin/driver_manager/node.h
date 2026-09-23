// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_NODE_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_NODE_H_

#include <fidl/fuchsia.component.runner/cpp/wire.h>
#include <fidl/fuchsia.driver.development/cpp/fidl.h>
#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.driver.host/cpp/wire.h>
#include <fidl/fuchsia.driver.index/cpp/wire.h>
#include <fidl/fuchsia.power.broker/cpp/fidl.h>
#include <lib/zx/event.h>
#include <zircon/assert.h>
#include <zircon/types.h>

#include <list>
#include <memory>
#include <unordered_map>
#include <utility>
#include <variant>

#include "src/devices/bin/driver_manager/bind/bind_result_tracker.h"
#include "src/devices/bin/driver_manager/component_owner.h"
#include "src/devices/bin/driver_manager/controller_allowlist_passthrough.h"
#include "src/devices/bin/driver_manager/devfs/devfs.h"
#include "src/devices/bin/driver_manager/dictionary_util.h"
#include "src/devices/bin/driver_manager/driver_host.h"
#include "src/devices/bin/driver_manager/memory_attribution.h"
#include "src/devices/bin/driver_manager/node_types.h"
#include "src/devices/bin/driver_manager/shutdown/node_removal_tracker.h"
#include "src/devices/bin/driver_manager/shutdown/node_shutdown_coordinator.h"

namespace driver_manager {

// Definitions for OfferTransport, NodeOffer, ToFidl, and CreateCompositeOffer are now in
// node_types.h

class Node;
struct NodeInfo;
class NodeRemovalTracker;
class BootupTracker;
class Resource;
struct PowerElementStartArgs;

class DriverHostConnection : public fidl::WireAsyncEventHandler<fuchsia_driver_host::Driver> {
 public:
  explicit DriverHostConnection(Node* node) : node_(node) {}

  void on_fidl_error(fidl::UnbindInfo info) override;

 private:
  Node* node_;
};

class ComponentControllerConnection
    : public fidl::WireAsyncEventHandler<fuchsia_component::Controller> {
 public:
  explicit ComponentControllerConnection(Node* node) : node_(node) {}

  void on_fidl_error(fidl::UnbindInfo info) override;

  void handle_unknown_event(
      fidl::UnknownEventMetadata<fuchsia_component::Controller> metadata) override {}

 private:
  Node* node_;
};

using AddNodeResultCallback =
    fit::callback<void(fit::result<fuchsia_driver_framework::NodeError, std::shared_ptr<Node>>)>;

using OnBindWaitCompleter =
    fit::callback<void(zx::result<fuchsia_driver_framework::wire::DriverResult>)>;

struct PowerElementHandles {
  fidl::Client<fuchsia_power_broker::ElementControl> element_control;
  fidl::ServerEnd<fuchsia_power_broker::ElementRunner> element_runner;
  fidl::Client<fuchsia_power_broker::Lessor> lessor;
};

class NodeManager {
 public:
  virtual ~NodeManager() = default;

  // Attempt to bind `node`.
  // A nullptr for result_tracker is acceptable if the caller doesn't intend to
  // track the results.
  virtual void Bind(Node& node, std::shared_ptr<BindResultTracker> result_tracker) = 0;
  virtual void Bind(Resource& resource, std::shared_ptr<BindResultTracker> result_tracker) = 0;

  virtual void BindToUrl(Node& node, std::string_view driver_url_suffix,
                         std::shared_ptr<BindResultTracker> result_tracker) {
    ZX_PANIC("Unimplemented BindToUrl");
  }

  virtual zx::result<> StartDriver(Node& node, std::string_view url,
                                   fuchsia_driver_framework::DriverPackageType package_type) {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  virtual void OnNodeBound(std::shared_ptr<const Node> node) {}

  virtual DriverHost* GetDriverHost(std::string_view driver_name_name_for_colocation) = 0;

  virtual zx::result<DriverHost*> CreateDriverHost(
      bool use_next_vdso, std::string_view driver_name_name_for_colocation) = 0;

  // Creates the driver host component, loads the driver host using dynamic linking,
  // and calls |cb| on completion or error.
  virtual void CreateDriverHostDynamicLinker(
      std::string_view driver_name_name_for_colocation,
      fit::callback<void(zx::result<DriverHost*>)> completion_cb) {
    completion_cb(zx::error(ZX_ERR_NOT_SUPPORTED));
  }

  // Destroys the driver host component asynchronously. Calls |cb| on completion.
  virtual void DestroyDriverHostComponent(std::string_view driver_host_name_for_colocation,
                                          fit::callback<void(zx::result<>)> completion_cb) {
    completion_cb(zx::error(ZX_ERR_NOT_SUPPORTED));
  }

  // DriverHost lifetimes are managed through a linked list, and they will delete themselves
  // when the FIDL connection is closed. Currently in the Node class we store a raw pointer to the
  // DriverHost object, and do not have a way to remove it from the class when the underlying
  // DriverHost object is deallocated. This function will return true if the underlying DriverHost
  // object is still alive and in the linked list. Otherwise returns false.
  virtual bool IsDriverHostValid(DriverHost* driver_host) const { return true; }

  virtual void RebindComposite(std::string spec, std::optional<std::string> driver_url,
                               fit::callback<void(zx::result<>)> callback) {}

  virtual void TryResolvePendingNodes() {}

  virtual bool IsTestShutdownDelayEnabled() const { return false; }
  virtual std::weak_ptr<std::mt19937> GetShutdownTestRng() const {
    return std::weak_ptr<std::mt19937>();
  }

  virtual void WaitForBootup(fit::callback<void()> callback) { callback(); }

  virtual void ImportDictionary(fuchsia_component_sandbox::DictionaryRef dictionary,
                                fit::callback<void(zx::result<uint64_t>)> callback) {
    callback(zx::error(ZX_ERR_NOT_SUPPORTED));
  }

  virtual void RequestMatchPendingNode(
      fidl::VectorView<fuchsia_driver_framework::wire::ParentSpec2> dependencies,
      fit::callback<
          void(fidl::WireUnownedResult<fuchsia_driver_index::DriverIndex::MatchPendingNode>&)>
          match_callback) {
    ZX_PANIC("Unimplemented RequestMatchPendingNode");
  }

  // Create a power element where |element_token| is the access token for the newly created
  // element. |deps| are the tokens this element should depend on.
  // |is_hermetic_power_test| indicates whether power dependencies or token overrides are active.
  // |cb| is called with:
  //   - `zx::ok(true)` if the power element was successfully created
  //   - `zx::ok(false)` if the power element could not be created because this is not a suspend-
  //     enabled platform.
  //   - `zx::error` if an error happens creating the element on a suspend-enabled platform.
  virtual void CreatePowerElement(
      std::optional<fidl::ClientEnd<fuchsia_power_broker::Topology>> topology_client,
      std::string_view name, fuchsia_power_broker::DependencyToken element_token,
      std::vector<fuchsia_power_broker::DependencyToken> deps,
      fidl::ServerEnd<fuchsia_power_broker::ElementControl> control,
      fidl::ClientEnd<fuchsia_power_broker::ElementRunner> runner,
      fidl::ServerEnd<fuchsia_power_broker::Lessor> lessor, Collection for_collection,
      std::optional<fuchsia_power_broker::DependencyToken> cpu_token_override,
      std::optional<zx::eventpair> initial_lease_token, bool is_hermetic_power_test,
      fit::callback<void(zx::result<bool>)> cb) {
    cb(zx::error(ZX_ERR_NOT_SUPPORTED));
  }

  virtual DictionaryUtil& dictionary_util() { ZX_PANIC("Unimplemented dictionary_util"); }

  virtual bool SuspendEnabled() { ZX_PANIC("Unimplemented SuspendEnabled"); }

  virtual std::optional<fuchsia_power_broker::DependencyToken> StorageElementToken() {
    return std::nullopt;
  }

  virtual MemoryAttributor& memory_attributor() { ZX_PANIC("Unimplemented memory_attributor"); }

  virtual ResourceId GetNextResourceId() { return 0; }

  virtual void RebootSystem() {}
};

class Node : public fidl::WireServer<fuchsia_driver_framework::NodeController>,
             public fidl::WireServer<fuchsia_driver_framework::Node>,
             public fidl::WireServer<fuchsia_component_runner::ComponentController>,
             public fidl::WireServer<fuchsia_device::Controller>,
             public std::enable_shared_from_this<Node>,
             public NodeShutdownBridge,
             public ComponentOwner {
  friend DriverHostConnection;
  friend ComponentControllerConnection;
  friend class DriverRunner;
  friend class PendingNodeManager;

 public:
  Node(std::string_view name, std::weak_ptr<Node> parent, NodeManager* node_manager,
       async_dispatcher_t* dispatcher, std::map<std::string, zx::event> node_token_overrides = {});
  Node(std::string_view name, std::unordered_map<std::string, std::weak_ptr<Node>> parents,
       std::vector<std::string> parents_names, NodeManager* node_manager,
       async_dispatcher_t* dispatcher, uint32_t primary_index,
       std::map<std::string, zx::event> node_token_overrides = {});

  ~Node() override;

  static zx::result<std::shared_ptr<Node>> CreateCompositeNode(
      std::string_view node_name, std::vector<std::weak_ptr<Resource>> dependencies,
      std::vector<std::string> parents_names,
      const std::vector<fuchsia_driver_framework::NodePropertyEntry2>& parent_properties,
      NodeManager* driver_binder, async_dispatcher_t* dispatcher,
      std::string_view driver_host_name_for_colocation, uint32_t primary_index = 0,
      std::map<std::string, zx::event> node_token_overrides = {});

  // This is called when |node_ref_| is unbound from the dispatcher.
  void OnNodeServerUnbound(fidl::UnbindInfo info);

  // NodeShutdownBridge
  // Exposed for testing.
  bool HasDriverComponent() const override {
    auto* driver_component = std::get_if<DriverComponent>(&state_);
    return driver_component && driver_component->state != DriverState::kStopped;
  }

  void OnBind();
  void OnMatchError(zx_status_t status);
  void OnStartError(zx_status_t status);

  void SearchNamespaceSvcDirForEntry(
      fidl::ClientEnd<fuchsia_io::Directory> svc_dir, std::string_view entry_name,
      fit::callback<void(zx::result<fidl::ClientEnd<fuchsia_io::Directory>>)> cb);

  bool HasDriverComponentController() const override { return component_controller_.is_valid(); }

  void RemoveResource(const std::shared_ptr<Resource>& resource);

  void InitializeSelfResource(
      std::vector<fuchsia_driver_framework::NodeProperty2> properties = {},
      std::vector<NodeOffer> offers = {},
      std::optional<fuchsia_driver_framework::BusInfo> bus_info = std::nullopt);

  // Begin the removal process for a Node. This function ensures that a Node is
  // only removed after all of its children are removed. It also ensures that
  // a Node is only removed after the driver that is bound to it has been stopped.
  // This is safe to call multiple times.
  // There are multiple reasons a Node's removal will be started:
  //   - The system is being stopped.
  //   - The Node had an unexpected error or disconnect
  // During a system stop, Remove is expected to be called twice:
  // once with |removal_set| == kPackage, and once with |removal_set| == kAll.
  // Errors and disconnects that are unrecoverable should call Remove(kAll, nullptr).
  void Remove(RemovalSet removal_set, NodeRemovalTracker* removal_tracker);

  // `callback` is invoked once the node has finished being added or an error
  // has occurred.
  void AddChild(fuchsia_driver_framework::NodeAddArgs args,
                fidl::ServerEnd<fuchsia_driver_framework::NodeController> controller,
                fidl::ServerEnd<fuchsia_driver_framework::Node> node,
                AddNodeResultCallback callback);

  // Add this Node to its parents. This should be called when the node is created. Exposed for
  // testing.
  void AddToParents();

  void SetController(fidl::ServerEnd<fuchsia_driver_framework::NodeController> controller);
  void SetOwnedByParent(fidl::ServerEnd<fuchsia_driver_framework::Node> node_ref);

  // Begins the process of restarting the node. Restarting a node includes stopping and removing
  // all children nodes, stopping the driver that is bound to the node, and asking the NodeManager
  // to bind the node again. The restart operation is very similar to the Remove operation, the
  // difference being once the children are removed, and the driver stopped, we don't remove the
  // node from the topology but instead bind the node again.
  void RestartNode();

  // Begins the process of quarantining the node. This is basically performing a Remove,
  // but instead of removing the node from the topology, we keep it in a stopped state so that it
  // can be orphaned if its driver is ever disabled. That way new drivers can be bound to the node.
  void QuarantineNode();

  void RemoveCompositeNodeForRebind(fit::callback<void(zx::result<>)> completer);

  // Restarting a node WithRematch, means that instead of re-using the currently bound driver,
  // another MatchDriver call will be made into the driver index to find a new driver to bind.
  void RestartNodeWithRematch(std::optional<std::string> restart_driver_url_suffix,
                              fit::callback<void(zx::result<>)> completer);
  void RestartNodeWithRematch();

  void StartDriver(
      fuchsia_component_runner::wire::ComponentStartInfo start_info,
      fidl::ServerEnd<fuchsia_component_runner::ComponentController> component_controller,
      fit::callback<void(zx::result<>)> cb);

  // ComponentOwner
  void SetController(fidl::ClientEnd<fuchsia_component::Controller> component_controller) override;
  void OnComponentStarted(const std::weak_ptr<BootupTracker>& bootup_tracker,
                          const std::string& moniker,
                          zx::result<StartedComponent> component) override;
  void RequestStartComponent(fuchsia_process::wire::HandleInfo startup_handle,
                             const std::string& moniker,
                             const std::weak_ptr<BootupTracker>& bootup_tracker) override;

  bool IsComposite() const { return std::holds_alternative<Composite>(type_); }

  // Evaluates the given rematch_flags against the node. Returns true if rematch should take place,
  // false otherwise. Rematching is done based on the node type and url both matching:
  // For node type, if the node is a composite, the rematch flags must contain the flag
  // for the composite variant that the node is. No validation for non-composites.
  // For the url, rematch takes place if either:
  //  - the url matches the requested_url and the 'requested' flag is available.
  //  - the url does not match and the 'non_requested' flag is available.
  bool EvaluateRematchFlags(fuchsia_driver_development::RestartRematchFlags rematch_flags,
                            std::string_view requested_url) const;

  // Creates the node's topological path by combining each primary parent's name together,
  // separated by '/'.
  // E.g: dev/sys/topo/path
  std::string MakeTopologicalPath(bool deduplicate = false) const;

  // Make the node's component moniker by making the topological path and then replacing
  // characters not allowed by the component framework.
  // E.g: dev.sys.topo.path
  std::string MakeComponentMoniker() const;

  // Returns a duplicate of the node's power element token, if available.
  // If the node has no power token, this will return an invalid zx::event.
  zx::event DuplicatePowerToken() const {
    if (!power_element_token_.is_valid()) {
      return zx::event();
    }
    zx::event dupe;
    zx_status_t dupe_result = power_element_token_.duplicate(ZX_RIGHT_SAME_RIGHTS, &dupe);
    ZX_ASSERT_MSG(dupe_result == ZX_OK, "Element token duplication failed.");
    return dupe;
  }

  std::optional<zx::eventpair> TakeStartupLease() { return std::move(startup_lease_); }

  // Exposed for testing.
  void set_startup_lease_for_testing(std::optional<zx::eventpair> lease) {
    startup_lease_ = std::move(lease);
  }

  // Exposed for testing.
  Node* GetPrimaryParent() const {
    if (auto* composite = std::get_if<Composite>(&type_); composite) {
      if (composite->primary_node_index_ < composite->parents_.size()) {
        return composite->parents_[composite->primary_node_index_].lock().get();
      }
      return nullptr;
    }
    auto parent = std::get<Normal>(type_).parent_.lock();
    return parent ? parent.get() : nullptr;
  }

  // This should be used on the root node. Install the root node at the top of the devfs filesystem.
  void SetupDevfsForRootNode(std::shared_ptr<Devfs>& devfs) {
    devfs = std::make_shared<Devfs>(devfs_device_.topological_node(), dispatcher_);
  }

  // This is exposed for testing. Setup this node's devfs nodes.
  void AddToDevfsForTesting(Devnode& parent) {
    parent.add_child(name_, std::nullopt, Devnode::Target(), devfs_device_);
  }

  void SetShouldDestroy();

  // Invoked when a bind sequence has been completed. It allows us to reply to outstanding bind
  // requests that may have originated from the node.
  void CompleteBind(zx::result<> result);

  NodeShutdownCoordinator& GetNodeShutdownCoordinator();

  NodeState GetNodeState() { return GetNodeShutdownCoordinator().node_state(); }

  const std::string& name() const { return name_; }

  NodeType type() const {
    return std::holds_alternative<Normal>(type_) ? NodeType::kNormal : NodeType::kComposite;
  }

  const DriverHost* driver_host() const {
    if (node_manager_.has_value() && node_manager_.value()->IsDriverHostValid(*driver_host_)) {
      return *driver_host_;
    }

    return nullptr;
  }

  DriverHost* driver_host() {
    if (node_manager_.has_value() && node_manager_.value()->IsDriverHostValid(*driver_host_)) {
      return *driver_host_;
    }

    return nullptr;
  }

  const std::string& driver_url() const;

  bool quarantined() const { return std::holds_alternative<Quarantined>(state_); }

  std::span<const std::weak_ptr<Node>> parents() const {
    if (IsComposite()) {
      return std::get<Composite>(type_).parents_;
    }
    if (auto& parent = std::get<Normal>(type_).parent_; parent.lock() != nullptr) {
      return std::span(&parent, 1);
    }
    return {};
  }

  const std::list<std::shared_ptr<Node>>& children() const { return children_; }

  const std::vector<NodeOffer>& offers() const;

  const std::vector<fuchsia_driver_framework::NodeSymbol>& symbols() const { return symbols_; }

  // Returns the node properties of the node and its parents if the node is a composite node.
  // See properties of the Composite struct for more info.
  size_t properties_size() const {
    if (IsComposite()) {
      return std::get<Composite>(type_).properties_.size();
    }
    return GetNodeProperties().has_value() ? 1 : 0;
  }

  // Returns the node properties of the node or the node's parent if the node is a composite node.
  // Returns std::nullopt if the node is a non-composite and `parent_name` is not "default".
  // Returns std::nullopt if the parent node cannot be found.
  // See `properties_` property for more info.
  std::optional<std::vector<fuchsia_driver_framework::NodeProperty2>> GetNodeProperties(
      std::string_view parent_name = "default") const;

  fuchsia_driver_framework::NodePropertyDictionary2 GetNodePropertyDict() const;

  void PrepareDictionary(fit::callback<void(zx::result<>)> callback);

  void SetSubtreeDictionaryRef(
      std::optional<fuchsia_component_sandbox::CapabilityId> subtree_dictionary_ref) {
    if (subtree_dictionary_ref.has_value()) {
      ZX_ASSERT_MSG(!dictionary_ref_.has_value(), "cannot use SubtreeDictionaryRef on this node");
    }
    subtree_dictionary_ref_ = subtree_dictionary_ref;
  }

  void set_dictionary_ref(std::optional<fuchsia_component_sandbox::CapabilityId> ref) {
    dictionary_ref_ = ref;
  }

  /// Sets the power dependency overrides for this node. These overrides will be
  /// used instead of the default power dependencies when the driver is started.
  /// This is primarily intended for use in tests via the driver restart flow,
  /// allowing tests to provide isolated or custom dependencies.
  void SetPowerDependencyOverrides(std::optional<std::vector<fuchsia_power_broker::LevelDependency>>
                                       power_dependency_overrides) {
    power_dependency_overrides_ = std::move(power_dependency_overrides);
  }

  void MarkAsCompositeParent() { state_ = CompositeParent{}; }

  void UnmarkAsCompositeParent() { state_ = Unbound{}; }

  bool HasSubtreeDictionaryRef() const { return subtree_dictionary_ref_.has_value(); }

  bool SkipInjectedOffers() const override { return HasSubtreeDictionaryRef(); }

  std::optional<fuchsia_component_sandbox::DictionaryRef> TakeDictionary() override {
    std::optional<fuchsia_component_sandbox::DictionaryRef> temp;
    std::swap(temp, offers_dictionary_);
    return temp;
  }

  /// Sets the CPU token override for this node. This token will be used to
  /// create a dependency on the CPU power element when the driver is started.
  /// Like SetPowerDependencyOverrides, this is primarily meant for test scenarios.
  void SetCpuTokenOverride(std::optional<zx::event> token) {
    cpu_token_override_ = std::move(token);
  }

  /// Sets power token overrides for target child/composite nodes.
  void SetNodeTokenOverrides(std::map<std::string, zx::event> node_token_overrides) {
    node_token_overrides_ = std::move(node_token_overrides);
    auto override_it = node_token_overrides_.find(name_);
    if (override_it != node_token_overrides_.end() && override_it->second.is_valid()) {
      zx::event injected;
      ZX_ASSERT(ZX_OK == override_it->second.duplicate(ZX_RIGHT_SAME_RIGHTS, &injected));
      power_element_token_ = std::move(injected);
      return;
    }

    override_it = node_token_overrides_.find(MakeComponentMoniker());
    if (override_it != node_token_overrides_.end() && override_it->second.is_valid()) {
      zx::event injected;
      ZX_ASSERT(ZX_OK == override_it->second.duplicate(ZX_RIGHT_SAME_RIGHTS, &injected));
      power_element_token_ = std::move(injected);
      return;
    }
  }

  /// Creates a duplicate copy of the given map of node power token overrides.
  static std::map<std::string, zx::event> DuplicateTokenOverrides(
      const std::map<std::string, zx::event>& overrides);

  const Collection& collection() const { return collection_; }

  const fuchsia_driver_framework::DriverPackageType& driver_package_type() const {
    return driver_package_type_;
  }

  DevfsDevice& devfs_device() { return devfs_device_; }

  bool can_multibind_composites() const { return can_multibind_composites_; }

  /// Returns true if this node or subtree is operating within a hermetic power test,
  /// indicated by active dictionary, power dependency, CPU token, or node power token overrides.
  bool IsHermeticPowerTest() const {
    return subtree_dictionary_ref_.has_value() || power_dependency_overrides_.has_value() ||
           cpu_token_override_.has_value() || !node_token_overrides_.empty();
  }

  void set_collection(Collection collection) { collection_ = collection; }

  void set_driver_package_type(fuchsia_driver_framework::DriverPackageType driver_package_type) {
    driver_package_type_ = driver_package_type;
  }

  void set_symbols(std::vector<fuchsia_driver_framework::NodeSymbol> symbols) {
    symbols_ = std::move(symbols);
  }

  void set_can_multibind_composites(bool can_multibind_composites) {
    can_multibind_composites_ = can_multibind_composites;
  }

  void set_driver_host_name_for_colocation(std::string_view name) {
    driver_host_name_for_colocation_ = name;
  }

  std::optional<zx_koid_t> token_koid() const {
    auto* driver_component = std::get_if<DriverComponent>(&state_);
    return driver_component ? std::optional(driver_component->component_instance_koid)
                            : std::nullopt;
  }
  std::vector<fuchsia_driver_framework::BusInfo> GetBusTopology() const;

  ShutdownIntent shutdown_intent() { return GetNodeShutdownCoordinator().shutdown_intent(); }

  bool is_bound() const {
    if (is_bound_override_.has_value()) {
      return *is_bound_override_;
    }
    return std::holds_alternative<DriverComponent>(state_);
  }

  // Exposed for testing.
  const std::vector<std::shared_ptr<Resource>>& provided_resources() const {
    return provided_resources_;
  }
  void add_provided_resource_for_testing(std::shared_ptr<Resource> resource) {
    provided_resources_.push_back(std::move(resource));
  }

  std::optional<std::shared_ptr<Resource>> GetSelfResource() const;

  // Exposed for testing.
  void set_bound_for_testing(bool bound) { is_bound_override_ = bound; }

 private:
  struct DriverComponent {
    DriverComponent(
        Node& node, std::string url,
        fidl::ServerEnd<fuchsia_component_runner::ComponentController> component_controller,
        fidl::ServerEnd<fuchsia_driver_framework::Node> node_server,
        fidl::ClientEnd<fuchsia_driver_host::Driver> driver, zx::event component_instance);

    // This represents the Driver Component instance within the Component Framework.
    // When we send the OnStop event through it, that signals to the Component Framework
    // that this driver component instance has stopped.
    fidl::ServerBinding<fuchsia_component_runner::ComponentController>
        runner_component_controller_ref;
    fidl::ServerBinding<fuchsia_driver_framework::Node> node_ref_;
    fidl::WireClient<fuchsia_driver_host::Driver> driver;
    std::string driver_url;
    zx::event component_instance;
    zx_koid_t component_instance_koid = 0;

    DriverState state = DriverState::kBinding;
  };

  struct EnumValue {
    std::string value;
  };
  using PropertyValue = std::variant<std::monostate, uint32_t, std::string, bool, EnumValue>;

  struct Property {
    std::string key;
    PropertyValue value;
  };

  struct PropertiesEntry {
    std::string name;
    std::vector<Property> properties;
  };

  // fidl::WireServer<fuchsia_device::Controller>
  void ConnectToDeviceFidl(ConnectToDeviceFidlRequestView request,
                           ConnectToDeviceFidlCompleter::Sync& completer) override;
  void ConnectToController(ConnectToControllerRequestView request,
                           ConnectToControllerCompleter::Sync& completer) override;
  void Bind(BindRequestView request, BindCompleter::Sync& completer) override;
  void Rebind(RebindRequestView request, RebindCompleter::Sync& completer) override;
  void UnbindChildren(UnbindChildrenCompleter::Sync& completer) override;
  void ScheduleUnbind(ScheduleUnbindCompleter::Sync& completer) override;
  void GetTopologicalPath(GetTopologicalPathCompleter::Sync& completer) override;

  void OnDriverHostFidlError(fidl::UnbindInfo info);
  void OnComponentControllerFidlError(fidl::UnbindInfo info);
  void RestartNodeOnCrash();

  // fidl::WireServer<fuchsia_component_runner::ComponentController>
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_component_runner::ComponentController> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  void Stop(StopCompleter::Sync& completer) override;
  void Kill(KillCompleter::Sync& completer) override;

  // fidl::WireServer<fuchsia_driver_framework::NodeController>
  void Remove(RemoveCompleter::Sync& completer) override;
  void RequestBind(RequestBindRequestView request, RequestBindCompleter::Sync& completer) override;
  void WaitForDriver(WaitForDriverCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_driver_framework::NodeController> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  // fidl::WireServer<fuchsia_driver_framework::Node>
  void AddChild(AddChildRequestView request, AddChildCompleter::Sync& completer) override;
  void ProvideResource(
      fidl::WireServer<fuchsia_driver_framework::Node>::ProvideResourceRequestView request,
      fidl::WireServer<fuchsia_driver_framework::Node>::ProvideResourceCompleter::Sync& completer)
      override;
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_driver_framework::Node> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override;

  // NodeShutdownBridge
  NodeInfo GetRemovalTrackerInfo() override;
  void StopDriver() override;
  void StopDriverComponent() override;
  bool MaybeDestroyDriverComponent() override;
  void FinishShutdown(fit::callback<void()> shutdown_callback) override;
  bool HasChildren() const override { return !children_.empty(); }
  bool HasDriver() const override {
    return std::holds_alternative<DriverComponent>(state_) &&
           std::get<DriverComponent>(state_).driver;
  }

  bool IsPendingBind() const override {
    auto* driver_component = std::get_if<DriverComponent>(&state_);
    if (!driver_component) {
      return false;
    }
    return driver_component->state == DriverState::kBinding;
  }

  void BindHelper(bool force_rebind, std::optional<std::string> driver_url_suffix,
                  fit::callback<void(zx_status_t)> on_bind_complete);

  // Shutdown helpers:
  // Remove a child from this parent
  void RemoveChild(const std::shared_ptr<Node>& child);

  // Start the node's driver back up.
  void FinishRestart();

  // Called to wrap things up before going to the quarantine state.
  void FinishQuarantine();

  // Clear out the values associated with the driver on the driver host.
  void ClearHostDriver();

  // Call `callback` once child node with the name `name` has been removed.
  // Returns an error if a child node with the name `name` exists and is not in
  // the process of being removed.
  void WaitForChildToExit(
      std::string_view name,
      fit::callback<void(fit::result<fuchsia_driver_framework::NodeError>)> callback);

  std::shared_ptr<BindResultTracker> CreateBindResultTracker(bool silent = false);

  void AddChildHelper(fuchsia_driver_framework::NodeAddArgs args,
                      fidl::ServerEnd<fuchsia_driver_framework::NodeController> controller,
                      fidl::ServerEnd<fuchsia_driver_framework::Node> node,
                      AddNodeResultCallback callback);

  // Creates a passthrough for the associated devfs node that will connect to
  // the device controller of this node if no connector provided, or the connection
  // type is not supported.
  Devnode::Target CreateDevfsPassthrough(
      std::optional<fidl::ClientEnd<fuchsia_device_fs::Connector>> connector,
      std::optional<fidl::ClientEnd<fuchsia_device_fs::Connector>> controller_connector,
      bool allow_controller_connection, const std::string& class_name);

  zx_status_t ConnectDeviceInterface(zx::channel channel);

  static std::vector<Property> ToProperty(
      std::span<const fuchsia_driver_framework::NodeProperty2> properties);
  static std::vector<fuchsia_driver_framework::NodeProperty2> PropertyToFidl(
      std::span<const Property> properties);

  // Set properties to composite node properties containing a clone of the node properties of
  // `parents_`.
  void SetCompositeParentProperties(
      const fuchsia_driver_framework::NodePropertyDictionary2& parent_properties);

  // Update `properties_dict_` to identify the contents of `properties_`.
  void StartDriverWithDynamicLinker(
      DriverHost::DriverLoadArgs load_args, DriverHost::DriverStartArgs start_args,
      std::string_view url,
      fidl::ServerEnd<fuchsia_component_runner::ComponentController> component_controller,
      PowerElementStartArgs power_element_args, fit::callback<void(zx::result<>)> cb);

  // Creates a driver host, if necessary, and then starts the driver with the dynamic linker. If
  // |colocate| is true a driver host is not created because we use an existing one.
  void CreateHostAndStartDriverWithDynamicLinker(
      DriverHost::DriverLoadArgs load_args, DriverHost::DriverStartArgs start_args,
      std::string_view url,
      fidl::ServerEnd<fuchsia_component_runner::ComponentController> component_controller,
      PowerElementStartArgs power_element_args, bool found_driver_host,
      fit::callback<void(zx::result<>)> cb);

  zx::result<zx::event> DuplicateNodeToken();

  // Return the handle ID of the power token. This should only be used for
  // examining handle identity, not for any actual operations on the Zircon
  // object.
  zx_handle_t GetPowerTokenHandle() { return power_element_token_.get(); }

  std::string name_;

  struct Normal {
    std::weak_ptr<Node> parent_;
  };

  struct Composite {
    std::vector<std::weak_ptr<Node>> parents_;
    std::vector<std::string> parents_names_;
    uint32_t primary_index_;
    uint32_t primary_node_index_;
    std::vector<PropertiesEntry> properties_;
  };

  std::variant<Normal, Composite> type_;

  std::list<std::shared_ptr<Node>> children_;
  fit::nullable<NodeManager*> node_manager_;
  async_dispatcher_t* const dispatcher_;

  // TODO(https://fxbug.dev/42073492): Set this flag from NodeAddArgs.
  bool can_multibind_composites_ = true;

  bool host_restart_on_crash_ = false;
  bool is_driver_host_root_ = false;

  std::vector<fuchsia_driver_framework::NodeSymbol> symbols_;

  std::vector<std::shared_ptr<Resource>> received_resources_;
  std::vector<std::shared_ptr<Resource>> provided_resources_;
  std::shared_ptr<Resource> self_resource_;

  // A component framework dictionary that should be provided to the driver
  // that binds to this node.
  // This ref will become an offers_dictionary_ in PrepareDictionary before the driver starts.
  //
  // The difference between |subtree_dictionary_ref_| and |dictionary_ref_| is the subtree one
  // is always passed down the node tree to children, as it contains non-driver protocol
  // capabilities for testing (the ones injected in offer_injection). This is only used for system
  // testing at the moment through |RestartWithDictionary|. |dictionary_ref_| is the node specific
  // dictionary that contains offers that the node provides that are type |kDictionaryOffer|.
  // These two cannot be used together as it is.
  std::optional<fuchsia_component_sandbox::CapabilityId> subtree_dictionary_ref_;
  std::optional<fuchsia_component_sandbox::CapabilityId> dictionary_ref_;
  std::optional<fuchsia_component_sandbox::DictionaryRef> offers_dictionary_;
  std::list<DirReceiverImpl> dir_receivers_;

  Collection collection_ = Collection::kNone;
  fuchsia_driver_framework::DriverPackageType driver_package_type_;
  // If specified by the parent in NodeAddArgs, this will be used to determine which driver host to
  // load into.
  std::string driver_host_name_for_colocation_;
  fit::nullable<DriverHost*> driver_host_;

  /// Resets the power handles for this node. This should be called when the
  /// power element needs to be recreated.
  void ResetPowerHandles() { pe_handles_ = std::nullopt; }

  std::optional<std::vector<fuchsia_power_broker::LevelDependency>> power_dependency_overrides_;
  std::optional<zx::event> cpu_token_override_;
  /// Power element token overrides keyed by node name/moniker for targeting child or composite
  /// nodes.
  std::map<std::string, zx::event> node_token_overrides_;

  // An outstanding rebind request.
  fit::callback<void(zx::result<>)> pending_bind_completer_;

  // Set by RemoveCompositeNodeForRebind(). Invoked when the node finished shutting down.
  fit::callback<void(zx::result<>)> composite_rebind_completer_;

  std::optional<std::string> restart_driver_url_suffix_;

  // Invoked when the node has been fully removed.
  fit::callback<void()> remove_complete_callback_;
  bool should_destroy_ = false;
  fidl::WireClient<fuchsia_component::Controller> component_controller_;

  struct Unbound {};
  struct Starting {
    std::string driver_url;
  };
  struct OwnedByParent {
    explicit OwnedByParent(fidl::ServerEnd<fuchsia_driver_framework::Node> node, Node* child);
    fidl::ServerBinding<fuchsia_driver_framework::Node> node_ref_;
  };
  struct CompositeParent {};
  struct Quarantined {
    std::string driver_url;
  };
  // Valid State transitions:
  // * Unbound -> Starting, OwnedByParent, CompositeParent
  // * Starting -> Unbound, DriverComponent, Quarantined
  // * OwnedByParent -> Unbound
  // * CompositeParent -> Unbound
  // * DriverComponent -> Unbound, Quarantined
  std::variant<Unbound, Starting, OwnedByParent, CompositeParent, DriverComponent, Quarantined>
      state_;

  std::optional<fidl::ServerBinding<fuchsia_driver_framework::NodeController>> controller_ref_;
  OnBindWaitCompleter bind_wait_completer_;
  std::optional<fuchsia_driver_framework::wire::DriverResult> bind_err_;

  std::unique_ptr<NodeShutdownCoordinator> node_shutdown_coordinator_;

  // This represents the node's presence in devfs, both it's topological path and it's class path.
  DevfsDevice devfs_device_;

  // Connector to service exported to devfs
  std::optional<fidl::ClientEnd<fuchsia_device_fs::Connector>> devfs_connector_;
  // Connector to fuchsia_device/Controller exported to devfs.
  // This is only used by the compat shim to override the node's own controller
  // implementation.
  fidl::ServerBindingGroup<fuchsia_device::Controller> dev_controller_bindings_;
  std::unique_ptr<ControllerAllowlistPassthrough> controller_allowlist_passthrough_;

  // Completers that are waiting for the node to unbind all of its children.
  std::vector<UnbindChildrenCompleter::Async> unbinding_children_completers_;

  // Handlers for FIDL connections.
  DriverHostConnection driver_host_handler_;
  ComponentControllerConnection component_controller_handler_;

  fuchsia_power_broker::DependencyToken power_element_token_;
  std::optional<PowerElementHandles> pe_handles_;
  std::optional<zx::eventpair> startup_lease_;
  std::optional<bool> is_bound_override_;
};

}  // namespace driver_manager

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_NODE_H_

// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/driver_runner.h"

#include <fidl/fuchsia.component.sandbox/cpp/common_types_format.h>
#include <fidl/fuchsia.driver.development/cpp/wire.h>
#include <fidl/fuchsia.driver.host/cpp/wire.h>
#include <fidl/fuchsia.driver.index/cpp/wire.h>
#include <fidl/fuchsia.driver.token/cpp/wire.h>
#include <fidl/fuchsia.power.broker/cpp/fidl.h>
#include <fidl/fuchsia.power.system/cpp/fidl.h>
#include <fidl/fuchsia.process/cpp/wire.h>
#include <lib/async/cpp/task.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/directory.h>
#include <lib/fit/defer.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/status.h>

#include <forward_list>
#include <memory>
#include <optional>
#include <queue>
#include <random>
#include <stack>
#include <utility>

#include "fidl/fuchsia.power.broker/cpp/natural_types.h"
#include "src/devices/bin/driver_manager/async_sharder.h"
#include "src/devices/bin/driver_manager/composite/composite_node_spec.h"
#include "src/devices/bin/driver_manager/node_property_conversion.h"
#include "src/devices/bin/driver_manager/resource.h"
#include "src/devices/lib/log/log.h"
#include "src/lib/fxl/strings/join_strings.h"

namespace fdf {

using namespace fuchsia_driver_framework;
}
namespace fdh = fuchsia_driver_host;
namespace fdd = fuchsia_driver_development;
namespace fdi = fuchsia_driver_index;
namespace fio = fuchsia_io;
namespace frunner = fuchsia_component_runner;
namespace fcomponent = fuchsia_component;
namespace fdecl = fuchsia_component_decl;
namespace fpower = fuchsia_hardware_power_statecontrol;

using InspectStack = std::stack<std::pair<inspect::Node*, const driver_manager::Node*>>;

namespace driver_manager {

namespace {

constexpr auto kBootScheme = "fuchsia-boot://";
constexpr std::string_view kRootDeviceName = "dev";

void InspectNode(inspect::Inspector& inspector, InspectStack& stack) {
  std::forward_list<inspect::Node> roots;
  std::unordered_set<const Node*> unique_nodes;
  while (!stack.empty()) {
    // Pop the current root and node to operate on.
    auto [root, node] = stack.top();
    stack.pop();

    auto [_, inserted] = unique_nodes.insert(node);
    if (!inserted) {
      // Only insert unique nodes from the DAG.
      continue;
    }

    // Populate root with data from node.
    if (const auto& offers = node->offers(); !offers.empty()) {
      auto array = root->CreateStringArray("offers", offers.size());
      for (size_t i = 0; i < offers.size(); i++) {
        array.Set(i, offers[i].service_name);
      }
      root->Record(std::move(array));
    }
    if (auto symbols = node->symbols(); !symbols.empty()) {
      auto array = root->CreateStringArray("symbols", symbols.size());
      for (size_t i = 0; i < symbols.size(); i++) {
        array.Set(i, symbols[i].name().value());
      }
      root->Record(std::move(array));
    }
    if (auto properties = node->GetNodeProperties(); properties && !properties->empty()) {
      root->RecordChild("properties", [&](inspect::Node& properties_array) {
        for (uint32_t i = 0; i < properties->size(); ++i) {
          properties_array.RecordChild(std::to_string(i), [&](inspect::Node& inspect_property) {
            auto& property = properties.value()[i];
            inspect_property.RecordString("key", property.key());

            if (const auto& str_prop = property.value().string_value(); str_prop.has_value()) {
              inspect_property.RecordString("value", str_prop.value());

            } else if (const auto& int_prop = property.value().int_value(); int_prop.has_value()) {
              inspect_property.RecordUint("value", int_prop.value());

            } else if (const auto& enum_prop = property.value().enum_value();
                       enum_prop.has_value()) {
              inspect_property.RecordString("value", enum_prop.value());

            } else if (const auto& bool_prop = property.value().bool_value();
                       bool_prop.has_value()) {
              inspect_property.RecordBool("value", bool_prop.value());

            } else {
              inspect_property.RecordString("value", "UNKNOWN VALUE TYPE");
            }
          });
        }
      });
    }

    root->RecordString("type", node->IsComposite() ? "Composite Device" : "Device");
    root->RecordString("topological_path", node->MakeTopologicalPath());

    root->RecordString("driver", node->driver_url());

    // Push children of this node onto the stack. We do this in reverse order to
    // ensure the children are handled in order, from first to last.
    auto& children = node->children();
    for (auto child = children.rbegin(), end = children.rend(); child != end; ++child) {
      auto& name = (*child)->name();
      auto& root_for_child = roots.emplace_front(root->CreateChild(name));
      stack.emplace(&root_for_child, child->get());
    }
  }

  // Store all of the roots in the inspector.
  for (auto& root : roots) {
    inspector.GetRoot().Record(std::move(root));
  }
}

fidl::StringView CollectionName(Collection collection) {
  switch (collection) {
    case Collection::kNone:
      return {};
    case Collection::kBoot:
      return "boot-drivers";
    case Collection::kPackage:
      return "base-drivers";
    case Collection::kFullPackage:
      return "full-drivers";
  }
}

Collection ToCollection(fdf::DriverPackageType package) {
  switch (package) {
    case fdf::DriverPackageType::kBoot:
      return Collection::kBoot;
    case fdf::DriverPackageType::kBase:
      return Collection::kPackage;
    case fdf::DriverPackageType::kCached:
    case fdf::DriverPackageType::kUniverse:
      return Collection::kFullPackage;
    default:
      return Collection::kNone;
  }
}

// Choose the highest ranked collection between `collection` and `node`'s
// parents. If one of `node`'s parent's collection is none then check the
// parent's parents and so on.
Collection GetHighestRankingCollection(const Node& node, Collection collection) {
  std::stack<std::weak_ptr<Node>> ancestors;
  for (const auto& parent : node.parents()) {
    ancestors.emplace(parent);
  }

  // Find the highest ranked collection out of `node`'s parent nodes. If a
  // node's collection is none then check that node's parents and so on.
  while (!ancestors.empty()) {
    auto ancestor = ancestors.top();
    ancestors.pop();
    auto ancestor_ptr = ancestor.lock();
    if (!ancestor_ptr) {
      fdf_log::warn("Ancestor node released");
      continue;
    }

    auto ancestor_collection = ancestor_ptr->collection();
    if (ancestor_collection == Collection::kNone) {
      // Check ancestor's parents to see what the collection of the ancestor
      // should be.
      for (const auto& parent : ancestor_ptr->parents()) {
        ancestors.emplace(parent);
      }
    } else if (ancestor_collection > collection) {
      collection = ancestor_collection;
    }
  }

  return collection;
}

// Specifies the direction of traversal for the search helper.
enum class Direction : uint8_t {
  // Traverse downwards through children (e.g. from parent to descendants).
  kDown,
  // Traverse upwards through parents (e.g. from child to ancestors).
  kUp,
};

// Perform a Breadth-First-Search (BFS) over the node topology starting from |starting_node|,
// applying the visitor function on the node being visited.
// |direction| controls whether we traverse downwards (children) or upwards (parents).
// The return value of the visitor function is a boolean for whether the children/parents of
// the node should be visited. If it returns false, the traversal of that branch is pruned.
void PerformBFS(const std::shared_ptr<Node>& starting_node, Direction direction,
                fit::function<bool(const std::shared_ptr<driver_manager::Node>&)> visitor) {
  if (!starting_node) {
    return;
  }

  std::unordered_set<const Node*> visited;
  std::queue<std::shared_ptr<Node>> node_queue;
  visited.insert(starting_node.get());
  node_queue.push(starting_node);

  while (!node_queue.empty()) {
    auto current = std::move(node_queue.front());
    node_queue.pop();

    if (!visitor(current)) {
      continue;
    }

    if (direction == Direction::kDown) {
      for (const auto& child : current->children()) {
        if (child->GetPrimaryParent() != current.get()) {
          continue;
        }

        if (visited.insert(child.get()).second) {
          node_queue.push(child);
        }
      }
    } else {
      for (const auto& parent_weak : current->parents()) {
        if (auto parent = parent_weak.lock()) {
          if (visited.insert(parent.get()).second) {
            node_queue.push(std::move(parent));
          }
        }
      }
    }
  }
}

// Performs a downward BFS over the node topology starting from |starting_node|.
void PerformBFS(const std::shared_ptr<Node>& starting_node,
                fit::function<bool(const std::shared_ptr<driver_manager::Node>&)> visitor) {
  PerformBFS(starting_node, Direction::kDown, std::move(visitor));
}

void CallStartDriverOnRunner(Runner& runner, Node& node, const std::string& moniker,
                             std::string_view url,
                             const std::shared_ptr<BootupTracker>& bootup_tracker) {
  if (!node.HasDriverComponentController()) {
    auto [controller_client, controller_request] =
        fidl::Endpoints<fcomponent::Controller>::Create();
    node.SetController(std::move(controller_client));
    runner.CreateDriverComponent(node.shared_from_this(), std::move(controller_request), moniker,
                                 url, CollectionName(node.collection()).get(), node.offers());
  } else {
    runner.StartDriverComponent(moniker);
  }
}

// Exists in fsl, but perhaps a bit of duplication is better than a bit of dependency.
zx_koid_t GetKoid(const zx::event& handle) {
  zx_info_handle_basic_t info;
  zx_status_t status =
      zx_object_get_info(handle.get(), ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  return status == ZX_OK ? info.koid : ZX_KOID_INVALID;
}

}  // namespace

Collection ToCollection(const Node& node, fdf::DriverPackageType package_type) {
  Collection collection = ToCollection(package_type);
  return GetHighestRankingCollection(node, collection);
}

DriverRunner::DriverRunner(
    fidl::ClientEnd<fcomponent::Realm> realm,
    fidl::ClientEnd<fcomponent::Introspector> introspector,
    fidl::ClientEnd<fuchsia_component_sandbox::CapabilityStore> capability_store,
    fidl::ClientEnd<fdi::DriverIndex> driver_index, inspect::ComponentInspector& inspect,
    LoaderServiceFactory loader_service_factory, async_dispatcher_t* dispatcher,
    bool enable_test_shutdown_delays, OfferInjector offer_injector,
    fidl::ClientEnd<fuchsia_power_broker::Topology> topology_client,
    std::optional<DynamicLinkerArgs> dynamic_linker_args,
    std::optional<fidl::ClientEnd<fuchsia_power_system::CpuElementManager>> cpu_element_mgr,
    bool wait_for_storage_token_from_driver_,
    std::optional<fidl::ClientEnd<fpower::Admin>> statecontrol_admin)
    : driver_index_(std::move(driver_index), dispatcher),
      loader_service_factory_(std::move(loader_service_factory)),
      dictionary_util_(std::move(capability_store), dispatcher),
      dispatcher_(dispatcher),
      root_node_(std::make_shared<Node>(kRootDeviceName, std::weak_ptr<Node>{}, this, dispatcher)),
      pending_node_manager_(this, dispatcher),
      composite_node_spec_manager_(this),
      bind_manager_(this, this, dispatcher),
      runner_(dispatcher, fidl::WireClient(std::move(realm), dispatcher),
              fidl::WireClient(std::move(introspector), dispatcher), offer_injector),
      removal_tracker_(dispatcher),
      enable_test_shutdown_delays_(enable_test_shutdown_delays),
      dynamic_linker_args_(std::move(dynamic_linker_args)),
      memory_attributor_(dispatcher_) {
  root_node_->InitializeSelfResource();
  if (enable_test_shutdown_delays_) {
    // TODO(https://fxbug.dev/42084497): Allow the seed to be set from the configuration.
    auto seed = std::chrono::system_clock::now().time_since_epoch().count();
    fdf_log::info("Shutdown test delays enabled. Using seed {}", seed);
    shutdown_test_delay_rng_ = std::make_shared<std::mt19937>(static_cast<uint32_t>(seed));
  }

  inspect.root().RecordLazyNode("driver_runner", [this] { return Inspect(); });

  // Pick a non-zero starting id so that folks cannot rely on the driver host process names being
  // stable.
  std::random_device rd;
  std::mt19937 gen(rd());
  std::uniform_int_distribution<> distrib(0, 1000);
  next_driver_host_id_ = distrib(gen);

  bootup_tracker_ = std::make_shared<BootupTracker>(&bind_manager_, dispatcher);
  runner_.SetBootupTracker(bootup_tracker_);

  // Setup the driver notifier.
  auto [notifier_client, notifier_server] =
      fidl::Endpoints<fuchsia_driver_index::DriverNotifier>::Create();
  driver_notifier_bindings_.AddBinding(dispatcher_, std::move(notifier_server), this,
                                       fidl::kIgnoreBindingClosure);
  fidl::OneWayStatus status = driver_index_->SetNotifier(std::move(notifier_client));
  if (!status.ok()) {
    fdf_log::warn("Failed to set the driver notifier: {}", status.status_string());
  }

  if (statecontrol_admin.has_value()) {
    statecontrol_admin_ =
        fidl::Client<fpower::Admin>(std::move(statecontrol_admin.value()), dispatcher_);
  }

  power_manager_ = std::make_unique<PowerManager>(dispatcher_, std::move(topology_client),
                                                  std::move(cpu_element_mgr),
                                                  wait_for_storage_token_from_driver_);
}

// fidl::WireServer<fuchsia_driver_token::Debug>
void DriverRunner::LogStackTrace(LogStackTraceRequestView request,
                                 LogStackTraceCompleter::Sync& completer) {
  const zx_koid_t node_token_koid = GetKoid(request->node_token);
  if (node_token_koid == ZX_KOID_INVALID) {
    fdf_log::error("provided node token is not valid");
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
  }

  std::shared_ptr<const Node> node = nullptr;
  PerformBFS(
      root_node_,
      [&node, node_token_koid](const std::shared_ptr<driver_manager::Node>& current) -> bool {
        if (node != nullptr) {
          // Already found it.
          return false;
        }
        std::optional current_koid = current->token_koid();
        if (current_koid && current_koid.value() == node_token_koid) {
          node = current;
          return false;
        }
        return true;
      });
  if (node == nullptr) {
    completer.ReplyError(ZX_ERR_NOT_FOUND);
    fdf_log::warn("no such node: node_token_koid={}", node_token_koid);
    return;
  }
  const DriverHost* host = node->driver_host();
  if (host == nullptr) {
    completer.ReplyError(ZX_ERR_NOT_FOUND);
    fdf_log::warn("node has no host: node_token_koid={}", node_token_koid);
    return;
  }
  fdf_log::info("stack trace requested for host: node_token_koid={}", node_token_koid);
  host->TriggerStackTrace();
  completer.ReplySuccess();
}

void DriverRunner::GetHostKoid(GetHostKoidRequestView request,
                               GetHostKoidCompleter::Sync& completer) {
  const zx_koid_t node_token_koid = GetKoid(request->node_token);
  if (node_token_koid == ZX_KOID_INVALID) {
    fdf_log::error("provided node token is not valid");
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  std::shared_ptr<const Node> node = nullptr;
  PerformBFS(
      root_node_,
      [&node, node_token_koid](const std::shared_ptr<driver_manager::Node>& current) -> bool {
        if (node != nullptr) {
          // Already found it.
          return false;
        }
        std::optional current_koid = current->token_koid();
        if (current_koid && current_koid.value() == node_token_koid) {
          node = current;
          return false;
        }
        return true;
      });
  if (node == nullptr) {
    completer.ReplyError(ZX_ERR_NOT_FOUND);
    fdf_log::warn("no such node: node_token_koid={}", node_token_koid);
    return;
  }
  const DriverHost* host = node->driver_host();
  if (host == nullptr) {
    completer.ReplyError(ZX_ERR_NOT_FOUND);
    fdf_log::warn("node has no host: node_token_koid={}", node_token_koid);
    return;
  }

  host->GetProcessKoidAsync([completer = completer.ToAsync(),
                             node_token_koid](zx::result<uint64_t> host_koid_res) mutable {
    if (host_koid_res.is_error()) {
      completer.ReplyError(host_koid_res.status_value());
      fdf_log::warn("node host has no koid: node_token_koid={}, status={}", node_token_koid,
                    host_koid_res.status_string());
      return;
    }

    completer.ReplySuccess(host_koid_res.value());
  });
}

void DriverRunner::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_driver_token::Debug> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf_log::warn("Unknown Debug request: {}", metadata.method_ordinal);
}

void DriverRunner::BindNodesForCompositeNodeSpec() { TryBindAllAvailable(); }

void DriverRunner::AddSpec(AddSpecRequestView request, AddSpecCompleter::Sync& completer) {
  if (!request->has_name() || (!request->has_parents() && !request->has_parents2())) {
    completer.Reply(fit::error(fdf::CompositeNodeSpecError::kMissingArgs));
    return;
  }

  if (!request->has_parents() && !request->has_parents2()) {
    completer.Reply(fit::error(fdf::CompositeNodeSpecError::kDuplicateParents));
    return;
  }

  std::vector<fuchsia_driver_framework::ParentSpec2> parents;
  if (request->has_parents()) {
    if (request->parents().empty()) {
      completer.Reply(fit::error(fdf::CompositeNodeSpecError::kEmptyNodes));
      return;
    }
    auto to_parent_spec2 = [](const auto& parent) {
      auto parent_spec = fidl::ToNatural(parent);
      std::vector<fuchsia_driver_framework::BindRule2> bind_rules;
      std::transform(parent_spec.bind_rules().begin(), parent_spec.bind_rules().end(),
                     std::back_inserter(bind_rules), ToBindRule2);

      std::vector<fuchsia_driver_framework::NodeProperty2> properties;
      std::transform(parent_spec.properties().begin(), parent_spec.properties().end(),
                     std::back_inserter(properties),
                     [](const auto& prop) { return ToProperty2(prop); });
      return fuchsia_driver_framework::ParentSpec2{{
          .bind_rules = std::move(bind_rules),
          .properties = std::move(properties),
      }};
    };

    std::transform(request->parents().cbegin(), request->parents().cend(),
                   std::back_inserter(parents), to_parent_spec2);
  }

  if (request->has_parents2()) {
    if (request->parents2().empty()) {
      completer.Reply(fit::error(fdf::CompositeNodeSpecError::kEmptyNodes));
      return;
    }
    parents = fidl::ToNatural(request->parents2()).value();
  }

  auto spec = std::make_unique<CompositeNodeSpec>(
      CompositeNodeSpecCreateInfo{
          .name = std::string(request->name().get()),
          .parents = std::move(parents),
          .driver_host_name_for_colocation = request->has_driver_host()
                                                 ? std::string(request->driver_host().get())
                                                 : std::string(),
      },
      dispatcher_, this);
  composite_node_spec_manager_.AddSpec(
      *request, std::move(spec),
      [completer = completer.ToAsync()](
          fit::result<fuchsia_driver_framework::CompositeNodeSpecError> result) mutable {
        completer.Reply(result);
      });
}

void DriverRunner::FindDriverCrash(FindDriverCrashRequestView request,
                                   FindDriverCrashCompleter::Sync& completer) {
  for (const DriverHostComponent& host : driver_hosts_) {
    zx::result process_koid = host.GetProcessKoid();
    if (process_koid.is_ok() && process_koid.value() == request->process_koid) {
      host.GetCrashInfo(
          request->thread_koid,
          [this, async_completer = completer.ToAsync()](
              zx::result<fuchsia_driver_host::DriverCrashInfo> info_result) mutable {
            if (info_result.is_error()) {
              async_completer.ReplyError(info_result.error_value());
              return;
            }
            fuchsia_driver_host::DriverCrashInfo& found = info_result.value();
            zx_info_handle_basic_t info;
            zx_status_t status = found.node_token()->get_info(ZX_INFO_HANDLE_BASIC, &info,
                                                              sizeof(info), nullptr, nullptr);
            if (status != ZX_OK) {
              async_completer.ReplyError(ZX_ERR_INTERNAL);
              return;
            }

            const Node* node = nullptr;
            PerformBFS(root_node_, [&node, token_koid = info.koid](
                                       const std::shared_ptr<driver_manager::Node>& current) {
              if (node != nullptr) {
                // Already found it.
                return false;
              }
              std::optional current_koid = current->token_koid();
              if (current_koid && current_koid.value() == token_koid) {
                node = current.get();
                return false;
              }
              return true;
            });
            if (node == nullptr) {
              async_completer.ReplyError(ZX_ERR_NOT_FOUND);
              return;
            }

            fidl::Arena arena;
            async_completer.ReplySuccess(fuchsia_driver_crash::wire::DriverCrashInfo::Builder(arena)
                                             .node_moniker(arena, node->MakeComponentMoniker())
                                             .url(arena, found.url().value())
                                             .Build());
          });
      return;
    }
  }
  completer.ReplyError(ZX_ERR_NOT_FOUND);
}

void DriverRunner::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_driver_framework::CompositeNodeManager> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  std::string method_type;
  switch (metadata.unknown_method_type) {
    case fidl::UnknownMethodType::kOneWay:
      method_type = "one-way";
      break;
    case fidl::UnknownMethodType::kTwoWay:
      method_type = "two-way";
      break;
  };

  fdf_log::warn("CompositeNodeManager received unknown {} method. Ordinal: {}", method_type,
                metadata.method_ordinal);
}

void DriverRunner::Get(GetRequest& request,
                       fidl::Completer<fidl::internal::NaturalCompleterBase<
                           fuchsia_driver_token::NodeBusTopology::Get>>::Sync& completer) {
  zx_info_handle_basic_t info;
  zx_status_t status =
      request.token().get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  if (status != ZX_OK) {
    completer.Reply(zx::error(status));
    return;
  }
  const Node* node = nullptr;
  PerformBFS(root_node_,
             [&node, token_koid = info.koid](const std::shared_ptr<driver_manager::Node>& current) {
               if (node != nullptr) {
                 // Already found it.
                 return false;
               }
               std::optional current_koid = current->token_koid();
               if (current_koid && current_koid.value() == token_koid) {
                 node = current.get();
                 return false;
               }
               return true;
             });
  if (node == nullptr) {
    completer.Reply(zx::error(ZX_ERR_NOT_FOUND));
    return;
  }

  completer.Reply(zx::ok(node->GetBusTopology()));
}

void DriverRunner::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_driver_token::NodeBusTopology> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  std::string method_type;
  switch (metadata.unknown_method_type) {
    case fidl::UnknownMethodType::kOneWay:
      method_type = "one-way";
      break;
    case fidl::UnknownMethodType::kTwoWay:
      method_type = "two-way";
      break;
  };

  fdf_log::warn("NodeBusTopology received unknown {} method. Ordinal: {}", method_type,
                metadata.method_ordinal);
}

void DriverRunner::AddSpecToDriverIndex(fuchsia_driver_framework::wire::CompositeNodeSpec group,
                                        AddToIndexCallback callback) {
  driver_index_->AddCompositeNodeSpec(group).Then(
      [callback = std::move(callback)](
          fidl::WireUnownedResult<fdi::DriverIndex::AddCompositeNodeSpec>& result) mutable {
        if (!result.ok()) {
          fdf_log::error("DriverIndex::AddCompositeNodeSpec failed {}", result.status());
          callback(zx::error(result.status()));
          return;
        }

        if (result->is_error()) {
          callback(result->take_error());
          return;
        }

        callback(zx::ok());
      });
}

fpromise::promise<inspect::Inspector> DriverRunner::Inspect() const {
  // Create our inspector.
  inspect::Inspector inspector(inspect::InspectSettings{.maximum_size = 4 * 256 * 1024});

  // Make the device tree inspect nodes.
  auto device_tree = inspector.GetRoot().CreateChild("node_topology");
  auto root = device_tree.CreateChild(root_node_->name());
  InspectStack stack{{std::make_pair(&root, root_node_.get())}};
  InspectNode(inspector, stack);
  device_tree.Record(std::move(root));
  inspector.GetRoot().Record(std::move(device_tree));

  bind_manager_.RecordInspect(inspector);
  composite_node_spec_manager_.RecordInspect(inspector);

  return fpromise::make_ok_promise(inspector);
}

std::vector<fdd::wire::CompositeNodeInfo> DriverRunner::GetCompositeListInfo(
    fidl::AnyArena& arena) const {
  auto spec_composite_list = composite_node_spec_manager_.GetCompositeInfo(arena);
  auto list = bind_manager_.GetCompositeListInfo(arena);
  list.reserve(list.size() + spec_composite_list.size());
  list.insert(list.end(), std::make_move_iterator(spec_composite_list.begin()),
              std::make_move_iterator(spec_composite_list.end()));
  return list;
}

void DriverRunner::WaitForBootup(fit::callback<void()> callback) {
  bootup_tracker_->WaitForBootup(std::move(callback));
}

void DriverRunner::PublishComponentRunner(component::OutgoingDirectory& outgoing) {
  zx::result result = runner_.Publish(outgoing);
  ZX_ASSERT_MSG(result.is_ok(), "%s", result.status_string());

  result = memory_attributor_.Publish(outgoing);
  ZX_ASSERT_MSG(result.is_ok(), "%s", result.status_string());

  result = outgoing.AddUnmanagedProtocol<fdf::CompositeNodeManager>(
      manager_bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure));
  ZX_ASSERT_MSG(result.is_ok(), "%s", result.status_string());

  result = outgoing.AddUnmanagedProtocol<fdf::NodeManager>(
      node_manager_bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure));
  ZX_ASSERT_MSG(result.is_ok(), "%s", result.status_string());

  result = outgoing.AddUnmanagedProtocol<fuchsia_driver_token::NodeBusTopology>(
      bus_topo_bindings_.CreateHandler(this, dispatcher_, [](fidl::UnbindInfo info) {
        if (info.is_user_initiated() || info.is_peer_closed()) {
          return;
        }
        fdf_log::warn("Unexpected closure of NodeBusTopology: {}", info.FormatDescription());
      }));
  ZX_ASSERT_MSG(result.is_ok(), "%s", result.status_string());

  result = outgoing.AddUnmanagedProtocol<fuchsia_driver_crash::CrashIntrospect>(
      crash_introspect_bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure));
  ZX_ASSERT_MSG(result.is_ok(), "%s", result.status_string());
}

zx::result<> DriverRunner::StartRootDriver(std::string_view url) {
  fdf::DriverPackageType package = cpp20::starts_with(url, kBootScheme)
                                       ? fdf::DriverPackageType::kBoot
                                       : fdf::DriverPackageType::kBase;
  bootup_tracker_->Start();
  WaitForBootup([this]() { this->OnBootupComplete(); });
  root_node_->set_driver_host_name_for_colocation("root");
  return StartDriver(*root_node_, url, package);
}

void DriverRunner::StartDevfsDriver(std::shared_ptr<driver_manager::Devfs>& devfs) {
  auto [controller_client, controller_request] = fidl::Endpoints<fcomponent::Controller>::Create();
  devfs->SetController(std::move(controller_client));

  std::vector<NodeOffer> offers;
  runner_.CreateDriverComponent(devfs, std::move(controller_request), "devfs_driver",
                                "fuchsia-boot:///devfs-driver#meta/devfs-driver.cm",
                                CollectionName(Collection::kBoot).get(), offers);
}

void DriverRunner::NewDriverAvailable(NewDriverAvailableCompleter::Sync& completer) {
  TryBindAllAvailable();
  pending_node_manager_.MatchPendingNodesWithoutDriver();
}

void DriverRunner::TryBindAllAvailable(NodeBindingInfoResultCallback result_callback) {
  bind_manager_.TryBindAllAvailable(std::move(result_callback));
}

zx::result<> DriverRunner::StartDriver(Node& node, std::string_view url,
                                       fdf::DriverPackageType package_type) {
  // Ensure `node`'s collection is equal to or higher ranked than its ancestor
  // nodes' collections. This is to avoid node components having a dependency
  // cycle with each other. For example, node components in the boot driver
  // collection depend on the devfs component which ultimately depends on all
  // components within the package driver collection. If a package driver
  // component depended on a component in the boot driver collection (a lower
  // ranked collection than the package driver collection) then a cyclic
  // dependency would occur.
  node.set_collection(ToCollection(node, package_type));
  node.set_driver_package_type(package_type);

  std::weak_ptr node_weak = node.shared_from_this();
  std::string url_string(url.data(), url.size());
  auto moniker = node.MakeComponentMoniker();
  bootup_tracker_->NotifyNewStartRequest(moniker, url_string, node.shared_from_this());

  node.PrepareDictionary([this, node_weak, moniker, url_string](zx::result<> result) {
    if (!result.is_ok()) {
      return;
    }

    std::shared_ptr node = node_weak.lock();
    if (!node) {
      return;
    }

    CallStartDriverOnRunner(runner_, *node, moniker, url_string, bootup_tracker_);
  });

  return zx::ok();
}

void DriverRunner::Bind(Node& node, std::shared_ptr<BindResultTracker> result_tracker) {
  BindToUrl(node, {}, std::move(result_tracker));
}

void DriverRunner::Bind(Resource& resource, std::shared_ptr<BindResultTracker> result_tracker) {
  bind_manager_.Bind(resource, {}, std::move(result_tracker));
}

void DriverRunner::TryResolvePendingNodes() {
  pending_node_manager_.TryResolvePendingNodes(
      bind_manager_.bind_resource_set().CurrentMultibindResources());
}

void DriverRunner::BindToUrl(Node& node, std::string_view driver_url_suffix,
                             std::shared_ptr<BindResultTracker> result_tracker) {
  auto self_resource = node.GetSelfResource();
  ZX_ASSERT(self_resource.has_value());
  bind_manager_.Bind(*self_resource.value(), driver_url_suffix, std::move(result_tracker));
}

void DriverRunner::RebindComposite(std::string spec, std::optional<std::string> driver_url,
                                   fit::callback<void(zx::result<>)> callback) {
  composite_node_spec_manager_.Rebind(spec, driver_url, std::move(callback));
}

ResourceId DriverRunner::GetNextResourceId() { return next_resource_id_++; }

void DriverRunner::RebindCompositesWithDriver(const std::string& url,
                                              fit::callback<void(size_t)> complete_callback) {
  std::unordered_set<std::string> names;
  PerformBFS(root_node_, [&names, url](const std::shared_ptr<driver_manager::Node>& current) {
    if (current->type() == driver_manager::NodeType::kComposite && current->driver_url() == url) {
      fdf_log::debug("RebindCompositesWithDriver rebinding composite {}",
                     current->MakeComponentMoniker());
      names.insert(current->name());
      return false;
    }

    return true;
  });

  if (names.empty()) {
    complete_callback(0);
    return;
  }

  auto complete_wrapper = [complete_callback = std::move(complete_callback), count = names.size()](
                              zx::result<>) mutable { complete_callback(count); };

  std::shared_ptr<AsyncSharder> sharder =
      std::make_shared<AsyncSharder>(names.size(), std::move(complete_wrapper));

  for (const auto& name : names) {
    RebindComposite(name, std::nullopt,
                    [sharder](zx::result<>) mutable { sharder->CompleteShard(); });
  }
}

DriverHost* DriverRunner::GetDriverHost(std::string_view driver_host_name_for_colocation) {
  if (driver_host_name_for_colocation.empty()) {
    return nullptr;
  }
  for (auto& driver_host : driver_hosts_) {
    if (driver_host.name_for_colocation() == driver_host_name_for_colocation) {
      return &driver_host;
    }
  }
  return nullptr;
}

zx::result<DriverHost*> DriverRunner::CreateDriverHost(
    bool use_next_vdso, std::string_view driver_host_name_for_colocation) {
  auto endpoints = fidl::Endpoints<fio::Directory>::Create();
  std::string name;
  if (!driver_host_name_for_colocation.empty()) {
    std::string_view suffix = driver_host_name_for_colocation;
    suffix = suffix.starts_with("#") ? suffix.substr(1) : suffix;
    name = std::format("driver-host-{}", suffix);
  } else {
    name = std::format("driver-host-{}", next_driver_host_id_++);
  }

  std::shared_ptr<bool> connected = std::make_shared<bool>(false);
  auto create =
      CreateDriverHostComponent(name, std::move(endpoints.server), connected, use_next_vdso);
  if (create.is_error()) {
    return create.take_error();
  }

  auto client_end = component::ConnectAt<fdh::DriverHost>(endpoints.client);
  if (client_end.is_error()) {
    fdf_log::error("Failed to connect to service '{}': {}",
                   fidl::DiscoverableProtocolName<fdh::DriverHost>, client_end.status_string());
    return client_end.take_error();
  }

  auto loader_service_client = loader_service_factory_();
  if (loader_service_client.is_error()) {
    fdf_log::error("Failed to connect to service fuchsia.ldsvc/Loader: {}",
                   loader_service_client.status_string());
    return loader_service_client.take_error();
  }

  auto driver_host =
      std::make_unique<DriverHostComponent>(std::move(*client_end), dispatcher_, &driver_hosts_,
                                            connected, driver_host_name_for_colocation);
  auto result = driver_host->InstallLoader(std::move(*loader_service_client));
  if (result.is_error()) {
    fdf_log::error("Failed to install loader service: {}", result);
    return result.take_error();
  }

  auto driver_host_ptr = driver_host.get();
  driver_hosts_.push_back(std::move(driver_host));

  return zx::ok(driver_host_ptr);
}

void DriverRunner::CreateDriverHostDynamicLinker(
    std::string_view driver_host_name_for_colocation,
    fit::callback<void(zx::result<DriverHost*>)> completion_cb) {
  if (!dynamic_linker_args_.has_value()) {
    fdf_log::error("Dynamic linker was not available");
    completion_cb(zx::error(ZX_ERR_NOT_SUPPORTED));
    return;
  }

  auto endpoints = fidl::Endpoints<fio::Directory>::Create();

  auto client_end = component::ConnectAt<fdh::DriverHost>(endpoints.client);
  if (client_end.is_error()) {
    fdf_log::error("Failed to connect to service '{}': {}",
                   fidl::DiscoverableProtocolName<fdh::DriverHost>, client_end.status_string());
    completion_cb(client_end.take_error());
    return;
  }

  // TODO(https://fxbug.dev/349831408): for now we use the same driver host launcher client
  // channel for each driver host.
  if (!driver_host_launcher_.has_value()) {
    auto client = dynamic_linker_args_->linker_service_factory();
    if (client.is_error()) {
      fdf_log::error("Failed to create driver host launcher client");
      completion_cb(client.take_error());
      return;
    }
    driver_host_launcher_ = fidl::WireSharedClient<fuchsia_driver_loader::DriverHostLauncher>(
        std::move(*client), dispatcher_);
  }
  std::shared_ptr<bool> connected = std::make_shared<bool>(false);
  dynamic_linker_args_->driver_host_runner->StartDriverHost(
      driver_host_launcher_->Clone(), std::move(endpoints.server), connected,
      [this, completion_cb = std::move(completion_cb), client_end = std::move(client_end),
       connected = std::move(connected), name = std::string(driver_host_name_for_colocation)](
          zx::result<fidl::ClientEnd<fuchsia_driver_loader::DriverHost>> result) mutable {
        if (result.is_error()) {
          completion_cb(result.take_error());
          return;
        }

        auto driver_host = std::make_unique<DriverHostComponent>(
            std::move(*client_end), dispatcher_, &driver_hosts_, connected, name,
            std::move(*result));

        auto driver_host_ptr = driver_host.get();
        driver_hosts_.push_back(std::move(driver_host));
        completion_cb(zx::ok(driver_host_ptr));
      });
}

bool DriverRunner::IsDriverHostValid(DriverHost* driver_host) const {
  return driver_hosts_.find_if([driver_host](const DriverHostComponent& host) {
    return &host == driver_host;
  }) != driver_hosts_.end();
}

zx::result<std::string> DriverRunner::StartDriver(
    Node& node, fuchsia_driver_framework::wire::DriverInfo driver_info) {
  if (!driver_info.has_url()) {
    fdf_log::error("Failed to start driver for node '{}', the driver URL is missing", node.name());
    return zx::error(ZX_ERR_INTERNAL);
  }

  auto pkg_type =
      driver_info.has_package_type() ? driver_info.package_type() : fdf::DriverPackageType::kBase;
  auto result = StartDriver(node, driver_info.url().get(), pkg_type);
  if (result.is_error()) {
    return result.take_error();
  }
  return zx::ok(std::string(driver_info.url().get()));
}

zx::result<BindSpecResult> DriverRunner::BindToParentSpec(fidl::AnyArena& arena,
                                                          CompositeParents composite_parents,
                                                          std::weak_ptr<Resource> resource,
                                                          bool enable_multibind) {
  return this->composite_node_spec_manager_.BindParentSpec(arena, composite_parents, resource,
                                                           enable_multibind);
}

void DriverRunner::CreatePowerElement(
    std::optional<fidl::ClientEnd<fuchsia_power_broker::Topology>> topology_client,
    std::string_view name, fuchsia_power_broker::DependencyToken element_token,
    std::vector<fuchsia_power_broker::DependencyToken> deps,
    fidl::ServerEnd<fuchsia_power_broker::ElementControl> control,
    fidl::ClientEnd<fuchsia_power_broker::ElementRunner> runner,
    fidl::ServerEnd<fuchsia_power_broker::Lessor> lessor, Collection for_collection,
    std::optional<fuchsia_power_broker::DependencyToken> cpu_token_override,
    std::optional<zx::eventpair> initial_lease_token, bool is_hermetic_power_test,
    fit::callback<void(zx::result<bool>)> cb) {
  if (power_manager_) {
    power_manager_->CreatePowerElement(
        std::move(topology_client), name, std::move(element_token), std::move(deps),
        std::move(control), std::move(runner), std::move(lessor), for_collection,
        std::move(cpu_token_override), std::move(initial_lease_token), is_hermetic_power_test,
        std::move(cb));
  } else {
    cb(zx::ok(false));
  }
}

void DriverRunner::OnBootupComplete() {
  if (power_manager_) {
    power_manager_->OnBootupComplete(root_node_);
  }
}

std::optional<fuchsia_power_broker::DependencyToken> DriverRunner::StorageElementToken() {
  return power_manager_->StorageElementToken();
}

void DriverRunner::RequestMatchFromDriverIndex(
    fuchsia_driver_index::wire::MatchDriverArgs args,
    fit::callback<void(fidl::WireUnownedResult<fdi::DriverIndex::MatchDriver>&)> match_callback) {
  driver_index()->MatchDriver(args).Then(std::move(match_callback));
}

void DriverRunner::RequestMatchPendingNode(
    fidl::VectorView<fuchsia_driver_framework::wire::ParentSpec2> dependencies,
    fit::callback<
        void(fidl::WireUnownedResult<fuchsia_driver_index::DriverIndex::MatchPendingNode>&)>
        match_callback) {
  driver_index()->MatchPendingNode(dependencies).Then(std::move(match_callback));
}

void DriverRunner::RequestRebindFromDriverIndex(std::string spec,
                                                std::optional<std::string> driver_url_suffix,
                                                fit::callback<void(zx::result<>)> callback) {
  fidl::Arena allocator;
  fidl::StringView fidl_driver_url = driver_url_suffix == std::nullopt
                                         ? fidl::StringView()
                                         : fidl::StringView(allocator, driver_url_suffix.value());
  driver_index_->RebindCompositeNodeSpec(fidl::StringView(allocator, spec), fidl_driver_url)
      .Then(
          [callback = std::move(callback)](
              fidl::WireUnownedResult<fdi::DriverIndex::RebindCompositeNodeSpec>& result) mutable {
            if (!result.ok()) {
              fdf_log::error(
                  "Failed to send a composite rebind request to the Driver Index failed {}",
                  result.error().FormatDescription());
              callback(zx::error(result.status()));
              return;
            }

            if (result->is_error()) {
              callback(result->take_error());
              return;
            }
            callback(zx::ok());
          });
}

zx::result<> DriverRunner::CreateDriverHostComponent(
    std::string moniker, fidl::ServerEnd<fuchsia_io::Directory> exposed_dir,
    std::shared_ptr<bool> exposed_dir_connected, bool use_next_vdso) {
  auto do_create = [this, moniker, exposed_dir = std::move(exposed_dir),
                    exposed_dir_connected = std::move(exposed_dir_connected),
                    use_next_vdso]() mutable {
    constexpr std::string_view kUrl = "fuchsia-boot:///driver_host#meta/driver_host.cm";
    constexpr std::string_view kNextUrl = "fuchsia-boot:///driver_host#meta/driver_host_next.cm";
    fidl::Arena arena;
    auto child_decl_builder = fdecl::wire::Child::Builder(arena)
                                  .name(moniker)
                                  .url(use_next_vdso ? kNextUrl : kUrl)
                                  .startup(fdecl::wire::StartupMode::kLazy);
    auto child_args_builder = fcomponent::wire::CreateChildArgs::Builder(arena);
    auto open_callback =
        [moniker](fidl::WireUnownedResult<fcomponent::Realm::OpenExposedDir>& result) {
          if (!result.ok()) {
            fdf_log::error("Failed to open exposed directory for driver host: '{}': {}", moniker,
                           result.FormatDescription());
            return;
          }
          if (result->is_error()) {
            fdf_log::error("Failed to open exposed directory for driver host: '{}': {}", moniker,
                           static_cast<uint32_t>(result->error_value()));
          }
        };
    auto create_callback =
        [this, moniker, exposed_dir = std::move(exposed_dir),
         exposed_dir_connected = std::move(exposed_dir_connected),
         open_callback = std::move(open_callback)](
            fidl::WireUnownedResult<fcomponent::Realm::CreateChild>& result) mutable {
          if (!result.ok()) {
            fdf_log::error("Failed to create driver host '{}': {}", moniker,
                           result.error().FormatDescription());
            return;
          }
          if (result->is_error()) {
            fdf_log::error("Failed to create driver host '{}': {}", moniker,
                           static_cast<uint32_t>(result->error_value()));
            return;
          }
          fdecl::wire::ChildRef child_ref{
              .name = fidl::StringView::FromExternal(moniker),
              .collection = "driver-hosts",
          };
          runner_.realm()
              ->OpenExposedDir(child_ref, std::move(exposed_dir))
              .ThenExactlyOnce(std::move(open_callback));
          *exposed_dir_connected = true;
        };
    runner_.realm()
        ->CreateChild(
            fdecl::wire::CollectionRef{
                .name = "driver-hosts",
            },
            child_decl_builder.Build(), child_args_builder.Build())
        .Then(std::move(create_callback));
  };

  auto it = pending_driver_host_destructions_.find(moniker);
  if (it != pending_driver_host_destructions_.end()) {
    it->second.push_back([do_create = std::move(do_create)](zx::result<> result) mutable {
      if (result.is_ok()) {
        do_create();
      }
    });
  } else {
    do_create();
  }
  return zx::ok();
}

void DriverRunner::DestroyDriverHostComponent(std::string_view driver_host_name_for_colocation,
                                              fit::callback<void(zx::result<>)> completion_cb) {
  std::string name;
  if (!driver_host_name_for_colocation.empty()) {
    std::string_view suffix = driver_host_name_for_colocation;
    suffix = suffix.starts_with("#") ? suffix.substr(1) : suffix;
    name = std::format("driver-host-{}", suffix);
  } else {
    // Cannot reliably destroy an unnamed host here by name.
    completion_cb(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  // Clear name_for_colocation on the dying host so GetDriverHost will not match it.
  for (auto& host : driver_hosts_) {
    if (host.name_for_colocation() == driver_host_name_for_colocation) {
      host.set_name_for_colocation("");
    }
  }

  // If destruction is already in flight for this moniker, coalesce callbacks.
  auto it = pending_driver_host_destructions_.find(name);
  if (it != pending_driver_host_destructions_.end()) {
    if (completion_cb) {
      it->second.push_back(std::move(completion_cb));
    }
    return;
  }

  auto& callbacks = pending_driver_host_destructions_[name];
  if (completion_cb) {
    callbacks.push_back(std::move(completion_cb));
  }

  fdecl::wire::ChildRef child_ref{
      .name = fidl::StringView::FromExternal(name),
      .collection = "driver-hosts",
  };
  runner_.realm()->DestroyChild(child_ref).Then(
      [this,
       moniker = name](fidl::WireUnownedResult<fcomponent::Realm::DestroyChild>& result) mutable {
        zx::result<> final_status = zx::ok();
        if (!result.ok()) {
          fdf_log::error("Failed to destroy driver host '{}': {}", moniker,
                         result.FormatDescription());
          final_status = zx::error(result.status());
        } else if (result->is_error()) {
          // If the component has already been cleaned up by component_manager
          // or is not found, we treat it as success.
          if (result->error_value() == fcomponent::wire::Error::kInstanceNotFound ||
              result->error_value() == fcomponent::wire::Error::kInstanceDied) {
            final_status = zx::ok();
          } else {
            fdf_log::error("Failed to destroy driver host '{}': {}", moniker,
                           static_cast<uint32_t>(result->error_value()));
            final_status = zx::error(ZX_ERR_INTERNAL);
          }
        }

        auto it = pending_driver_host_destructions_.find(moniker);
        if (it != pending_driver_host_destructions_.end()) {
          auto pending = std::move(it->second);
          pending_driver_host_destructions_.erase(it);
          for (auto& cb : pending) {
            cb(final_status);
          }
        }
      });
}

zx::result<uint32_t> DriverRunner::RestartNodesColocatedWithDriverUrl(
    std::string_view url, fdd::RestartRematchFlags rematch_flags) {
  auto driver_hosts = DriverHostsWithDriverUrl(url);

  // Helper lambda to check if a node is part of the restarting set.
  auto is_in_restarting_set = [&driver_hosts, url](Node& node) {
    bool is_in_driver_host =
        node.driver_host() && driver_hosts.find(node.driver_host()) != driver_hosts.end();
    bool is_matching_url = node.driver_url() == url;
    return is_in_driver_host || is_matching_url;
  };

  // Caches for ancestor traversal across candidate nodes:
  // - `restarting_ancestor_cache`: Nodes that are in the restarting set or have an ancestor
  //   in the restarting set. Encountering any of these during upward traversal immediately
  //   confirms that the start node has a restarting ancestor.
  // - `no_restarting_ancestor_cache`: Nodes that have no ancestors in the restarting set (and
  //   are not in the restarting set). Encountering any of these during upward traversal prunes
  //   the branch since no restarting ancestors exist above it.
  std::unordered_set<const Node*> restarting_ancestor_cache;
  std::unordered_set<const Node*> no_restarting_ancestor_cache;

  if (root_node_ && !is_in_restarting_set(*root_node_)) {
    no_restarting_ancestor_cache.insert(root_node_.get());
  }

  // Helper lambda to check if a node has any ancestor (traversing upwards across all parents)
  // that is in the restarting set.
  auto has_ancestor_in_restarting_set = [&](const std::shared_ptr<Node>& start_node) {
    if (restarting_ancestor_cache.contains(start_node.get())) {
      return true;
    }
    if (no_restarting_ancestor_cache.contains(start_node.get())) {
      return false;
    }

    bool has_ancestor = false;
    std::vector<const Node*> visited_nodes;

    PerformBFS(
        start_node, Direction::kUp, [&](const std::shared_ptr<driver_manager::Node>& current) {
          if (has_ancestor) {
            return false;
          }
          if (current == start_node) {
            return true;
          }

          if (restarting_ancestor_cache.contains(current.get()) || is_in_restarting_set(*current)) {
            has_ancestor = true;
            restarting_ancestor_cache.insert(current.get());
            return false;
          }

          if (no_restarting_ancestor_cache.contains(current.get())) {
            // Prune traversal up this branch: no ancestors here are in the restarting set.
            return false;
          }

          visited_nodes.push_back(current.get());
          return true;
        });

    if (has_ancestor) {
      restarting_ancestor_cache.insert(start_node.get());
    } else {
      no_restarting_ancestor_cache.insert(start_node.get());
      for (const Node* node : visited_nodes) {
        no_restarting_ancestor_cache.insert(node);
      }
    }
    return has_ancestor;
  };

  // Perform a BFS over the node topology. If a node's host is one of the driver_hosts
  // we collected, or if a node's driver URL matches `url`, check if it has any ancestors in
  // the restarting set. If not, collect that node as a topmost node to restart. Skip its
  // children since they will go away as part of its restart.
  std::vector<std::shared_ptr<driver_manager::Node>> nodes_to_restart;
  PerformBFS(root_node_, [&](const std::shared_ptr<driver_manager::Node>& current) {
    if (!is_in_restarting_set(*current)) {
      // Not in one of the restarting hosts or matching the URL. Continue to visit the children.
      return true;
    }

    if (!has_ancestor_in_restarting_set(current)) {
      nodes_to_restart.push_back(current);
    }
    return false;
  });

  if (nodes_to_restart.empty()) {
    return zx::ok(0u);
  }

  // Collect any named driver host components that need to be destroyed and unregister
  // their colocation name upfront.
  //
  // Unregistering `name_for_colocation_` immediately ensures that `GetDriverHost("tag")` will
  // return nullptr while the dying driver host process is shutting down. When the restarting
  // nodes re-bind/start, `GetDriverHost("tag")` will trigger `CreateDriverHost()` to spawn a
  // fresh driver host process for the tag instead of reusing the dying host instance.
  std::unordered_set<std::string> tags_to_destroy;
  for (DriverHost* host : driver_hosts) {
    if (std::string_view tag = host->name_for_colocation(); !tag.empty()) {
      tags_to_destroy.insert(std::string(tag));
      host->set_name_for_colocation("");
    }
  }

  // Initiate node restart first so that the nodes start their shutdown flow and send StopDriver()
  // to their drivers.
  for (const auto& current : nodes_to_restart) {
    if (current->EvaluateRematchFlags(rematch_flags, url)) {
      if (current->type() == driver_manager::NodeType::kComposite) {
        fdf_log::debug("RestartNodesColocatedWithDriverUrl rebinding composite {}",
                       current->MakeComponentMoniker());
        RebindComposite(current->name(), std::nullopt, [](zx::result<>) {});
        continue;
      }

      fdf_log::debug("RestartNodesColocatedWithDriverUrl restarting node with rematch {}",
                     current->MakeComponentMoniker());
      current->RestartNodeWithRematch();
      continue;
    }

    fdf_log::debug("RestartNodesColocatedWithDriverUrl restarting node {}",
                   current->MakeComponentMoniker());
    current->RestartNode();
  }

  // Destroy the old driver host components in ComponentManager.
  for (const auto& tag : tags_to_destroy) {
    DestroyDriverHostComponent(tag, [](zx::result<>) {});
  }

  // Count the number of nodes that matched the URL but didn't have a driver host yet.
  // These nodes will be restarted by the driver manager in response to the driver's restart.
  size_t matching_nodes_without_hosts = 0;
  for (const auto& current : nodes_to_restart) {
    if (current->driver_url() == url && current->driver_host() == nullptr) {
      matching_nodes_without_hosts++;
    }
  }

  return zx::ok(static_cast<uint32_t>(driver_hosts.size() + matching_nodes_without_hosts));
}

void DriverRunner::RestartWithDictionary(fidl::StringView moniker,
                                         fuchsia_component_sandbox::wire::DictionaryRef dictionary,
                                         zx::eventpair reset_eventpair) {
  dictionary_util_.ImportDictionaryWire(std::move(dictionary), [this,
                                                                moniker =
                                                                    std::string(moniker.get()),
                                                                reset_eventpair =
                                                                    std::move(reset_eventpair)](
                                                                   zx::result<
                                                                       fuchsia_component_sandbox::
                                                                           NewCapabilityId>
                                                                       result) mutable {
    if (result.is_error()) {
      return;
    }

    std::shared_ptr<driver_manager::Node> restarted_node = nullptr;
    PerformBFS(root_node_, [&](const std::shared_ptr<driver_manager::Node>& current) {
      if (current->MakeComponentMoniker() == moniker && current->HasDriverComponent()) {
        if (current->HasSubtreeDictionaryRef()) {
          fdf_log::error(
              "RestartWithDictionary requested node id already contains a dictionary_ref from another RestartWithDictionary operation.");
          return false;
        }
        ZX_ASSERT_MSG(restarted_node == nullptr, "Multiple nodes with same moniker not possible.");
        restarted_node = current;
        current->SetSubtreeDictionaryRef(result.value());
        current->RestartNode();
        return false;
      }

      return true;
    });

    if (restarted_node != nullptr) {
      std::weak_ptr<driver_manager::Node> weak_restarted_node = restarted_node;
      std::unique_ptr<async::WaitOnce> wait = std::make_unique<async::WaitOnce>(
          reset_eventpair.release(), ZX_EVENTPAIR_PEER_CLOSED | ZX_EVENTPAIR_SIGNALED);
      async::WaitOnce* wait_ptr = wait.get();
      zx_status_t status = wait_ptr->Begin(
          dispatcher_,
          [weak_restarted_node = std::move(weak_restarted_node), moved_wait = std::move(wait)](
              async_dispatcher_t* dispatcher, async::WaitOnce* wait, zx_status_t status,
              const zx_packet_signal_t* signal) {
            fdf_log::info("RestartWithDictionary operation released.");
            auto restarted_node = weak_restarted_node.lock();
            if (!restarted_node) {
              return;
            }
            restarted_node->SetSubtreeDictionaryRef(std::nullopt);
            restarted_node->RestartNode();
          });

      if (status != ZX_OK) {
        fdf_log::error("Failed to Begin async::Wait for RestartWithDictionary.");
      }
    }
  });
}

void DriverRunner::RestartWithDictionaryAndPowerDependencies(
    std::string moniker, fuchsia_component_sandbox::DictionaryRef dictionary,
    std::vector<fuchsia_power_broker::LevelDependency> power_dependencies,
    std::optional<zx::event> cpu_token_override,
    std::vector<fuchsia_driver_development::NodePowerTokenOverride> node_power_token_overrides,
    zx::eventpair release_fence) {
  dictionary_util_.ImportDictionary(std::move(dictionary), [this, moniker = std::move(moniker),
                                                            power_dependencies =
                                                                std::move(power_dependencies),
                                                            cpu_token_override =
                                                                std::move(cpu_token_override),
                                                            node_power_token_overrides = std::move(
                                                                node_power_token_overrides),
                                                            release_fence =
                                                                std::move(release_fence)](
                                                               zx::result<
                                                                   fuchsia_component_sandbox::
                                                                       NewCapabilityId>
                                                                   result) mutable {
    if (result.is_error()) {
      fdf_log::error(
          "Failed to import dictionary for RestartWithDictionaryAndPowerDependencies: {}",
          result.status_string());
      return;
    }

    std::shared_ptr<driver_manager::Node> restarted_node = nullptr;
    PerformBFS(root_node_, [&](const std::shared_ptr<driver_manager::Node>& current) {
      if (current->MakeComponentMoniker() == moniker && current->HasDriverComponent()) {
        if (current->HasSubtreeDictionaryRef()) {
          fdf_log::error(
              "RestartWithDictionaryAndPowerDependencies requested node id already contains a dictionary_ref from another restart operation.");
          return false;
        }
        ZX_ASSERT_MSG(restarted_node == nullptr, "Multiple nodes with same moniker not possible.");
        restarted_node = current;
        current->SetSubtreeDictionaryRef(result.value());

        // Clone power dependencies for the node
        std::vector<fuchsia_power_broker::LevelDependency> deps;
        for (const auto& dep : power_dependencies) {
          if (!dep.requires_token().has_value() || !dep.dependent_level().has_value() ||
              !dep.requires_level_by_preference().has_value()) {
            fdf_log::warn("Power dependency is invalid, skipping.");
            continue;
          }
          fuchsia_power_broker::DependencyToken clone;
          zx_status_t status = dep.requires_token()->duplicate(ZX_RIGHT_SAME_RIGHTS, &clone);
          if (status != ZX_OK) {
            fdf_log::error("Failed to duplicate power token: {}", zx_status_get_string(status));
            continue;
          }
          deps.push_back(fuchsia_power_broker::LevelDependency{{
              .dependent_level = dep.dependent_level().value(),
              .requires_token = std::move(clone),
              .requires_level_by_preference = dep.requires_level_by_preference().value(),
          }});
        }
        current->SetPowerDependencyOverrides(std::move(deps));

        if (cpu_token_override.has_value()) {
          zx::event clone;
          zx_status_t status = cpu_token_override->duplicate(ZX_RIGHT_SAME_RIGHTS, &clone);
          if (status == ZX_OK) {
            current->SetCpuTokenOverride(std::move(clone));
          } else {
            fdf_log::error("Failed to duplicate CPU token override: {}",
                           zx_status_get_string(status));
          }
        }

        std::map<std::string, zx::event> token_overrides_map;
        for (auto& override_entry : node_power_token_overrides) {
          if (!override_entry.target_node().empty() && override_entry.token().is_valid()) {
            token_overrides_map.emplace(override_entry.target_node(),
                                        std::move(override_entry.token()));
          }
        }
        current->SetNodeTokenOverrides(std::move(token_overrides_map));

        current->RestartNode();
        return false;
      }

      return true;
    });

    if (restarted_node != nullptr) {
      std::weak_ptr<driver_manager::Node> weak_restarted_node = restarted_node;
      std::unique_ptr<async::WaitOnce> wait = std::make_unique<async::WaitOnce>(
          release_fence.release(), ZX_EVENTPAIR_PEER_CLOSED | ZX_EVENTPAIR_SIGNALED);
      async::WaitOnce* wait_ptr = wait.get();
      zx_status_t status = wait_ptr->Begin(
          dispatcher_,
          [weak_restarted_node = std::move(weak_restarted_node), moved_wait = std::move(wait)](
              async_dispatcher_t* dispatcher, async::WaitOnce* wait, zx_status_t status,
              const zx_packet_signal_t* signal) {
            fdf_log::info("RestartWithDictionaryAndPowerDependencies operation released.");
            auto restarted_node = weak_restarted_node.lock();
            if (!restarted_node) {
              return;
            }
            restarted_node->SetSubtreeDictionaryRef(std::nullopt);
            restarted_node->SetPowerDependencyOverrides(std::nullopt);
            restarted_node->SetCpuTokenOverride(std::nullopt);
            restarted_node->SetNodeTokenOverrides({});
            restarted_node->RestartNode();
          });

      if (status != ZX_OK) {
        fdf_log::error(
            "Failed to Begin async::Wait for RestartWithDictionaryAndPowerDependencies.");
      }
    }
  });
}

std::unordered_set<DriverHost*> DriverRunner::DriverHostsWithDriverUrl(std::string_view url) {
  std::unordered_set<DriverHost*> result_hosts;

  // Perform a BFS over the node topology, if the current node's driver url is the url we are
  // interested in, add the driver host it is in to the result set.
  PerformBFS(root_node_,
             [&result_hosts, url](const std::shared_ptr<driver_manager::Node>& current) {
               if (current->driver_url() == url && current->driver_host()) {
                 result_hosts.insert(current->driver_host());
               }
               return true;
             });

  return result_hosts;
}

void DriverRunner::RebootSystem() {
  if (!statecontrol_admin_.is_valid()) {
    fdf_log::error("Cannot reboot system: statecontrol_admin_ is not connected.");
    return;
  }

  fpower::ShutdownOptions options{{
      .action = fpower::ShutdownAction::kReboot,
      .reasons = {{fpower::ShutdownReason::kCriticalDriverFailure}},
  }};

  statecontrol_admin_->Shutdown(std::move(options))
      .Then([](fidl::Result<fpower::Admin::Shutdown>& result) {
        if (result.is_error()) {
          if (result.error_value().is_framework_error()) {
            fdf_log::error("Shutdown request failed (FIDL error): {}",
                           result.error_value().framework_error().FormatDescription());
          } else {
            zx_status_t status = result.error_value().domain_error();
            if (status != ZX_ERR_ALREADY_EXISTS) {
              fdf_log::error("Shutdown request failed (domain error): {}",
                             zx_status_get_string(status));
            }
          }
        }
      });
}

void DriverRunner::AddNode(AddNodeRequestView request, AddNodeCompleter::Sync& completer) {
  if (!request->node.has_name() || !request->node.has_dependencies() ||
      request->node.dependencies().empty()) {
    completer.Reply(fit::error(fuchsia_driver_framework::NodeError::kUnsupportedArgs));
    return;
  }

  for (const auto& dep : request->node.dependencies()) {
    if (!dep.has_selector()) {
      completer.Reply(fit::error(fuchsia_driver_framework::NodeError::kUnsupportedArgs));
      return;
    }
  }

  auto result = pending_node_manager_.AddNode(
      fidl::ToNatural(request->node), std::move(request->controller), std::move(request->node_ref));
  if (result.is_error()) {
    completer.Reply(fit::error(result.error_value()));
    return;
  }
  completer.Reply(fit::ok());
}

void DriverRunner::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_driver_framework::NodeManager> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf_log::warn("NodeManager received unknown method. Ordinal: {}", metadata.method_ordinal);
}

void DriverRunner::LeaseAllDriversForShutdown(fit::callback<void()> callback) {
  if (power_manager_) {
    power_manager_->LeaseAllDrivers(root_node_, std::move(callback));
  } else {
    callback();
  }
}

}  // namespace driver_manager

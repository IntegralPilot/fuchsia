// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/node.h"

#include <dirent.h>
#include <fidl/fuchsia.component/cpp/common_types_format.h>
#include <fidl/fuchsia.driver.framework/cpp/common_types_format.h>
#include <fidl/fuchsia.power.broker/cpp/fidl.h>
#include <lib/component/incoming/cpp/directory_watcher.h>
#include <lib/driver/component/cpp/internal/start_args.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/fdio/directory.h>
#include <zircon/errors.h>

#include <algorithm>
#include <cstring>
#include <deque>
#include <optional>
#include <queue>
#include <unordered_set>
#include <utility>
#include <variant>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/platform/cpp/bind.h>

#include "src/devices/bin/driver_manager/async_sharder.h"
#include "src/devices/bin/driver_manager/bind/bind_result_tracker.h"
#include "src/devices/bin/driver_manager/bootup_tracker.h"
#include "src/devices/bin/driver_manager/controller_allowlist_passthrough.h"
#include "src/devices/bin/driver_manager/node_property_conversion.h"
#include "src/devices/bin/driver_manager/resource.h"
#include "src/devices/bin/driver_manager/shutdown/node_removal_tracker.h"
#include "src/devices/lib/log/log.h"
#include "src/lib/fxl/strings/join_strings.h"

namespace fdf {
using namespace fuchsia_driver_framework;
}  // namespace fdf
namespace fdecl = fuchsia_component_decl;
namespace fcomponent = fuchsia_component;

namespace driver_manager {

void DriverHostConnection::on_fidl_error(fidl::UnbindInfo info) {
  node_->OnDriverHostFidlError(info);
}

void ComponentControllerConnection::on_fidl_error(fidl::UnbindInfo info) {
  node_->OnComponentControllerFidlError(info);
}

namespace {

const std::string kUnboundUrl = "unbound";
const std::string kOwnedByParentUrl = "owned by parent";
const std::string kCompositeParent = "owned by composite(s)";

// TODO(https://fxbug.dev/42075799): Remove this flag once composite node spec rebind once all
// clients are updated to the new Rebind() behavior and this is fully implemented on both DFv1 and
// DFv2.
constexpr bool kEnableCompositeNodeSpecRebind = false;

const char* CollectionName(Collection collection) {
  switch (collection) {
    case Collection::kNone:
      return "";
    case Collection::kBoot:
      return "boot-drivers";
    case Collection::kPackage:
      return "base-drivers";
    case Collection::kFullPackage:
      return "full-drivers";
  }
}

zx::result<> RegistrationErrorToResult(fuchsia_power_broker::RegisterDependencyTokenError e) {
  switch (e) {
    case fuchsia_power_broker::RegisterDependencyTokenError::kAlreadyInUse:
      return zx::error(ZX_ERR_ALREADY_EXISTS);
    case fuchsia_power_broker::RegisterDependencyTokenError::kInternal:
      return zx::error(ZX_ERR_INTERNAL);
    default:
      ZX_PANIC("Unknown register dependency token error");
  }
}

}  // namespace

// Processes the offer by validating it has a source_name and adding a source ref to it.
// Returns the offer back out if it succeeds.
fit::result<fuchsia_driver_framework::NodeError, NodeOffer> ProcessNodeOffer(
    const fuchsia_driver_framework::Offer& add_offer, Collection source_collection,
    std::string_view source_name) {
  std::optional<OfferTransport> transport;
  std::optional<fdecl::Offer> fdecl_offer;

  switch (add_offer.Which()) {
    case fuchsia_driver_framework::Offer::Tag::kZirconTransport:
      fdecl_offer.emplace(add_offer.zircon_transport().value());
      transport.emplace(OfferTransport::ZirconTransport);
      break;
    case fuchsia_driver_framework::Offer::Tag::kDriverTransport:
      fdecl_offer.emplace(add_offer.driver_transport().value());
      transport.emplace(OfferTransport::DriverTransport);
      break;
    case fuchsia_driver_framework::Offer::Tag::kDictionaryOffer:
      fdecl_offer.emplace(add_offer.dictionary_offer().value());
      transport.emplace(OfferTransport::Dictionary);
      break;
    default:
      fdf_log::error("Unknown offer transport type {}", static_cast<uint32_t>(add_offer.Which()));
      return fit::error(fuchsia_driver_framework::NodeError::kInternal);
  }

  auto service_offer = fdecl_offer->service();
  if (!service_offer.has_value()) {
    return fit::as_error(fuchsia_driver_framework::NodeError::kUnsupportedArgs);
  }

  if (!service_offer->source_name().has_value()) {
    return fit::as_error(fuchsia_driver_framework::NodeError::kOfferSourceNameMissing);
  }

  if (service_offer->target_name().has_value() &&
      service_offer->target_name().value() != service_offer->source_name().value()) {
    return fit::as_error(fuchsia_driver_framework::NodeError::kUnsupportedArgs);
  }

  if (service_offer->source().has_value() || service_offer->target().has_value()) {
    return fit::as_error(fuchsia_driver_framework::NodeError::kOfferRefExists);
  }

  if (!service_offer->source_instance_filter().has_value()) {
    return fit::as_error(fuchsia_driver_framework::NodeError::kOfferSourceInstanceFilterMissing);
  }

  if (!service_offer->renamed_instances().has_value()) {
    return fit::as_error(fuchsia_driver_framework::NodeError::kOfferRenamedInstancesMissing);
  }

  if (transport.value() == OfferTransport::Dictionary) {
    source_name = "dictionary";
    source_collection = Collection::kNone;
  }

  return fit::ok(NodeOffer{
      .source_name = std::string(source_name),
      .source_collection = source_collection,
      .transport = transport.value(),
      .service_name = service_offer->source_name().value(),
      .source_instance_filter = service_offer->source_instance_filter().value(),
      .renamed_instances = service_offer->renamed_instances().value(),
  });
}

namespace {

// Returns true if the service should be excluded from automatic fuchsia.Service
// property generation.
//
// We exclude:
// - Services that are handled specially (e.g. power, interrupts).
// - Services that are only for testing or compatibility.
// - Services that contain "Metadata" in their name. Metadata services are
//   helper services used to retrieve metadata, and generating a fuchsia.Service
//   property for them could cause conflicts if the node also offers a regular
//   service.
//
// In the future, we should migrate drivers to manually specify the
// bind_fuchsia::SERVICE property instead of relying on this exclusion list,
// and eventually remove this list.
bool ShouldExcludeService(std::string_view service_name) {
  if (service_name == "fuchsia.driver.compat.Service" ||
      service_name == "fuchsia.hardware.power.PowerTokenService" ||
      service_name == "fuchsia.hardware.interrupt.ControllerRegistryService" ||
      service_name == "fuchsia.hardware.goldfish.ControlService") {
    return true;
  }
  if (service_name.find("Metadata") != std::string_view::npos) {
    return true;
  }
  return false;
}

// Processes the offer by validating it has a source_name and adding a source ref to it.
// Returns a tuple containing the offer as well as node property that provides transport
// information for the offer.
fit::result<fdf::NodeError, std::tuple<NodeOffer, std::optional<fdf::NodeProperty2>>>
ProcessNodeOfferWithTransportProperty(const fdf::Offer& add_offer, Collection source_collection,
                                      std::string_view source_name,
                                      bool generate_service_property) {
  fit::result result = ProcessNodeOffer(add_offer, source_collection, source_name);
  if (result.is_error()) {
    return result.take_error();
  }

  NodeOffer processed_offer = std::move(result.value());

  const std::string& name = processed_offer.service_name;
  if (ShouldExcludeService(name)) {
    return fit::ok(std::make_tuple(std::move(processed_offer), std::nullopt));
  }

  std::optional<fdf::NodeProperty2> service_property;
  if (generate_service_property) {
    service_property = fdf::MakeProperty2(bind_fuchsia::SERVICE, name);
  }

  return fit::ok(std::make_tuple(std::move(processed_offer), std::move(service_property)));
}

bool IsDefaultOffer(std::string_view target_name) { return target_name == "default"; }

template <typename T>
void CloseIfExists(std::optional<fidl::ServerBinding<T>>& ref) {
  if (ref) {
    ref->Close(ZX_OK);
  }
}

fit::result<fdf::NodeError> ValidateSymbols(std::vector<fdf::NodeSymbol>& symbols) {
  std::unordered_set<std::string_view> names;
  for (auto& symbol : symbols) {
    if (!symbol.name().has_value()) {
      fdf_log::error("SymbolError: a symbol is missing a name");
      return fit::error(fdf::NodeError::kSymbolNameMissing);
    }
    if (!symbol.address().has_value()) {
      fdf_log::error("SymbolError: symbol '{}' is missing an address", symbol.name().value());
      return fit::error(fdf::NodeError::kSymbolAddressMissing);
    }
    auto [_, inserted] = names.emplace(symbol.name().value());
    if (!inserted) {
      fdf_log::error("SymbolError: symbol '{}' already exists", symbol.name().value());
      return fit::error(fdf::NodeError::kSymbolAlreadyExists);
    }
  }
  return fit::ok();
}

}  // namespace

fdf::Offer ToFidl(const NodeOffer& offer) {
  auto service = fdecl::OfferService{{
      .source = fdecl::Ref::WithChild(fdecl::ChildRef{{
          .name = offer.source_name,
          .collection = CollectionName(offer.source_collection),
      }}),
      .source_name = offer.service_name,
      .target_name = offer.service_name,
      .source_instance_filter = offer.source_instance_filter,
      .renamed_instances = offer.renamed_instances,
  }};

  auto fdecl_offer = fdecl::Offer::WithService(service);
  switch (offer.transport) {
    case OfferTransport::ZirconTransport:
      return fdf::Offer::WithZirconTransport(std::move(fdecl_offer));
    case OfferTransport::DriverTransport:
      return fdf::Offer::WithDriverTransport(std::move(fdecl_offer));
    case OfferTransport::Dictionary:
      return fdf::Offer::WithDictionaryOffer(std::move(fdecl_offer));
  }
}

NodeOffer CreateCompositeOffer(const NodeOffer& offer, std::string_view parents_name,
                               bool primary_parent) {
  size_t new_instance_count = offer.renamed_instances.size();
  if (primary_parent) {
    for (const auto& instance : offer.renamed_instances) {
      if (IsDefaultOffer(instance.target_name())) {
        new_instance_count++;
      }
    }
  }

  size_t new_filter_count = offer.source_instance_filter.size();
  if (primary_parent) {
    for (const auto& filter : offer.source_instance_filter) {
      if (IsDefaultOffer(filter)) {
        new_filter_count++;
      }
    }
  }

  std::vector<fdecl::NameMapping> mappings;
  mappings.reserve(new_instance_count);
  for (const auto& instance : offer.renamed_instances) {
    // The instance is not "default", so copy it over.
    if (!IsDefaultOffer(instance.target_name())) {
      mappings.emplace_back(instance);
      continue;
    }

    // We are the primary parent, so add the "default" offer.
    if (primary_parent) {
      mappings.emplace_back(instance);
    }

    // Rename the instance to match the parent's name.
    mappings.emplace_back(instance.source_name(), std::string(parents_name));
  }
  ZX_ASSERT(mappings.size() == new_instance_count);

  std::vector<std::string> filters;
  filters.reserve(new_filter_count);
  for (const auto& filter : offer.source_instance_filter) {
    // The filter is not "default", so copy it over.
    if (!IsDefaultOffer(filter)) {
      filters.push_back(filter);
      continue;
    }

    // We are the primary parent, so add the "default" filter.
    if (primary_parent) {
      filters.push_back("default");
    }

    // Rename the filter to match the parent's name.
    filters.emplace_back(parents_name);
  }
  ZX_ASSERT(filters.size() == new_filter_count);

  return NodeOffer{
      .source_name = offer.source_name,
      .source_collection = offer.source_collection,
      .transport = offer.transport,
      .service_name = offer.service_name,
      .source_instance_filter = std::move(filters),
      .renamed_instances = std::move(mappings),
  };
}

zx::event ResolvePowerElementToken(const Node& node,
                                   const std::map<std::string, zx::event>& node_token_overrides) {
  zx::event power_element_token;

  // Try using the name.
  auto override_it = node_token_overrides.find(node.name());
  if (override_it != node_token_overrides.end() && override_it->second.is_valid()) {
    zx::event injected;
    ZX_ASSERT(ZX_OK == override_it->second.duplicate(ZX_RIGHT_SAME_RIGHTS, &injected));
    power_element_token = std::move(injected);
    return power_element_token;
  }

  // Try using the moniker.
  override_it = node_token_overrides.find(node.MakeComponentMoniker());
  if (override_it != node_token_overrides.end() && override_it->second.is_valid()) {
    zx::event injected;
    ZX_ASSERT(ZX_OK == override_it->second.duplicate(ZX_RIGHT_SAME_RIGHTS, &injected));
    power_element_token = std::move(injected);
    return power_element_token;
  }

  // Didn't have an override, just make a new zx::event.
  zx::event::create(0, &power_element_token);
  return power_element_token;
}

Node::Node(std::string_view name, std::weak_ptr<Node> parent, NodeManager* node_manager,
           async_dispatcher_t* dispatcher, std::map<std::string, zx::event> node_token_overrides)
    : name_(name),
      type_(Normal{.parent_ = std::move(parent)}),
      node_manager_(node_manager),
      dispatcher_(dispatcher),
      node_token_overrides_(std::move(node_token_overrides)),
      driver_host_handler_(this),
      component_controller_handler_(this) {
  // By default, we set `driver_host_` to match the primary parent's
  // `driver_host_`. If the node is then subsequently bound to a driver in a
  // different driver host, this value will be updated to match.
  if (auto* parent = GetPrimaryParent(); parent) {
    driver_host_ = parent->driver_host_;
  }
  power_element_token_ = ResolvePowerElementToken(*this, node_token_overrides_);
}

Node::Node(std::string_view name, std::unordered_map<std::string, std::weak_ptr<Node>> parents,
           std::vector<std::string> parents_names, NodeManager* node_manager,
           async_dispatcher_t* dispatcher, uint32_t primary_index,
           std::map<std::string, zx::event> node_token_overrides)
    : name_(name),
      node_manager_(node_manager),
      dispatcher_(dispatcher),
      node_token_overrides_(std::move(node_token_overrides)),
      driver_host_handler_(this),
      component_controller_handler_(this) {
  std::vector<std::weak_ptr<Node>> parents_vec;
  uint32_t primary_node_index = 0;

  // The `parents` map contains an entry for every resource dependency of the composite,
  // where the key is the parent name. If the composite has multiple resource
  // dependencies from the same parent node, the `parents` map will contain duplicate
  // parent nodes under different keys.
  //
  // We iterate through `parents_names` in order to preserve the original dependency order,
  // and construct the `parents_vec` with only unique parent nodes. At the same time, we
  // determine the index of the primary parent node in both the ordered `parents_names_`
  // and unique `parents_` lists.
  for (size_t i = 0; i < parents_names.size(); ++i) {
    const auto& parent_name = parents_names[i];
    auto parent_node_wk = parents.at(parent_name);
    auto parent_node = parent_node_wk.lock();
    auto it = std::find_if(
        parents_vec.begin(), parents_vec.end(),
        [&parent_node](const std::weak_ptr<Node>& node) { return node.lock() == parent_node; });
    uint32_t node_index;
    if (it == parents_vec.end()) {
      node_index = static_cast<uint32_t>(parents_vec.size());
      parents_vec.push_back(parent_node_wk);
    } else {
      node_index = static_cast<uint32_t>(std::distance(parents_vec.begin(), it));
    }

    if (i == primary_index) {
      primary_node_index = node_index;
    }
  }

  type_ = Composite{
      .parents_ = std::move(parents_vec),
      .parents_names_ = std::move(parents_names),
      .primary_index_ = primary_index,
      .primary_node_index_ = primary_node_index,
  };

  ZX_ASSERT(primary_node_index < std::get<Composite>(type_).parents_.size());

  // By default, we set `driver_host_` to match the primary parent's
  // `driver_host_`. If the node is then subsequently bound to a driver in a
  // different driver host, this value will be updated to match.
  driver_host_ = GetPrimaryParent()->driver_host_;
  power_element_token_ = ResolvePowerElementToken(*this, node_token_overrides_);
}

std::map<std::string, zx::event> Node::DuplicateTokenOverrides(
    const std::map<std::string, zx::event>& overrides) {
  std::map<std::string, zx::event> copy;
  for (const auto& [node_name, token] : overrides) {
    if (token.is_valid()) {
      zx::event clone;
      ZX_ASSERT(ZX_OK == token.duplicate(ZX_RIGHT_SAME_RIGHTS, &clone));
      copy.emplace(node_name, std::move(clone));
    }
  }
  return copy;
}

zx::result<std::shared_ptr<Node>> Node::CreateCompositeNode(
    std::string_view node_name, std::vector<std::weak_ptr<Resource>> dependencies,
    std::vector<std::string> parents_names,
    const std::vector<fuchsia_driver_framework::NodePropertyEntry2>& parent_properties,
    NodeManager* driver_binder, async_dispatcher_t* dispatcher,
    std::string_view driver_host_name_for_colocation, uint32_t primary_index,
    std::map<std::string, zx::event> node_token_overrides) {
  ZX_ASSERT(!dependencies.empty());
  ZX_ASSERT(parents_names.size() == dependencies.size());

  if (dependencies.size() != parent_properties.size()) {
    fdf_log::error(
        "Missing parent properties. Expected {} entries, equal to the number of dependencies {}.",
        parent_properties.size(), dependencies.size());
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  if (primary_index >= dependencies.size()) {
    fdf_log::error("Primary node index is out of bounds");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  std::unordered_map<std::string, std::weak_ptr<Node>> parents_map;
  std::vector<std::shared_ptr<Resource>> received_resources;

  for (uint32_t i = 0; i < dependencies.size(); ++i) {
    auto parent_resource = dependencies[i].lock();
    if (!parent_resource) {
      fdf_log::error("Parent resource freed before use");
      return zx::error(ZX_ERR_INTERNAL);
    }
    received_resources.push_back(parent_resource);
    parents_map[parents_names[i]] = parent_resource->owner().lock();
  }

  // Create the node token overrides. The logic here uses the passed in
  // overrides first. Then it adds overrides from the primary parent and then
  // the rest of the parents. It does not replace tokens already in the
  // overrides map. The effect is first to use the passed in tokens, then those
  // from the primary parent, and then those from other parents.
  std::map<std::string, zx::event> composite_token_overrides = std::move(node_token_overrides);
  auto primary_parent_resource = dependencies[primary_index].lock();
  auto primary_parent = primary_parent_resource->owner().lock();
  if (primary_parent && !primary_parent->node_token_overrides_.empty()) {
    for (const auto& [key, token] : primary_parent->node_token_overrides_) {
      if (token.is_valid()) {
        if (composite_token_overrides.contains(key)) {
          fdf_log::warn("Duplicate token override for key '{}', existing override used instead.",
                        key);
          continue;
        }
        zx::event clone;
        ZX_ASSERT(ZX_OK == token.duplicate(ZX_RIGHT_SAME_RIGHTS, &clone));
        composite_token_overrides.emplace(key, std::move(clone));
      }
    }
  }

  for (const auto& [name, parent] : parents_map) {
    auto parent_ptr = parent.lock();
    if (parent_ptr && !parent_ptr->node_token_overrides_.empty()) {
      for (const auto& [key, token] : parent_ptr->node_token_overrides_) {
        if (token.is_valid()) {
          if (composite_token_overrides.contains(key)) {
            fdf_log::warn("Duplicate token override for key '{}', existing override used instead.",
                          key);
            continue;
          }
          zx::event clone;
          ZX_ASSERT(ZX_OK == token.duplicate(ZX_RIGHT_SAME_RIGHTS, &clone));
          composite_token_overrides.emplace(key, std::move(clone));
        }
      }
    }
  }

  std::shared_ptr composite = std::make_shared<Node>(
      node_name, std::move(parents_map), std::move(parents_names), driver_binder, dispatcher,
      primary_index, std::move(composite_token_overrides));
  composite->received_resources_ = std::move(received_resources);
  for (auto& resource : composite->received_resources_) {
    resource->AddDependent(composite);
  }

  composite->SetCompositeParentProperties(parent_properties);
  composite->driver_host_name_for_colocation_ = driver_host_name_for_colocation;

  Node* primary = composite->GetPrimaryParent();
  // We know that our device has a parent because we're creating it.
  ZX_ASSERT(primary);

  // Copy the symbols from the primary parent.
  composite->symbols_ = primary->symbols_;

  bool has_dictionary_offer = false;

  // Copy the offers from each parent resource.
  std::vector<NodeOffer> node_offers;
  size_t parent_index = 0;
  for (const std::shared_ptr<Resource>& resource : composite->received_resources_) {
    if (!resource) {
      fdf_log::error("Composite parent resource freed before use");
      return zx::error(ZX_ERR_INTERNAL);
    }
    auto parent_offers = resource->offers();
    node_offers.reserve(node_offers.size() + parent_offers.size());

    for (auto& parent_offer : parent_offers) {
      NodeOffer offer = CreateCompositeOffer(
          parent_offer, std::get<Composite>(composite->type_).parents_names_[parent_index],
          parent_index == primary_index);

      if (offer.transport == OfferTransport::Dictionary) {
        has_dictionary_offer = true;
      }

      node_offers.push_back(std::move(offer));
    }

    parent_index++;
  }

  // Copy the subtree dictionary of the primary parent node down to the composite.
  if (primary->HasSubtreeDictionaryRef()) {
    composite->SetSubtreeDictionaryRef(primary->subtree_dictionary_ref_);
    ZX_ASSERT_MSG(!has_dictionary_offer, "Cannot use dictionary offers on node %s",
                  std::string(node_name).c_str());
  }

  composite->InitializeSelfResource({}, std::move(node_offers));
  composite->AddToParents();
  ZX_ASSERT_MSG(primary->devfs_device_.topological_node().has_value(), "%s",
                composite->MakeTopologicalPath().c_str());

  // TODO(https://fxbug.dev/331779666): disable controller access for composite nodes
  primary->devfs_device_.topological_node().value().add_child(
      composite->name_, std::nullopt,
      composite->CreateDevfsPassthrough(std::nullopt, std::nullopt, true, ""),
      composite->devfs_device_);
  composite->devfs_device_.publish();
  return zx::ok(std::move(composite));
}

Node::~Node() {
  // TODO(https://fxbug.dev/42085057): Notify the NodeRemovalTracker if the node is deallocated
  // before shutdown is complete.
  if (GetNodeState() != NodeState::kDestroyed) {
    fdf_log::info("Node {} deallocating while at state {}", MakeComponentMoniker(),
                  GetNodeShutdownCoordinator().NodeStateAsString());
  }

  if (bind_wait_completer_) {
    bind_wait_completer_(zx::error(ZX_ERR_CANCELED));
  }
  CloseIfExists(controller_ref_);
  if (auto* driver_component = std::get_if<DriverComponent>(&state_); driver_component) {
    driver_component->node_ref_.Close(ZX_OK);
  } else if (auto* node = std::get_if<OwnedByParent>(&state_); node) {
    node->node_ref_.Close(ZX_OK);
  }
  component_controller_ = {};

  for (auto& completer : unbinding_children_completers_) {
    completer.Reply(zx::error(ZX_ERR_CANCELED));
  }

  if (pending_bind_completer_) {
    pending_bind_completer_(zx::error(ZX_ERR_CANCELED));
  }

  if (composite_rebind_completer_) {
    fdf_log::warn("Unable to rebind node {} since it deallocated before completing shutdown",
                  MakeComponentMoniker());
    composite_rebind_completer_(zx::error(ZX_ERR_CANCELED));
  }
}

const std::string& Node::driver_url() const {
  if (auto* driver_component = std::get_if<DriverComponent>(&state_); driver_component) {
    return driver_component->driver_url;
  }

  if (auto* starting = std::get_if<Starting>(&state_); starting) {
    return starting->driver_url;
  }

  if (auto* quarantined = std::get_if<Quarantined>(&state_); quarantined) {
    return quarantined->driver_url;
  }

  if (std::holds_alternative<OwnedByParent>(state_)) {
    return kOwnedByParentUrl;
  }

  if (std::holds_alternative<CompositeParent>(state_)) {
    return kCompositeParent;
  }

  return kUnboundUrl;
}

std::string Node::MakeTopologicalPath(bool deduplicate) const {
  std::deque<std::string_view> names;
  std::string_view prev;
  for (auto node = this; node != nullptr; node = node->GetPrimaryParent()) {
    std::string_view name = node->name();
    if (!deduplicate || name != prev) {
      names.push_front(name);
      prev = name;
    }
  }
  return fxl::JoinStrings(names, "/");
}

std::string Node::MakeComponentMoniker() const {
  std::string topo_path = MakeTopologicalPath(true);

  const std::string_view kPrefix = "dev/sys/platform/pt/";
  const std::string_view kPrefix2 = "dev/sys/platform/";
  if (topo_path == "dev") {
    topo_path = "root";
  } else if (topo_path == "dev/sys/platform/pt") {
    topo_path = "board";
  } else if (topo_path.starts_with(kPrefix)) {
    topo_path.erase(0, kPrefix.length());
  } else if (topo_path.starts_with(kPrefix2)) {
    topo_path.erase(0, kPrefix2.length());
  }

  // The driver's component name is based on the node name, which means that the
  // node name cam only have [a-z0-9-_.] characters. DFv1 composites contain ':'
  // which is not allowed, so replace those characters.
  // TODO(https://fxbug.dev/42062456): Migrate driver names to only use CF valid characters.
  std::ranges::replace(topo_path, ':', '_');
  // Since we use '.' to denote topology, replace them with '_'.
  std::ranges::replace(topo_path, '.', '_');
  std::ranges::replace(topo_path, '/', '.');
  return topo_path;
}

void Node::SetController(fidl::ClientEnd<fcomponent::Controller> component_controller) {
  component_controller_.Bind(std::move(component_controller), dispatcher_,
                             &component_controller_handler_);
}

void Node::SetShouldDestroy() { should_destroy_ = true; }

bool g_use_test_process_koid = false;

void Node::OnBind() {
  if (controller_ref_) {
    zx::result<zx::event> node_token = DuplicateNodeToken();
    if (node_token.is_error()) {
      return;
    }

    fit::result result =
        fidl::SendEvent(*controller_ref_)->OnBind({{.node_token = std::move(node_token.value())}});
    if (result.is_error()) {
      fdf_log::error("Failed to send OnBind event: {}", result.error_value().FormatDescription());
    }
  }

  zx::result node_token = DuplicateNodeToken();
  if (node_token.is_error()) {
    return;
  }
  auto koid = token_koid();
  if (!koid) {
    return;
  }

  zx_koid_t process_koid;
  if (g_use_test_process_koid) {
    process_koid = 1337;
  } else {
    zx::result result = driver_host()->GetProcessKoid();
    if (result.is_error()) {
      return;
    }
    process_koid = result.value();
  }
  node_manager_.value()->memory_attributor().AddDriver(std::move(node_token.value()), koid.value(),
                                                       process_koid);

  // Report on successes immediately without waiting for boot-up to complete.
  bind_err_ = {};

  if (IsComposite()) {
    for (auto& parent_ref : parents()) {
      std::shared_ptr<Node> parent = parent_ref.lock();
      if (parent && parent->bind_wait_completer_) {
        zx::result<zx::event> node_token = DuplicateNodeToken();
        if (node_token.is_error()) {
          return;
        }

        parent->bind_wait_completer_(zx::ok(
            fdf::wire::DriverResult::WithDriverStartedNodeToken(std::move(node_token.value()))));
      }
    }
  } else if (bind_wait_completer_) {
    zx::result<zx::event> node_token = DuplicateNodeToken();
    if (node_token.is_error()) {
      return;
    }
    bind_wait_completer_(
        zx::ok(fdf::wire::DriverResult::WithDriverStartedNodeToken(std::move(node_token.value()))));
  }
  node_manager_.value()->OnNodeBound(shared_from_this());
}

void Node::OnMatchError(zx_status_t status) {
  // Set a value that we can use in the boot-up callback that we set in WaitForDriver.
  bind_err_ = fdf::wire::DriverResult::WithMatchError(status);
}
void Node::OnStartError(zx_status_t status) {
  // Set a value that we can use in the boot-up callback that we set in WaitForDriver.
  bind_err_ = fdf::wire::DriverResult::WithStartError(status);
}

void Node::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_component_runner::ComponentController> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf_log::info("Unknown ComponentController method request received: {}", metadata.method_ordinal);
}

void Node::Stop(StopCompleter::Sync& completer) {
  fdf_log::debug("Calling Remove on {} because of Stop() from component framework.", name());
  Remove(RemovalSet::kAll, nullptr);
}

void Node::Kill(KillCompleter::Sync& completer) {
  fdf_log::debug("Calling Remove on {} because of Kill() from component framework.", name());
  Remove(RemovalSet::kAll, nullptr);
}

void Node::CompleteBind(zx::result<> result) {
  ZX_ASSERT(!std::holds_alternative<OwnedByParent>(state_));

  if (result.is_error()) {
    fdf_log::warn("Bind failed for node '{}'", MakeComponentMoniker());

    if (GetNodeState() == NodeState::kRunning && !std::holds_alternative<Unbound>(state_)) {
      fdf_log::debug("Quarantining node '{}'", MakeComponentMoniker());
      QuarantineNode();
    } else {
      state_ = Unbound{};
    }
  }

  if (auto* driver_component = std::get_if<DriverComponent>(&state_); driver_component) {
    if (driver_component->state == DriverState::kStopped) {
      fdf_log::warn(
          "Node {} in state {} completed bind with result {} but the component is already Stopped.",
          name(), GetNodeShutdownCoordinator().NodeStateAsString(), result);

      // Even if the driver started successfully, the component is no longer there (this can happen
      // if component framework is shutting down the system) so we can't say the bind succeeded.
      result = zx::error(ZX_ERR_CANCELED);
    } else {
      ZX_ASSERT_MSG(driver_component->state == DriverState::kBinding,
                    "Node %s CompleteBind() invoked at invalid state %d", name().c_str(),
                    driver_component->state);
      ZX_ASSERT(result.is_ok());
      if (!driver_component->driver) {
        fdf_log::error("Node {}: Driver started successfully but channel closed during binding.",
                       name());
        driver_component->state = DriverState::kStopped;
        // Even if the driver started successfully, the driver host channel is no longer there,
        // so we can't say the bind succeeded.
        result = zx::error(ZX_ERR_PEER_CLOSED);
        if (!host_restart_on_crash_) {
          Remove(RemovalSet::kAll, nullptr);
        }
      } else {
        driver_component->state = DriverState::kRunning;
        OnBind();
      }
    }
  }

  if (pending_bind_completer_) {
    pending_bind_completer_(result);
  }

  GetNodeShutdownCoordinator().CheckNodeState();
}

void Node::AddToParents() {
  auto this_node = shared_from_this();
  for (auto& parent : parents()) {
    if (auto ptr = parent.lock(); ptr) {
      ptr->children_.push_back(this_node);
      continue;
    }
    fdf_log::warn("Parent freed before child {} could be added to it", name());
  }
}

void Node::SetController(fidl::ServerEnd<fuchsia_driver_framework::NodeController> controller) {
  controller_ref_.emplace(dispatcher_, std::move(controller), this,
                          [weak = weak_from_this()](fidl::UnbindInfo info) {
                            std::shared_ptr self = weak.lock();
                            if (self && self->controller_ref_) {
                              self->controller_ref_.reset();
                            }
                          });
}

void Node::SetOwnedByParent(fidl::ServerEnd<fuchsia_driver_framework::Node> node_ref) {
  state_.emplace<OwnedByParent>(std::move(node_ref), this);
}

NodeShutdownCoordinator& Node::GetNodeShutdownCoordinator() {
  if (!node_shutdown_coordinator_) {
    bool is_shutdown_test_delay_enabled =
        node_manager_.has_value() && node_manager_.value()->IsTestShutdownDelayEnabled();
    auto shutdown_rng = node_manager_.has_value() ? node_manager_.value()->GetShutdownTestRng()
                                                  : std::weak_ptr<std::mt19937>();
    node_shutdown_coordinator_ = std::make_unique<NodeShutdownCoordinator>(
        name_, this, dispatcher_, is_shutdown_test_delay_enabled, shutdown_rng);
  }
  return *node_shutdown_coordinator_;
}

// TODO(https://fxbug.dev/42075799): If the node invoking this function cannot multibind to
// composites, is parenting one composite node, and is not in a state for removal, then it should
// attempt to bind to something else.
void Node::RemoveChild(const std::shared_ptr<Node>& child) {
  fdf_log::debug("RemoveChild {} from parent {}", child->name(), name());
  std::erase(children_, child);
  if (!unbinding_children_completers_.empty() && children_.empty()) {
    for (auto& completer : unbinding_children_completers_) {
      completer.ReplySuccess();
    }
    unbinding_children_completers_.clear();
  }
  GetNodeShutdownCoordinator().CheckNodeState();
}

void Node::FinishShutdown(fit::callback<void()> shutdown_callback) {
  ZX_ASSERT_MSG(GetNodeState() == NodeState::kWaitingOnDestroy,
                "FinishShutdown called in invalid node state: %s",
                GetNodeShutdownCoordinator().NodeStateAsString());
  if (auto koid = token_koid(); koid.has_value()) {
    node_manager_.value()->memory_attributor().RemoveDriver(koid.value());
  }
  if (shutdown_intent() == ShutdownIntent::kRestart) {
    fdf_log::debug("Node: {} finishing restart", name());
    shutdown_callback();
    FinishRestart();
    return;
  }

  if (shutdown_intent() == ShutdownIntent::kQuarantine) {
    fdf_log::debug("Node: {} finishing quarantine", name());
    shutdown_callback();
    FinishQuarantine();
    return;
  }

  if (bind_wait_completer_) {
    bind_wait_completer_(zx::error(ZX_ERR_CANCELED));
  }
  fdf_log::debug("Node: {} finishing shutdown", name());
  CloseIfExists(controller_ref_);
  if (auto* driver_component = std::get_if<DriverComponent>(&state_); driver_component) {
    driver_component->node_ref_.Close(ZX_OK);
  } else if (auto* node = std::get_if<OwnedByParent>(&state_); node) {
    node->node_ref_.Close(ZX_OK);
  }
  devfs_device_.unpublish();

  // Store a shared_ptr to ourselves so we won't be freed halfway through this function.
  std::shared_ptr this_node = shared_from_this();
  for (auto& parent : parents()) {
    if (auto ptr = parent.lock(); ptr) {
      ptr->RemoveChild(this_node);
      continue;
    }
    fdf_log::warn("Parent freed before child {} could be removed from it", name());
  }
  state_ = Unbound{};

  for (auto& resource : received_resources_) {
    if (resource) {
      resource->RemoveDependent(this_node);
    }
  }
  received_resources_.clear();

  // Only call Remove() on provided resources. Self resources act as a container for resource
  // elements in a node so the node's own removal logic will handle it.
  auto provided_resources = std::move(provided_resources_);
  for (auto& resource : provided_resources) {
    if (resource) {
      resource->Remove();
    }
  }

  if (IsComposite()) {
    std::get<Composite>(type_).parents_.clear();
  } else {
    std::get<Normal>(type_).parent_ = {};
  }

  shutdown_callback();

  if (remove_complete_callback_) {
    remove_complete_callback_();
  }

  if (shutdown_intent() == ShutdownIntent::kRebindComposite && composite_rebind_completer_) {
    composite_rebind_completer_(zx::ok());
  }
}

void Node::FinishRestart() {
  ZX_ASSERT_MSG(shutdown_intent() == ShutdownIntent::kRestart,
                "FinishRestart called when node is not restarting.");

  pe_handles_.reset();
  GetNodeShutdownCoordinator().ResetShutdown();

  // Store previous url before we reset the state_.
  std::string previous_url = driver_url();
  if (previous_url == kUnboundUrl || previous_url.empty()) {
    fdf_log::error("Node {}: Refusing to restart with invalid driver URL '{}'", name(),
                   previous_url);
    return;
  }

  // Perform cleanups for previous driver before we try to start the next driver.
  if (auto* driver_component = std::get_if<DriverComponent>(&state_); driver_component) {
    driver_component->node_ref_.Close(ZX_OK);
  } else if (auto* node = std::get_if<OwnedByParent>(&state_); node) {
    node->node_ref_.Close(ZX_OK);
  }
  state_ = Unbound{};

  if (restart_driver_url_suffix_.has_value()) {
    auto tracker = CreateBindResultTracker();
    node_manager_.value()->BindToUrl(*this, restart_driver_url_suffix_.value(), std::move(tracker));
    restart_driver_url_suffix_.reset();
    return;
  }

  zx::result start_result =
      node_manager_.value()->StartDriver(*this, previous_url, driver_package_type_);
  if (start_result.is_error()) {
    fdf_log::error("Failed to start driver '{}': {}", name(), start_result);
  }
}

void Node::FinishQuarantine() {
  ZX_ASSERT_MSG(shutdown_intent() == ShutdownIntent::kQuarantine,
                "FinishQuarantine called when node is not quarantining.");

  GetNodeShutdownCoordinator().ResetShutdown();

  // |QuarantineNode()| sets this.
  ZX_ASSERT_MSG(std::holds_alternative<Quarantined>(state_),
                "Node::state_ was not set to Quarantined");
}

void Node::ClearHostDriver() {
  if (auto* driver_component = std::get_if<DriverComponent>(&state_); driver_component) {
    driver_component->driver = {};
  }
}

// State table for package driver:
//                                   Initial States
//                 Running | Prestop|  WoC   | WoDriver | Stopping
// Remove(kPkg)      WoC   |  WoC   | Ignore |  Error!  |  Error!
// Remove(kAll)      WoC   |  WoC   |  WoC   |  Error!  |  Error!
// children empty    N/A   |  N/A   |WoDriver|  Error!  |  Error!
// Driver exit       WoC   |  WoC   |  WoC   | Stopping |  Error!
//
// State table for boot driver:
//                                   Initial States
//                  Running | Prestop |  WoC   | WoDriver | Stopping
// Remove(kPkg)     Prestop | Ignore  | Ignore |  Ignore  |  Ignore
// Remove(kAll)      WoC    |   WoC   | Ignore |  Ignore  |  Ignore
// children empty    N/A    |   N/A   |WoDriver|  Ignore  |  Ignore
// Driver exit       WoC    |   WoC   |  WoC   | Stopping |  Ignore
// Boot drivers go into the Prestop state when Remove(kPackage) is set, to signify that
// a removal is taking place, but this node will not be removed yet, even if all its children
// are removed.
void Node::Remove(RemovalSet removal_set, NodeRemovalTracker* removal_tracker) {
  NodeShutdownCoordinator::Remove(shared_from_this(), removal_set, removal_tracker);
}

void Node::RestartNode() {
  GetNodeShutdownCoordinator().set_shutdown_intent(ShutdownIntent::kRestart);
  Remove(RemovalSet::kAll, nullptr);
}

void Node::RestartNodeOnCrash() {
  std::string driver_host_name = driver_host_name_for_colocation_;
  if (node_manager_.has_value() && !driver_host_name.empty()) {
    node_manager_.value()->DestroyDriverHostComponent(driver_host_name, [](zx::result<> result) {
      if (result.is_error()) {
        fdf_log::error("Failed to destroy old driver host during restart: {}",
                       result.status_string());
      }
    });
  }
  RestartNode();
}

void Node::QuarantineNode() {
  // Perform cleanups for previous driver.
  if (auto* driver_component = std::get_if<DriverComponent>(&state_); driver_component) {
    driver_component->node_ref_.Close(ZX_OK);
  } else {
    ZX_ASSERT(std::holds_alternative<Starting>(state_));
  }
  state_ = Quarantined{.driver_url = driver_url()};

  GetNodeShutdownCoordinator().set_shutdown_intent(ShutdownIntent::kQuarantine);
  Remove(RemovalSet::kAll, nullptr);
}

// TODO(https://fxbug.dev/42082343): Handle the case in which this function is called during node
// removal.
void Node::RestartNodeWithRematch(std::optional<std::string> restart_driver_url_suffix,
                                  fit::callback<void(zx::result<>)> completer) {
  if (pending_bind_completer_) {
    completer(zx::error(ZX_ERR_ALREADY_EXISTS));
    return;
  }

  pending_bind_completer_ = std::move(completer);
  restart_driver_url_suffix_ = std::move(restart_driver_url_suffix);
  RestartNode();
}

void Node::RestartNodeWithRematch() {
  RestartNodeWithRematch("", [](zx::result<> result) {});
}

// TODO(https://fxbug.dev/42082343): Handle the case in which this function is called during node
// removal.
void Node::RemoveCompositeNodeForRebind(fit::callback<void(zx::result<>)> completer) {
  if (composite_rebind_completer_) {
    completer(zx::error(ZX_ERR_ALREADY_EXISTS));
    return;
  }

  if (!IsComposite()) {
    completer(zx::error(ZX_ERR_NOT_SUPPORTED));
    return;
  }

  composite_rebind_completer_ = std::move(completer);
  GetNodeShutdownCoordinator().set_shutdown_intent(ShutdownIntent::kRebindComposite);
  Remove(RemovalSet::kAll, nullptr);
}

std::shared_ptr<BindResultTracker> Node::CreateBindResultTracker(bool silent) {
  return std::make_shared<BindResultTracker>(
      1, [weak_self = weak_from_this(),
          silent](fidl::VectorView<fuchsia_driver_development::wire::NodeBindingInfo> info) {
        std::shared_ptr self = weak_self.lock();
        if (!self) {
          return;
        }
        // We expect a single successful "bind". If we don't get it, we can assume the bind
        // request failed. If we do get it, we will continue to wait for the driver's start hook
        // to complete, which will only occur after the successful bind. The remaining flow will
        // be similar to the RestartNode flow.
        if (info.size() < 1) {
          // Failed binding attempt should make the node have an unbound url. Reset this in case
          // there was a previous driver on this node that had failed to start and was stored
          // as Quarantined{} in state_ as part of the node quarantining.
          self->state_ = Unbound{};

          self->OnMatchError(ZX_ERR_NOT_FOUND);
          if (!silent) {
            self->CompleteBind(zx::error(ZX_ERR_NOT_FOUND));
          }
        } else if (info.size() > 1) {
          fdf_log::error("Unexpectedly bound multiple drivers to a single node");
          self->OnMatchError(ZX_ERR_BAD_STATE);
          if (!silent) {
            self->CompleteBind(zx::error(ZX_ERR_BAD_STATE));
          }
        }
      });
}

std::vector<Node::Property> Node::ToProperty(std::span<const fdf::NodeProperty2> properties) {
  std::vector<Property> node_properties;
  node_properties.reserve(properties.size());
  std::ranges::transform(properties, std::back_inserter(node_properties), [](const auto& property) {
    if (const auto& val = property.value().string_value(); val) {
      return Property{
          .key = property.key(),
          .value = val.value(),
      };
    }
    if (const auto& val = property.value().bool_value(); val) {
      return Property{
          .key = property.key(),
          .value = val.value(),
      };
    }
    if (const auto& val = property.value().int_value(); val) {
      return Property{
          .key = property.key(),
          .value = val.value(),
      };
    }
    if (const auto& val = property.value().enum_value(); val) {
      return Property{
          .key = property.key(),
          .value = EnumValue{.value = val.value()},
      };
    }
    return Property{
        .key = property.key(),
        .value = std::monostate(),
    };
  });
  return node_properties;
}

std::vector<fdf::NodeProperty2> Node::PropertyToFidl(std::span<const Property> properties) {
  std::vector<fdf::NodeProperty2> node_properties;
  node_properties.reserve(properties.size());
  std::ranges::transform(properties, std::back_inserter(node_properties), [](const auto& property) {
    if (const auto* val = std::get_if<std::string>(&property.value); val) {
      return fdf::NodeProperty2{{
          .key = property.key,
          .value = fdf::NodePropertyValue::WithStringValue(*val),
      }};
    }
    if (const auto* val = std::get_if<bool>(&property.value); val) {
      return fdf::NodeProperty2{{
          .key = property.key,
          .value = fdf::NodePropertyValue::WithBoolValue(*val),
      }};
    }
    if (const auto* val = std::get_if<uint32_t>(&property.value); val) {
      return fdf::NodeProperty2{{
          .key = property.key,
          .value = fdf::NodePropertyValue::WithIntValue(*val),
      }};
    }
    if (const auto* val = std::get_if<EnumValue>(&property.value); val) {
      return fdf::NodeProperty2{{
          .key = property.key,
          .value = fdf::NodePropertyValue::WithEnumValue(val->value),
      }};
    }
    return fdf::NodeProperty2{{
        .key = property.key,
        .value = fdf::NodePropertyValue::WithStringValue("UNKNOWN"),
    }};
  });

  return node_properties;
}

void Node::SetCompositeParentProperties(const fdf::NodePropertyDictionary2& parent_properties) {
  std::vector<PropertiesEntry> properties;
  properties.reserve(parent_properties.size() + 1);
  std::ranges::transform(parent_properties, std::back_inserter(properties), [](const auto& entry) {
    return PropertiesEntry{
        .name = entry.name(),
        .properties = ToProperty(entry.properties()),
    };
  });

  auto& composite = std::get<Composite>(type_);
  ZX_ASSERT(composite.primary_index_ < parent_properties.size());
  const auto& default_node_properties = parent_properties[composite.primary_index_].properties();
  properties.emplace_back(PropertiesEntry{
      .name = "default",
      .properties = ToProperty(default_node_properties),
  });

  composite.properties_ = std::move(properties);
}

std::vector<fdf::BusInfo> Node::GetBusTopology() const {
  std::vector<fdf::BusInfo> segments;
  for (auto node = this; node != nullptr; node = node->GetPrimaryParent()) {
    if (node->self_resource_ && node->self_resource_->bus_info().has_value()) {
      segments.push_back(node->self_resource_->bus_info().value());
    }
  }
  std::ranges::reverse(segments);
  return segments;
}

Node::OwnedByParent::OwnedByParent(fidl::ServerEnd<fdf::Node> node, Node* child)
    : node_ref_(child->dispatcher_, std::move(node), child,
                [](Node* node, fidl::UnbindInfo info) { node->OnNodeServerUnbound(info); }) {}

void Node::AddChildHelper(fuchsia_driver_framework::NodeAddArgs args,
                          fidl::ServerEnd<fuchsia_driver_framework::NodeController> controller,
                          fidl::ServerEnd<fuchsia_driver_framework::Node> node,
                          AddNodeResultCallback callback) {
  if (!unbinding_children_completers_.empty()) {
    fdf_log::error("Failed to add node: Node is currently unbinding all of its children");
    callback(fit::as_error(fdf::NodeError::kUnbindChildrenInProgress));
    return;
  }
  if (node_manager_ == nullptr) {
    fdf_log::warn("Failed to add Node, as this Node '{}' was removed", name());
    callback(fit::as_error(fdf::NodeError::kNodeRemoved));
    return;
  }
  if (GetNodeShutdownCoordinator().IsShuttingDown()) {
    fdf_log::warn("Failed to add Node, as this Node '{}' is being removed", name());
    callback(fit::as_error(fdf::NodeError::kNodeRemoved));
    return;
  }
  if (!args.name().has_value()) {
    fdf_log::error("Failed to add Node, a name must be provided");
    callback(fit::as_error(fdf::NodeError::kNameMissing));
    return;
  }
  std::string_view name = args.name().value();
  for (auto& child : children_) {
    if (child->name() == name) {
      fdf_log::error("Failed to add Node '{}', name already exists among siblings", name);
      callback(fit::as_error(fdf::NodeError::kNameAlreadyExists));
      return;
    }
  };
  std::map<std::string, zx::event> child_token_overrides;
  if (!node_token_overrides_.empty()) {
    child_token_overrides = DuplicateTokenOverrides(node_token_overrides_);
  }
  std::shared_ptr child = std::make_shared<Node>(name, weak_from_this(), *node_manager_,
                                                 dispatcher_, std::move(child_token_overrides));
  if (cpu_token_override_.has_value()) {
    zx::event token_copy;
    ZX_ASSERT(cpu_token_override_->duplicate(ZX_RIGHT_SAME_RIGHTS, &token_copy) == ZX_OK);
    child->SetCpuTokenOverride(std::move(token_copy));
  }

  auto& fdf_offers = args.offers2();
  std::vector<fuchsia_driver_framework::NodeProperty2> properties;

  const auto& arg_properties = args.properties2();
  if (arg_properties.has_value()) {
    properties = arg_properties.value();
  }

  const auto& arg_deprecated_properties = args.properties();
  if (arg_deprecated_properties.has_value()) {
    if (arg_properties.has_value()) {
      fdf_log::error(
          "Failed to add Node '{}'. Found values for both properties and properties2 are set. Only one of the fields can be set.",
          name);
      callback(fit::as_error(fdf::NodeError::kUnsupportedArgs));
      return;
    }

    properties.reserve(arg_deprecated_properties->size());
    for (auto& property : arg_deprecated_properties.value()) {
      if (property.key().Which() == fuchsia_driver_framework::NodePropertyKey::Tag::kIntValue) {
        fdf_log::error(
            "Failed to add Node '{}'. Found integer-based key {} which is no longer supported.",
            name, property.key().int_value().value());
        callback(fit::as_error(fdf::NodeError::kUnsupportedArgs));
        return;
      }
      properties.emplace_back(ToProperty2(property));
    }
  }
  if (args.driver_host()) {
    child->driver_host_name_for_colocation_ = args.driver_host().value();
  }

  // If the node already has a manually specified fuchsia.Service property,
  // we disable automatic generation of fuchsia.Service properties for all
  // offers. This allows drivers to manually specify which service should be
  // used for binding, avoiding conflicts when a node offers multiple services.
  bool has_manual_service_property = false;
  for (const auto& property : properties) {
    if (property.key() == bind_fuchsia::SERVICE) {
      has_manual_service_property = true;
      break;
    }
  }

  std::vector<NodeOffer> child_offers;
  bool has_dictionary_offer = false;
  if (fdf_offers.has_value()) {
    child_offers.reserve(fdf_offers.value().size());

    // Find a parent node with a collection. This indicates that a driver has
    // been bound to the node, and the driver is running within the collection.
    Node* source_node = this;
    while (source_node && source_node->collection_ == Collection::kNone) {
      if (source_node->GetPrimaryParent() == nullptr) {
        break;
      }
      source_node = source_node->GetPrimaryParent();
    }
    std::string source_name = source_node->MakeComponentMoniker();
    Collection source_collection = source_node->collection_;

    for (auto& fdf_offer : fdf_offers.value()) {
      if (fdf_offer.Which() == fuchsia_driver_framework::Offer::Tag::kDictionaryOffer) {
        has_dictionary_offer = true;
      }

      fit::result new_offer = ProcessNodeOfferWithTransportProperty(
          fdf_offer, source_collection, source_name, !has_manual_service_property);
      if (new_offer.is_error()) {
        fdf_log::error("Failed to add Node '{}': Bad add offer: {}", child->MakeTopologicalPath(),
                       new_offer.error_value());
        callback(new_offer.take_error());
        return;
      }
      auto [processed_offer, service_property] = std::move(new_offer.value());
      child_offers.emplace_back(processed_offer);
      if (service_property.has_value()) {
        properties.emplace_back(std::move(service_property.value()));
      }
    }
  }

  std::optional<fdf::BusInfo> bus_info = std::move(args.bus_info());

  // Copy the subtree dictionary of a parent node down to the child.
  if (subtree_dictionary_ref_.has_value()) {
    child->subtree_dictionary_ref_ = subtree_dictionary_ref_;
    ZX_ASSERT_MSG(!has_dictionary_offer, "Cannot use dictionary offers on node %s",
                  std::string(name).c_str());
  }

  child->InitializeSelfResource(std::move(properties), std::move(child_offers),
                                std::move(bus_info));

  if (auto& symbols = args.symbols(); symbols.has_value()) {
    auto is_valid = ValidateSymbols(symbols.value());
    if (is_valid.is_error()) {
      fdf_log::error("Failed to add Node '{}', bad symbols", name);
      callback(fit::as_error(is_valid.error_value()));
      return;
    }

    child->symbols_ = std::move(args.symbols().value());
  }

  Devnode::Target devfs_target;
  std::optional<std::string_view> devfs_class_path;
  std::string class_name = "Unknown_Class_name";
  auto& devfs_args = args.devfs_args();
  if (devfs_args.has_value()) {
    if (devfs_args->class_name().has_value()) {
      devfs_class_path = devfs_args->class_name();
      class_name = std::string(devfs_args->class_name().value());
    }
    // We do not populate the connection to the controller unless it is specifically
    // supported through the connector_supports argument.
    bool allow_controller_connection = (devfs_args->connector_supports().has_value() &&
                                        (devfs_args->connector_supports().value() &
                                         fuchsia_device_fs::ConnectionType::kController));
    if (allow_controller_connection && !devfs_args->class_name().has_value()) {
      class_name = "No_class_name_but_driver_url_is_" + driver_url();
    }

    devfs_target = child->CreateDevfsPassthrough(std::move(devfs_args->connector()),
                                                 std::move(devfs_args->controller_connector()),
                                                 allow_controller_connection, class_name);
  } else {
    devfs_target = child->CreateDevfsPassthrough(std::nullopt, std::nullopt, false, class_name);
  }
  ZX_ASSERT(devfs_device_.topological_node().has_value());
  zx_status_t status = devfs_device_.topological_node()->add_child(
      child->name_, devfs_class_path, std::move(devfs_target), child->devfs_device_);
  ZX_ASSERT_MSG(status == ZX_OK, "%s failed to export: %s", child->MakeTopologicalPath().c_str(),
                zx_status_get_string(status));
  ZX_ASSERT(child->devfs_device_.topological_node().has_value());
  child->devfs_device_.publish();

  if (controller.is_valid()) {
    child->controller_ref_.emplace(dispatcher_, std::move(controller), child.get(),
                                   [weak = child->weak_from_this()](fidl::UnbindInfo info) {
                                     std::shared_ptr self = weak.lock();
                                     if (self && self->controller_ref_) {
                                       self->controller_ref_.reset();
                                     }
                                   });
  }

  auto finish = [weak_self = weak_from_this(), child, node = std::move(node)]() mutable {
    std::shared_ptr<Node> self = weak_self.lock();
    if (!self) {
      fdf_log::warn("Parent of '{}' freed before AddChild dictionary import completed",
                    child->name());
      return;
    }
    if (node.is_valid()) {
      child->state_.emplace<OwnedByParent>(std::move(node), child.get());
    } else {
      auto tracker = child->CreateBindResultTracker(/*silent=*/true);
      (*self->node_manager_)->Bind(*child, std::move(tracker));
    }

    child->AddToParents();
  };

  if ((has_dictionary_offer && !args.offers_dictionary().has_value()) ||
      (!has_dictionary_offer && args.offers_dictionary().has_value())) {
    callback(fit::as_error(fdf::NodeError::kUnsupportedArgs));
    return;
  }

  if (args.offers_dictionary().has_value()) {
    (*node_manager_)
        ->dictionary_util()
        .ImportDictionary(
            std::move(args.offers_dictionary()).value(),
            [child, callback = std::move(callback), finish = std::move(finish)](
                zx::result<fuchsia_component_sandbox::wire::NewCapabilityId> result) mutable {
              // If the import failed it will log a warning, but we don't need to
              // fail the child creation.
              if (!result.is_ok()) {
                finish();
                callback(fit::ok(child));
                return;
              }

              std::unordered_map<std::string,
                                 fidl::ClientEnd<fuchsia_component_sandbox::DirReceiver>>
                  map;
              for (const auto& offer : child->offers()) {
                if (offer.transport != OfferTransport::Dictionary) {
                  continue;
                }

                auto [client, server] =
                    fidl::Endpoints<fuchsia_component_sandbox::DirReceiver>::Create();
                map[offer.service_name] = std::move(client);

                (*child->node_manager_)
                    ->dictionary_util()
                    .DictionaryDirConnectorOpen(
                        result.value(), offer.service_name,
                        [child, server = std::move(server),
                         offer](zx::result<fidl::ClientEnd<fuchsia_io::Directory>> result) mutable {
                          if (result.is_ok()) {
                            std::unordered_map<std::string, std::string>
                                target_to_source_instance_mapping;
                            for (auto& mapping : offer.renamed_instances) {
                              target_to_source_instance_mapping[mapping.target_name()] =
                                  mapping.source_name();
                            }

                            std::vector<DirInfo> dirs;
                            dirs.emplace_back(DirInfo{
                                .dir = std::move(result.value()),
                                .target_service_name = offer.service_name,
                                .target_to_source_instance_mapping =
                                    target_to_source_instance_mapping,
                                .is_primary = true,
                            });

                            child->dir_receivers_.emplace_back(std::move(server), std::move(dirs),
                                                               child->dispatcher_);
                          }
                        });
              }

              (*child->node_manager_)
                  ->dictionary_util()
                  .CreateDictionaryWith(
                      std::move(map),
                      [child, finish = std::move(finish), callback = std::move(callback)](
                          zx::result<fuchsia_component_sandbox::CapabilityId> result) mutable {
                        if (!result.is_ok()) {
                          finish();
                          callback(fit::ok(child));
                          return;
                        }

                        ZX_ASSERT_MSG(!child->subtree_dictionary_ref_.has_value(),
                                      "Cannot set dictionary_ref_ on node %s",
                                      child->name().c_str());
                        child->dictionary_ref_ = result.value();
                        finish();
                        callback(fit::ok(child));
                      });
            });
  } else {
    finish();
    callback(fit::ok(child));
  }
}

void Node::WaitForChildToExit(std::string_view name,
                              fit::callback<void(fit::result<fdf::NodeError>)> callback) {
  for (auto& child : children_) {
    if (child->name() != name) {
      continue;
    }
    if (!child->GetNodeShutdownCoordinator().IsShuttingDown()) {
      fdf_log::error("Failed to add Node '{}', name already exists among siblings", name);
      callback(fit::as_error(fdf::NodeError::kNameAlreadyExists));
      return;
    }
    if (child->remove_complete_callback_) {
      fdf_log::error(
          "Failed to add Node '{}': Node with name already exists and is marked to be replaced.",
          name);
      callback(fit::as_error(fdf::NodeError::kNameAlreadyExists));
      return;
    }
    child->remove_complete_callback_ = [callback = std::move(callback)]() mutable {
      callback(fit::success());
    };
    child->GetNodeShutdownCoordinator().CheckNodeState();
    return;
  };
  callback(fit::success());
}

void Node::AddChild(fuchsia_driver_framework::NodeAddArgs args,
                    fidl::ServerEnd<fuchsia_driver_framework::NodeController> controller,
                    fidl::ServerEnd<fuchsia_driver_framework::Node> node,
                    AddNodeResultCallback callback) {
  if (!args.name().has_value()) {
    fdf_log::error("Failed to add Node, a name must be provided");
    callback(fit::as_error(fdf::NodeError::kNameMissing));
    return;
  }

  // Verify the properties.
  if (args.properties().has_value() && args.properties2().has_value()) {
    fdf_log::error("Failed to add Node, both properties and properties2 fields were set");
    callback(fit::as_error(fdf::NodeError::kUnsupportedArgs));
    return;
  }

  // Only check for unique property keys for properties2 since properties is deprecated.
  if (args.properties2().has_value()) {
    std::unordered_set<std::string> property_keys;
    for (auto& property : args.properties2().value()) {
      if (property_keys.contains(property.key())) {
        fdf_log::error(
            "Failed to add Node since properties2 contain multiple properties with the same key");
        callback(fit::as_error(fdf::NodeError::kDuplicatePropertyKeys));
        return;
      }
      property_keys.insert(property.key());
    }
  }

  std::string name = args.name().value();
  WaitForChildToExit(
      name, [self = shared_from_this(), args = std::move(args), controller = std::move(controller),
             node = std::move(node),
             callback = std::move(callback)](fit::result<fdf::NodeError> result) mutable {
        if (result.is_error()) {
          callback(result.take_error());
          return;
        }
        self->AddChildHelper(std::move(args), std::move(controller), std::move(node),
                             std::move(callback));
      });
}

void Node::ProvideResource(
    fidl::WireServer<fuchsia_driver_framework::Node>::ProvideResourceRequestView request,
    fidl::WireServer<fuchsia_driver_framework::Node>::ProvideResourceCompleter::Sync& completer) {
  if (!request->resource.has_name() || !request->resource.has_properties() ||
      !request->resource.has_offers()) {
    fdf_log::error("Failed to provide resource: Missing name, properties, or offers fields");
    completer.ReplyError(fdf::NodeError::kUnsupportedArgs);
    return;
  }
  ResourceId id = (*node_manager_)->GetNextResourceId();
  auto natural_resource = fidl::ToNatural(request->resource);
  std::vector<NodeOffer> node_offers;
  if (natural_resource.offers().has_value()) {
    node_offers.reserve(natural_resource.offers()->size());

    Node* source_node = this;
    while (source_node && source_node->collection_ == Collection::kNone) {
      if (source_node->GetPrimaryParent() == nullptr) {
        break;
      }
      source_node = source_node->GetPrimaryParent();
    }
    std::string source_name = source_node->MakeComponentMoniker();
    Collection source_collection = source_node->collection_;

    // If the node already has a manually specified fuchsia.Service property,
    // we disable automatic generation of fuchsia.Service properties for all
    // offers. This allows drivers to manually specify which service should be
    // used for binding, avoiding conflicts when a node offers multiple services.
    bool has_manual_service_property = false;
    for (const auto& property : *natural_resource.properties()) {
      if (property.key() == bind_fuchsia::SERVICE) {
        has_manual_service_property = true;
        break;
      }
    }

    for (const auto& fdf_offer : *natural_resource.offers()) {
      fit::result new_offer = ProcessNodeOfferWithTransportProperty(
          fdf_offer, source_collection, source_name, !has_manual_service_property);
      if (new_offer.is_error()) {
        fdf_log::error("Failed to provide resource '{}': Bad offer: {}",
                       natural_resource.name().value(), new_offer.error_value());
        completer.ReplyError(new_offer.error_value());
        return;
      }
      auto [processed_offer, service_property] = std::move(new_offer.value());
      node_offers.push_back(std::move(processed_offer));
      if (service_property.has_value()) {
        natural_resource.properties()->push_back(std::move(service_property.value()));
      }
    }
  }
  auto resource = std::make_shared<Resource>(
      id, weak_from_this(), std::move(natural_resource.name().value()),
      std::move(natural_resource.properties().value()), std::move(node_offers),
      std::move(natural_resource.bus_info()), dispatcher_);
  resource->Bind(std::move(request->controller));
  (*node_manager_)->Bind(*resource, nullptr);
  provided_resources_.push_back(std::move(resource));

  completer.ReplySuccess();
}

void Node::RemoveResource(const std::shared_ptr<Resource>& resource) {
  // Only remove the resource if it's not the self resource.
  if (resource->is_self_resource()) {
    return;
  }
  std::erase_if(provided_resources_, [&resource](const auto& r) { return r == resource; });
}

void Node::InitializeSelfResource(std::vector<fuchsia_driver_framework::NodeProperty2> properties,
                                  std::vector<NodeOffer> offers,
                                  std::optional<fuchsia_driver_framework::BusInfo> bus_info) {
  ResourceId id = (*node_manager_)->GetNextResourceId();
  self_resource_ = std::make_shared<Resource>(id, weak_from_this(), name_, std::move(properties),
                                              std::move(offers), std::move(bus_info), dispatcher_);
  self_resource_->set_is_self_resource(true);
}

void Node::OnNodeServerUnbound(fidl::UnbindInfo info) {
  // If the unbind is initiated from us, we don't need to do anything to handle
  // the closure.
  if (info.is_user_initiated()) {
    return;
  }

  if (GetNodeShutdownCoordinator().IsShuttingDown()) {
    return;
  }

  // If the driver fails to bind to the node, don't remove the node.
  if (IsPendingBind()) {
    fdf_log::warn("The driver for node {} failed to bind.", name());
    return;
  }

  if (GetNodeState() == NodeState::kRunning) {
    // If the node is running but this node closure has happened, then we want to restart
    // the node if it has the host_restart_on_crash_ enabled on it.
    if (host_restart_on_crash_) {
      fdf_log::info("Restarting node {} due to node closure while running.", name());
      RestartNodeOnCrash();
      return;
    }

    fdf_log::warn("fdf::Node binding for node {} closed while the node was running: {}", name(),
                  info.FormatDescription());
  }

  Remove(RemovalSet::kAll, nullptr);
}

void Node::Remove(RemoveCompleter::Sync& completer) {
  fdf_log::debug("Remove() Fidl call for {}", name());
  SetShouldDestroy();
  Remove(RemovalSet::kAll, nullptr);
}

void Node::RequestBind(RequestBindRequestView request, RequestBindCompleter::Sync& completer) {
  bool force_rebind = false;
  if (request->has_force_rebind()) {
    force_rebind = request->force_rebind();
  }

  std::optional<std::string> driver_url_suffix;
  if (request->has_driver_url_suffix()) {
    driver_url_suffix = std::string(request->driver_url_suffix().get());
  }

  BindHelper(force_rebind, std::move(driver_url_suffix),
             [completer = completer.ToAsync()](zx_status_t status) mutable {
               if (status == ZX_OK) {
                 completer.ReplySuccess();
               } else {
                 completer.ReplyError(status);
               }
             });
}

void Node::WaitForDriver(WaitForDriverCompleter::Sync& completer) {
  fidl::Arena arena;

  // First check if we are a driver component that is already started and running.
  if (auto* driver_component = std::get_if<DriverComponent>(&state_); driver_component) {
    if (driver_component->state == DriverState::kRunning) {
      zx::result token_res = DuplicateNodeToken();
      if (token_res.is_ok()) {
        completer.ReplySuccess(
            fdf::wire::DriverResult::WithDriverStartedNodeToken(std::move(token_res.value())));
      } else {
        completer.ReplyError(token_res.error_value());
      }

      return;
    }
  }

  // Then if we are a composite check if we have a child (completed composite) that is started and
  // running.
  if (auto* composite_parent = std::get_if<CompositeParent>(&state_); composite_parent) {
    if (children().size() > 1) {
      fdf_log::warn(
          "Node {} is a composite parent to multiple composites, the WaitForDriver will only report on the first child to be found running.",
          name());
    }
    for (const auto& child : children()) {
      if (auto* driver_component = std::get_if<DriverComponent>(&child->state_); driver_component) {
        if (driver_component->state == DriverState::kRunning) {
          zx::result token_res = child->DuplicateNodeToken();
          if (token_res.is_ok()) {
            completer.ReplySuccess(
                fdf::wire::DriverResult::WithDriverStartedNodeToken(std::move(token_res.value())));
          } else {
            completer.ReplyError(token_res.error_value());
          }

          return;
        }
      }
    }
  }

  if (bind_wait_completer_) {
    fdf_log::warn("WaitForDriver for {} is already called.", name());
    completer.ReplyError(ZX_ERR_ALREADY_EXISTS);
    return;
  }

  // Otherwise set our completer, which will be called when OnBind happens or we boot-up without a
  // successful bind on this node or a child node for the composite case.
  bind_wait_completer_ =
      [completer = completer.ToAsync()](zx::result<fdf::wire::DriverResult> result) mutable {
        if (result.is_error()) {
          completer.ReplyError(result.error_value());
        } else {
          completer.ReplySuccess(std::move(result.value()));
        }
      };

  // If boot-up completes without us having reported a success, then failure will be sent here.
  // Otherwise this will just be a no-op since the bind_err_ is reset on success.
  // For multiple errors, the last error will be reported since it will be read from the bind_err_.
  (*node_manager_)->WaitForBootup([weak = weak_from_this()]() {
    std::shared_ptr<Node> self = weak.lock();
    if (!self) {
      return;
    }

    if (!self->bind_err_) {
      return;
    }

    if (self->IsComposite()) {
      for (auto& parent_ref : self->parents()) {
        std::shared_ptr<Node> parent = parent_ref.lock();
        if (parent && parent->bind_wait_completer_) {
          parent->bind_wait_completer_(zx::ok(*std::move(self->bind_err_)));
        }
      }
    } else if (self->bind_wait_completer_) {
      self->bind_wait_completer_(zx::ok(*std::move(self->bind_err_)));
    }
  });
}

void Node::BindHelper(bool force_rebind, std::optional<std::string> driver_url_suffix,
                      fit::callback<void(zx_status_t)> on_bind_complete) {
  if (std::holds_alternative<DriverComponent>(state_) && !force_rebind) {
    on_bind_complete(ZX_ERR_ALREADY_BOUND);
    return;
  }

  if (pending_bind_completer_) {
    on_bind_complete(ZX_ERR_ALREADY_EXISTS);
    return;
  }

  auto completer_wrapper = [on_bind_complete =
                                std::move(on_bind_complete)](zx::result<> result) mutable {
    on_bind_complete(result.status_value());
  };

  if (std::holds_alternative<DriverComponent>(state_)) {
    RestartNodeWithRematch(driver_url_suffix, std::move(completer_wrapper));
    return;
  }

  pending_bind_completer_ = std::move(completer_wrapper);
  auto tracker = CreateBindResultTracker();
  if (driver_url_suffix.has_value()) {
    node_manager_.value()->BindToUrl(*this, driver_url_suffix.value(), std::move(tracker));
  } else {
    node_manager_.value()->Bind(*this, std::move(tracker));
  }
}

void Node::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_driver_framework::NodeController> metadata,
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

  fdf_log::warn("fdf::NodeController received unknown {} method. Ordinal: {}", method_type,
                metadata.method_ordinal);
}

void Node::AddChild(AddChildRequestView request, AddChildCompleter::Sync& completer) {
  AddChild(fidl::ToNatural(request->args), std::move(request->controller), std::move(request->node),
           [completer = completer.ToAsync()](
               fit::result<fdf::NodeError, std::shared_ptr<Node>> result) mutable {
             if (result.is_error()) {
               completer.Reply(result.take_error());
             } else {
               completer.ReplySuccess();
             }
           });
}

void Node::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_driver_framework::Node> metadata,
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

  fdf_log::warn("fdf::Node received unknown {} method. Ordinal: {}", method_type,
                metadata.method_ordinal);
}

// This structure is based on the definition in fuchsia.io/Directory in the doc
// comment for Directory.ReadDirents.
struct dir_ent {
  // Describes the inode of the entry.
  uint64_t ino;
  // Describes the length of the dirent name in bytes.
  uint8_t size;
  // Describes the type of the entry. Aligned with the
  // POSIX d_type values.
  uint8_t type;
  // Unterminated name of entry.
  char name[0];
} __PACKED;

// Manually scans the directory entries in a namespace to find a specific protocol.
// We do this manual scan rather than just attempting to `Open` the protocol directly
// because `Open` is asynchronous and may not fail immediately if the protocol is
// missing. By verifying its existence first, we avoid relying on a silent failure
// or hanging behavior when a custom power topology isn't provided.
void Node::SearchNamespaceSvcDirForEntry(
    fidl::ClientEnd<fuchsia_io::Directory> svc_dir, std::string_view entry_name,
    fit::callback<void(zx::result<fidl::ClientEnd<fuchsia_io::Directory>>)> cb) {
  // Manual scan of directory entries to find a specific protocol.
  // We do this because Open is asynchronous and may not fail immediately.
  auto cloned_svc_dir_result = component::Clone(svc_dir);
  if (cloned_svc_dir_result.is_error()) {
    cb(zx::error(cloned_svc_dir_result.status_value()));
    return;
  }
  auto cloned_svc_dir = std::move(cloned_svc_dir_result.value());

  auto client =
      std::make_shared<fidl::Client<fuchsia_io::Directory>>(std::move(cloned_svc_dir), dispatcher_);
  std::string protocol_name(entry_name);

  auto recursive_read = std::make_shared<fit::function<void(
      std::shared_ptr<fidl::Client<fuchsia_io::Directory>>, std::string,
      fidl::ClientEnd<fuchsia_io::Directory>,
      fit::callback<void(zx::result<fidl::ClientEnd<fuchsia_io::Directory>>)>)>>();

  *recursive_read =
      [recursive_read = recursive_read](
          std::shared_ptr<fidl::Client<fuchsia_io::Directory>> client_ptr,
          std::string protocol_name, fidl::ClientEnd<fuchsia_io::Directory> svc_dir,
          fit::callback<void(zx::result<fidl::ClientEnd<fuchsia_io::Directory>>)> cb) mutable {
        // Read 60K at a time. The max channel message size is 64K, we leave a
        // little margine for error here.
        (*client_ptr)
            ->ReadDirents({60 * 1024})
            .Then([client_ptr, svc_dir = std::move(svc_dir), protocol_name, cb = std::move(cb),
                   recursive_read](
                      fidl::Result<fuchsia_io::Directory::ReadDirents>& result) mutable {
              if (result.is_error()) {
                fdf_log::error("Failed to read `/svc` dir: {}",
                               result.error_value().FormatDescription());
                cb(zx::error(result.error_value().status()));
                // Break the cycle
                *recursive_read = nullptr;
                return;
              }

              const std::vector<uint8_t>& dirents = result->dirents();
              if (dirents.empty()) {
                cb(zx::error(ZX_ERR_NOT_FOUND));
                // Break the cycle
                *recursive_read = nullptr;
                return;
              }

              size_t dir_ent_base_size = sizeof(dir_ent);

              size_t offset = 0;
              while ((offset + dir_ent_base_size) < dirents.size()) {
                dir_ent* entry = reinterpret_cast<dir_ent*>(const_cast<uint8_t*>(&dirents[offset]));

                if (offset + dir_ent_base_size + entry->size > dirents.size()) {
                  break;
                }

                // If we find the entry, call the callback and exit.
                if (entry->size == protocol_name.length() &&
                    strncmp(entry->name, protocol_name.c_str(), protocol_name.length()) == 0) {
                  cb(zx::ok(std::move(svc_dir)));
                  // Break the cycle
                  *recursive_read = nullptr;
                  return;
                }
                offset = offset + dir_ent_base_size + entry->size;
              }

              // Did not find it in this chunk, read more.
              (*recursive_read)(client_ptr, protocol_name, std::move(svc_dir), std::move(cb));
            });
      };

  (*recursive_read)(client, protocol_name, std::move(svc_dir), std::move(cb));
}

void Node::StartDriver(
    fuchsia_component_runner::wire::ComponentStartInfo start_info,
    fidl::ServerEnd<fuchsia_component_runner::ComponentController> component_controller,
    fit::callback<void(zx::result<>)> cb) {
  if (GetNodeState() == NodeState::kStopped) {
    GetNodeShutdownCoordinator().ResetShutdown();
  }

  auto url = start_info.resolved_url().get();

  bool colocate =
      fdf_internal::ProgramValue(start_info.program(), "colocate").value_or("") == "true";
  bool host_restart_on_crash =
      fdf_internal::ProgramValue(start_info.program(), "host_restart_on_crash").value_or("") ==
      "true";
  bool use_next_vdso =
      fdf_internal::ProgramValue(start_info.program(), "use_next_vdso").value_or("") == "true";
  bool use_dynamic_linker =
      fdf_internal::ProgramValue(start_info.program(), "use_dynamic_linker").value_or("") == "true";

  state_ = Starting{.driver_url = std::string(url)};

  if (host_restart_on_crash && colocate) {
    fdf_log::error(
        "Failed to start driver '{}'. Both host_restart_on_crash and colocate cannot be true.",
        url);
    cb(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  host_restart_on_crash_ = host_restart_on_crash;
  is_driver_host_root_ = !colocate;

  if (colocate && !driver_host()) {
    fdf_log::error(
        "Failed to start driver '{}', driver is colocated but does not have a parent with a "
        "driver host",
        url);
    cb(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  if (colocate && !driver_host_name_for_colocation_.empty() && driver_host() &&
      driver_host()->name_for_colocation() != driver_host_name_for_colocation_) {
    fdf_log::warn(
        "Driver '{}' requested 'colocate: true' with a custom colocation group '{}' that does not "
        "match the parent. Ignoring this custom name; placing driver in parent group '{}'",
        url, driver_host_name_for_colocation_, driver_host()->name_for_colocation());
  }

  bool found_driver_host = colocate;

  fidl::ClientEnd<fuchsia_io::Directory> svc_dir;
  if (start_info.has_ns()) {
    for (auto& entry : start_info.ns()) {
      if (entry.path().get() == "/svc") {
        auto result = component::Clone(entry.directory());
        if (result.is_ok()) {
          svc_dir = std::move(result.value());
        }
        break;
      }
    }
  }

  fidl::Arena arena;

  auto symbols = fidl::VectorView<fdf::wire::NodeSymbol>();
  if (colocate) {
    symbols = fidl::ToWire(arena, this->symbols());
  }

  std::vector<fdf::Offer> natural_offers;
  natural_offers.reserve(offers().size());
  std::ranges::transform(offers(), std::back_inserter(natural_offers), ToFidl);
  auto offers = fidl::ToWire(arena, natural_offers);
  auto properties = fidl::ToWire(arena, GetNodePropertyDict());

  if (found_driver_host) {
    // Whether dynamic linking is enabled for a driver host is determined by the first driver in the
    // host. Otherwise for colocated drivers, we need to match what has been set for the driver
    // host.
    if (use_dynamic_linker != driver_host()->IsDynamicLinkingEnabled()) {
      fdf_log::error(
          "Failed to start driver '{}', driver is colocated and set use_dynamic_linker={} "
          "but its driver host is not configured for this",
          url, use_dynamic_linker ? "true" : "false");
      cb(zx::error(ZX_ERR_INVALID_ARGS));
      return;
    }
  }
  std::optional<DriverHost::DriverLoadArgs> dynamic_linker_load_args;
  std::optional<DriverHost::DriverStartArgs> dynamic_linker_start_args;
  if (use_dynamic_linker) {
    auto result = DriverHost::DriverLoadArgs::Create(start_info);
    if (result.is_error()) {
      cb(result.take_error());
      return;
    }
    dynamic_linker_load_args = std::move(*result);
    dynamic_linker_start_args =
        DriverHost::DriverStartArgs(properties, symbols, offers, start_info);
  }

  // These are the start args used if we're not starting with the dynamic linker.
  DriverHost::DriverStartArgs normal_start_args =
      DriverHost::DriverStartArgs(properties, symbols, offers, start_info);
  std::vector<fuchsia_power_broker::DependencyToken> deps;

  bool suspend_enabled_for_node = (*node_manager_)->SuspendEnabled() || IsHermeticPowerTest();

  // Only create a series of dependencies if this is a suspend-enabled platform. On non-enabled
  // platforms we'll use an empty vector for the rest of this function, which is valid. The
  // rest of the driver start code also has checks so it works properly on non-enabled platforms.
  if (suspend_enabled_for_node) {
    if (power_dependency_overrides_.has_value()) {
      for (const auto& dep : power_dependency_overrides_.value()) {
        if (!dep.requires_token().has_value()) {
          fdf_log::error("Power dependency override is missing requires_token, skipping.");
          continue;
        }
        fuchsia_power_broker::DependencyToken clone;
        zx_status_t dupe_result = dep.requires_token()->duplicate(ZX_RIGHT_SAME_RIGHTS, &clone);
        if (dupe_result != ZX_OK) {
          fdf_log::error("Power token duplicate failed for node: {}", std::string(url));
          cb(zx::error(dupe_result));
          return;
        }
        deps.push_back(std::move(clone));
      }
    } else {
      std::span<const std::weak_ptr<Node>> parent_nodes = parents();
      std::queue<std::weak_ptr<Node>> ancestors;
      for (const auto& parent : parent_nodes) {
        ancestors.push(parent);
      }

      std::unordered_set<zx_handle_t> visited_ancestor_koids;

      // Collect dependencies on the most immediate ancestor nodes which have attached drivers.
      // A parent driver might give us a valid token if:
      //   * the parent went away after we identified it
      //   * the parent failed registering a dependency token when it was created
      //   * duplication of the parent's dependency token fails
      while (!ancestors.empty()) {
        std::shared_ptr<Node> ancestor = ancestors.front().lock();
        ancestors.pop();
        if (!ancestor) {
          continue;
        }

        // If this node is part of a power test, but the ancestor is not, skip
        // the ancestor because we can't mix, in-test and out-of-test nodes.
        if (IsHermeticPowerTest() && !ancestor->IsHermeticPowerTest()) {
          continue;
        }

        if (!ancestor->is_bound()) {
          for (const auto& elder : ancestor->parents()) {
            ancestors.push(elder);
          }
          continue;
        }

        // Prevent multiple dependencies on the same ancestor.
        if (visited_ancestor_koids.contains(ancestor->GetPowerTokenHandle())) {
          // We arrived back at the same ancestors through multiple parents.
          continue;
        }

        zx::event token_copy = ancestor->DuplicatePowerToken();
        if (!token_copy.is_valid()) {
          fdf_log::error("Power token invalid on suspend-enabled platform for node: {}",
                         std::string(url));
          cb(zx::error(ZX_ERR_BAD_STATE));
          return;
        }

        visited_ancestor_koids.insert(ancestor->GetPowerTokenHandle());
        deps.push_back(std::move(token_copy));
      }
    }
  }

  zx::event clone;
  ZX_ASSERT_MSG(power_element_token_.duplicate(ZX_RIGHT_SAME_RIGHTS, &clone) == ZX_OK,
                "Event duplication failed while collecting power element dependencies");

  auto [lessor_client, lessor_server] = fidl::Endpoints<fuchsia_power_broker::Lessor>::Create();
  auto [element_control_client, element_control_server] =
      fidl::Endpoints<fuchsia_power_broker::ElementControl>::Create();
  auto [element_runner_client, element_runner_server] =
      fidl::Endpoints<fuchsia_power_broker::ElementRunner>::Create();

  // Create a shared pointer because we want to create a callbacks below that have access to these
  // handles and then after these callbacks are created we use one of the handles to make a FIDL
  // call.
  std::shared_ptr<PowerElementHandles> handles_ptr = std::make_shared<PowerElementHandles>(
      fidl::Client<fuchsia_power_broker::ElementControl>(std::move(element_control_client),
                                                         dispatcher_),
      std::move(element_runner_server),
      fidl::Client<fuchsia_power_broker::Lessor>(std::move(lessor_client), dispatcher_));

  auto url_str = std::string(start_info.resolved_url().get());

  std::optional<fuchsia_power_broker::DependencyToken> cpu_token_override;
  if (cpu_token_override_.has_value()) {
    fuchsia_power_broker::DependencyToken clone_token;
    ZX_ASSERT_MSG(cpu_token_override_->duplicate(ZX_RIGHT_SAME_RIGHTS, &clone_token) == ZX_OK,
                  "Event duplication failed for cpu token override");
    cpu_token_override = std::move(clone_token);
  }

  fit::callback<void(zx::result<std::optional<fidl::ClientEnd<fuchsia_power_broker::Topology>>>)>
      do_start_driver = [weak_self = weak_from_this(), handles_ptr = handles_ptr,
                         cb = std::move(cb), use_dynamic_linker = use_dynamic_linker, url = url_str,
                         found_driver_host = found_driver_host,
                         dynamic_linker_start_args = std::move(dynamic_linker_start_args),
                         dynamic_linker_load_args = std::move(dynamic_linker_load_args),
                         component_controller = std::move(component_controller),
                         use_next_vdso = use_next_vdso,
                         normal_start_args = std::move(normal_start_args), clone = std::move(clone),
                         deps = std::move(deps),
                         element_control_server = std::move(element_control_server),
                         element_runner_client = std::move(element_runner_client),
                         lessor_server = std::move(lessor_server),
                         cpu_token_override = std::move(cpu_token_override)](
                            zx::result<
                                std::optional<fidl::ClientEnd<fuchsia_power_broker::Topology>>>
                                topology_client) mutable {
        std::shared_ptr<driver_manager::Node> self = weak_self.lock();
        if (!self) {
          cb(zx::error(ZX_ERR_CANCELED));
          return;
        }

        std::optional<fidl::ClientEnd<fuchsia_power_broker::Topology>> topology_channel =
            std::nullopt;
        if (topology_client.is_ok()) {
          topology_channel = std::move(topology_client.value());
        }

        bool create_startup_lease =
            (*self->node_manager_)->SuspendEnabled() && !self->IsHermeticPowerTest();
        std::optional<zx::eventpair> startup_lease_peer;
        if (create_startup_lease) {
          zx::eventpair startup_lease, lease_peer;
          zx_status_t status = zx::eventpair::create(0, &startup_lease, &lease_peer);
          ZX_ASSERT(status == ZX_OK);
          self->startup_lease_ = std::move(startup_lease);
          startup_lease_peer = std::move(lease_peer);
        }

        (*self->node_manager_)
            ->CreatePowerElement(
                std::move(topology_channel), self->name_, std::move(clone), std::move(deps),
                std::move(element_control_server), std::move(element_runner_client),
                std::move(lessor_server), self->collection(), std::move(cpu_token_override),
                std::move(startup_lease_peer), self->IsHermeticPowerTest(),
                [weak_self = self->weak_from_this(), handles_ptr = handles_ptr, cb = std::move(cb),
                 use_dynamic_linker = use_dynamic_linker, url = url,
                 found_driver_host = found_driver_host,
                 dynamic_linker_start_args = std::move(dynamic_linker_start_args),
                 dynamic_linker_load_args = std::move(dynamic_linker_load_args),
                 component_controller = std::move(component_controller),
                 use_next_vdso = use_next_vdso, normal_start_args = std::move(normal_start_args)](
                    zx::result<bool> pe_created) mutable {
                  std::shared_ptr<driver_manager::Node> self = weak_self.lock();
                  if (!self) {
                    cb(zx::error(ZX_ERR_CANCELED));
                    return;
                  }

                  if (pe_created.is_error()) {
                    fdf_log::warn("Failed creating power element for driver {}", url);
                    cb(pe_created.take_error());
                    return;
                  }
                  zx::event copy;
                  ZX_ASSERT_MSG(
                      self->power_element_token_.duplicate(ZX_RIGHT_SAME_RIGHTS, &copy) == ZX_OK,
                      "Power element token duplication failed after element creation.");

                  // Run this after we create the lease for the power element, or
                  // immediately if lease is not created because this is not a
                  // power-enabled platform.
                  fit::callback<void(zx::result<>)> create_host_and_start_driver_cb =
                      [weak_self = self->weak_from_this(), found_driver_host = found_driver_host,
                       use_dynamic_linker = use_dynamic_linker,
                       dynamic_linker_load_args = std::move(dynamic_linker_load_args),
                       dynamic_linker_start_args = std::move(dynamic_linker_start_args), url = url,
                       component_controller = std::move(component_controller),
                       use_next_vdso = use_next_vdso, cb = std::move(cb),
                       normal_start_args =
                           std::move(normal_start_args)](zx::result<> power_setup) mutable {
                        std::shared_ptr<driver_manager::Node> self = weak_self.lock();
                        if (!self) {
                          return;
                        }

                        if (power_setup.is_error()) {
                          fdf_log::warn("{} failed to start because power element setup failed",
                                        std::string(url));
                          cb(power_setup);
                          return;
                        }

                        PowerElementStartArgs pe_args;
                        // Package up power-related args for sending to the driver host.
                        if (self->pe_handles_.has_value()) {
                          auto element_control_channel =
                              self->pe_handles_->element_control.UnbindMaybeGetEndpoint();
                          ZX_ASSERT_MSG(element_control_channel.is_ok(),
                                        "Failed unbinding element control channel for transfer");
                          pe_args.element_control = std::move(*element_control_channel);

                          auto lessor_channel = self->pe_handles_->lessor.UnbindMaybeGetEndpoint();
                          ZX_ASSERT_MSG(lessor_channel.is_ok(),
                                        "Failed unbinding lessor channel for transfer");
                          pe_args.lessor = std::move(*lessor_channel);

                          zx::event token_copy = self->DuplicatePowerToken();
                          ZX_ASSERT_MSG(token_copy.is_valid(),
                                        "Power element token invalid on suspend-enabled platform.");
                          pe_args.power_element_token = std::move(token_copy);

                          pe_args.element_runner = std::move(self->pe_handles_->element_runner);
                        }

                        // If we're using the dynamic linker, we don't continue
                        // starting the driver here.
                        if (use_dynamic_linker) {
                          self->CreateHostAndStartDriverWithDynamicLinker(
                              std::move(*dynamic_linker_load_args),
                              std::move(*dynamic_linker_start_args), url,
                              std::move(component_controller), std::move(pe_args),
                              found_driver_host, std::move(cb));
                          return;
                        }

                        if (!found_driver_host) {
                          self->driver_host_ =
                              (*self->node_manager_)
                                  ->GetDriverHost(self->driver_host_name_for_colocation_);
                          if (self->driver_host_) {
                            found_driver_host = true;
                            if (use_dynamic_linker !=
                                self->driver_host()->IsDynamicLinkingEnabled()) {
                              fdf_log::error(
                                  "Failed to start driver '{}', driver is colocated and set "
                                  "use_dynamic_linker={} but its driver host is not configured "
                                  "for this",
                                  url, use_dynamic_linker ? "true" : "false");
                              cb(zx::error(ZX_ERR_INVALID_ARGS));
                              return;
                            }
                          }
                        }

                        // Since we're not colocating we need to create a new driver host.
                        if (!found_driver_host) {
                          auto result =
                              (*self->node_manager_)
                                  ->CreateDriverHost(use_next_vdso,
                                                     self->driver_host_name_for_colocation_);
                          if (result.is_error()) {
                            cb(result.take_error());
                            return;
                          }
                          self->driver_host_ = result.value();
                        }

                        // Finally, talk to the driver host and start the driver.

                        // Bind the Node associated with the driver.
                        auto [client_end, server_end] = fidl::Endpoints<fdf::Node>::Create();
                        fdf_log::info("Binding {} to {}", url, self->name());
                        auto driver_endpoints =
                            fidl::Endpoints<fuchsia_driver_host::Driver>::Create();

                        zx::event node_token;
                        if (normal_start_args.start_info_.component_instance().has_value()) {
                          node_token =
                              std::move(*normal_start_args.start_info_.component_instance());
                        } else {
                          fdf_log::warn("Component instance not provided in start request");
                          ZX_ASSERT(zx::event::create(0, &node_token) == ZX_OK);
                        }

                        zx::event node_token_dup;
                        ZX_ASSERT(node_token.duplicate(ZX_RIGHT_SAME_RIGHTS, &node_token_dup) ==
                                  ZX_OK);

                        self->state_.emplace<DriverComponent>(
                            *self, std::string(url), std::move(component_controller),
                            std::move(server_end), std::move(driver_endpoints.client),
                            std::move(node_token_dup));

                        fidl::Arena arena;
                        auto properties = fidl::ToWire(arena, normal_start_args.node_properties_);
                        auto symbols = fidl::ToWire(arena, normal_start_args.symbols_);
                        auto offers = fidl::ToWire(arena, normal_start_args.offers_);
                        auto start_info =
                            fidl::ToWire(arena, std::move(normal_start_args.start_info_));
                        self->driver_host_.value()->Start(
                            std::move(client_end), self->name_, properties, symbols, offers,
                            start_info, std::move(node_token), std::move(driver_endpoints.server),
                            std::move(pe_args),
                            [weak_self = self->weak_from_this(), name = self->name_,
                             cb = std::move(cb)](zx::result<> result) mutable {
                              auto node_ptr = weak_self.lock();
                              if (!node_ptr) {
                                fdf_log::warn("Node '{}' freed before it is used", name);
                                cb(result);
                                return;
                              }

                              if (result.is_error()) {
                                fdf_log::warn("Failed to start driver host for {}",
                                              node_ptr->MakeComponentMoniker());
                              }
                              cb(result);
                            });
                      };

                  // If this is a suspend-enabled platform, CreatePowerElement returns
                  // zx::result<true>. If suspend isn't enabled, we get a zx::result<false>, which
                  // is not an error, but the element channels are not valid, so we shouldn't stash
                  // them and we shouldn't register a dependency token or lease the power element
                  // since we didn't create it.
                  if (pe_created.value()) {
                    // Now that the power element is created, move the handles for it into the node
                    self->pe_handles_ = std::move(*handles_ptr);

                    // Register a dependency token on the power element so other elements can depend
                    // on it. After doing this we'll call the start callback.
                    self->pe_handles_->element_control
                        ->RegisterDependencyToken({{
                            .token = std::move(copy),
                        }})
                        .Then([weak_self = self->weak_from_this(),
                               create_host_and_start_driver_cb =
                                   std::move(create_host_and_start_driver_cb),
                               url](fidl::Result<
                                    fuchsia_power_broker::ElementControl::RegisterDependencyToken>
                                        call_result) mutable {
                          std::shared_ptr<driver_manager::Node> self = weak_self.lock();
                          if (!self) {
                            create_host_and_start_driver_cb(zx::error(ZX_ERR_CANCELED));
                            return;
                          }
                          if (call_result.is_error()) {
                            fdf_log::warn(
                                "Failed creating power dependency token for {}, this may lead to improper ",
                                std::string(url), "power management behavior.");
                            self->power_element_token_.reset();
                            if (call_result.error_value().is_framework_error()) {
                              create_host_and_start_driver_cb(
                                  zx::error(call_result.error_value().framework_error().status()));
                            } else {
                              create_host_and_start_driver_cb(RegistrationErrorToResult(
                                  call_result.error_value().domain_error()));
                            }
                            return;
                          }
                          create_host_and_start_driver_cb(zx::ok());
                        });
                  } else {
                    // CreatePowerElement returned zx::ok(false).
                    create_host_and_start_driver_cb(zx::ok());
                  }
                });
      };

  // If the platform is not suspend enabled, skip the directory search below.
  if (!suspend_enabled_for_node) {
    do_start_driver(zx::ok(std::nullopt));
    return;
  }

  // Skip the directory search if the driver doesn't have a svc dir.
  if (!svc_dir.is_valid()) {
    fdf_log::info(
        "Node::StartDriver svc_dir unavailable, using driver manager for `fuchsia.power.broker.Topology`.");
    do_start_driver(zx::ok(std::nullopt));
    return;
  }

  // If we're suspend-enabled and the driver has an `svc` dir, look for
  // `Topology` in it.
  SearchNamespaceSvcDirForEntry(
      std::move(svc_dir), fidl::DiscoverableProtocolName<fuchsia_power_broker::Topology>,
      [do_start_driver = std::move(do_start_driver)](
          zx::result<fidl::ClientEnd<fuchsia_io::Directory>> search_result) mutable {
        if (search_result.is_error()) {
          do_start_driver(zx::ok(std::nullopt));
          return;
        }

        auto endpoints = fidl::Endpoints<fuchsia_power_broker::Topology>::Create();
        zx_status_t status =
            fdio_service_connect_at(search_result.value().channel().get(),
                                    fidl::DiscoverableProtocolName<fuchsia_power_broker::Topology>,
                                    endpoints.server.TakeChannel().release());
        if (status != ZX_OK) {
          do_start_driver(zx::ok(std::nullopt));
          return;
        }
        do_start_driver(zx::ok(std::move(endpoints.client)));
      });
}

void Node::OnComponentStarted(const std::weak_ptr<BootupTracker>& bootup_tracker,
                              const std::string& moniker, zx::result<StartedComponent> component) {
  if (component.is_error()) {
    OnStartError(component.error_value());
    CompleteBind(component.take_error());
    if (auto tracker_ptr = bootup_tracker.lock(); tracker_ptr) {
      tracker_ptr->NotifyStartComplete(moniker);
    }
    return;
  }

  fidl::Arena arena;
  StartDriver(fidl::ToWire(arena, std::move(component->info)),
              std::move(component->component_controller),
              [node_weak = weak_from_this(), moniker, bootup_tracker](zx::result<> result) {
                if (std::shared_ptr node = node_weak.lock(); node) {
                  if (result.is_error()) {
                    node->OnStartError(result.error_value());
                  }
                  node->CompleteBind(result);
                }

                if (auto tracker_ptr = bootup_tracker.lock(); tracker_ptr) {
                  tracker_ptr->NotifyStartComplete(moniker);
                }
              });
}

void Node::RequestStartComponent(fuchsia_process::wire::HandleInfo startup_handle,
                                 const std::string& moniker,
                                 const std::weak_ptr<BootupTracker>& bootup_tracker) {
  fidl::Arena arena;
  fidl::VectorView<fuchsia_process::wire::HandleInfo> handles(arena, 1);
  handles[0] = std::move(startup_handle);

  auto [client, server] = fidl::Endpoints<fuchsia_component::ExecutionController>::Create();

  component_controller_
      ->Start(
          fuchsia_component::wire::StartChildArgs::Builder(arena).numbered_handles(handles).Build(),
          std::move(server))
      .Then([bootup_tracker, moniker, node_weak = weak_from_this()](
                fidl::WireUnownedResult<fcomponent::Controller::Start>& result) {
        bool is_error = false;
        if (!result.ok()) {
          fdf_log::error("Failed to send StartComponent. {}", result.status_string());
          is_error = true;
        } else if (result->is_error()) {
          fdf_log::error("Failed to StartComponent. {}", result->error_value());
          is_error = true;
        }

        if (is_error) {
          if (std::shared_ptr node = node_weak.lock(); node) {
            node->OnComponentStarted(bootup_tracker, moniker, zx::error(ZX_ERR_INTERNAL));
          }
        }
      });
}

void Node::CreateHostAndStartDriverWithDynamicLinker(
    DriverHost::DriverLoadArgs load_args, DriverHost::DriverStartArgs start_args,
    std::string_view url,
    fidl::ServerEnd<fuchsia_component_runner::ComponentController> component_controller,
    PowerElementStartArgs power_element_args, bool found_driver_host,
    fit::callback<void(zx::result<>)> cb) {
  if (!found_driver_host) {
    driver_host_ = (*node_manager_)->GetDriverHost(driver_host_name_for_colocation_);
    if (driver_host_) {
      found_driver_host = true;
    }
  }

  if (found_driver_host) {
    if (!driver_host()->IsDynamicLinkingEnabled()) {
      fdf_log::error(
          "Failed to start driver '{}', driver is colocated and set use_dynamic_linker=true "
          "but its driver host is not configured for this",
          url);
      cb(zx::error(ZX_ERR_INVALID_ARGS));
      return;
    }
    StartDriverWithDynamicLinker(std::move(load_args), std::move(start_args), url,
                                 std::move(component_controller), std::move(power_element_args),
                                 std::move(cb));
    return;
  }

  (*node_manager_)
      ->CreateDriverHostDynamicLinker(
          driver_host_name_for_colocation_,
          [weak_self = weak_from_this(), name = name_, load_args = std::move(load_args),
           start_args = std::move(start_args), url = std::string(url),
           component_controller = std::move(component_controller),
           power_element_args = std::move(power_element_args),
           cb = std::move(cb)](zx::result<DriverHost*> driver_host) mutable {
            std::shared_ptr<driver_manager::Node> node_ptr = weak_self.lock();
            if (!node_ptr) {
              fdf_log::warn("Node '{}' freed before it is used", name);
              cb(zx::error(ZX_ERR_BAD_STATE));
              return;
            }

            if (driver_host.is_error()) {
              cb(driver_host.take_error());
              return;
            }
            node_ptr->driver_host_ = driver_host.value();
            node_ptr->StartDriverWithDynamicLinker(std::move(load_args), std::move(start_args), url,
                                                   std::move(component_controller),
                                                   std::move(power_element_args), std::move(cb));
          });
}

void Node::StartDriverWithDynamicLinker(
    DriverHost::DriverLoadArgs load_args, DriverHost::DriverStartArgs start_args,
    std::string_view url,
    fidl::ServerEnd<fuchsia_component_runner::ComponentController> component_controller,
    PowerElementStartArgs power_element_args, fit::callback<void(zx::result<>)> cb) {
  auto [client_end, server_end] = fidl::Endpoints<fdf::Node>::Create();

  auto driver_endpoints = fidl::Endpoints<fuchsia_driver_host::Driver>::Create();

  zx::event node_token, node_token_dup;
  if (start_args.start_info_.component_instance().has_value()) {
    node_token = std::move(start_args.start_info_.component_instance().value());
  } else {
    ZX_ASSERT(zx::event::create(0, &node_token) == ZX_OK);
  }
  ZX_ASSERT(node_token.duplicate(ZX_RIGHT_SAME_RIGHTS, &node_token_dup) == ZX_OK);

  state_.emplace<DriverComponent>(*this, std::string(url), std::move(component_controller),
                                  std::move(server_end), std::move(driver_endpoints.client),
                                  std::move(node_token_dup));
  driver_host()->StartWithDynamicLinker(std::move(client_end), name_, std::move(load_args),
                                        std::move(start_args), std::move(node_token),
                                        std::move(driver_endpoints.server),
                                        std::move(power_element_args), std::move(cb));
}

zx::result<zx::event> Node::DuplicateNodeToken() {
  ZX_ASSERT(std::holds_alternative<DriverComponent>(state_));

  zx::event node_token;
  zx_status_t status = std::get<DriverComponent>(state_).component_instance.duplicate(
      ZX_RIGHT_SAME_RIGHTS, &node_token);
  if (status != ZX_OK) {
    fdf_log::error("Failed to DuplicateNodeToken: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  return zx::ok(std::move(node_token));
}

void Node::PrepareDictionary(fit::callback<void(zx::result<>)> callback) {
  std::optional<fuchsia_component_sandbox::CapabilityId> to_export;
  if (subtree_dictionary_ref_.has_value()) {
    to_export = subtree_dictionary_ref_;
  } else if (dictionary_ref_.has_value()) {
    to_export = dictionary_ref_;
  }

  if (to_export) {
    (*node_manager_)
        ->dictionary_util()
        .CopyExportDictionary(
            to_export.value(),
            [weak_self = weak_from_this(), callback = std::move(callback)](
                zx::result<fuchsia_component_sandbox::DictionaryRef> result) mutable {
              std::shared_ptr self = weak_self.lock();
              if (!self) {
                return;
              }
              if (result.is_error()) {
                callback(result.take_error());
                return;
              }

              self->offers_dictionary_ = std::move(result.value());
              callback(zx::ok());
            });
    return;
  }

  if (!IsComposite()) {
    // No dictionary and not a composite.
    callback(fit::ok());
    return;
  }

  // We are a composite with parents who may or may not have dictionaries.
  struct MapEntry {
    fuchsia_component_sandbox::CapabilityId dictionary_ref;
    NodeOffer offer;
    std::string parent_name;
    bool is_primary;
  };
  std::unordered_map<std::string, std::vector<MapEntry>> service_map;

  const auto& parent_names = std::get<Composite>(type_).parents_names_;
  auto primary_index = std::get<Composite>(type_).primary_index_;

  // Populate the service map.
  for (size_t i = 0; i < received_resources_.size(); i++) {
    std::shared_ptr<Node> parent = received_resources_[i]->owner().lock();
    if (!parent) {
      continue;
    }

    if (!parent->dictionary_ref_.has_value()) {
      continue;
    }

    for (const auto& offer : received_resources_[i]->offers()) {
      if (offer.transport != OfferTransport::Dictionary) {
        continue;
      }

      service_map[offer.service_name].emplace_back(MapEntry{
          .dictionary_ref = parent->dictionary_ref_.value(),
          .offer = offer,
          .parent_name = parent_names[i],
          .is_primary = primary_index == i,
      });
    }
  }

  size_t total_shards = 0;
  for (auto& [_, entries] : service_map) {
    total_shards += entries.size();
  }

  // No dictionary based services.
  if (total_shards == 0) {
    callback(fit::ok());
    return;
  }

  struct ShardResult {
    fidl::ClientEnd<fuchsia_io::Directory> dir;
    NodeOffer offer;
    std::string parent_name;
    bool is_primary;
  };

  std::shared_ptr<ResultAsyncSharder<ShardResult>> sharder = std::make_shared<
      ResultAsyncSharder<ShardResult>>(
      total_shards, [weak_self = weak_from_this(), callback = std::move(callback)](
                        zx::result<std::vector<ShardResult>> result) mutable {
        std::shared_ptr self = weak_self.lock();
        if (!self) {
          return;
        }
        if (result.is_error()) {
          callback(result.take_error());
          return;
        }

        std::unordered_map<std::string, std::vector<DirInfo>> dirs;
        for (ShardResult& shard : result.value()) {
          std::unordered_map<std::string, std::string> target_to_source_instance_mapping;
          for (auto& mapping : shard.offer.renamed_instances) {
            target_to_source_instance_mapping[mapping.target_name()] = mapping.source_name();
          }

          dirs[shard.offer.service_name].emplace_back(DirInfo{
              .dir = std::move(shard.dir),
              .target_service_name = shard.offer.service_name,
              .target_to_source_instance_mapping = target_to_source_instance_mapping,
              .parent_name = shard.parent_name,
              .is_primary = shard.is_primary,
          });
        }

        std::unordered_map<std::string, fidl::ClientEnd<fuchsia_component_sandbox::DirReceiver>>
            map;

        for (auto& offer : self->offers()) {
          if (offer.transport != OfferTransport::Dictionary || !dirs.contains(offer.service_name)) {
            continue;
          }

          auto [client, server] = fidl::Endpoints<fuchsia_component_sandbox::DirReceiver>::Create();
          map[offer.service_name] = std::move(client);
          self->dir_receivers_.emplace_back(std::move(server), std::move(dirs[offer.service_name]),
                                            self->dispatcher_);
          dirs.erase(offer.service_name);
        }

        (*self->node_manager_)
            ->dictionary_util()
            .CreateDictionaryWith(
                std::move(map),
                [weak_self, callback = std::move(callback)](
                    zx::result<fuchsia_component_sandbox::CapabilityId> result) mutable {
                  std::shared_ptr self = weak_self.lock();
                  if (!self) {
                    return;
                  }
                  if (!result.is_ok()) {
                    callback(result.take_error());
                    return;
                  }

                  ZX_ASSERT_MSG(!self->subtree_dictionary_ref_.has_value(),
                                "Cannot set dictionary_ref_ on node %s", self->name().c_str());
                  self->dictionary_ref_ = result.value();
                  (*self->node_manager_)
                      ->dictionary_util()
                      .CopyExportDictionary(
                          self->dictionary_ref_.value(),
                          [weak_self, callback = std::move(callback)](
                              zx::result<fuchsia_component_sandbox::DictionaryRef> result) mutable {
                            std::shared_ptr self = weak_self.lock();
                            if (!self) {
                              return;
                            }
                            if (result.is_error()) {
                              callback(result.take_error());
                              return;
                            }

                            self->offers_dictionary_ = std::move(result.value());
                            callback(zx::ok());
                          });
                });
      });

  for (auto& [_, entries] : service_map) {
    for (const auto& [dictionary_ref, offer, parent_name, is_primary] : entries) {
      (*node_manager_)
          ->dictionary_util()
          .DictionaryDirConnectorOpen(
              dictionary_ref, offer.service_name,
              [sharder, offer, parent_name,
               is_primary](zx::result<fidl::ClientEnd<fuchsia_io::Directory>> result) {
                if (result.is_ok()) {
                  sharder->CompleteShard({
                      .dir = std::move(result.value()),
                      .offer = offer,
                      .parent_name = parent_name,
                      .is_primary = is_primary,
                  });
                } else {
                  sharder->CompleteShardError(result.error_value());
                }
              });
    }
  }
}

bool Node::EvaluateRematchFlags(fuchsia_driver_development::RestartRematchFlags rematch_flags,
                                std::string_view requested_url) const {
  if (IsComposite() &&
      !(rematch_flags & fuchsia_driver_development::RestartRematchFlags::kCompositeSpec)) {
    return false;
  }

  if (driver_url() == requested_url &&
      !(rematch_flags & fuchsia_driver_development::RestartRematchFlags::kRequested)) {
    return false;
  }

  if (driver_url() != requested_url &&
      !(rematch_flags & fuchsia_driver_development::RestartRematchFlags::kNonRequested)) {
    return false;
  }

  return true;
}

NodeInfo Node::GetRemovalTrackerInfo() {
  return NodeInfo{
      .name = MakeComponentMoniker(),
      .driver_url = driver_url(),
      .collection = collection_,
      .state = GetNodeState(),
      .node = weak_from_this(),
  };
}

void Node::StopDriver() {
  ZX_ASSERT_MSG(GetNodeState() == NodeState::kWaitingOnChildren,
                "StopDriver called in invalid node state: %s",
                GetNodeShutdownCoordinator().NodeStateAsString());
  if (!HasDriver()) {
    return;
  }
  auto& driver_component = std::get<DriverComponent>(state_);

  if (driver_component.state == DriverState::kBinding) {
    fdf_log::warn("Stopping driver '{}' for node '{}' while bind is in process",
                  driver_component.driver_url, MakeComponentMoniker());
    return;
  }

  fidl::OneWayStatus result = driver_component.driver->Stop();
  if (result.ok()) {
    return;  // We'll now wait for the channel to close
  }

  fdf_log::error("Node: {} failed to stop driver: {}", name(), result.FormatDescription());
  // Continue to clear out the driver, since we can't talk to it.
  ClearHostDriver();
}

void Node::StopDriverComponent() {
  ZX_ASSERT_MSG(GetNodeState() == NodeState::kWaitingOnDriver,
                "StopDriverComponent called in invalid node state: %s",
                GetNodeShutdownCoordinator().NodeStateAsString());
  auto* driver_component = std::get_if<DriverComponent>(&state_);
  if (!driver_component) {
    return;
  }

  // If its already stopped we don't need to send the OnStop.
  if (driver_component->state == DriverState::kStopped) {
    return;
  }

  // Send an epitaph to the component manager and close the connection. The
  // server of a `ComponentController` protocol is expected to send an epitaph
  // before closing the associated connection.
  fit::result<fidl::OneWayError> res =
      fidl::SendEvent(driver_component->runner_component_controller_ref)
          ->OnStop(fuchsia_component_runner::ComponentStopInfo{});
  if (res.is_error()) {
    fdf_log::warn("Node::StopDriverComponent failed to send OnStop event.");
  }
}

bool Node::MaybeDestroyDriverComponent() {
  ZX_ASSERT_MSG(component_controller_.is_valid(),
                "DestroyDriverComponent called without a valid controller.");

  // The non-removal intent flows require destroying the component and making a new one.
  // Also if we are waiting for the removal, we want to destroy the component as either a
  // new node is intended to replace the existing which requires a new component, or the
  // parent node is trying to remove the child node in which case the child has to destroy
  // its component to proceed with removal. We also want to destroy the root node if it is
  // stopping, hence the empty parent check.
  if ((shutdown_intent() != ShutdownIntent::kRemoval || remove_complete_callback_ ||
       should_destroy_ || parents().empty())) {
    component_controller_->Destroy().Then(
        [weak_self = weak_from_this()](
            fidl::WireUnownedResult<fuchsia_component::Controller::Destroy>& result) {
          std::shared_ptr self = weak_self.lock();
          if (!self) {
            return;
          }

          if (!result.ok()) {
            fdf_log::error("Node: {}: Failed to send request to destroy component: {}", self->name_,
                           result.error().FormatDescription());
          } else if (result->is_error()) {
            fdf_log::error("Node: {}: Failed to destroy driver component: {}", self->name_,
                           result->error_value());
          }

          fdf_log::debug("Destroy component started for {}", self->MakeComponentMoniker());
          // Reset this flag since we used it.
          self->should_destroy_ = false;
        });

    return true;
  }

  return false;
}

void Node::OnDriverHostFidlError(fidl::UnbindInfo info) {
  ClearHostDriver();

  // The only valid way a driver host should shut down the Driver channel
  // is with the ZX_OK epitaph.
  // TODO(b/322235974): Increase the log severity to ERROR once we resolve the component
  // shutdown order in DriverTestRealm.
  if (info.reason() != fidl::Reason::kPeerClosedWhileReading || info.status() != ZX_OK) {
    fdf_log::warn("Node: {}: driver channel shutdown with: {}", name(), info.FormatDescription());
  }

  // Expected driver host closure.
  if (GetNodeState() == NodeState::kWaitingOnDriver) {
    fdf_log::debug("Node: {}: driver host channel had expected shutdown.", MakeComponentMoniker());
    GetNodeShutdownCoordinator().CheckNodeState();
    return;
  }

  // Unexpected driver host closure.

  if (GetNodeShutdownCoordinator().IsShuttingDown()) {
    return;
  }

  if (host_restart_on_crash_) {
    fdf_log::warn("Restarting node {} because of unexpected driver channel shutdown.", name());
    RestartNodeOnCrash();
    return;
  }

  bool is_crash = (info.status() != ZX_OK && info.status() != ZX_ERR_CANCELED);
  if (is_crash && is_driver_host_root_ && node_manager_.has_value()) {
    fdf_log::error("Node {} driver host crashed. Rebooting system.", name());
    node_manager_.value()->RebootSystem();
  }

  // If the driver fails to bind to the node, don't remove the node.
  if (IsPendingBind()) {
    fdf_log::debug("Node: {}: driver channel closed during binding.", MakeComponentMoniker());
    return;
  }

  fdf_log::warn("Removing node {} because of unexpected driver channel shutdown.", name());
  Remove(RemovalSet::kAll, nullptr);
}

void Node::OnComponentControllerFidlError(fidl::UnbindInfo info) {
  component_controller_ = {};

  // Expected component controller closure.
  if (GetNodeState() == NodeState::kWaitingOnDestroy) {
    fdf_log::debug("Node: {}: component controller channel had expected shutdown. {}", name(),
                   info.FormatDescription());
    GetNodeShutdownCoordinator().CheckNodeState();
    return;
  }

  // Unexpected component controller closure.

  fdf_log::warn("Node: {}: unexpected component controller channel shutdown. in state {}. {}",
                name(), GetNodeShutdownCoordinator().NodeStateAsString(), info.FormatDescription());
}

std::optional<std::vector<fdf::NodeProperty2>> Node::GetNodeProperties(
    std::string_view parent_name) const {
  if (IsComposite()) {
    const auto& composite = std::get<Composite>(type_);
    for (const auto& entry : composite.properties_) {
      if (entry.name == parent_name) {
        return PropertyToFidl(entry.properties);
      }
    }
    return std::nullopt;
  }
  if (parent_name != "default") {
    return std::nullopt;
  }
  ZX_ASSERT(self_resource_);
  return self_resource_->properties();
}

std::optional<std::shared_ptr<Resource>> Node::GetSelfResource() const { return self_resource_; }

const std::vector<NodeOffer>& Node::offers() const {
  ZX_ASSERT(self_resource_);
  return self_resource_->offers();
}

fdf::NodePropertyDictionary2 Node::GetNodePropertyDict() const {
  fdf::NodePropertyDictionary2 dict;
  if (IsComposite()) {
    const auto& composite = std::get<Composite>(type_);
    dict.reserve(composite.properties_.size());
    std::ranges::transform(composite.properties_, std::back_inserter(dict),
                           [this](const auto& entry) {
                             return fdf::NodePropertyEntry2{{
                                 .name = entry.name,
                                 .properties = std::move(GetNodeProperties(entry.name).value()),
                             }};
                           });
  } else {
    auto props = GetNodeProperties();
    if (props.has_value()) {
      dict.emplace_back(fdf::NodePropertyEntry2{{
          .name = "default",
          .properties = std::move(*props),
      }});
    }
  }
  return dict;
}

Node::DriverComponent::DriverComponent(
    Node& node, std::string url,
    fidl::ServerEnd<fuchsia_component_runner::ComponentController> component_controller,
    fidl::ServerEnd<fuchsia_driver_framework::Node> node_server,
    fidl::ClientEnd<fuchsia_driver_host::Driver> driver, zx::event component_inst)
    : runner_component_controller_ref(
          node.dispatcher_, std::move(component_controller), &node,
          [](Node* node, fidl::UnbindInfo info) {
            if (auto* driver_component = std::get_if<DriverComponent>(&node->state_);
                driver_component) {
              if (node->GetNodeState() == NodeState::kWaitingOnDriverComponent) {
                fdf_log::debug("Node: {}: runner component controller channel had expected close.",
                               node->name());
                driver_component->state = DriverState::kStopped;
                node->GetNodeShutdownCoordinator().CheckNodeState();
              } else {
                fdf_log::warn(
                    "Node: {}: runner component controller channel had unexpected close. Node state: {}",
                    node->name(), node->GetNodeShutdownCoordinator().NodeStateAsString());
                driver_component->state = DriverState::kStopped;
                node->Remove(RemovalSet::kAll, nullptr);
              }
            }
          }),
      node_ref_(node.dispatcher_, std::move(node_server), &node,
                [](Node* node, fidl::UnbindInfo info) { node->OnNodeServerUnbound(info); }),
      driver(std::move(driver), node.dispatcher_, &node.driver_host_handler_),
      driver_url(std::move(url)),
      component_instance(std::move(component_inst)) {
  zx_info_handle_basic_t info;
  zx_status_t status =
      component_instance.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  ZX_ASSERT_MSG(status == ZX_OK, "status %s", zx_status_get_string(status));
  component_instance_koid = info.koid;
}

void Node::ConnectToDeviceFidl(ConnectToDeviceFidlRequestView request,
                               ConnectToDeviceFidlCompleter::Sync& completer) {
  zx_status_t status = ConnectDeviceInterface(std::move(request->server));
  if (status != ZX_OK) {
    fdf_log::error("{}: Failed to connect to device fidl: ", zx_status_get_string(status));
  }
}

void Node::ConnectToController(ConnectToControllerRequestView request,
                               ConnectToControllerCompleter::Sync& completer) {
  // This should never be called
  ZX_ASSERT_MSG(false,
                "Connect To controller should never be called in node.cc,"
                " as it is intercepted by the ControllerAllowlistPassthrough");
}

void Node::Bind(BindRequestView request, BindCompleter::Sync& completer) {
  BindHelper(false, {{request->driver.data(), request->driver.size()}},
             [completer = completer.ToAsync()](zx_status_t status) mutable {
               if (status == ZX_OK) {
                 completer.ReplySuccess();
               } else {
                 completer.ReplyError(status);
               }
             });
}

void Node::Rebind(RebindRequestView request, RebindCompleter::Sync& completer) {
  std::optional<std::string> url;
  if (!request->driver.is_null() && !request->driver.empty()) {
    url = std::string(request->driver.get());
  }

  auto rebind_callback = [completer = completer.ToAsync()](zx::result<> result) mutable {
    if (result.is_ok()) {
      completer.ReplySuccess();
    } else {
      completer.ReplyError(result.error_value());
    }
  };

  if (kEnableCompositeNodeSpecRebind && IsComposite()) {
    node_manager_.value()->RebindComposite(name_, url, std::move(rebind_callback));
    return;
  }

  RestartNodeWithRematch(url, std::move(rebind_callback));
}

void Node::UnbindChildren(UnbindChildrenCompleter::Sync& completer) {
  if (children_.empty()) {
    completer.ReplySuccess();
    return;
  }

  unbinding_children_completers_.emplace_back(completer.ToAsync());
  if (unbinding_children_completers_.size() == 1) {
    // Iterate over a copy of `children_` because `children_` may be modified during
    // `Node::Remove` which would mess up the for loop.
    std::vector<std::shared_ptr<Node>> children{children_.begin(), children_.end()};
    for (const auto& child : children) {
      child->SetShouldDestroy();
      child->Remove(RemovalSet::kAll, nullptr);
    }
  }
}

void Node::ScheduleUnbind(ScheduleUnbindCompleter::Sync& completer) {
  Remove(RemovalSet::kAll, nullptr);
  completer.ReplySuccess();
}
void Node::GetTopologicalPath(GetTopologicalPathCompleter::Sync& completer) {
  completer.ReplySuccess(fidl::StringView::FromExternal("/" + MakeTopologicalPath()));
}

zx_status_t Node::ConnectDeviceInterface(zx::channel channel) {
  if (!devfs_connector_.has_value()) {
    return ZX_ERR_INTERNAL;
  }
  return fidl::WireCall(devfs_connector_.value())->Connect(std::move(channel)).status();
}

Devnode::Target Node::CreateDevfsPassthrough(
    std::optional<fidl::ClientEnd<fuchsia_device_fs::Connector>> connector,
    std::optional<fidl::ClientEnd<fuchsia_device_fs::Connector>> controller_connector,
    bool allow_controller_connection, const std::string& class_name) {
  controller_allowlist_passthrough_ = ControllerAllowlistPassthrough::Create(
      std::move(controller_connector), weak_from_this(), dispatcher_, class_name);
  devfs_connector_ = std::move(connector);
  return Devnode::PassThrough(
      [node = weak_from_this(), node_name = name_](zx::channel server_end) {
        std::shared_ptr locked_node = node.lock();
        if (!locked_node) {
          fdf_log::error("Node was freed before it was used for {}.", node_name);
          return ZX_ERR_BAD_STATE;
        }
        return locked_node->ConnectDeviceInterface(std::move(server_end));
      },
      [node = weak_from_this(), allow_controller_connection,
       node_name = name_](fidl::ServerEnd<fuchsia_device::Controller> server_end) {
        if (!allow_controller_connection) {
          fdf_log::warn(
              "Connection to {} controller interface failed, as that node did not"
              " include controller support in its DevAddArgs",
              node_name);
          return ZX_ERR_PROTOCOL_NOT_SUPPORTED;
        }
        std::shared_ptr locked_node = node.lock();
        if (!locked_node) {
          fdf_log::error("Node was freed before it was used for {}.", node_name);
          return ZX_ERR_BAD_STATE;
        }
        return locked_node->controller_allowlist_passthrough_->Connect(std::move(server_end));
      });
}

}  // namespace driver_manager

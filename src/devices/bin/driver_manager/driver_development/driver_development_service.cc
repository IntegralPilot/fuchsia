// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/driver_development/driver_development_service.h"

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>

#include <algorithm>
#include <queue>
#include <unordered_map>
#include <unordered_set>

#include "src/devices/bin/driver_manager/driver_development/info_iterator.h"
#include "src/devices/bin/driver_manager/node_property_conversion.h"
#include "src/devices/lib/log/log.h"

namespace fdd = fuchsia_driver_development;
namespace fdf {
using namespace fuchsia_driver_framework;
}

namespace driver_manager {

namespace {

void SetNodeInfoBuilderNodeProperties(
    fidl::AnyArena& allocator, const driver_manager::Node& node,
    fidl::WireTableBuilder<fuchsia_driver_development::wire::NodeInfo> node_info_builder) {
  // Composite node's "default" node properties are its primary parent's node properties which
  // should not be used.
  if (node.type() == driver_manager::NodeType::kComposite) {
    return;
  }

  auto properties = node.GetNodeProperties();
  // Avoid allocating an empty VectorView.
  if (!properties.has_value() || properties->empty()) {
    return;
  }

  fidl::VectorView<fdf::wire::NodeProperty> node_properties(allocator, properties->size());
  for (size_t i = 0; i < properties->size(); ++i) {
    node_properties[i] = ToDeprecatedProperty(allocator, properties.value()[i]);
  }
  node_info_builder.node_property_list(node_properties);
}

zx::result<fdd::wire::NodeInfo> CreateDeviceInfo(fidl::AnyArena& allocator,
                                                 const driver_manager::Node* node) {
  auto device_info = fdd::wire::NodeInfo::Builder(allocator);

  device_info.id(reinterpret_cast<uint64_t>(node));

  const auto& children = node->children();
  fidl::VectorView<uint64_t> child_ids(allocator, children.size());
  size_t i = 0;
  for (const auto& child : children) {
    child_ids[i++] = reinterpret_cast<uint64_t>(child.get());
  }
  if (!child_ids.empty()) {
    device_info.child_ids(child_ids);
  }

  const auto& parents = node->parents();
  fidl::VectorView<uint64_t> parent_ids(allocator, parents.size());
  i = 0;
  for (const auto& parent : parents) {
    auto parent_ptr = parent.lock();
    if (!parent_ptr) {
      fdf_log::error("Parent node freed before it could be used");
      return zx::error(ZX_ERR_INTERNAL);
    }
    parent_ids[i++] = reinterpret_cast<uint64_t>(parent_ptr.get());
  }
  if (!parent_ids.empty()) {
    device_info.parent_ids(parent_ids);
  }

  device_info.moniker(fidl::StringView(allocator, node->MakeComponentMoniker()));
  device_info.topological_path(fidl::StringView(allocator, node->MakeTopologicalPath()));

  device_info.quarantined(node->quarantined());
  device_info.bound_driver_url(fidl::StringView(allocator, node->driver_url()));

  SetNodeInfoBuilderNodeProperties(allocator, *node, device_info);

  auto driver_host = node->driver_host();
  if (node->is_bound() && driver_host) {
    auto result = driver_host->GetProcessKoid();
    if (result.is_error()) {
      // ZX_ERR_SHOULD_WAIT means the driver host hasn't been fully connected to yet.
      // ZX_ERR_PEER_CLOSED means the host closed while we tried to query it.
      if (result.error_value() != ZX_ERR_SHOULD_WAIT &&
          result.error_value() != ZX_ERR_PEER_CLOSED) {
        fdf_log::error("Failed to get the process KOID of a driver host: {}",
                       zx_status_get_string(result.status_value()));
        return zx::error(result.status_value());
      }
    } else {
      device_info.driver_host_koid(result.value());
    }
  }

  // Copy over the offers.
  const std::vector<NodeOffer>& offers = node->offers();
  if (!offers.empty()) {
    fidl::VectorView<fuchsia_component_decl::wire::Offer> node_offers(allocator, offers.size());
    for (size_t i = 0; i < offers.size(); i++) {
      const NodeOffer& offer = offers[i];
      switch (offer.transport) {
        case OfferTransport::DriverTransport:
          node_offers[i] = fidl::ToWire(allocator, ToFidl(offer).driver_transport().value());
          break;
        case OfferTransport::ZirconTransport:
          node_offers[i] = fidl::ToWire(allocator, ToFidl(offer).zircon_transport().value());
          break;
        case OfferTransport::Dictionary:
          node_offers[i] = fidl::ToWire(allocator, ToFidl(offer).dictionary_offer().value());
          break;
      }
    }

    device_info.offer_list(node_offers);
  }

  std::vector topo_path = node->GetBusTopology();
  device_info.bus_topology(fidl::ToWire(allocator, topo_path));

  return zx::ok(device_info.Build());
}

}  // namespace

DriverDevelopmentService::DriverDevelopmentService(driver_manager::DriverRunner& driver_runner,
                                                   async_dispatcher_t* dispatcher)
    : driver_runner_(driver_runner), dispatcher_(dispatcher) {}

void DriverDevelopmentService::Publish(component::OutgoingDirectory& outgoing) {
  auto result = outgoing.AddUnmanagedProtocol<fdd::Manager>(
      bindings_.CreateHandler(this, dispatcher_, [](fidl::UnbindInfo info) {
        if (!info.is_peer_closed()) {
          fdf_log::warn("development service closed. {}", info.FormatDescription());
        }
      }));
  ZX_ASSERT(result.is_ok());
}

void DriverDevelopmentService::GetNodeInfo(GetNodeInfoRequestView request,
                                           GetNodeInfoCompleter::Sync& completer) {
  auto arena = std::make_unique<fidl::Arena<512>>();
  std::vector<fdd::wire::NodeInfo> device_infos;

  std::unordered_set<const driver_manager::Node*> unique_nodes;
  std::queue<const driver_manager::Node*> remaining_nodes;
  remaining_nodes.push(driver_runner_.root_node().get());
  while (!remaining_nodes.empty()) {
    auto node = remaining_nodes.front();
    remaining_nodes.pop();
    auto [_, inserted] = unique_nodes.insert(node);
    if (!inserted) {
      // Only insert unique nodes from the DAG.
      continue;
    }
    const auto& children = node->children();
    for (const auto& child : children) {
      remaining_nodes.push(child.get());
    }

    std::string moniker = node->MakeComponentMoniker();
    if (!request->node_filter.empty()) {
      bool found = false;
      for (const auto& node_filter : request->node_filter) {
        if (request->exact_match) {
          if (moniker == node_filter.get()) {
            found = true;
            break;
          }
        } else {
          if (moniker.find(node_filter.get()) != std::string::npos) {
            found = true;
            break;
          }
        }
      }
      if (!found) {
        continue;
      }
    }

    auto result = CreateDeviceInfo(*arena, node);
    if (result.is_error()) {
      return;
    }
    device_infos.push_back(std::move(result.value()));
  }
  auto iterator = std::make_unique<driver_development::DeviceInfoIterator>(std::move(arena),
                                                                           std::move(device_infos));
  fidl::BindServer(
      this->dispatcher_, std::move(request->iterator), std::move(iterator),
      [](auto* self, fidl::UnbindInfo info, fidl::ServerEnd<fdd::NodeInfoIterator> server_end) {
        if (info.is_user_initiated()) {
          return;
        }
        if (info.is_peer_closed()) {
          // For this development protocol, the client is free to disconnect
          // at any time.
          return;
        }
        fdf_log::error("Error serving '{}': {}", fidl::DiscoverableProtocolName<fdd::Manager>,
                       info.FormatDescription());
      });
}

void DriverDevelopmentService::GetCompositeInfo(GetCompositeInfoRequestView request,
                                                GetCompositeInfoCompleter::Sync& completer) {
  auto arena = std::make_unique<fidl::Arena<512>>();
  std::vector<fdd::wire::CompositeNodeInfo> list = driver_runner_.GetCompositeListInfo(*arena);
  auto iterator = std::make_unique<driver_development::CompositeInfoIterator>(std::move(arena),
                                                                              std::move(list));
  fidl::BindServer(this->dispatcher_, std::move(request->iterator), std::move(iterator),
                   [](auto* server, fidl::UnbindInfo info, auto channel) {
                     if (!info.is_peer_closed()) {
                       fdf_log::warn("Closed CompositeInfoIterator: {}", info.lossy_description());
                     }
                   });
}

void DriverDevelopmentService::GetDriverInfo(GetDriverInfoRequestView request,
                                             GetDriverInfoCompleter::Sync& completer) {
  auto driver_index_client = component::Connect<fuchsia_driver_index::DevelopmentManager>();
  if (driver_index_client.is_error()) {
    fdf_log::error("Failed to connect to service '{}': {}",
                   fidl::DiscoverableProtocolName<fuchsia_driver_index::DevelopmentManager>,
                   driver_index_client.status_string());
    request->iterator.Close(driver_index_client.status_value());
    return;
  }

  fidl::WireSyncClient driver_index{std::move(*driver_index_client)};
  auto info_result =
      driver_index->GetDriverInfo(std::move(request->driver_filter), std::move(request->iterator));
  if (!info_result.ok()) {
    fdf_log::error("Failed to call DriverIndex::GetDriverInfo: {}", info_result.error());
  }
}

void DriverDevelopmentService::GetCompositeNodeSpecs(
    GetCompositeNodeSpecsRequestView request, GetCompositeNodeSpecsCompleter::Sync& completer) {
  auto driver_index_client = component::Connect<fuchsia_driver_index::DevelopmentManager>();
  if (driver_index_client.is_error()) {
    fdf_log::error("Failed to connect to service '{}': {}",
                   fidl::DiscoverableProtocolName<fuchsia_driver_index::DevelopmentManager>,
                   driver_index_client.status_string());
    request->iterator.Close(driver_index_client.status_value());
    return;
  }

  fidl::WireSyncClient driver_index{std::move(*driver_index_client)};
  auto info_result =
      driver_index->GetCompositeNodeSpecs(request->name_filter, std::move(request->iterator));
  if (!info_result.ok()) {
    fdf_log::error("Failed to call DriverIndex::GetCompositeNodeSpecs: {}", info_result.error());
  }
}

void DriverDevelopmentService::GetDriverHostInfo(GetDriverHostInfoRequestView request,
                                                 GetDriverHostInfoCompleter::Sync& completer) {
  std::unordered_map<zx_koid_t, std::unordered_set<std::string>> driver_host_to_drivers;
  std::unordered_set<const driver_manager::Node*> unique_nodes;
  std::queue<const driver_manager::Node*> remaining_nodes;
  if (driver_runner_.root_node()) {
    remaining_nodes.push(driver_runner_.root_node().get());
  }
  while (!remaining_nodes.empty()) {
    auto node = remaining_nodes.front();
    remaining_nodes.pop();
    if (!unique_nodes.insert(node).second) {
      continue;
    }
    for (const auto& child : node->children()) {
      remaining_nodes.push(child.get());
    }

    if (node->is_bound()) {
      auto host = node->driver_host();
      if (host) {
        auto koid_result = host->GetProcessKoid();
        if (koid_result.is_ok()) {
          driver_host_to_drivers[koid_result.value()].insert(node->driver_url());
        }
      }
    }
  }

  std::deque<fdd::DriverHostInfo> info;
  for (const auto& driver_host : driver_runner_.driver_hosts()) {
    zx::result process_info = driver_host.GetProcessInfo();
    if (process_info.is_error()) {
      continue;
    }
    std::vector<fdd::ThreadInfo> threads;
    for (const auto& thread : process_info->threads()) {
      threads.emplace_back(fdd::ThreadInfo{{
          .koid = thread.koid(),
          .name = thread.name(),
          .scheduler_role = thread.scheduler_role(),
      }});
    }
    std::vector<fdd::DispatcherInfo> dispatchers;
    for (const auto& dispatcher : process_info->dispatchers()) {
      dispatchers.emplace_back(fdd::DispatcherInfo{{
          .driver = dispatcher.driver(),
          .name = dispatcher.name(),
          .options = dispatcher.options(),
          .scheduler_role = dispatcher.scheduler_role(),
      }});
    }

    std::vector<std::string> drivers;
    if (auto it = driver_host_to_drivers.find(process_info->process_koid());
        it != driver_host_to_drivers.end()) {
      drivers.assign(it->second.begin(), it->second.end());
      std::sort(drivers.begin(), drivers.end());
    }

    info.emplace_back(fdd::DriverHostInfo{{
        .process_koid = process_info->process_koid(),
        .threads = std::move(threads),
        .drivers = std::move(drivers),
        .dispatchers = std::move(dispatchers),
        .name = std::string(driver_host.name_for_colocation()),
    }});
  }

  auto iterator = std::make_unique<driver_development::DriverHostInfoIterator>(std::move(info));
  fidl::BindServer(this->dispatcher_, std::move(request->iterator), std::move(iterator),
                   [](auto* self, fidl::UnbindInfo info,
                      fidl::ServerEnd<fdd::DriverHostInfoIterator> server_end) {
                     if (info.is_user_initiated()) {
                       return;
                     }
                     if (info.is_peer_closed()) {
                       // For this development protocol, the client is free to disconnect
                       // at any time.
                       return;
                     }
                     fdf_log::error("Error serving '{}': {}",
                                    fidl::DiscoverableProtocolName<fdd::Manager>,
                                    info.FormatDescription());
                   });
}

void DriverDevelopmentService::DisableDriver(DisableDriverRequestView request,
                                             DisableDriverCompleter::Sync& completer) {
  auto driver_index_client = component::Connect<fuchsia_driver_index::DevelopmentManager>();
  if (driver_index_client.is_error()) {
    fdf_log::error("Failed to connect to service '{}': {}",
                   fidl::DiscoverableProtocolName<fuchsia_driver_index::DevelopmentManager>,
                   driver_index_client.status_string());
    completer.Close(driver_index_client.status_value());
    return;
  }

  fidl::WireSyncClient driver_index{std::move(*driver_index_client)};
  auto disable_result = driver_index->DisableDriver(request->driver_url, request->package_hash);
  if (!disable_result.ok()) {
    fdf_log::error("Failed to call DriverIndex::DisableDriver: {}", disable_result.error());
    completer.Close(disable_result.error().status());
    return;
  }

  completer.Reply(disable_result.value());
}

void DriverDevelopmentService::EnableDriver(EnableDriverRequestView request,
                                            EnableDriverCompleter::Sync& completer) {
  auto driver_index_client = component::Connect<fuchsia_driver_index::DevelopmentManager>();
  if (driver_index_client.is_error()) {
    fdf_log::error("Failed to connect to service '{}': {}",
                   fidl::DiscoverableProtocolName<fuchsia_driver_index::DevelopmentManager>,
                   driver_index_client.status_string());
    completer.Close(driver_index_client.status_value());
    return;
  }

  fidl::WireSyncClient driver_index{std::move(*driver_index_client)};
  auto enable_result = driver_index->EnableDriver(request->driver_url, request->package_hash);
  if (!enable_result.ok()) {
    fdf_log::error("Failed to call DriverIndex::EnableDriver: {}", enable_result.error());
    completer.Close(enable_result.error().status());
    return;
  }

  completer.Reply(enable_result.value());
}

void DriverDevelopmentService::RestartDriverHosts(RestartDriverHostsRequestView request,
                                                  RestartDriverHostsCompleter::Sync& completer) {
  auto result = driver_runner_.RestartNodesColocatedWithDriverUrl(request->driver_url.get(),
                                                                  request->rematch_flags);
  if (result.is_ok()) {
    completer.ReplySuccess(result.value());
  } else {
    completer.ReplyError(result.error_value());
  }
}

void DriverDevelopmentService::BindAllUnboundNodes(BindAllUnboundNodesCompleter::Sync& completer) {
  auto callback =
      [completer = completer.ToAsync()](
          fidl::VectorView<fuchsia_driver_development::wire::NodeBindingInfo> result) mutable {
        completer.ReplySuccess(result);
      };
  driver_runner_.TryBindAllAvailable(std::move(callback));
}

void DriverDevelopmentService::BindAllUnboundNodes2(
    BindAllUnboundNodes2Completer::Sync& completer) {
  auto callback =
      [completer = completer.ToAsync()](
          fidl::VectorView<fuchsia_driver_development::wire::NodeBindingInfo> result) mutable {
        completer.ReplySuccess(result);
      };
  driver_runner_.TryBindAllAvailable(std::move(callback));
}

void DriverDevelopmentService::AddTestNode(AddTestNodeRequestView request,
                                           AddTestNodeCompleter::Sync& completer) {
  fuchsia_driver_framework::NodeAddArgs args;
  args.name(fidl::ToNatural(request->args.name()));
  args.properties(fidl::ToNatural(request->args.properties()));

  driver_runner_.root_node()->AddChild(
      std::move(args), /* controller */ {}, /* node */ {},
      [this, completer = completer.ToAsync()](fit::result<fuchsia_driver_framework::wire::NodeError,
                                                          std::shared_ptr<driver_manager::Node>>
                                                  result) mutable {
        if (result.is_error()) {
          completer.Reply(result.take_error());
        } else {
          auto node = result.value();
          test_nodes_[node->name()] = node;
          completer.ReplySuccess();
        }
      });
}

void DriverDevelopmentService::RemoveTestNode(RemoveTestNodeRequestView request,
                                              RemoveTestNodeCompleter::Sync& completer) {
  auto name = std::string(request->name.get());
  if (test_nodes_.count(name) == 0) {
    completer.ReplyError(ZX_ERR_NOT_FOUND);
    return;
  }

  auto node = test_nodes_[name].lock();
  if (!node) {
    completer.ReplySuccess();
    test_nodes_.erase(name);
    return;
  }

  node->SetShouldDestroy();
  node->Remove(driver_manager::RemovalSet::kAll, nullptr);
  test_nodes_.erase(name);
  completer.ReplySuccess();
}

void DriverDevelopmentService::WaitForBootup(WaitForBootupCompleter::Sync& completer) {
  driver_runner_.WaitForBootup([completer = completer.ToAsync()]() mutable { completer.Reply(); });
}

void DriverDevelopmentService::RestartWithDictionary(
    RestartWithDictionaryRequestView request, RestartWithDictionaryCompleter::Sync& completer) {
  zx::eventpair endpoint0, endpoint1;
  zx_status_t status = zx::eventpair::create(0, &endpoint0, &endpoint1);
  if (status != ZX_OK) {
    completer.ReplyError(status);
    return;
  }

  driver_runner_.RestartWithDictionary(std::move(request->moniker), std::move(request->dictionary),
                                       std::move(endpoint1));
  completer.ReplySuccess(std::move(endpoint0));
}

void DriverDevelopmentService::RestartWithDictionaryAndPowerDependencies(
    RestartWithDictionaryAndPowerDependenciesRequestView request,
    RestartWithDictionaryAndPowerDependenciesCompleter::Sync& completer) {
  zx::eventpair endpoint0, endpoint1;
  zx_status_t status = zx::eventpair::create(0, &endpoint0, &endpoint1);
  if (status != ZX_OK) {
    completer.ReplyError(status);
    return;
  }

  driver_runner_.RestartWithDictionaryAndPowerDependencies(
      std::string(request->moniker.get()), fidl::ToNatural(std::move(request->dictionary)),
      fidl::ToNatural(request->power_dependencies).value(),
      request->cpu_token_override.is_valid()
          ? std::make_optional(std::move(request->cpu_token_override))
          : std::nullopt,
      fidl::ToNatural(std::move(request->node_power_token_overrides))
          .value_or(std::vector<fuchsia_driver_development::NodePowerTokenOverride>{}),
      std::move(endpoint1));
  completer.ReplySuccess(std::move(endpoint0));
}

void DriverDevelopmentService::RebindCompositesWithDriver(
    RebindCompositesWithDriverRequestView request,
    RebindCompositesWithDriverCompleter::Sync& completer) {
  auto driver_index_client = component::Connect<fuchsia_driver_index::DevelopmentManager>();
  if (driver_index_client.is_error()) {
    fdf_log::error("Failed to connect to service '{}': {}",
                   fidl::DiscoverableProtocolName<fuchsia_driver_index::DevelopmentManager>,
                   driver_index_client.status_string());
    completer.Close(driver_index_client.status_value());
    return;
  }

  fidl::WireSyncClient driver_index{std::move(*driver_index_client)};
  auto index_rebind_result = driver_index->RebindCompositesWithDriver(request->driver_url);
  if (!index_rebind_result.ok()) {
    fdf_log::error("Failed to call DriverIndex::RebindCompositesWithDriver: {}",
                   index_rebind_result.error());
    completer.ReplyError(index_rebind_result.error().status());
    return;
  }
  if (index_rebind_result.value().is_error()) {
    fdf_log::error("DriverIndex::RebindCompositesWithDriver failed: {}",
                   zx_status_get_string(index_rebind_result.value().error_value()));
    completer.ReplyError(index_rebind_result.value().error_value());
    return;
  }

  driver_runner_.RebindCompositesWithDriver(
      std::string(request->driver_url.get()),
      [completer = completer.ToAsync()](size_t count) mutable {
        completer.ReplySuccess(static_cast<uint32_t>(count));
      });
}

void DriverDevelopmentService::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_driver_development::Manager> metadata,
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

  fdf_log::warn("DriverDevelopmentService received unknown {} method. Ordinal: {}", method_type,
                metadata.method_ordinal);
}

}  // namespace driver_manager

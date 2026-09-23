// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/test_realm.h"

#include <fidl/fuchsia.component/cpp/fidl.h>
#include <fidl/fuchsia.io/cpp/fidl.h>
#include <fidl/fuchsia.sys2/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/syslog/cpp/macros.h>
#include <unistd.h>

#include <cstdint>
#include <memory>
#include <optional>
#include <string>

#include "src/developer/debug/shared/status.h"

namespace debug_agent {

namespace {

const std::string kCapabilityRequested = "capability_requested";

// Helper to simplify request pipelining.
template <typename Protocol>
fidl::ServerEnd<Protocol> CreateEndpointsAndBind(fidl::SyncClient<Protocol>& client) {
  auto [client_end, server_end] = fidl::Endpoints<Protocol>::Create();
  client.Bind(std::move(client_end));
  return std::move(server_end);
}

// Helper to convert method-specific errors into |debug::Status|.
template <typename Method>
debug::Status ErrorToStatus(fidl::ErrorsIn<Method> error) {
  if (error.is_framework_error()) {
    return debug::ZxStatus(error.framework_error().status());
  }

  return debug::Status(error.FormatDescription());
}

// Reads a manifest from a |ManifestBytesIterator| producing a vector of bytes.
fit::result<debug::Status, std::vector<uint8_t>> DrainManifestBytesIterator(
    fidl::ClientEnd<fuchsia_sys2::ManifestBytesIterator> iterator_client_end) {
  fidl::SyncClient iterator(std::move(iterator_client_end));
  std::vector<uint8_t> result;

  while (true) {
    auto next_res = iterator->Next();
    if (next_res.is_error()) {
      return fit::error(debug::ZxStatus(next_res.error_value().status(),
                                        next_res.error_value().FormatDescription()));
    }

    if (next_res->infos().empty()) {
      break;
    }

    result.insert(result.end(), next_res->infos().begin(), next_res->infos().end());
  }

  return fit::ok(std::move(result));
}

// Gets the component declaration for a realm specified by |realm_query|.
fit::result<debug::Status, fidl::Box<fuchsia_component_decl::Component>> GetResolvedDeclaration(
    std::string realm_moniker, fidl::SyncClient<fuchsia_sys2::RealmQuery>& realm_query) {
  auto get_manifest_res = realm_query->GetResolvedDeclaration({std::move(realm_moniker)});
  if (get_manifest_res.is_error()) {
    return fit::error(ErrorToStatus(get_manifest_res.error_value()));
  }

  auto drain_res = DrainManifestBytesIterator(std::move(get_manifest_res->iterator()));
  if (drain_res.is_error()) {
    return fit::error(drain_res.error_value());
  }

  auto unpersist_res = fidl::InplaceUnpersist<fuchsia_component_decl::wire::Component>(
      cpp20::span<uint8_t>(drain_res->begin(), drain_res->size()));
  if (unpersist_res.is_error()) {
    return fit::error(debug::ZxStatus(unpersist_res.error_value().status(),
                                      unpersist_res.error_value().FormatDescription()));
  }

  return fit::ok(fidl::ToNatural(*unpersist_res));
}

// Returns the target of an |Offer|.
std::optional<fuchsia_component_decl::Ref> TargetOf(const fuchsia_component_decl::Offer& offer) {
  switch (offer.Which()) {
    case fuchsia_component_decl::Offer::Tag::kService:
      return offer.service()->target();
    case fuchsia_component_decl::Offer::Tag::kProtocol:
      return offer.protocol()->target();
    case fuchsia_component_decl::Offer::Tag::kDirectory:
      return offer.directory()->target();
    case fuchsia_component_decl::Offer::Tag::kStorage:
      return offer.storage()->target();
    case fuchsia_component_decl::Offer::Tag::kRunner:
      return offer.runner()->target();
    case fuchsia_component_decl::Offer::Tag::kResolver:
      return offer.resolver()->target();
    case fuchsia_component_decl::Offer::Tag::kEventStream:
      return offer.event_stream()->target();
    default:
      return std::nullopt;
  }
}

// Gets all offers in the manifest targeting |test_collection|.
fit::result<debug::Status, std::vector<fuchsia_component_decl::Offer>> ValidateAndGetOffers(
    fuchsia_component_decl::Component& manifest, const std::string& test_collection) {
  if (!manifest.collections().has_value() ||
      !std::any_of(
          manifest.collections()->begin(), manifest.collections()->end(),
          [&test_collection](auto& collection) { return collection.name() == test_collection; })) {
    return fit::error(debug::Status("Manifest contains no offers matching test collection"));
  }

  if (!manifest.exposes().has_value() ||
      !std::any_of(manifest.exposes()->begin(), manifest.exposes()->end(), [](auto& expose) {
        return expose.protocol().has_value() &&
               expose.protocol()->target_name() ==
                   fidl::DiscoverableProtocolName<fuchsia_component::Realm>;
      })) {
    return fit::error(debug::Status("Manifest does not offer Realm procotol"));
  }

  bool capability_requested = false;
  std::vector<fuchsia_component_decl::Offer> offers;
  for (auto& offer : *manifest.offers()) {
    // Ignore offers that aren't targeted at |test_collection|.
    auto target = TargetOf(offer);
    if (!target.has_value() || !target->collection().has_value() ||
        target->collection()->name() != test_collection) {
      continue;
    }

    if (offer.event_stream().has_value()) {
      // Look for 'capability_requested' event stream offers whose source is
      // parent and whose scope includes |test_collection|.
      auto& event_stream = offer.event_stream().value();
      if (event_stream.target_name() == kCapabilityRequested && event_stream.source().has_value() &&
          event_stream.source()->parent().has_value() && event_stream.scope().has_value() &&
          std::any_of(event_stream.scope()->begin(), event_stream.scope()->end(),
                      [&test_collection](auto& ref) {
                        return ref.collection().has_value() &&
                               ref.collection()->name() == test_collection;
                      })) {
        // Note 'capability_requested' event.
        capability_requested |= event_stream.target_name() == kCapabilityRequested;
      }
    }

    offers.push_back(std::move(offer));
  }

  if (!capability_requested) {
    return fit::error(debug::Status("Capability not requested"));
  }

  return fit::ok(offers);
}

}  // namespace

fit::result<debug::Status, TestRealmAndOffers> GetTestRealmAndOffers(
    const std::string& realm_arg,
    fidl::SyncClient<fuchsia_sys2::LifecycleController> lifecycle_controller,
    fidl::SyncClient<fuchsia_sys2::RealmQuery> realm_query) {
  size_t separator_position = realm_arg.rfind('/');
  if (separator_position == std::string::npos || separator_position == 0) {
    return fit::error(
        debug::Status("Realm parameter must take the form <realm>/<test collection>"));
  }

  std::string realm_moniker = realm_arg.substr(0, separator_position);
  std::string test_collection = realm_arg.substr(separator_position + 1);

  fidl::Result resolve_instance_res = lifecycle_controller->ResolveInstance({realm_moniker});
  if (resolve_instance_res.is_error()) {
    return fit::error(ErrorToStatus(resolve_instance_res.error_value()));
  }

  auto get_resolved_decl_res = GetResolvedDeclaration(realm_moniker, realm_query);
  if (get_resolved_decl_res.is_error()) {
    return fit::error(get_resolved_decl_res.error_value());
  }
  auto& component_decl = *get_resolved_decl_res;
  auto get_offers_res = ValidateAndGetOffers(*component_decl, test_collection);
  if (get_offers_res.is_error()) {
    return fit::error(get_offers_res.error_value());
  }
  auto& offers = get_offers_res.value();

  fidl::SyncClient<fuchsia_io::Directory> directory;
  fidl::ServerEnd directory_server_end = CreateEndpointsAndBind(directory);
  auto exposed_dir_open_res =
      realm_query->OpenDirectory({std::move(realm_moniker), fuchsia_sys2::OpenDirType::kExposedDir,
                                  std::move(directory_server_end)});
  if (exposed_dir_open_res.is_error()) {
    return fit::error(ErrorToStatus(exposed_dir_open_res.error_value()));
  }

  auto [realm_client_end, realm_server_end] = fidl::Endpoints<fuchsia_component::Realm>::Create();
  auto realm_connect_result =
      directory->Open({fidl::DiscoverableProtocolName<fuchsia_component::Realm>,
                       fuchsia_io::Flags::kProtocolService,
                       {},
                       realm_server_end.TakeChannel()});
  if (realm_connect_result.is_error()) {
    return fit::error(debug::ZxStatus(realm_connect_result.error_value().status()));
  }

  return fit::ok(TestRealmAndOffers{
      .realm = std::move(realm_client_end),
      .offers = std::move(offers),
      .test_collection = std::move(test_collection),
  });
}

fit::result<debug::Status, TestRealmAndOffers> GetTestRealmAndOffers(const std::string& realm_arg) {
  auto connect_res = component::Connect<fuchsia_sys2::LifecycleController>(
      "/svc/fuchsia.sys2.LifecycleController.root");
  if (!connect_res.is_ok())
    return fit::error(debug::ZxStatus(connect_res.error_value()));
  fidl::SyncClient lifecycle_controller(std::move(*connect_res));

  auto realm_query_res =
      component::Connect<fuchsia_sys2::RealmQuery>("/svc/fuchsia.sys2.RealmQuery.root");
  if (realm_query_res.is_error()) {
    return fit::error(debug::ZxStatus(realm_query_res.error_value()));
  }
  fidl::SyncClient realm_query(std::move(*realm_query_res));

  return GetTestRealmAndOffers(realm_arg, std::move(lifecycle_controller), std::move(realm_query));
}

}  // namespace debug_agent

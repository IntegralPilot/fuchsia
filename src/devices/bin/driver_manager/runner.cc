// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/runner.h"

#include <fidl/fuchsia.component/cpp/common_types_format.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/processargs.h>

#include <optional>

#include "src/devices/lib/log/log.h"

namespace {

namespace fprocess = fuchsia_process;
namespace frunner = fuchsia_component_runner;
namespace fcomponent = fuchsia_component;
namespace fdecl = fuchsia_component_decl;

constexpr uint32_t kTokenId = PA_HND(PA_USER0, 0);

zx::result<zx_koid_t> GetKoid(zx_handle_t handle) {
  zx_info_handle_basic_t info{};
  if (zx_status_t status =
          zx_object_get_info(handle, ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
      status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(info.koid);
}

}  // namespace

namespace driver_manager {

zx::result<> Runner::Publish(component::OutgoingDirectory& outgoing) {
  return outgoing.AddUnmanagedProtocol<frunner::ComponentRunner>(
      bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure));
}

void Runner::CreateDriverComponent(const std::shared_ptr<ComponentOwner>& owner,
                                   fidl::ServerEnd<fcomponent::Controller> controller_request,
                                   std::string_view moniker, std::string_view url,
                                   std::string_view collection_name,
                                   const std::vector<NodeOffer>& offers) {
  fidl::Arena arena;
  auto child_decl = fdecl::wire::Child::Builder(arena)
                        .name(fidl::StringView::FromExternal(moniker))
                        .url(fidl::StringView::FromExternal(url))
                        .startup(fdecl::wire::StartupMode::kLazy)
                        .Build();

  auto child_args_builder = fcomponent::wire::CreateChildArgs::Builder(arena);

  if (controller_request.is_valid()) {
    child_args_builder.controller(std::move(controller_request));
  }

  auto offers_dictionary = owner->TakeDictionary();

  auto offers_filtered = offers;
  for (auto it = offers_filtered.begin(); it != offers_filtered.end();) {
    if (it->transport == OfferTransport::Dictionary) {
      it = offers_filtered.erase(it);
    } else {
      ++it;
    }
  }

  size_t offers_count;
  if (!owner->SkipInjectedOffers()) {
    offers_count = offers_filtered.size() + offer_injector_.ExtraOffersCount();
  } else {
    offers_count = offers_filtered.size();
  }
  fidl::VectorView<fdecl::wire::Offer> dynamic_offers(arena, offers_count);
  if (!offers_filtered.empty()) {
    for (size_t i = 0; i < offers_filtered.size(); i++) {
      const NodeOffer& offer = offers_filtered[i];
      switch (offer.transport) {
        case OfferTransport::DriverTransport:
          dynamic_offers[i] = fidl::ToWire(arena, ToFidl(offer).driver_transport().value());
          break;
        case OfferTransport::ZirconTransport:
          dynamic_offers[i] = fidl::ToWire(arena, ToFidl(offer).zircon_transport().value());
          break;
        case OfferTransport::Dictionary:
          __UNREACHABLE;
      }
    }
  }
  if (!owner->SkipInjectedOffers()) {
    offer_injector_.Inject(arena, dynamic_offers, offers_filtered.size());
  }

  child_args_builder.dynamic_offers(dynamic_offers);

  if (offers_dictionary) {
    child_args_builder.dictionary(fidl::ToWire(arena, std::move(offers_dictionary.value())));
  }

  std::string child_moniker(moniker);

  auto create_callback =
      [this,
       child_moniker](fidl::WireUnownedResult<fcomponent::Realm::CreateChild>& result) mutable {
        bool is_error = false;
        if (!result.ok()) {
          fdf_log::error("Failed to create child '{}': {}", child_moniker, result.error());
          is_error = true;
        } else if (result.value().is_error()) {
          fdf_log::error("Failed to create child '{}': {}", child_moniker,
                         result.value().error_value());
          is_error = true;
        }
        if (is_error) {
          zx::result result = CallCallback(child_moniker, zx::error(ZX_ERR_INTERNAL));
          if (result.is_error()) {
            fdf_log::error("Failed to find driver request for '{}': {}", child_moniker, result);
          }

          return;
        }

        StartDriverComponent(child_moniker);
      };
  realm_
      ->CreateChild(
          fdecl::wire::CollectionRef{
              .name = fidl::StringView::FromExternal(collection_name),
          },
          child_decl, child_args_builder.Build())
      .Then(std::move(create_callback));

  moniker_to_owner_[child_moniker] = owner;
}

void Runner::StartDriverComponent(const std::string& moniker) {
  auto it = moniker_to_owner_.find(moniker);
  if (it == moniker_to_owner_.end()) {
    return;
  }

  std::shared_ptr owner = it->second.lock();
  if (!owner) {
    return;
  }

  // When we start a driver, we associate an unforgeable token (the KOID of a
  // zx::event) with the start request, through the use of the numbered_handles
  // field. We do this so:
  //  1. We can securely validate the origin of the request
  //  2. We avoid collisions that can occur when relying on the package URL
  //  3. We avoid relying on the resolved URL matching the package URL
  zx::event token;
  zx_status_t status = zx::event::create(0, &token);
  if (status != ZX_OK) {
    owner->OnComponentStarted(bootup_tracker_, std::string(moniker), zx::error(status));
    return;
  }

  zx::result koid = GetKoid(token.get());
  if (koid.is_error()) {
    owner->OnComponentStarted(bootup_tracker_, std::string(moniker), koid.take_error());
    return;
  }

  start_requests_.emplace(koid.value(), moniker);

  fprocess::wire::HandleInfo handle_info = {
      .handle = std::move(token),
      .id = kTokenId,
  };

  owner->RequestStartComponent(std::move(handle_info), moniker, bootup_tracker_);
}

void Runner::Start(StartRequestView request, StartCompleter::Sync& completer) {
  std::string url = std::string(request->start_info.resolved_url().get());

  // We use the numbered handle, if it exists, to locate the moniker of the node we are starting
  // a driver for. This will be the case for starts that we have issued ourself through
  // |Runner::StartDriverComponent|.
  std::optional<zx_koid_t> token_koid;
  if (request->start_info.has_numbered_handles()) {
    for (auto& h : request->start_info.numbered_handles()) {
      if (h.id == kTokenId && h.handle) {
        zx::result koid = GetKoid(h.handle.get());
        if (koid.is_error()) {
          completer.Close(ZX_ERR_INVALID_ARGS);
          return;
        }
        token_koid.emplace(*koid);
        break;
      }
    }
  }

  if (token_koid.has_value()) {
    auto it = start_requests_.find(*token_koid);
    if (it == start_requests_.end()) {
      completer.Close(ZX_ERR_NOT_FOUND);
      return;
    }

    std::string moniker = it->second;
    start_requests_.erase(it);

    zx::result cb_result =
        CallCallback(moniker, zx::ok(StartedComponent{
                                  .info = fidl::ToNatural(request->start_info),
                                  .component_controller = std::move(request->controller),
                              }));
    if (cb_result.is_error()) {
      fdf_log::error("Failed to start driver '{}', unknown request for driver {}", url, moniker);
      completer.Close(ZX_ERR_UNAVAILABLE);
    }

    return;
  }

  // Otherwise we need to locate it using the component framework's introspection.
  // This will happen if the component framework issues a start on the component manually, which
  // can happen for various reasons, like an ffx component reload being issued.
  zx::event token;
  zx_status_t status =
      request->start_info.component_instance().duplicate(ZX_RIGHT_SAME_RIGHTS, &token);
  if (status != ZX_OK) {
    fdf_log::error("Failed to clone component_instance token.");
    completer.Close(ZX_ERR_INTERNAL);
    return;
  }
  introspector_->GetMoniker(std::move(token))
      .Then([this, completer = completer.ToAsync(),
             start_info = fidl::ToNatural(request->start_info),
             controller = std::move(request->controller),
             url](fidl::WireUnownedResult<fcomponent::Introspector::GetMoniker>& result) mutable {
        if (!result.ok()) {
          fdf_log::error("Failed to GetMoniker. {}", result.FormatDescription());
          completer.Close(ZX_ERR_INTERNAL);
          return;
        }

        if (result.value().is_error()) {
          fdf_log::error("Failed to GetMoniker. {}", result.value().error_value());
          completer.Close(ZX_ERR_INTERNAL);
          return;
        }

        std::string moniker(result.value()->moniker.get());
        size_t split_point = moniker.find(':');
        if (split_point <= 0) {
          fdf_log::error("moniker does not contain collection");
          completer.Close(ZX_ERR_INVALID_ARGS);
          return;
        }

        moniker = moniker.substr(split_point + 1);
        zx::result cb_result =
            CallCallback(moniker, zx::ok(StartedComponent{
                                      .info = std::move(start_info),
                                      .component_controller = std::move(controller),
                                  }));
        if (cb_result.is_error()) {
          fdf_log::error("Failed to start driver '{}', unknown request for driver {}", url,
                         moniker);
          completer.Close(ZX_ERR_UNAVAILABLE);
        }
      });
}

void Runner::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_component_runner::ComponentRunner> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf_log::warn("Unknown ComponentRunner request {}", metadata.method_ordinal);
}

zx::result<> Runner::CallCallback(const std::string& moniker,
                                  zx::result<StartedComponent> component) {
  auto it = moniker_to_owner_.find(moniker);
  if (it == moniker_to_owner_.end()) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  std::shared_ptr owner = it->second.lock();
  if (!owner) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  owner->OnComponentStarted(bootup_tracker_, moniker, std::move(component));
  return zx::ok();
}

}  // namespace driver_manager

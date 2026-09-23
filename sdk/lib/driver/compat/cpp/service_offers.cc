// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/component/incoming/cpp/directory.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/driver/compat/cpp/service_offers.h>
#include <lib/driver/component/cpp/node_add_args.h>

namespace compat {

std::vector<fuchsia_driver_framework::wire::Offer> ServiceOffersV1::CreateOffers2(
    fidl::ArenaBase& arena) {
  std::vector<fuchsia_driver_framework::wire::Offer> offers;
  for (const auto& service_name : zircon_offers_) {
    offers.push_back(fuchsia_driver_framework::wire::Offer::WithZirconTransport(
        arena, fdf::MakeOffer(arena, service_name, name_)));
  }
  for (const auto& service_name : driver_offers_) {
    offers.push_back(fuchsia_driver_framework::wire::Offer::WithDriverTransport(
        arena, fdf::MakeOffer(arena, service_name, name_)));
  }
  return offers;
}

std::vector<fuchsia_driver_framework::Offer> ServiceOffersV1::CreateOffers2() {
  std::vector<fuchsia_driver_framework::Offer> offers;
  for (const auto& service_name : zircon_offers_) {
    offers.push_back(
        fuchsia_driver_framework::Offer::WithZirconTransport(fdf::MakeOffer(service_name, name_)));
  }
  for (const auto& service_name : driver_offers_) {
    offers.push_back(
        fuchsia_driver_framework::Offer::WithDriverTransport(fdf::MakeOffer(service_name, name_)));
  }
  return offers;
}

zx_status_t ServiceOffersV1::Serve(async_dispatcher_t* dispatcher,
                                   component::OutgoingDirectory* outgoing) {
  // Add each service in the device as an service in our outgoing directory.
  // We rename each instance from "default" into the child name, and then rename it back to default
  // via the offer.
  for (const auto& service_name : zircon_offers_) {
    const auto instance_path = std::string("svc/").append(service_name).append("/default");
    auto client = component::OpenDirectoryAt(dir_, instance_path);
    if (client.is_error()) {
      return client.status_value();
    }

    const auto path = std::string("svc/").append(service_name);
    auto result = outgoing->AddDirectoryAt(std::move(*client), path, name_);
    if (result.is_error()) {
      return result.error_value();
    }
    stop_serving_ = [this, outgoing, path]() { (void)outgoing->RemoveDirectoryAt(path, name_); };
  }

  for (const auto& service_name : driver_offers_) {
    const auto instance_path = std::string("svc/").append(service_name).append("/default");
    auto client = component::OpenDirectoryAt(dir_, instance_path);
    if (client.is_error()) {
      return client.status_value();
    }

    const auto path = std::string("svc/").append(service_name);
    auto result = outgoing->AddDirectoryAt(std::move(*client), path, name_);
    if (result.is_error()) {
      return result.error_value();
    }
    stop_serving_ = [this, outgoing, path]() { (void)outgoing->RemoveDirectoryAt(path, name_); };
  }
  return ZX_OK;
}

zx_status_t ServiceOffersV1::Serve(async_dispatcher_t* dispatcher,
                                   fdf::OutgoingDirectory* outgoing) {
  // Add each service in the device as an service in our outgoing directory.
  // We rename each instance from "default" into the child name, and then rename it back to default
  // via the offer.
  for (const auto& service_name : zircon_offers_) {
    const auto instance_path = std::string("svc/").append(service_name).append("/default");
    auto client = component::OpenDirectoryAt(dir_, instance_path);
    if (client.is_error()) {
      return client.status_value();
    }

    const auto path = std::string("svc/").append(service_name);
    auto result = outgoing->AddDirectoryAt(std::move(*client), path, name_);
    if (result.is_error()) {
      return result.error_value();
    }
    stop_serving_ = [this, outgoing, path]() { (void)outgoing->RemoveDirectoryAt(path, name_); };
  }

  for (const auto& service_name : driver_offers_) {
    const auto instance_path = std::string("svc/").append(service_name).append("/default");
    auto client = component::OpenDirectoryAt(dir_, instance_path);
    if (client.is_error()) {
      return client.status_value();
    }

    const auto path = std::string("svc/").append(service_name);
    auto result = outgoing->AddDirectoryAt(std::move(*client), path, name_);
    if (result.is_error()) {
      return result.error_value();
    }
    stop_serving_ = [this, outgoing, path]() { (void)outgoing->RemoveDirectoryAt(path, name_); };
  }
  return ZX_OK;
}

}  // namespace compat

// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_COMPONENT_CPP_NODE_OFFERS_H_
#define LIB_DRIVER_COMPONENT_CPP_NODE_OFFERS_H_

#include <fidl/fuchsia.component.decl/cpp/natural_types.h>
#include <fidl/fuchsia.component.decl/cpp/wire_types.h>
#include <fidl/fuchsia.driver.framework/cpp/natural_types.h>
#include <fidl/fuchsia.driver.framework/cpp/wire_types.h>
#include <lib/component/incoming/cpp/constants.h>
#include <lib/fidl/cpp/wire/arena.h>
#include <lib/fidl/cpp/wire/traits.h>
#include <lib/fidl_driver/cpp/transport.h>

#include <string_view>

namespace fdf {

fuchsia_component_decl::Offer MakeOffer(
    std::string_view service_name, std::string_view instance_name = component::kDefaultInstance);

fuchsia_component_decl::wire::Offer MakeOffer(
    fidl::AnyArena& arena, std::string_view service_name,
    std::string_view instance_name = component::kDefaultInstance);

template <typename Service>
  requires fidl::IsServiceV<Service>
fuchsia_driver_framework::Offer MakeOffer2(
    std::string_view instance_name = component::kDefaultInstance) {
  if constexpr (std::is_same_v<typename Service::Transport, fidl::internal::DriverTransport>) {
    return fuchsia_driver_framework::Offer::WithDriverTransport(
        MakeOffer(Service::Name, instance_name));
  } else if constexpr (std::is_same_v<typename Service::Transport,
                                      fidl::internal::ChannelTransport>) {
    return fuchsia_driver_framework::Offer::WithZirconTransport(
        MakeOffer(Service::Name, instance_name));
  } else {
    static_assert(false, "Service must be using DriverTransport or ChannelTransport.");
  }
}

template <typename Service>
  requires fidl::IsServiceV<Service>
fuchsia_driver_framework::wire::Offer MakeOffer2(
    fidl::AnyArena& arena, std::string_view instance_name = component::kDefaultInstance) {
  if constexpr (std::is_same_v<typename Service::Transport, fidl::internal::DriverTransport>) {
    return fuchsia_driver_framework::wire::Offer::WithDriverTransport(
        arena, MakeOffer(arena, Service::Name, instance_name));
  } else if constexpr (std::is_same_v<typename Service::Transport,
                                      fidl::internal::ChannelTransport>) {
    return fuchsia_driver_framework::wire::Offer::WithZirconTransport(
        arena, MakeOffer(arena, Service::Name, instance_name));
  } else {
    static_assert(false, "Service must be using DriverTransport or ChannelTransport.");
  }
}

}  // namespace fdf

#endif  // LIB_DRIVER_COMPONENT_CPP_NODE_OFFERS_H_

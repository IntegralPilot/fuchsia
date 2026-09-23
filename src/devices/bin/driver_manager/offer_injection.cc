// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/offer_injection.h"

namespace driver_manager {

namespace fdecl = fuchsia_component_decl;

size_t OfferInjector::ExtraOffersCount() const {
  size_t res = 0;
  if (power_config_.power_inject_offer) {
    // 1 for the broker, 3 for the SAG.
    res += 4;
  }

  return res;
}

void OfferInjector::Inject(fidl::AnyArena& arena,
                           fidl::VectorView<fdecl::wire::Offer>& dynamic_offers,
                           size_t start_index) const {
  fdecl::wire::Ref sag_source = fdecl::wire::Ref::WithVoidType(fdecl::wire::VoidRef{});
  fdecl::wire::Availability sag_availability = fdecl::wire::Availability::kOptional;
  fdecl::wire::Ref broker_source = fdecl::wire::Ref::WithVoidType(fdecl::wire::VoidRef{});
  fdecl::wire::Availability broker_availability = fdecl::wire::Availability::kOptional;
  fdecl::wire::Ref cpu_element_source = fdecl::wire::Ref::WithVoidType(fdecl::wire::VoidRef{});
  fdecl::wire::Availability cpu_element_availability = fdecl::wire::Availability::kOptional;
  if (power_config_.power_suspend_enabled) {
    sag_source = fdecl::wire::Ref::WithChild(
        arena, fdecl::wire::ChildRef{
                   .name = fidl::StringView(arena, "system-activity-governor"),
               });
    sag_availability = fdecl::wire::Availability::kRequired;

    broker_source =
        fdecl::wire::Ref::WithChild(arena, fdecl::wire::ChildRef{
                                               .name = fidl::StringView(arena, "power-broker"),
                                           });
    broker_availability = fdecl::wire::Availability::kRequired;

    cpu_element_source = fdecl::wire::Ref::WithChild(
        arena, fdecl::wire::ChildRef{.name = fidl::StringView(arena, "driver_manager")});
    cpu_element_availability = fdecl::wire::Availability::kRequired;
  }

  size_t offset = 0;

  if (power_config_.power_inject_offer) {
    dynamic_offers[start_index + offset++] = fdecl::wire::Offer::WithProtocol(
        arena, fdecl::wire::OfferProtocol::Builder(arena)
                   .source_name(arena, "fuchsia.power.system.ActivityGovernor")
                   .target_name(arena, "fuchsia.power.system.ActivityGovernor")
                   .source(sag_source)
                   .dependency_type(fdecl::wire::DependencyType::kWeak)
                   .availability(sag_availability)
                   .Build());
    dynamic_offers[start_index + offset++] = fdecl::wire::Offer::WithProtocol(
        arena, fdecl::wire::OfferProtocol::Builder(arena)
                   .source_name(arena, "fuchsia.power.system.ExecutionStateManager")
                   .target_name(arena, "fuchsia.power.system.ExecutionStateManager")
                   .source(sag_source)
                   .dependency_type(fdecl::wire::DependencyType::kWeak)
                   .availability(sag_availability)
                   .Build());
    dynamic_offers[start_index + offset++] = fdecl::wire::Offer::WithProtocol(
        arena, fdecl::wire::OfferProtocol::Builder(arena)
                   .source_name(arena, "fuchsia.power.system.CpuElementManager")
                   .target_name(arena, "fuchsia.power.system.CpuElementManager")
                   .source(cpu_element_source)
                   .dependency_type(fdecl::wire::DependencyType::kWeak)
                   .availability(cpu_element_availability)
                   .Build());
    dynamic_offers[start_index + offset++] = fdecl::wire::Offer::WithProtocol(
        arena, fdecl::wire::OfferProtocol::Builder(arena)
                   .source_name(arena, "fuchsia.power.broker.Topology")
                   .target_name(arena, "fuchsia.power.broker.Topology")
                   .source(broker_source)
                   .dependency_type(fdecl::wire::DependencyType::kWeak)
                   .availability(broker_availability)
                   .Build());
  }
}

}  // namespace driver_manager

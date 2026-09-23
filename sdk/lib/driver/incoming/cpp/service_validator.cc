// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.component.decl/cpp/fidl.h>
#include <lib/driver/incoming/cpp/service_validator.h>

namespace fdf {

ServiceValidator::ServiceValidator(const std::vector<fuchsia_driver_framework::Offer>& offers) {
  for (const auto& offer : offers) {
    const fuchsia_component_decl::Offer* inner_offer = nullptr;
    if (offer.Which() == fuchsia_driver_framework::Offer::Tag::kDriverTransport) {
      inner_offer = &offer.driver_transport().value();
    } else if (offer.Which() == fuchsia_driver_framework::Offer::Tag::kZirconTransport) {
      inner_offer = &offer.zircon_transport().value();
    }
#if FUCHSIA_API_LEVEL_AT_LEAST(30)
    else if (offer.Which() == fuchsia_driver_framework::Offer::Tag::kDictionaryOffer) {
      inner_offer = &offer.dictionary_offer().value();
    }
#endif

    if (!inner_offer || inner_offer->Which() != fuchsia_component_decl::Offer::Tag::kService) {
      continue;
    }

    const fuchsia_component_decl::OfferService* service = &inner_offer->service().value();
    if (!service->renamed_instances().has_value()) {
      continue;
    }

    std::optional<std::unordered_set<std::string>> filter_set;
    if (service->source_instance_filter().has_value()) {
      filter_set.emplace(std::unordered_set<std::string>{});

      for (const auto& instance_filter : service->source_instance_filter().value()) {
        filter_set->insert(instance_filter);
      }
    }

    std::string service_name = service->target_name().value();
    for (const auto& instance : service->renamed_instances().value()) {
      const std::string& instance_name = instance.target_name();

      if (filter_set.has_value() && filter_set->find(instance_name) == filter_set->end()) {
        continue;
      }

      std::unordered_map<std::string, std::unordered_set<std::string>>* target_map;
      if (offer.Which() == fuchsia_driver_framework::Offer::Tag::kDriverTransport) {
        target_map = &instance_to_driver_service_mapping_;
      } else if (offer.Which() == fuchsia_driver_framework::Offer::Tag::kZirconTransport) {
        target_map = &instance_to_zircon_service_mapping_;
      }
#if FUCHSIA_API_LEVEL_AT_LEAST(30)
      else if (offer.Which() == fuchsia_driver_framework::Offer::Tag::kDictionaryOffer) {
        target_map = &instance_to_zircon_service_mapping_;
      }
#endif
      else {
        continue;
      }

      if (target_map->find(instance_name) == target_map->end()) {
        (*target_map)[instance_name] = std::unordered_set<std::string>();
      }

      (*target_map)[instance_name].insert(service_name);
    }
  }
}

bool ServiceValidator::IsValidZirconServiceInstance(const std::string& service_name,
                                                    const std::string& instance) const {
  auto entry = instance_to_zircon_service_mapping_.find(instance);
  if (entry != instance_to_zircon_service_mapping_.end()) {
    auto value = entry->second.find(service_name);
    if (value != entry->second.end()) {
      return true;
    }
  }

  return false;
}

bool ServiceValidator::IsValidDriverServiceInstance(const std::string& service_name,
                                                    const std::string& instance) const {
  auto entry = instance_to_driver_service_mapping_.find(instance);
  if (entry != instance_to_driver_service_mapping_.end()) {
    auto value = entry->second.find(service_name);
    if (value != entry->second.end()) {
      return true;
    }
  }

  return false;
}

}  // namespace fdf

// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_component_decl as fdecl;

#[derive(Clone, Copy)]
pub struct PowerOffersConfig {
    pub power_inject_offer: bool,
    pub power_suspend_enabled: bool,
}

#[derive(Clone)]
pub struct OfferInjector {
    power_config: PowerOffersConfig,
}

impl OfferInjector {
    pub fn new(power_config: PowerOffersConfig) -> Self {
        Self { power_config }
    }

    pub fn extra_offers_count(&self) -> usize {
        let mut res = 0;
        if self.power_config.power_inject_offer {
            // 1 for the broker, 3 for the SAG.
            res += 4;
        }
        res
    }

    pub fn inject(&self, dynamic_offers: &mut [fdecl::Offer], start_index: usize) {
        let (sag_source, sag_availability) = if self.power_config.power_suspend_enabled {
            (
                fdecl::Ref::Child(fdecl::ChildRef {
                    name: "system-activity-governor".to_string(),
                    collection: None,
                }),
                fdecl::Availability::Required,
            )
        } else {
            (fdecl::Ref::VoidType(fdecl::VoidRef {}), fdecl::Availability::Optional)
        };

        let (broker_source, broker_availability) = if self.power_config.power_suspend_enabled {
            (
                fdecl::Ref::Child(fdecl::ChildRef {
                    name: "power-broker".to_string(),
                    collection: None,
                }),
                fdecl::Availability::Required,
            )
        } else {
            (fdecl::Ref::VoidType(fdecl::VoidRef {}), fdecl::Availability::Optional)
        };

        let mut offset = 0;

        if self.power_config.power_inject_offer {
            dynamic_offers[start_index + offset] = fdecl::Offer::Protocol(fdecl::OfferProtocol {
                source: Some(sag_source.clone()),
                source_name: Some("fuchsia.power.system.ActivityGovernor".to_string()),
                target_name: Some("fuchsia.power.system.ActivityGovernor".to_string()),
                dependency_type: Some(fdecl::DependencyType::Weak),
                availability: Some(sag_availability),
                ..Default::default()
            });
            offset += 1;

            dynamic_offers[start_index + offset] = fdecl::Offer::Protocol(fdecl::OfferProtocol {
                source: Some(sag_source.clone()),
                source_name: Some("fuchsia.power.system.ExecutionStateManager".to_string()),
                target_name: Some("fuchsia.power.system.ExecutionStateManager".to_string()),
                dependency_type: Some(fdecl::DependencyType::Weak),
                availability: Some(sag_availability),
                ..Default::default()
            });
            offset += 1;

            dynamic_offers[start_index + offset] = fdecl::Offer::Protocol(fdecl::OfferProtocol {
                source: Some(sag_source),
                source_name: Some("fuchsia.power.system.CpuElementManager".to_string()),
                target_name: Some("fuchsia.power.system.CpuElementManager".to_string()),
                dependency_type: Some(fdecl::DependencyType::Weak),
                availability: Some(sag_availability),
                ..Default::default()
            });
            offset += 1;

            dynamic_offers[start_index + offset] = fdecl::Offer::Protocol(fdecl::OfferProtocol {
                source: Some(broker_source),
                source_name: Some("fuchsia.power.broker.Topology".to_string()),
                target_name: Some("fuchsia.power.broker.Topology".to_string()),
                dependency_type: Some(fdecl::DependencyType::Weak),
                availability: Some(broker_availability),
                ..Default::default()
            });
        }
    }
}

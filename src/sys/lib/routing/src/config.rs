// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::RoutingError;
use crate::bedrock::dict_ext::DictExt;
use crate::bedrock::request_metadata;
use crate::component_instance::ComponentInstanceInterface;
use cm_rust::{Availability, FidlIntoNative};
use router_error::Explain;
use runtime_capabilities::Data;
use std::sync::Arc;
use zx_status as zx;

/// Get a specific configuration use declaration from the structured config key value.
pub fn get_use_config_from_key<'a>(
    key: &str,
    decl: &'a cm_rust::ComponentDecl,
) -> Option<&'a cm_rust::UseConfigurationDecl> {
    decl.uses.iter().find_map(|use_| match use_ {
        cm_rust::UseDecl::Config(c) => (c.target_name == key).then_some(&**c),
        _ => None,
    })
}

/// Routes the config value referenced in `use_config` from `component`. Returns the default value
/// if the capability is not available (i.e. routed from void), or if `use_config` has transitional
/// availability and routing fails with an error that maps to `NOT_FOUND`.
pub async fn route_config_value<C>(
    use_config: &cm_rust::UseConfigurationDecl,
    component: &Arc<C>,
) -> Result<Option<cm_rust::ConfigValue>, router_error::RouterError>
where
    C: ComponentInstanceInterface + 'static,
{
    let component_sandbox =
        component.component_sandbox().await.map_err(|e| RoutingError::from(e))?;
    let capability =
        match component_sandbox.program_input.config().get_capability(&use_config.target_name) {
            Some(c) => c,
            None => {
                return Err(RoutingError::BedrockNotPresentInDictionary {
                    name: use_config.target_name.to_string(),
                    moniker: component.moniker().clone().into(),
                }
                .into());
            }
        };
    let runtime_capabilities::Capability::DataRouter(router) = capability else {
        return Err(RoutingError::BedrockWrongCapabilityType {
            actual: format!("{:?}", capability),
            expected: "Router".to_string(),
            moniker: component.moniker().clone().into(),
        }
        .into());
    };
    let request = request_metadata::config_metadata(use_config.availability);
    let data = match router.route(request, component.as_weak().into()).await {
        Ok(Some(d)) => d,
        Ok(None) => return Ok(use_config.default.clone()),
        Err(e)
            if use_config.availability == Availability::Transitional
                && e.as_zx_status() == zx::Status::NOT_FOUND =>
        {
            return Ok(use_config.default.clone());
        }
        Err(e) => return Err(e),
    };
    let Data::Bytes(bytes) = &*data else {
        return Err(RoutingError::BedrockWrongCapabilityType {
            actual: format!("{:?}", data),
            expected: "Data::bytes".to_string(),
            moniker: component.moniker().clone().into(),
        }
        .into());
    };
    let config_value: fidl_fuchsia_component_decl::ConfigValue = match fidl::unpersist(bytes) {
        Ok(v) => v,
        Err(_) => {
            return Err(RoutingError::BedrockWrongCapabilityType {
                actual: "{unknown}".into(),
                expected: "fuchsia.component.decl.ConfigValue".into(),
                moniker: component.moniker().clone().into(),
            }
            .into());
        }
    };

    Ok(Some(config_value.fidl_into_native()))
}

// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use errors::ffx_error;
use fdomain_fuchsia_developer_remotecontrol as rc;
use fdomain_fuchsia_sys2 as fsys;

/// Obtain the root LifecycleController protocol using the RemoteControl protocol.
pub async fn connect_to_lifecycle_controller(
    rcs_proxy: &rc::RemoteControlProxy,
) -> Result<fsys::LifecycleControllerProxy> {
    let lifecycle_controller =
        rcs::root_lifecycle_controller(&rcs_proxy, std::time::Duration::from_secs(15))
            .await
            .map_err(|err| ffx_error!("Could not open LifecycleController: {err}"))?;
    Ok(lifecycle_controller)
}

/// Obtain the root RealmQuery protocol using the RemoteControl protocol.
pub async fn connect_to_realm_query(
    rcs_proxy: &rc::RemoteControlProxy,
) -> Result<fsys::RealmQueryProxy> {
    let realm_query = rcs::root_realm_query(&rcs_proxy, std::time::Duration::from_secs(15))
        .await
        .map_err(|err| ffx_error!("Could not open RealmQuery: {err}"))?;
    Ok(realm_query)
}

/// Obtain the root ConfigOverride protocol using the RemoteControl protocol.
pub async fn connect_to_config_override(
    rcs_proxy: &rc::RemoteControlProxy,
) -> Result<fsys::ConfigOverrideProxy> {
    let config_override = rcs::root_config_override(&rcs_proxy, std::time::Duration::from_secs(15))
        .await
        .map_err(|err| ffx_error!("Could not open ConfigOverride: {err}"))?;
    Ok(config_override)
}

/// Obtain the root RouteValidator protocol using the RemoteControl protocol.
pub async fn connect_to_route_validator(
    rcs_proxy: &rc::RemoteControlProxy,
) -> Result<fsys::RouteValidatorProxy> {
    let route_validator = rcs::root_route_validator(&rcs_proxy, std::time::Duration::from_secs(15))
        .await
        .map_err(|err| ffx_error!("Could not open LifecycleController: {err}"))?;
    Ok(route_validator)
}

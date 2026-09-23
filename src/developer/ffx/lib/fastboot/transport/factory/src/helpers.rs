// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use discovery::{
    DiscoveryBuilder, DiscoverySources, FastbootConnectionState, TargetHandle, TargetState,
};
use ffx_config::EnvironmentContext;
use ffx_fastboot_interface::interface_factory::InterfaceFactoryError;
use std::path::PathBuf;

pub(crate) async fn rediscover_helper<F, U>(
    context: &EnvironmentContext,
    fastboot_file_path: &Option<PathBuf>,
    target_name: &String,
    mut filter: F,
    cb: &mut U,
) -> Result<(), InterfaceFactoryError>
where
    F: FnMut(&TargetHandle) -> bool,
    U: FnMut(FastbootConnectionState) -> Result<(), InterfaceFactoryError>,
{
    let discovery = DiscoveryBuilder::default()
        .set_source(
            DiscoverySources::MDNS | DiscoverySources::MANUAL | DiscoverySources::FASTBOOT_FILE,
        )
        .with_fastboot_devices_file_path(fastboot_file_path.clone())
        .build(context);
    let query = discovery::query::TargetInfoQuery::NodenameOrId(target_name.clone());
    let targets = discovery.discover_devices(query).await.map_err(anyhow::Error::from)?;
    for handle in targets {
        if filter(&handle) {
            // This is the first event that matches our name.
            // Mutate our internal understanding of the address
            // the target is at with the new address discovered
            match handle.state {
                TargetState::Fastboot(ts) => cb(ts.connection_state)?,
                state @ _ => {
                    return Err(InterfaceFactoryError::RediscoverTargetNotInFastboot(
                        target_name.to_string(),
                        state.to_string(),
                    ));
                }
            };
            return Ok(());
        }
    }
    Ok(())
}

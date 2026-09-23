// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::indexer::*;
use crate::resolved_driver::{DriverPackageType, ResolvedDriver};
use anyhow::Context;
use fidl_fuchsia_component_resolution as fresolution;
use futures::stream::{self, StreamExt};
use std::collections::HashSet;
use std::rc::Rc;

fn log_error(err: anyhow::Error) -> anyhow::Error {
    log::error!("{:#?}", err);
    err
}

pub async fn load_boot_drivers(
    boot_drivers: &Vec<String>,
    resolver: &fresolution::ResolverProxy,
    eager_drivers: &HashSet<cm_types::Url>,
    disabled_drivers: &HashSet<cm_types::Url>,
) -> Result<Vec<ResolvedDriver>, anyhow::Error> {
    let resolved_drivers = load_drivers(
        boot_drivers,
        &resolver,
        &eager_drivers,
        &disabled_drivers,
        DriverPackageType::Boot,
    )
    .await
    .context("Error loading boot packages")
    .map_err(log_error)?;
    Ok(resolved_drivers)
}

pub async fn load_base_drivers(
    indexer: Rc<Indexer>,
    base_drivers: &Vec<String>,
    resolver: &fresolution::ResolverProxy,
    eager_drivers: &HashSet<cm_types::Url>,
    disabled_drivers: &HashSet<cm_types::Url>,
) -> Result<(), anyhow::Error> {
    let resolved_drivers = load_drivers(
        &base_drivers,
        &resolver,
        &eager_drivers,
        &disabled_drivers,
        DriverPackageType::Base,
    )
    .await
    .context("Error loading base packages")
    .map_err(log_error)?;
    for resolved_driver in &resolved_drivers {
        let mut composite_node_spec_manager = indexer.composite_node_spec_manager.borrow_mut();
        composite_node_spec_manager.new_driver_available(resolved_driver);
    }
    indexer.load_base_repo(resolved_drivers);
    Ok(())
}

pub async fn load_drivers(
    drivers: &Vec<String>,
    resolver: &fresolution::ResolverProxy,
    eager_drivers: &HashSet<cm_types::Url>,
    disabled_drivers: &HashSet<cm_types::Url>,
    package_type: DriverPackageType,
) -> Result<Vec<ResolvedDriver>, anyhow::Error> {
    Ok(stream::iter(drivers)
        .map(|driver_url| async move {
            let url = match cm_types::Url::new(driver_url) {
                Ok(u) => u,
                Err(e) => {
                    log::error!("Found bad driver url: {}: error: {}", driver_url, e);
                    return None;
                }
            };
            let resolve = ResolvedDriver::resolve(url, &resolver, package_type).await;
            if resolve.is_err() {
                return None;
            }

            let mut resolved_driver = resolve.unwrap();
            if disabled_drivers.contains(&resolved_driver.component_url) {
                log::info!("Skipping driver: {}", resolved_driver.component_url);
                return None;
            }
            log::info!("Found driver: {}", resolved_driver.component_url);
            if eager_drivers.contains(&resolved_driver.component_url) {
                resolved_driver.fallback = false;
            }
            Some(resolved_driver)
        })
        .buffer_unordered(30)
        .filter_map(|d| async move { d })
        .collect()
        .await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolved_driver::load_driver;
    use {fidl_fuchsia_component_decl as fdecl, fidl_fuchsia_io as fio};

    #[fuchsia::test]
    async fn test_load_fallback_driver() {
        const DRIVER_URL: &str = "fuchsia-boot:///#meta/test-fallback-component.cm";
        let driver_url = cm_types::Url::new(DRIVER_URL).unwrap();
        let pkg = fuchsia_fs::directory::open_in_namespace("/pkg", fio::PERM_READABLE).unwrap();
        let manifest = fuchsia_fs::directory::open_file(
            &pkg,
            "meta/test-fallback-component.cm",
            fio::PERM_READABLE,
        )
        .await
        .unwrap();
        let decl = fuchsia_fs::file::read_fidl::<fdecl::Component>(&manifest).await.unwrap();
        let fallback_driver = load_driver(
            driver_url,
            decl,
            fuchsia_pkg::PackageDirectory::from_proxy(pkg),
            DriverPackageType::Boot,
            None,
        )
        .await
        .expect("Fallback driver was not loaded");
        assert!(fallback_driver.fallback);
    }
}

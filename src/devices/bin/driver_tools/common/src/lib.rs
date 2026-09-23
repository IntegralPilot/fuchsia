// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, anyhow, format_err};
use flex_client::ProxyHasDomain;
use {flex_fuchsia_driver_development as fdd, flex_fuchsia_driver_framework as fdf};

#[derive(Debug)]
pub struct Device(pub fdd::NodeInfo);

impl Device {
    /// Gets the full moniker name of the device.
    pub fn get_moniker(&self) -> Result<&str> {
        let moniker = self.0.moniker.as_deref();
        Ok(moniker.ok_or_else(|| format_err!("Missing moniker"))?)
    }

    /// Gets the full identifying path name of the device.
    pub fn get_full_name(&self) -> Result<&str> {
        self.get_moniker()
    }

    /// Gets the last ordinal of the device's moniker.
    ///
    /// For a `moniker` value of "this.is.a.moniker.foo.bar", "bar" will be returned.
    pub fn extract_name(&self) -> Result<&str> {
        let moniker = self.get_moniker()?;
        let (_, name) = moniker.rsplit_once('.').unwrap_or(("", &moniker));
        Ok(name)
    }
}

impl std::convert::From<fdd::NodeInfo> for Device {
    fn from(device_info: fdd::NodeInfo) -> Device {
        Device(device_info)
    }
}

/// Combines pagination results into a single vector.
pub async fn get_device_info(
    service: &fdd::ManagerProxy,
    device_filter: &[String],
    exact_match: bool,
) -> Result<Vec<fdd::NodeInfo>> {
    let (iterator, iterator_server) =
        service.domain().create_proxy::<fdd::NodeInfoIteratorMarker>();

    service
        .get_node_info(device_filter, iterator_server, exact_match)
        .context("FIDL call to get device info failed")?;

    let mut info_result = Vec::new();

    'outer: loop {
        // To minimize round trips we request several results in one go.
        let device_info_futures = vec![
            iterator.get_next(),
            iterator.get_next(),
            iterator.get_next(),
            iterator.get_next(),
            iterator.get_next(),
            iterator.get_next(),
            iterator.get_next(),
            iterator.get_next(),
            iterator.get_next(),
            iterator.get_next(),
            iterator.get_next(),
            iterator.get_next(),
        ];
        let device_info_result = futures::future::join_all(device_info_futures).await;
        for result in device_info_result {
            let mut device_info: Vec<fdd::NodeInfo> =
                result.context("FIDL call to get device info failed")?;
            if device_info.is_empty() {
                break 'outer;
            }
            info_result.append(&mut device_info)
        }
    }
    Ok(info_result)
}

/// Combines pagination results into a single vector.
pub async fn get_driver_info(
    service: &fdd::ManagerProxy,
    driver_filter: &[String],
) -> Result<Vec<fdf::DriverInfo>> {
    let (iterator, iterator_server) =
        service.domain().create_proxy::<fdd::DriverInfoIteratorMarker>();

    service
        .get_driver_info(driver_filter, iterator_server)
        .context("FIDL call to get driver info failed")?;

    let mut info_result = Vec::new();
    'outer: loop {
        // To minimize round trips we request several results in one go.
        let driver_info_futures = vec![
            iterator.get_next(),
            iterator.get_next(),
            iterator.get_next(),
            iterator.get_next(),
            iterator.get_next(),
            iterator.get_next(),
            iterator.get_next(),
            iterator.get_next(),
        ];
        let driver_info_result = futures::future::join_all(driver_info_futures).await;
        for result in driver_info_result {
            let mut driver_info: Vec<fdf::DriverInfo> =
                result.context("FIDL call to get driver info failed")?;
            if driver_info.is_empty() {
                break 'outer;
            }
            info_result.append(&mut driver_info)
        }
    }
    Ok(info_result)
}

pub async fn get_drivers_from_query(
    query: &str,
    driver_development_proxy: &fdd::ManagerProxy,
) -> Result<Vec<fdf::DriverInfo>> {
    // Try to get exactly matching driver first by treating the query as a filter.
    // If that fails or returns multiple, we'll do manual filtering.
    let driver_filter = [query.to_string()];
    let driver_info = get_driver_info(driver_development_proxy, &driver_filter).await;

    match driver_info {
        Ok(drivers) if !drivers.is_empty() => Ok(drivers),
        _ => {
            // If direct filter didn't work, get all drivers and filter manually.
            let empty: [String; 0] = [];
            let all_drivers = get_driver_info(driver_development_proxy, &empty).await?;
            Ok(all_drivers
                .into_iter()
                .filter(|driver| {
                    let url_match = driver.url.as_ref().map(|u| u.contains(query)).unwrap_or(false);
                    if url_match {
                        return true;
                    }
                    driver.name.as_ref().map(|n| n.contains(query)).unwrap_or(false)
                })
                .collect())
        }
    }
}

#[derive(Debug)]
pub enum GetSingleDriverError {
    NotFound(String),
    Ambiguous { query: String, drivers: String },
    UnderlyingError(anyhow::Error),
}

impl std::fmt::Display for GetSingleDriverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GetSingleDriverError::NotFound(query) => {
                write!(f, "No matching driver found for query {:?}.", query)
            }
            GetSingleDriverError::Ambiguous { query, drivers } => {
                write!(
                    f,
                    "The query {:?} matches more than one driver:\n{}\n\nTo avoid ambiguity, use a more specific query.",
                    query, drivers
                )
            }
            GetSingleDriverError::UnderlyingError(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for GetSingleDriverError {}

impl From<anyhow::Error> for GetSingleDriverError {
    fn from(e: anyhow::Error) -> Self {
        GetSingleDriverError::UnderlyingError(e)
    }
}

pub async fn get_single_driver_from_query(
    query: &str,
    driver_development_proxy: &fdd::ManagerProxy,
) -> Result<fdf::DriverInfo, GetSingleDriverError> {
    let mut filtered_drivers = get_drivers_from_query(query, driver_development_proxy)
        .await
        .map_err(anyhow::Error::from)?;

    if filtered_drivers.len() > 1 {
        let driver_names: Vec<String> = filtered_drivers
            .iter()
            .map(|d| {
                d.url.as_ref().cloned().unwrap_or_else(|| {
                    d.name.as_ref().cloned().unwrap_or_else(|| "Unknown".to_string())
                })
            })
            .collect();
        let driver_names = driver_names.join("\n");
        return Err(GetSingleDriverError::Ambiguous {
            query: query.to_string(),
            drivers: driver_names,
        });
    }

    if filtered_drivers.is_empty() {
        return Err(GetSingleDriverError::NotFound(query.to_string()));
    }

    Ok(filtered_drivers.remove(0))
}

/// Combines pagination results into a single vector.
pub async fn get_composite_info(
    service: &fdd::ManagerProxy,
) -> Result<Vec<fdd::CompositeNodeInfo>> {
    let (iterator, iterator_server) =
        service.domain().create_proxy::<fdd::CompositeInfoIteratorMarker>();

    service
        .get_composite_info(iterator_server)
        .context("FIDL call to get composite info failed")?;

    let mut info_result = Vec::new();
    loop {
        let mut info =
            iterator.get_next().await.context("FIDL call to get composite info failed")?;
        if info.is_empty() {
            break;
        }
        info_result.append(&mut info)
    }
    Ok(info_result)
}

/// Combines pagination results into a single vector.
pub async fn get_driver_host_info(service: &fdd::ManagerProxy) -> Result<Vec<fdd::DriverHostInfo>> {
    let (iterator, iterator_server) =
        service.domain().create_proxy::<fdd::DriverHostInfoIteratorMarker>();

    service
        .get_driver_host_info(iterator_server)
        .context("FIDL call to get driver host info failed")?;

    let mut info_result = Vec::new();
    loop {
        let mut info: Vec<fdd::DriverHostInfo> =
            iterator.get_next().await.context("FIDL call to get driver host info failed")?;
        if info.is_empty() {
            break;
        }
        info_result.append(&mut info)
    }
    Ok(info_result)
}

/// Combines pagination results into a single vector.
pub async fn get_composite_node_specs(
    service: &fdd::ManagerProxy,
    name_filter: Option<String>,
) -> Result<Vec<fdf::CompositeInfo>> {
    let (iterator, iterator_server) =
        service.domain().create_proxy::<fdd::CompositeNodeSpecIteratorMarker>();

    service
        .get_composite_node_specs(name_filter.as_deref(), iterator_server)
        .context("FIDL call to get node groups failed")?;

    let mut info_result = Vec::new();
    loop {
        let mut node_groups: Vec<fdf::CompositeInfo> =
            iterator.get_next().await.context("FIDL call to get node groups failed")?;
        if node_groups.is_empty() {
            break;
        }
        info_result.append(&mut node_groups)
    }
    Ok(info_result)
}

/// Gets the desired DriverInfo instance.
///
/// Filter based on the driver's URL.
/// For example: "fuchsia-boot://domain/#meta/foo.cm"
///
/// # Arguments
/// * `driver_filter` - Filter to the driver that matches the given filter.
pub async fn get_driver_by_filter(
    driver_filter: &str,
    driver_development_proxy: &fdd::ManagerProxy,
) -> Result<fdf::DriverInfo> {
    let filter_list: [String; 1] = [driver_filter.to_string()];
    let driver_list = get_driver_info(driver_development_proxy, &filter_list).await?;
    if driver_list.len() != 1 {
        return Err(anyhow!(
            "There should be exactly one match for '{}'. Found {}.",
            driver_filter,
            driver_list.len()
        ));
    }
    let mut driver_info: Option<fdf::DriverInfo> = None;

    // Confirm this is the correct match.
    let driver = &driver_list[0];
    if let Some(ref url) = driver.url {
        if url == driver_filter {
            driver_info = Some(driver.clone());
        }
    }
    match driver_info {
        Some(driver) => Ok(driver),
        _ => Err(anyhow!("Did not find matching driver for: {}", driver_filter)),
    }
}

/// Gets the driver that is bound to the given device.
///
/// Is able to fuzzy match on the device's topological path, where the shortest match
/// will be the one chosen.
///
/// # Arguments
/// * `device_topo_path` - The device's topological path. e.g. sys/platform/.../device
pub async fn get_driver_by_device(
    device_topo_path: &str,
    driver_development_proxy: &fdd::ManagerProxy,
) -> Result<fdf::DriverInfo> {
    let device_filter: [String; 1] = [device_topo_path.to_string()];
    let mut device_list =
        get_device_info(driver_development_proxy, &device_filter, /* exact_match= */ true).await?;
    if device_list.len() != 1 {
        let fuzzy_device_list = get_device_info(
            driver_development_proxy,
            &device_filter,
            /* exact_match= */ false,
        )
        .await?;
        if fuzzy_device_list.is_empty() {
            return Err(anyhow!("No devices matched the query: {}", device_topo_path));
        } else if fuzzy_device_list.len() > 1 {
            let mut builder = "Found multiple matches. Did you mean one of these?\n\n".to_string();
            for item in fuzzy_device_list {
                let device: Device = item.into();
                // We don't appear to have a string builder crate in-tree.
                builder = format!("{}{}\n", builder, device.get_full_name()?);
            }
            return Err(anyhow!(builder));
        }
        device_list = fuzzy_device_list;
    }

    let found_device = device_list.remove(0);
    match found_device.bound_driver_url {
        Some(ref driver_filter) => {
            get_driver_by_filter(driver_filter, driver_development_proxy).await
        }
        _ => Err(anyhow!("Did not find driver for device {}", device_topo_path)),
    }
}

/// Gets the devices that are bound to the given driver.
///
/// Filter based on the driver's URL.
/// For example: "fuchsia-boot://domain/#meta/foo.cm"
///
/// # Arguments
/// * `driver_filter` - Filter to the driver that matches the given filter.
pub async fn get_devices_by_driver(
    driver_filter: &str,
    driver_development_proxy: &fdd::ManagerProxy,
) -> Result<Vec<Device>> {
    let driver_info_fut = get_driver_by_filter(driver_filter, driver_development_proxy);
    let empty: [String; 0] = [];
    let device_list_fut =
        get_device_info(driver_development_proxy, &empty, /* exact_match= */ false);

    let (driver_info, device_list) = futures::join!(driver_info_fut, device_list_fut);
    let (driver_info, device_list) = (driver_info?, device_list?);

    let mut matches: Vec<Device> = Vec::new();
    for device_item in device_list {
        let device: Device = device_item.into();
        if let (Some(bound_driver_url), Some(url)) = (&device.0.bound_driver_url, &driver_info.url)
        {
            if url == bound_driver_url {
                matches.push(device);
            }
        }
    }
    Ok(matches)
}

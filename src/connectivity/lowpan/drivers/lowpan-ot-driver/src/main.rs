// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! LoWPAN OpenThread Driver
#![warn(rust_2018_idioms)]

use anyhow::Error;
use fidl_fuchsia_buildinfo::{BuildInfo, ProviderMarker};
use fidl_fuchsia_factory_lowpan::{FactoryRegisterMarker, FactoryRegisterProxyInterface};
use fidl_fuchsia_lowpan_driver::{RegisterMarker, RegisterProxyInterface};
use fidl_fuchsia_lowpan_spinel::{
    self as fspinel, DeviceMarker as SpinelDeviceMarker, DeviceProxy as SpinelDeviceProxy,
    DeviceSetupMarker as SpinelDeviceSetupMarker,
};
use fuchsia_component::client::{Service, connect_to_protocol, connect_to_protocol_at};

use lowpan_driver_common::net::*;
use lowpan_driver_common::spinel::SpinelDeviceSink;
use lowpan_driver_common::{register_and_serve_driver, register_and_serve_driver_factory};
use openthread_fuchsia::Platform as OtPlatform;

use config::Config;
use fidl::endpoints::create_proxy;

use crate::driver::{OtDriver, ProductMetadata, get_product_metadata};
use crate::prelude::*;
use fuchsia as _;
use futures::channel::mpsc;
use openthread::ot::BorderAgent;
use std::ffi::CString;
use std::num::NonZeroU32;

mod bootstrap;
mod config;
mod convert_ext;
mod driver;

#[macro_use]
mod prelude {
    #![allow(unused_imports)]

    pub use crate::Result;
    pub use crate::convert_ext::{FromExt as _, IntoExt as _};
    pub use anyhow::{Context as _, bail, format_err};
    pub use fasync::TimeoutExt as _;
    pub use fidl::endpoints::Proxy as _;
    pub use fidl_fuchsia_net_ext as fnet_ext;
    pub use fuchsia_async as fasync;
    pub use futures::future::BoxFuture;
    pub use futures::stream::BoxStream;
    pub use log::{debug, error, info, trace, warn};
    pub use lowpan_driver_common::ZxResult;
    pub use lowpan_driver_common::pii::MarkPii;
    pub use net_declare::{fidl_ip, fidl_ip_v6};
    pub use std::convert::TryInto;
    pub use std::fmt::Debug;
    pub use zx as fz;
    pub use zx_status::Status as ZxStatus;

    pub use futures::prelude::*;
    pub use openthread::prelude::*;
}

pub type Result<T = (), E = anyhow::Error> = std::result::Result<T, E>;

const MAX_EXPONENTIAL_BACKOFF_DELAY_SEC: i64 = 180;
const RESET_EXPONENTIAL_BACKOFF_TIMER_MIN: i64 = 5;
const SERVICE_CHANNEL_SIZE: usize = 100;
const MAX_VENDOR_SW_VERSION_TLV_LENGTH: usize = 16;

impl Config {
    async fn open_spinel_device_proxy(&self) -> Result<SpinelDeviceProxy, Error> {
        let spinel_device_setup_proxy = if let Some(path) = self.ot_radio_path.as_deref() {
            info!("Attempting to use Spinel RCP at {}", path);
            fuchsia_component::client::connect_to_protocol_at_path::<SpinelDeviceSetupMarker>(path)
                .context("Error opening Spinel RCP at path")?
        } else {
            let service = Service::open(fspinel::ServiceMarker).context("open service")?;
            let mut watcher = service.watch().await.context("create watcher")?;
            // We take the first discovered instance. In typical configurations, there is
            // only one spinel service instance. If multiple exist, the first one is used.
            let instance = watcher
                .try_next()
                .await
                .context("watch service")?
                .ok_or_else(|| format_err!("no spinel service instances"))?;
            info!("Attempting to use Spinel RCP instance {}", instance.instance_name());
            instance.connect_to_device_setup().context("connect to device setup")?
        };

        let (client_side, server_side) = fidl::endpoints::create_endpoints::<SpinelDeviceMarker>();

        spinel_device_setup_proxy
            .set_channel(server_side)
            .await?
            .map_err(ZxStatus::err_from_raw)
            .context(
            "Unable to set server-side FIDL channel via spinel_device_setup_proxy.set_channel()",
        )?;

        Ok(client_side.into_proxy())
    }

    fn get_backbone_netif_index_by_config(&self) -> Option<ot::NetifIndex> {
        if self.backbone_name.as_ref().unwrap().is_empty() {
            info!("Backbone interface is disabled");
            return None;
        }

        let c_name = CString::new(self.backbone_name.as_ref().unwrap().as_bytes().to_vec())
            .expect("Invalid backbone interface name");

        // SAFETY: Calling `if_name_toindex` is safe assuming that the C-string pointer
        //         being passed into it is valid, which is guaranteed by `CString::as_ptr()`.
        let index = unsafe { libc::if_nametoindex(c_name.as_ptr()) };

        if index == 0 {
            error!("Unable to look up index of interface {:?}", self.backbone_name);
            return None;
        }

        info!("Backbone interface is {:?} (index {})", self.backbone_name, index);

        Some(index)
    }

    fn get_backbone_netif_index_by_wlan_availability(&self) -> Option<ot::NetifIndex> {
        let state = connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
            .expect("error connecting to StateMarker");
        let (watcher_client, watcher_server) =
            create_proxy::<fidl_fuchsia_net_interfaces::WatcherMarker>();
        state
            .get_watcher(&fidl_fuchsia_net_interfaces::WatcherOptions::default(), watcher_server)
            .expect("error getting interface watcher");

        let get_nicid_fut = async move {
            loop {
                match watcher_client.watch().await.expect("") {
                    fidl_fuchsia_net_interfaces::Event::Existing(
                        fidl_fuchsia_net_interfaces::Properties {
                            id,
                            name,
                            online,
                            port_class,
                            ..
                        },
                    ) => {
                        info!(
                            "NICID: {:?}, name: {:?}, online: {:?}, port_class: {:?}",
                            id, name, online, port_class
                        );
                        if let (
                            Some(fidl_fuchsia_net_interfaces::PortClass::Device(
                                fidl_fuchsia_hardware_network::PortClass::WlanClient,
                            )),
                            Some(true),
                        ) = (port_class, online)
                        {
                            return Some(id.unwrap_or(0) as ot::NetifIndex);
                        }
                    }
                    fidl_fuchsia_net_interfaces::Event::Idle(
                        fidl_fuchsia_net_interfaces::Empty {},
                    ) => {
                        break;
                    }
                    _ => {}
                }
            }
            None
        };

        futures::executor::block_on(get_nicid_fut)
    }

    fn get_backbone_netif_index(&self) -> Option<ot::NetifIndex> {
        if self.backbone_name.is_none() {
            self.get_backbone_netif_index_by_wlan_availability()
        } else {
            self.get_backbone_netif_index_by_config()
        }
    }

    async fn get_build_info(&self) -> Result<BuildInfo, Error> {
        let provider = connect_to_protocol::<ProviderMarker>()?;
        let build_info = provider.get_build_info().await?;
        Ok(build_info)
    }

    fn compress_build_version(&self, original_version: &str) -> String {
        // If it's already 16 bytes or less, return as-is.
        if original_version.len() <= MAX_VENDOR_SW_VERSION_TLV_LENGTH {
            return original_version.to_string();
        }

        if let Some(idx1) = original_version.find('.') {
            // Find the second dot first, so we know exactly how long the date field is.
            if let Some(offset) = original_version[idx1 + 1..].find('.') {
                let idx2 = idx1 + 1 + offset;

                // Try to remove the century only if the date field is long enough (>= 4 chars).
                let date_field_len = idx2 - (idx1 + 1);
                if date_field_len >= 4 {
                    let mut temp_version = String::with_capacity(original_version.len() - 2);
                    temp_version.push_str(&original_version[..idx1 + 1]); // e.g., "31."
                    temp_version.push_str(&original_version[idx1 + 3..]); // e.g., "260312.103.1"

                    if temp_version.len() <= MAX_VENDOR_SW_VERSION_TLV_LENGTH {
                        return temp_version;
                    }
                }

                // If still too long, try to remove the date entirely.
                let mut temp_version = String::with_capacity(original_version.len());
                temp_version.push_str(&original_version[..idx1]); // e.g., "31"
                temp_version.push_str(&original_version[idx2..]); // e.g., ".103.1"

                if temp_version.len() <= MAX_VENDOR_SW_VERSION_TLV_LENGTH {
                    return temp_version;
                }

                // If still too long, try to keep the release version separated from the
                // remaining date-less string, i.e. keep one dot for the release version for
                // the better readability.
                let formatted_version = if let Some(first_dot_idx) = temp_version.find('.') {
                    let (head, tail) = temp_version.split_at(first_dot_idx + 1);
                    format!("{}{}", head, tail.replace('.', ""))
                } else {
                    temp_version.clone() // if no dot found
                };
                if formatted_version.len() <= MAX_VENDOR_SW_VERSION_TLV_LENGTH {
                    return formatted_version;
                }

                // If still too long, explicitly truncate to 16 characters safely.
                return formatted_version.chars().take(MAX_VENDOR_SW_VERSION_TLV_LENGTH).collect();
            }
        }

        // Fallback truncation if the version string is invalid.
        original_version.chars().take(MAX_VENDOR_SW_VERSION_TLV_LENGTH).collect()
    }

    /// Async method which returns the future that runs the driver.
    async fn prepare_to_run(&self) -> Result<impl Future<Output = Result<(), Error>>, Error> {
        let spinel_device_proxy = self.open_spinel_device_proxy().await?;
        debug!("Spinel device proxy initialized");

        let spinel_sink = SpinelDeviceSink::new(spinel_device_proxy);
        let spinel_stream = spinel_sink.take_stream();

        let netif = TunNetworkInterface::try_new(Some(self.name.clone()))
            .await
            .context("Unable to start TUN driver")?;

        let mut builder = OtPlatform::build().thread_netif_index(
            netif
                .get_index()
                .try_into()
                .expect("Network interface index is too large for OpenThread"),
        );

        netif
            .set_ipv6_forwarding_enabled(true)
            .await
            .expect("Unable to enable ipv6 packet forwarding on lowpan interface");

        netif
            .set_ipv4_forwarding_enabled(true)
            .await
            .expect("Unable to enable ipv4 packet forwarding on lowpan interface");

        let backbone_netif_index = self.get_backbone_netif_index();
        let backbone_if = BackboneNetworkInterface::new(backbone_netif_index.unwrap_or(0).into());

        if let Some(index) = backbone_netif_index {
            builder = builder.backbone_netif_index(index);
        }

        let ot_instance = ot::Instance::new(builder.init(spinel_sink, spinel_stream));

        // TODO: might switch to check if the infra_if instance is constructed successfully
        if let Some(index) = backbone_netif_index.and_then(NonZeroU32::new) {
            ot_instance
                .border_routing_init(index.get(), true)
                .context("Unable to initialize OpenThread border routing")?;

            ot_instance.border_routing_set_enabled(true).context("border_routing_set_enabled")?;

            ot_instance.set_backbone_router_enabled(true);
        } else {
            warn!("Backbone interface not set, border routing not supported");
        }

        // The OpenThread stack enables ePSKc mode by default, which can cause feature
        // control conflicts. To prevent this, the LowPAN driver forces ePSKc mode off
        // during initialization. The feature will only be re-enabled upon explicit request
        // from the user via feature config.
        ot_instance.border_agent_ephemeral_key_set_enabled(false /* enable */);

        let product_metadata =
            get_product_metadata(connect_to_protocol::<fidl_fuchsia_hwinfo::ProductMarker>()?)
                .await;
        if let Err(e) = ot_instance.set_vendor_name(&product_metadata.vendor()) {
            warn!("Failed to set the vendor name {:?}", e);
        }
        if let Err(e) = ot_instance.set_vendor_model(&product_metadata.product()) {
            warn!("Failed to set the vendor model {:?}", e);
        }

        let build_info = self.get_build_info().await;
        let version = match &build_info {
            Ok(info) => info.version.as_deref().unwrap_or("unknown"),
            Err(e) => {
                warn!("Failed to get build info: {:?}. Using default version 'unknown'.", e);
                "unknown"
            }
        };
        // TODO(b/517822224): Remove this compression logic if the maximum vendor software version
        // string length is increased to 32 bytes.
        let compressed_version = self.compress_build_version(version);
        if let Err(e) = ot_instance.set_vendor_sw_version(&compressed_version) {
            warn!("Failed to set the vendor sw version {:?} {:?}", version, e);
        }

        let publisher =
            connect_to_protocol::<fidl_fuchsia_net_mdns::ServiceInstancePublisherMarker>().unwrap();

        let driver_future = run_driver(
            self.name.clone(),
            connect_to_protocol_at::<RegisterMarker>(self.service_prefix.as_str())
                .context("Failed to connect to Lowpan Registry service")?,
            connect_to_protocol_at::<FactoryRegisterMarker>(self.service_prefix.as_str()).ok(),
            ot_instance,
            netif,
            backbone_if,
            product_metadata,
            publisher,
        );

        Ok(driver_future)
    }
}

async fn run_driver<N, RP, RFP, NI, BI>(
    name: N,
    registry: RP,
    factory_registry: Option<RFP>,
    ot_instance: OtInstanceBox,
    net_if: NI,
    backbone_if: BI,
    product_metadata: ProductMetadata,
    publisher: fidl_fuchsia_net_mdns::ServiceInstancePublisherProxy,
) -> Result<(), Error>
where
    N: AsRef<str>,
    RP: RegisterProxyInterface,
    RFP: FactoryRegisterProxyInterface,
    NI: NetworkInterface + Debug,
    BI: BackboneInterface,
{
    let name = name.as_ref();
    let (epskc_sender, epskc_receiver) = mpsc::channel(SERVICE_CHANNEL_SIZE);
    let (border_agent_sender, border_agent_receiver) = mpsc::channel(SERVICE_CHANNEL_SIZE);
    let driver = OtDriver::new(
        ot_instance,
        net_if,
        backbone_if,
        product_metadata,
        epskc_sender,
        border_agent_sender,
    );

    let driver_ref = &driver;

    let lowpan_device_task = register_and_serve_driver(name, registry, driver_ref);

    info!("Registered OpenThread LoWPAN device {}", name);

    let lowpan_device_factory_task = async move {
        if let Some(factory_registry) = factory_registry {
            if let Err(err) =
                register_and_serve_driver_factory(name, factory_registry, driver_ref).await
            {
                warn!("Unable to register and serve factory commands for {}: {:?}", name, err);
            }
        }

        // If the factory interface throws an error, don't kill the driver;
        // just let the rest keep running.
        futures::future::pending::<Result<(), Error>>().await
    };

    // All three of these tasks will run indefinitely
    // as long as there are no irrecoverable problems.
    //
    // We use `stream::select_all` here so that only the
    // futures that actually need to be polled get polled.
    futures::stream::select_all([
        driver.main_loop_stream(epskc_receiver, border_agent_receiver, publisher).boxed(),
        lowpan_device_task.into_stream().boxed(),
        lowpan_device_factory_task.into_stream().boxed(),
    ])
    .try_collect::<()>()
    .await?;

    info!("OpenThread LoWPAN device {} has shutdown.", name);

    Ok(())
}

// The OpenThread platform implementation currently requires a multithreaded executor.
#[fuchsia::main(threads = 10, logging = false)]
async fn main() -> Result<(), Error> {
    use std::path::Path;

    let config = Config::try_new().context("Config::try_new")?;

    // Use the diagnostics_log library directly rather than e.g. the #[fuchsia::main] macro on
    // the main function, so that we can specify the logging severity level at runtime based on a
    // command line argument.
    diagnostics_log::initialize(
        diagnostics_log::PublishOptions::default().minimum_severity(config.log_level),
    )?;

    // Make sure OpenThread is logging at a similar level as the rest of the system.
    ot::set_logging_level(openthread_fuchsia::logging::ot_log_level_from(config.log_level));

    if Path::new("/config/data/bootstrap_config.json").exists() {
        warn!("Bootstrapping thread. Skipping ot-driver loop.");
        return bootstrap::bootstrap_thread().await;
    }

    let mut attempt_count = 0;
    loop {
        info!("Starting LoWPAN OT Driver");

        let driver_future = config
            .prepare_to_run()
            .inspect_err(|e| error!("main:prepare_to_run: {:?}", e))
            .await
            .context("main:prepare_to_run")?
            .boxed();

        let start_timestamp = fasync::MonotonicInstant::now();

        let ret = driver_future.await.context("main:driver_task");

        if (fasync::MonotonicInstant::now() - start_timestamp).into_minutes()
            >= RESET_EXPONENTIAL_BACKOFF_TIMER_MIN
        {
            // If the past run has been running for `RESET_EXPONENTIAL_BACKOFF_TIMER_MIN`
            // minutes or longer, then we go ahead and reset the attempt count.
            attempt_count = 0;
        }

        if config.max_auto_restarts <= attempt_count {
            panic!("Failed {} attempts to restart OpenThread: {ret:?}", config.max_auto_restarts);
        }

        // Implement an exponential backoff for restarts.
        let delay = (1 << attempt_count).min(MAX_EXPONENTIAL_BACKOFF_DELAY_SEC);

        if ret
            .as_ref()
            .map_err(|err| err.is::<driver::ResetRequested>() || err.is::<BackboneNetworkChanged>())
            .err()
            .unwrap_or(false)
        {
            // This is an expected OpenThread reset.
            warn!("OpenThread Reset: {:?}", ret);
        } else {
            error!("Unexpected shutdown: {:?}", ret);
            warn!("Will attempt to restart in {} seconds.", delay);

            fasync::Timer::new(fasync::MonotonicInstant::after(fz::Duration::from_seconds(delay)))
                .await;

            attempt_count += 1;

            info!("Restart attempt {} ({} max)", attempt_count, config.max_auto_restarts);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Config;

    #[fuchsia::test]
    fn test_compress_build_version_under_limit() {
        let config = Config::default();
        // Exactly 16 bytes
        let version = "31.12345.1234567";
        assert_eq!(config.compress_build_version(version), "31.12345.1234567");

        // Under 16 bytes
        let version = "31.103.1";
        assert_eq!(config.compress_build_version(version), "31.103.1");
    }

    #[fuchsia::test]
    fn test_compress_build_version_removing_century() {
        let config = Config::default();
        // Original: 17 bytes -> Century removed: 15 bytes
        let version = "31.20260312.103.1";
        assert_eq!(config.compress_build_version(version), "31.260312.103.1");
    }

    #[fuchsia::test]
    fn test_compress_build_version_removing_date_entirely() {
        let config = Config::default();
        // Original: 23 bytes -> Century removed: 21 bytes -> Date removed: 14 bytes
        let version = "29.20251023.103.2102100";
        assert_eq!(config.compress_build_version(version), "29.103.2102100");
    }

    #[fuchsia::test]
    fn test_compress_build_version_removing_dots() {
        let config = Config::default();
        // Original: 26 bytes -> Century removed: 24 bytes ("123.261212.12345.12345.6")
        //                    -> Date removed: 17 bytes ("123.12345.12345.6")
        //                    -> Dots removed (1 dot left): 15 bytes ("123.12345123456")
        let version = "123.20261212.12345.12345.6";
        assert_eq!(config.compress_build_version(version), "123.12345123456");
    }

    #[fuchsia::test]
    fn test_compress_build_version_truncating_to_16_bytes_fallback() {
        let config = Config::default();
        // Original: 30 bytes
        // Date removed: 21 bytes ("123.1234567.123456789")
        // Dots removed (1 dot left): 20 bytes ("123.1234567123456789")
        // Truncated: 16 bytes
        let version = "123.20261212.1234567.123456789";
        assert_eq!(config.compress_build_version(version), "123.123456712345");
    }

    #[fuchsia::test]
    fn test_compress_build_version_truncating_unrecognized_format() {
        let config = Config::default();
        // E.g., the version is the latest commit date.
        let version = "2026-01-28T05:00:35+00:00";
        assert_eq!(config.compress_build_version(version), "2026-01-28T05:00");
    }

    #[fuchsia::test]
    fn test_compress_build_version_malformed_short_date() {
        let config = Config::default();
        // Tests safety when the segment after the first dot is unexpectedly short
        // Cannot remove century safely, should fallback to date removal or truncation
        let version = "31.20.103.2102100"; // 17 chars
        assert_eq!(config.compress_build_version(version), "31.103.2102100");
    }
}

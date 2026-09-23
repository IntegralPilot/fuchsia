// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[allow(unused_imports)]
use super::prelude::*;

use futures::channel::mpsc;
use openthread::ot;
use zx::MonotonicDuration;

mod api;
mod border_agent;
mod connectivity_state;
mod convert;
mod detailed_logging;
mod dhcpv6pd;
mod driver_state;
mod error_adapter;
mod host_to_thread;
mod joiner;
mod multicast_routing_manager;
mod nat64;
mod ot_ctl;
mod srp_proxy;
mod tasks;
mod thread_to_host;

#[cfg(test)]
mod tests;

pub use connectivity_state::*;
pub use convert::*;
pub use dhcpv6pd::*;
use driver_state::*;
pub use error_adapter::*;
use lowpan_driver_common::AsyncCondition;
pub use lowpan_driver_common::net::{BackboneInterface, NetworkInterface};
pub use multicast_routing_manager::*;
pub use nat64::*;
pub use srp_proxy::*;

const DEFAULT_SCAN_DWELL_TIME_MS: u32 = 200;

#[cfg(not(test))]
const DEFAULT_TIMEOUT: MonotonicDuration = MonotonicDuration::from_seconds(5);

#[cfg(test)]
const DEFAULT_TIMEOUT: MonotonicDuration = MonotonicDuration::from_seconds(90);

#[allow(unused)]
const JOIN_TIMEOUT: MonotonicDuration = MonotonicDuration::from_seconds(120);

/// Extra time that is added to the network/energy scans, in addition to the calculated timeout.
const SCAN_EXTRA_TIMEOUT: MonotonicDuration = MonotonicDuration::from_seconds(10);

#[allow(unused)]
const STD_IPV6_NET_PREFIX_LEN: u8 = 64;

#[derive(thiserror::Error, Debug, Eq, PartialEq)]
pub struct ResetRequested;

impl std::fmt::Display for ResetRequested {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self, f)
    }
}

#[derive(Debug)]
pub struct OtDriver<OT, NI, BI> {
    /// Internal, mutex-protected driver state.
    driver_state: fuchsia_sync::Mutex<DriverState<OT>>,

    /// Condition that fires whenever the above `driver_state` changes.
    driver_state_change: AsyncCondition,

    /// Network Interface. Provides the interface to netstack.
    net_if: NI,

    /// Backbone Interface. Provides support of bouder routing and TREL.
    backbone_if: BI,

    /// Information about the platform on which the driver is running.  This information is cached
    /// to prevent repeated service reconnections and redundant queries.
    product_metadata: ProductMetadata,

    border_agent_publisher:
        fuchsia_sync::Mutex<mpsc::Sender<border_agent::BorderAgentPublishRequest>>,

    /// The current meshcop TXT records.
    #[allow(clippy::type_complexity)]
    border_agent_current_txt_entries: std::sync::Arc<futures::lock::Mutex<Vec<(String, Vec<u8>)>>>,

    /// Additional TXT records set by `meshcop_update_txt_entries`.
    border_agent_vendor_txt_entries: futures::lock::Mutex<Vec<(String, Vec<u8>)>>,

    /// Information about the platform on which the driver is running.  This information is cached
    /// to prevent repeated service reconnections and redundant queries.
    /// Commands ePSKc publishing logic to start or stop.
    epskc_publisher: fuchsia_sync::Mutex<mpsc::Sender<border_agent::PublishServiceRequest>>,
}

impl<OT: ot::Cli + ot::BackboneRouter, NI: NetworkInterface, BI: BackboneInterface>
    OtDriver<OT, NI, BI>
{
    pub fn new(
        ot_instance: OT,
        net_if: NI,
        backbone_if: BI,
        product_metadata: ProductMetadata,
        epskc_publisher: mpsc::Sender<border_agent::PublishServiceRequest>,
        border_agent_publisher: mpsc::Sender<border_agent::BorderAgentPublishRequest>,
    ) -> Self {
        OtDriver {
            driver_state: fuchsia_sync::Mutex::new(DriverState::new(
                ot_instance,
                net_if.get_nicid(),
                backbone_if.get_nicid(),
            )),
            driver_state_change: AsyncCondition::new(),
            net_if,
            backbone_if,
            border_agent_current_txt_entries: std::sync::Arc::new(futures::lock::Mutex::new(
                vec![],
            )),
            border_agent_vendor_txt_entries: futures::lock::Mutex::new(vec![]),
            product_metadata,
            epskc_publisher: fuchsia_sync::Mutex::new(epskc_publisher),
            border_agent_publisher: fuchsia_sync::Mutex::new(border_agent_publisher),
        }
    }

    /// Decorates the given future with error mapping,
    /// reset handling, and a standard timeout.
    pub fn apply_standard_combinators<'a, F>(
        &'a self,
        future: F,
    ) -> impl Future<Output = ZxResult<F::Ok>> + 'a
    where
        F: TryFuture<Error = anyhow::Error> + Unpin + Send + 'a,
        <F as TryFuture>::Ok: Send,
    {
        future
            .inspect_err(|e| error!("apply_standard_combinators: error is \"{:?}\"", e))
            .map_err(|e| ZxStatus::from(ErrorAdapter(e)))
            .on_timeout(fasync::MonotonicInstant::after(DEFAULT_TIMEOUT), || {
                error!("Timeout");
                Err(ZxStatus::TIMED_OUT)
            })
    }

    pub fn is_net_type_supported(&self, net_type: &str) -> bool {
        net_type.starts_with(fidl_fuchsia_lowpan_device::NET_TYPE_THREAD_1_X)
    }
}

/// Helper type for performing cleanup operations when dropped.
struct CleanupFunc<F: FnOnce()>(Option<F>);
impl<F: FnOnce()> CleanupFunc<F> {
    /// Disarms the cleanup func so that it will not execute when dropped.
    #[allow(dead_code)]
    fn disarm(&mut self) {
        let _ = self.0.take();
    }
}
impl<F: FnOnce()> Drop for CleanupFunc<F> {
    fn drop(&mut self) {
        if let Some(func) = self.0.take() {
            func();
        }
    }
}

static DEFAULT_VENDOR: &str = "Unknown";
static DEFAULT_PRODUCT: &str = "Fuchsia";

#[derive(Debug)]
pub(crate) struct ProductMetadata {
    vendor: String,
    product: String,
}

impl ProductMetadata {
    fn new(vendor: String, product: String) -> Self {
        Self { vendor, product }
    }

    pub(crate) fn vendor(&self) -> String {
        self.vendor.clone()
    }

    pub(crate) fn product(&self) -> String {
        self.product.clone()
    }
}

impl std::default::Default for ProductMetadata {
    fn default() -> Self {
        Self::new(DEFAULT_VENDOR.to_string(), DEFAULT_PRODUCT.to_string())
    }
}

pub(crate) async fn get_product_metadata(
    hw_info_proxy: fidl_fuchsia_hwinfo::ProductProxy,
) -> ProductMetadata {
    match hw_info_proxy.get_info().await {
        Ok(info) => {
            let vendor = info.manufacturer.unwrap_or_else(|| DEFAULT_VENDOR.to_string());
            let product = info.model.unwrap_or_else(|| DEFAULT_PRODUCT.to_string());
            ProductMetadata::new(vendor, product)
        }
        Err(err) => {
            warn!("Unable to get product info: {:?}", err);
            ProductMetadata::default()
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use assert_matches::assert_matches;
    use fidl::endpoints::create_proxy;
    use fuchsia_async::TestExecutor;
    use std::pin::pin;
    use std::task::Poll;

    #[fuchsia::test]
    fn test_product_metadata() {
        let vendor = "qwer";
        let product = "asdf";
        let product_metadata = ProductMetadata::new(vendor.to_string(), product.to_string());

        assert_eq!(product_metadata.vendor(), vendor.to_string());
        assert_eq!(product_metadata.product(), product.to_string());
    }

    #[fuchsia::test]
    fn test_product_metadata_default() {
        let product_metadata = ProductMetadata::default();

        assert_eq!(product_metadata.vendor(), DEFAULT_VENDOR.to_string());
        assert_eq!(product_metadata.product(), DEFAULT_PRODUCT.to_string());
    }

    #[fuchsia::test]
    fn test_get_metadata_succeeds() {
        let mut exec = TestExecutor::new();
        let (proxy, server) = create_proxy::<fidl_fuchsia_hwinfo::ProductMarker>();
        let mut req_stream = server.into_stream();
        let fut = get_product_metadata(proxy);
        let mut fut = pin!(fut);

        let fake_manufacturer = String::from("asdf");
        let fake_model = String::from("qwer");

        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut req_stream.next()),
            Poll::Ready(Some(Ok(fidl_fuchsia_hwinfo::ProductRequest::GetInfo { responder }))) => {
                responder.send(&fidl_fuchsia_hwinfo::ProductInfo {
                    manufacturer: Some(fake_manufacturer.clone()),
                    model: Some(fake_model.clone()),
                    ..Default::default()
                }).expect("failed to send response")
            }
        );

        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(metadata) => {
            assert_eq!(metadata.vendor(), fake_manufacturer.to_string());
            assert_eq!(metadata.product(), fake_model.to_string());
        });
    }

    #[fuchsia::test]
    fn test_empty_product_info() {
        let mut exec = TestExecutor::new();
        let (proxy, server) = create_proxy::<fidl_fuchsia_hwinfo::ProductMarker>();
        let mut req_stream = server.into_stream();
        let fut = get_product_metadata(proxy);
        let mut fut = pin!(fut);

        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut req_stream.next()),
            Poll::Ready(Some(Ok(fidl_fuchsia_hwinfo::ProductRequest::GetInfo { responder }))) => {
                responder.send(&fidl_fuchsia_hwinfo::ProductInfo {
                    ..Default::default()
                }).expect("failed to send response")
            }
        );

        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(metadata) => {
            assert_eq!(metadata.vendor(), DEFAULT_VENDOR.to_string());
            assert_eq!(metadata.product(), DEFAULT_PRODUCT.to_string());
        });
    }

    #[fuchsia::test]
    fn test_info_query_fails() {
        let mut exec = TestExecutor::new();

        // Drop the server end so the request fails.
        let (proxy, _) = create_proxy::<fidl_fuchsia_hwinfo::ProductMarker>();
        let fut = get_product_metadata(proxy);
        let mut fut = pin!(fut);

        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(metadata) => {
            assert_eq!(metadata.vendor(), DEFAULT_VENDOR.to_string());
            assert_eq!(metadata.product(), DEFAULT_PRODUCT.to_string());
        });
    }
}

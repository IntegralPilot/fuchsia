// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mode_management::phy_manager::PhyManagerApi;
use fidl_fuchsia_wlan_product_deprecatedconfiguration as fidl_deprecated;
use futures::lock::Mutex;
use futures::{StreamExt, select};
use ieee80211::MacAddr;
use log::{error, info};
use std::sync::Arc;

#[derive(Clone)]
pub struct DeprecatedConfigurator {
    phy_manager: Arc<Mutex<dyn PhyManagerApi>>,
}

impl DeprecatedConfigurator {
    pub fn new(phy_manager: Arc<Mutex<dyn PhyManagerApi>>) -> Self {
        DeprecatedConfigurator { phy_manager }
    }

    pub async fn serve_deprecated_configuration(
        self,
        mut requests: fidl_deprecated::DeprecatedConfiguratorRequestStream,
    ) {
        loop {
            select! {
                req = requests.select_next_some() => match req {
                    Ok(req) => match req {
                        fidl_deprecated::DeprecatedConfiguratorRequest::SuggestAccessPointMacAddress{mac, responder} => {
                            info!("setting suggested AP MAC");
                            let mac = MacAddr::from(mac.octets);

                            if !mac.is_unicast() || mac == ieee80211::NULL_ADDR {
                                error!("rejecting invalid suggested AP MAC");
                                if let Err(e) = responder.send(Err(fidl_deprecated::SuggestMacAddressError::InvalidArguments)) {
                                    error!("could not send SuggestAccessPointMacAddress response: {:?}", e);
                                }
                                continue;
                            }

                            let mut phy_manager = self.phy_manager.lock().await;
                            phy_manager.suggest_ap_mac(mac);

                            match responder.send(Ok(())) {
                                Ok(()) => {}
                                Err(e) => {
                                    error!("could not send SuggestAccessPointMacAddress response: {:?}", e);
                                }
                            }
                        }
                    }
                    Err(e) => error!("encountered an error while serving deprecated configuration requests: {}", e)
                },
                complete => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::types as client_types;
    use crate::mode_management::Defect;
    use crate::mode_management::phy_manager::{CreateClientIfacesReason, PhyManagerError};
    use crate::mode_management::recovery::RecoverySummary;
    use assert_matches::assert_matches;
    use async_trait::async_trait;
    use fidl::endpoints::create_proxy;
    use fuchsia_async as fasync;
    use futures::task::Poll;
    use std::collections::HashMap;
    use std::pin::pin;
    use std::unimplemented;
    use test_case::test_case;

    #[derive(Debug)]
    struct StubPhyManager(Option<MacAddr>);

    impl StubPhyManager {
        fn new() -> Self {
            StubPhyManager(None)
        }
    }

    #[async_trait(?Send)]
    impl PhyManagerApi for StubPhyManager {
        async fn add_phy(&mut self, _phy_id: u16) -> Result<(), PhyManagerError> {
            unimplemented!();
        }

        fn remove_phy(&mut self, _phy_id: u16) {
            unimplemented!();
        }

        async fn on_iface_added(&mut self, _iface_id: u16) -> Result<(), PhyManagerError> {
            unimplemented!();
        }

        fn on_iface_removed(&mut self, _iface_id: u16) {
            unimplemented!();
        }

        async fn create_all_client_ifaces(
            &mut self,
            _reason: CreateClientIfacesReason,
        ) -> HashMap<u16, Result<Vec<u16>, PhyManagerError>> {
            unimplemented!();
        }

        fn client_connections_enabled(&self) -> bool {
            unimplemented!();
        }

        async fn destroy_all_client_ifaces(&mut self) -> Result<(), PhyManagerError> {
            unimplemented!();
        }

        fn get_client(&mut self) -> Option<u16> {
            unimplemented!();
        }

        async fn create_or_get_ap_iface(&mut self) -> Result<Option<u16>, PhyManagerError> {
            unimplemented!();
        }

        async fn destroy_ap_iface(&mut self, _iface_id: u16) -> Result<(), PhyManagerError> {
            unimplemented!();
        }

        async fn destroy_all_ap_ifaces(&mut self) -> Result<(), PhyManagerError> {
            unimplemented!();
        }

        fn suggest_ap_mac(&mut self, mac: MacAddr) {
            self.0 = Some(mac);
        }

        fn get_phy_ids(&self) -> Vec<u16> {
            unimplemented!();
        }

        fn log_phy_add_failure(&mut self) {
            unimplemented!();
        }

        async fn set_country_code(
            &mut self,
            _country_code: Option<client_types::CountryCode>,
        ) -> Result<(), PhyManagerError> {
            unimplemented!();
        }

        fn record_defect(&mut self, _defect: Defect) {
            unimplemented!();
        }

        async fn perform_recovery(&mut self, _summary: RecoverySummary) {
            unimplemented!();
        }
    }

    #[fuchsia::test]
    fn test_suggest_mac_succeeds() {
        let mut exec = fasync::TestExecutor::new();

        // Set up the DeprecatedConfigurator.
        let phy_manager = Arc::new(Mutex::new(StubPhyManager::new()));
        let configurator = DeprecatedConfigurator::new(phy_manager.clone());

        // Create the request stream and proxy.
        let (configurator_proxy, remote) =
            create_proxy::<fidl_deprecated::DeprecatedConfiguratorMarker>();
        let stream = remote.into_stream();

        // Kick off the serve loop and wait for it to stall out waiting for requests.
        let fut = configurator.serve_deprecated_configuration(stream);
        let mut fut = pin!(fut);
        assert!(exec.run_until_stalled(&mut fut).is_pending());

        // Issue a request to set the MAC address.
        let octets = [0xde, 0xad, 0xbe, 0xef, 0x00, 0x01];
        let mac = fidl_fuchsia_net::MacAddress { octets };
        let mut suggest_fut = configurator_proxy.suggest_access_point_mac_address(&mac);
        assert!(exec.run_until_stalled(&mut fut).is_pending());

        // Verify that the MAC has been set on the PhyManager
        let lock_fut = phy_manager.lock();
        let mut lock_fut = pin!(lock_fut);
        let phy_manager = assert_matches!(
            exec.run_until_stalled(&mut lock_fut),
            Poll::Ready(phy_manager) => {
                phy_manager
            }
        );
        let expected_mac = MacAddr::from(octets);
        assert_eq!(Some(expected_mac), phy_manager.0);

        assert_matches!(exec.run_until_stalled(&mut suggest_fut), Poll::Ready(Ok(Ok(()))));
    }

    #[test_case([0xff, 0xff, 0xff, 0xff, 0xff, 0xff] ; "broadcast")]
    #[test_case([0x01, 0x00, 0x5e, 0x00, 0x00, 0x01] ; "multicast")]
    #[fuchsia::test]
    fn test_suggest_mac_fails(octets: [u8; 6]) {
        let mut exec = fasync::TestExecutor::new();

        // Set up the DeprecatedConfigurator.
        let phy_manager = Arc::new(Mutex::new(StubPhyManager::new()));
        let configurator = DeprecatedConfigurator::new(phy_manager.clone());

        // Create the request stream and proxy.
        let (configurator_proxy, remote) =
            create_proxy::<fidl_deprecated::DeprecatedConfiguratorMarker>();
        let stream = remote.into_stream();

        // Kick off the serve loop and wait for it to stall out waiting for requests.
        let fut = configurator.serve_deprecated_configuration(stream);
        let mut fut = pin!(fut);
        assert!(exec.run_until_stalled(&mut fut).is_pending());

        // Issue a request to set the MAC address.
        let mac = fidl_fuchsia_net::MacAddress { octets };
        let mut suggest_fut = configurator_proxy.suggest_access_point_mac_address(&mac);
        assert!(exec.run_until_stalled(&mut fut).is_pending());

        // Verify that the MAC has been set on the PhyManager
        let lock_fut = phy_manager.lock();
        let mut lock_fut = pin!(lock_fut);
        let phy_manager = assert_matches!(
            exec.run_until_stalled(&mut lock_fut),
            Poll::Ready(phy_manager) => {
                phy_manager
            }
        );
        assert_eq!(None, phy_manager.0);

        assert_matches!(
            exec.run_until_stalled(&mut suggest_fut),
            Poll::Ready(Ok(Err(fidl_deprecated::SuggestMacAddressError::InvalidArguments)))
        );
    }
}

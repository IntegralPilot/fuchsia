// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fdf_component::{Driver, DriverContext, DriverError, Node, driver_register};
use fidl::endpoints::Proxy;
use fidl_fuchsia_wlan_fullmac as fidl_fullmac;
use fuchsia_sync::Mutex;
use log::{error, info};
use std::sync::Arc;
use wlan_ffi_transport::completers::Completer;
use wlan_fullmac_mlme::FullmacMlmeHandle;
use wlan_fullmac_mlme::device::FullmacDevice;
use zx::Status;

struct WlanifDriver {
    _node: Arc<Mutex<Option<Node>>>,
    mlme_handle: Mutex<Option<FullmacMlmeHandle>>,
    stop_called: Arc<Mutex<bool>>,
}

driver_register!(WlanifDriver);

impl Driver for WlanifDriver {
    const NAME: &str = "wlanif";

    async fn start(mut context: DriverContext) -> Result<Self, DriverError> {
        info!("wlanif driver starting...");

        let node = context.take_node()?;

        // Connect to the fullmac impl service exposed by our parent
        let service = context.incoming.service_marker(fidl_fullmac::ServiceMarker).connect()?;
        let fullmac_proxy = service.connect_to_wlan_fullmac_impl().map_err(|e| {
            error!("failed to connect to fullmac impl: {:?}", e);
            Status::CONNECTION_REFUSED
        })?;

        let sync_proxy = fidl_fullmac::WlanFullmacImpl_SynchronousProxy::new(
            fullmac_proxy.into_channel().expect("failed to convert into channel").into(),
        );

        let device = FullmacDevice::new(sync_proxy);

        // The Completer handles driver shutdown, which can happen in two ways:
        // 1. The driver framework begins unbind (handled in `stop`). The driver must stop
        //    the Rust MLME before completing. This is the expected sequence.
        // 2. The Rust MLME thread exits early (e.g. channel dropped). The Completer handles this
        //    by dropping the Node to signal shutdown to FDF. This is considered an error in the sequence.
        let node = Arc::new(Mutex::new(Some(node)));
        let node_clone = Arc::clone(&node);
        let stop_called = Arc::new(Mutex::new(false));
        let stop_called_clone = Arc::clone(&stop_called);
        let completer = Completer::new(move |status| {
            info!("MLME exited with status: {}", status);
            if !*stop_called_clone.lock() {
                error!("MLME exited before stop() was called");
            }
            // Reset the node to let the Driver Framework know we are shutting down
            node_clone.lock().take();
        });

        let mlme_handle =
            match wlan_fullmac_mlme::start_and_serve_on_separate_thread(device, completer).await {
                Ok(handle) => handle,
                Err(e) => {
                    error!("Failed to start FullMAC MLME: {:?}", e);
                    return Err(Status::INTERNAL.into());
                }
            };

        Ok(Self { _node: node, mlme_handle: Mutex::new(Some(mlme_handle)), stop_called })
    }

    async fn stop(&self) {
        info!("wlanif driver stopping...");
        *self.stop_called.lock() = true;
        if let Some(handle) = self.mlme_handle.lock().as_mut() {
            handle.request_stop();
        }
    }
}

impl Drop for WlanifDriver {
    fn drop(&mut self) {
        if let Some(handle) = self.mlme_handle.lock().take() {
            handle.delete();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fdf_component::ServiceOffer;
    use fdf_component::testing::harness::TestHarness;
    use fidl_fuchsia_wlan_common as fidl_common;

    use fidl_fuchsia_wlan_sme as fidl_sme;
    use fuchsia_async as fasync;
    use fuchsia_component::server::ServiceFs;
    use futures::StreamExt;

    use zx::Channel;

    #[derive(Debug, PartialEq, Clone)]
    enum FullmacCall {
        Init,
        Query,
        QuerySecuritySupport,
        QuerySpectrumManagementSupport,
    }

    struct MockState {
        calls: Vec<FullmacCall>,
        query_error: Option<zx::Status>,
        query_security_error: Option<zx::Status>,
        query_spectrum_error: Option<zx::Status>,
        ifc: Option<fidl::endpoints::ClientEnd<fidl_fullmac::WlanFullmacImplIfcMarker>>,
        sme: Option<fidl::endpoints::ClientEnd<fidl_sme::GenericSmeMarker>>,
    }

    #[derive(Clone)]
    struct MockFullmacImpl {
        state: Arc<Mutex<MockState>>,
    }

    impl MockFullmacImpl {
        fn create_and_offer(
            service_fs: &mut ServiceFs<fuchsia_component::server::ServiceObj<'static, ()>>,
        ) -> (Self, fidl_fuchsia_driver_framework::Offer) {
            let mock = Self {
                state: Arc::new(Mutex::new(MockState {
                    calls: vec![],
                    query_error: None,
                    query_security_error: None,
                    query_spectrum_error: None,
                    ifc: None,
                    sme: None,
                })),
            };
            let mock_clone = mock.clone();
            let offer = ServiceOffer::<fidl_fullmac::ServiceMarker>::new()
                .add_default_named(
                    service_fs,
                    "default",
                    move |req: fidl_fullmac::ServiceRequest| {
                        let mock = mock_clone.clone();
                        match req {
                            fidl_fullmac::ServiceRequest::WlanFullmacImpl(stream) => {
                                fasync::Task::spawn(mock.run(stream)).detach();
                            }
                        }
                    },
                )
                .build_zircon_offer();
            (mock, offer)
        }

        fn take_ifc(
            &self,
        ) -> Option<fidl::endpoints::ClientEnd<fidl_fullmac::WlanFullmacImplIfcMarker>> {
            self.state.lock().ifc.take()
        }

        fn take_sme(&self) -> Option<fidl::endpoints::ClientEnd<fidl_sme::GenericSmeMarker>> {
            self.state.lock().sme.take()
        }

        fn drain_calls(&self) -> Vec<FullmacCall> {
            self.state.lock().calls.drain(..).collect()
        }

        fn set_query_error(&self, status: zx::Status) {
            self.state.lock().query_error = Some(status);
        }

        fn set_query_security_error(&self, status: zx::Status) {
            self.state.lock().query_security_error = Some(status);
        }

        fn set_query_spectrum_error(&self, status: zx::Status) {
            self.state.lock().query_spectrum_error = Some(status);
        }

        async fn run(self, mut stream: fidl_fullmac::WlanFullmacImpl_RequestStream) {
            while let Some(req) = stream.next().await {
                match req.unwrap() {
                    fidl_fullmac::WlanFullmacImpl_Request::Init { payload, responder } => {
                        let (generic_sme_client, generic_sme_server) =
                            fidl::endpoints::create_endpoints::<fidl_sme::GenericSmeMarker>();

                        let mut state = self.state.lock();
                        state.calls.push(FullmacCall::Init);
                        state.ifc = Some(payload.ifc.unwrap());
                        state.sme = Some(generic_sme_client);

                        let (sme_client, sme_server) = Channel::create();

                        responder
                            .send(Ok(fidl_fullmac::WlanFullmacImplInitResponse {
                                sme_channel: Some(sme_server),
                                ..Default::default()
                            }))
                            .unwrap();

                        // Trigger USME bootstrap immediately after driver init to unblock MLME startup.
                        let usme_proxy = fidl_sme::UsmeBootstrapProxy::new(
                            fidl::AsyncChannel::from_channel(sme_client),
                        );
                        let legacy_privacy_support = fidl_sme::LegacyPrivacySupport {
                            wep_supported: false,
                            wpa1_supported: false,
                        };
                        let _ = usme_proxy.start(generic_sme_server, &legacy_privacy_support);
                    }
                    fidl_fullmac::WlanFullmacImpl_Request::Query { responder } => {
                        self.state.lock().calls.push(FullmacCall::Query);
                        let error = self.state.lock().query_error;
                        if let Some(status) = error {
                            responder.send(Err(status.into_raw())).unwrap();
                        } else {
                            responder
                                .send(Ok(&fidl_fullmac::WlanFullmacImplQueryResponse {
                                    sta_addr: Some([8, 8, 8, 8, 8, 8]),
                                    factory_addr: Some([8, 8, 8, 8, 8, 8]),
                                    role: Some(fidl_common::WlanMacRole::Client),
                                    band_caps: Some(vec![]),
                                    ..Default::default()
                                }))
                                .unwrap();
                        }
                    }
                    fidl_fullmac::WlanFullmacImpl_Request::QuerySecuritySupport { responder } => {
                        self.state.lock().calls.push(FullmacCall::QuerySecuritySupport);
                        let error = self.state.lock().query_security_error;
                        if let Some(status) = error {
                            responder.send(Err(status.into_raw())).unwrap();
                        } else {
                            responder
                                .send(Ok(
                                    &fidl_fullmac::WlanFullmacImplQuerySecuritySupportResponse {
                                        resp: Some(fidl_common::SecuritySupport {
                                            sae: Some(fidl_common::SaeFeature {
                                                driver_handler_supported: Some(false),
                                                sme_handler_supported: Some(true),
                                                hash_to_element_supported: Some(false),
                                                ..Default::default()
                                            }),
                                            mfp: Some(fidl_common::MfpFeature {
                                                supported: Some(false),
                                                ..Default::default()
                                            }),
                                            owe: Some(fidl_common::OweFeature {
                                                supported: Some(false),
                                                ..Default::default()
                                            }),
                                            ..Default::default()
                                        }),
                                        ..Default::default()
                                    },
                                ))
                                .unwrap();
                        }
                    }
                    fidl_fullmac::WlanFullmacImpl_Request::QuerySpectrumManagementSupport {
                        responder,
                    } => {
                        self.state.lock().calls.push(FullmacCall::QuerySpectrumManagementSupport);
                        let error = self.state.lock().query_spectrum_error;
                        if let Some(status) = error {
                            responder.send(Err(status.into_raw())).unwrap();
                        } else {
                            responder
                                .send(Ok(
                                    &fidl_fullmac::WlanFullmacImplQuerySpectrumManagementSupportResponse {
                                        resp: Some(fidl_common::SpectrumManagementSupport {
                                            ..Default::default()
                                        }),
                                        ..Default::default()
                                    },
                                ))
                                .unwrap();
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    #[fuchsia::test]
    async fn test_driver_lifecycle() {
        let mut service_fs = ServiceFs::new();
        let (mock, offer) = MockFullmacImpl::create_and_offer(&mut service_fs);

        let mut harness =
            TestHarness::<WlanifDriver>::new().set_driver_incoming(service_fs).add_offer(offer);

        let started_driver = harness.start_driver().await.expect("failed to start driver");

        // Verify calls after start.
        let calls = mock.drain_calls();
        assert!(calls.len() >= 4);
        assert_eq!(
            &calls[0..4],
            &[
                FullmacCall::Init,
                FullmacCall::Query,
                FullmacCall::QuerySecuritySupport,
                FullmacCall::QuerySpectrumManagementSupport,
            ]
        );

        started_driver.stop_driver().await;
    }

    #[fuchsia::test]
    async fn test_start_fails_if_query_fails() {
        let mut service_fs = ServiceFs::new();
        let (mock, offer) = MockFullmacImpl::create_and_offer(&mut service_fs);
        mock.set_query_error(zx::Status::INTERNAL);

        let mut harness =
            TestHarness::<WlanifDriver>::new().set_driver_incoming(service_fs).add_offer(offer);

        let start_result = harness.start_driver().await;
        assert!(start_result.is_err());
    }

    #[fuchsia::test]
    async fn test_start_fails_if_query_security_fails() {
        let mut service_fs = ServiceFs::new();
        let (mock, offer) = MockFullmacImpl::create_and_offer(&mut service_fs);
        mock.set_query_security_error(zx::Status::INTERNAL);

        let mut harness =
            TestHarness::<WlanifDriver>::new().set_driver_incoming(service_fs).add_offer(offer);

        let start_result = harness.start_driver().await;
        assert!(start_result.is_err());
    }

    #[fuchsia::test]
    async fn test_start_fails_if_query_spectrum_fails() {
        let mut service_fs = ServiceFs::new();
        let (mock, offer) = MockFullmacImpl::create_and_offer(&mut service_fs);
        mock.set_query_spectrum_error(zx::Status::INTERNAL);

        let mut harness =
            TestHarness::<WlanifDriver>::new().set_driver_incoming(service_fs).add_offer(offer);

        let start_result = harness.start_driver().await;
        assert!(start_result.is_err());
    }

    const MAX_SHUTDOWN_ATTEMPTS: u32 = 200;

    async fn wait_for_dropped_node(driver: &WlanifDriver) {
        for _ in 0..MAX_SHUTDOWN_ATTEMPTS {
            if driver._node.lock().is_none() {
                return;
            }
            fasync::Timer::new(std::time::Duration::from_millis(10)).await;
        }
        panic!("timeout waiting for driver shutdown");
    }

    #[fuchsia::test]
    async fn test_dropping_ifc_causes_dropped_node() {
        let mut service_fs = ServiceFs::new();
        let (mock, offer) = MockFullmacImpl::create_and_offer(&mut service_fs);

        let mut harness =
            TestHarness::<WlanifDriver>::new().set_driver_incoming(service_fs).add_offer(offer);

        let started_driver = harness.start_driver().await.expect("failed to start driver");
        let driver = started_driver.get_driver().expect("expected driver");

        let ifc = mock.take_ifc().expect("expected ifc channel");
        std::mem::drop(ifc);

        wait_for_dropped_node(driver).await;

        started_driver.stop_driver().await;
    }

    #[fuchsia::test]
    async fn test_dropping_sme_causes_dropped_node() {
        let mut service_fs = ServiceFs::new();
        let (mock, offer) = MockFullmacImpl::create_and_offer(&mut service_fs);

        let mut harness =
            TestHarness::<WlanifDriver>::new().set_driver_incoming(service_fs).add_offer(offer);

        let started_driver = harness.start_driver().await.expect("failed to start driver");
        let driver = started_driver.get_driver().expect("expected driver");

        let sme = mock.take_sme().expect("expected sme channel");
        std::mem::drop(sme);

        wait_for_dropped_node(driver).await;

        started_driver.stop_driver().await;
    }
}

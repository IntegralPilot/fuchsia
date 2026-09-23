// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mode_management::iface_manager_api::IfaceManagerApi;
use anyhow::{Context, Error};
use fidl_fuchsia_location_namedplace::RegulatoryRegionWatcherProxy;
use futures::channel::oneshot;
use futures::lock::Mutex;
use log::{info, warn};
use std::sync::Arc;

pub struct RegulatoryManager<I: IfaceManagerApi + ?Sized> {
    regulatory_service: RegulatoryRegionWatcherProxy,
    iface_manager: Arc<Mutex<I>>,
}

mod country_code {
    use anyhow::{Error, format_err};
    use std::str::FromStr;
    const CODE_LENGTH: usize = 2;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct CountryCode([u8; CODE_LENGTH]);

    impl FromStr for CountryCode {
        type Err = Error;
        fn from_str(region: &str) -> Result<Self, Self::Err> {
            region
                .as_bytes()
                .try_into()
                .map_err(|_| {
                    format_err!("Invalid length: {region}, {} != {CODE_LENGTH}", region.len())
                })
                .map(CountryCode)
        }
    }

    impl From<CountryCode> for [u8; CODE_LENGTH] {
        fn from(country_code: CountryCode) -> Self {
            country_code.0
        }
    }
}

pub use country_code::CountryCode;

impl<I: IfaceManagerApi + ?Sized> RegulatoryManager<I> {
    pub fn new(
        regulatory_service: RegulatoryRegionWatcherProxy,
        iface_manager: Arc<Mutex<I>>,
    ) -> Self {
        RegulatoryManager { regulatory_service, iface_manager }
    }

    pub async fn run(&self, policy_notifier: oneshot::Sender<()>) -> Result<(), Error> {
        let mut policy_notifier = Some(policy_notifier);
        loop {
            let region_update = self
                .regulatory_service
                .get_region_update()
                .await
                .context("failed to get_update()")?;
            let region_string = match region_update {
                Some(region_string) => region_string,
                None => {
                    info!("No cached regulatory region is available.");
                    if let Some(notifier) = policy_notifier.take()
                        && notifier.send(()).is_err()
                    {
                        info!("Could not notify policy layer of initial region setting");
                    };

                    continue;
                }
            };

            info!("Received regulatory region code {}", region_string);

            let country_code = match region_string.parse::<CountryCode>() {
                Err(e) => {
                    warn!("{:?}", e.context("Failed to parse region string."));
                    continue;
                }
                Ok(country_code) => country_code,
            };

            // Apply the new country code.
            let mut iface_manager = self.iface_manager.lock().await;
            if let Err(e) = iface_manager.set_country(Some(country_code)).await {
                warn!("Failed to set region code: {:?}", e);
                continue;
            }

            if let Some(notifier) = policy_notifier.take()
                && notifier.send(()).is_err()
            {
                info!("Could not notify policy layer of initial region setting");
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Arc, IfaceManagerApi, Mutex, RegulatoryManager};
    use crate::access_point::{state_machine as ap_fsm, types as ap_types};
    use crate::client::types as client_types;
    use crate::mode_management::iface_manager_api::{ConnectAttemptRequest, SmeForScan};
    use anyhow::{Error, format_err};
    use assert_matches::assert_matches;
    use async_trait::async_trait;
    use fidl::endpoints::create_proxy;
    use fidl_fuchsia_location_namedplace::{
        RegulatoryRegionWatcherMarker, RegulatoryRegionWatcherRequest,
        RegulatoryRegionWatcherRequestStream,
    };
    use fuchsia_async as fasync;
    use futures::channel::{mpsc, oneshot};
    use futures::stream::{self, Stream, StreamExt};
    use futures::task::Poll;
    use std::pin::pin;
    use std::unimplemented;

    /// Holds all of the boilerplate required for testing RegulatoryManager.
    struct TestContext<S: Stream<Item = Result<(), Error>> + Unpin> {
        iface_manager: Arc<Mutex<StubIfaceManager<S>>>,
        regulatory_manager: RegulatoryManager<StubIfaceManager<S>>,
        regulatory_region_requests: RegulatoryRegionWatcherRequestStream,
        regulatory_sender: oneshot::Sender<()>,
        regulatory_receiver: oneshot::Receiver<()>,
        // Fields are dropped in declaration order. Always drop executor last because we hold other
        // zircon objects tied to the executor in this struct, and those can't outlive the executor.
        //
        // See
        // - https://fuchsia-docs.firebaseapp.com/rust/fuchsia_async/struct.TestExecutor.html
        // - https://doc.rust-lang.org/reference/destructors.html.
        executor: fasync::TestExecutor,
    }

    impl<S> TestContext<S>
    where
        S: Stream<Item = Result<(), Error>> + Unpin,
    {
        fn new(
            iface_manager: StubIfaceManager<S>,
        ) -> TestContext<impl Stream<Item = Result<(), Error>> + Unpin> {
            let executor = fasync::TestExecutor::new();
            let (regulatory_region_proxy, regulatory_region_server_channel) =
                create_proxy::<RegulatoryRegionWatcherMarker>();
            let iface_manager = Arc::new(Mutex::new(iface_manager));
            let regulatory_manager =
                RegulatoryManager::new(regulatory_region_proxy, iface_manager.clone());
            let regulatory_region_requests = regulatory_region_server_channel.into_stream();

            let (regulatory_sender, regulatory_receiver) = oneshot::channel();
            Self {
                executor,
                iface_manager,
                regulatory_manager,
                regulatory_region_requests,
                regulatory_sender,
                regulatory_receiver,
            }
        }
    }

    #[fuchsia::test]
    fn ignore_update_with_short_region_code() {
        let mut context = TestContext::new(make_default_stub_iface_manager());
        let regulatory_fut = context.regulatory_manager.run(context.regulatory_sender);
        let mut regulatory_fut = pin!(regulatory_fut);
        assert!(context.executor.run_until_stalled(&mut regulatory_fut).is_pending());

        let region_request_fut = &mut context.regulatory_region_requests.next();
        let responder = assert_matches!(
            context.executor.run_until_stalled(region_request_fut),
            Poll::Ready(Some(Ok(RegulatoryRegionWatcherRequest::GetRegionUpdate{responder}))) => responder
        );
        responder.send(Some("U")).expect("failed to send response");
        assert_matches!(context.executor.run_until_stalled(&mut regulatory_fut), Poll::Pending);

        assert_matches!(
            &context.executor.run_until_stalled(&mut context.regulatory_receiver),
            Poll::Pending
        );

        // Verify that there is a new region update request.
        let region_request_fut = &mut context.regulatory_region_requests.next();
        assert_matches!(
            context.executor.run_until_stalled(region_request_fut),
            Poll::Ready(Some(_))
        );
    }

    #[fuchsia::test]
    fn update_with_long_region_code_fails() {
        let mut context = TestContext::new(make_default_stub_iface_manager());
        let regulatory_fut = context.regulatory_manager.run(context.regulatory_sender);
        let mut regulatory_fut = pin!(regulatory_fut);
        assert!(context.executor.run_until_stalled(&mut regulatory_fut).is_pending());

        let region_request_fut = &mut context.regulatory_region_requests.next();
        let responder = assert_matches!(
            context.executor.run_until_stalled(region_request_fut),
            Poll::Ready(Some(Ok(RegulatoryRegionWatcherRequest::GetRegionUpdate{responder}))) => responder
        );
        assert_matches!(responder.send(Some("USA")), Err(fidl::Error::StringTooLong { .. }));
        assert_matches!(context.executor.run_until_stalled(&mut regulatory_fut), Poll::Pending);
    }

    #[fuchsia::test]
    fn propagates_update_on_region_code_with_valid_length() {
        let mut context = TestContext::new(make_default_stub_iface_manager());
        let regulatory_fut = context.regulatory_manager.run(context.regulatory_sender);
        let mut regulatory_fut = pin!(regulatory_fut);
        assert!(context.executor.run_until_stalled(&mut regulatory_fut).is_pending());

        let region_request_fut = &mut context.regulatory_region_requests.next();
        let region_responder = assert_matches!(
            context.executor.run_until_stalled(region_request_fut),
            Poll::Ready(Some(Ok(RegulatoryRegionWatcherRequest::GetRegionUpdate{responder}))) => responder
        );
        region_responder.send(Some("US")).expect("failed to send region response");
        assert!(context.executor.run_until_stalled(&mut regulatory_fut).is_pending());

        let iface_manager_fut = context.iface_manager.lock();
        let mut iface_manager_fut = pin!(iface_manager_fut);
        match context.executor.run_until_stalled(&mut iface_manager_fut) {
            Poll::Ready(iface_manager) => {
                assert_eq!(iface_manager.country_code, Some("US".parse().unwrap()))
            }
            Poll::Pending => panic!("Expected to be able to lock the IfaceManager."),
        };
    }

    #[fuchsia::test]
    fn does_not_propagate_invalid_length_region_code() {
        let mut context = TestContext::new(make_default_stub_iface_manager());
        let regulatory_fut = context.regulatory_manager.run(context.regulatory_sender);
        let mut regulatory_fut = pin!(regulatory_fut);
        assert!(context.executor.run_until_stalled(&mut regulatory_fut).is_pending());

        let region_request_fut = &mut context.regulatory_region_requests.next();
        let region_responder = assert_matches!(
            context.executor.run_until_stalled(region_request_fut),
            Poll::Ready(Some(Ok(RegulatoryRegionWatcherRequest::GetRegionUpdate{responder}))) => responder
        );
        region_responder.send(Some("U")).expect("failed to send region response");

        // Drive the RegulatoryManager until stalled, then verify that RegulatoryManager did not
        // send a request to the IfaceManager.
        let _ = context.executor.run_until_stalled(&mut regulatory_fut);

        let iface_manager_fut = context.iface_manager.lock();
        let mut iface_manager_fut = pin!(iface_manager_fut);
        match context.executor.run_until_stalled(&mut iface_manager_fut) {
            Poll::Ready(iface_manager) => assert_eq!(iface_manager.country_code, None),
            Poll::Pending => panic!("Expected to be able to lock the IfaceManager."),
        }

        assert_matches!(
            &context.executor.run_until_stalled(&mut context.regulatory_receiver),
            Poll::Pending
        );
    }

    #[fuchsia::test]
    fn does_not_propagate_null_update() {
        let mut context = TestContext::new(make_default_stub_iface_manager());
        let regulatory_fut = context.regulatory_manager.run(context.regulatory_sender);
        let mut regulatory_fut = pin!(regulatory_fut);
        assert!(context.executor.run_until_stalled(&mut regulatory_fut).is_pending());

        // Set the regulatory region to be non-None initially.
        {
            let iface_manager_fut = context.iface_manager.lock();
            let mut iface_manager_fut = pin!(iface_manager_fut);
            match context.executor.run_until_stalled(&mut iface_manager_fut) {
                Poll::Ready(mut iface_manager) => {
                    iface_manager.country_code = Some("US".parse().unwrap())
                }
                Poll::Pending => panic!("Expected to be able to lock the IfaceManager."),
            }
        }

        let region_request_fut = &mut context.regulatory_region_requests.next();
        let region_responder = assert_matches!(
            context.executor.run_until_stalled(region_request_fut),
            Poll::Ready(Some(Ok(RegulatoryRegionWatcherRequest::GetRegionUpdate{responder}))) => responder
        );
        region_responder.send(None).expect("failed to send region response");
        // Run RegulatoryManager until stalled. Getting a null update should not cause an error.
        assert!(context.executor.run_until_stalled(&mut regulatory_fut).is_pending());

        // Verify that no region change is applied to the IfaceManager.
        {
            let iface_manager_fut = context.iface_manager.lock();
            let mut iface_manager_fut = pin!(iface_manager_fut);
            match context.executor.run_until_stalled(&mut iface_manager_fut) {
                Poll::Ready(iface_manager) => assert!(iface_manager.country_code.is_some()),
                Poll::Pending => panic!("Expected to be able to lock the IfaceManager."),
            }
        }

        // Verify that the policy API is instructed to begin serving.
        assert_matches!(
            &context.executor.run_until_stalled(&mut context.regulatory_receiver),
            Poll::Ready(Ok(()))
        );
    }

    #[fuchsia::test]
    fn absorbs_iface_manager_failure() {
        let (mut set_country_responder, set_country_response_stream) = mpsc::channel(0);
        let mut context =
            TestContext::new(StubIfaceManager { country_code: None, set_country_response_stream });
        let regulatory_fut = context.regulatory_manager.run(context.regulatory_sender);
        let mut regulatory_fut = pin!(regulatory_fut);
        assert!(context.executor.run_until_stalled(&mut regulatory_fut).is_pending());

        // Drive the RegulatoryManager to request an update from RegulatoryRegionWatcher,
        // and deliver a RegulatoryRegion update.
        let region_request_fut = &mut context.regulatory_region_requests.next();
        let region_responder = assert_matches!(
            context.executor.run_until_stalled(region_request_fut),
            Poll::Ready(Some(Ok(RegulatoryRegionWatcherRequest::GetRegionUpdate{responder}))) => responder
        );
        region_responder.send(Some("US")).expect("failed to send region response");
        assert!(context.executor.run_until_stalled(&mut regulatory_fut).is_pending());

        // Setup an error response for when the RegulatoryManager tries to set the country code.
        set_country_responder
            .try_send(Err(format_err!("sending a test error")))
            .expect("internal error: failed to send fake response to StubIfaceManager");

        assert_matches!(&context.executor.run_until_stalled(&mut regulatory_fut), Poll::Pending);

        // Verify that there is a new region update request.
        let region_request_fut = &mut context.regulatory_region_requests.next();
        assert_matches!(
            context.executor.run_until_stalled(region_request_fut),
            Poll::Ready(Some(_))
        );
    }

    #[fuchsia::test]
    fn propagates_multiple_valid_region_code_updates_to_device_service() {
        let mut context = TestContext::new(make_default_stub_iface_manager());
        let regulatory_fut = context.regulatory_manager.run(context.regulatory_sender);
        let mut regulatory_fut = pin!(regulatory_fut);

        // Receive first `RegulatoryRegionWatcher` update, and propagate it to `IfaceManager`.
        {
            assert!(context.executor.run_until_stalled(&mut regulatory_fut).is_pending());

            let region_request_fut = &mut context.regulatory_region_requests.next();
            let region_responder = assert_matches!(
                context.executor.run_until_stalled(region_request_fut),
                Poll::Ready(Some(Ok(
                    RegulatoryRegionWatcherRequest::GetRegionUpdate{responder}))) => responder
            );
            region_responder.send(Some("US")).expect("failed to send region response");
            assert!(context.executor.run_until_stalled(&mut regulatory_fut).is_pending());

            let iface_manager_fut = context.iface_manager.lock();
            let mut iface_manager_fut = pin!(iface_manager_fut);
            match context.executor.run_until_stalled(&mut iface_manager_fut) {
                Poll::Ready(iface_manager) => {
                    assert_eq!(iface_manager.country_code, Some("US".parse().unwrap()))
                }
                Poll::Pending => panic!("Expected to be able to lock the IfaceManager."),
            }
        }

        assert!(context.executor.run_until_stalled(&mut regulatory_fut).is_pending());
        assert_matches!(
            &context.executor.run_until_stalled(&mut context.regulatory_receiver),
            Poll::Ready(Ok(()))
        );

        // Receive second `RegulatoryRegionWatcher` update, and propagate it to `IfaceManager`.
        {
            assert!(context.executor.run_until_stalled(&mut regulatory_fut).is_pending());

            let region_request_fut = &mut context.regulatory_region_requests.next();
            let region_responder = assert_matches!(
                context.executor.run_until_stalled(region_request_fut),
                Poll::Ready(Some(Ok(
                    RegulatoryRegionWatcherRequest::GetRegionUpdate{responder}))) => responder
            );
            region_responder.send(Some("CA")).expect("failed to send region response");
            assert!(context.executor.run_until_stalled(&mut regulatory_fut).is_pending());

            let iface_manager_fut = context.iface_manager.lock();
            let mut iface_manager_fut = pin!(iface_manager_fut);
            match context.executor.run_until_stalled(&mut iface_manager_fut) {
                Poll::Ready(iface_manager) => {
                    assert_eq!(iface_manager.country_code, Some("CA".parse().unwrap()))
                }
                Poll::Pending => panic!("Expected to be able to lock the IfaceManager."),
            };
        }
    }

    struct StubIfaceManager<S: Stream<Item = Result<(), Error>> + Unpin> {
        country_code: Option<client_types::CountryCode>,
        set_country_response_stream: S,
    }

    /// A default StubIfaceManager
    /// * immediately returns Ok() in response to stop_client_connections(), and
    /// * immediately returns Ok() in response to start_client_connections()
    fn make_default_stub_iface_manager()
    -> StubIfaceManager<impl Stream<Item = Result<(), Error>> + Unpin> {
        StubIfaceManager {
            country_code: None,
            set_country_response_stream: stream::unfold((), |_| async { Some((Ok(()), ())) })
                .boxed(),
        }
    }

    #[async_trait(?Send)]
    impl<S: Stream<Item = Result<(), Error>> + Unpin> IfaceManagerApi for StubIfaceManager<S> {
        async fn disconnect(
            &mut self,
            _network_id: client_types::NetworkIdentifier,
            _reason: client_types::DisconnectReason,
        ) -> Result<(), Error> {
            unimplemented!();
        }

        async fn connect(&mut self, _connect_req: ConnectAttemptRequest) -> Result<(), Error> {
            unimplemented!();
        }

        async fn record_idle_client(&mut self, _iface_id: u16) -> Result<(), Error> {
            unimplemented!();
        }

        async fn has_idle_client(&mut self) -> Result<bool, Error> {
            unimplemented!();
        }

        async fn handle_added_iface(&mut self, _iface_id: u16) -> Result<(), Error> {
            unimplemented!();
        }

        async fn handle_removed_iface(&mut self, _iface_id: u16) -> Result<(), Error> {
            unimplemented!();
        }

        async fn get_sme_proxy_for_scan(&mut self) -> Result<SmeForScan, Error> {
            unimplemented!()
        }

        async fn stop_client_connections(
            &mut self,
            _reason: client_types::DisconnectReason,
        ) -> Result<(), Error> {
            unimplemented!()
        }

        async fn start_client_connections(&mut self) -> Result<(), Error> {
            unimplemented!()
        }

        async fn start_ap(
            &mut self,
            _config: ap_fsm::ApConfig,
        ) -> Result<oneshot::Receiver<()>, Error> {
            unimplemented!();
        }

        async fn stop_ap(
            &mut self,
            _ssid: ap_types::Ssid,
            _password: Vec<u8>,
        ) -> Result<(), Error> {
            unimplemented!();
        }

        async fn stop_all_aps(&mut self) -> Result<(), Error> {
            unimplemented!()
        }

        async fn set_country(
            &mut self,
            country_code: Option<client_types::CountryCode>,
        ) -> Result<(), Error> {
            self.country_code = country_code;
            self.set_country_response_stream
                .next()
                .await
                .expect("internal error: failed to receive fake response from test case")
        }
    }
}

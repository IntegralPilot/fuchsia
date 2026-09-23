// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Turn on additional lints that could lead to unexpected crashes in production code
#![warn(clippy::indexing_slicing)]
#![cfg_attr(test, allow(clippy::indexing_slicing))]
#![warn(clippy::unwrap_used)]
#![cfg_attr(test, allow(clippy::unwrap_used))]
#![warn(clippy::expect_used)]
#![cfg_attr(test, allow(clippy::expect_used))]
#![warn(clippy::unreachable)]
#![cfg_attr(test, allow(clippy::unreachable))]
#![warn(clippy::unimplemented)]
#![cfg_attr(test, allow(clippy::unimplemented))]

mod convert;
pub mod device;
mod logger;
mod mlme_main_loop;
mod wlan_fullmac_impl_ifc_request_handler;

use crate::convert::fullmac_to_mlme;
use crate::device::DeviceOps;
use crate::mlme_main_loop::create_mlme_main_loop;
use anyhow::bail;
use fidl_fuchsia_wlan_common as fidl_common;
use fidl_fuchsia_wlan_internal as fidl_internal;
use fidl_fuchsia_wlan_mlme as fidl_mlme;
use fidl_fuchsia_wlan_sme as fidl_sme;
use fuchsia_async as fasync;
use fuchsia_inspect::Inspector;
use futures::StreamExt;
use futures::channel::{mpsc, oneshot};
use futures::future::BoxFuture;
use log::{error, info, warn};
use wlan_common::sink::UnboundedSink;
use wlan_ffi_transport::completers::Completer;
use wlan_sme::serve::create_sme;

#[derive(thiserror::Error, Debug)]
pub enum FullmacMlmeError {
    #[error("device.start failed: {0}")]
    DeviceStartFailed(zx::Status),
    #[error("Failed to get usme bootstrap stream: {0}")]
    FailedToGetUsmeBootstrapStream(fidl::Error),
    #[error("USME bootstrap stream failed: {0}")]
    UsmeBootstrapStreamFailed(fidl::Error),
    #[error("USME bootstrap stream terminated")]
    UsmeBootstrapStreamTerminated,
    #[error("Failed to duplicate inspect VMO")]
    FailedToDuplicateInspectVmo,
    #[error("Failed to respond to usme bootstrap request: {0}")]
    FailedToRespondToUsmeBootstrapRequest(fidl::Error),
    #[error("Failed to get generic SME stream: {0}")]
    FailedToGetGenericSmeStream(fidl::Error),
    #[error("Invalid MAC implementation type: {0:?}")]
    InvalidMacImplementationType(fidl_common::MacImplementationType),
    #[error("Invalid data plane type: {0:?}")]
    InvalidDataPlaneType(fidl_common::DataPlaneType),
    #[error("Failed to query vendor driver: {0:?}")]
    FailedToQueryVendorDriver(anyhow::Error),
    #[error("Failed to convert query from vendor driver: {0:?}")]
    FailedToConvertVendorDriverQuery(anyhow::Error),
    #[error("Failed to create sme: {0}")]
    FailedToCreateSme(anyhow::Error),
    #[error("Failed to create WlanFullmacImplIfcRequestStream: {0}")]
    FailedToCreateIfcRequestStream(fidl::Error),
}

#[derive(Debug)]
struct FullmacDriverEventSink(pub UnboundedSink<FullmacDriverEvent>);

#[derive(Debug)]
enum FullmacDriverEvent {
    Stop,
    OnScanResult { result: fidl_mlme::ScanResult },
    OnScanEnd { end: fidl_mlme::ScanEnd },
    OnScheduledScanMatchesAvailable { txn_id: u64 },
    OnScheduledScanStoppedByFirmware { txn_id: u64 },
    ConnectConf { resp: fidl_mlme::ConnectConfirm },
    RoamConf { conf: fidl_mlme::RoamConfirm },
    RoamStartInd { ind: fidl_mlme::RoamStartIndication },
    RoamResultInd { ind: fidl_mlme::RoamResultIndication },
    AuthInd { ind: fidl_mlme::AuthenticateIndication },
    DeauthConf { resp: fidl_mlme::DeauthenticateConfirm },
    DeauthInd { ind: fidl_mlme::DeauthenticateIndication },
    AssocInd { ind: fidl_mlme::AssociateIndication },
    DisassocConf { resp: fidl_mlme::DisassociateConfirm },
    DisassocInd { ind: fidl_mlme::DisassociateIndication },
    StartConf { resp: fidl_mlme::StartConfirm },
    StopConf { resp: fidl_mlme::StopConfirm },
    EapolConf { resp: fidl_mlme::EapolConfirm },
    OnChannelSwitch { resp: fidl_internal::ChannelSwitchInfo },
    SignalReport { ind: fidl_internal::SignalReportIndication },
    EapolInd { ind: fidl_mlme::EapolIndication },
    OnPmkAvailable { info: fidl_mlme::PmkInfo },
    SaeHandshakeInd { ind: fidl_mlme::SaeHandshakeIndication },
    SaeFrameRx { frame: fidl_mlme::SaeFrame },
    OnWmmStatusResp { status: i32, resp: fidl_internal::WmmStatusResponse },
}

enum DriverState {
    Running,
    Stopping,
}

pub struct FullmacMlmeHandle {
    driver_event_sender: mpsc::UnboundedSender<FullmacDriverEvent>,
    mlme_loop_join_handle: Option<std::thread::JoinHandle<()>>,
    stop_requested: bool,
}

impl FullmacMlmeHandle {
    pub fn request_stop(&mut self) {
        info!("Requesting MLME stop...");
        if let Err(e) = self.driver_event_sender.unbounded_send(FullmacDriverEvent::Stop) {
            error!("Cannot signal MLME event loop thread: {}", e);
        } else {
            // Only set this to true if sending the Stop event succeeded. Most likely, this means
            // that the receiver was dropped and the MLME thread has already exited. But just in
            // case sending can result in a different error, this will let us retry sending the
            // Stop event in |Self::delete|. If it fails again, then we only emit another error
            // log.
            self.stop_requested = true;
        }
    }

    pub fn delete(mut self) {
        if !self.stop_requested {
            warn!("Called delete on FullmacMlmeHandle before calling stop.");
            self.request_stop()
        }

        match self.mlme_loop_join_handle.take() {
            Some(join_handle) => {
                if let Err(e) = join_handle.join() {
                    error!("MLME event loop thread panicked: {:?}", e);
                }
            }
            None => warn!("Called stop on already stopped MLME"),
        }
        info!("MLME main loop thread has shutdown.");
    }
}

const INSPECT_VMO_SIZE_BYTES: usize = 1000 * 1024;

/// Starts and serves the FullMAC MLME on a separate thread.
///
/// This will block until the FullMAC MLME has been initialized. MLME is considered "initialized"
/// after it bootstraps USME, queries the vendor driver for supported hardware features, and
/// creates the SME and MLME main loop futures. See the `start` function in this file for details.
///
/// Returns a handle to MLME on success, and an error if MLME failed to initialize.
pub async fn start_and_serve_on_separate_thread<F, D: DeviceOps + Send + 'static>(
    device: D,
    shutdown_completer: Completer<F>,
) -> anyhow::Result<FullmacMlmeHandle>
where
    F: FnOnce(zx::sys::zx_status_t) + 'static,
{
    logger::init();

    let (driver_event_sender, driver_event_stream) = mpsc::unbounded();
    let driver_event_sender_clone = driver_event_sender.clone();
    let inspector =
        Inspector::new(fuchsia_inspect::InspectorConfig::default().size(INSPECT_VMO_SIZE_BYTES));

    let (startup_sender, startup_receiver) = oneshot::channel();
    let mlme_loop_join_handle = std::thread::spawn(move || {
        // NOTE: Until MLME can be made async, MLME needs two threads to be able to
        // send requests to the vendor driver and receive requests from the vendor driver
        // simultaneously.
        let mut executor = fasync::SendExecutorBuilder::new().num_threads(2).build();

        info!("Starting WLAN MLME main loop");
        let future = start_and_serve(
            device,
            driver_event_sender_clone,
            driver_event_stream,
            inspector,
            startup_sender,
        );
        let result = executor.run(future);
        shutdown_completer.reply(result);
    });

    match startup_receiver.await {
        Ok(Ok(())) => (),
        Ok(Err(err)) => bail!(
            "MLME failed to start with error {}. MLME main loop returned {:?}.",
            err,
            mlme_loop_join_handle.join()
        ),
        Err(oneshot::Canceled) => bail!(
            "MLME thread dropped startup_sender. MLME main loop returned {:?}",
            mlme_loop_join_handle.join()
        ),
    };

    Ok(FullmacMlmeHandle {
        driver_event_sender,
        mlme_loop_join_handle: Some(mlme_loop_join_handle),
        stop_requested: false,
    })
}

/// Contains the initialized MLME main loop and SME futures.
///
/// Both futures do not hold references, so they should satisfy the 'static lifetime.
struct StartedDriver {
    mlme_main_loop_fut: BoxFuture<'static, anyhow::Result<()>>,
    sme_fut: BoxFuture<'static, anyhow::Result<()>>,
}

/// This initializes the MLME and SME, then on successful initialization runs the MLME main loop
/// future and SME futures concurrently until completion.
///
/// Notifies when startup is complete through |startup_sender|.
///
/// If initialization fails, then this exits immediately.
///
/// # Panics
///
/// This panics if sending over |startup_sender| returns an error. This probably means the
/// thread that owns |startup_sender| already exited to an error/panic.
async fn start_and_serve<D: DeviceOps + Send + 'static>(
    device: D,
    driver_event_sender: mpsc::UnboundedSender<FullmacDriverEvent>,
    driver_event_stream: mpsc::UnboundedReceiver<FullmacDriverEvent>,
    inspector: Inspector,
    startup_sender: oneshot::Sender<Result<(), FullmacMlmeError>>,
) -> Result<(), zx::Status> {
    let StartedDriver { mlme_main_loop_fut, sme_fut } =
        match start(device, driver_event_stream, driver_event_sender, inspector).await {
            Ok(initialized_mlme) => {
                if let Err(e) = startup_sender.send(Ok(())) {
                    error!("Failed to send startup response: {:?}", e);
                    return Err(zx::Status::INTERNAL);
                }
                initialized_mlme
            }
            Err(e) => {
                if let Err(e) = startup_sender.send(Err(e)) {
                    error!("Failed to send startup response: {:?}", e);
                }
                return Err(zx::Status::INTERNAL);
            }
        };

    match futures::try_join!(mlme_main_loop_fut, sme_fut) {
        Ok(_) => {
            info!("MLME and/or SME event loop exited gracefully");
            Ok(())
        }
        Err(e) => {
            error!("MLME and/or SME event loop exited with error: {:?}", e);
            Err(zx::Status::INTERNAL)
        }
    }
}

/// Starts the MLME and SME.
///
/// This:
/// - Handles the channel exchange over WlanFullmacImpl::Start().
/// - Retrieves the generic SME channel over the USME bootstrap channel.
/// - Creates the SME future.
/// - Creates the MLME main loop future.
///
/// On success, returns a `StartedDriver` that contains the MLME main loop and SME futures.
async fn start<D: DeviceOps + Send + 'static>(
    mut device: D,
    driver_event_stream: mpsc::UnboundedReceiver<FullmacDriverEvent>,
    driver_event_sender: mpsc::UnboundedSender<FullmacDriverEvent>,
    inspector: Inspector,
) -> Result<StartedDriver, FullmacMlmeError> {
    let (fullmac_ifc_client_end, fullmac_ifc_request_stream) =
        fidl::endpoints::create_request_stream();

    let usme_bootstrap_protocol_channel =
        device.init(fullmac_ifc_client_end).map_err(FullmacMlmeError::DeviceStartFailed)?;

    let server = fidl::endpoints::ServerEnd::<fidl_sme::UsmeBootstrapMarker>::new(
        usme_bootstrap_protocol_channel,
    );

    let mut usme_bootstrap_stream = server.into_stream();

    let fidl_sme::UsmeBootstrapRequest::Start {
        generic_sme_server,
        legacy_privacy_support,
        responder,
        ..
    } = usme_bootstrap_stream
        .next()
        .await
        .ok_or(FullmacMlmeError::UsmeBootstrapStreamTerminated)?
        .map_err(FullmacMlmeError::UsmeBootstrapStreamFailed)?;

    let inspect_vmo =
        inspector.duplicate_vmo().ok_or(FullmacMlmeError::FailedToDuplicateInspectVmo)?;

    responder.send(inspect_vmo).map_err(FullmacMlmeError::FailedToRespondToUsmeBootstrapRequest)?;

    let generic_sme_stream = generic_sme_server.into_stream();

    // Create SME
    let cfg = wlan_sme::Config {
        wep_supported: legacy_privacy_support.wep_supported,
        wpa1_supported: legacy_privacy_support.wpa1_supported,
    };

    let (mlme_event_sender, mlme_event_receiver) = mpsc::unbounded();
    let mlme_event_sink = UnboundedSink::new(mlme_event_sender);

    let device_info =
        device.query_device_info().map_err(FullmacMlmeError::FailedToQueryVendorDriver)?;
    let device_info = fullmac_to_mlme::convert_device_info(device_info)
        .map_err(FullmacMlmeError::FailedToConvertVendorDriverQuery)?;

    let security_support =
        device.query_security_support().map_err(FullmacMlmeError::FailedToQueryVendorDriver)?;

    let spectrum_management_support = device
        .query_spectrum_management_support()
        .map_err(FullmacMlmeError::FailedToQueryVendorDriver)?;

    let (mlme_request_stream, sme_fut) = create_sme(
        cfg.into(),
        mlme_event_receiver,
        &device_info,
        security_support,
        spectrum_management_support,
        inspector,
        generic_sme_stream,
    )
    .map_err(FullmacMlmeError::FailedToCreateSme)?;

    let driver_event_sink = FullmacDriverEventSink(UnboundedSink::new(driver_event_sender));
    let mlme_main_loop_fut = create_mlme_main_loop(
        device,
        mlme_request_stream,
        mlme_event_sink,
        driver_event_stream,
        driver_event_sink,
        fullmac_ifc_request_stream,
    );

    Ok(StartedDriver { mlme_main_loop_fut, sme_fut })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::test_utils::{DriverCall, FakeFullmacDevice, FakeFullmacDeviceMocks};
    use assert_matches::assert_matches;
    use fuchsia_async as fasync;
    use fuchsia_sync::Mutex;
    use futures::Future;
    use futures::task::Poll;
    use std::pin::Pin;
    use std::sync::Arc;

    #[test]
    fn test_happy_path() {
        let (mut h, mut test_fut) = TestHelper::set_up();
        assert_matches!(h.exec.run_until_stalled(&mut test_fut), Poll::Pending);

        let startup_result =
            assert_matches!(h.startup_receiver.try_recv(), Ok(Some(result)) => result);
        assert_matches!(startup_result, Ok(()));

        let (client_sme_proxy, client_sme_server) =
            fidl::endpoints::create_proxy::<fidl_sme::ClientSmeMarker>();

        let mut client_sme_response_fut =
            h.generic_sme_proxy.as_ref().unwrap().get_client_sme(client_sme_server);
        assert_matches!(h.exec.run_until_stalled(&mut client_sme_response_fut), Poll::Pending);
        assert_matches!(h.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        assert_matches!(
            h.exec.run_until_stalled(&mut client_sme_response_fut),
            Poll::Ready(Ok(Ok(())))
        );

        let mut status_fut = client_sme_proxy.status();
        assert_matches!(h.exec.run_until_stalled(&mut status_fut), Poll::Pending);
        assert_matches!(h.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let status_result = assert_matches!(h.exec.run_until_stalled(&mut status_fut), Poll::Ready(result) => result);
        assert_matches!(status_result, Ok(fidl_sme::ClientStatusResponse::Idle(_)));
    }

    #[test]
    fn test_mlme_exits_due_to_driver_event() {
        let (mut h, mut test_fut) = TestHelper::set_up();
        assert_matches!(h.exec.run_until_stalled(&mut test_fut), Poll::Pending);

        let startup_result =
            assert_matches!(h.startup_receiver.try_recv(), Ok(Some(result)) => result);
        assert_matches!(startup_result, Ok(()));

        h.driver_event_sender
            .unbounded_send(FullmacDriverEvent::Stop)
            .expect("expect sending driver Stop event to succeed");
        assert_matches!(h.exec.run_until_stalled(&mut test_fut), Poll::Ready(Ok(())));
    }

    #[test]
    fn test_mlme_startup_fails_due_to_device_start() {
        let (mut h, mut test_fut) = TestHelper::set_up();
        h.fake_device.lock().start_fn_status_mock = Some(zx::sys::ZX_ERR_BAD_STATE);
        assert_matches!(
            h.exec.run_until_stalled(&mut test_fut),
            Poll::Ready(Err(zx::Status::INTERNAL))
        );

        let startup_result =
            assert_matches!(h.startup_receiver.try_recv(), Ok(Some(result)) => result);
        assert_matches!(startup_result, Err(FullmacMlmeError::DeviceStartFailed(_)))
    }

    #[test]
    fn test_mlme_startup_fails_due_to_usme_bootstrap_terminated() {
        let bootstrap = false;
        let (mut h, mut test_fut) = TestHelper::set_up_with_usme_bootstrap(bootstrap);
        h.usme_bootstrap_proxy.take();
        assert_matches!(
            h.exec.run_until_stalled(&mut test_fut),
            Poll::Ready(Err(zx::Status::INTERNAL))
        );

        let startup_result =
            assert_matches!(h.startup_receiver.try_recv(), Ok(Some(result)) => result);
        assert_matches!(startup_result, Err(FullmacMlmeError::UsmeBootstrapStreamTerminated));
    }

    #[test]
    fn test_mlme_startup_fails_due_to_query_device_info_error() {
        let (mut h, mut test_fut) = TestHelper::set_up();
        h.fake_device.lock().query_device_info_mock = None;
        assert_matches!(
            h.exec.run_until_stalled(&mut test_fut),
            Poll::Ready(Err(zx::Status::INTERNAL))
        );

        let startup_result =
            assert_matches!(h.startup_receiver.try_recv(), Ok(Some(result)) => result);
        assert_matches!(startup_result, Err(FullmacMlmeError::FailedToQueryVendorDriver(_)));
    }

    #[test]
    fn test_mlme_startup_fails_due_to_query_security_support_error() {
        let (mut h, mut test_fut) = TestHelper::set_up();
        h.fake_device.lock().query_security_support_mock = None;
        assert_matches!(
            h.exec.run_until_stalled(&mut test_fut),
            Poll::Ready(Err(zx::Status::INTERNAL))
        );

        let startup_result =
            assert_matches!(h.startup_receiver.try_recv(), Ok(Some(result)) => result);
        assert_matches!(startup_result, Err(FullmacMlmeError::FailedToQueryVendorDriver(_)));
    }

    #[test]
    fn test_mlme_startup_fails_due_to_query_spectrum_management_support_error() {
        let (mut h, mut test_fut) = TestHelper::set_up();
        h.fake_device.lock().query_spectrum_management_support_mock = None;
        assert_matches!(
            h.exec.run_until_stalled(&mut test_fut),
            Poll::Ready(Err(zx::Status::INTERNAL))
        );

        let startup_result =
            assert_matches!(h.startup_receiver.try_recv(), Ok(Some(result)) => result);
        assert_matches!(startup_result, Err(FullmacMlmeError::FailedToQueryVendorDriver(_)));
    }

    #[test]
    fn test_mlme_startup_fails_due_to_failure_to_convert_query_device_info() {
        let (mut h, mut test_fut) = TestHelper::set_up();
        h.fake_device.lock().query_device_info_mock.as_mut().unwrap().role = None;
        assert_matches!(
            h.exec.run_until_stalled(&mut test_fut),
            Poll::Ready(Err(zx::Status::INTERNAL))
        );

        let startup_result =
            assert_matches!(h.startup_receiver.try_recv(), Ok(Some(result)) => result);
        assert_matches!(startup_result, Err(FullmacMlmeError::FailedToConvertVendorDriverQuery(_)));
    }

    #[test]
    fn test_mlme_startup_exits_due_to_sme_channel_closure() {
        let (mut h, mut test_fut) = TestHelper::set_up();
        assert_matches!(h.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        assert_matches!(h.startup_receiver.try_recv(), Ok(Some(Ok(()))));

        std::mem::drop(h.generic_sme_proxy.take());
        assert_matches!(
            h.exec.run_until_stalled(&mut test_fut),
            Poll::Ready(Err(zx::Status::INTERNAL))
        );
    }

    #[test]
    fn test_mlme_startup_exits_due_to_wlan_fullmac_impl_ifc_channel_closure() {
        let (mut h, mut test_fut) = TestHelper::set_up();
        assert_matches!(h.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        assert_matches!(h.startup_receiver.try_recv(), Ok(Some(Ok(()))));

        std::mem::drop(h.fake_device.lock().fullmac_ifc_client_end.take());
        assert_matches!(
            h.exec.run_until_stalled(&mut test_fut),
            Poll::Ready(Err(zx::Status::INTERNAL))
        );
    }

    struct TestHelper {
        fake_device: Arc<Mutex<FakeFullmacDeviceMocks>>,
        driver_event_sender: mpsc::UnboundedSender<FullmacDriverEvent>,
        usme_bootstrap_proxy: Option<fidl_sme::UsmeBootstrapProxy>,
        _usme_bootstrap_result: Option<fidl::client::QueryResponseFut<zx::Vmo>>,
        generic_sme_proxy: Option<fidl_sme::GenericSmeProxy>,
        startup_receiver: oneshot::Receiver<Result<(), FullmacMlmeError>>,
        _driver_calls: mpsc::UnboundedReceiver<DriverCall>,
        exec: fasync::TestExecutor,
    }

    impl TestHelper {
        pub fn set_up() -> (Self, Pin<Box<impl Future<Output = Result<(), zx::Status>>>>) {
            let bootstrap = true;
            Self::set_up_with_usme_bootstrap(bootstrap)
        }

        pub fn set_up_with_usme_bootstrap(
            bootstrap: bool,
        ) -> (Self, Pin<Box<impl Future<Output = Result<(), zx::Status>>>>) {
            let exec = fasync::TestExecutor::new();

            let (mut fake_device, _driver_calls) = FakeFullmacDevice::new();
            let usme_bootstrap_proxy =
                fake_device.usme_bootstrap_client_end.take().unwrap().into_proxy();

            let (generic_sme_proxy, generic_sme_server_end) =
                fidl::endpoints::create_proxy::<fidl_sme::GenericSmeMarker>();
            let usme_bootstrap_result = if bootstrap {
                Some(usme_bootstrap_proxy.start(
                    generic_sme_server_end,
                    &fidl_sme::LegacyPrivacySupport { wep_supported: false, wpa1_supported: false },
                ))
            } else {
                None
            };

            let (driver_event_sender, driver_event_stream) = mpsc::unbounded();
            let inspector = Inspector::default();
            let (startup_sender, startup_receiver) = oneshot::channel();

            let mocks = fake_device.mocks.clone();

            let test_fut = Box::pin(start_and_serve(
                fake_device,
                driver_event_sender.clone(),
                driver_event_stream,
                inspector,
                startup_sender,
            ));

            let test_helper = TestHelper {
                fake_device: mocks,
                driver_event_sender,
                usme_bootstrap_proxy: Some(usme_bootstrap_proxy),
                _usme_bootstrap_result: usme_bootstrap_result,
                generic_sme_proxy: Some(generic_sme_proxy),
                startup_receiver,
                _driver_calls,
                exec,
            };
            (test_helper, test_fut)
        }
    }
}

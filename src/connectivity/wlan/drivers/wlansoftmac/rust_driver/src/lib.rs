// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use fidl::endpoints::{ProtocolMarker, Proxy};
use fidl_fuchsia_wlan_common as fidl_common;
use fidl_fuchsia_wlan_sme as fidl_sme;
use fidl_fuchsia_wlan_softmac as fidl_softmac;
use fuchsia_async::{MonotonicDuration, Task};
use fuchsia_inspect::Inspector;
use futures::channel::mpsc;
use futures::channel::oneshot::{self, Canceled};
use futures::{Future, FutureExt, StreamExt};
use log::{error, info, warn};
use std::pin::Pin;
use wlan_ffi_transport::completers::Completer;
use wlan_ffi_transport::{EthernetTx, WlanRx};
use wlan_fidl_ext::{ResponderExt, SendResultExt, WithName};
use wlan_mlme::device::DeviceOps;
use wlan_mlme::{DriverEvent, DriverEventSink};
use wlan_sme::serve::create_sme;
use wlan_trace as wtrace;

const INSPECT_VMO_SIZE_BYTES: usize = 1000 * 1024;

/// Run the bridged wlansoftmac driver composed of the following servers:
///
///   - WlanSoftmacIfcBridge server
///   - MLME server
///   - SME server
///
/// The WlanSoftmacIfcBridge server future executes on a parallel thread because otherwise
/// synchronous calls from the MLME server into the vendor driver could deadlock if the vendor
/// driver calls a WlanSoftmacIfcBridge method before returning from a synchronous call. For
/// example, when the MLME server synchronously calls WlanSoftmac.StartActiveScan(), the vendor
/// driver may call WlanSoftmacIfc.NotifyScanComplete() before returning from
/// WlanSoftmac.StartActiveScan(). This can occur when the scan request results in immediate
/// cancellation despite the request having valid arguments.
///
/// This function calls `start_completer()` when MLME initialization completes successfully, and
/// will return in one of four cases:
///
///   - An error occurred during initialization.
///   - An error occurred while running.
///   - An error occurred during shutdown.
///   - Shutdown completed successfully.
///
/// If an error occurs during the bridge driver's initialization, `start_completer()` will not be
/// called.
pub async fn start_and_serve<F, D: DeviceOps + 'static>(
    start_completer: Completer<F>,
    device: D,
) -> Result<(), zx::Status>
where
    F: FnOnce(zx::sys::zx_status_t) + 'static,
{
    wtrace::duration!("rust_driver::start_and_serve");
    let (driver_event_sink, driver_event_stream) = DriverEventSink::new();

    let (mlme_init_sender, mlme_init_receiver) = oneshot::channel();
    let StartedDriver { softmac_ifc_bridge_request_stream, mlme, sme } =
        match start(mlme_init_sender, driver_event_sink.clone(), driver_event_stream, device).await
        {
            Err(status) => {
                start_completer.reply(Err(status));
                return Err(status);
            }
            Ok(x) => x,
        };

    start_completer.reply(Ok(()));

    serve(mlme_init_receiver, driver_event_sink, softmac_ifc_bridge_request_stream, mlme, sme).await
}

struct StartedDriver<Mlme, Sme> {
    pub softmac_ifc_bridge_request_stream: fidl_softmac::WlanSoftmacIfcBridgeRequestStream,
    pub mlme: Mlme,
    pub sme: Sme,
}

/// Start the bridged wlansoftmac driver by creating components to run two futures:
///
///   - MLME server
///   - SME server
///
/// This function will use the provided |device| to make various calls into the vendor driver
/// necessary to configure and create the components to run the futures.
async fn start<D: DeviceOps + 'static>(
    mlme_init_sender: oneshot::Sender<()>,
    driver_event_sink: DriverEventSink,
    driver_event_stream: mpsc::UnboundedReceiver<DriverEvent>,
    mut device: D,
) -> Result<
    StartedDriver<
        Pin<Box<dyn Future<Output = Result<(), Error>>>>,
        Pin<Box<impl Future<Output = Result<(), Error>>>>,
    >,
    zx::Status,
> {
    wtrace::duration!("rust_driver::start");

    let (softmac_ifc_bridge_proxy, softmac_ifc_bridge_request_stream) =
        fidl::endpoints::create_proxy_and_stream::<fidl_softmac::WlanSoftmacIfcBridgeMarker>();

    // Bootstrap USME
    let BootstrappedGenericSme { generic_sme_request_stream, legacy_privacy_support, inspector } =
        bootstrap_generic_sme(&mut device, driver_event_sink, softmac_ifc_bridge_proxy).await?;

    info!("Querying device information...");

    // Make a series of queries to gather device information from the vendor driver.
    let softmac_info = device.wlan_softmac_query_response().await?;
    let sta_addr = softmac_info.sta_addr;
    let device_info = match wlan_mlme::mlme_device_info_from_softmac(softmac_info) {
        Ok(info) => info,
        Err(e) => {
            error!("Failed to get MLME device info: {}", e);
            return Err(zx::Status::INTERNAL);
        }
    };

    let security_support = device.security_support().await?;
    let spectrum_management_support = device.spectrum_management_support().await?;

    info!("Querying complete!");

    let config = wlan_sme::Config {
        wep_supported: legacy_privacy_support.wep_supported,
        wpa1_supported: legacy_privacy_support.wpa1_supported,
    };

    // TODO(https://fxbug.dev/42077094): The MLME event stream should be moved out of DeviceOps
    // entirely.
    let mlme_event_stream = match device.take_mlme_event_stream() {
        Some(mlme_event_stream) => mlme_event_stream,
        None => {
            error!("Failed to take MLME event stream.");
            return Err(zx::Status::INTERNAL);
        }
    };

    // Create an SME future to serve
    let (mlme_request_stream, sme) = match create_sme(
        config,
        mlme_event_stream,
        &device_info,
        security_support,
        spectrum_management_support,
        inspector,
        generic_sme_request_stream,
    ) {
        Ok((mlme_request_stream, sme)) => (mlme_request_stream, sme),
        Err(e) => {
            error!("Failed to create sme: {}", e);
            return Err(zx::Status::INTERNAL);
        }
    };

    // Create an MLME future to serve
    let mlme: Pin<Box<dyn Future<Output = Result<(), Error>>>> = match device_info.role {
        fidl_common::WlanMacRole::Client => {
            info!("Running wlansoftmac with client role");
            let config = wlan_mlme::client::ClientConfig {
                ensure_on_channel_time: MonotonicDuration::from_millis(500).into_nanos(),
            };
            Box::pin(wlan_mlme::mlme_main_loop::<wlan_mlme::client::ClientMlme<D>>(
                mlme_init_sender,
                config,
                device,
                mlme_request_stream,
                driver_event_stream,
            ))
        }
        fidl_common::WlanMacRole::Ap => {
            info!("Running wlansoftmac with AP role");
            let sta_addr = match sta_addr {
                Some(sta_addr) => sta_addr,
                None => {
                    error!("Driver provided no STA address.");
                    return Err(zx::Status::INTERNAL);
                }
            };
            let config = ieee80211::Bssid::from(sta_addr);
            Box::pin(wlan_mlme::mlme_main_loop::<wlan_mlme::ap::Ap<D>>(
                mlme_init_sender,
                config,
                device,
                mlme_request_stream,
                driver_event_stream,
            ))
        }
        unsupported => {
            error!("Unsupported mac role: {:?}", unsupported);
            return Err(zx::Status::INTERNAL);
        }
    };

    Ok(StartedDriver { softmac_ifc_bridge_request_stream, mlme, sme })
}

/// Await on futures hosting the following three servers:
///
///   - WlanSoftmacIfcBridge server
///   - MLME server
///   - SME server
///
/// The WlanSoftmacIfcBridge server runs on a parallel thread but will be shut down before this
/// function returns. This is true even if this function exits with an error.
///
/// Upon receiving a DriverEvent::Stop, the MLME server will shut down first. Then this function
/// will await the completion of WlanSoftmacIfcBridge server and SME server. Both will shut down as
/// a consequence of MLME server shut down.
async fn serve(
    mlme_init_receiver: oneshot::Receiver<()>,
    driver_event_sink: DriverEventSink,
    softmac_ifc_bridge_request_stream: fidl_softmac::WlanSoftmacIfcBridgeRequestStream,
    mlme: Pin<Box<dyn Future<Output = Result<(), Error>>>>,
    sme: Pin<Box<impl Future<Output = Result<(), Error>>>>,
) -> Result<(), zx::Status> {
    wtrace::duration!("rust_driver::serve");

    // Create a oneshot::channel to signal to this executor when WlanSoftmacIfcBridge
    // server exits.
    let (bridge_exit_sender, bridge_exit_receiver) = oneshot::channel();
    // Spawn a Task to host the WlanSoftmacIfcBridge server.
    let bridge = Task::local(async move {
        let _: Result<(), ()> = bridge_exit_sender
            .send(
                serve_wlan_softmac_ifc_bridge(driver_event_sink, softmac_ifc_bridge_request_stream)
                    .await,
            )
            .map_err(|result| {
                error!("Failed to send serve_wlan_softmac_ifc_bridge() result: {:?}", result)
            });
    });

    let mut mlme = mlme.fuse();
    let mut sme = sme.fuse();

    // oneshot::Receiver implements FusedFuture incorrectly, so we must call .fuse()
    // to get the right behavior in the select!().
    //
    // See https://github.com/rust-lang/futures-rs/issues/2455 for more details.
    let mut bridge_exit_receiver = bridge_exit_receiver.fuse();
    let mut mlme_init_receiver = mlme_init_receiver.fuse();

    info!("Starting MLME and waiting on MLME initialization to complete...");
    // Run the MLME server and wait for the MLME to signal initialization completion.
    //
    // The order of the futures in this select is not arbitrary. During initialization, there is
    // an edge case where MLME could be stopped before initialization completes. By polling
    // the MLME future first, we can unit test handling this edge case by completing the MLME
    // future and initialization, in that order, and then polling the future returned by
    // serve() (i.e., this function).
    {
        wtrace::duration!("initialize MLME");
        futures::select! {
            mlme_result = mlme => {
                match mlme_result {
                    Err(e) => {
                        error!("MLME future completed with error during initialization: {:?}", e);
                        std::mem::drop(bridge);
                        return Err(zx::Status::INTERNAL);
                    }
                    Ok(()) => {

                        // It's possible MLME received a DriverEvent::Stop and returned after
                        // signaling initialization completed and before mlme_init_receiver being
                        // polled. If that's the case, then log a warning that SME never started and
                        // return Ok. Exiting the server in this way should be considered okay
                        // because MLME signaled initialization completed and exited successfully.
                        match mlme_init_receiver.now_or_never() {
                            None | Some(Err(Canceled)) => {
                                error!("MLME future completed before signaling initialization complete.");
                                std::mem::drop(bridge);
                                return Err(zx::Status::INTERNAL);
                            }
                            Some(Ok(())) => {
                                warn!("SME never started. MLME future completed successfully just after initialization.");
                                std::mem::drop(bridge);
                                return Ok(());
                            }
                        }
                    }
                }
            }
            init_result = mlme_init_receiver => {
                match init_result {
                    Ok(()) => (),
                    Err(e) => {
                        error!("MLME dropped the initialization signaler: {}", e);
                        std::mem::drop(bridge);
                        return Err(zx::Status::INTERNAL);
                    }
                }
            },
        }
    }

    info!("Starting SME and WlanSoftmacIfc servers...");

    // Run the SME and MLME servers.
    {
        wtrace::duration!("run MLME and SME");
        // This loop-select has two phases.
        //
        // In the first phase, all three futures are running. The first phase will break
        // the loop with an error if any of the following events occurs:
        //
        //   - SME future completes before MLME.
        //   - Any future completes with an error.
        //
        // If the bridge_exit_receiver completes successfully, the MLME and SME futures continue.
        // It's possible for bridge_exit_receiver to complete before MLME because
        // the bridge server exits upon receiving the StopBridgedDriver message while the MLME
        // future consumes the StopBridgedDriver message and responds asynchronously.
        //
        // The first phase ends successfully only if the MLME future completes successfully.
        //
        // The second phase runs the SME future and, if not complete, the
        // bridge_exit_receiver future. The second phase ends with an error if either the
        // SME future or bridge_exit_receiver future return an error.  Otherwise, the
        // second phase ends successfully.
        let mut mlme_future_complete = false;
        loop {
            futures::select! {
                mlme_result = mlme => {
                    match mlme_result {
                        Ok(()) => {
                            info!("MLME shut down gracefully.");
                            mlme_future_complete = true;
                        },
                        Err(e) => {
                            error!("MLME shut down with error: {}", e);
                            break Err(zx::Status::INTERNAL)
                        }
                    }
                }
                bridge_result = bridge_exit_receiver => {
                    // We expect the bridge to shut itself down immediately upon receiving a
                    // StopBridgedDriver message, so it's often the case that the bridge task
                    // will exit before MLME. When the bridge task completes first, both
                    // the `mlme` and `sme` futures should continue to run.
                    match bridge_result {
                        Err(Canceled) => {
                            error!("SoftmacIfcBridge result sender dropped unexpectedly.");
                            break Err(zx::Status::INTERNAL)
                        }
                        Ok(Err(e)) => {
                            error!("SoftmacIfcBridge server shut down with error: {}", e);
                            break Err(zx::Status::INTERNAL)
                        }
                        Ok(Ok(())) => info!("SoftmacIfcBridge server shut down gracefully"),
                    }
                }
                sme_result = sme => {
                    if mlme_future_complete {
                        match sme_result {
                            Err(e) => {
                                error!("SME shut down with error: {}", e);
                                break Err(zx::Status::INTERNAL)
                            }
                            Ok(()) => info!("SME shut down gracefully"),
                        }
                    } else {
                        error!("SME shut down before MLME: {:?}", sme_result);
                        break Err(zx::Status::INTERNAL)
                    }
                }
                complete => break Ok(())
            }
        }
    }
}

struct BootstrappedGenericSme {
    pub generic_sme_request_stream: fidl_sme::GenericSmeRequestStream,
    pub legacy_privacy_support: fidl_sme::LegacyPrivacySupport,
    pub inspector: Inspector,
}

/// Call WlanSoftmac.Start() to retrieve the server end of UsmeBootstrap channel and wait
/// for a UsmeBootstrap.Start() message to provide the server end of a GenericSme channel.
///
/// Any errors encountered in this function are fatal for the wlansoftmac driver. Failure to
/// bootstrap GenericSme request stream will result in a driver no other component can communicate
/// with.
async fn bootstrap_generic_sme<D: DeviceOps>(
    device: &mut D,
    driver_event_sink: DriverEventSink,
    softmac_ifc_bridge_proxy: fidl_softmac::WlanSoftmacIfcBridgeProxy,
) -> Result<BootstrappedGenericSme, zx::Status> {
    wtrace::duration!("rust_driver::bootstrap_generic_sme");
    info!("Bootstrapping GenericSme...");

    let ifc_bridge = softmac_ifc_bridge_proxy.into_client_end().map_err(|_| {
        error!(
            "Failed to convert {} into client end.",
            fidl_softmac::WlanSoftmacIfcBridgeMarker::DEBUG_NAME
        );
        zx::Status::INTERNAL
    })?;

    // Calling WlanSoftmac.Start() indicates to the vendor driver that this driver (wlansoftmac) is
    // ready to receive WlanSoftmacIfc messages. wlansoftmac will buffer all WlanSoftmacIfc messages
    // in an mpsc::UnboundedReceiver<DriverEvent> sink until the MLME server drains them.
    let usme_bootstrap_channel_via_iface_creation = match device
        .start(
            ifc_bridge,
            EthernetTx::new(Box::new(driver_event_sink.clone())),
            WlanRx::new(Box::new(driver_event_sink)),
        )
        .await
    {
        Ok(channel) => channel,
        Err(status) => {
            error!("Failed to receive a UsmeBootstrap handle: {}", status);
            return Err(status);
        }
    };
    info!("Bootstrap complete!");

    let server = fidl::endpoints::ServerEnd::<fidl_sme::UsmeBootstrapMarker>::new(
        usme_bootstrap_channel_via_iface_creation,
    );
    let mut usme_bootstrap_stream = server.into_stream();

    let (generic_sme_server, legacy_privacy_support, responder) =
        match usme_bootstrap_stream.next().await {
            Some(Ok(fidl_sme::UsmeBootstrapRequest::Start {
                generic_sme_server,
                legacy_privacy_support,
                responder,
                ..
            })) => (generic_sme_server, legacy_privacy_support, responder),
            Some(Err(e)) => {
                error!("Received an error on USME bootstrap request stream: {}", e);
                return Err(zx::Status::BAD_STATE);
            }
            None => {
                // This is always an error because the SME server should not drop
                // the USME client endpoint until MLME shut down first.
                error!("USME bootstrap stream terminated unexpectedly");
                return Err(zx::Status::BAD_STATE);
            }
        };

    let inspector =
        Inspector::new(fuchsia_inspect::InspectorConfig::default().size(INSPECT_VMO_SIZE_BYTES));

    let inspect_vmo = match inspector.duplicate_vmo() {
        Some(vmo) => vmo,
        None => {
            error!("Failed to duplicate inspect VMO");
            return Err(zx::Status::INTERNAL);
        }
    };
    if let Err(e) = responder.send(inspect_vmo).into() {
        error!("Failed to respond to UsmeBootstrap.Start(): {}", e);
        return Err(zx::Status::INTERNAL);
    }
    let generic_sme_request_stream = generic_sme_server.into_stream();

    Ok(BootstrappedGenericSme { generic_sme_request_stream, legacy_privacy_support, inspector })
}

async fn serve_wlan_softmac_ifc_bridge(
    driver_event_sink: DriverEventSink,
    mut softmac_ifc_bridge_request_stream: fidl_softmac::WlanSoftmacIfcBridgeRequestStream,
) -> Result<(), anyhow::Error> {
    loop {
        let request = match softmac_ifc_bridge_request_stream.next().await {
            Some(Ok(request)) => request,
            Some(Err(e)) => {
                return Err(format_err!("WlanSoftmacIfcBridge stream failed: {}", e));
            }
            None => {
                return Err(format_err!(
                    "WlanSoftmacIfcBridge stream terminated unexpectedly by client"
                ));
            }
        };
        match request {
            fidl_softmac::WlanSoftmacIfcBridgeRequest::ReportTxResult { tx_result, responder } => {
                let responder = driver_event_sink.unbounded_send_or_respond(
                    DriverEvent::TxResultReport { tx_result },
                    responder,
                    (),
                )?;
                responder.send().format_send_err_with_context("ReportTxResult")?;
            }
            fidl_softmac::WlanSoftmacIfcBridgeRequest::NotifyScanComplete {
                payload,
                responder,
            } => {
                let ((status, scan_id), responder) = responder.unpack_fields_or_respond((
                    payload.status.with_name("status"),
                    payload.scan_id.with_name("scan_id"),
                ))?;
                let status = zx::Status::ok(status);
                let responder = driver_event_sink.unbounded_send_or_respond(
                    DriverEvent::ScanComplete { status, scan_id },
                    responder,
                    (),
                )?;
                responder.send().format_send_err_with_context("NotifyScanComplete")?;
            }
            fidl_softmac::WlanSoftmacIfcBridgeRequest::StopBridgedDriver { responder } => {
                if let Err(e) = driver_event_sink.unbounded_send(DriverEvent::Stop { responder }) {
                    let error_string = e.to_string();
                    let event = e.into_inner();
                    let e = format_err!("Failed to queue {}: {}", event, error_string);
                    let DriverEvent::Stop { responder } = event else {
                        unreachable!();
                    };
                    responder.send().format_send_err_with_context("StopBridgedDriver")?;
                    return Err(e);
                }
                return Ok(());
            }
            fidl_softmac::WlanSoftmacIfcBridgeRequest::_UnknownMethod { .. } => {
                warn!("Received unknown request on WlanSoftmacIfcBridge channel");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::format_err;
    use assert_matches::assert_matches;
    use diagnostics_assertions::assert_data_tree;
    use fuchsia_async::TestExecutor;
    use fuchsia_inspect::InspectorConfig;
    use futures::stream::FuturesUnordered;
    use futures::task::Poll;
    use std::pin::pin;
    use test_case::test_case;
    use wlan_mlme::device::test_utils::{FakeDevice, FakeDeviceConfig};
    use zx::Vmo;

    struct BootstrapGenericSmeTestHarness {
        _softmac_ifc_bridge_request_stream: fidl_softmac::WlanSoftmacIfcBridgeRequestStream,
    }

    // We could implement BootstrapGenericSmeTestHarness::new() instead of a macro, but doing so requires
    // pinning the FakeDevice and WlanSoftmacIfcProtocol (and its associated DriverEventSink). While the
    // pinning itself is feasible, it leads to a complex harness implementation that outweighs the benefit
    // of using a harness to begin with.
    macro_rules! make_bootstrap_generic_sme_test_harness {
        (&mut $fake_device:ident, $driver_event_sink:ident $(,)?) => {{
            let (softmac_ifc_bridge_proxy, _softmac_ifc_bridge_request_stream) =
                fidl::endpoints::create_proxy_and_stream::<fidl_softmac::WlanSoftmacIfcBridgeMarker>();
            (
                Box::pin(bootstrap_generic_sme(
                    &mut $fake_device,
                    $driver_event_sink,
                    softmac_ifc_bridge_proxy,
                )),
                BootstrapGenericSmeTestHarness {
                    _softmac_ifc_bridge_request_stream,
                }
            )
        }};
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn bootstrap_generic_sme_fails_to_retrieve_usme_bootstrap_handle() {
        let (mut fake_device, _fake_device_state) = FakeDevice::new_with_config(
            FakeDeviceConfig::default().with_mock_start_result(Err(zx::Status::INTERRUPTED_RETRY)),
        )
        .await;
        let (driver_event_sink, _driver_event_stream) = DriverEventSink::new();

        let (mut bootstrap_generic_sme_fut, _harness) =
            make_bootstrap_generic_sme_test_harness!(&mut fake_device, driver_event_sink);
        match TestExecutor::poll_until_stalled(&mut bootstrap_generic_sme_fut).await {
            Poll::Ready(Err(zx::Status::INTERRUPTED_RETRY)) => (),
            Poll::Ready(Err(status)) => panic!("Failed with wrong status: {}", status),
            Poll::Ready(Ok(_)) => panic!("Succeeded unexpectedly"),
            Poll::Pending => panic!("bootstrap_generic_sme() unexpectedly stalled"),
        }
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn boostrap_generic_sme_fails_on_error_from_bootstrap_stream() {
        let (mut fake_device, fake_device_state) =
            FakeDevice::new_with_config(FakeDeviceConfig::default()).await;
        let (driver_event_sink, _driver_event_stream) = DriverEventSink::new();

        let (mut bootstrap_generic_sme_fut, _harness) =
            make_bootstrap_generic_sme_test_harness!(&mut fake_device, driver_event_sink);
        assert!(matches!(
            TestExecutor::poll_until_stalled(&mut bootstrap_generic_sme_fut).await,
            Poll::Pending
        ));

        // Write an invalid FIDL message to the USME bootstrap channel.
        let usme_bootstrap_channel =
            fake_device_state.lock().usme_bootstrap_client_end.take().unwrap().into_channel();
        usme_bootstrap_channel.write(&[], &mut []).unwrap();

        assert!(matches!(
            TestExecutor::poll_until_stalled(&mut bootstrap_generic_sme_fut).await,
            Poll::Ready(Err(zx::Status::BAD_STATE))
        ));
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn boostrap_generic_sme_fails_on_closed_bootstrap_stream() {
        let (mut fake_device, fake_device_state) =
            FakeDevice::new_with_config(FakeDeviceConfig::default()).await;
        let (driver_event_sink, _driver_event_stream) = DriverEventSink::new();

        let (mut bootstrap_generic_sme_fut, _harness) =
            make_bootstrap_generic_sme_test_harness!(&mut fake_device, driver_event_sink);
        assert!(matches!(
            TestExecutor::poll_until_stalled(&mut bootstrap_generic_sme_fut).await,
            Poll::Pending
        ));

        // Drop the client end of USME bootstrap channel.
        let _ = fake_device_state.lock().usme_bootstrap_client_end.take().unwrap();

        assert!(matches!(
            TestExecutor::poll_until_stalled(&mut bootstrap_generic_sme_fut).await,
            Poll::Ready(Err(zx::Status::BAD_STATE))
        ));
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn boostrap_generic_sme_succeeds() {
        let (mut fake_device, fake_device_state) =
            FakeDevice::new_with_config(FakeDeviceConfig::default()).await;
        let (driver_event_sink, _driver_event_stream) = DriverEventSink::new();

        let (mut bootstrap_generic_sme_fut, _harness) =
            make_bootstrap_generic_sme_test_harness!(&mut fake_device, driver_event_sink);
        assert!(matches!(
            TestExecutor::poll_until_stalled(&mut bootstrap_generic_sme_fut).await,
            Poll::Pending
        ));

        let usme_bootstrap_proxy =
            fake_device_state.lock().usme_bootstrap_client_end.take().unwrap().into_proxy();

        let sent_legacy_privacy_support =
            fidl_sme::LegacyPrivacySupport { wep_supported: false, wpa1_supported: false };
        let (generic_sme_proxy, generic_sme_server) =
            fidl::endpoints::create_proxy::<fidl_sme::GenericSmeMarker>();
        let inspect_vmo_fut =
            usme_bootstrap_proxy.start(generic_sme_server, &sent_legacy_privacy_support);
        let mut inspect_vmo_fut = pin!(inspect_vmo_fut);
        assert!(matches!(
            TestExecutor::poll_until_stalled(&mut inspect_vmo_fut).await,
            Poll::Pending
        ));

        let BootstrappedGenericSme {
            mut generic_sme_request_stream,
            legacy_privacy_support: received_legacy_privacy_support,
            inspector,
        } = match TestExecutor::poll_until_stalled(&mut bootstrap_generic_sme_fut).await {
            Poll::Pending => panic!("bootstrap_generic_sme_fut() did not complete!"),
            Poll::Ready(x) => x.unwrap(),
        };
        let inspect_vmo = match TestExecutor::poll_until_stalled(&mut inspect_vmo_fut).await {
            Poll::Pending => panic!("Failed to receive an inspect VMO."),
            Poll::Ready(x) => x.unwrap(),
        };

        // Send a GenericSme.Query() to check the generic_sme_proxy
        // and generic_sme_stream are connected.
        let query_fut = generic_sme_proxy.query();
        let mut query_fut = pin!(query_fut);
        assert!(matches!(TestExecutor::poll_until_stalled(&mut query_fut).await, Poll::Pending));
        let next_generic_sme_request_fut = generic_sme_request_stream.next();
        let mut next_generic_sme_request_fut = pin!(next_generic_sme_request_fut);
        assert!(matches!(
            TestExecutor::poll_until_stalled(&mut next_generic_sme_request_fut).await,
            Poll::Ready(Some(Ok(fidl_sme::GenericSmeRequest::Query { .. })))
        ));

        assert_eq!(received_legacy_privacy_support, sent_legacy_privacy_support);

        // Add a child node through the bootstrapped inspector and verify the node appears inspect_vmo.
        let returned_inspector = Inspector::new(InspectorConfig::default().vmo(inspect_vmo));
        let _a = inspector.root().create_child("a");
        assert_data_tree!(returned_inspector, root: {
            a: {},
        });
    }

    struct StartTestHarness {
        pub mlme_init_receiver: Pin<Box<oneshot::Receiver<()>>>,
        // TODO(https://fxbug.dev/335283785): Remove or explain unused code.
        #[allow(dead_code)]
        pub driver_event_sink: DriverEventSink,
    }

    impl StartTestHarness {
        fn new(
            fake_device: FakeDevice,
        ) -> (
            impl Future<
                Output = Result<
                    StartedDriver<
                        Pin<Box<dyn Future<Output = Result<(), Error>>>>,
                        Pin<Box<impl Future<Output = Result<(), Error>>>>,
                    >,
                    zx::Status,
                >,
            >,
            Self,
        ) {
            let (mlme_init_sender, mlme_init_receiver) = oneshot::channel();
            let (driver_event_sink, driver_event_stream) = DriverEventSink::new();

            (
                Box::pin(start(
                    mlme_init_sender,
                    driver_event_sink.clone(),
                    driver_event_stream,
                    fake_device,
                )),
                Self { mlme_init_receiver: Box::pin(mlme_init_receiver), driver_event_sink },
            )
        }
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn start_fails_on_bad_bootstrap() {
        let (fake_device, _fake_device_state) = FakeDevice::new_with_config(
            FakeDeviceConfig::default().with_mock_start_result(Err(zx::Status::INTERRUPTED_RETRY)),
        )
        .await;
        let (mut start_fut, _harness) = StartTestHarness::new(fake_device);

        assert!(matches!(
            TestExecutor::poll_until_stalled(&mut start_fut).await,
            Poll::Ready(Err(zx::Status::INTERRUPTED_RETRY))
        ));
    }

    fn bootstrap_generic_sme_proxy_and_inspect_vmo(
        usme_bootstrap_client_end: fidl::endpoints::ClientEnd<fidl_sme::UsmeBootstrapMarker>,
    ) -> (fidl_sme::GenericSmeProxy, impl Future<Output = Result<Vmo, fidl::Error>>) {
        let usme_client_proxy = usme_bootstrap_client_end.into_proxy();

        let legacy_privacy_support =
            fidl_sme::LegacyPrivacySupport { wep_supported: false, wpa1_supported: false };
        let (generic_sme_proxy, generic_sme_server) =
            fidl::endpoints::create_proxy::<fidl_sme::GenericSmeMarker>();
        (generic_sme_proxy, usme_client_proxy.start(generic_sme_server, &legacy_privacy_support))
    }

    #[test_case(FakeDeviceConfig::default().with_mock_query_response(Err(zx::Status::IO_DATA_INTEGRITY)), zx::Status::IO_DATA_INTEGRITY)]
    #[test_case(FakeDeviceConfig::default().with_mock_security_support(Err(zx::Status::IO_DATA_INTEGRITY)), zx::Status::IO_DATA_INTEGRITY)]
    #[test_case(FakeDeviceConfig::default().with_mock_spectrum_management_support(Err(zx::Status::IO_DATA_INTEGRITY)), zx::Status::IO_DATA_INTEGRITY)]
    #[test_case(FakeDeviceConfig::default().with_mock_mac_role(fidl_common::WlanMacRole::__SourceBreaking { unknown_ordinal: 0 }), zx::Status::INTERNAL)]
    #[fuchsia::test(allow_stalls = false)]
    async fn start_fails_on_query_error(
        fake_device_config: FakeDeviceConfig,
        expected_status: zx::Status,
    ) {
        let (fake_device, fake_device_state) =
            FakeDevice::new_with_config(fake_device_config).await;
        let (mut start_fut, _harness) = StartTestHarness::new(fake_device);

        let usme_bootstrap_client_end =
            fake_device_state.lock().usme_bootstrap_client_end.take().unwrap();
        let (_generic_sme_proxy, _inspect_vmo_fut) =
            bootstrap_generic_sme_proxy_and_inspect_vmo(usme_bootstrap_client_end);

        match TestExecutor::poll_until_stalled(&mut start_fut).await {
            Poll::Ready(Err(status)) => assert_eq!(status, expected_status),
            Poll::Pending => panic!("start_fut still pending!"),
            Poll::Ready(Ok(_)) => panic!("start_fut completed with Ok value"),
        }
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn start_fail_on_dropped_mlme_event_stream() {
        let (fake_device, fake_device_state) = FakeDevice::new().await;
        let (mut start_fut, _harness) = StartTestHarness::new(fake_device);

        let usme_bootstrap_client_end =
            fake_device_state.lock().usme_bootstrap_client_end.take().unwrap();
        let (_generic_sme_proxy, _inspect_vmo_fut) =
            bootstrap_generic_sme_proxy_and_inspect_vmo(usme_bootstrap_client_end);

        let _ = fake_device_state.lock().mlme_event_stream.take();
        match TestExecutor::poll_until_stalled(&mut start_fut).await {
            Poll::Ready(Err(status)) => assert_eq!(status, zx::Status::INTERNAL),
            Poll::Pending => panic!("start_fut still pending!"),
            Poll::Ready(Ok(_)) => panic!("start_fut completed with Ok value"),
        }
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn start_succeeds() {
        let (fake_device, fake_device_state) = FakeDevice::new_with_config(
            FakeDeviceConfig::default()
                .with_mock_sta_addr([2u8; 6])
                .with_mock_mac_role(fidl_common::WlanMacRole::Client),
        )
        .await;
        let (mut start_fut, mut harness) = StartTestHarness::new(fake_device);

        let usme_bootstrap_client_end =
            fake_device_state.lock().usme_bootstrap_client_end.take().unwrap();
        let (generic_sme_proxy, _inspect_vmo_fut) =
            bootstrap_generic_sme_proxy_and_inspect_vmo(usme_bootstrap_client_end);

        let StartedDriver {
            softmac_ifc_bridge_request_stream: _softmac_ifc_bridge_request_stream,
            mut mlme,
            sme,
        } = match TestExecutor::poll_until_stalled(&mut start_fut).await {
            Poll::Ready(Ok(x)) => x,
            Poll::Ready(Err(status)) => {
                panic!("start_fut unexpectedly failed; {}", status)
            }
            Poll::Pending => panic!("start_fut still pending!"),
        };

        assert_matches!(TestExecutor::poll_until_stalled(&mut mlme).await, Poll::Pending);
        assert!(matches!(
            TestExecutor::poll_until_stalled(&mut harness.mlme_init_receiver).await,
            Poll::Ready(Ok(()))
        ));

        let resp_fut = generic_sme_proxy.query();
        let mut resp_fut = pin!(resp_fut);
        assert_matches!(TestExecutor::poll_until_stalled(&mut resp_fut).await, Poll::Pending);

        let sme_and_mlme = [sme, mlme].into_iter().collect::<FuturesUnordered<_>>();
        let mut sme_and_mlme = pin!(sme_and_mlme);
        assert!(matches!(
            TestExecutor::poll_until_stalled(&mut sme_and_mlme.next()).await,
            Poll::Pending
        ));

        assert!(matches!(
            TestExecutor::poll_until_stalled(&mut resp_fut).await,
            Poll::Ready(Ok(fidl_sme::GenericSmeQuery {
                role: fidl_common::WlanMacRole::Client,
                sta_addr: [2, 2, 2, 2, 2, 2],
                factory_addr: [2, 2, 2, 2, 2, 2],
            }))
        ));
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn serve_wlansoftmac_ifc_bridge_fails_on_request_stream_error() {
        let (driver_event_sink, _driver_event_stream) = DriverEventSink::new();
        let (softmac_ifc_bridge_client, softmac_ifc_bridge_server) =
            fidl::endpoints::create_endpoints::<fidl_softmac::WlanSoftmacIfcBridgeMarker>();
        let softmac_ifc_bridge_request_stream = softmac_ifc_bridge_server.into_stream();
        let softmac_ifc_bridge_channel = softmac_ifc_bridge_client.into_channel();

        let server_fut =
            serve_wlan_softmac_ifc_bridge(driver_event_sink, softmac_ifc_bridge_request_stream);
        let mut server_fut = pin!(server_fut);
        assert_matches!(TestExecutor::poll_until_stalled(&mut server_fut).await, Poll::Pending);

        softmac_ifc_bridge_channel.write(&[], &mut []).unwrap();
        assert_matches!(
            TestExecutor::poll_until_stalled(&mut server_fut).await,
            Poll::Ready(Err(_))
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn serve_wlansoftmac_ifc_bridge_exits_on_request_stream_end() {
        let (driver_event_sink, _driver_event_stream) = DriverEventSink::new();
        let (softmac_ifc_bridge_client, softmac_ifc_bridge_server) =
            fidl::endpoints::create_endpoints::<fidl_softmac::WlanSoftmacIfcBridgeMarker>();
        let softmac_ifc_bridge_request_stream = softmac_ifc_bridge_server.into_stream();

        let server_fut =
            serve_wlan_softmac_ifc_bridge(driver_event_sink, softmac_ifc_bridge_request_stream);
        let mut server_fut = pin!(server_fut);
        assert_matches!(TestExecutor::poll_until_stalled(&mut server_fut).await, Poll::Pending);

        drop(softmac_ifc_bridge_client);
        assert_matches!(
            TestExecutor::poll_until_stalled(&mut server_fut).await,
            Poll::Ready(Err(_))
        );
    }

    #[test_case(fidl_softmac::WlanSoftmacIfcBaseNotifyScanCompleteRequest {
                status: None,
                scan_id: Some(754),
                ..Default::default()
    })]
    #[test_case(fidl_softmac::WlanSoftmacIfcBaseNotifyScanCompleteRequest {
                status: Some(zx::sys::ZX_OK),
                scan_id: None,
                ..Default::default()
            })]
    #[fuchsia::test(allow_stalls = false)]
    async fn serve_wlansoftmac_ifc_bridge_exits_on_invalid_notify_scan_complete_request(
        request: fidl_softmac::WlanSoftmacIfcBaseNotifyScanCompleteRequest,
    ) {
        let (driver_event_sink, mut driver_event_stream) = DriverEventSink::new();
        let (softmac_ifc_bridge_proxy, softmac_ifc_bridge_server) =
            fidl::endpoints::create_proxy::<fidl_softmac::WlanSoftmacIfcBridgeMarker>();
        let softmac_ifc_bridge_request_stream = softmac_ifc_bridge_server.into_stream();

        let server_fut =
            serve_wlan_softmac_ifc_bridge(driver_event_sink, softmac_ifc_bridge_request_stream);
        let mut server_fut = pin!(server_fut);

        let resp_fut = softmac_ifc_bridge_proxy.notify_scan_complete(&request);
        let mut resp_fut = pin!(resp_fut);
        assert_matches!(TestExecutor::poll_until_stalled(&mut resp_fut).await, Poll::Pending);
        assert_matches!(
            TestExecutor::poll_until_stalled(&mut server_fut).await,
            Poll::Ready(Err(_))
        );
        assert_matches!(TestExecutor::poll_until_stalled(&mut resp_fut).await, Poll::Ready(Ok(())));
        assert!(matches!(driver_event_stream.try_next(), Ok(None)));
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn serve_wlansoftmac_ifc_bridge_enqueues_notify_scan_complete() {
        let (driver_event_sink, mut driver_event_stream) = DriverEventSink::new();
        let (softmac_ifc_bridge_proxy, softmac_ifc_bridge_server) =
            fidl::endpoints::create_proxy::<fidl_softmac::WlanSoftmacIfcBridgeMarker>();
        let softmac_ifc_bridge_request_stream = softmac_ifc_bridge_server.into_stream();

        let server_fut =
            serve_wlan_softmac_ifc_bridge(driver_event_sink, softmac_ifc_bridge_request_stream);
        let mut server_fut = pin!(server_fut);

        let resp_fut = softmac_ifc_bridge_proxy.notify_scan_complete(
            &fidl_softmac::WlanSoftmacIfcBaseNotifyScanCompleteRequest {
                status: Some(zx::sys::ZX_OK),
                scan_id: Some(754),
                ..Default::default()
            },
        );
        let mut resp_fut = pin!(resp_fut);
        assert_matches!(TestExecutor::poll_until_stalled(&mut resp_fut).await, Poll::Pending);
        assert_matches!(TestExecutor::poll_until_stalled(&mut server_fut).await, Poll::Pending);
        assert_matches!(TestExecutor::poll_until_stalled(&mut resp_fut).await, Poll::Ready(Ok(())));

        assert!(matches!(
            driver_event_stream.try_next(),
            Ok(Some(DriverEvent::ScanComplete { status: Ok(()), scan_id: 754 }))
        ));
    }

    struct ServeTestHarness {
        pub mlme_init_sender: oneshot::Sender<()>,
        pub driver_event_stream: mpsc::UnboundedReceiver<DriverEvent>,
        pub softmac_ifc_bridge_proxy: fidl_softmac::WlanSoftmacIfcBridgeProxy,
        pub complete_mlme_sender: oneshot::Sender<Result<(), anyhow::Error>>,
        pub complete_sme_sender: oneshot::Sender<Result<(), anyhow::Error>>,
    }

    impl ServeTestHarness {
        fn new() -> (Pin<Box<impl Future<Output = Result<(), zx::Status>>>>, ServeTestHarness) {
            let (mlme_init_sender, mlme_init_receiver) = oneshot::channel();
            let (driver_event_sink, driver_event_stream) = DriverEventSink::new();
            let (softmac_ifc_bridge_proxy, softmac_ifc_bridge_server) =
                fidl::endpoints::create_proxy::<fidl_softmac::WlanSoftmacIfcBridgeMarker>();
            let softmac_ifc_bridge_request_stream = softmac_ifc_bridge_server.into_stream();
            let (complete_mlme_sender, complete_mlme_receiver) = oneshot::channel();
            let mlme = Box::pin(async { complete_mlme_receiver.await.unwrap() });
            let (complete_sme_sender, complete_sme_receiver) = oneshot::channel();
            let sme = Box::pin(async { complete_sme_receiver.await.unwrap() });

            (
                Box::pin(serve(
                    mlme_init_receiver,
                    driver_event_sink,
                    softmac_ifc_bridge_request_stream,
                    mlme,
                    sme,
                )),
                ServeTestHarness {
                    mlme_init_sender,
                    driver_event_stream,
                    softmac_ifc_bridge_proxy,
                    complete_mlme_sender,
                    complete_sme_sender,
                },
            )
        }
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn serve_wlansoftmac_ifc_bridge_enqueues_report_tx_result() {
        let (driver_event_sink, mut driver_event_stream) = DriverEventSink::new();
        let (softmac_ifc_bridge_proxy, softmac_ifc_bridge_server) =
            fidl::endpoints::create_proxy::<fidl_softmac::WlanSoftmacIfcBridgeMarker>();
        let softmac_ifc_bridge_request_stream = softmac_ifc_bridge_server.into_stream();

        let server_fut =
            serve_wlan_softmac_ifc_bridge(driver_event_sink, softmac_ifc_bridge_request_stream);
        let mut server_fut = pin!(server_fut);

        let resp_fut = softmac_ifc_bridge_proxy.report_tx_result(&fidl_softmac::WlanTxResult {
            tx_result_entry: [fidl_softmac::WlanTxResultEntry {
                tx_vector_idx: fidl_softmac::WLAN_TX_VECTOR_IDX_INVALID,
                attempts: 0,
            }; fidl_softmac::WLAN_TX_RESULT_MAX_ENTRY as usize],
            peer_addr: [3; 6],
            result_code: fidl_softmac::WlanTxResultCode::Failed,
        });
        let mut resp_fut = pin!(resp_fut);
        assert_matches!(TestExecutor::poll_until_stalled(&mut resp_fut).await, Poll::Pending);
        assert_matches!(TestExecutor::poll_until_stalled(&mut server_fut).await, Poll::Pending);
        assert_matches!(TestExecutor::poll_until_stalled(&mut resp_fut).await, Poll::Ready(Ok(())));

        match driver_event_stream.try_next().unwrap().unwrap() {
            DriverEvent::TxResultReport { tx_result } => {
                assert_eq!(
                    tx_result,
                    fidl_softmac::WlanTxResult {
                        tx_result_entry: [fidl_softmac::WlanTxResultEntry {
                            tx_vector_idx: fidl_softmac::WLAN_TX_VECTOR_IDX_INVALID,
                            attempts: 0
                        };
                            fidl_softmac::WLAN_TX_RESULT_MAX_ENTRY as usize],
                        peer_addr: [3; 6],
                        result_code: fidl_softmac::WlanTxResultCode::Failed,
                    }
                );
            }
            _ => panic!("Unexpected DriverEvent!"),
        }
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn serve_exits_with_error_if_mlme_init_sender_dropped() {
        let (mut serve_fut, harness) = ServeTestHarness::new();

        assert_matches!(TestExecutor::poll_until_stalled(&mut serve_fut).await, Poll::Pending);
        drop(harness.mlme_init_sender);
        assert_matches!(
            TestExecutor::poll_until_stalled(&mut serve_fut).await,
            Poll::Ready(Err(zx::Status::INTERNAL))
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn serve_exits_successfully_if_mlme_completes_just_before_init_sender_dropped() {
        let (mut serve_fut, harness) = ServeTestHarness::new();

        harness.complete_mlme_sender.send(Ok(())).unwrap();
        drop(harness.mlme_init_sender);
        assert_matches!(
            TestExecutor::poll_until_stalled(&mut serve_fut).await,
            Poll::Ready(Err(zx::Status::INTERNAL))
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn serve_exits_successfully_if_mlme_completes_just_before_init() {
        let (mut serve_fut, harness) = ServeTestHarness::new();

        harness.complete_mlme_sender.send(Ok(())).unwrap();
        harness.mlme_init_sender.send(()).unwrap();
        assert_matches!(
            TestExecutor::poll_until_stalled(&mut serve_fut).await,
            Poll::Ready(Ok(()))
        );
    }

    #[test_case(Ok(()))]
    #[test_case(Err(format_err!("")))]
    #[fuchsia::test(allow_stalls = false)]
    async fn serve_exits_with_error_if_mlme_completes_before_init(
        early_mlme_result: Result<(), Error>,
    ) {
        let (mut serve_fut, harness) = ServeTestHarness::new();

        assert_matches!(TestExecutor::poll_until_stalled(&mut serve_fut).await, Poll::Pending);
        harness.complete_mlme_sender.send(early_mlme_result).unwrap();
        assert_matches!(
            TestExecutor::poll_until_stalled(&mut serve_fut).await,
            Poll::Ready(Err(zx::Status::INTERNAL))
        );
    }

    #[test_case(Ok(()))]
    #[test_case(Err(format_err!("")))]
    #[fuchsia::test(allow_stalls = false)]
    async fn serve_exits_with_error_if_sme_shuts_down_before_mlme(
        early_sme_result: Result<(), Error>,
    ) {
        let (mut serve_fut, harness) = ServeTestHarness::new();

        assert_matches!(TestExecutor::poll_until_stalled(&mut serve_fut).await, Poll::Pending);
        harness.mlme_init_sender.send(()).unwrap();
        assert_matches!(TestExecutor::poll_until_stalled(&mut serve_fut).await, Poll::Pending);
        harness.complete_sme_sender.send(early_sme_result).unwrap();
        assert_matches!(
            TestExecutor::poll_until_stalled(&mut serve_fut).await,
            Poll::Ready(Err(zx::Status::INTERNAL))
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn serve_exits_with_error_if_mlme_completes_with_error() {
        let (mut serve_fut, harness) = ServeTestHarness::new();

        assert_matches!(TestExecutor::poll_until_stalled(&mut serve_fut).await, Poll::Pending);
        harness.mlme_init_sender.send(()).unwrap();
        assert_matches!(TestExecutor::poll_until_stalled(&mut serve_fut).await, Poll::Pending);
        harness.complete_mlme_sender.send(Err(format_err!("mlme error"))).unwrap();
        assert_eq!(
            TestExecutor::poll_until_stalled(&mut serve_fut).await,
            Poll::Ready(Err(zx::Status::INTERNAL))
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn serve_exits_with_error_if_sme_shuts_down_with_error() {
        let (mut serve_fut, harness) = ServeTestHarness::new();

        assert_matches!(TestExecutor::poll_until_stalled(&mut serve_fut).await, Poll::Pending);
        harness.mlme_init_sender.send(()).unwrap();
        assert_matches!(TestExecutor::poll_until_stalled(&mut serve_fut).await, Poll::Pending);
        harness.complete_mlme_sender.send(Ok(())).unwrap();
        harness.complete_sme_sender.send(Err(format_err!("sme error"))).unwrap();
        assert_eq!(
            TestExecutor::poll_until_stalled(&mut serve_fut).await,
            Poll::Ready(Err(zx::Status::INTERNAL))
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn serve_exits_with_error_if_bridge_exits_early_with_error() {
        let (mut serve_fut, harness) = ServeTestHarness::new();

        assert_matches!(TestExecutor::poll_until_stalled(&mut serve_fut).await, Poll::Pending);
        harness.mlme_init_sender.send(()).unwrap();
        assert_matches!(TestExecutor::poll_until_stalled(&mut serve_fut).await, Poll::Pending);

        // Cause the bridge to encounter an error when the client drops its endpoint.
        drop(harness.softmac_ifc_bridge_proxy);
        assert_eq!(serve_fut.await, Err(zx::Status::INTERNAL));
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn serve_exits_with_error_if_bridge_cannot_queue_stop() {
        let (mut serve_fut, harness) = ServeTestHarness::new();

        assert_matches!(TestExecutor::poll_until_stalled(&mut serve_fut).await, Poll::Pending);
        harness.mlme_init_sender.send(()).unwrap();
        assert_matches!(TestExecutor::poll_until_stalled(&mut serve_fut).await, Poll::Pending);

        // Cause the bridge to encounter an error when the bridge cannot queue the DriverEvent::Stop.
        drop(harness.driver_event_stream);
        harness.softmac_ifc_bridge_proxy.stop_bridged_driver().await.unwrap();
        assert_eq!(serve_fut.await, Err(zx::Status::INTERNAL));
    }

    #[test_case(true)]
    #[test_case(false)]
    #[fuchsia::test(allow_stalls = false)]
    async fn serve_shuts_down_gracefully(bridge_shutdown_before_mlme: bool) {
        let (mut serve_fut, mut harness) = ServeTestHarness::new();

        assert_matches!(TestExecutor::poll_until_stalled(&mut serve_fut).await, Poll::Pending);
        harness.mlme_init_sender.send(()).unwrap();
        assert_matches!(TestExecutor::poll_until_stalled(&mut serve_fut).await, Poll::Pending);
        let mut stop_response_fut = harness.softmac_ifc_bridge_proxy.stop_bridged_driver();

        // The server should not fail if the MLME future completes before or after the bridge futures
        // completes. This is because the bridge server sends the message for MLME to shutdown just
        // before completing. And so it's possible for MLME to shutdown before the bridge server completes.
        //
        // Note that both orderings are possible even with a single threaded executor. This is because
        // the bridge futures actually sends two messages before completing. The first to MLME to shutdown
        // and the second to the bridge exit receiver with the result from the bridge completing. Thus,
        // there is a race between the MLME and bridge exit receiver, either of which could run first
        // depending on the executor.
        if bridge_shutdown_before_mlme {
            assert_matches!(
                TestExecutor::poll_until_stalled(&mut stop_response_fut).await,
                Poll::Pending
            );
            let responder = assert_matches!(harness.driver_event_stream.next().await,
                Some(DriverEvent::Stop{ responder }) => responder);
            responder.send().unwrap();
            assert_matches!(
                TestExecutor::poll_until_stalled(&mut stop_response_fut).await,
                Poll::Ready(Ok(()))
            );

            harness.complete_mlme_sender.send(Ok(())).unwrap();
        } else {
            harness.complete_mlme_sender.send(Ok(())).unwrap();
            assert_eq!(TestExecutor::poll_until_stalled(&mut serve_fut).await, Poll::Pending);

            assert_matches!(
                TestExecutor::poll_until_stalled(&mut stop_response_fut).await,
                Poll::Pending
            );
            let responder = assert_matches!(harness.driver_event_stream.next().await,
                Some(DriverEvent::Stop{ responder }) => responder);
            responder.send().unwrap();
            assert_matches!(
                TestExecutor::poll_until_stalled(&mut stop_response_fut).await,
                Poll::Ready(Ok(()))
            );
        }

        assert_eq!(TestExecutor::poll_until_stalled(&mut serve_fut).await, Poll::Pending);
        harness.complete_sme_sender.send(Ok(())).unwrap();
        assert_eq!(TestExecutor::poll_until_stalled(&mut serve_fut).await, Poll::Ready(Ok(())));
    }

    #[derive(Debug)]
    struct StartAndServeTestHarness<F> {
        pub start_and_serve_fut: F,
        pub start_complete_receiver: oneshot::Receiver<zx::sys::zx_status_t>,
        pub generic_sme_proxy: fidl_sme::GenericSmeProxy,
    }

    /// This function wraps start_and_serve() with a FakeDevice provided by a test.
    ///
    /// The returned start_and_serve() future will run the WlanSoftmacIfcBridge, MLME, and SME servers when
    /// run on an executor.
    ///
    /// An Err value will be returned if start_and_serve() encounters an error completing the bootstrap
    /// of the SME server.
    async fn start_and_serve_with_device(
        fake_device: FakeDevice,
    ) -> Result<StartAndServeTestHarness<impl Future<Output = Result<(), zx::Status>>>, zx::Status>
    {
        let (start_complete_sender, mut start_complete_receiver) =
            oneshot::channel::<zx::sys::zx_status_t>();
        let start_and_serve_fut = start_and_serve(
            Completer::new(move |status| {
                start_complete_sender.send(status).expect("Failed to signal start complete.")
            }),
            fake_device.clone(),
        );
        let mut start_and_serve_fut = Box::pin(start_and_serve_fut);

        let usme_bootstrap_client_end = fake_device.state().lock().usme_bootstrap_client_end.take();
        match usme_bootstrap_client_end {
            // Simulate an errant initialization case where the UsmeBootstrap client end has been dropped
            // during initialization.
            None => match TestExecutor::poll_until_stalled(&mut start_and_serve_fut).await {
                Poll::Pending => panic!(
                    "start_and_serve() failed to exit when the UsmeBootstrap client was dropped."
                ),
                Poll::Ready(result) => {
                    assert_matches!(
                        TestExecutor::poll_until_stalled(&mut start_complete_receiver).await,
                        Poll::Ready(Ok(status)) => assert_ne!(status, zx::sys::ZX_OK)
                    );
                    return Err(result.unwrap_err());
                }
            },
            // Simulate the normal initialization case where the the UsmeBootstrap client end is active
            // during initialization.
            Some(usme_bootstrap_client_end) => {
                let (generic_sme_proxy, inspect_vmo_fut) =
                    bootstrap_generic_sme_proxy_and_inspect_vmo(usme_bootstrap_client_end);
                let start_and_serve_fut =
                    match TestExecutor::poll_until_stalled(&mut start_and_serve_fut).await {
                        Poll::Pending => start_and_serve_fut,
                        Poll::Ready(result) => {
                            assert_matches!(
                                TestExecutor::poll_until_stalled(&mut start_complete_receiver)
                                    .await,
                                Poll::Ready(Ok(status)) => assert_ne!(status, zx::sys::ZX_OK)
                            );
                            return Err(result.unwrap_err());
                        }
                    };

                inspect_vmo_fut.await.expect("Failed to bootstrap USME.");

                Ok(StartAndServeTestHarness {
                    start_and_serve_fut,
                    start_complete_receiver,
                    generic_sme_proxy,
                })
            }
        }
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn start_and_serve_fails_on_dropped_usme_bootstrap_client() {
        let (fake_device, fake_device_state) = FakeDevice::new().await;
        fake_device_state.lock().usme_bootstrap_client_end = None;
        match start_and_serve_with_device(fake_device.clone()).await {
            Ok(_) => panic!(
                "start_and_serve() does not fail when the UsmeBootstrap client end is dropped."
            ),
            Err(status) => assert_eq!(status, zx::Status::BAD_STATE),
        }
    }

    // Exhaustive feature tests are unit tested on start()
    #[fuchsia::test(allow_stalls = false)]
    async fn start_and_serve_fails_on_dropped_mlme_event_stream() {
        let (mut fake_device, _fake_device_state) = FakeDevice::new().await;
        let _ = fake_device.take_mlme_event_stream();
        match start_and_serve_with_device(fake_device.clone()).await {
            Ok(_) => {
                panic!("start_and_serve() does not fail when the MLME event stream is missing.")
            }
            Err(status) => assert_eq!(status, zx::Status::INTERNAL),
        }
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn start_and_serve_fails_on_dropped_generic_sme_client() {
        let (fake_device, _fake_device_state) = FakeDevice::new().await;
        let StartAndServeTestHarness {
            mut start_and_serve_fut,
            mut start_complete_receiver,
            generic_sme_proxy,
        } = start_and_serve_with_device(fake_device)
            .await
            .expect("Failed to initiate wlansoftmac setup.");
        assert_matches!(
            TestExecutor::poll_until_stalled(&mut start_complete_receiver).await,
            Poll::Ready(Ok(status)) => assert_eq!(zx::sys::ZX_OK, status)
        );
        assert_eq!(TestExecutor::poll_until_stalled(&mut start_and_serve_fut).await, Poll::Pending);

        drop(generic_sme_proxy);

        assert_eq!(
            TestExecutor::poll_until_stalled(&mut start_and_serve_fut).await,
            Poll::Ready(Err(zx::Status::INTERNAL))
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn start_and_serve_shuts_down_gracefully() {
        let (fake_device, fake_device_state) = FakeDevice::new().await;
        let StartAndServeTestHarness {
            mut start_and_serve_fut,
            mut start_complete_receiver,
            generic_sme_proxy: _generic_sme_proxy,
        } = start_and_serve_with_device(fake_device)
            .await
            .expect("Failed to initiate wlansoftmac setup.");
        assert_eq!(TestExecutor::poll_until_stalled(&mut start_and_serve_fut).await, Poll::Pending);
        assert_matches!(
            TestExecutor::poll_until_stalled(&mut start_complete_receiver).await,
            Poll::Ready(Ok(status)) => assert_eq!(zx::sys::ZX_OK, status)
        );

        let wlan_softmac_ifc_bridge_proxy =
            fake_device_state.lock().wlan_softmac_ifc_bridge_proxy.take().unwrap();
        let stop_response_fut = wlan_softmac_ifc_bridge_proxy.stop_bridged_driver();
        assert_matches!(futures::join!(start_and_serve_fut, stop_response_fut), (Ok(()), Ok(())));
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn start_and_serve_responds_to_generic_sme_requests() {
        let (fake_device, fake_device_state) = FakeDevice::new().await;
        let StartAndServeTestHarness {
            mut start_and_serve_fut,
            mut start_complete_receiver,
            generic_sme_proxy,
        } = start_and_serve_with_device(fake_device)
            .await
            .expect("Failed to initiate wlansoftmac setup.");
        assert_matches!(
            TestExecutor::poll_until_stalled(&mut start_complete_receiver).await,
            Poll::Ready(Ok(status)) => assert_eq!(zx::sys::ZX_OK, status)
        );

        let (sme_telemetry_proxy, sme_telemetry_server) = fidl::endpoints::create_proxy();
        let (client_sme_proxy, client_sme_server) = fidl::endpoints::create_proxy();

        let resp_fut = generic_sme_proxy.get_sme_telemetry(sme_telemetry_server);
        let mut resp_fut = pin!(resp_fut);

        // First poll `get_sme_telemetry` to send a `GetSmeTelemetry` request to the SME server, and then
        // poll the SME server process it. Finally, expect `get_sme_telemetry` to complete with `Ok(())`.
        assert_matches!(TestExecutor::poll_until_stalled(&mut resp_fut).await, Poll::Pending);
        assert_eq!(TestExecutor::poll_until_stalled(&mut start_and_serve_fut).await, Poll::Pending);
        assert_matches!(
            TestExecutor::poll_until_stalled(&mut resp_fut).await,
            Poll::Ready(Ok(Ok(())))
        );

        let resp_fut = generic_sme_proxy.get_client_sme(client_sme_server);
        let mut resp_fut = pin!(resp_fut);

        // First poll `get_client_sme` to send a `GetClientSme` request to the SME server, and then poll the
        // SME server process it. Finally, expect `get_client_sme` to complete with `Ok(())`.
        assert_matches!(TestExecutor::poll_until_stalled(&mut resp_fut).await, Poll::Pending);
        assert_eq!(TestExecutor::poll_until_stalled(&mut start_and_serve_fut).await, Poll::Pending);
        resp_fut.await.expect("Generic SME proxy failed").expect("Client SME request failed");

        let wlan_softmac_ifc_bridge_proxy =
            fake_device_state.lock().wlan_softmac_ifc_bridge_proxy.take().unwrap();
        let stop_response_fut = wlan_softmac_ifc_bridge_proxy.stop_bridged_driver();
        assert_matches!(futures::join!(start_and_serve_fut, stop_response_fut), (Ok(()), Ok(())));

        // All SME proxies should shutdown.
        assert!(generic_sme_proxy.is_closed());
        assert!(sme_telemetry_proxy.is_closed());
        assert!(client_sme_proxy.is_closed());
    }

    // Mocking a passive scan verifies the path through SME, MLME, and the FFI is functional. Other paths
    // are much more complex to mock and sufficiently covered by other testing. For example, queueing an
    // Ethernet frame requires mocking an association first, and the outcome of a reported Tx result cannot
    // be confirmed because the Minstrel is internal to MLME.
    #[fuchsia::test(allow_stalls = false)]
    async fn start_and_serve_responds_to_passive_scan_request() {
        let (fake_device, fake_device_state) = FakeDevice::new().await;
        let StartAndServeTestHarness {
            mut start_and_serve_fut,
            mut start_complete_receiver,
            generic_sme_proxy,
        } = start_and_serve_with_device(fake_device)
            .await
            .expect("Failed to initiate wlansoftmac setup.");
        assert_matches!(
            TestExecutor::poll_until_stalled(&mut start_complete_receiver).await,
            Poll::Ready(Ok(status)) => assert_eq!(zx::sys::ZX_OK, status)
        );

        let (client_sme_proxy, client_sme_server) = fidl::endpoints::create_proxy();

        let resp_fut = generic_sme_proxy.get_client_sme(client_sme_server);
        let mut resp_fut = pin!(resp_fut);
        assert_matches!(TestExecutor::poll_until_stalled(&mut resp_fut).await, Poll::Pending);
        assert_eq!(TestExecutor::poll_until_stalled(&mut start_and_serve_fut).await, Poll::Pending);
        assert!(matches!(
            TestExecutor::poll_until_stalled(&mut resp_fut).await,
            Poll::Ready(Ok(Ok(())))
        ));

        let scan_response_fut =
            client_sme_proxy.scan(&fidl_sme::ScanRequest::Passive(fidl_sme::PassiveScanRequest {
                channels: vec![],
            }));
        let mut scan_response_fut = pin!(scan_response_fut);
        assert!(matches!(
            TestExecutor::poll_until_stalled(&mut scan_response_fut).await,
            Poll::Pending
        ));

        assert!(fake_device_state.lock().captured_passive_scan_request.is_none());
        assert_eq!(TestExecutor::poll_until_stalled(&mut start_and_serve_fut).await, Poll::Pending);
        assert!(fake_device_state.lock().captured_passive_scan_request.is_some());

        let wlan_softmac_ifc_bridge_proxy =
            fake_device_state.lock().wlan_softmac_ifc_bridge_proxy.take().unwrap();
        let notify_scan_complete_fut = wlan_softmac_ifc_bridge_proxy.notify_scan_complete(
            &fidl_softmac::WlanSoftmacIfcBaseNotifyScanCompleteRequest {
                status: Some(zx::sys::ZX_OK),
                scan_id: Some(0),
                ..Default::default()
            },
        );
        notify_scan_complete_fut.await.expect("Failed to receive NotifyScanComplete response");
        assert_eq!(TestExecutor::poll_until_stalled(&mut start_and_serve_fut).await, Poll::Pending);
        assert!(matches!(
            TestExecutor::poll_until_stalled(&mut scan_response_fut).await,
            Poll::Ready(Ok(_))
        ));

        let stop_response_fut = wlan_softmac_ifc_bridge_proxy.stop_bridged_driver();
        assert_matches!(futures::join!(start_and_serve_fut, stop_response_fut), (Ok(()), Ok(())));

        // All SME proxies should shutdown.
        assert!(generic_sme_proxy.is_closed());
        assert!(client_sme_proxy.is_closed());
    }
}

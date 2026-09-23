// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! C bindings for wlansoftmac-rust crate.
#![deny(unsafe_op_in_unsafe_fn)]

use driver_diagnostics_log::PublishOptions;
use fidl_fuchsia_wlan_softmac as fidl_softmac;
use fuchsia_async::LocalExecutor;
use log::{error, info};
use std::ffi::c_void;
use std::sync::Once;
use wlan_ffi_transport::completers::Completer;
use wlan_ffi_transport::{EthernetRx, FfiEthernetRx, FfiWlanTx, WlanTx};
use wlan_mlme::device::Device;
use wlan_trace as wtrace;

use fdf::{AsAsyncDispatcherRef, DispatcherBuilder};

static LOGGER_ONCE: Once = Once::new();

/// Start and run a bridged wlansoftmac driver hosting an MLME server and an SME server.
///
/// The driver is "bridged" in the sense that it requires a bridge to a Fuchsia driver to
/// communicate with other Fuchsia drivers over the FDF transport. After the bridged
/// driver starts successfully, `run_start_completer` will be called with `start_completer`.
/// If the bridged driver does not start successfully, this function will return a
/// non-`ZX_OK` status.
///
/// This function returns `ZX_OK` only if shutdown completes successfully. A successful
/// shutdown only occurs if the bridged driver receives a
/// `fuchsia.wlan.softmac/WlanSoftmacIfcBridge.StopBridgedDriver` message and the subsequent
/// teardown of the hosted server succeeds.
///
/// In all other scenarios, e.g., failure during startup, while running, or during shutdown,
/// this function will return a non-`ZX_OK` value.
///
/// # Safety
///
/// This function is unsafe for the following reasons:
///
///   - This function cannot guarantee `run_start_completer` is thread-safe.
///   - This function cannot guarantee `start_completer` points to a valid object when
///     `run_start_completer` is called.
///   - This function cannot guarantee `run_shutdown_completer` is thread-safe.
///   - This function cannot guarantee `shutdown_completer` points to a valid object when
///     `run_shutdown_completer` is called.
///   - This function cannot guarantee `wlan_softmac_bridge_client_handle` is a valid handle.
///
/// By calling this function, the caller promises the following:
///
///   - The `run_start_completer` function is thread-safe.
///   - The `start_completer` pointer will point to a valid object at least until
///     `run_start_completer` is called.
///   - The `run_shutdown_completer` function is thread-safe.
///   - The `shutdown_completer` pointer will point to a valid object at least until
///     `run_shutdown_completer` is called.
///   - The `wlan_softmac_bridge_client_handle` is a valid handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn start_bridged_wlansoftmac(
    start_completer: *mut c_void,
    run_start_completer: unsafe extern "C" fn(
        start_completer: *mut c_void,
        status: zx::sys::zx_status_t,
    ),
    shutdown_completer: *mut c_void,
    run_shutdown_completer: unsafe extern "C" fn(
        shutdown_completer: *mut c_void,
        status: zx::sys::zx_status_t,
    ),
    ethernet_rx: FfiEthernetRx,
    wlan_tx: FfiWlanTx,
    wlan_softmac_bridge_client_handle: zx::sys::zx_handle_t,
) -> zx::sys::zx_status_t {
    // The Fuchsia syslog must not be initialized from Rust more than once per process. In the case
    // of MLME, that means we can only call it once for both the client and ap modules. Ensure this
    // by using a shared `Once::call_once()`.
    LOGGER_ONCE.call_once(|| {
        if log::log_enabled!(log::Level::Info) {
            // Assuming logger is already initialized if log level is enabled.
            return;
        }
        // Initialize logging with a tag that can be used to filter for forwarding to console
        driver_diagnostics_log::initialize_sync(
            PublishOptions::default()
                .tags(&["wlan"])
                .enable_metatag(driver_diagnostics_log::Metatag::Target),
        );
    });

    // Safety: The provided closure is safe to send to another thread because start_completer
    // and run_start_completer are thread-safe.
    let start_completer = unsafe {
        Completer::new_unchecked(move |status| {
            // Safety: This is safe because the caller of this function promised
            // `run_start_completer` is thread-safe and `start_completer` is valid until
            // its called.
            run_start_completer(start_completer, status);
        })
    };

    let dispatcher = match DispatcherBuilder::new()
        .name("bridged-wlansoftmac")
        .allow_thread_blocking()
        .shutdown_observer(|_dispatcher| {
            wtrace::duration!("bridged-wlansoftmac dispatcher shutdown_handler");
            info!("Completed bridged-wlansoftmac dispatcher shutdown");
        })
        // Create a released dispatcher to defer destruction until driver
        // shutdown. The framework will shutdown the dispatcher
        // despite its release here because the framework tagged this
        // driver as the owner of the dispatcher upon creation. The
        // framework always automatically shuts down all dispatchers
        // owned by a driver during the driver's shut down.
        .create_released()
    {
        Ok(dispatcher) => dispatcher,
        Err(status) => {
            info!("Failed to create dispatcher for MLME: {}", status);
            start_completer.reply(status.into());
            return status.into_raw();
        }
    };

    let ethernet_rx = EthernetRx::new(ethernet_rx);
    let wlan_tx = WlanTx::new(wlan_tx);

    // Safety: The provided closure is safe to send to another thread because shutdown_completer
    // and run_shutdown_completer are thread-safe.
    let shutdown_completer = unsafe {
        Completer::new_unchecked(move |status| {
            // Safety: This is safe because the caller of this function promised
            // `run_shutdown_completer` is thread-safe and `shutdown_completer` is valid until
            // its called.
            run_shutdown_completer(shutdown_completer, status);
        })
    };

    let task = dispatcher.post_task_sync(move |status| {
        wtrace::duration!("Rust MLME dispatcher");
        if let Err(status) = Result::from(status) {
            error!("Rust MLME dispatcher failed to start: {status}");
            return;
        }

        let mut executor = LocalExecutor::default();

        let wlan_softmac_bridge_proxy = {
            // Safety: This is safe because the caller promises `wlan_softmac_bridge_client_handle`
            // is a valid handle.
            let handle =
                unsafe { fidl::NullableHandle::from_raw(wlan_softmac_bridge_client_handle) };
            let channel = fidl::Channel::from(handle);
            let channel = fidl::AsyncChannel::from_channel(channel);
            fidl_softmac::WlanSoftmacBridgeProxy::new(channel)
        };
        let device = Device::new(wlan_softmac_bridge_proxy, ethernet_rx, wlan_tx);

        let result =
            executor.run_singlethreaded(wlansoftmac_rust::start_and_serve(start_completer, device));
        shutdown_completer.reply(result);
    });

    zx::Status::result_into_raw(task)
}

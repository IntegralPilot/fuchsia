// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::reboot_reasons::ShutdownOptionsWrapper;

use fidl::endpoints::Proxy;
use fidl_fuchsia_hardware_power_statecontrol::{self as fpower, ShutdownAction};
use fuchsia_async::{self as fasync, DurationExt, TimeoutExt};
use fuchsia_inspect::{self as inspect, NumericProperty};
use futures::TryStreamExt;
use futures::lock::Mutex;
use futures::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;
use zx::AsHandleRef;
use zx::sys::zx_handle_t;

/// Summary: Provides an implementation of the
/// fuchsia.hardware.power.statecontrol.ShutdownWatcherRegister and RebootMethodsWatcherRegister
/// protocols that allows other components in the system to register to be notified of pending
/// system shutdown requests and the associated shutdown reason.
///
/// FIDL dependencies:
///     - fuchsia.hardware.power.statecontrol.ShutdownWatcherRegister and
///       RebootMethodsWatcherRegister: the server provides a Register API that other components in
///       the system can use to receive notifications about system shutdown events and reasons
///     - fuchsia.hardware.power.statecontrol.ShutdownWatcher, TerminalStateWatcher, and
///       RebootMethodsWatcher: the server receives an instance of this protocol over the
///       ShutdownWatcherRegister and RebootMethodsWatcherRegister channels, and uses this channel
///       to send shutdown notifications.
//
// TODO(https://fxbug.dev/414413282): Remove everything related to RebootMethodsWatcherRegister once
// only F28+ is supported.
pub struct ShutdownWatcher {
    /// Contains all the registered RebootMethodsWatcher channels to be notified when a reboot
    /// request is received.
    reboot_watchers: Arc<Mutex<HashMap<u32, fpower::RebootWatcherProxy>>>,
    /// Contains all the registered ShutdownWatcher channels to be notified when a shutdown request
    /// is received.
    shutdown_watchers: Arc<Mutex<HashMap<zx_handle_t, fpower::ShutdownWatcherProxy>>>,
    /// Contains all the registered TerminalStateWatcher channels to be notified when a transition
    /// to a terminal state begins.
    terminal_state_watchers: Arc<Mutex<HashMap<zx_handle_t, fpower::TerminalStateWatcherProxy>>>,
    inspect: Arc<Mutex<InspectData>>,
}

impl ShutdownWatcher {
    const NOTIFY_SHUTDOWN_RESPONSE_TIMEOUT: zx::MonotonicDuration =
        zx::MonotonicDuration::from_seconds(
            fpower::MAX_SHUTDOWN_WATCHER_RESPONSE_TIME_SECONDS as i64,
        );

    const NOTIFY_REBOOT_RESPONSE_TIMEOUT: zx::MonotonicDuration =
        zx::MonotonicDuration::from_seconds(
            fpower::MAX_REBOOT_WATCHER_RESPONSE_TIME_SECONDS as i64,
        );

    const NOTIFY_TERMINAL_STATE_RESPONSE_TIMEOUT: zx::MonotonicDuration =
        zx::MonotonicDuration::from_seconds(
            fpower::MAX_TERMINAL_STATE_WATCHER_RESPONSE_TIME_SECONDS as i64,
        );

    #[cfg(test)]
    fn new() -> Arc<Self> {
        let inspector = inspect::Inspector::new(fuchsia_inspect::InspectorConfig::default());
        Self::new_with_inspector(&inspector)
    }

    pub fn new_with_inspector(inspector: &inspect::Inspector) -> Arc<Self> {
        Arc::new(Self {
            reboot_watchers: Arc::new(Mutex::new(HashMap::new())),
            shutdown_watchers: Arc::new(Mutex::new(HashMap::new())),
            terminal_state_watchers: Arc::new(Mutex::new(HashMap::new())),
            inspect: Arc::new(Mutex::new(InspectData::new(
                inspector.root(),
                "ShutdownWatcher".to_string(),
            ))),
        })
    }

    /// Handles a new client connection to the RebootMethodsWatcherRegister service.
    pub async fn handle_reboot_register_request(
        self: Arc<Self>,
        mut stream: fpower::RebootMethodsWatcherRegisterRequestStream,
    ) {
        while let Ok(Some(req)) = stream.try_next().await {
            match req {
                fpower::RebootMethodsWatcherRegisterRequest::RegisterWatcher {
                    watcher,
                    responder,
                } => {
                    self.add_reboot_watcher(watcher.into_proxy()).await;
                    let _ = responder.send();
                }
            }
        }
    }

    /// Handles a new client connection to the ShutdownWatcherRegister service.
    pub async fn handle_shutdown_register_request(
        self: Arc<Self>,
        mut stream: fpower::ShutdownWatcherRegisterRequestStream,
    ) {
        while let Ok(Some(req)) = stream.try_next().await {
            match req {
                fpower::ShutdownWatcherRegisterRequest::RegisterWatcher { watcher, responder } => {
                    self.add_shutdown_watcher(watcher.into_proxy()).await;
                    let _ = responder.send();
                }
                fpower::ShutdownWatcherRegisterRequest::RegisterTerminalStateWatcher {
                    watcher,
                    responder,
                } => {
                    self.add_terminal_state_watcher(watcher.into_proxy()).await;
                    let _ = responder.send();
                }
                fpower::ShutdownWatcherRegisterRequest::_UnknownMethod { ordinal, .. } => {
                    println!("[shutdown-shim]: error, unimplemented method ordinal: {ordinal}")
                }
            }
        }
    }

    /// Adds a new RebootMethodsWatcher channel to the list of registered watchers.
    async fn add_reboot_watcher(&self, watcher: fpower::RebootWatcherProxy) {
        fuchsia_trace::duration!(
            c"shutdown-shim",
            c"ShutdownWatcher::add_reboot_watcher",
            "watcher" => watcher.as_channel().as_handle_ref().raw_handle()
        );

        // If the client closes the watcher channel, remove it from our `reboot_watchers` map
        println!("[shutdown-shim] Adding a reboot watcher");
        let key = watcher.as_channel().as_handle_ref().raw_handle();
        let proxy = watcher.clone();
        let reboot_watchers = self.reboot_watchers.clone();
        let inspect = self.inspect.clone();
        fasync::Task::spawn(async move {
            let _ = proxy.on_closed().await;
            {
                reboot_watchers.lock().await.remove(&key);
            }
            inspect.lock().await.remove_reboot_watcher();
        })
        .detach();

        {
            let mut watchers_mut = self.reboot_watchers.lock().await;
            watchers_mut.insert(key, watcher);
        }
        self.inspect.lock().await.add_reboot_watcher();
    }

    /// Adds a new ShutdownWatcher channel to the list of registered watchers.
    async fn add_shutdown_watcher(&self, watcher: fpower::ShutdownWatcherProxy) {
        fuchsia_trace::duration!(
            c"shutdown-shim",
            c"ShutdownWatcher::add_shutdown_watcher",
            "watcher" => watcher.as_channel().as_handle_ref().raw_handle()
        );

        // If the client closes the watcher channel, remove it from our `shutdown_watchers` map
        println!("[shutdown-shim] Adding a shutdown watcher");
        let key = watcher.as_channel().as_handle_ref().raw_handle();
        let proxy = watcher.clone();
        let shutdown_watchers = self.shutdown_watchers.clone();
        let inspect = self.inspect.clone();
        fasync::Task::spawn(async move {
            let _ = proxy.on_closed().await;
            {
                shutdown_watchers.lock().await.remove(&key);
            }
            inspect.lock().await.remove_reboot_watcher();
        })
        .detach();

        {
            let mut watchers_mut = self.shutdown_watchers.lock().await;
            watchers_mut.insert(key, watcher);
        }
        self.inspect.lock().await.add_reboot_watcher();
    }

    /// Adds a new TerminalStateWatcher channel to the list of registered watchers.
    async fn add_terminal_state_watcher(&self, watcher: fpower::TerminalStateWatcherProxy) {
        fuchsia_trace::duration!(
            c"shutdown-shim",
            c"ShutdownWatcher::add_terminal_state_watcher",
            "watcher" => watcher.as_channel().as_handle_ref().raw_handle()
        );

        // If the client closes the watcher channel, remove it from our `terminal_state_watchers` map
        println!("[shutdown-shim] Adding a terminal state watcher");
        let key = watcher.as_channel().as_handle_ref().raw_handle();
        let proxy = watcher.clone();
        let terminal_state_watchers = self.terminal_state_watchers.clone();
        let inspect = self.inspect.clone();
        fasync::Task::spawn(async move {
            let _ = proxy.on_closed().await;
            {
                terminal_state_watchers.lock().await.remove(&key);
            }
            inspect.lock().await.remove_terminal_state_watcher();
        })
        .detach();

        {
            let mut watchers_mut = self.terminal_state_watchers.lock().await;
            watchers_mut.insert(key, watcher);
        }
        self.inspect.lock().await.add_terminal_state_watcher();
    }

    /// Handles the SystemShutdown message by notifying the appropriate registered watchers.
    pub async fn handle_system_shutdown_message(&self, options: Option<ShutdownOptionsWrapper>) {
        let terminal_state_fut =
            self.notify_terminal_state_watchers(Self::NOTIFY_TERMINAL_STATE_RESPONSE_TIMEOUT);

        let Some(options) = options else {
            terminal_state_fut.await;
            return;
        };

        if options.action == ShutdownAction::Reboot && !options.reasons.is_empty() {
            futures::join!(
                self.notify_shutdown_watchers(
                    options.clone(),
                    Self::NOTIFY_SHUTDOWN_RESPONSE_TIMEOUT
                ),
                self.notify_reboot_watchers(options, Self::NOTIFY_REBOOT_RESPONSE_TIMEOUT),
                terminal_state_fut
            );
        } else {
            futures::join!(
                self.notify_shutdown_watchers(options, Self::NOTIFY_SHUTDOWN_RESPONSE_TIMEOUT),
                terminal_state_fut
            );
        }
    }

    async fn notify_reboot_watchers(
        &self,
        options: ShutdownOptionsWrapper,
        timeout: zx::MonotonicDuration,
    ) {
        // Instead of waiting for https://fxbug.dev/42120903, use begin and end pair of macros.
        fuchsia_trace::duration_begin!(
            c"shutdown-shim",
            c"ShutdownWatcher::notify_reboot_watchers",
            "options" => format!("{:?}", options).as_str()
        );

        // Create a future for each watcher that calls the watcher's `on_reboot` method and returns
        // the watcher proxy if the response was received within the timeout, or None otherwise. We
        // take this approach so that watchers that timed out have their channel dropped
        // (https://fxbug.dev/42131208).
        let watcher_futures = {
            // Take the current watchers out of the RefCell because we'll be modifying the vector
            let watchers = self.reboot_watchers.lock().await;
            println!("[shutdown-shim] notifying {:?} watchers of reboot", watchers.len());
            watchers.clone().into_iter().map(|(key, watcher_proxy)| {
                let options = options.clone();
                async move {
                    let deadline = timeout.after_now();
                    let result = watcher_proxy
                        .on_reboot(&options.into())
                        .map_err(|_| ())
                        .on_timeout(deadline, || Err(()))
                        .await;
                    match result {
                        Ok(()) => Some((key, watcher_proxy)),
                        Err(()) => None,
                    }
                }
            })
        };

        // Run all of the futures, collecting the successful watcher proxies into a vector
        let new_watchers = futures::future::join_all(watcher_futures)
            .await
            .into_iter()
            .filter_map(|watcher_opt| watcher_opt) // Unwrap the Options while filtering out None
            .collect();

        // Repopulate the successful watcher proxies back into the `reboot_watchers` RefCell
        *self.reboot_watchers.lock().await = new_watchers;

        fuchsia_trace::duration_end!(
            c"shutdown-shim",
            c"ShutdownWatcher::notify_reboot_watchers",
            "options" => format!("{:?}", options).as_str()
        );
    }

    async fn notify_shutdown_watchers(
        &self,
        options: ShutdownOptionsWrapper,
        timeout: zx::MonotonicDuration,
    ) {
        // Instead of waiting for https://fxbug.dev/42120903, use begin and end pair of macros.
        fuchsia_trace::duration_begin!(
            c"shutdown-shim",
            c"ShutdownWatcher::notify_shutdown_watchers",
            "options" => format!("{:?}", options).as_str()
        );

        // Create a future for each watcher that calls the watcher's `on_shutdown` method and
        // returns the watcher proxy if the response was received within the timeout, or None
        // otherwise. We take this approach so that watchers that timed out have their channel
        // dropped (https://fxbug.dev/42131208).
        let watcher_futures = {
            // Take the current watchers out of `shutdown_watchers` because we'll be modifying the
            // vector.
            let watchers = self.shutdown_watchers.lock().await;
            println!("[shutdown-shim] notifying {:?} watchers of shutdown", watchers.len());
            watchers.clone().into_iter().map(|(key, watcher_proxy)| {
                let options = options.clone();
                async move {
                    let deadline = timeout.after_now();
                    let result = watcher_proxy
                        .on_shutdown(&options.into())
                        .map_err(|_| ())
                        .on_timeout(deadline, || Err(()))
                        .await;

                    match result {
                        Ok(()) => Some((key, watcher_proxy)),
                        Err(()) => None,
                    }
                }
            })
        };

        // Run all of the futures, collecting the successful watcher proxies into a vector
        let new_watchers = futures::future::join_all(watcher_futures)
            .await
            .into_iter()
            .filter_map(|watcher_opt| watcher_opt) // Unwrap the Options while filtering out None
            .collect();

        // Repopulate the successful watcher proxies back into `shutdown_watchers`
        *self.shutdown_watchers.lock().await = new_watchers;

        fuchsia_trace::duration_end!(
            c"shutdown-shim",
            c"ShutdownWatcher::notify_shutdown_watchers",
            "options" => format!("{:?}", options).as_str()
        );
    }

    async fn notify_terminal_state_watchers(&self, timeout: zx::MonotonicDuration) {
        fuchsia_trace::duration!(
            c"shutdown-shim",
            c"ShutdownWatcher::notify_terminal_state_watchers",
        );

        // Create a future for each watcher that calls the watcher's
        // `on_terminal_state_transition_started` method and returns the watcher proxy if the
        // response was received within the timeout, or None otherwise. We take this approach so
        // that watchers that timed out have their channel dropped (https://fxbug.dev/42131208).
        let watcher_futures = {
            // Take the current watchers out of `terminal_state_watchers` because we'll be modifying
            // the vector.
            let watchers = self.terminal_state_watchers.lock().await;
            println!("[shutdown-shim] notifying {:?} watchers of terminal state", watchers.len());
            watchers.clone().into_iter().map(|(key, watcher_proxy)| async move {
                let deadline = timeout.after_now();
                let result = watcher_proxy
                    .on_terminal_state_transition_started()
                    .map_err(|_| ())
                    .on_timeout(deadline, || Err(()))
                    .await;

                match result {
                    Ok(()) => Some((key, watcher_proxy)),
                    Err(()) => None,
                }
            })
        };

        // Run all of the futures, collecting the successful watcher proxies into a vector
        let new_watchers = futures::future::join_all(watcher_futures)
            .await
            .into_iter()
            .filter_map(|watcher_opt| watcher_opt) // Unwrap the Options while filtering out None
            .collect();

        // Repopulate the successful watcher proxies back into `terminal_state_watchers`
        *self.terminal_state_watchers.lock().await = new_watchers;
    }
}

struct InspectData {
    reboot_watcher_current_connections: inspect::UintProperty,
    reboot_watcher_total_connections: inspect::UintProperty,
    terminal_state_watcher_current_connections: inspect::UintProperty,
    terminal_state_watcher_total_connections: inspect::UintProperty,
}

// TODO(https://fxbug.dev/414413282): rename nodes to shutdown_watcher_*. This will require also
// updating the inspect selectors. For now, both reboot and shutdown watchers will increment these
// counters.
impl InspectData {
    fn new(parent: &inspect::Node, name: String) -> Self {
        // Create a local root node and properties
        let root = parent.create_child(name);
        let reboot_watcher_current_connections =
            root.create_uint("reboot_watcher_current_connections", 0);
        let reboot_watcher_total_connections =
            root.create_uint("reboot_watcher_total_connections", 0);
        let terminal_state_watcher_current_connections =
            root.create_uint("terminal_state_watcher_current_connections", 0);
        let terminal_state_watcher_total_connections =
            root.create_uint("terminal_state_watcher_total_connections", 0);

        // Pass ownership of the new node to the parent node, otherwise it'll be dropped
        parent.record(root);

        InspectData {
            reboot_watcher_current_connections,
            reboot_watcher_total_connections,
            terminal_state_watcher_current_connections,
            terminal_state_watcher_total_connections,
        }
    }

    fn add_reboot_watcher(&self) {
        self.reboot_watcher_current_connections.add(1);
        self.reboot_watcher_total_connections.add(1);
    }

    fn remove_reboot_watcher(&self) {
        self.reboot_watcher_current_connections.subtract(1);
    }

    fn add_terminal_state_watcher(&self) {
        self.terminal_state_watcher_current_connections.add(1);
        self.terminal_state_watcher_total_connections.add(1);
    }

    fn remove_terminal_state_watcher(&self) {
        self.terminal_state_watcher_current_connections.subtract(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use diagnostics_assertions::assert_data_tree;
    use fidl::endpoints::{ControlHandle, RequestStream};
    use fidl_fuchsia_hardware_power_statecontrol::{ShutdownAction, ShutdownReason};

    // In this test, convert float to integer for simpilification
    fn seconds(seconds: f64) -> zx::MonotonicDuration {
        zx::MonotonicDuration::from_seconds(seconds as i64)
    }

    /// Tests for the presence and correctness of inspect data
    #[fuchsia::test]
    async fn test_inspect_data() {
        let inspector = inspect::Inspector::default();
        let registrar = ShutdownWatcher::new_with_inspector(&inspector);

        assert_data_tree!(
            inspector,
            root: {
                ShutdownWatcher: {
                    reboot_watcher_current_connections: 0u64,
                    reboot_watcher_total_connections: 0u64,
                    terminal_state_watcher_current_connections: 0u64,
                    terminal_state_watcher_total_connections: 0u64,
                }
            }
        );

        let (watcher_proxy, s) = fidl::endpoints::create_proxy::<fpower::RebootWatcherMarker>();
        registrar.add_reboot_watcher(watcher_proxy.clone()).await;

        assert_data_tree!(
            inspector,
            root: {
                ShutdownWatcher: {
                    reboot_watcher_current_connections: 1u64,
                    reboot_watcher_total_connections: 1u64,
                    terminal_state_watcher_current_connections: 0u64,
                    terminal_state_watcher_total_connections: 0u64,
                }
            }
        );

        drop(s);
        watcher_proxy.on_closed().await.expect("closed");

        assert_data_tree!(
            @retry 10,
            inspector,
            root: {
                ShutdownWatcher: {
                    reboot_watcher_current_connections: 0u64,
                    reboot_watcher_total_connections: 1u64,
                    terminal_state_watcher_current_connections: 0u64,
                    terminal_state_watcher_total_connections: 0u64,
                }
            }
        );

        let (watcher_proxy, s) = fidl::endpoints::create_proxy::<fpower::RebootWatcherMarker>();
        registrar.add_reboot_watcher(watcher_proxy.clone()).await;

        assert_data_tree!(
            @retry 10,
            inspector,
            root: {
                ShutdownWatcher: {
                    reboot_watcher_current_connections: 1u64,
                    reboot_watcher_total_connections: 2u64,
                    terminal_state_watcher_current_connections: 0u64,
                    terminal_state_watcher_total_connections: 0u64,
                }
            }
        );
        drop(s);
        watcher_proxy.on_closed().await.expect("closed");

        assert_data_tree!(
            @retry 10,
            inspector,
            root: {
                ShutdownWatcher: {
                    reboot_watcher_current_connections: 0u64,
                    reboot_watcher_total_connections: 2u64,
                    terminal_state_watcher_current_connections: 0u64,
                    terminal_state_watcher_total_connections: 0u64,
                }
            }
        );

        let (watcher_proxy, s) = fidl::endpoints::create_proxy::<fpower::ShutdownWatcherMarker>();
        registrar.add_shutdown_watcher(watcher_proxy.clone()).await;

        assert_data_tree!(
            @retry 10,
            inspector,
            root: {
                ShutdownWatcher: {
                    reboot_watcher_current_connections: 1u64,
                    reboot_watcher_total_connections: 3u64,
                    terminal_state_watcher_current_connections: 0u64,
                    terminal_state_watcher_total_connections: 0u64,
                }
            }
        );
        drop(s);
        watcher_proxy.on_closed().await.expect("closed");

        assert_data_tree!(
            @retry 10,
            inspector,
            root: {
                ShutdownWatcher: {
                    reboot_watcher_current_connections: 0u64,
                    reboot_watcher_total_connections: 3u64,
                    terminal_state_watcher_current_connections: 0u64,
                    terminal_state_watcher_total_connections: 0u64,
                }
            }
        );

        let (watcher_proxy, s) =
            fidl::endpoints::create_proxy::<fpower::TerminalStateWatcherMarker>();
        registrar.add_terminal_state_watcher(watcher_proxy.clone()).await;

        assert_data_tree!(
            @retry 10,
            inspector,
            root: {
                ShutdownWatcher: {
                    reboot_watcher_current_connections: 0u64,
                    reboot_watcher_total_connections: 3u64,
                    terminal_state_watcher_current_connections: 1u64,
                    terminal_state_watcher_total_connections: 1u64,
                }
            }
        );
        drop(s);
        watcher_proxy.on_closed().await.expect("closed");

        assert_data_tree!(
            @retry 10,
            inspector,
            root: {
                ShutdownWatcher: {
                    reboot_watcher_current_connections: 0u64,
                    reboot_watcher_total_connections: 3u64,
                    terminal_state_watcher_current_connections: 0u64,
                    terminal_state_watcher_total_connections: 1u64,
                }
            }
        );
    }

    /// Tests that a client can successfully register a reboot watcher, and the registered watcher
    /// receives the expected reboot notification.
    #[fuchsia::test]
    async fn test_add_reboot_watcher() {
        let registrar = ShutdownWatcher::new();

        // Create the proxy/stream to register the watcher
        let (register_proxy, register_stream) = fidl::endpoints::create_proxy_and_stream::<
            fpower::RebootMethodsWatcherRegisterMarker,
        >();

        // Start the RebootMethodsWatcherRegister server that will handle Register calls from
        // register_proxy
        let registrar_clone = registrar.clone();
        fasync::Task::local(async move {
            registrar_clone.handle_reboot_register_request(register_stream).await;
        })
        .detach();

        // Create the watcher proxy/stream to receive reboot notifications
        let (watcher_client, mut watcher_stream) =
            fidl::endpoints::create_request_stream::<fpower::RebootWatcherMarker>();

        // Call the Register API, passing in the watcher_client end
        assert_matches!(register_proxy.register_watcher(watcher_client).await, Ok(()));
        // Signal the watchers
        registrar
            .notify_reboot_watchers(
                ShutdownOptionsWrapper::new(ShutdownAction::Reboot, ShutdownReason::UserRequest),
                seconds(0.0),
            )
            .await;

        // Verify the watcher_stream gets the correct reboot notification
        let reasons = assert_matches!(
            watcher_stream.try_next().await.unwrap().unwrap(),
            fpower::RebootWatcherRequest::OnReboot {
                options: fpower::RebootOptions{reasons: Some(reasons), ..},
                ..
            } => reasons
        );
        assert_eq!(&reasons[..], [fpower::RebootReason2::UserRequest]);
    }

    /// Tests that a client can successfully register a shutdown watcher, and the registered watcher
    /// receives the expected shutdown notification.
    #[fuchsia::test]
    async fn test_add_shutdown_watcher() {
        let registrar = ShutdownWatcher::new();

        // Create the proxy/stream to register the watcher
        let (register_proxy, register_stream) =
            fidl::endpoints::create_proxy_and_stream::<fpower::ShutdownWatcherRegisterMarker>();

        // Start the ShutdownWatcherRegister server that will handle Register calls from
        // register_proxy.
        let registrar_clone = registrar.clone();
        fasync::Task::local(async move {
            registrar_clone.handle_shutdown_register_request(register_stream).await;
        })
        .detach();

        // Create the watcher proxy/stream to receive shutdown notifications
        let (watcher_client, mut watcher_stream) =
            fidl::endpoints::create_request_stream::<fpower::ShutdownWatcherMarker>();

        // Call the Register API, passing in the watcher_client end
        assert_matches!(register_proxy.register_watcher(watcher_client).await, Ok(()));

        // Signal the watchers
        registrar
            .notify_shutdown_watchers(
                ShutdownOptionsWrapper::new(ShutdownAction::Poweroff, ShutdownReason::UserRequest),
                seconds(0.0),
            )
            .await;

        // Verify the watcher_stream gets the correct shutdown notification
        let (action, reasons) = assert_matches!(
            watcher_stream.try_next().await.unwrap().unwrap(),
            fpower::ShutdownWatcherRequest::OnShutdown {
                options: fpower::ShutdownOptions{action: Some(action), reasons: Some(reasons), ..},
                ..
            } => (action, reasons)
        );
        assert_eq!(action, ShutdownAction::Poweroff);
        assert_eq!(&reasons[..], [ShutdownReason::UserRequest]);
    }

    /// Tests that a client can successfully register a terminal state watcher, and the registered
    /// watcher receives the expected notification.
    #[fuchsia::test]
    async fn test_add_terminal_state_watcher() {
        let registrar = ShutdownWatcher::new();

        // Create the proxy/stream to register the watcher
        let (register_proxy, register_stream) =
            fidl::endpoints::create_proxy_and_stream::<fpower::ShutdownWatcherRegisterMarker>();

        // Start the ShutdownWatcherRegister server that will handle Register calls from
        // register_proxy.
        let registrar_clone = registrar.clone();
        fasync::Task::local(async move {
            registrar_clone.handle_shutdown_register_request(register_stream).await;
        })
        .detach();

        // Create the watcher proxy/stream to receive terminal state notifications
        let (watcher_client, mut watcher_stream) =
            fidl::endpoints::create_request_stream::<fpower::TerminalStateWatcherMarker>();

        // Call the Register API, passing in the watcher_client end
        assert_matches!(
            register_proxy.register_terminal_state_watcher(watcher_client).await,
            Ok(())
        );

        // Signal the watchers
        registrar.notify_terminal_state_watchers(seconds(0.0)).await;

        // Verify the watcher_stream gets the correct notification
        assert_matches!(
            watcher_stream.try_next().await.unwrap().unwrap(),
            fpower::TerminalStateWatcherRequest::OnTerminalStateTransitionStarted { .. }
        );
    }

    /// Tests that a reboot watcher is delivered the correct reboot reason
    #[fuchsia::test]
    async fn test_reboot_watcher_reason() {
        let registrar = ShutdownWatcher::new();
        let (watcher_proxy, mut watcher_stream) =
            fidl::endpoints::create_proxy_and_stream::<fpower::RebootWatcherMarker>();
        registrar.add_reboot_watcher(watcher_proxy).await;
        registrar
            .notify_reboot_watchers(
                ShutdownOptionsWrapper::new(
                    ShutdownAction::Reboot,
                    ShutdownReason::HighTemperature,
                ),
                seconds(0.0),
            )
            .await;

        let reasons = match watcher_stream.try_next().await {
            Ok(Some(fpower::RebootWatcherRequest::OnReboot {
                options: fpower::RebootOptions { reasons: Some(reasons), .. },
                ..
            })) => reasons,
            e => panic!("Unexpected watcher_stream result: {:?}", e),
        };

        assert_eq!(&reasons[..], [fpower::RebootReason2::HighTemperature]);
    }

    /// Tests that a shutdown watcher is delivered the correct shutdown reason. This test does not
    /// register via the ShutdownWatcherRegister protocol.
    #[fuchsia::test]
    async fn test_shutdown_watcher_reason() {
        let registrar = ShutdownWatcher::new();
        let (watcher_proxy, mut watcher_stream) =
            fidl::endpoints::create_proxy_and_stream::<fpower::ShutdownWatcherMarker>();
        registrar.add_shutdown_watcher(watcher_proxy).await;
        registrar
            .notify_shutdown_watchers(
                ShutdownOptionsWrapper::new(
                    ShutdownAction::Poweroff,
                    ShutdownReason::HighTemperature,
                ),
                seconds(0.0),
            )
            .await;

        let (action, reasons) = assert_matches!(
            watcher_stream.try_next().await.unwrap().unwrap(),
            fpower::ShutdownWatcherRequest::OnShutdown {
                options: fpower::ShutdownOptions{action: Some(action), reasons: Some(reasons), ..},
                ..
            } => (action, reasons)
        );
        assert_eq!(action, ShutdownAction::Poweroff);
        assert_eq!(&reasons[..], [ShutdownReason::HighTemperature]);
    }

    /// Tests that if there are multiple registered reboot watchers, each one will receive the
    /// expected reboot notification.
    #[fuchsia::test]
    async fn test_multiple_reboot_watchers() {
        let registrar = ShutdownWatcher::new();

        // Create three separate reboot watchers
        let (watcher_proxy1, mut watcher_stream1) =
            fidl::endpoints::create_proxy_and_stream::<fpower::RebootWatcherMarker>();
        registrar.add_reboot_watcher(watcher_proxy1).await;

        let (watcher_proxy2, mut watcher_stream2) =
            fidl::endpoints::create_proxy_and_stream::<fpower::RebootWatcherMarker>();
        registrar.add_reboot_watcher(watcher_proxy2).await;

        let (watcher_proxy3, mut watcher_stream3) =
            fidl::endpoints::create_proxy_and_stream::<fpower::RebootWatcherMarker>();
        registrar.add_reboot_watcher(watcher_proxy3).await;

        // Close the channel of the first watcher to verify the registrar still correctly notifies the
        // second and third watchers
        watcher_stream1.control_handle().shutdown();

        registrar
            .notify_reboot_watchers(
                ShutdownOptionsWrapper::new(
                    ShutdownAction::Reboot,
                    ShutdownReason::HighTemperature,
                ),
                seconds(0.0),
            )
            .await;

        // The first watcher should get None because its channel was closed
        match watcher_stream1.try_next().await {
            Ok(None) => {}
            e => panic!("Unexpected watcher_stream1 result: {:?}", e),
        };

        // Verify the watcher received the correct OnReboot request
        match watcher_stream2.try_next().await {
            Ok(Some(fpower::RebootWatcherRequest::OnReboot {
                options: fpower::RebootOptions { reasons: Some(reasons), .. },
                ..
            })) => {
                assert_eq!(&reasons[..], [fpower::RebootReason2::HighTemperature])
            }
            e => panic!("Unexpected watcher_stream2 result: {:?}", e),
        };

        // Verify the watcher received the correct OnReboot request
        match watcher_stream3.try_next().await {
            Ok(Some(fpower::RebootWatcherRequest::OnReboot {
                options: fpower::RebootOptions { reasons: Some(reasons), .. },
                ..
            })) => assert_eq!(&reasons[..], [fpower::RebootReason2::HighTemperature]),
            e => panic!("Unexpected watcher_stream3 result: {:?}", e),
        };
    }

    /// Tests that if there are multiple registered shutdown watchers, each one will receive the
    /// expected shutdown notification.
    #[fuchsia::test]
    async fn test_multiple_shutdown_watchers() {
        let registrar = ShutdownWatcher::new();

        // Create three separate shutdown watchers
        let (watcher_proxy1, mut watcher_stream1) =
            fidl::endpoints::create_proxy_and_stream::<fpower::ShutdownWatcherMarker>();
        registrar.add_shutdown_watcher(watcher_proxy1).await;

        let (watcher_proxy2, mut watcher_stream2) =
            fidl::endpoints::create_proxy_and_stream::<fpower::ShutdownWatcherMarker>();
        registrar.add_shutdown_watcher(watcher_proxy2).await;

        let (watcher_proxy3, mut watcher_stream3) =
            fidl::endpoints::create_proxy_and_stream::<fpower::ShutdownWatcherMarker>();
        registrar.add_shutdown_watcher(watcher_proxy3).await;

        // Close the channel of the first watcher to verify the registrar still correctly notifies
        // the second and third watchers.
        watcher_stream1.control_handle().shutdown();

        registrar
            .notify_shutdown_watchers(
                ShutdownOptionsWrapper::new(
                    ShutdownAction::Reboot,
                    ShutdownReason::HighTemperature,
                ),
                seconds(0.0),
            )
            .await;

        // The first watcher should get None because its channel was closed
        match watcher_stream1.try_next().await {
            Ok(None) => {}
            e => panic!("Unexpected watcher_stream1 result: {:?}", e),
        };

        // Verify the watcher received the correct OnShutdown request
        let (action, reasons) = assert_matches!(
            watcher_stream2.try_next().await.unwrap().unwrap(),
            fpower::ShutdownWatcherRequest::OnShutdown {
                options: fpower::ShutdownOptions{action: Some(action), reasons: Some(reasons), ..},
                ..
            } => (action, reasons)
        );
        assert_eq!(action, ShutdownAction::Reboot);
        assert_eq!(&reasons[..], [ShutdownReason::HighTemperature]);

        // Verify the watcher received the correct OnShutdown request
        let (action, reasons) = assert_matches!(
            watcher_stream3.try_next().await.unwrap().unwrap(),
            fpower::ShutdownWatcherRequest::OnShutdown {
                options: fpower::ShutdownOptions{action: Some(action), reasons: Some(reasons), ..},
                ..
            } => (action, reasons)
        );
        assert_eq!(action, ShutdownAction::Reboot);
        assert_eq!(&reasons[..], [ShutdownReason::HighTemperature]);
    }

    /// Tests that the deprecated reboot watchers only receive notifications for
    /// ShutdownAction::Reboot.
    #[fuchsia::test]
    async fn reboot_watchers_only_notified_of_reboots() {
        let registrar = ShutdownWatcher::new();
        let (watcher_proxy, mut watcher_stream) =
            fidl::endpoints::create_proxy_and_stream::<fpower::RebootWatcherMarker>();
        registrar.add_reboot_watcher(watcher_proxy).await;

        registrar
            .handle_system_shutdown_message(Some(ShutdownOptionsWrapper::new(
                ShutdownAction::Poweroff,
                ShutdownReason::HighTemperature,
            )))
            .await;
        assert!(futures::poll!(watcher_stream.try_next()).is_pending());

        registrar
            .handle_system_shutdown_message(Some(ShutdownOptionsWrapper::new(
                ShutdownAction::RebootToBootloader,
                ShutdownReason::HighTemperature,
            )))
            .await;
        assert!(futures::poll!(watcher_stream.try_next()).is_pending());

        registrar
            .handle_system_shutdown_message(Some(ShutdownOptionsWrapper::new(
                ShutdownAction::RebootToRecovery,
                ShutdownReason::HighTemperature,
            )))
            .await;
        assert!(futures::poll!(watcher_stream.try_next()).is_pending());

        registrar
            .handle_system_shutdown_message(Some(ShutdownOptionsWrapper::new(
                ShutdownAction::Reboot,
                ShutdownReason::HighTemperature,
            )))
            .await;
        match watcher_stream.try_next().await {
            Ok(Some(fpower::RebootWatcherRequest::OnReboot {
                options: fpower::RebootOptions { reasons: Some(reasons), .. },
                ..
            })) => {
                assert_eq!(&reasons[..], [fpower::RebootReason2::HighTemperature])
            }
            e => panic!("Unexpected watcher_stream result: {:?}", e),
        };
    }

    /// Tests that shutdown, terminal state, and reboot watchers are notified of reboots.
    /// Shutdown watchers are notified of other shutdown actions.
    /// Terminal state watchers are notified of all reboot or shutdown actions.
    #[fuchsia::test]
    async fn shutdown_terminal_state_and_reboot_watchers_notified() {
        let registrar = ShutdownWatcher::new();

        let (shutdown_watcher_proxy, mut shutdown_watcher_stream) =
            fidl::endpoints::create_proxy_and_stream::<fpower::ShutdownWatcherMarker>();
        registrar.add_shutdown_watcher(shutdown_watcher_proxy).await;

        let (reboot_watcher_proxy, mut reboot_watcher_stream) =
            fidl::endpoints::create_proxy_and_stream::<fpower::RebootWatcherMarker>();
        registrar.add_reboot_watcher(reboot_watcher_proxy).await;

        let (terminal_state_watcher_proxy, mut terminal_state_watcher_stream) =
            fidl::endpoints::create_proxy_and_stream::<fpower::TerminalStateWatcherMarker>();
        registrar.add_terminal_state_watcher(terminal_state_watcher_proxy).await;

        registrar
            .handle_system_shutdown_message(Some(ShutdownOptionsWrapper::new(
                ShutdownAction::Reboot,
                ShutdownReason::HighTemperature,
            )))
            .await;

        let (action, reasons) = assert_matches!(
            shutdown_watcher_stream.try_next().await.unwrap().unwrap(),
            fpower::ShutdownWatcherRequest::OnShutdown {
                options: fpower::ShutdownOptions{action: Some(action), reasons: Some(reasons), ..},
                ..
            } => (action, reasons)
        );
        assert_eq!(action, ShutdownAction::Reboot);
        assert_eq!(&reasons[..], [ShutdownReason::HighTemperature]);

        match reboot_watcher_stream.try_next().await {
            Ok(Some(fpower::RebootWatcherRequest::OnReboot {
                options: fpower::RebootOptions { reasons: Some(reasons), .. },
                ..
            })) => {
                assert_eq!(&reasons[..], [fpower::RebootReason2::HighTemperature])
            }
            e => panic!("Unexpected watcher_stream result: {:?}", e),
        };

        assert_matches!(
            terminal_state_watcher_stream.try_next().await.unwrap().unwrap(),
            fpower::TerminalStateWatcherRequest::OnTerminalStateTransitionStarted { .. }
        );
    }

    #[fuchsia::test]
    async fn shutdown_watchers_notified_for_poweroff() {
        let registrar = ShutdownWatcher::new();

        let (watcher_proxy, mut watcher_stream) =
            fidl::endpoints::create_proxy_and_stream::<fpower::ShutdownWatcherMarker>();
        registrar.add_shutdown_watcher(watcher_proxy).await;

        registrar
            .handle_system_shutdown_message(Some(ShutdownOptionsWrapper::new(
                ShutdownAction::Poweroff,
                ShutdownReason::HighTemperature,
            )))
            .await;
        let (action, reasons) = assert_matches!(
            watcher_stream.try_next().await.unwrap().unwrap(),
            fpower::ShutdownWatcherRequest::OnShutdown {
                options: fpower::ShutdownOptions{action: Some(action), reasons: Some(reasons), ..},
                ..
            } => (action, reasons)
        );
        assert_eq!(action, ShutdownAction::Poweroff);
        assert_eq!(&reasons[..], [ShutdownReason::HighTemperature]);
    }

    #[fuchsia::test]
    async fn shutdown_watchers_notified_for_reboot_to_bootloader() {
        let registrar = ShutdownWatcher::new();

        let (watcher_proxy, mut watcher_stream) =
            fidl::endpoints::create_proxy_and_stream::<fpower::ShutdownWatcherMarker>();
        registrar.add_shutdown_watcher(watcher_proxy).await;

        registrar
            .handle_system_shutdown_message(Some(ShutdownOptionsWrapper::new(
                ShutdownAction::RebootToBootloader,
                ShutdownReason::HighTemperature,
            )))
            .await;
        let (action, reasons) = assert_matches!(
            watcher_stream.try_next().await.unwrap().unwrap(),
            fpower::ShutdownWatcherRequest::OnShutdown {
                options: fpower::ShutdownOptions{action: Some(action), reasons: Some(reasons), ..},
                ..
            } => (action, reasons)
        );
        assert_eq!(action, ShutdownAction::RebootToBootloader);
        assert_eq!(&reasons[..], [ShutdownReason::HighTemperature]);
    }

    #[fuchsia::test]
    async fn shutdown_watchers_notified_for_reboot_to_recovery() {
        let registrar = ShutdownWatcher::new();

        let (watcher_proxy, mut watcher_stream) =
            fidl::endpoints::create_proxy_and_stream::<fpower::ShutdownWatcherMarker>();
        registrar.add_shutdown_watcher(watcher_proxy).await;

        registrar
            .handle_system_shutdown_message(Some(ShutdownOptionsWrapper::new(
                ShutdownAction::RebootToRecovery,
                ShutdownReason::HighTemperature,
            )))
            .await;
        let (action, reasons) = assert_matches!(
            watcher_stream.try_next().await.unwrap().unwrap(),
            fpower::ShutdownWatcherRequest::OnShutdown {
                options: fpower::ShutdownOptions{action: Some(action), reasons: Some(reasons), ..},
                ..
            } => (action, reasons)
        );
        assert_eq!(action, ShutdownAction::RebootToRecovery);
        assert_eq!(&reasons[..], [ShutdownReason::HighTemperature]);
    }

    #[fuchsia::test]
    fn test_reboot_watcher_response_delay() {
        let mut exec = fasync::TestExecutor::new();
        let registrar = ShutdownWatcher::new();

        // Register the reboot watcher
        let (watcher_proxy, mut watcher_stream) =
            fidl::endpoints::create_proxy_and_stream::<fpower::RebootWatcherMarker>();
        let fut = async {
            registrar.add_reboot_watcher(watcher_proxy).await;
            assert_eq!(registrar.reboot_watchers.lock().await.len(), 1);
        };
        exec.run_singlethreaded(fut);

        // Set up the notify future
        let notify_future = registrar.notify_reboot_watchers(
            ShutdownOptionsWrapper::new(ShutdownAction::Reboot, ShutdownReason::HighTemperature),
            seconds(1.0),
        );
        futures::pin_mut!(notify_future);

        // Verify that the notify future can't complete on the first attempt (because the watcher
        // will not have responded)
        assert!(exec.run_until_stalled(&mut notify_future).is_pending());

        // Ack the reboot notification, allowing the shutdown flow to continue
        let fpower::RebootWatcherRequest::OnReboot { responder, .. } =
            exec.run_singlethreaded(&mut watcher_stream.try_next()).unwrap().unwrap();
        assert_matches!(responder.send(), Ok(()));

        // Verify the notify future can now complete
        assert!(exec.run_until_stalled(&mut notify_future).is_ready());
    }

    #[fuchsia::test]
    fn test_shutdown_watcher_response_delay() {
        let mut exec = fasync::TestExecutor::new();
        let registrar = ShutdownWatcher::new();

        // Register the shutdown watcher
        let (watcher_proxy, mut watcher_stream) =
            fidl::endpoints::create_proxy_and_stream::<fpower::ShutdownWatcherMarker>();
        let fut = async {
            registrar.add_shutdown_watcher(watcher_proxy).await;
            assert_eq!(registrar.shutdown_watchers.lock().await.len(), 1);
        };
        exec.run_singlethreaded(fut);

        // Set up the notify future
        let notify_future = registrar.notify_shutdown_watchers(
            ShutdownOptionsWrapper::new(ShutdownAction::Reboot, ShutdownReason::HighTemperature),
            seconds(1.0),
        );
        futures::pin_mut!(notify_future);

        // Verify that the notify future can't complete on the first attempt (because the watcher
        // will not have responded)
        assert!(exec.run_until_stalled(&mut notify_future).is_pending());

        // Ack the shutdown notification, allowing the shutdown flow to continue
        match exec.run_singlethreaded(&mut watcher_stream.try_next()).unwrap().unwrap() {
            fpower::ShutdownWatcherRequest::OnShutdown { responder, .. } => {
                assert_matches!(responder.send(), Ok(()));
            }
            fpower::ShutdownWatcherRequest::_UnknownMethod { .. } => unimplemented!(),
        }

        // Verify the notify future can now complete
        assert!(exec.run_until_stalled(&mut notify_future).is_ready());
    }

    /// Tests that a reboot watcher is able to delay the shutdown but will time out after the
    /// expected duration. The test also verifies that when a watcher times out, it is removed
    /// from the list of registered reboot watchers.
    #[fuchsia::test]
    fn test_reboot_watcher_response_timeout() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let registrar = ShutdownWatcher::new();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(0));

        // Register the reboot watcher
        let (watcher_proxy, _watcher_stream) =
            fidl::endpoints::create_proxy_and_stream::<fpower::RebootWatcherMarker>();
        let fut = async {
            registrar.add_reboot_watcher(watcher_proxy).await;
            assert_eq!(registrar.reboot_watchers.lock().await.len(), 1);
        };
        futures::pin_mut!(fut);
        exec.run_until_stalled(&mut fut).is_ready();

        // Set up the notify future
        let notify_future = registrar.notify_reboot_watchers(
            ShutdownOptionsWrapper::new(ShutdownAction::Reboot, ShutdownReason::HighTemperature),
            seconds(1.0),
        );
        futures::pin_mut!(notify_future);

        // Verify that the notify future can't complete on the first attempt (because the watcher
        // will not have responded)
        assert!(exec.run_until_stalled(&mut notify_future).is_pending());

        // Wake the timer that causes the watcher timeout to fire
        assert_eq!(exec.wake_next_timer(), Some(fasync::MonotonicInstant::from_nanos(1e9 as i64)));

        // Verify the notify future can now complete
        assert!(exec.run_until_stalled(&mut notify_future).is_ready());

        // Since the watcher timed out, verify it is removed from `reboot_watchers`
        let fut = async {
            assert_eq!(registrar.reboot_watchers.lock().await.len(), 0);
        };
        futures::pin_mut!(fut);
        exec.run_until_stalled(&mut fut).is_ready();
    }

    /// Tests that a shutdown watcher is able to delay the shutdown but will time out after the
    /// expected duration. The test also verifies that when a watcher times out, it is removed
    /// from the list of registered shutdown watchers.
    #[fuchsia::test]
    fn test_shutdown_watcher_response_timeout() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let registrar = ShutdownWatcher::new();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(0));

        // Register the shutdown watcher
        let (watcher_proxy, _watcher_stream) =
            fidl::endpoints::create_proxy_and_stream::<fpower::ShutdownWatcherMarker>();
        let fut = async {
            registrar.add_shutdown_watcher(watcher_proxy).await;
            assert_eq!(registrar.shutdown_watchers.lock().await.len(), 1);
        };
        futures::pin_mut!(fut);
        exec.run_until_stalled(&mut fut).is_ready();

        // Set up the notify future
        let notify_future = registrar.notify_shutdown_watchers(
            ShutdownOptionsWrapper::new(ShutdownAction::Reboot, ShutdownReason::HighTemperature),
            seconds(1.0),
        );
        futures::pin_mut!(notify_future);

        // Verify that the notify future can't complete on the first attempt (because the watcher
        // will not have responded)
        assert!(exec.run_until_stalled(&mut notify_future).is_pending());

        // Wake the timer that causes the watcher timeout to fire
        assert_eq!(exec.wake_next_timer(), Some(fasync::MonotonicInstant::from_nanos(1e9 as i64)));

        // Verify the notify future can now complete
        assert!(exec.run_until_stalled(&mut notify_future).is_ready());

        // Since the watcher timed out, verify it is removed from `shutdown_watchers`
        let fut = async {
            assert_eq!(registrar.shutdown_watchers.lock().await.len(), 0);
        };
        futures::pin_mut!(fut);
        exec.run_until_stalled(&mut fut).is_ready();
    }

    /// Tests that an unsuccessful RebootWatcher registration results in the
    /// RebootMethodsWatcherRegister channel being closed.
    #[fuchsia::test]
    async fn test_watcher_register_fail() {
        let registrar = ShutdownWatcher::new();

        // Create the registration proxy/stream
        let (register_proxy, register_stream) = fidl::endpoints::create_proxy_and_stream::<
            fpower::RebootMethodsWatcherRegisterMarker,
        >();

        // Start the RebootMethodsWatcherRegister server that will handle Register requests from
        // `register_proxy`
        fasync::Task::local(async move {
            registrar.handle_reboot_register_request(register_stream).await;
        })
        .detach();

        // Send an invalid request to the server to force a failure
        assert_matches!(register_proxy.as_channel().write(&[], &mut []), Ok(()));

        // Verify the RebootMethodsWatcherRegister channel is closed
        assert_matches!(register_proxy.on_closed().await, Ok(zx::Signals::CHANNEL_PEER_CLOSED));
    }

    /// Tests that an unsuccessful ShutdownWatcher registration results in the
    /// ShutdownWatcherRegister channel being closed.
    #[fuchsia::test]
    async fn test_shutdown_watcher_register_fail() {
        let registrar = ShutdownWatcher::new();

        // Create the registration proxy/stream
        let (register_proxy, register_stream) =
            fidl::endpoints::create_proxy_and_stream::<fpower::ShutdownWatcherRegisterMarker>();

        // Start the ShutdownWatcherRegister server that will handle Register requests from
        // `register_proxy`
        fasync::Task::local(async move {
            registrar.handle_shutdown_register_request(register_stream).await;
        })
        .detach();

        // Send an invalid request to the server to force a failure
        assert_matches!(register_proxy.as_channel().write(&[], &mut []), Ok(()));

        // Verify the ShutdownWatcherRegister channel is closed
        assert_matches!(register_proxy.on_closed().await, Ok(zx::Signals::CHANNEL_PEER_CLOSED));
    }

    /// Tests that if there are multiple currently registered terminal state watchers, each one will
    /// receive the expected notification.
    #[fuchsia::test]
    async fn test_multiple_terminal_state_watchers() {
        let registrar = ShutdownWatcher::new();

        // Create three separate terminal state watchers
        let (watcher_proxy1, mut watcher_stream1) =
            fidl::endpoints::create_proxy_and_stream::<fpower::TerminalStateWatcherMarker>();
        registrar.add_terminal_state_watcher(watcher_proxy1).await;

        let (watcher_proxy2, mut watcher_stream2) =
            fidl::endpoints::create_proxy_and_stream::<fpower::TerminalStateWatcherMarker>();
        registrar.add_terminal_state_watcher(watcher_proxy2).await;

        let (watcher_proxy3, mut watcher_stream3) =
            fidl::endpoints::create_proxy_and_stream::<fpower::TerminalStateWatcherMarker>();
        registrar.add_terminal_state_watcher(watcher_proxy3).await;

        // Signal the watchers
        registrar.notify_terminal_state_watchers(seconds(0.0)).await;

        // Verify each watcher stream gets the correct notification
        assert_matches!(
            watcher_stream1.try_next().await.unwrap().unwrap(),
            fpower::TerminalStateWatcherRequest::OnTerminalStateTransitionStarted { .. }
        );
        assert_matches!(
            watcher_stream2.try_next().await.unwrap().unwrap(),
            fpower::TerminalStateWatcherRequest::OnTerminalStateTransitionStarted { .. }
        );
        assert_matches!(
            watcher_stream3.try_next().await.unwrap().unwrap(),
            fpower::TerminalStateWatcherRequest::OnTerminalStateTransitionStarted { .. }
        );
    }

    #[fuchsia::test]
    fn test_terminal_state_watcher_response_delay() {
        let mut exec = fasync::TestExecutor::new();
        let registrar = ShutdownWatcher::new();

        // Register the terminal state watcher
        let (watcher_proxy, mut watcher_stream) =
            fidl::endpoints::create_proxy_and_stream::<fpower::TerminalStateWatcherMarker>();
        let fut = async {
            registrar.add_terminal_state_watcher(watcher_proxy).await;
            assert_eq!(registrar.terminal_state_watchers.lock().await.len(), 1);
        };
        exec.run_singlethreaded(fut);

        // Set up the notify future
        let notify_future = registrar.notify_terminal_state_watchers(seconds(1.0));
        futures::pin_mut!(notify_future);

        // Verify that the notify future can't complete on the first attempt (because the watcher
        // will not have responded)
        assert!(exec.run_until_stalled(&mut notify_future).is_pending());

        // Ack the notification, allowing the shutdown flow to continue
        match exec.run_singlethreaded(&mut watcher_stream.try_next()).unwrap().unwrap() {
            fpower::TerminalStateWatcherRequest::OnTerminalStateTransitionStarted {
                responder,
                ..
            } => {
                assert_matches!(responder.send(), Ok(()));
            }
            fpower::TerminalStateWatcherRequest::_UnknownMethod { .. } => unimplemented!(),
        }

        // Verify the notify future can now complete
        assert!(exec.run_until_stalled(&mut notify_future).is_ready());
    }

    /// Tests that a terminal state watcher is able to delay the shutdown but will time out after the
    /// expected duration. The test also verifies that when a watcher times out, it is removed
    /// from the list of registered terminal state watchers.
    #[fuchsia::test]
    fn test_terminal_state_watcher_response_timeout() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let registrar = ShutdownWatcher::new();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(0));

        // Register the terminal state watcher
        let (watcher_proxy, _watcher_stream) =
            fidl::endpoints::create_proxy_and_stream::<fpower::TerminalStateWatcherMarker>();
        let fut = async {
            registrar.add_terminal_state_watcher(watcher_proxy).await;
            assert_eq!(registrar.terminal_state_watchers.lock().await.len(), 1);
        };
        futures::pin_mut!(fut);
        exec.run_until_stalled(&mut fut).is_ready();

        // Set up the notify future
        let notify_future = registrar.notify_terminal_state_watchers(seconds(1.0));
        futures::pin_mut!(notify_future);

        // Verify that the notify future can't complete on the first attempt (because the watcher
        // will not have responded)
        assert!(exec.run_until_stalled(&mut notify_future).is_pending());

        // Wake the timer that causes the watcher timeout to fire
        assert_eq!(exec.wake_next_timer(), Some(fasync::MonotonicInstant::from_nanos(1e9 as i64)));

        // Verify the notify future can now complete
        assert!(exec.run_until_stalled(&mut notify_future).is_ready());

        // Since the watcher timed out, verify it is removed from `terminal_state_watchers`
        let fut = async {
            assert_eq!(registrar.terminal_state_watchers.lock().await.len(), 0);
        };
        futures::pin_mut!(fut);
        exec.run_until_stalled(&mut fut).is_ready();
    }
}

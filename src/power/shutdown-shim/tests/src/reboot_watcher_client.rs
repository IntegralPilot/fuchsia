// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_component_test::RealmInstance;
use futures::channel::mpsc;
use futures::{StreamExt, TryStreamExt};
use log::*;
use {fidl_fuchsia_hardware_power_statecontrol as fpower, fuchsia_async as fasync};

/// Convenience type for interacting with the Power Manager's RebootWatcher service.
pub struct RebootWatcherClient {
    _watcher_task: fasync::Task<()>,
    reboot_options_receiver: mpsc::Receiver<fpower::RebootOptions>,
}

impl RebootWatcherClient {
    pub async fn new(realm: &RealmInstance) -> Self {
        let (mut reboot_options_sender, reboot_options_receiver) = mpsc::channel(1);

        // Create a new watcher proxy/stream and register the proxy end with Power Manager
        let watcher_register_proxy: fpower::RebootMethodsWatcherRegisterProxy = realm
            .root
            .connect_to_protocol_at_exposed_dir()
            .expect("Should be able to connect the proxy correctly");
        let (watcher_client, mut watcher_request_stream) =
            fidl::endpoints::create_request_stream::<fpower::RebootWatcherMarker>();
        watcher_register_proxy
            .register_watcher(watcher_client)
            .await
            .expect("Failed to register reboot watcher");

        let _watcher_task = fasync::Task::local(async move {
            while let Some(fpower::RebootWatcherRequest::OnReboot { options, responder }) =
                watcher_request_stream.try_next().await.unwrap()
            {
                info!("Received reboot options: {:?}", options);
                let _ = responder.send();
                reboot_options_sender.try_send(options).expect("Failed to notify reboot reason");
            }
        });

        Self { _watcher_task, reboot_options_receiver }
    }

    /// Returns the next reboot reason that the reboot watcher has received, or hangs until one is
    /// received.
    pub async fn get_reboot_options(&mut self) -> fpower::RebootOptions {
        self.reboot_options_receiver.next().await.expect("Failed to wait for reboot reason")
    }
}

/// Convenience type for interacting with the Power Manager's ShutdownWatcher service.
pub struct ShutdownWatcherClient {
    _watcher_task: fasync::Task<()>,
    shutdown_options_receiver: mpsc::Receiver<fpower::ShutdownOptions>,
}

impl ShutdownWatcherClient {
    pub async fn new(realm: &RealmInstance) -> Self {
        let (mut shutdown_options_sender, shutdown_options_receiver) = mpsc::channel(1);

        // Create a new watcher proxy/stream and register the proxy end with shutdown-shim.
        let watcher_register_proxy: fpower::ShutdownWatcherRegisterProxy = realm
            .root
            .connect_to_protocol_at_exposed_dir()
            .expect("Should be able to connect the proxy correctly");
        let (watcher_client, mut watcher_request_stream) =
            fidl::endpoints::create_request_stream::<fpower::ShutdownWatcherMarker>();
        watcher_register_proxy
            .register_watcher(watcher_client)
            .await
            .expect("Failed to register shutdown watcher");

        let _watcher_task = fasync::Task::local(async move {
            while let Some(fpower::ShutdownWatcherRequest::OnShutdown { options, responder }) =
                watcher_request_stream.try_next().await.unwrap()
            {
                info!("Received shutdown options: {:?}", options);
                let _ = responder.send();
                shutdown_options_sender
                    .try_send(options)
                    .expect("Failed to notify shutdown reason");
            }
        });

        Self { _watcher_task, shutdown_options_receiver }
    }

    /// Returns the next shutdown options that the shutdown watcher has received, or hangs until one
    /// is received.
    pub async fn get_shutdown_options(&mut self) -> fpower::ShutdownOptions {
        self.shutdown_options_receiver.next().await.expect("Failed to wait for shutdown options")
    }
}

/// Convenience type for interacting with the Power Manager's TerminalStateWatcher service.
pub struct TerminalStateWatcherClient {
    _watcher_task: fasync::Task<()>,
    terminal_state_transition_started_receiver: mpsc::Receiver<()>,
}

impl TerminalStateWatcherClient {
    pub async fn new(realm: &RealmInstance) -> Self {
        let (
            mut terminal_state_transition_started_sender,
            terminal_state_transition_started_receiver,
        ) = mpsc::channel(1);

        // Create a new watcher proxy/stream and register the proxy end with shutdown-shim.
        let watcher_register_proxy: fpower::ShutdownWatcherRegisterProxy = realm
            .root
            .connect_to_protocol_at_exposed_dir()
            .expect("Should be able to connect the proxy correctly");
        let (watcher_client, mut watcher_request_stream) =
            fidl::endpoints::create_request_stream::<fpower::TerminalStateWatcherMarker>();
        watcher_register_proxy
            .register_terminal_state_watcher(watcher_client)
            .await
            .expect("Failed to register terminal state watcher");

        let _watcher_task = fasync::Task::local(async move {
            while let Some(
                fpower::TerminalStateWatcherRequest::OnTerminalStateTransitionStarted { responder },
            ) = watcher_request_stream.try_next().await.unwrap()
            {
                info!("Received terminal state transition started");
                let _ = responder.send();
                terminal_state_transition_started_sender
                    .try_send(())
                    .expect("Failed to notify terminal state transition started");
            }
        });

        Self { _watcher_task, terminal_state_transition_started_receiver }
    }

    pub async fn wait_for_terminal_state_transition_started(&mut self) {
        self.terminal_state_transition_started_receiver
            .next()
            .await
            .expect("Failed to wait for terminal state transition started");
    }
}

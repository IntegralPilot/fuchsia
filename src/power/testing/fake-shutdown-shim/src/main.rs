// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use fidl_fuchsia_hardware_power_statecontrol as fstatecontrol;
use fuchsia_component::server::ServiceFs;
use futures::StreamExt;
use futures::lock::Mutex;
use log::{error, info, warn};
use std::sync::Arc;

enum IncomingRequest {
    Admin(fstatecontrol::AdminRequestStream),
    ShutdownWatcherRegister(fstatecontrol::ShutdownWatcherRegisterRequestStream),
}

struct State {
    shutdown_watchers: Vec<fstatecontrol::ShutdownWatcherProxy>,
    terminal_state_watchers: Vec<fstatecontrol::TerminalStateWatcherProxy>,
}

impl State {
    fn new() -> Self {
        Self { shutdown_watchers: Vec::new(), terminal_state_watchers: Vec::new() }
    }

    async fn notify_shutdown(&mut self, options: &fstatecontrol::ShutdownOptions) {
        info!("Notifying {} shutdown watchers", self.shutdown_watchers.len());
        for watcher in &self.shutdown_watchers {
            if let Err(e) = watcher.on_shutdown(options).await {
                warn!("Failed to notify shutdown watcher: {:?}", e);
            }
        }
    }

    async fn notify_terminal_state(&mut self) {
        info!("Notifying {} terminal state watchers", self.terminal_state_watchers.len());
        for watcher in &self.terminal_state_watchers {
            if let Err(e) = watcher.on_terminal_state_transition_started().await {
                warn!("Failed to notify terminal state watcher: {:?}", e);
            }
        }
    }
}

async fn handle_admin_request(
    mut stream: fstatecontrol::AdminRequestStream,
    state: Arc<Mutex<State>>,
) {
    while let Some(request) = stream.next().await {
        match request {
            Ok(fstatecontrol::AdminRequest::Shutdown { options, responder }) => {
                info!("Received Admin.Shutdown");
                let mut state = state.lock().await;
                state.notify_terminal_state().await;
                state.notify_shutdown(&options).await;
                let _ = responder.send(Ok(()));
            }
            Ok(fstatecontrol::AdminRequest::PerformReboot { options: _, responder }) => {
                info!("Received Admin.PerformReboot");
                let mut state = state.lock().await;
                state.notify_terminal_state().await;
                let _ = responder.send(Ok(()));
            }
            Err(e) => {
                error!("Error handling Admin request: {:?}", e);
            }
            Ok(_) => {
                error!("Unsupported Admin request");
            }
        }
    }
}

async fn handle_shutdown_watcher_register_request(
    mut stream: fstatecontrol::ShutdownWatcherRegisterRequestStream,
    state: Arc<Mutex<State>>,
) {
    while let Some(request) = stream.next().await {
        match request {
            Ok(fstatecontrol::ShutdownWatcherRegisterRequest::RegisterWatcher {
                watcher,
                responder,
            }) => {
                info!("Received ShutdownWatcherRegister.RegisterWatcher");
                state.lock().await.shutdown_watchers.push(watcher.into_proxy());
                let _ = responder.send();
            }
            Ok(fstatecontrol::ShutdownWatcherRegisterRequest::RegisterTerminalStateWatcher {
                watcher,
                responder,
            }) => {
                info!("Received ShutdownWatcherRegister.RegisterTerminalStateWatcher");
                state.lock().await.terminal_state_watchers.push(watcher.into_proxy());
                let _ = responder.send();
            }
            Err(e) => {
                error!("Error handling ShutdownWatcherRegister request: {:?}", e);
            }
            Ok(_) => {
                error!("Unsupported ShutdownWatcherRegister request");
            }
        }
    }
}

#[fuchsia::main]
async fn main() -> Result<()> {
    info!("Starting fake-shutdown-shim");

    let state = Arc::new(Mutex::new(State::new()));
    let mut fs = ServiceFs::new_local();

    fs.dir("svc").add_fidl_service(IncomingRequest::Admin);
    fs.dir("svc").add_fidl_service(IncomingRequest::ShutdownWatcherRegister);

    fs.take_and_serve_directory_handle().context("failed to serve outgoing namespace")?;

    fs.for_each_concurrent(None, move |request: IncomingRequest| {
        let state = state.clone();
        async move {
            match request {
                IncomingRequest::Admin(stream) => {
                    handle_admin_request(stream, state).await;
                }
                IncomingRequest::ShutdownWatcherRegister(stream) => {
                    handle_shutdown_watcher_register_request(stream, state).await;
                }
            }
        }
    })
    .await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::{create_proxy_and_stream, create_request_stream};
    use fuchsia_async as fasync;

    #[fuchsia::test]
    async fn test_admin_shutdown_notifies_watchers() {
        let state = Arc::new(Mutex::new(State::new()));

        let (admin_proxy, admin_stream) = create_proxy_and_stream::<fstatecontrol::AdminMarker>();
        let admin_state = state.clone();
        fasync::Task::spawn(async move {
            handle_admin_request(admin_stream, admin_state).await;
        })
        .detach();

        let (register_proxy, register_stream) =
            create_proxy_and_stream::<fstatecontrol::ShutdownWatcherRegisterMarker>();
        let register_state = state.clone();
        fasync::Task::spawn(async move {
            handle_shutdown_watcher_register_request(register_stream, register_state).await;
        })
        .detach();

        let (shutdown_client, mut shutdown_stream) =
            create_request_stream::<fstatecontrol::ShutdownWatcherMarker>();
        register_proxy.register_watcher(shutdown_client).await.unwrap();

        let (terminal_client, mut terminal_stream) =
            create_request_stream::<fstatecontrol::TerminalStateWatcherMarker>();
        register_proxy.register_terminal_state_watcher(terminal_client).await.unwrap();

        let options = fstatecontrol::ShutdownOptions {
            action: Some(fstatecontrol::ShutdownAction::Reboot),
            reasons: Some(vec![fstatecontrol::ShutdownReason::UserRequest]),
            ..Default::default()
        };
        let shutdown_fut = admin_proxy.shutdown(&options);

        let req = terminal_stream.next().await.unwrap().unwrap();
        match req {
            fstatecontrol::TerminalStateWatcherRequest::OnTerminalStateTransitionStarted {
                responder,
            } => {
                responder.send().unwrap();
            }
            _ => panic!("Unexpected request"),
        }

        let req = shutdown_stream.next().await.unwrap().unwrap();
        match req {
            fstatecontrol::ShutdownWatcherRequest::OnShutdown { options: _, responder } => {
                responder.send().unwrap();
            }
            _ => panic!("Unexpected request"),
        }

        shutdown_fut.await.unwrap().unwrap();
    }

    #[fuchsia::test]
    async fn test_admin_reboot_notifies_terminal_watcher() {
        let state = Arc::new(Mutex::new(State::new()));
        let (admin_proxy, admin_stream) = create_proxy_and_stream::<fstatecontrol::AdminMarker>();
        let admin_state = state.clone();
        fasync::Task::spawn(async move {
            handle_admin_request(admin_stream, admin_state).await;
        })
        .detach();

        let (register_proxy, register_stream) =
            create_proxy_and_stream::<fstatecontrol::ShutdownWatcherRegisterMarker>();
        let register_state = state.clone();
        fasync::Task::spawn(async move {
            handle_shutdown_watcher_register_request(register_stream, register_state).await;
        })
        .detach();

        let (terminal_client, mut terminal_stream) =
            create_request_stream::<fstatecontrol::TerminalStateWatcherMarker>();
        register_proxy.register_terminal_state_watcher(terminal_client).await.unwrap();

        let options = fstatecontrol::RebootOptions {
            reasons: Some(vec![fstatecontrol::RebootReason2::UserRequest]),
            ..Default::default()
        };
        let reboot_fut = admin_proxy.perform_reboot(&options);

        let req = terminal_stream.next().await.unwrap().unwrap();
        match req {
            fstatecontrol::TerminalStateWatcherRequest::OnTerminalStateTransitionStarted {
                responder,
            } => {
                responder.send().unwrap();
            }
            _ => panic!("Unexpected request"),
        }

        reboot_fut.await.unwrap().unwrap();
    }
}

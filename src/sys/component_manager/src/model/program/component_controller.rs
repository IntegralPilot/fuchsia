// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::{EscrowRequest, StopInfo};
use fidl_fuchsia_component_runner as fcrunner;
use fuchsia_async as fasync;
use fuchsia_sync::Mutex;
use futures::channel::oneshot;
use futures::future::{BoxFuture, Shared};
use futures::{FutureExt, StreamExt};
use moniker::Moniker;
use std::ops::Deref;
use std::sync::Arc;

/// Wrapper around the `ComponentControllerProxy` with utilities for handling events.
pub struct ComponentController {
    /// The wrapped `ComponentController` connection.
    inner: fcrunner::ComponentControllerProxy,

    /// Receiver for termination events coming from the connection.
    termination_value_recv: Shared<oneshot::Receiver<StopInfo>>,

    /// State stored by the `ComponentController` server.
    state: Arc<Mutex<State>>,

    /// The task listening for events.
    _event_listener_task: fasync::Task<()>,
}

struct State {
    /// The last value seen in an `ComponentController.OnEscrow` event.
    escrow: Option<EscrowRequest>,
}

impl Deref for ComponentController {
    type Target = fcrunner::ComponentControllerProxy;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<'a> ComponentController {
    /// Create a new wrapper around a `ComponentControllerProxy`.
    ///
    /// A task will be spawned to dispatch events on the `ComponentControllerProxy`.
    pub fn new(
        proxy: fcrunner::ComponentControllerProxy,
        diagnostics_sender: Option<oneshot::Sender<fcrunner::ComponentDiagnostics>>,
        moniker: Moniker,
        escrow_stats_node: fuchsia_inspect::Node,
    ) -> Self {
        let (epitaph_sender, termination_value_recv) = oneshot::channel();

        let state = Arc::new(Mutex::new(State { escrow: None }));
        let event_stream = proxy.take_event_stream();
        let events_fut = Self::listen_for_events(
            event_stream,
            epitaph_sender,
            state.clone(),
            diagnostics_sender,
            moniker,
            escrow_stats_node,
        );
        let event_listener_task = fasync::Task::spawn(events_fut);

        Self {
            inner: proxy,
            termination_value_recv: termination_value_recv.shared(),
            state,
            _event_listener_task: event_listener_task,
        }
    }

    /// Returns state that the program has escrowed, if any.
    pub fn finalize(self) -> Option<EscrowRequest> {
        let mut state = self.state.lock();
        state.escrow.take()
    }

    /// Obtain a future for the termination value.
    ///
    /// If the async task handling the `ComponentControllerProxy` unexpectedly exits, the future
    /// will resolve to `PEER_CLOSED`. Otherwise, it will resolve to the `zx::Status` representing
    /// the termination status sent on the channel.
    ///
    /// This method may be called multiple times from multiple threads.
    pub fn wait_for_termination(&self) -> BoxFuture<'static, StopInfo> {
        let val = self.termination_value_recv.clone();
        async move {
            val.await.unwrap_or(StopInfo {
                termination_status: Err(zx::Status::PEER_CLOSED),
                exit_code: None,
            })
        }
        .boxed()
    }

    async fn listen_for_events(
        mut event_stream: fcrunner::ComponentControllerEventStream,
        termination_sender: oneshot::Sender<StopInfo>,
        state: Arc<Mutex<State>>,
        mut diagnostics_sender: Option<oneshot::Sender<fcrunner::ComponentDiagnostics>>,
        moniker: Moniker,
        escrow_stats_node: fuchsia_inspect::Node,
    ) {
        let mut termination_sender = Some(termination_sender);
        while let Some(value) = event_stream.next().await {
            match value {
                Err(fidl::Error::ClientChannelClosed { epitaph, .. }) => {
                    termination_sender.take().and_then(|sender| {
                        sender
                            .send(StopInfo { termination_status: epitaph.into(), exit_code: None })
                            .ok()
                    });
                }
                Err(_) => {
                    termination_sender.take().and_then(|sender| {
                        sender
                            .send(StopInfo {
                                termination_status: Err(zx::Status::PEER_CLOSED),
                                exit_code: None,
                            })
                            .ok()
                    });
                }
                Ok(event) => match event {
                    fcrunner::ComponentControllerEvent::OnPublishDiagnostics {
                        payload, ..
                    } => {
                        diagnostics_sender.take().and_then(|sender| sender.send(payload).ok());
                    }
                    fcrunner::ComponentControllerEvent::OnEscrow { payload } => {
                        state.lock().escrow =
                            Some(EscrowRequest::from_fidl(payload, &moniker, &escrow_stats_node));
                    }
                    fcrunner::ComponentControllerEvent::OnStop { payload } => {
                        termination_sender.take().map(|sender| sender.send(payload.into()));
                        break;
                    }
                    fcrunner::ComponentControllerEvent::_UnknownEvent { .. } => (),
                },
            }
        }
        termination_sender.take().map(|sender| {
            sender
                .send(StopInfo {
                    termination_status: Err(zx::Status::PEER_CLOSED),
                    exit_code: None,
                })
                .unwrap_or(())
        });
    }

    /// Get the KOID of the `ComponentController` FIDL server endpoint.
    #[cfg(test)]
    pub fn peer_koid(&self) -> zx::Koid {
        use fidl::endpoints::Proxy;
        use zx::AsHandleRef;

        self.inner
            .as_channel()
            .as_handle_ref()
            .basic_info()
            .expect("basic info should not require any rights")
            .related_koid
    }
}

#[cfg(all(test, not(feature = "src_model_tests")))]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fidl::prelude::*;

    #[fuchsia::test]
    async fn handles_diagnostics_event() {
        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fcrunner::ComponentControllerMarker>();
        let (sender, receiver) = oneshot::channel();
        let _controller = ComponentController::new(
            proxy,
            Some(sender),
            Moniker::root(),
            fuchsia_inspect::Inspector::default().root().clone_weak(),
        );
        stream
            .control_handle()
            .send_on_publish_diagnostics(fcrunner::ComponentDiagnostics {
                tasks: Some(fcrunner::ComponentTasks::default()),
                ..Default::default()
            })
            .expect("sent diagnostics");
        assert_matches!(receiver.await, Ok(fcrunner::ComponentDiagnostics { tasks: Some(_), .. }));
    }

    #[fuchsia::test]
    async fn handles_connection_epitaph() {
        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fcrunner::ComponentControllerMarker>();
        let controller = ComponentController::new(
            proxy,
            None,
            Moniker::root(),
            fuchsia_inspect::Inspector::default().root().clone_weak(),
        );
        let epitaph_fut = controller.wait_for_termination();
        stream.control_handle().shutdown_with_epitaph(Err(zx::Status::UNAVAILABLE));
        assert_eq!(epitaph_fut.await.termination_status, Err(zx::Status::UNAVAILABLE));
    }

    #[fuchsia::test]
    async fn handles_epitaph_for_closed_connection() {
        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fcrunner::ComponentControllerMarker>();
        let controller = ComponentController::new(
            proxy,
            None,
            Moniker::root(),
            fuchsia_inspect::Inspector::default().root().clone_weak(),
        );
        let epitaph_fut = controller.wait_for_termination();
        drop(stream);
        assert_eq!(epitaph_fut.await.termination_status, Err(zx::Status::PEER_CLOSED));
    }

    #[fuchsia::test]
    async fn handles_epitaph_for_dropped_controller() {
        let (proxy, _) = fidl::endpoints::create_proxy::<fcrunner::ComponentControllerMarker>();
        let controller = ComponentController::new(
            proxy,
            None,
            Moniker::root(),
            fuchsia_inspect::Inspector::default().root().clone_weak(),
        );
        let epitaph_fut = controller.wait_for_termination();
        drop(controller);
        assert_eq!(epitaph_fut.await.termination_status, Err(zx::Status::PEER_CLOSED));
    }
}

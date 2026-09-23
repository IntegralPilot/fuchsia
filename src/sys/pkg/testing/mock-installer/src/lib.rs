// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assert_matches::assert_matches;
use flex_fuchsia_update_installer as fidl_installer;

use fidl_fuchsia_update_installer_ext::{State, StateId};
use fidl_installer::{
    InstallerMarker, InstallerProxy, InstallerRequest, InstallerRequestStream, MonitorProxy,
    Options, RebootControllerRequest, UpdateNotStartedReason,
};
use fuchsia_async as fasync;
use fuchsia_sync::Mutex;
use futures::channel::mpsc;
use futures::prelude::*;
use pretty_assertions::assert_eq;
use std::sync::Arc;

#[derive(PartialEq, Debug)]
pub enum CapturedUpdateInstallerRequest {
    StartUpdate { url: String, options: Options, reboot_controller_present: bool },
    SuspendUpdate { attempt_id: Option<String> },
    ResumeUpdate { attempt_id: Option<String> },
    CancelUpdate { attempt_id: Option<String> },
}

// Options does not impl Eq, but it is semantically Eq.
impl Eq for CapturedUpdateInstallerRequest {}

#[derive(Eq, PartialEq, Debug)]
pub enum CapturedRebootControllerRequest {
    Unblock,
    Detach,
}

pub struct MockUpdateInstallerServiceBuilder {
    states: Vec<State>,
    states_receiver: Option<mpsc::Receiver<State>>,
    start_update_response: Result<String, UpdateNotStartedReason>,
}

impl MockUpdateInstallerServiceBuilder {
    /// The mock installer will sends these states to the caller of StartUpdate.
    /// It will only work for the first StartUpdate call.
    /// Ignored if states_receiver exists.
    pub fn states(self, states: Vec<State>) -> Self {
        Self { states, ..self }
    }

    /// When the mock installer receives a state it will forward it to the caller of StartUpdate.
    /// It will only work for the first StartUpdate call.
    pub fn states_receiver(self, states_receiver: mpsc::Receiver<State>) -> Self {
        Self { states_receiver: Some(states_receiver), ..self }
    }

    pub fn start_update_response(
        self,
        start_update_response: Result<String, UpdateNotStartedReason>,
    ) -> Self {
        Self { start_update_response, ..self }
    }

    pub fn build(self) -> MockUpdateInstallerService {
        let Self { states, states_receiver, start_update_response } = self;
        let states_receiver = match states_receiver {
            Some(states_receiver) => states_receiver,
            None => {
                let (mut sender, receiver) = mpsc::channel(0);
                fasync::Task::spawn(async move {
                    for state in states {
                        sender.send(state).await.unwrap();
                    }
                })
                .detach();
                receiver
            }
        };
        MockUpdateInstallerService {
            start_update_response: Mutex::new(start_update_response),
            states_receiver: Mutex::new(Some(states_receiver)),
            captured_args: Mutex::new(vec![]),
            reboot_controller_requests: Mutex::new(vec![]),
        }
    }
}

pub struct MockUpdateInstallerService {
    start_update_response: Mutex<Result<String, UpdateNotStartedReason>>,
    states_receiver: Mutex<Option<mpsc::Receiver<State>>>,
    captured_args: Mutex<Vec<CapturedUpdateInstallerRequest>>,
    reboot_controller_requests: Mutex<Vec<CapturedRebootControllerRequest>>,
}

impl MockUpdateInstallerService {
    pub fn builder() -> MockUpdateInstallerServiceBuilder {
        MockUpdateInstallerServiceBuilder {
            states: vec![],
            start_update_response: Ok("id".into()),
            states_receiver: None,
        }
    }

    pub fn with_states(states: Vec<State>) -> Self {
        Self::builder().states(states).build()
    }

    pub fn with_response(start_update_response: Result<String, UpdateNotStartedReason>) -> Self {
        Self::builder().start_update_response(start_update_response).build()
    }

    pub async fn run_service(self: Arc<Self>, mut stream: InstallerRequestStream) {
        while let Some(req) = stream.try_next().await.unwrap() {
            match req {
                InstallerRequest::StartUpdate {
                    url,
                    options,
                    monitor,
                    reboot_controller,
                    responder,
                } => {
                    self.captured_args.lock().push(CapturedUpdateInstallerRequest::StartUpdate {
                        url: url.url,
                        options,
                        reboot_controller_present: reboot_controller.is_some(),
                    });
                    let proxy = monitor.into_proxy();
                    fasync::Task::spawn(Arc::clone(&self).send_states(proxy)).detach();
                    if let Some(reboot_controller) = reboot_controller {
                        let service = Arc::clone(&self);
                        fasync::Task::spawn(async move {
                            let mut stream = reboot_controller.into_stream();

                            while let Some(request) = stream.try_next().await.unwrap() {
                                let request = match request {
                                    RebootControllerRequest::Unblock { .. } => {
                                        CapturedRebootControllerRequest::Unblock
                                    }
                                    RebootControllerRequest::Detach { .. } => {
                                        CapturedRebootControllerRequest::Detach
                                    }
                                };
                                service.reboot_controller_requests.lock().push(request);
                            }
                        })
                        .detach();
                    }
                    let response = self.start_update_response.lock();
                    responder.send(response.as_deref().map_err(|e| *e)).unwrap();
                }
                InstallerRequest::SuspendUpdate { attempt_id, responder } => {
                    self.captured_args
                        .lock()
                        .push(CapturedUpdateInstallerRequest::SuspendUpdate { attempt_id });
                    responder.send(Ok(())).unwrap();
                }
                InstallerRequest::ResumeUpdate { attempt_id, responder } => {
                    self.captured_args
                        .lock()
                        .push(CapturedUpdateInstallerRequest::ResumeUpdate { attempt_id });
                    responder.send(Ok(())).unwrap();
                }
                InstallerRequest::CancelUpdate { attempt_id, responder } => {
                    self.captured_args
                        .lock()
                        .push(CapturedUpdateInstallerRequest::CancelUpdate { attempt_id });
                    responder.send(Ok(())).unwrap();
                }
                InstallerRequest::MonitorUpdate { .. } => {
                    panic!("unexpected request: {req:?}");
                }
            }
        }
    }

    async fn send_states(self: Arc<Self>, monitor: MonitorProxy) {
        let mut receiver = self
            .states_receiver
            .lock()
            .take()
            .expect("mock installer only supports a single StartUpdate call");

        while let Some(state) = receiver.next().await {
            Self::send_state(state, &monitor).await;
        }
    }

    async fn send_state(state: State, monitor: &MonitorProxy) {
        let is_reboot = state.id() == StateId::Reboot;
        let result = monitor.on_state(&state.into()).await;
        if is_reboot {
            assert_matches!(result, Err(_));
        } else {
            assert_matches!(result, Ok(()));
        }
    }

    pub fn assert_installer_called_with(&self, expected_args: Vec<CapturedUpdateInstallerRequest>) {
        assert_eq!(*self.captured_args.lock(), expected_args);
    }

    pub fn captured_args(&self) -> &Mutex<Vec<CapturedUpdateInstallerRequest>> {
        &self.captured_args
    }

    pub fn assert_reboot_controller_called_with(
        &self,
        expected_requests: Vec<CapturedRebootControllerRequest>,
    ) {
        assert_eq!(*self.reboot_controller_requests.lock(), expected_requests);
    }

    #[cfg(feature = "fdomain")]
    pub fn spawn_installer_service(
        self: Arc<Self>,
        client: Arc<fdomain_client::Client>,
    ) -> InstallerProxy {
        let (proxy, stream) = client.create_proxy_and_stream::<InstallerMarker>();

        fasync::Task::spawn(self.run_service(stream)).detach();

        proxy
    }

    #[cfg(not(feature = "fdomain"))]
    pub fn spawn_installer_service(self: Arc<Self>) -> InstallerProxy {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<InstallerMarker>();

        fasync::Task::spawn(self.run_service(stream)).detach();

        proxy
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flex_fuchsia_update_installer::{Initiator, MonitorMarker, MonitorRequest};
    use pretty_assertions::assert_eq;

    #[fuchsia::test]
    async fn test_mock_installer() {
        #[cfg(feature = "fdomain")]
        let client = fdomain_local::local_client_empty();
        #[cfg(not(feature = "fdomain"))]
        let client = fidl::endpoints::ZirconClient;
        let installer_service =
            Arc::new(MockUpdateInstallerService::with_states(vec![State::Prepare]));
        let proxy = Arc::clone(&installer_service).spawn_installer_service(
            #[cfg(feature = "fdomain")]
            client.clone(),
        );
        let url =
            flex_fuchsia_pkg::PackageUrl { url: "fuchsia-pkg://fuchsia.com/update".to_string() };
        let options = Options {
            initiator: Some(Initiator::User),
            should_write_recovery: Some(true),
            allow_attach_to_existing_attempt: Some(true),
            ..Default::default()
        };
        let (monitor_client_end, stream) = client.create_request_stream::<MonitorMarker>();
        proxy
            .start_update(&url, &options, monitor_client_end, None)
            .await
            .expect("made start_update call")
            .expect("start_update call succeeded");
        assert_eq!(
            vec![State::Prepare],
            stream
                .map_ok(|MonitorRequest::OnState { state, responder }| {
                    responder.send().unwrap();
                    State::try_from(state).unwrap()
                })
                .try_collect::<Vec<_>>()
                .await
                .unwrap()
        );
    }
}

// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::update::{Config, EnvironmentConnector, RebootController, Updater};
use anyhow::anyhow;
use event_queue::{EventQueue, Notify};
use fidl_fuchsia_update_installer::{
    CancelError, ResumeError, SuspendError, UpdateNotStartedReason,
};
use fidl_fuchsia_update_installer_ext::State;
use fuchsia_async as fasync;
use fuchsia_inspect as inspect;
use fuchsia_inspect_contrib::nodes::{BoundedListNode, NodeTimeExt};
use futures::channel::{mpsc, oneshot};
use futures::prelude::*;
use futures::select;
use futures::stream::FusedStream;
use log::{error, warn};
use std::time::Duration;

const INSPECT_STATUS_NODE_NAME: &str = "status";
// Suspend is allowed at most 7 days, after that update will automatically resume.
const MAX_SUSPEND_DURATION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
// Cancel is allowed 100 times per boot.
const MAX_NUM_CANCEL_REQUESTS: usize = 100;

/// Start a install manager task that:
///  * Runs an update attempt in a seperate task.
///  * Provides a control handle to forward FIDL requests to the update attempt task.
pub async fn start_install_manager<N, U, E>(
    updater: U,
    node: inspect::Node,
) -> (InstallManagerControlHandle<N>, impl Future<Output = ()>)
where
    N: Notify<Event = State> + Send + 'static,
    U: Updater,
    E: EnvironmentConnector,
{
    let (send, recv) = mpsc::channel(0);
    (InstallManagerControlHandle(send), run::<N, U, E>(recv, updater, node))
}

/// The install manager task.
async fn run<N, U, E>(
    mut recv: mpsc::Receiver<ControlRequest<N>>,
    mut updater: U,
    node: inspect::Node,
) where
    N: Notify<Event = State> + Send + 'static,
    U: Updater,
    E: EnvironmentConnector,
{
    // Set up event queue to keep track of all the Monitors.
    let (monitor_queue_fut, mut monitor_queue) = EventQueue::new();
    let eq_task = fasync::Task::spawn(monitor_queue_fut);

    let attempt_node = node.create_child("current_attempt");
    let mut requests_node = BoundedListNode::new(node.create_child("requests"), 100);

    let mut num_cancel_requests = 0;

    // Each iteration of this loop is one update attempt.
    loop {
        // There is no active update attempt, so let's wait for a start request.
        let StartRequestData { config, monitor, reboot_controller, responder } =
            match handle_idle_control_request(&mut recv, &mut requests_node).await {
                Some(start_data) => start_data,
                None => {
                    // Ensure all monitors get the remaining state updates.
                    drop(monitor_queue);
                    eq_task.await;
                    return;
                }
            };
        let reboot_controller = reboot_controller.unwrap_or_else(RebootController::unblocked);

        // We connect to FIDL services on each update attempt (rather than once at the
        // beginning) to prevent stale connections.
        let env = match E::connect() {
            Ok(env) => env,
            Err(e) => {
                error!("Error connecting to services: {:#}", anyhow!(e));
                // This fails the update attempt because it drops the responder, which closes
                // the zx channel that we got the start request from.
                continue;
            }
        };

        // Now we can actually start the task that manages the update attempt.
        let update_url = &config.update_url.clone();
        let should_write_recovery = config.should_write_recovery;
        let (cancel_sender, cancel_receiver) = oneshot::channel();
        let (attempt_id, attempt_stream) =
            updater.update(config, env, reboot_controller, cancel_receiver).await;
        futures::pin_mut!(attempt_stream);

        // Set up inspect nodes.
        let mut status_node = attempt_node.create_child(INSPECT_STATUS_NODE_NAME);
        let start_time = fasync::BootInstant::now();
        // Don't use the record_ method so that it will be cleared when the attempt is over.
        let _time_property = attempt_node.create_int("start_boot_ns", start_time.into_nanos());

        // Don't forget to add the first monitor to the queue and respond to StartUpdate :)
        if let Err(e) = monitor_queue.add_client(monitor).await {
            warn!("error adding client to monitor queue: {:#}", anyhow!(e));
        }
        let _ = responder.send(Ok(attempt_id.clone()));

        let mut suspend_state = SuspendState::Running;
        let suspend_deadline = start_time + MAX_SUSPEND_DURATION.into();
        let mut cancel_sender = Some(cancel_sender);

        // For this update attempt, handle events both from the FIDL server and the update task.
        loop {
            // We use this enum to make the body of the select as short as possible. Otherwise,
            // we'd need to set the crate's recursion_limit to be higher.
            enum Op<N: Notify> {
                Request(ControlRequest<N>),
                Status(Option<State>),
            }
            let op = match suspend_state {
                SuspendState::Suspended => {
                    select! {
                        req = recv.select_next_some() => Op::Request(req),
                        () = fasync::Timer::new(suspend_deadline).fuse() => {
                            suspend_state.resume();
                            continue;
                        }
                    }
                }
                SuspendState::Running => {
                    select! {
                        req = recv.select_next_some() => Op::Request(req),
                        state = attempt_stream.next() => Op::Status(state)
                    }
                }
            };
            match op {
                // We got a FIDL requests (via the mpsc::Receiver).
                Op::Request(req) => {
                    requests_node.add_entry(|node| {
                        req.write_to_inspect(Some(&attempt_id), node);
                    });
                    handle_active_control_request(
                        req,
                        &mut monitor_queue,
                        &attempt_id,
                        update_url,
                        should_write_recovery,
                        &mut suspend_state,
                        suspend_deadline,
                        &mut cancel_sender,
                        &mut num_cancel_requests,
                    )
                    .await
                }
                // The update task has given us a progress update, so let's forward
                // that to all the monitors.
                Op::Status(Some(state)) => {
                    drop(status_node);
                    status_node = attempt_node.create_child(INSPECT_STATUS_NODE_NAME);
                    state.write_to_inspect(&status_node);
                    if let Err(e) = monitor_queue.queue_event(state).await {
                        warn!("error sending state to monitor_queue: {:#}", anyhow!(e));
                    }
                }
                // The update task tells us the update is over, so let's notify all monitors.
                Op::Status(None) => {
                    drop(status_node);
                    if let Err(e) = monitor_queue.clear().await {
                        warn!("error clearing clients of monitor_queue: {:#}", anyhow!(e));
                    }
                    break;
                }
            }
        }
    }
}

/// Returns when we get a Start control request (i.e. a StartUpdate FIDL request forwarded
/// from the FIDL server).
async fn handle_idle_control_request<N>(
    recv: &mut mpsc::Receiver<ControlRequest<N>>,
    requests_node: &mut BoundedListNode,
) -> Option<StartRequestData<N>>
where
    N: Notify,
{
    // If the stream of control requests terminated while an update attempt was running,
    // this stream has already yielded None, and so further calls to next() may panic.
    // Instead, check if the stream is terminated via its FusedStream implementation
    // before proceeding.
    if recv.is_terminated() {
        return None;
    }

    // Right now we are in a state where there is no update running.
    while let Some(control_request) = recv.next().await {
        requests_node.add_entry(|node| {
            control_request.write_to_inspect(None, node);
        });
        match control_request {
            ControlRequest::Start(start_data) => {
                return Some(start_data);
            }
            ControlRequest::Monitor(MonitorRequestData { responder, .. }) => {
                let _ = responder.send(false);
            }
            ControlRequest::Suspend(SuspendRequestData { responder, .. }) => {
                let _ = responder.send(Err(SuspendError::NoUpdateInProgress));
            }
            ControlRequest::Resume(ResumeRequestData { responder, .. }) => {
                let _ = responder.send(Err(ResumeError::NoUpdateInProgress));
            }
            ControlRequest::Cancel(CancelRequestData { responder, .. }) => {
                let _ = responder.send(Err(CancelError::NoUpdateInProgress));
            }
        }
    }
    None
}

/// Handle the logic for the install manager task will do when receiving a control request
/// while an update is underway.
async fn handle_active_control_request<N>(
    req: ControlRequest<N>,
    monitor_queue: &mut event_queue::ControlHandle<N>,
    attempt_id: &str,
    update_url: &http::Uri,
    should_write_recovery: bool,
    suspend_state: &mut SuspendState,
    suspend_deadline: fasync::BootInstant,
    cancel_sender: &mut Option<oneshot::Sender<()>>,
    num_cancel_requests: &mut usize,
) where
    N: Notify,
{
    let attempt_id_mismatch =
        |request_id: Option<String>| request_id.is_some_and(|request_id| request_id != attempt_id);

    match req {
        ControlRequest::Start(StartRequestData {
            responder,
            config,
            monitor,
            reboot_controller,
        }) => {
            // Only attach monitor if she's compatible with current update check.
            // Note: We can only attach a reboot controller during the FIRST start request.
            // Any subsequent request with a reboot controller should fail.
            if reboot_controller.is_none()
                && config.allow_attach_to_existing_attempt
                && &config.update_url == update_url
                && config.should_write_recovery == should_write_recovery
            {
                if let Err(e) = monitor_queue.add_client(monitor).await {
                    warn!("error adding client to monitor queue: {:#}", anyhow!(e));
                }
                let _ = responder.send(Ok(attempt_id.to_string()));
            } else {
                let _ = responder.send(Err(UpdateNotStartedReason::AlreadyInProgress));
            }
            suspend_state.resume();
        }
        ControlRequest::Monitor(MonitorRequestData { attempt_id, monitor, responder }) => {
            // If an attempt ID is provided, ensure it matches the current attempt.
            if attempt_id_mismatch(attempt_id) {
                let _ = responder.send(false);
                return;
            }

            if let Err(e) = monitor_queue.add_client(monitor).await {
                warn!("error adding client to monitor queue: {:#}", anyhow!(e));
            }
            let _ = responder.send(true);
        }
        ControlRequest::Suspend(SuspendRequestData { attempt_id, responder }) => {
            // If an attempt ID is provided, ensure it matches the current attempt.
            if attempt_id_mismatch(attempt_id) {
                let _ = responder.send(Err(SuspendError::AttemptIdMismatch));
                return;
            }

            if fasync::BootInstant::now() > suspend_deadline {
                let _ = responder.send(Err(SuspendError::SuspendLimitExceeded));
                return;
            }
            suspend_state.suspend();
            let _ = responder.send(Ok(()));
        }
        ControlRequest::Resume(ResumeRequestData { attempt_id, responder }) => {
            // If an attempt ID is provided, ensure it matches the current attempt.
            if attempt_id_mismatch(attempt_id) {
                let _ = responder.send(Err(ResumeError::AttemptIdMismatch));
                return;
            }

            suspend_state.resume();
            let _ = responder.send(Ok(()));
        }
        ControlRequest::Cancel(CancelRequestData { attempt_id, responder }) => {
            // If an attempt ID is provided, ensure it matches the current attempt.
            if attempt_id_mismatch(attempt_id) {
                let _ = responder.send(Err(CancelError::AttemptIdMismatch));
                return;
            }

            let response = match cancel_sender.take() {
                Some(cancel_sender) => {
                    if *num_cancel_requests >= MAX_NUM_CANCEL_REQUESTS {
                        let _ = responder.send(Err(CancelError::CancelLimitExceeded));
                        return;
                    }
                    *num_cancel_requests += 1;
                    cancel_sender.send(()).map_err(|()| CancelError::UpdateCannotBeCanceled)
                }
                None => Ok(()),
            };
            let _ = responder.send(response);
        }
    }
}

/// A handle to forward installer FIDL requests to the install manager task.
#[derive(Clone)]
pub struct InstallManagerControlHandle<N>(mpsc::Sender<ControlRequest<N>>)
where
    N: Notify;

/// Error indicating that the install manager task no longer exists.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
#[error("install manager dropped before all its control handles")]
pub struct InstallManagerGone;

impl From<mpsc::SendError> for InstallManagerGone {
    fn from(_: mpsc::SendError) -> Self {
        InstallManagerGone
    }
}

impl From<oneshot::Canceled> for InstallManagerGone {
    fn from(_: oneshot::Canceled) -> Self {
        InstallManagerGone
    }
}

/// This can be used to forward requests to the install manager task.
impl<N> InstallManagerControlHandle<N>
where
    N: Notify,
{
    /// Forward StartUpdate requests to the install manager task.
    pub async fn start_update(
        &mut self,
        config: Config,
        monitor: N,
        reboot_controller: Option<RebootController>,
    ) -> Result<Result<String, UpdateNotStartedReason>, InstallManagerGone> {
        let (responder, receive_response) = oneshot::channel();
        self.0
            .send(ControlRequest::Start(StartRequestData {
                config,
                monitor,
                reboot_controller,
                responder,
            }))
            .await?;
        Ok(receive_response.await?)
    }

    /// Forward MonitorUpdate requests to the install manager task.
    pub async fn monitor_update(
        &mut self,
        attempt_id: Option<String>,
        monitor: N,
    ) -> Result<bool, InstallManagerGone> {
        let (responder, receive_response) = oneshot::channel();
        self.0
            .send(ControlRequest::Monitor(MonitorRequestData { attempt_id, monitor, responder }))
            .await?;
        Ok(receive_response.await?)
    }

    /// Forward SuspendUpdate requests to the install manager task.
    pub async fn suspend_update(
        &mut self,
        attempt_id: Option<String>,
    ) -> Result<Result<(), SuspendError>, InstallManagerGone> {
        let (responder, receive_response) = oneshot::channel();
        self.0.send(ControlRequest::Suspend(SuspendRequestData { attempt_id, responder })).await?;
        Ok(receive_response.await?)
    }

    /// Forward ResumeUpdate requests to the install manager task.
    pub async fn resume_update(
        &mut self,
        attempt_id: Option<String>,
    ) -> Result<Result<(), ResumeError>, InstallManagerGone> {
        let (responder, receive_response) = oneshot::channel();
        self.0.send(ControlRequest::Resume(ResumeRequestData { attempt_id, responder })).await?;
        Ok(receive_response.await?)
    }

    /// Forward CancelUpdate requests to the install manager task.
    pub async fn cancel_update(
        &mut self,
        attempt_id: Option<String>,
    ) -> Result<Result<(), CancelError>, InstallManagerGone> {
        let (responder, receive_response) = oneshot::channel();
        self.0.send(ControlRequest::Cancel(CancelRequestData { attempt_id, responder })).await?;
        Ok(receive_response.await?)
    }
}

/// Requests that can be forwarded to the install manager task.
enum ControlRequest<N>
where
    N: Notify,
{
    Start(StartRequestData<N>),
    Monitor(MonitorRequestData<N>),
    Suspend(SuspendRequestData),
    Resume(ResumeRequestData),
    Cancel(CancelRequestData),
}

impl<N: Notify> ControlRequest<N> {
    fn write_to_inspect(&self, in_progress_attempt_id: Option<&str>, node: &inspect::Node) {
        NodeTimeExt::<zx::BootTimeline>::record_time(node, "boot_ns");
        node.record_string("already_in_progress_attempt_id", format!("{in_progress_attempt_id:?}"));

        match self {
            Self::Start(data) => {
                node.record_string("request", "start");
                node.record_string("initiator", format!("{:?}", data.config.initiator));
                node.record_string("update_url", data.config.update_url.to_string());
                node.record_string(
                    "should_write_recovery",
                    format!("{:?}", data.config.should_write_recovery),
                );
                node.record_string(
                    "allow_attach_to_existing_attempt",
                    format!("{:?}", data.config.allow_attach_to_existing_attempt),
                );
            }
            Self::Monitor(data) => {
                node.record_string("request", "monitor");
                if let Some(attempt_id) = &data.attempt_id {
                    node.record_string("attempt_id", attempt_id);
                }
            }
            Self::Suspend(data) => {
                node.record_string("request", "suspend");
                if let Some(attempt_id) = &data.attempt_id {
                    node.record_string("attempt_id", attempt_id);
                }
            }
            Self::Resume(data) => {
                node.record_string("request", "resume");
                if let Some(attempt_id) = &data.attempt_id {
                    node.record_string("attempt_id", attempt_id);
                }
            }
            Self::Cancel(data) => {
                node.record_string("request", "cancel");
                if let Some(attempt_id) = &data.attempt_id {
                    node.record_string("attempt_id", attempt_id);
                }
            }
        }
    }
}

struct StartRequestData<N>
where
    N: Notify,
{
    config: Config,
    monitor: N,
    reboot_controller: Option<RebootController>,
    responder: oneshot::Sender<Result<String, UpdateNotStartedReason>>,
}

struct MonitorRequestData<N>
where
    N: Notify,
{
    attempt_id: Option<String>,
    monitor: N,
    responder: oneshot::Sender<bool>,
}

struct SuspendRequestData {
    attempt_id: Option<String>,
    responder: oneshot::Sender<Result<(), SuspendError>>,
}

struct ResumeRequestData {
    attempt_id: Option<String>,
    responder: oneshot::Sender<Result<(), ResumeError>>,
}

struct CancelRequestData {
    attempt_id: Option<String>,
    responder: oneshot::Sender<Result<(), CancelError>>,
}

enum SuspendState {
    Suspended,
    Running,
}

impl SuspendState {
    fn suspend(&mut self) {
        *self = Self::Suspended
    }

    fn resume(&mut self) {
        *self = Self::Running
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::{
        ConfigBuilder, Environment, NamespaceBuildInfo, NamespaceCobaltConnector,
        NamespaceSystemInfo,
    };
    use async_trait::async_trait;
    use diagnostics_assertions::{AnyProperty, assert_data_tree};
    use event_queue::ClosedClient;
    use fidl_fuchsia_hardware_power_statecontrol::AdminMarker as PowerStateControlMarker;
    use fidl_fuchsia_paver::{BootManagerMarker, DataSinkMarker};
    use fidl_fuchsia_pkg::{
        PackageCacheMarker, PackageResolverMarker, RetainedBlobsMarker, RetainedPackagesMarker,
    };
    use fidl_fuchsia_pkg_garbagecollector::ManagerMarker as SpaceManagerMarker;
    use fidl_fuchsia_update_installer_ext::{
        PrepareFailureReason, Progress, UpdateInfo, UpdateInfoAndProgress,
    };
    use fuchsia_inspect::Inspector;
    use fuchsia_sync::Mutex;
    use mpsc::{Receiver, Sender};
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::Poll;

    const CALLBACK_CHANNEL_SIZE: usize = 20;

    struct FakeStateNotifier {
        sender: Arc<Mutex<Sender<State>>>,
    }
    impl FakeStateNotifier {
        fn new_callback_and_receiver() -> (Self, Receiver<State>) {
            let (sender, receiver) = mpsc::channel(CALLBACK_CHANNEL_SIZE);
            (Self { sender: Arc::new(Mutex::new(sender)) }, receiver)
        }
    }
    impl Notify for FakeStateNotifier {
        type Event = State;
        type NotifyFuture = future::Ready<Result<(), ClosedClient>>;
        fn notify(&self, state: State) -> Self::NotifyFuture {
            self.sender.lock().try_send(state).expect("FakeStateNotifier failed to send state");
            future::ready(Ok(()))
        }
    }

    struct StubEnvironmentConnector;
    impl EnvironmentConnector for StubEnvironmentConnector {
        fn connect() -> Result<Environment, anyhow::Error> {
            let (data_sink, _) = fidl::endpoints::create_proxy::<DataSinkMarker>();
            let (boot_manager, _) = fidl::endpoints::create_proxy::<BootManagerMarker>();
            let (pkg_resolver, _) = fidl::endpoints::create_proxy::<PackageResolverMarker>();
            let (pkg_cache, _) = fidl::endpoints::create_proxy::<PackageCacheMarker>();
            let (retained_packages, _) = fidl::endpoints::create_proxy::<RetainedPackagesMarker>();
            let (retained_blobs, _) = fidl::endpoints::create_proxy::<RetainedBlobsMarker>();
            let (space_manager, _) = fidl::endpoints::create_proxy::<SpaceManagerMarker>();
            let (power_state_control, _) =
                fidl::endpoints::create_proxy::<PowerStateControlMarker>();
            let (ota_downloader, _) =
                fidl::endpoints::create_proxy::<fidl_fuchsia_pkg_internal::OtaDownloaderMarker>();

            Ok(Environment {
                data_sink,
                boot_manager,
                pkg_resolver,
                pkg_cache,
                retained_packages,
                retained_blobs,
                space_manager,
                power_state_control,
                ota_downloader,
                build_info: NamespaceBuildInfo,
                cobalt_connector: NamespaceCobaltConnector,
                system_info: NamespaceSystemInfo,
            })
        }
    }

    struct FakeUpdater(mpsc::Receiver<(String, mpsc::Receiver<State>)>);
    impl FakeUpdater {
        fn new(receiver: mpsc::Receiver<(String, mpsc::Receiver<State>)>) -> Self {
            Self(receiver)
        }
    }

    #[async_trait(?Send)]
    impl Updater for FakeUpdater {
        type UpdateStream = mpsc::Receiver<State>;

        async fn update(
            &mut self,
            _config: Config,
            _env: Environment,
            _reboot_controller: RebootController,
            _cancel_receiver: oneshot::Receiver<()>,
        ) -> (String, Self::UpdateStream) {
            self.0.next().await.unwrap()
        }
    }

    async fn start_install_manager_with_update_id(
        id: &str,
    ) -> (
        InstallManagerControlHandle<FakeStateNotifier>,
        fasync::Task<()>,
        mpsc::Sender<(String, mpsc::Receiver<State>)>,
        mpsc::Sender<State>,
    ) {
        let inspector = Inspector::default();
        let node = inspector.root().create_child("test_does_not_use_inspect");
        start_install_manager_with_update_id_and_node(id, node).await
    }

    async fn start_install_manager_with_update_id_and_node(
        id: &str,
        node: inspect::Node,
    ) -> (
        InstallManagerControlHandle<FakeStateNotifier>,
        fasync::Task<()>,
        mpsc::Sender<(String, mpsc::Receiver<State>)>,
        mpsc::Sender<State>,
    ) {
        // We use this channel to send the attempt id and state receiver to the update task, for
        // each update attempt. This allows tests to control when an update attempt ends -- all they
        // need to do is drop the state sender.
        let (mut updater_sender, updater_receiver) = mpsc::channel(0);
        let updater = FakeUpdater::new(updater_receiver);
        let (state_sender, state_receiver) = mpsc::channel(0);
        let (install_manager_ch, fut) =
            start_install_manager::<FakeStateNotifier, FakeUpdater, StubEnvironmentConnector>(
                updater, node,
            )
            .await;
        let install_manager_task = fasync::Task::local(fut);

        // We just use try_send because send calls are blocked on the receiver receiving the
        // event... and the receiver won't receive the event until we do a start_update request.
        updater_sender.try_send((id.to_string(), state_receiver)).unwrap();

        (install_manager_ch, install_manager_task, updater_sender, state_sender)
    }

    #[fuchsia::test]
    async fn monitor_update_fails_when_no_update_running() {
        let (mut install_manager_ch, _install_manager_task, _updater_sender, _state_sender) =
            start_install_manager_with_update_id("my-attempt").await;
        let (notifier0, _state_receiver0) = FakeStateNotifier::new_callback_and_receiver();
        let (notifier1, _state_receiver1) = FakeStateNotifier::new_callback_and_receiver();

        assert_eq!(install_manager_ch.monitor_update(None, notifier0).await, Ok(false));
        assert_eq!(
            install_manager_ch.monitor_update(Some("my-attempt".to_string()), notifier1).await,
            Ok(false)
        );
    }

    #[fuchsia::test]
    async fn monitor_update_fails_with_wrong_id() {
        let (mut install_manager_ch, _install_manager_task, _updater_sender, _state_sender) =
            start_install_manager_with_update_id("my-attempt").await;
        let (notifier0, _state_receiver0) = FakeStateNotifier::new_callback_and_receiver();
        let (notifier1, _state_receiver1) = FakeStateNotifier::new_callback_and_receiver();

        assert_eq!(
            install_manager_ch
                .start_update(ConfigBuilder::new().build().unwrap(), notifier0, None)
                .await,
            Ok(Ok("my-attempt".to_string()))
        );

        assert_eq!(
            install_manager_ch.monitor_update(Some("unknown id".to_string()), notifier1).await,
            Ok(false)
        );
    }

    #[fuchsia::test]
    async fn monitor_connects_via_start_update() {
        let (mut install_manager_ch, _install_manager_task, _updater_sender, mut state_sender) =
            start_install_manager_with_update_id("my-attempt").await;
        let (notifier, state_receiver) = FakeStateNotifier::new_callback_and_receiver();
        assert_eq!(
            install_manager_ch
                .start_update(ConfigBuilder::new().build().unwrap(), notifier, None)
                .await,
            Ok(Ok("my-attempt".to_string()))
        );

        let () = state_sender.send(State::Prepare).await.unwrap();
        let () =
            state_sender.send(State::FailPrepare(PrepareFailureReason::Internal)).await.unwrap();
        drop(state_sender);

        assert_eq!(
            state_receiver.collect::<Vec<State>>().await,
            vec![State::Prepare, State::FailPrepare(PrepareFailureReason::Internal)]
        );
    }

    #[fuchsia::test]
    async fn monitor_update_succeeds() {
        let (mut install_manager_ch, _install_manager_task, _updater_sender, mut state_sender) =
            start_install_manager_with_update_id("my-attempt").await;
        let (notifier0, state_receiver0) = FakeStateNotifier::new_callback_and_receiver();
        let (notifier1, state_receiver1) = FakeStateNotifier::new_callback_and_receiver();
        let (notifier2, state_receiver2) = FakeStateNotifier::new_callback_and_receiver();

        // Show we can successfuly add each notifier via either start_update or monitor_update.
        assert_eq!(
            install_manager_ch
                .start_update(ConfigBuilder::new().build().unwrap(), notifier0, None)
                .await,
            Ok(Ok("my-attempt".to_string()))
        );
        assert_eq!(install_manager_ch.monitor_update(None, notifier1).await, Ok(true));
        assert_eq!(
            install_manager_ch.monitor_update(Some("my-attempt".to_string()), notifier2).await,
            Ok(true)
        );

        // Send state updates to the update task.
        let () = state_sender.send(State::Prepare).await.unwrap();
        let () =
            state_sender.send(State::FailPrepare(PrepareFailureReason::Internal)).await.unwrap();
        drop(state_sender);

        // Each monitor should get these events.
        assert_eq!(
            state_receiver0.collect::<Vec<State>>().await,
            vec![State::Prepare, State::FailPrepare(PrepareFailureReason::Internal)]
        );
        assert_eq!(
            state_receiver1.collect::<Vec<State>>().await,
            vec![State::Prepare, State::FailPrepare(PrepareFailureReason::Internal)]
        );
        assert_eq!(
            state_receiver2.collect::<Vec<State>>().await,
            vec![State::Prepare, State::FailPrepare(PrepareFailureReason::Internal)]
        );
    }

    #[fuchsia::test]
    async fn succeed_additional_start_requests_when_compatible() {
        let (mut install_manager_ch, _install_manager_task, _updater_sender, mut state_sender) =
            start_install_manager_with_update_id("my-attempt").await;
        let (notifier0, state_receiver0) = FakeStateNotifier::new_callback_and_receiver();
        let (notifier1, state_receiver1) = FakeStateNotifier::new_callback_and_receiver();
        assert_eq!(
            install_manager_ch
                .start_update(ConfigBuilder::new().build().unwrap(), notifier0, None)
                .await,
            Ok(Ok("my-attempt".to_string()))
        );

        // The second start_update request is acceptable because the config is compatible.
        assert_eq!(
            install_manager_ch
                .start_update(
                    ConfigBuilder::new().allow_attach_to_existing_attempt(true).build().unwrap(),
                    notifier1,
                    None
                )
                .await,
            Ok(Ok("my-attempt".to_string()))
        );

        // Send state updates to the update task, then end the update by dropping the sender.
        let () = state_sender.send(State::Prepare).await.unwrap();
        let () =
            state_sender.send(State::FailPrepare(PrepareFailureReason::Internal)).await.unwrap();
        drop(state_sender);

        // Each monitor should get these events.
        assert_eq!(
            state_receiver0.collect::<Vec<State>>().await,
            vec![State::Prepare, State::FailPrepare(PrepareFailureReason::Internal)]
        );
        assert_eq!(
            state_receiver1.collect::<Vec<State>>().await,
            vec![State::Prepare, State::FailPrepare(PrepareFailureReason::Internal)]
        );
    }

    #[test]
    fn suspend_resume_succeeds() {
        let mut exec = fasync::TestExecutor::new();
        let inspector = Inspector::default();
        let (mut install_manager_ch, _install_manager_task, _updater_sender, mut state_sender) =
            exec.run_singlethreaded(start_install_manager_with_update_id_and_node(
                "my-attempt",
                inspector.root().create_child("current_attempt"),
            ));
        let (notifier, mut state_receiver) = FakeStateNotifier::new_callback_and_receiver();

        exec.run_singlethreaded(async {
            assert_eq!(
                install_manager_ch
                    .start_update(ConfigBuilder::new().build().unwrap(), notifier, None)
                    .await,
                Ok(Ok("my-attempt".to_string()))
            );

            let () = state_sender.send(State::Prepare).await.unwrap();
            assert_eq!(state_receiver.next().await, Some(State::Prepare));

            assert_eq!(
                install_manager_ch.suspend_update(Some("my-attempt".into())).await,
                Ok(Ok(()))
            );
        });

        // send and receive states are pending once suspended
        let mut send_fut = state_sender.send(State::FailPrepare(PrepareFailureReason::Internal));
        assert_eq!(exec.run_until_stalled(&mut send_fut), Poll::Pending);

        let mut recv_fut = state_receiver.next();
        assert_eq!(exec.run_until_stalled(&mut recv_fut), Poll::Pending);

        exec.run_singlethreaded(async {
            assert_eq!(
                install_manager_ch.resume_update(Some("my-attempt".into())).await,
                Ok(Ok(()))
            );

            // can send and receive states after resume
            let () = send_fut.await.unwrap();
            assert_eq!(recv_fut.await, Some(State::FailPrepare(PrepareFailureReason::Internal)));
        });

        assert_data_tree!(
            @executor exec,
            inspector,
            root: {
                current_attempt: contains {
                    requests: {
                        "0": contains {
                            request: "start",
                        },
                        "1": {
                            request: "suspend",
                            already_in_progress_attempt_id: r#"Some("my-attempt")"#,
                            attempt_id: "my-attempt",
                            boot_ns: AnyProperty,
                        },
                        "2": {
                            request: "resume",
                            already_in_progress_attempt_id: r#"Some("my-attempt")"#,
                            attempt_id: "my-attempt",
                            boot_ns: AnyProperty,
                        },
                    },
                }
            }
        );
    }

    #[fuchsia::test]
    async fn suspend_resume_errors() {
        let (mut install_manager_ch, _install_manager_task, _updater_sender, mut state_sender) =
            start_install_manager_with_update_id("my-attempt").await;

        assert_eq!(
            install_manager_ch.suspend_update(None).await,
            Ok(Err(SuspendError::NoUpdateInProgress))
        );

        assert_eq!(
            install_manager_ch.resume_update(None).await,
            Ok(Err(ResumeError::NoUpdateInProgress))
        );

        let (notifier, state_receiver) = FakeStateNotifier::new_callback_and_receiver();
        assert_eq!(
            install_manager_ch
                .start_update(ConfigBuilder::new().build().unwrap(), notifier, None)
                .await,
            Ok(Ok("my-attempt".to_string()))
        );

        let () = state_sender.send(State::Prepare).await.unwrap();

        assert_eq!(
            install_manager_ch.suspend_update(Some("wrong-id".into())).await,
            Ok(Err(SuspendError::AttemptIdMismatch))
        );
        assert_eq!(
            install_manager_ch.resume_update(Some("wrong-id".into())).await,
            Ok(Err(ResumeError::AttemptIdMismatch))
        );
        assert_eq!(install_manager_ch.suspend_update(None).await, Ok(Ok(())));
        assert_eq!(install_manager_ch.resume_update(None).await, Ok(Ok(())));

        let () =
            state_sender.send(State::FailPrepare(PrepareFailureReason::Internal)).await.unwrap();
        drop(state_sender);

        assert_eq!(
            state_receiver.collect::<Vec<State>>().await,
            vec![State::Prepare, State::FailPrepare(PrepareFailureReason::Internal)]
        );

        assert_eq!(
            install_manager_ch.suspend_update(None).await,
            Ok(Err(SuspendError::NoUpdateInProgress))
        );

        assert_eq!(
            install_manager_ch.resume_update(None).await,
            Ok(Err(ResumeError::NoUpdateInProgress))
        );
    }

    #[test]
    fn suspend_exceed_max_duration() {
        // Like run_singlethreaded for fake time executor if you are sure the future will complete.
        fn run_fut<T>(exec: &mut fasync::TestExecutor, fut: impl futures::Future<Output = T>) -> T {
            futures::pin_mut!(fut);
            let std::task::Poll::Ready(t) = exec.run_until_stalled(&mut fut) else {
                panic!("future stalled");
            };
            t
        }

        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let (mut install_manager_ch, _install_manager_task, _updater_sender, mut state_sender) =
            run_fut(&mut exec, start_install_manager_with_update_id("my-attempt"));
        let (notifier, mut state_receiver) = FakeStateNotifier::new_callback_and_receiver();

        let () = run_fut(&mut exec, async {
            assert_eq!(
                install_manager_ch
                    .start_update(ConfigBuilder::new().build().unwrap(), notifier, None)
                    .await,
                Ok(Ok("my-attempt".to_string()))
            );

            let () = state_sender.send(State::Prepare).await.unwrap();
            assert_eq!(state_receiver.next().await, Some(State::Prepare));

            assert_eq!(install_manager_ch.suspend_update(None).await, Ok(Ok(())));
        });

        // send and receive states are pending once suspended
        let mut send_fut = state_sender.send(State::FailPrepare(PrepareFailureReason::Internal));
        assert_eq!(exec.run_until_stalled(&mut send_fut), Poll::Pending);

        let mut recv_fut = state_receiver.next();
        assert_eq!(exec.run_until_stalled(&mut recv_fut), Poll::Pending);

        assert_eq!(
            exec.wake_next_boot_timer().unwrap() - exec.boot_now(),
            MAX_SUSPEND_DURATION.into()
        );

        // update should be resumed
        let () = run_fut(&mut exec, async {
            let () = send_fut.await.unwrap();
            assert_eq!(recv_fut.await, Some(State::FailPrepare(PrepareFailureReason::Internal)));
        });
    }

    struct CancelableUpdater;

    #[async_trait(?Send)]
    impl Updater for CancelableUpdater {
        type UpdateStream = Pin<Box<dyn FusedStream<Item = State>>>;

        async fn update(
            &mut self,
            _config: Config,
            _env: Environment,
            _reboot_controller: RebootController,
            cancel_receiver: oneshot::Receiver<()>,
        ) -> (String, Self::UpdateStream) {
            let stream = async_generator::generate(move |mut co| async move {
                co.yield_(State::Prepare).await;
                if let Ok(()) = cancel_receiver.await {
                    co.yield_(State::Canceled).await;
                }
            })
            .into_yielded();
            ("my-attempt".to_string(), Box::pin(stream))
        }
    }

    #[fuchsia::test]
    async fn cancel_update() {
        let inspector = Inspector::default();
        let node = inspector.root().create_child("current_attempt");
        let (mut install_manager_ch, fut) = start_install_manager::<
            FakeStateNotifier,
            CancelableUpdater,
            StubEnvironmentConnector,
        >(CancelableUpdater, node)
        .await;
        let _install_manager_task = fasync::Task::local(fut);

        assert_eq!(
            install_manager_ch.cancel_update(None).await,
            Ok(Err(CancelError::NoUpdateInProgress))
        );

        let (notifier, mut state_receiver) = FakeStateNotifier::new_callback_and_receiver();
        assert_eq!(
            install_manager_ch
                .start_update(ConfigBuilder::new().build().unwrap(), notifier, None)
                .await,
            Ok(Ok("my-attempt".to_string()))
        );

        assert_eq!(state_receiver.next().await, Some(State::Prepare));

        assert_eq!(install_manager_ch.cancel_update(Some("my-attempt".into())).await, Ok(Ok(())));

        assert_eq!(state_receiver.next().await, Some(State::Canceled));

        assert_eq!(
            install_manager_ch.cancel_update(Some("my-attempt".into())).await,
            Ok(Err(CancelError::NoUpdateInProgress))
        );

        assert_data_tree!(
            inspector,
            root: {
                current_attempt: contains {
                    requests: {
                        "0": {
                            request: "cancel",
                            already_in_progress_attempt_id: "None",
                            boot_ns: AnyProperty,
                        },
                        "1": contains {
                            request: "start",
                        },
                        "2": {
                            request: "cancel",
                            already_in_progress_attempt_id: r#"Some("my-attempt")"#,
                            attempt_id: "my-attempt",
                            boot_ns: AnyProperty,
                        },
                        "3": {
                            request: "cancel",
                            already_in_progress_attempt_id: "None",
                            attempt_id: "my-attempt",
                            boot_ns: AnyProperty,
                        },
                    },
                }
            }
        );
    }

    #[fuchsia::test]
    async fn cancel_update_limit() {
        let inspector = Inspector::default();
        let node = inspector.root().create_child("current_attempt");
        let (mut install_manager_ch, fut) = start_install_manager::<
            FakeStateNotifier,
            CancelableUpdater,
            StubEnvironmentConnector,
        >(CancelableUpdater, node)
        .await;
        let _install_manager_task = fasync::Task::local(fut);

        for _ in 0..MAX_NUM_CANCEL_REQUESTS {
            let (notifier, mut state_receiver) = FakeStateNotifier::new_callback_and_receiver();
            assert_eq!(
                install_manager_ch
                    .start_update(ConfigBuilder::new().build().unwrap(), notifier, None)
                    .await,
                Ok(Ok("my-attempt".to_string()))
            );
            assert_eq!(state_receiver.next().await, Some(State::Prepare));
            assert_eq!(install_manager_ch.cancel_update(None).await, Ok(Ok(())));
            assert_eq!(state_receiver.next().await, Some(State::Canceled));
        }

        let (notifier, mut state_receiver) = FakeStateNotifier::new_callback_and_receiver();
        assert_eq!(
            install_manager_ch
                .start_update(ConfigBuilder::new().build().unwrap(), notifier, None)
                .await,
            Ok(Ok("my-attempt".to_string()))
        );
        assert_eq!(state_receiver.next().await, Some(State::Prepare));
        assert_eq!(
            install_manager_ch.cancel_update(None).await,
            Ok(Err(CancelError::CancelLimitExceeded))
        );
    }

    #[fuchsia::test]
    async fn cancel_update_errors() {
        let (mut install_manager_ch, _install_manager_task, _updater_sender, _state_sender) =
            start_install_manager_with_update_id("my-attempt").await;

        let (notifier, _state_receiver) = FakeStateNotifier::new_callback_and_receiver();
        assert_eq!(
            install_manager_ch
                .start_update(ConfigBuilder::new().build().unwrap(), notifier, None)
                .await,
            Ok(Ok("my-attempt".to_string()))
        );

        assert_eq!(
            install_manager_ch.cancel_update(Some("wrong-id".into())).await,
            Ok(Err(CancelError::AttemptIdMismatch))
        );
        assert_eq!(
            install_manager_ch.cancel_update(None).await,
            Ok(Err(CancelError::UpdateCannotBeCanceled))
        );
    }

    #[fuchsia::test]
    async fn fail_additional_start_requests_when_config_incompatible() {
        let (mut install_manager_ch, _install_manager_task, mut updater_sender, state_sender0) =
            start_install_manager_with_update_id("first-attempt-id").await;
        let (notifier0, mut state_receiver0) = FakeStateNotifier::new_callback_and_receiver();
        let (notifier1, _state_receiver1) = FakeStateNotifier::new_callback_and_receiver();
        let (notifier2, _state_receiver2) = FakeStateNotifier::new_callback_and_receiver();
        let (notifier3, _state_receiver3) = FakeStateNotifier::new_callback_and_receiver();
        let (notifier4, _state_receiver4) = FakeStateNotifier::new_callback_and_receiver();

        assert_eq!(
            install_manager_ch
                .start_update(ConfigBuilder::new().build().unwrap(), notifier0, None)
                .await,
            Ok(Ok("first-attempt-id".to_string()))
        );

        // Fails because allow_attach_to_existing_attempt is false.
        assert_eq!(
            install_manager_ch
                .start_update(ConfigBuilder::new().build().unwrap(), notifier1, None)
                .await,
            Ok(Err(UpdateNotStartedReason::AlreadyInProgress))
        );

        // Fails because different update URL.
        assert_eq!(
            install_manager_ch
                .start_update(
                    ConfigBuilder::new()
                        .update_url("fuchsia-pkg://fuchsia.test/different-url")
                        .allow_attach_to_existing_attempt(true)
                        .build()
                        .unwrap(),
                    notifier2,
                    None
                )
                .await,
            Ok(Err(UpdateNotStartedReason::AlreadyInProgress))
        );

        // Fails because incompatible configs (i.e. should_write_recovery is different).
        assert_eq!(
            install_manager_ch
                .start_update(
                    ConfigBuilder::new()
                        .allow_attach_to_existing_attempt(true)
                        .should_write_recovery(false)
                        .build()
                        .unwrap(),
                    notifier3,
                    None
                )
                .await,
            Ok(Err(UpdateNotStartedReason::AlreadyInProgress))
        );

        // Fails because we can't attach reboot controller in second start request.
        assert_eq!(
            install_manager_ch
                .start_update(
                    ConfigBuilder::new().allow_attach_to_existing_attempt(true).build().unwrap(),
                    notifier4,
                    Some(RebootController::unblocked()),
                )
                .await,
            Ok(Err(UpdateNotStartedReason::AlreadyInProgress))
        );

        // End the current update attempt.
        drop(state_sender0);
        assert_eq!(state_receiver0.next().await, None);

        // Set what update() should return in the second attempt.
        let (_state_sender1, recv) = mpsc::channel(0);
        updater_sender.try_send(("second-attempt-id".to_string(), recv)).unwrap();

        // Now that there's no update in progress, start_update should work regardless of config.
        let (notifier5, _state_receiver5) = FakeStateNotifier::new_callback_and_receiver();
        assert_eq!(
            install_manager_ch
                .start_update(ConfigBuilder::new().build().unwrap(), notifier5, None)
                .await,
            Ok(Ok("second-attempt-id".to_string()))
        );
    }

    #[fuchsia::test]
    async fn update_attempt_finishes_after_dropping_control_handle() {
        let (mut install_manager_ch, install_manager_task, _updater_sender, mut state_sender) =
            start_install_manager_with_update_id("my-attempt").await;
        let (notifier, state_receiver) = FakeStateNotifier::new_callback_and_receiver();

        assert_eq!(
            install_manager_ch
                .start_update(ConfigBuilder::new().build().unwrap(), notifier, None)
                .await,
            Ok(Ok("my-attempt".to_string()))
        );

        // Close the channel that sends ControlRequests to the update manager task.
        drop(install_manager_ch);

        // Even though the ControlRequest channel was dropped, the update attempt should still
        // be running it should be able to receive state events from the monitor stream.
        let () = state_sender.send(State::Prepare).await.unwrap();
        let () =
            state_sender.send(State::FailPrepare(PrepareFailureReason::Internal)).await.unwrap();

        // Even if we drop the sender (which ends the update attempt), the state receivers
        // should still receive all the events we've sent up until this point.
        drop(state_sender);
        assert_eq!(
            state_receiver.collect::<Vec<State>>().await,
            vec![State::Prepare, State::FailPrepare(PrepareFailureReason::Internal)]
        );

        // Ensures the update manager task stops after it sends the buffered state events to monitors.
        install_manager_task.await;
    }

    #[fuchsia::test]
    async fn writes_status_update_to_inspect() {
        let inspector = Inspector::default();
        let (mut install_manager_ch, _install_manager_task, _updater_sender, mut state_sender) =
            start_install_manager_with_update_id_and_node(
                "my-attempt",
                inspector.root().create_child("install_manager"),
            )
            .await;
        let (notifier, mut state_receiver) = FakeStateNotifier::new_callback_and_receiver();
        assert_eq!(
            install_manager_ch
                .start_update(ConfigBuilder::new().build().unwrap(), notifier, None)
                .await,
            Ok(Ok("my-attempt".to_string()))
        );

        let () = state_sender.send(State::Prepare).await.unwrap();

        // Note for inspect tests: it is very important that we read the state from monitors
        // to prevent race conditions. We can only guarantee inspect state is written once the
        // status update is forwarded to monitors.
        let _ = state_receiver.next().await;

        assert_data_tree!(
            inspector,
            root: {
                install_manager: {
                    current_attempt: {
                        start_boot_ns: AnyProperty,
                        status: {
                            state: "prepare"
                        },
                    },
                    requests: {
                        "0": {
                            request: "start",
                            already_in_progress_attempt_id: "None",
                            boot_ns: AnyProperty,
                            allow_attach_to_existing_attempt: "false",
                            initiator: "Manual",
                            should_write_recovery: "true",
                            update_url: "fuchsia-pkg://fuchsia.test/update",
                        },
                    },
                }
            }
        );
    }

    #[fuchsia::test]
    async fn writes_newest_status_update_to_inspect() {
        let inspector = Inspector::default();
        let (mut install_manager_ch, _install_manager_task, _updater_sender, mut state_sender) =
            start_install_manager_with_update_id_and_node(
                "my-attempt",
                inspector.root().create_child("install_manager"),
            )
            .await;
        let (notifier, mut state_receiver) = FakeStateNotifier::new_callback_and_receiver();
        assert_eq!(
            install_manager_ch
                .start_update(ConfigBuilder::new().build().unwrap(), notifier, None)
                .await,
            Ok(Ok("my-attempt".to_string()))
        );

        let () = state_sender.send(State::Prepare).await.unwrap();
        let () = state_sender
            .send(State::Fetch(
                UpdateInfoAndProgress::builder()
                    .info(UpdateInfo::builder().download_size(100).build())
                    .progress(
                        Progress::builder().fraction_completed(0.5).bytes_downloaded(50).build(),
                    )
                    .build(),
            ))
            .await
            .unwrap();
        let _ = state_receiver.next().await;
        let _ = state_receiver.next().await;

        assert_data_tree!(
            inspector,
            root: {
                install_manager: {
                    current_attempt: {
                        start_boot_ns: AnyProperty,
                        status: {
                            state: "fetch",
                            info: {
                                download_size: 100u64,
                            },
                            progress: {
                                fraction_completed: 0.5,
                                bytes_downloaded: 50u64,
                            },
                        },
                    },
                    requests: {
                        "0": contains {
                            request: "start",
                        },
                    },
                }
            }
        );
    }

    #[fuchsia::test]
    async fn writes_status_update_to_inspect_on_second_attempt() {
        let inspector = Inspector::default();
        let (mut install_manager_ch, _install_manager_task, mut updater_sender, mut state_sender) =
            start_install_manager_with_update_id_and_node(
                "first-attempt-id",
                inspector.root().create_child("install_manager"),
            )
            .await;
        let (notifier, mut state_receiver) = FakeStateNotifier::new_callback_and_receiver();

        // Start first update attempt and show status node is populated.
        assert_eq!(
            install_manager_ch
                .start_update(ConfigBuilder::new().build().unwrap(), notifier, None)
                .await,
            Ok(Ok("first-attempt-id".to_string()))
        );
        let () = state_sender.send(State::Prepare).await.unwrap();
        let _ = state_receiver.next().await;
        assert_data_tree!(
            inspector,
            root: {
                install_manager: {
                    current_attempt: {
                        start_boot_ns: AnyProperty,
                        status: {
                            state: "prepare"
                        },
                    },
                    requests: {
                        "0": contains {
                            request: "start",
                        },
                    },
                }
            }
        );

        // End the first update attempt, show status node is removed.
        drop(state_sender);
        let _ = state_receiver.next().await;
        assert_data_tree!(
            inspector,
            root: {
                install_manager: {
                    current_attempt: {},
                    requests: {
                        "0": contains {
                            request: "start",
                        },
                    },
                }
            }
        );

        // Start second update attempt and show status node is once again populated.
        let (mut state_sender1, recv) = mpsc::channel(0);
        updater_sender.try_send(("second-attempt-id".to_string(), recv)).unwrap();
        let (notifier, mut state_receiver1) = FakeStateNotifier::new_callback_and_receiver();
        assert_eq!(
            install_manager_ch
                .start_update(ConfigBuilder::new().build().unwrap(), notifier, None)
                .await,
            Ok(Ok("second-attempt-id".to_string()))
        );
        let () =
            state_sender1.send(State::FailPrepare(PrepareFailureReason::Internal)).await.unwrap();
        let _ = state_receiver1.next().await;
        assert_data_tree!(
            inspector,
            root: {
                install_manager: {
                    current_attempt: {
                        start_boot_ns: AnyProperty,
                        status: {
                            state: "fail_prepare",
                            reason: "Internal",
                        },
                    },
                    requests: {
                        "0": contains {
                            request: "start",
                        },
                        "1": contains {
                            request: "start",
                        },
                    },
                }
            }
        );
    }

    #[fuchsia::test]
    async fn writes_empty_attempt_node_if_no_attempt_running() {
        let inspector = Inspector::default();
        let (mut _install_manager_ch, _install_manager_task, _updater_sender, _state_sender) =
            start_install_manager_with_update_id_and_node(
                "my-attempt",
                inspector.root().create_child("current_attempt"),
            )
            .await;

        assert_data_tree!(
            inspector,
            root: {
                current_attempt: {}
            }
        );
    }

    /// The update attempt has started (so we should have a status node), but
    /// we haven't gotten a status update (so the said node should be empty).
    #[fuchsia::test]
    async fn writes_empty_status_node() {
        let inspector = Inspector::default();
        let (mut install_manager_ch, _install_manager_task, _updater_sender, _state_sender) =
            start_install_manager_with_update_id_and_node(
                "my-attempt",
                inspector.root().create_child("install_manager"),
            )
            .await;
        let (notifier, _state_receiver) = FakeStateNotifier::new_callback_and_receiver();
        assert_eq!(
            install_manager_ch
                .start_update(ConfigBuilder::new().build().unwrap(), notifier, None)
                .await,
            Ok(Ok("my-attempt".to_string()))
        );

        assert_data_tree!(
            inspector,
            root: {
                install_manager: {
                    current_attempt: {
                        start_boot_ns: AnyProperty,
                        status: {},
                    },
                    requests: {
                        "0": contains {
                            request: "start",
                        },
                    }
                }
            }
        );
    }
}

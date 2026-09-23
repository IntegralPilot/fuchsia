// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::api_metrics::{ApiEvent, ApiMetricsReporter};
use crate::app_set::FuchsiaAppSet;
use crate::inspect::{AppsNode, StateNode};
use anyhow::{Context as _, Error, anyhow};
use channel_config::ChannelConfigs;
use event_queue::{ClosedClient, ControlHandle, Event, EventQueue, Notify};
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_power::{CollaborativeRebootInitiatorMarker, CollaborativeRebootInitiatorProxy};
use fidl_fuchsia_power_internal::{
    CollaborativeRebootReason, CollaborativeRebootSchedulerMarker,
    CollaborativeRebootSchedulerProxy,
};
use fidl_fuchsia_update::{
    self as update, AttemptsMonitorMarker, CheckNotStartedReason, CheckingForUpdatesData,
    ErrorCheckingForUpdateData, Initiator, InstallationDeferralReason, InstallationDeferredData,
    InstallationErrorData, InstallationProgress, InstallingData, ListenerRequest,
    ListenerRequestStream, ManagerRequest, ManagerRequestStream, MonitorMarker, MonitorProxy,
    MonitorProxyInterface, NoUpdateAvailableData, NotifierProxy, UpdateInfo,
};
use fidl_fuchsia_update_channel::{ProviderRequest, ProviderRequestStream};
use fidl_fuchsia_update_channelcontrol::{ChannelControlRequest, ChannelControlRequestStream};
use fidl_fuchsia_update_ext::AttemptOptions;
use fidl_fuchsia_update_verify::{
    ComponentOtaHealthCheckRequest, ComponentOtaHealthCheckRequestStream, HealthStatus,
};
use fuchsia_async as fasync;
use fuchsia_component::client::connect_to_protocol;
use fuchsia_component::server::{ServiceFs, ServiceObjLocal};
use futures::future::BoxFuture;
use futures::lock::Mutex;
use futures::prelude::*;
use log::{error, info, warn};
use omaha_client::app_set::{AppSet as _, AppSetExt as _};
use omaha_client::common::CheckOptions;
use omaha_client::protocol::request::InstallSource;
use omaha_client::state_machine::{self, StartUpdateCheckResponse, StateMachineGone};
use omaha_client::storage::{Storage, StorageExt};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub manager_state: state_machine::State,
    pub version_available: Option<String>,
    pub install_progress: Option<f32>,
    pub urgent: Option<bool>,
}

impl From<State> for Option<update::State> {
    fn from(state: State) -> Self {
        let update = Some(UpdateInfo {
            version_available: state.version_available,
            urgent: state.urgent,
            download_size: None,
            ..Default::default()
        });
        let installation_progress = Some(InstallationProgress {
            fraction_completed: state.install_progress,
            ..Default::default()
        });
        match state.manager_state {
            state_machine::State::Idle => None,
            state_machine::State::CheckingForUpdates(_) => {
                Some(update::State::CheckingForUpdates(CheckingForUpdatesData::default()))
            }
            state_machine::State::ErrorCheckingForUpdate => {
                Some(update::State::ErrorCheckingForUpdate(ErrorCheckingForUpdateData::default()))
            }
            state_machine::State::NoUpdateAvailable => {
                Some(update::State::NoUpdateAvailable(NoUpdateAvailableData::default()))
            }
            state_machine::State::InstallationDeferredByPolicy => {
                Some(update::State::InstallationDeferredByPolicy(InstallationDeferredData {
                    update,
                    // For now, we deliberately only support one deferral reason. When we simplify
                    // the StateMachine type parameters, consider modifying the binary to support
                    // multiple deferral reasons.
                    deferral_reason: Some(InstallationDeferralReason::CurrentSystemNotCommitted),
                    ..Default::default()
                }))
            }
            state_machine::State::InstallingUpdate => {
                Some(update::State::InstallingUpdate(InstallingData {
                    update,
                    installation_progress,
                    ..Default::default()
                }))
            }
            state_machine::State::WaitingForReboot => {
                Some(update::State::WaitingForReboot(InstallingData {
                    update,
                    installation_progress,
                    ..Default::default()
                }))
            }
            state_machine::State::InstallationError => {
                Some(update::State::InstallationError(InstallationErrorData {
                    update,
                    installation_progress,
                    ..Default::default()
                }))
            }
        }
    }
}

#[derive(Clone, Debug)]
struct StateNotifier {
    proxy: MonitorProxy,
}

impl Notify for StateNotifier {
    type Event = State;
    type NotifyFuture = futures::future::Either<
        futures::future::Map<
            <MonitorProxy as MonitorProxyInterface>::OnStateResponseFut,
            fn(Result<(), fidl::Error>) -> Result<(), ClosedClient>,
        >,
        futures::future::Ready<Result<(), ClosedClient>>,
    >;

    fn notify(&self, state: State) -> Self::NotifyFuture {
        let map_fidl_err_to_closed: fn(Result<(), fidl::Error>) -> Result<(), ClosedClient> =
            |res| res.map_err(|_| ClosedClient);

        match state.into() {
            Some(state) => self.proxy.on_state(&state).map(map_fidl_err_to_closed).left_future(),
            None => future::ready(Ok(())).right_future(),
        }
    }
}

impl Event for State {
    fn can_merge(&self, other: &State) -> bool {
        if self.manager_state != other.manager_state {
            return false;
        }
        if self.version_available != other.version_available {
            warn!("version_available mismatch between two states: {:?}, {:?}", self, other);
        }
        true
    }
}

#[derive(Clone, Debug)]
struct AttemptNotifier {
    proxy: fidl_fuchsia_update::AttemptsMonitorProxy,
    control_handle: ControlHandle<StateNotifier>,
}

impl Notify for AttemptNotifier {
    type Event = fidl_fuchsia_update_ext::AttemptOptions;
    type NotifyFuture = futures::future::BoxFuture<'static, Result<(), ClosedClient>>;

    fn notify(&self, options: fidl_fuchsia_update_ext::AttemptOptions) -> Self::NotifyFuture {
        let mut update_attempt_event_queue = self.control_handle.clone();
        let proxy = self.proxy.clone();

        async move {
            let (monitor_proxy, monitor_server_end) =
                fidl::endpoints::create_proxy::<fidl_fuchsia_update::MonitorMarker>();
            update_attempt_event_queue
                .add_client(StateNotifier { proxy: monitor_proxy })
                .await
                .map_err(|_| ClosedClient)?;
            proxy.on_start(&options.into(), monitor_server_end).await.map_err(|_| ClosedClient)
        }
        .boxed()
    }
}

pub trait StateMachineController: Clone {
    fn start_update_check(
        &mut self,
        options: CheckOptions,
    ) -> BoxFuture<'_, Result<StartUpdateCheckResponse, StateMachineGone>>;
}

impl StateMachineController for state_machine::ControlHandle {
    fn start_update_check(
        &mut self,
        options: CheckOptions,
    ) -> BoxFuture<'_, Result<StartUpdateCheckResponse, StateMachineGone>> {
        self.start_update_check(options).boxed()
    }
}

pub struct FidlServer<ST, SM>
where
    ST: Storage,
    SM: StateMachineController,
{
    state_machine_control: SM,

    storage_ref: Rc<Mutex<ST>>,

    app_set: Rc<Mutex<FuchsiaAppSet>>,

    apps_node: AppsNode,

    state_node: StateNode,

    channel_configs: Option<ChannelConfigs>,

    // The current State, this is the internal representation of the fuchsia.update/State.
    state: State,

    single_monitor_queue: ControlHandle<StateNotifier>,

    attempt_monitor_queue: ControlHandle<AttemptNotifier>,

    metrics_reporter: Box<dyn ApiMetricsReporter>,

    completion_responder: CompletionResponder,

    current_channel: Option<String>,

    valid_service_url: bool,
}

pub enum IncomingServices {
    Manager(ManagerRequestStream),
    ChannelControl(ChannelControlRequestStream),
    ChannelProvider(ProviderRequestStream),
    Listener(ListenerRequestStream),
    HealthCheck(ComponentOtaHealthCheckRequestStream),
}

impl<ST, SM> FidlServer<ST, SM>
where
    ST: Storage + 'static,
    SM: StateMachineController,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        state_machine_control: SM,
        storage_ref: Rc<Mutex<ST>>,
        app_set: Rc<Mutex<FuchsiaAppSet>>,
        apps_node: AppsNode,
        state_node: StateNode,
        channel_configs: Option<ChannelConfigs>,
        metrics_reporter: Box<dyn ApiMetricsReporter>,
        current_channel: Option<String>,
        valid_service_url: bool,
    ) -> Self {
        let state = State {
            manager_state: state_machine::State::Idle,
            version_available: None,
            install_progress: None,
            urgent: None,
        };
        state_node.set(&state);
        let (single_monitor_queue_fut, single_monitor_queue) = EventQueue::new();
        let (attempt_monitor_queue_fut, attempt_monitor_queue) = EventQueue::new();
        fasync::Task::local(single_monitor_queue_fut).detach();
        fasync::Task::local(attempt_monitor_queue_fut).detach();
        let completion_responder = CompletionResponder::new();
        FidlServer {
            state_machine_control,
            storage_ref,
            app_set,
            apps_node,
            state_node,
            channel_configs,
            state,
            single_monitor_queue,
            attempt_monitor_queue,
            metrics_reporter,
            completion_responder,
            current_channel,
            valid_service_url,
        }
    }

    /// Runs the FIDL Server and the StateMachine.
    pub async fn run(
        server: Rc<RefCell<Self>>,
        mut fs: ServiceFs<ServiceObjLocal<'_, IncomingServices>>,
    ) {
        fs.dir("svc")
            .add_fidl_service(IncomingServices::Manager)
            .add_fidl_service(IncomingServices::ChannelControl)
            .add_fidl_service(IncomingServices::ChannelProvider)
            .add_fidl_service(IncomingServices::Listener)
            .add_fidl_service(IncomingServices::HealthCheck);
        const MAX_CONCURRENT: usize = 1000;
        // Handle each client connection concurrently.
        fs.for_each_concurrent(MAX_CONCURRENT, |stream| async {
            Self::handle_client(Rc::clone(&server), stream, &mut CollaborativeRebootFromSvcDir {})
                .await
                .unwrap_or_else(|e| error!("{:?}", e))
        })
        .await
    }

    /// Handle an incoming FIDL connection from a client.
    async fn handle_client<P: CollaborativeRebootProvider>(
        server: Rc<RefCell<Self>>,
        stream: IncomingServices,
        cr_provider: &mut P,
    ) -> Result<(), Error> {
        match stream {
            IncomingServices::Manager(mut stream) => {
                server.borrow_mut().metrics_reporter.emit_event(ApiEvent::UpdateManagerConnection);

                while let Some(request) =
                    stream.try_next().await.context("error receiving Manager request")?
                {
                    Self::handle_manager_request(Rc::clone(&server), request, cr_provider).await?;
                }
            }
            IncomingServices::ChannelControl(mut stream) => {
                while let Some(request) =
                    stream.try_next().await.context("error receiving ChannelControl request")?
                {
                    Self::handle_channel_control_request(Rc::clone(&server), request).await?;
                }
            }
            IncomingServices::ChannelProvider(mut stream) => {
                while let Some(request) =
                    stream.try_next().await.context("error receiving Provider request")?
                {
                    Self::handle_channel_provider_request(Rc::clone(&server), request).await?;
                }
            }
            IncomingServices::Listener(mut stream) => {
                while let Some(request) =
                    stream.try_next().await.context("error receiving CheckCompletion request")?
                {
                    Self::handle_on_update_check_completion_request(Rc::clone(&server), request)
                        .await?;
                }
            }
            IncomingServices::HealthCheck(mut stream) => {
                let valid_url = server.borrow().valid_service_url;

                while let Some(ComponentOtaHealthCheckRequest::GetHealthStatus { responder }) =
                    stream.try_next().await.expect("error running health check service")
                {
                    if valid_url {
                        responder
                            .send(HealthStatus::Healthy)
                            .expect("failed to send healthy status");
                    } else {
                        responder
                            .send(HealthStatus::Unhealthy)
                            .expect("failed to send unhealthy status");
                    }
                }
            }
        }
        Ok(())
    }

    /// Handle fuchsia.update.Manager requests.
    async fn handle_manager_request<P: CollaborativeRebootProvider>(
        server: Rc<RefCell<Self>>,
        request: ManagerRequest,
        cr_provider: &mut P,
    ) -> Result<(), Error> {
        match request {
            ManagerRequest::CheckNow { options, monitor, responder } => {
                let res = Self::handle_check_now(Rc::clone(&server), options, monitor).await;

                server
                    .borrow_mut()
                    .metrics_reporter
                    .emit_event(ApiEvent::UpdateManagerCheckNowResult(res));

                responder.send(res).context("error sending CheckNow response")?;
            }

            ManagerRequest::PerformPendingReboot { responder } => {
                // TODO(https://fxbug.dev/389134835): Delete this method once
                // it's removed from the FIDL. For now, proxy the request to
                // the new API.
                info!("Received PerformPendingReboot. Proxying it for Collaborative Reboot.");
                let response = cr_provider.new_initiator_proxy()?.perform_pending_reboot().await?;
                let rebooting = response.rebooting;
                info!("Received PerformPendingReboot response: rebooting={rebooting:?}");
                responder.send(rebooting.unwrap_or(false))?
            }

            ManagerRequest::MonitorAllUpdateChecks { attempts_monitor, control_handle: _ } => {
                if let Err(e) = Self::handle_monitor_all_updates(server, attempts_monitor).await {
                    error!("error monitoring all update checks: {:#}", anyhow!(e))
                }
            }
        }
        Ok(())
    }

    /// Handle fuchsia.update.channelcontrol.ChannelControl requests.
    async fn handle_channel_control_request(
        server: Rc<RefCell<Self>>,
        request: ChannelControlRequest,
    ) -> Result<(), Error> {
        match request {
            ChannelControlRequest::SetTarget { channel, responder } => {
                info!("Received SetTarget request with {}", channel);

                server
                    .borrow_mut()
                    .metrics_reporter
                    .emit_event(ApiEvent::UpdateChannelControlSetTarget);

                Self::handle_set_target(server, channel).await;

                responder.send().context("error sending SetTarget response from ChannelControl")?;
            }
            ChannelControlRequest::GetTarget { responder } => {
                let app_set = Rc::clone(&server.borrow().app_set);
                let app_set = app_set.lock().await;
                let channel = app_set.get_system_target_channel();
                responder
                    .send(channel)
                    .context("error sending GetTarget response from ChannelControl")?;
            }
            ChannelControlRequest::GetCurrent { responder } => {
                let (current_channel, app_set) = {
                    let server = server.borrow();
                    (server.current_channel.clone(), Rc::clone(&server.app_set))
                };
                let channel = match current_channel {
                    Some(channel) => channel,
                    None => app_set.lock().await.get_system_current_channel().to_owned(),
                };

                responder
                    .send(&channel)
                    .context("error sending GetCurrent response from ChannelControl")?;
            }
            ChannelControlRequest::GetTargetList { responder } => {
                let server = server.borrow();
                let channel_names: Vec<String> = match &server.channel_configs {
                    Some(channel_configs) => {
                        channel_configs.known_channels.iter().map(|cfg| cfg.name.clone()).collect()
                    }
                    None => Vec::new(),
                };
                responder
                    .send(&channel_names)
                    .context("error sending channel list response from ChannelControl")?;
            }
        }
        Ok(())
    }

    async fn handle_channel_provider_request(
        server: Rc<RefCell<Self>>,
        request: ProviderRequest,
    ) -> Result<(), Error> {
        match request {
            ProviderRequest::GetCurrent { responder } => {
                let (current_channel, app_set) = {
                    let server = server.borrow();
                    (server.current_channel.clone(), Rc::clone(&server.app_set))
                };
                let channel = match current_channel {
                    Some(channel) => channel,
                    None => app_set.lock().await.get_system_current_channel().to_owned(),
                };
                responder
                    .send(&channel)
                    .context("error sending GetCurrent response from Provider")?;
            }
        }
        Ok(())
    }

    async fn handle_on_update_check_completion_request(
        server: Rc<RefCell<Self>>,
        request: ListenerRequest,
    ) -> Result<(), Error> {
        match request {
            ListenerRequest::NotifyOnFirstUpdateCheck { payload, control_handle } => {
                let notifier = match payload.notifier {
                    Some(notifier) => notifier,
                    None => {
                        // This is a required field.
                        control_handle.shutdown_with_epitaph(zx::Status::INVALID_ARGS);
                        return Ok(());
                    }
                };
                server
                    .borrow_mut()
                    .completion_responder
                    .notify_when_appropriate(notifier.into_proxy());
            }
            ListenerRequest::_UnknownMethod { .. } => {}
        }
        Ok(())
    }

    async fn handle_check_now(
        server: Rc<RefCell<Self>>,
        options: fidl_fuchsia_update::CheckOptions,
        monitor: Option<ClientEnd<MonitorMarker>>,
    ) -> Result<(), CheckNotStartedReason> {
        info!("Received CheckNow request with {:?} and {:?}", options, monitor);

        let source = match options.initiator {
            Some(Initiator::User) => InstallSource::OnDemand,
            Some(Initiator::Service) => InstallSource::ScheduledTask,
            None => {
                return Err(CheckNotStartedReason::InvalidOptions);
            }
        };

        let mut state_machine_control = server.borrow().state_machine_control.clone();

        let check_options = CheckOptions { source };

        match state_machine_control.start_update_check(check_options).await {
            Ok(StartUpdateCheckResponse::Started) => {}
            Ok(StartUpdateCheckResponse::AlreadyRunning) => {
                if options.allow_attaching_to_existing_update_check != Some(true) {
                    return Err(CheckNotStartedReason::AlreadyInProgress);
                }
            }
            Ok(StartUpdateCheckResponse::Throttled) => {
                return Err(CheckNotStartedReason::Throttled);
            }
            Err(state_machine::StateMachineGone) => return Err(CheckNotStartedReason::Internal),
        }

        // Attach the monitor if passed for current update.
        if let Some(monitor) = monitor {
            let monitor_proxy = monitor.into_proxy();
            let mut single_monitor_queue = server.borrow().single_monitor_queue.clone();
            single_monitor_queue.add_client(StateNotifier { proxy: monitor_proxy }).await.map_err(
                |e| {
                    error!("error adding client to single_monitor_queue: {:?}", e);
                    CheckNotStartedReason::Internal
                },
            )?;
        }

        Ok(())
    }

    async fn handle_monitor_all_updates(
        server: Rc<RefCell<Self>>,
        attempts_monitor: ClientEnd<AttemptsMonitorMarker>,
    ) -> Result<(), Error> {
        let proxy = attempts_monitor.into_proxy();
        let mut attempt_monitor_queue = server.borrow().attempt_monitor_queue.clone();
        let control_handle = server.borrow().single_monitor_queue.clone();
        attempt_monitor_queue.add_client(AttemptNotifier { proxy, control_handle }).await?;
        Ok(())
    }

    async fn handle_set_target(server: Rc<RefCell<Self>>, channel: String) {
        // TODO: Verify that channel is valid.
        let app_set = Rc::clone(&server.borrow().app_set);
        let target_channel = app_set.lock().await.get_system_target_channel().to_owned();
        if channel.is_empty() {
            let default_channel_cfg = match &server.borrow().channel_configs {
                Some(cfgs) => cfgs.get_default_channel(),
                None => None,
            };
            let (channel_name, appid) = match default_channel_cfg {
                Some(cfg) => (Some(cfg.name), cfg.appid),
                None => (None, None),
            };
            if let Some(name) = &channel_name {
                // If the default channel is the same as the target channel, then this is a no-op.
                if name == &target_channel {
                    return;
                }
                warn!("setting device to default channel: '{}' with app id: '{:?}'", name, appid);
            }
            // TODO(https://fxbug.dev/42136893): only OTA that follows can change the current channel.
            // Simplify this logic.
            app_set.lock().await.set_system_target_channel(channel_name, appid);
        } else {
            // If the new target channel is the same as the existing target channel, then this is
            // a no-op.
            if channel == target_channel {
                return;
            }

            let (appid, storage_ref) = {
                let server = server.borrow();
                let channel_cfg = match &server.channel_configs {
                    Some(cfgs) => cfgs.get_channel(&channel),
                    None => None,
                };
                if channel_cfg.is_none() {
                    warn!("Channel {} not found in known channels", channel);
                }
                let appid = match channel_cfg {
                    Some(ref cfg) => cfg.appid.clone(),
                    None => None,
                };
                let storage_ref = Rc::clone(&server.storage_ref);
                (appid, storage_ref)
            };

            let mut storage = storage_ref.lock().await;
            {
                let mut app_set = app_set.lock().await;
                if let Some(id) = &appid
                    && id != app_set.get_system_app_id()
                {
                    warn!("Changing app id to: {}", id);
                }

                app_set.set_system_target_channel(Some(channel), appid);
                app_set.persist(&mut *storage).await;
            }
            storage.commit_or_log().await;
        }

        let app_set = app_set.lock().await;
        server.borrow().apps_node.set(&app_set);
    }

    /// The state change callback from StateMachine.
    pub async fn on_state_change<P: CollaborativeRebootProvider>(
        server: Rc<RefCell<Self>>,
        state: state_machine::State,
        cr_provider: &mut P,
    ) {
        server.borrow_mut().state.manager_state = state;

        match state {
            state_machine::State::Idle => {
                let state = &mut server.borrow_mut().state;
                state.install_progress = None;
                state.urgent = None;
            }
            state_machine::State::WaitingForReboot => {
                server.borrow_mut().state.install_progress = Some(1.);
            }
            _ => {}
        }

        server.borrow_mut().completion_responder.react_to(&state);

        Self::send_state_to_queue(Rc::clone(&server)).await;

        match state {
            state_machine::State::Idle | state_machine::State::WaitingForReboot => {
                let (mut single_monitor_queue, app_set) = {
                    let s = server.borrow();
                    s.state_node.set(&s.state);
                    (s.single_monitor_queue.clone(), Rc::clone(&s.app_set))
                };

                // Try to flush the states before starting to reboot.
                if state == state_machine::State::WaitingForReboot {
                    match single_monitor_queue.try_flush(Duration::from_secs(5)).await {
                        Ok(flush_future) => {
                            if let Err(e) = flush_future.await {
                                warn!("Timed out flushing single_monitor_queue: {:#}", anyhow!(e));
                            }
                        }
                        Err(e) => {
                            warn!("error trying to flush single_monitor_queue: {:#}", anyhow!(e))
                        }
                    }
                }

                if let Err(e) = single_monitor_queue.clear().await {
                    warn!("error clearing clients of single_monitor_queue: {:?}", e);
                }

                // The state machine might make changes to apps only at the end of an update,
                // update the apps node in inspect.
                let app_set = app_set.lock().await;
                server.borrow().apps_node.set(&app_set);
            }
            state_machine::State::CheckingForUpdates(install_source) => {
                let attempt_options = match install_source {
                    InstallSource::OnDemand => AttemptOptions { initiator: Initiator::User.into() },
                    InstallSource::ScheduledTask => {
                        AttemptOptions { initiator: Initiator::Service.into() }
                    }
                };
                let mut attempt_monitor_queue = {
                    let s = server.borrow();
                    s.state_node.set(&s.state);
                    s.attempt_monitor_queue.clone()
                };
                if let Err(e) = attempt_monitor_queue.queue_event(attempt_options).await {
                    warn!("error sending update to attempt queue: {:#}", anyhow!(e))
                }
            }
            _ => {
                let s = server.borrow();
                s.state_node.set(&s.state);
            }
        }

        if state == state_machine::State::WaitingForReboot {
            info!("Observed State::WaitingForReboot; scheduling collaborative reboot");
            match cr_provider.new_scheduler_proxy() {
                Err(e) => error!("Failed to connect to 'CollaborativeRebootScheduler': {e:?}"),
                Ok(proxy) => {
                    match proxy.schedule_reboot(CollaborativeRebootReason::SystemUpdate, None).await
                    {
                        Ok(()) => {}
                        Err(e) => error!("Failed to schedule collaborative reboot: {e:?}"),
                    }
                }
            };
        }
    }

    async fn send_state_to_queue(server: Rc<RefCell<Self>>) {
        let (mut single_monitor_queue, state) = {
            let server = server.borrow();
            (server.single_monitor_queue.clone(), server.state.clone())
        };
        if let Err(e) = single_monitor_queue.queue_event(state).await {
            warn!("error sending state to single_monitor_queue: {:?}", e)
        }
    }

    pub async fn on_progress_change(
        server: Rc<RefCell<Self>>,
        progress: state_machine::InstallProgress,
    ) {
        server.borrow_mut().state.install_progress = Some(progress.progress);
        Self::send_state_to_queue(server).await;
    }

    pub fn set_urgent_update(server: Rc<RefCell<Self>>, is_urgent: bool) {
        server.borrow_mut().state.urgent = Some(is_urgent);
    }
}

enum CompletionResponder {
    Waiting { notifiers: Vec<NotifierProxy> },
    Satisfied,
}

impl CompletionResponder {
    fn new() -> Self {
        CompletionResponder::Waiting { notifiers: Vec::new() }
    }

    fn notify_when_appropriate(&mut self, notifier: NotifierProxy) {
        match self {
            CompletionResponder::Waiting { notifiers } => notifiers.push(notifier),
            CompletionResponder::Satisfied => {
                if let Err(e) = notifier.notify() {
                    warn!(
                        "Received FIDL error notifying client of software update completion: {e:?}"
                    );
                }
            }
        }
    }

    fn react_to(&mut self, state: &state_machine::State) {
        // If a software update has completed, with or without error, and no reboot is needed,
        // then send any waiting notifications and change state to Satisfied. Otherwise return.
        match state {
            state_machine::State::ErrorCheckingForUpdate
            | state_machine::State::NoUpdateAvailable
            | state_machine::State::InstallationDeferredByPolicy => {}
            state_machine::State::Idle
            | state_machine::State::InstallingUpdate
            | state_machine::State::WaitingForReboot
            | state_machine::State::InstallationError
            | state_machine::State::CheckingForUpdates(_) => return,
        }
        match self {
            CompletionResponder::Satisfied => (),
            CompletionResponder::Waiting { notifiers } => {
                for notifier in notifiers.drain(..) {
                    if let Err(e) = notifier.notify() {
                        warn!(
                            "Received FIDL error notifying client of software update completion: {e:?}"
                        );
                    }
                }
                *self = CompletionResponder::Satisfied;
            }
        }
    }
}

/// A provider of Collaborative Reboot protocol connections
pub trait CollaborativeRebootProvider {
    fn new_scheduler_proxy(&mut self) -> Result<CollaborativeRebootSchedulerProxy, Error>;
    fn new_initiator_proxy(&mut self) -> Result<CollaborativeRebootInitiatorProxy, Error>;
}

/// An implementation of [`CollaborativeRebootProvider`] that connects to the
/// FIDL protocols from the svc directory.
pub struct CollaborativeRebootFromSvcDir {}

impl CollaborativeRebootProvider for CollaborativeRebootFromSvcDir {
    fn new_scheduler_proxy(&mut self) -> Result<CollaborativeRebootSchedulerProxy, Error> {
        connect_to_protocol::<CollaborativeRebootSchedulerMarker>()
    }
    fn new_initiator_proxy(&mut self) -> Result<CollaborativeRebootInitiatorProxy, Error> {
        connect_to_protocol::<CollaborativeRebootInitiatorMarker>()
    }
}

#[cfg(test)]
pub use stub::{
    FakeCollaborativeRebootInitiator, FakeCollaborativeRebootScheduler, FidlServerBuilder,
    MockOrRealStateMachineController, MockStateMachineController, NoCollaborativeReboot,
    StubFidlServer, expect_perform_pending_reboot_once, expect_system_update_scheduled_once,
};

#[cfg(test)]
mod stub {
    use super::*;
    use crate::api_metrics::StubApiMetricsReporter;
    use crate::app_set::{AppIdSource, AppMetadata};
    use crate::configuration;
    use crate::inspect::{LastResultsNode, ProtocolStateNode, ScheduleNode};
    use crate::observer::FuchsiaObserver;
    use fidl_fuchsia_power::{
        CollaborativeRebootInitiatorPerformPendingRebootResponse as PerformPendingRebootResponse,
        CollaborativeRebootInitiatorRequest, CollaborativeRebootInitiatorRequestStream,
    };
    use fidl_fuchsia_power_internal::{
        CollaborativeRebootSchedulerRequest, CollaborativeRebootSchedulerRequestStream,
    };
    use fuchsia_inspect::Inspector;
    use omaha_client::common::{App, CheckTiming, ProtocolState, UpdateCheckSchedule};
    use omaha_client::cup_ecdsa::StandardCupv2Handler;
    use omaha_client::http_request::StubHttpRequest;
    use omaha_client::installer::stub::{StubInstaller, StubPlan};
    use omaha_client::metrics::StubMetricsReporter;
    use omaha_client::policy::{CheckDecision, PolicyEngine, UpdateDecision};
    use omaha_client::request_builder::RequestParams;
    use omaha_client::state_machine::StateMachineBuilder;
    use omaha_client::storage::MemStorage;
    use omaha_client::time::timers::InfiniteTimer;
    use omaha_client::time::{MockTimeSource, TimeSource};

    #[derive(Clone)]
    pub struct MockStateMachineController {
        result: Result<StartUpdateCheckResponse, StateMachineGone>,
    }

    impl MockStateMachineController {
        pub fn new(result: Result<StartUpdateCheckResponse, StateMachineGone>) -> Self {
            Self { result }
        }
    }

    impl StateMachineController for MockStateMachineController {
        fn start_update_check(
            &mut self,
            _options: CheckOptions,
        ) -> BoxFuture<'_, Result<StartUpdateCheckResponse, StateMachineGone>> {
            future::ready(self.result.clone()).boxed()
        }
    }

    #[derive(Clone)]
    pub enum MockOrRealStateMachineController {
        Mock(MockStateMachineController),
        Real(state_machine::ControlHandle),
    }

    impl StateMachineController for MockOrRealStateMachineController {
        fn start_update_check(
            &mut self,
            options: CheckOptions,
        ) -> BoxFuture<'_, Result<StartUpdateCheckResponse, StateMachineGone>> {
            match self {
                Self::Mock(mock) => mock.start_update_check(options),
                Self::Real(real) => real.start_update_check(options).boxed(),
            }
        }
    }

    pub type StubFidlServer = FidlServer<MemStorage, MockOrRealStateMachineController>;

    pub struct FidlServerBuilder {
        app_set: Option<FuchsiaAppSet>,
        channel_configs: Option<ChannelConfigs>,
        apps_node: Option<AppsNode>,
        state_node: Option<StateNode>,
        state_machine_control: Option<MockStateMachineController>,
        time_source: Option<MockTimeSource>,
        current_channel: Option<String>,
        valid_service_url: bool,
    }

    impl FidlServerBuilder {
        pub fn new() -> Self {
            Self {
                app_set: None,
                channel_configs: None,
                apps_node: None,
                state_node: None,
                state_machine_control: None,
                time_source: None,
                current_channel: None,
                valid_service_url: false,
            }
        }
    }

    impl FidlServerBuilder {
        pub fn with_app_set(mut self, app_set: FuchsiaAppSet) -> Self {
            self.app_set = Some(app_set);
            self
        }

        pub fn with_apps_node(mut self, apps_node: AppsNode) -> Self {
            self.apps_node = Some(apps_node);
            self
        }

        pub fn with_state_node(mut self, state_node: StateNode) -> Self {
            self.state_node = Some(state_node);
            self
        }

        pub fn with_channel_configs(mut self, channel_configs: ChannelConfigs) -> Self {
            self.channel_configs = Some(channel_configs);
            self
        }

        pub fn with_service_url(mut self) -> Self {
            self.valid_service_url = true;
            self
        }

        pub fn state_machine_control(
            mut self,
            state_machine_control: MockStateMachineController,
        ) -> Self {
            self.state_machine_control = Some(state_machine_control);
            self
        }

        pub fn with_current_channel(mut self, current_channel: Option<String>) -> Self {
            self.current_channel = current_channel;
            self
        }

        pub async fn build(self) -> Rc<RefCell<StubFidlServer>> {
            let config = configuration::get_config("0.1.2", None, None);
            let storage_ref = Rc::new(Mutex::new(MemStorage::new()));

            let cup_handler: Option<StandardCupv2Handler> =
                config.omaha_public_keys.as_ref().map(StandardCupv2Handler::new);

            let app_set = self.app_set.unwrap_or_else(|| {
                FuchsiaAppSet::new(
                    App::builder().id("id").version([1, 0]).build(),
                    AppMetadata { appid_source: AppIdSource::VbMetadata },
                )
            });
            let app_set = Rc::new(Mutex::new(app_set));
            let time_source = self.time_source.unwrap_or_else(MockTimeSource::new_from_now);
            // A state machine with only stub implementations never yields from a poll.
            // Configure the state machine to schedule automatic update checks in the future and
            // block timers forever so we can control when update checks happen.
            let (state_machine_control, state_machine) = StateMachineBuilder::new(
                MockPolicyEngine { time_source },
                StubHttpRequest,
                StubInstaller::default(),
                InfiniteTimer,
                StubMetricsReporter,
                Rc::clone(&storage_ref),
                config,
                Rc::clone(&app_set),
                cup_handler,
            )
            .start()
            .await;
            let inspector = Inspector::default();
            let root = inspector.root();

            let apps_node =
                self.apps_node.unwrap_or_else(|| AppsNode::new(root.create_child("apps")));
            let state_node =
                self.state_node.unwrap_or_else(|| StateNode::new(root.create_child("state")));
            let state_machine_control = match self.state_machine_control {
                Some(mock) => MockOrRealStateMachineController::Mock(mock),
                None => MockOrRealStateMachineController::Real(state_machine_control),
            };
            let fidl = Rc::new(RefCell::new(FidlServer::new(
                state_machine_control,
                storage_ref,
                Rc::clone(&app_set),
                apps_node,
                state_node,
                self.channel_configs,
                Box::new(StubApiMetricsReporter),
                self.current_channel,
                self.valid_service_url,
            )));

            let schedule_node = ScheduleNode::new(root.create_child("schedule"));
            let protocol_state_node = ProtocolStateNode::new(root.create_child("protocol_state"));
            let last_results_node = LastResultsNode::new(root.create_child("last_results"));
            let platform_metrics_node = root.create_child("platform_metrics");

            let mut observer = FuchsiaObserver::new(
                Rc::clone(&fidl),
                schedule_node,
                protocol_state_node,
                last_results_node,
                platform_metrics_node,
            );
            fasync::Task::local(async move {
                futures::pin_mut!(state_machine);

                while let Some(event) = state_machine.next().await {
                    observer.on_event(event).await;
                }
            })
            .detach();

            fidl
        }
    }

    /// A mock PolicyEngine implementation that allows update checks with an interval of a few
    /// seconds.
    #[derive(Debug)]
    pub struct MockPolicyEngine {
        time_source: MockTimeSource,
    }

    impl PolicyEngine for MockPolicyEngine {
        type TimeSource = MockTimeSource;
        type InstallResult = ();
        type InstallPlan = StubPlan;

        fn time_source(&self) -> &Self::TimeSource {
            &self.time_source
        }

        fn compute_next_update_time(
            &mut self,
            _apps: &[App],
            _scheduling: &UpdateCheckSchedule,
            _protocol_state: &ProtocolState,
        ) -> BoxFuture<'_, CheckTiming> {
            let timing = CheckTiming::builder()
                .time(self.time_source.now() + Duration::from_secs(3))
                .build();
            future::ready(timing).boxed()
        }

        fn update_check_allowed(
            &mut self,
            _apps: &[App],
            _scheduling: &UpdateCheckSchedule,
            _protocol_state: &ProtocolState,
            check_options: &CheckOptions,
        ) -> BoxFuture<'_, CheckDecision> {
            future::ready(CheckDecision::Ok(RequestParams {
                source: check_options.source,
                use_configured_proxies: true,
                ..RequestParams::default()
            }))
            .boxed()
        }

        fn update_can_start<'p>(
            &mut self,
            _proposed_install_plan: &'p Self::InstallPlan,
        ) -> BoxFuture<'p, UpdateDecision> {
            future::ready(UpdateDecision::Ok).boxed()
        }

        fn reboot_allowed(
            &mut self,
            _check_options: &CheckOptions,
            _install_result: &Self::InstallResult,
        ) -> BoxFuture<'_, bool> {
            future::ready(true).boxed()
        }

        fn reboot_needed(&mut self, _install_plan: &Self::InstallPlan) -> BoxFuture<'_, bool> {
            future::ready(true).boxed()
        }
    }

    /// A fake that panics if Collaborative Reboot connections are attempted.
    pub struct NoCollaborativeReboot {}

    impl CollaborativeRebootProvider for NoCollaborativeReboot {
        fn new_scheduler_proxy(&mut self) -> Result<CollaborativeRebootSchedulerProxy, Error> {
            panic!("CollaborativeRebootScheduler connection was unexpectedly created")
        }
        fn new_initiator_proxy(&mut self) -> Result<CollaborativeRebootInitiatorProxy, Error> {
            panic!("CollaborativeRebootInitiator connection was unexpectedly created")
        }
    }

    /// A fake provider of a CollaborativeRebootScheduler connection.
    pub struct FakeCollaborativeRebootScheduler {
        sender:
            Option<futures::channel::oneshot::Sender<CollaborativeRebootSchedulerRequestStream>>,
    }

    impl FakeCollaborativeRebootScheduler {
        pub fn new()
        -> (futures::channel::oneshot::Receiver<CollaborativeRebootSchedulerRequestStream>, Self)
        {
            let (sender, receiver) = futures::channel::oneshot::channel();
            (receiver, Self { sender: Some(sender) })
        }
    }

    impl CollaborativeRebootProvider for FakeCollaborativeRebootScheduler {
        fn new_scheduler_proxy(&mut self) -> Result<CollaborativeRebootSchedulerProxy, Error> {
            let (client, rs) =
                fidl::endpoints::create_request_stream::<CollaborativeRebootSchedulerMarker>();
            if let Some(sender) = self.sender.take() {
                sender.send(rs).map_err(|_rs| ()).expect("send request stream should succeed");
                Ok(client.into_proxy())
            } else {
                panic!("new_scheduler_proxy called multiple times");
            }
        }
        fn new_initiator_proxy(&mut self) -> Result<CollaborativeRebootInitiatorProxy, Error> {
            panic!("CollaborativeRebootInitiator connection was unexpectedly created")
        }
    }

    // A helper function to drive a Fake Collaborative Reboot server expecting
    // exactly once scheduled request.
    pub async fn expect_system_update_scheduled_once(
        receiver: futures::channel::oneshot::Receiver<CollaborativeRebootSchedulerRequestStream>,
    ) {
        let num_calls = receiver
            .await
            .expect("receive request stream should succeed.")
            .fold(0, |num_calls, request| {
                match request.expect("receive scheduler request should succeed") {
                    CollaborativeRebootSchedulerRequest::ScheduleReboot {
                        reason,
                        cancel,
                        responder,
                    } => {
                        assert_eq!(reason, CollaborativeRebootReason::SystemUpdate);
                        assert_eq!(cancel, None);
                        responder.send().expect("responding to schedule should succeed");
                        futures::future::ready(num_calls + 1)
                    }
                }
            })
            .await;
        assert_eq!(num_calls, 1);
    }

    /// A fake provider of a CollaborativeRebootInitiator connection.
    pub struct FakeCollaborativeRebootInitiator {
        sender:
            Option<futures::channel::oneshot::Sender<CollaborativeRebootInitiatorRequestStream>>,
    }

    impl FakeCollaborativeRebootInitiator {
        pub fn new()
        -> (futures::channel::oneshot::Receiver<CollaborativeRebootInitiatorRequestStream>, Self)
        {
            let (sender, receiver) = futures::channel::oneshot::channel();
            (receiver, Self { sender: Some(sender) })
        }
    }

    impl CollaborativeRebootProvider for FakeCollaborativeRebootInitiator {
        fn new_initiator_proxy(&mut self) -> Result<CollaborativeRebootInitiatorProxy, Error> {
            let (client, rs) =
                fidl::endpoints::create_request_stream::<CollaborativeRebootInitiatorMarker>();
            if let Some(sender) = self.sender.take() {
                sender.send(rs).map_err(|_rs| ()).expect("send request stream should succeed");
                Ok(client.into_proxy())
            } else {
                panic!("new_initiator_proxy called multiple times");
            }
        }
        fn new_scheduler_proxy(&mut self) -> Result<CollaborativeRebootSchedulerProxy, Error> {
            panic!("CollaborativeRebootScheduler connection was unexpectedly created")
        }
    }

    // A helper function to drive a Fake Collaborative Reboot server expecting
    // exactly once PerformPendingReboot request.
    pub async fn expect_perform_pending_reboot_once(
        receiver: futures::channel::oneshot::Receiver<CollaborativeRebootInitiatorRequestStream>,
    ) {
        let num_calls = receiver
            .await
            .expect("receive request stream should succeed.")
            .fold(0, |num_calls, request| {
                match request.expect("receive initiator request should succeed") {
                    CollaborativeRebootInitiatorRequest::PerformPendingReboot { responder } => {
                        responder
                            .send(&PerformPendingRebootResponse {
                                rebooting: Some(true),
                                ..Default::default()
                            })
                            .expect("responding to request should succeed.");
                        futures::future::ready(num_calls + 1)
                    }
                }
            })
            .await;
        assert_eq!(num_calls, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_set::{AppIdSource, AppMetadata};
    use assert_matches::assert_matches;
    use channel_config::ChannelConfig;
    use diagnostics_assertions::assert_data_tree;
    use fidl::endpoints::{create_endpoints, create_proxy_and_stream, create_request_stream};
    use fidl_fuchsia_update::{
        self as update, AttemptsMonitorRequest, ListenerMarker,
        ListenerNotifyOnFirstUpdateCheckRequest, ManagerMarker, MonitorRequest,
        MonitorRequestStream, NotifierMarker, NotifierRequest,
    };
    use fidl_fuchsia_update_channel::ProviderMarker;
    use fidl_fuchsia_update_channelcontrol::ChannelControlMarker;
    use fidl_fuchsia_update_verify::ComponentOtaHealthCheckMarker;
    use fuchsia_async::TestExecutor;
    use fuchsia_inspect::Inspector;
    use futures::{TryStreamExt, pin_mut};
    use omaha_client::common::App;
    use omaha_client::protocol::Cohort;
    use std::task::Poll;
    use test_case::test_case;

    fn spawn_fidl_server<M: fidl::endpoints::ProtocolMarker>(
        fidl: Rc<RefCell<stub::StubFidlServer>>,
        service: fn(M::RequestStream) -> IncomingServices,
    ) -> M::Proxy {
        let (proxy, stream) = create_proxy_and_stream::<M>();
        fasync::Task::local(async move {
            FidlServer::handle_client(fidl, service(stream), &mut NoCollaborativeReboot {})
                .await
                .unwrap_or_else(|e| panic!("{}", e))
        })
        .detach();
        proxy
    }

    async fn next_n_on_state_events(
        mut request_stream: MonitorRequestStream,
        n: usize,
    ) -> Vec<update::State> {
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            let MonitorRequest::OnState { state, responder } =
                request_stream.next().await.unwrap().unwrap();
            responder.send().unwrap();
            v.push(state);
        }
        v
    }

    #[fuchsia::test]
    async fn test_on_state_change() {
        let fidl = FidlServerBuilder::new().build().await;
        FidlServer::on_state_change(
            Rc::clone(&fidl),
            state_machine::State::CheckingForUpdates(InstallSource::OnDemand),
            &mut NoCollaborativeReboot {},
        )
        .await;
        assert_eq!(
            state_machine::State::CheckingForUpdates(InstallSource::OnDemand),
            fidl.borrow().state.manager_state
        );
    }

    #[fuchsia::test]
    async fn test_check_now() {
        let fidl = FidlServerBuilder::new().build().await;
        let proxy = spawn_fidl_server::<ManagerMarker>(fidl, IncomingServices::Manager);
        let options = update::CheckOptions {
            initiator: Some(Initiator::User),
            allow_attaching_to_existing_update_check: Some(false),
            ..Default::default()
        };
        let result = proxy.check_now(&options, None).await.unwrap();
        assert_matches!(result, Ok(()));
    }

    #[fuchsia::test]
    async fn test_attempts_monitor() {
        let fidl = FidlServerBuilder::new().build().await;
        let proxy = spawn_fidl_server::<ManagerMarker>(fidl, IncomingServices::Manager);
        let options = update::CheckOptions {
            initiator: Some(Initiator::User),
            allow_attaching_to_existing_update_check: Some(false),
            ..Default::default()
        };
        let (client_end, mut request_stream) = fidl::endpoints::create_request_stream();
        assert_matches!(proxy.monitor_all_update_checks(client_end), Ok(()));
        assert_matches!(proxy.check_now(&options, None).await.unwrap(), Ok(()));

        let AttemptsMonitorRequest::OnStart { options, monitor, responder } =
            request_stream.next().await.unwrap().unwrap();

        assert_matches!(responder.send(), Ok(()));
        assert_matches!(options.initiator, Some(fidl_fuchsia_update::Initiator::User));

        let events = next_n_on_state_events(monitor.into_stream(), 2).await;
        assert_eq!(
            events,
            [
                update::State::CheckingForUpdates(CheckingForUpdatesData::default()),
                update::State::ErrorCheckingForUpdate(ErrorCheckingForUpdateData::default()),
            ]
        );
    }

    #[fuchsia::test]
    async fn test_check_now_invalid_options() {
        let fidl = FidlServerBuilder::new().build().await;
        let proxy = spawn_fidl_server::<ManagerMarker>(fidl, IncomingServices::Manager);
        let (client_end, mut stream) = create_request_stream::<MonitorMarker>();
        let options = update::CheckOptions {
            initiator: None,
            allow_attaching_to_existing_update_check: None,
            ..Default::default()
        };
        let result = proxy.check_now(&options, Some(client_end)).await.unwrap();
        assert_matches!(result, Err(CheckNotStartedReason::InvalidOptions));
        assert_matches!(stream.next().await, None);
    }

    #[fuchsia::test]
    async fn test_check_now_already_in_progress() {
        let fidl = FidlServerBuilder::new()
            .state_machine_control(MockStateMachineController::new(Ok(
                StartUpdateCheckResponse::AlreadyRunning,
            )))
            .build()
            .await;
        let proxy = spawn_fidl_server::<ManagerMarker>(fidl, IncomingServices::Manager);
        let options = update::CheckOptions {
            initiator: Some(Initiator::User),
            allow_attaching_to_existing_update_check: None,
            ..Default::default()
        };
        let result = proxy.check_now(&options, None).await.unwrap();
        assert_matches!(result, Err(CheckNotStartedReason::AlreadyInProgress));
    }

    #[fuchsia::test]
    async fn test_check_now_throttled() {
        let fidl = FidlServerBuilder::new()
            .state_machine_control(MockStateMachineController::new(Ok(
                StartUpdateCheckResponse::Throttled,
            )))
            .build()
            .await;
        let proxy = spawn_fidl_server::<ManagerMarker>(fidl, IncomingServices::Manager);
        let options = update::CheckOptions {
            initiator: Some(Initiator::User),
            allow_attaching_to_existing_update_check: None,
            ..Default::default()
        };
        let result = proxy.check_now(&options, None).await.unwrap();
        assert_matches!(result, Err(CheckNotStartedReason::Throttled));
    }

    #[fuchsia::test]
    async fn test_check_now_with_monitor() {
        let fidl = FidlServerBuilder::new().build().await;
        let proxy = spawn_fidl_server::<ManagerMarker>(Rc::clone(&fidl), IncomingServices::Manager);
        let (client_end, mut stream) = create_request_stream::<MonitorMarker>();
        let options = update::CheckOptions {
            initiator: Some(Initiator::User),
            allow_attaching_to_existing_update_check: Some(true),
            ..Default::default()
        };
        let result = proxy.check_now(&options, Some(client_end)).await.unwrap();
        assert_matches!(result, Ok(()));
        let expected_states = [
            update::State::CheckingForUpdates(CheckingForUpdatesData::default()),
            update::State::ErrorCheckingForUpdate(ErrorCheckingForUpdateData::default()),
        ];
        let mut expected_states = expected_states.iter();
        while let Some(event) = stream.try_next().await.unwrap() {
            match event {
                MonitorRequest::OnState { state, responder } => {
                    assert_eq!(Some(&state), expected_states.next());
                    responder.send().unwrap();
                }
            }
        }
        assert_eq!(None, expected_states.next());
    }

    #[fuchsia::test]
    async fn test_attempts_monitor_two_updates() {
        let fidl = FidlServerBuilder::new().build().await;
        let proxy = spawn_fidl_server::<ManagerMarker>(Rc::clone(&fidl), IncomingServices::Manager);

        let check_options_1 = update::CheckOptions {
            initiator: Some(Initiator::User),
            allow_attaching_to_existing_update_check: Some(true),
            ..Default::default()
        };

        let (attempt_client_end, mut attempt_request_stream) =
            fidl::endpoints::create_request_stream();
        assert_matches!(proxy.monitor_all_update_checks(attempt_client_end), Ok(()));
        assert_matches!(proxy.check_now(&check_options_1, None).await.unwrap(), Ok(()));

        let AttemptsMonitorRequest::OnStart { options, monitor, responder } =
            attempt_request_stream.next().await.unwrap().unwrap();

        assert_matches!(responder.send(), Ok(()));
        assert_matches!(options.initiator, Some(fidl_fuchsia_update::Initiator::User));

        let events = next_n_on_state_events(monitor.into_stream(), 2).await;
        assert_eq!(
            events,
            [
                update::State::CheckingForUpdates(CheckingForUpdatesData::default()),
                update::State::ErrorCheckingForUpdate(ErrorCheckingForUpdateData::default()),
            ]
        );

        // Check for a second update and see the results on the same attempts_monitor.
        let check_options_2 = update::CheckOptions {
            initiator: Some(Initiator::Service),
            allow_attaching_to_existing_update_check: Some(true),
            ..Default::default()
        };
        assert_matches!(proxy.check_now(&check_options_2, None).await.unwrap(), Ok(()));
        let AttemptsMonitorRequest::OnStart { options, monitor, responder } =
            attempt_request_stream.next().await.unwrap().unwrap();

        assert_matches!(responder.send(), Ok(()));
        assert_matches!(options.initiator, Some(fidl_fuchsia_update::Initiator::Service));

        let events = next_n_on_state_events(monitor.into_stream(), 2).await;
        assert_eq!(
            events,
            [
                update::State::CheckingForUpdates(CheckingForUpdatesData::default()),
                update::State::ErrorCheckingForUpdate(ErrorCheckingForUpdateData::default()),
            ]
        );
    }

    #[fuchsia::test]
    async fn test_check_now_with_closed_monitor() {
        let fidl = FidlServerBuilder::new().build().await;
        let proxy = spawn_fidl_server::<ManagerMarker>(Rc::clone(&fidl), IncomingServices::Manager);
        let (client_end, stream) = create_request_stream::<MonitorMarker>();
        drop(stream);
        let options = update::CheckOptions {
            initiator: Some(Initiator::User),
            allow_attaching_to_existing_update_check: Some(true),
            ..Default::default()
        };
        let result = proxy.check_now(&options, Some(client_end)).await.unwrap();
        assert_matches!(result, Ok(()));
    }

    #[fuchsia::test]
    async fn test_monitor_progress() {
        let fidl = FidlServerBuilder::new()
            .state_machine_control(MockStateMachineController::new(Ok(
                StartUpdateCheckResponse::Started,
            )))
            .build()
            .await;
        let proxy = spawn_fidl_server::<ManagerMarker>(Rc::clone(&fidl), IncomingServices::Manager);
        let (client_end, mut stream) = create_request_stream::<MonitorMarker>();
        let options = update::CheckOptions {
            initiator: Some(Initiator::User),
            allow_attaching_to_existing_update_check: Some(true),
            ..Default::default()
        };
        let result = proxy.check_now(&options, Some(client_end)).await.unwrap();
        assert_matches!(result, Ok(()));
        FidlServer::on_state_change(
            Rc::clone(&fidl),
            state_machine::State::InstallingUpdate,
            &mut NoCollaborativeReboot {},
        )
        .await;
        // Ignore the first InstallingUpdate state with no progress.
        let MonitorRequest::OnState { state: _, responder } =
            stream.try_next().await.unwrap().unwrap();
        responder.send().unwrap();

        let progresses = vec![0.0, 0.3, 0.9, 1.0];
        for &progress in &progresses {
            FidlServer::on_progress_change(
                Rc::clone(&fidl),
                state_machine::InstallProgress { progress },
            )
            .await;
            let MonitorRequest::OnState { state, responder } =
                stream.try_next().await.unwrap().unwrap();
            match state {
                update::State::InstallingUpdate(InstallingData {
                    update: _,
                    installation_progress,
                    ..
                }) => {
                    assert_eq!(installation_progress.unwrap().fraction_completed.unwrap(), progress)
                }
                state => panic!("unexpected state: {state:?}"),
            }
            responder.send().unwrap();
        }
    }

    #[fuchsia::test]
    async fn test_get_health_check_status() {
        let fidl = FidlServerBuilder::new().with_service_url().build().await;

        let proxy =
            spawn_fidl_server::<ComponentOtaHealthCheckMarker>(fidl, IncomingServices::HealthCheck);

        assert_eq!(HealthStatus::Healthy, proxy.get_health_status().await.unwrap());
    }

    #[fuchsia::test]
    async fn test_get_health_check_status_bad_service_url() {
        let fidl = FidlServerBuilder::new().build().await;

        let proxy =
            spawn_fidl_server::<ComponentOtaHealthCheckMarker>(fidl, IncomingServices::HealthCheck);

        assert_eq!(HealthStatus::Unhealthy, proxy.get_health_status().await.unwrap());
    }

    #[fuchsia::test]
    async fn test_get_channel_from_app() {
        let app_set = FuchsiaAppSet::new(
            App::builder()
                .id("id")
                .version([1, 0])
                .cohort(Cohort { name: "current-channel".to_string().into(), ..Cohort::default() })
                .build(),
            AppMetadata { appid_source: AppIdSource::VbMetadata },
        );
        let fidl = FidlServerBuilder::new().with_app_set(app_set).build().await;

        let proxy =
            spawn_fidl_server::<ChannelControlMarker>(fidl, IncomingServices::ChannelControl);

        assert_eq!("current-channel", proxy.get_current().await.unwrap());
    }

    #[fuchsia::test]
    async fn test_get_current_channel_from_constructor() {
        let fidl = FidlServerBuilder::new()
            .with_current_channel("current-channel".to_string().into())
            .build()
            .await;

        let proxy = spawn_fidl_server::<ChannelControlMarker>(
            Rc::clone(&fidl),
            IncomingServices::ChannelControl,
        );
        assert_eq!("current-channel", proxy.get_current().await.unwrap());
    }

    #[fuchsia::test]
    async fn test_changing_target_doesnt_change_current_channel() {
        let fidl = FidlServerBuilder::new()
            .with_current_channel("current-channel".to_string().into())
            .build()
            .await;

        let proxy = spawn_fidl_server::<ChannelControlMarker>(
            Rc::clone(&fidl),
            IncomingServices::ChannelControl,
        );
        assert_eq!("current-channel", proxy.get_current().await.unwrap());

        let app_set = Rc::clone(&fidl.borrow().app_set);
        app_set.lock().await.set_system_target_channel(None, None);

        assert_eq!("current-channel", proxy.get_current().await.unwrap());
    }

    #[fuchsia::test]
    async fn test_provider_get_channel_from_constructor() {
        let fidl = FidlServerBuilder::new()
            .with_current_channel("current-channel".to_string().into())
            .build()
            .await;

        let proxy = spawn_fidl_server::<ProviderMarker>(fidl, IncomingServices::ChannelProvider);

        assert_eq!("current-channel", proxy.get_current().await.unwrap());
    }

    #[fuchsia::test]
    async fn test_provider_get_current_channel_from_app() {
        let app_set = FuchsiaAppSet::new(
            App::builder()
                .id("id")
                .version([1, 0])
                .cohort(Cohort { name: "current-channel".to_string().into(), ..Cohort::default() })
                .build(),
            AppMetadata { appid_source: AppIdSource::VbMetadata },
        );
        let fidl = FidlServerBuilder::new().with_app_set(app_set).build().await;

        let proxy = spawn_fidl_server::<ProviderMarker>(fidl, IncomingServices::ChannelProvider);

        assert_eq!("current-channel", proxy.get_current().await.unwrap());
    }

    #[fuchsia::test]
    async fn test_changing_target_doesnt_change_current_channel_provider() {
        let fidl = FidlServerBuilder::new()
            .with_current_channel("current-channel".to_string().into())
            .build()
            .await;

        let proxy = spawn_fidl_server::<ProviderMarker>(
            Rc::clone(&fidl),
            IncomingServices::ChannelProvider,
        );

        assert_eq!("current-channel", proxy.get_current().await.unwrap());

        let app_set = Rc::clone(&fidl.borrow().app_set);
        app_set.lock().await.set_system_target_channel(None, None);

        assert_eq!("current-channel", proxy.get_current().await.unwrap());
    }

    #[fuchsia::test]
    async fn test_get_target() {
        let app_metadata = AppMetadata { appid_source: AppIdSource::VbMetadata };
        let app_set = FuchsiaAppSet::new(
            App::builder()
                .id("id")
                .version([1, 0])
                .cohort(Cohort::from_hint("target-channel"))
                .build(),
            app_metadata,
        );
        let fidl = FidlServerBuilder::new().with_app_set(app_set).build().await;

        let proxy =
            spawn_fidl_server::<ChannelControlMarker>(fidl, IncomingServices::ChannelControl);
        assert_eq!("target-channel", proxy.get_target().await.unwrap());
    }

    #[fuchsia::test]
    async fn test_set_target() {
        let fidl = FidlServerBuilder::new()
            .with_channel_configs(ChannelConfigs {
                default_channel: None,
                known_channels: vec![
                    ChannelConfig::new_for_test("some-channel"),
                    ChannelConfig::with_appid_for_test("target-channel", "target-id"),
                ],
            })
            .build()
            .await;

        let proxy = spawn_fidl_server::<ChannelControlMarker>(
            Rc::clone(&fidl),
            IncomingServices::ChannelControl,
        );
        proxy.set_target("target-channel").await.unwrap();

        let app_set = Rc::clone(&fidl.borrow().app_set);
        let apps = app_set.lock().await.get_apps();
        assert_eq!("target-channel", apps[0].get_target_channel());
        assert_eq!("target-id", apps[0].id);
        let storage = Rc::clone(&fidl.borrow().storage_ref);
        let storage = storage.lock().await;
        storage.get_string(&apps[0].id).await.unwrap();
        assert!(storage.committed());
    }

    #[fuchsia::test]
    async fn test_set_target_empty() {
        let fidl = FidlServerBuilder::new()
            .with_channel_configs(ChannelConfigs {
                default_channel: "default-channel".to_string().into(),
                known_channels: vec![ChannelConfig::with_appid_for_test(
                    "default-channel",
                    "default-app",
                )],
            })
            .build()
            .await;

        let proxy = spawn_fidl_server::<ChannelControlMarker>(
            Rc::clone(&fidl),
            IncomingServices::ChannelControl,
        );
        proxy.set_target("").await.unwrap();

        let app_set = Rc::clone(&fidl.borrow().app_set);
        let apps = app_set.lock().await.get_apps();
        assert_eq!("default-channel", apps[0].get_target_channel());
        assert_eq!("default-app", apps[0].id);
        let storage = Rc::clone(&fidl.borrow().storage_ref);
        let storage = storage.lock().await;
        // Default channel should not be persisted to storage.
        assert_eq!(None, storage.get_string(&apps[0].id).await);
    }

    #[fuchsia::test]
    async fn test_set_target_no_op() {
        let app_metadata = AppMetadata { appid_source: AppIdSource::VbMetadata };
        let app_set = FuchsiaAppSet::new(
            App::builder()
                .id("id")
                .version([1, 0])
                .cohort(Cohort::from_hint("target-channel"))
                .build(),
            app_metadata,
        );
        let fidl = FidlServerBuilder::new().with_app_set(app_set).build().await;

        let proxy = spawn_fidl_server::<ChannelControlMarker>(
            Rc::clone(&fidl),
            IncomingServices::ChannelControl,
        );
        proxy.set_target("target-channel").await.unwrap();

        let app_set = Rc::clone(&fidl.borrow().app_set);
        let apps = app_set.lock().await.get_apps();
        assert_eq!("target-channel", apps[0].get_target_channel());
        let storage = Rc::clone(&fidl.borrow().storage_ref);
        let storage = storage.lock().await;
        // Verify that app is not persisted to storage.
        assert_eq!(storage.get_string(&apps[0].id).await, None);
    }

    #[fuchsia::test]
    async fn test_get_target_list() {
        let fidl = FidlServerBuilder::new()
            .with_channel_configs(ChannelConfigs {
                default_channel: None,
                known_channels: vec![
                    ChannelConfig::new_for_test("some-channel"),
                    ChannelConfig::new_for_test("some-other-channel"),
                ],
            })
            .build()
            .await;

        let proxy =
            spawn_fidl_server::<ChannelControlMarker>(fidl, IncomingServices::ChannelControl);
        let response = proxy.get_target_list().await.unwrap();

        assert_eq!(2, response.len());
        assert!(response.contains(&"some-channel".to_string()));
        assert!(response.contains(&"some-other-channel".to_string()));
    }

    #[fuchsia::test]
    async fn test_get_target_list_when_no_channels_configured() {
        let fidl = FidlServerBuilder::new().build().await;

        let proxy =
            spawn_fidl_server::<ChannelControlMarker>(fidl, IncomingServices::ChannelControl);
        let response = proxy.get_target_list().await.unwrap();

        assert!(response.is_empty());
    }

    #[fuchsia::test]
    async fn test_inspect_apps_on_state_change() {
        for &state in &[state_machine::State::Idle, state_machine::State::WaitingForReboot] {
            let inspector = Inspector::default();
            let apps_node = AppsNode::new(inspector.root().create_child("apps"));
            let fidl = FidlServerBuilder::new().with_apps_node(apps_node).build().await;

            if state == state_machine::State::WaitingForReboot {
                let (request_stream_receiver, mut cr_provider) =
                    FakeCollaborativeRebootScheduler::new();
                let ((), ()) = futures::join!(
                    FidlServer::on_state_change(Rc::clone(&fidl), state, &mut cr_provider),
                    expect_system_update_scheduled_once(request_stream_receiver),
                );
            } else {
                FidlServer::on_state_change(Rc::clone(&fidl), state, &mut NoCollaborativeReboot {})
                    .await;
            }

            let app_set = Rc::clone(&fidl.borrow().app_set);
            assert_data_tree!(
                inspector,
                root: {
                    apps: {
                        apps: format!("{:?}", app_set.lock().await.get_apps()),
                        apps_metadata: "[(\"id\", AppMetadata { appid_source: VbMetadata })]".to_string(),
                    }
                }
            );
        }
    }

    #[fuchsia::test]
    async fn test_inspect_apps_on_channel_change() {
        let inspector = Inspector::default();
        let apps_node = AppsNode::new(inspector.root().create_child("apps"));
        let fidl = FidlServerBuilder::new()
            .with_apps_node(apps_node)
            .with_channel_configs(ChannelConfigs {
                default_channel: None,
                known_channels: vec![ChannelConfig::new_for_test("target-channel")],
            })
            .build()
            .await;

        let proxy = spawn_fidl_server::<ChannelControlMarker>(
            Rc::clone(&fidl),
            IncomingServices::ChannelControl,
        );
        proxy.set_target("target-channel").await.unwrap();

        let app_set = Rc::clone(&fidl.borrow().app_set);
        assert_data_tree!(
            inspector,
            root: {
                apps: {
                    apps: format!("{:?}", app_set.lock().await.get_apps()),
                    apps_metadata: "[(\"id\", AppMetadata { appid_source: VbMetadata })]".to_string(),
                }
            }
        );
    }

    #[fuchsia::test]
    async fn test_inspect_state() {
        let inspector = Inspector::default();
        let state_node = StateNode::new(inspector.root().create_child("state"));
        let fidl = FidlServerBuilder::new().with_state_node(state_node).build().await;

        assert_data_tree!(
            inspector,
            root: {
                state: {
                    state: format!("{:?}", fidl.borrow().state),
                }
            }
        );

        StubFidlServer::on_state_change(
            Rc::clone(&fidl),
            state_machine::State::InstallingUpdate,
            &mut NoCollaborativeReboot {},
        )
        .await;

        assert_data_tree!(
            inspector,
            root: {
                state: {
                    state: format!("{:?}", fidl.borrow().state),
                }
            }
        );
    }

    // Test that calls to PerformPendingReboot are proxied to
    // collaborative reboots.
    #[fuchsia::test]
    async fn test_perform_pending_reboot() {
        let fidl = FidlServerBuilder::new().build().await;

        let (proxy, stream) = create_proxy_and_stream::<ManagerMarker>();

        let (request_stream_receiver, mut cr_provider) = FakeCollaborativeRebootInitiator::new();
        let pending_reboot_fut = futures::future::join(
            proxy.perform_pending_reboot(),
            expect_perform_pending_reboot_once(request_stream_receiver),
        )
        .fuse();
        futures::pin_mut!(pending_reboot_fut);
        futures::select! {
            (result, ()) = pending_reboot_fut => assert_matches!(result, Ok(true)),
            _result = FidlServer::handle_client(
                fidl, IncomingServices::Manager(stream), &mut cr_provider).fuse() => unreachable!(
                    "handle_client never exits"
                ),
        };
    }

    #[fuchsia::test]
    async fn test_set_urgent_update() {
        let fidl = FidlServerBuilder::new().build().await;
        fidl.borrow_mut().state = State {
            manager_state: state_machine::State::Idle,
            version_available: None,
            install_progress: None,
            urgent: None,
        };

        FidlServer::set_urgent_update(Rc::clone(&fidl), true);
        assert_eq!(fidl.borrow().state.urgent, Some(true));

        FidlServer::on_state_change(
            Rc::clone(&fidl),
            state_machine::State::Idle,
            &mut NoCollaborativeReboot {},
        )
        .await;
        assert_eq!(fidl.borrow().state.urgent, None);
    }

    enum TestExpect {
        /// Listener is waiting for the correct state.
        Wait,
        /// Listener has notified clients.
        Notify,
    }

    #[test_case(state_machine::State::Idle, TestExpect::Wait)]
    #[test_case(
        state_machine::State::CheckingForUpdates(InstallSource::default()),
        TestExpect::Wait
    )]
    #[test_case(state_machine::State::ErrorCheckingForUpdate, TestExpect::Notify)]
    #[test_case(state_machine::State::NoUpdateAvailable, TestExpect::Notify)]
    #[test_case(state_machine::State::InstallationDeferredByPolicy, TestExpect::Notify)]
    #[test_case(state_machine::State::InstallingUpdate, TestExpect::Wait)]
    #[test_case(state_machine::State::WaitingForReboot, TestExpect::Wait)]
    #[test_case(state_machine::State::InstallationError, TestExpect::Wait)]
    #[fuchsia::test(allow_stalls = false)]
    async fn test_listener_response(state: state_machine::State, expect: TestExpect) {
        let fidl = FidlServerBuilder::new().build().await;
        if state == state_machine::State::WaitingForReboot {
            let (request_stream_receiver, mut cr_provider) =
                FakeCollaborativeRebootScheduler::new();
            let ((), ()) = futures::join!(
                FidlServer::on_state_change(Rc::clone(&fidl), state, &mut cr_provider),
                expect_system_update_scheduled_once(request_stream_receiver),
            );
        } else {
            FidlServer::on_state_change(Rc::clone(&fidl), state, &mut NoCollaborativeReboot {})
                .await;
        }
        let proxy = spawn_fidl_server::<ListenerMarker>(fidl, IncomingServices::Listener);

        let (notifier_client, notifier_server) = create_endpoints::<NotifierMarker>();
        assert_matches!(
            proxy.notify_on_first_update_check(ListenerNotifyOnFirstUpdateCheckRequest {
                notifier: Some(notifier_client),
                ..Default::default()
            }),
            Ok(())
        );

        let mut notifier_stream = notifier_server.into_stream();
        let notify_fut = notifier_stream.try_next();
        pin_mut!(notify_fut);
        let notify_result = TestExecutor::poll_until_stalled(&mut notify_fut).await;

        match expect {
            TestExpect::Notify => {
                assert_matches!(
                    notify_result,
                    Poll::Ready(Ok(Some(NotifierRequest::Notify { .. }))),
                    "Notifier didn't receive request"
                );
            }
            TestExpect::Wait => {
                assert_matches!(
                    notify_result,
                    Poll::Pending,
                    "Notifier received unexpected request"
                );
            }
        }
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn correct_response_sequencing() {
        let fidl = FidlServerBuilder::new().build().await;
        let proxy1 =
            spawn_fidl_server::<ListenerMarker>(Rc::clone(&fidl), IncomingServices::Listener);
        let proxy2 =
            spawn_fidl_server::<ListenerMarker>(Rc::clone(&fidl), IncomingServices::Listener);

        let (notifier_client_1, notifier_server_1) = create_endpoints::<NotifierMarker>();
        assert_matches!(
            proxy1.notify_on_first_update_check(ListenerNotifyOnFirstUpdateCheckRequest {
                notifier: Some(notifier_client_1),
                ..Default::default()
            }),
            Ok(())
        );
        let mut notifier_stream_1 = notifier_server_1.into_stream();
        let notifier1 = notifier_stream_1.try_next();
        pin_mut!(notifier1);

        let (notifier_client_2, notifier_server_2) = create_endpoints::<NotifierMarker>();
        assert_matches!(
            proxy2.notify_on_first_update_check(ListenerNotifyOnFirstUpdateCheckRequest {
                notifier: Some(notifier_client_2),
                ..Default::default()
            }),
            Ok(())
        );
        let mut notifier_stream_2 = notifier_server_2.into_stream();
        let notifier2 = notifier_stream_2.try_next();
        pin_mut!(notifier2);

        FidlServer::on_state_change(
            Rc::clone(&fidl),
            state_machine::State::Idle,
            &mut NoCollaborativeReboot {},
        )
        .await;
        assert_matches!(TestExecutor::poll_until_stalled(&mut notifier1).await, Poll::Pending);
        assert_matches!(TestExecutor::poll_until_stalled(&mut notifier2).await, Poll::Pending);

        FidlServer::on_state_change(
            Rc::clone(&fidl),
            state_machine::State::NoUpdateAvailable,
            &mut NoCollaborativeReboot {},
        )
        .await;
        assert_matches!(
            TestExecutor::poll_until_stalled(&mut notifier1).await,
            Poll::Ready(Ok(Some(NotifierRequest::Notify { .. })))
        );
        assert_matches!(
            TestExecutor::poll_until_stalled(&mut notifier2).await,
            Poll::Ready(Ok(Some(NotifierRequest::Notify { .. })))
        );

        // Clients that start to wait after the happy state is received should return immediately.
        let proxy3 = spawn_fidl_server::<ListenerMarker>(fidl, IncomingServices::Listener);
        let (notifier_client_3, notifier_server_3) = create_endpoints::<NotifierMarker>();
        assert_matches!(
            proxy3.notify_on_first_update_check(ListenerNotifyOnFirstUpdateCheckRequest {
                notifier: Some(notifier_client_3),
                ..Default::default()
            }),
            Ok(())
        );
        let mut notifier_stream_3 = notifier_server_3.into_stream();
        assert_matches!(
            notifier_stream_3.try_next().await,
            Ok(Some(NotifierRequest::Notify { .. }))
        );
    }
}

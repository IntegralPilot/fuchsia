// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::builtin::runner::BuiltinRunnerFactory;
use crate::model::resolver::Resolver;
use ::namespace::Namespace;
use ::routing::policy::ScopedPolicyChecker;
use ::routing::resolving::{ComponentAddress, ResolvedComponent, ResolvedPackage, ResolverError};
use anyhow::format_err;
use async_trait::async_trait;
use cm_rust::{ComponentDecl, ConfigValuesData};
use directed_graph::DirectedGraph;
use fidl::endpoints::{Proxy, RequestStream, ServerEnd};
use fidl::epitaph::ChannelEpitaphExt;
use fidl_fuchsia_component_runner as fcrunner;
use fidl_fuchsia_component_runner::{
    ComponentDiagnostics, ComponentTasks, Task as DiagnosticsTask,
};
use fidl_fuchsia_io as fio;
use fidl_fuchsia_process as fprocess;
use flyweights::FlyStr;
use fuchsia_async as fasync;
use fuchsia_sync::Mutex as SyncMutex;
use futures::channel::oneshot;
use futures::future::{AbortHandle, Abortable};
use futures::lock::Mutex;
use futures::prelude::*;
use log::warn;
use std::collections::{HashMap, HashSet};
use std::mem;
use std::sync::Arc;
use vfs::directory::entry::OpenRequest;
use vfs::file::vmo::read_only;
use vfs::pseudo_directory;
use vfs::service::endpoint;
use zx::{AsHandleRef, Koid};

#[cfg(not(feature = "src_model_tests"))]
use {crate::model::program::Program, fidl::endpoints::create_endpoints};

#[derive(Debug)]
pub struct MockResolver {
    inner: SyncMutex<MockResolverInner>,
}

#[derive(Debug, Default)]
struct MockResolverInner {
    components: HashMap<String, ComponentDecl>,
    configs: HashMap<FlyStr, ConfigValuesData>,
    blockers: HashMap<String, Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
}

impl MockResolver {
    pub fn new() -> Self {
        MockResolver { inner: SyncMutex::new(Default::default()) }
    }

    pub(crate) async fn resolve_async(
        &self,
        component_url: String,
    ) -> Result<ResolvedComponent, ResolverError> {
        let (decl, config_values, maybe_blocker, client) = {
            const NAME_PREFIX: &str = "test:///";
            if !component_url.starts_with(NAME_PREFIX) {
                return Err(ResolverError::manifest_not_found(anyhow::format_err!(
                    "invalid component url: {}",
                    component_url
                )));
            }
            let (_, name) = component_url.split_at(NAME_PREFIX.len());
            let mut guard = self.inner.lock();
            let decl = guard.components.get(name).cloned().ok_or_else(|| {
                ResolverError::manifest_not_found(format_err!("not in the hashmap"))
            })?;
            let config_values = match &decl.config {
                None => None,
                Some(config_decl) => match &config_decl.value_source {
                    cm_rust::ConfigValueSource::Capabilities(_) => None,
                    cm_rust::ConfigValueSource::PackagePath(path) => Some(
                        guard
                            .configs
                            .get(&FlyStr::new(path))
                            .ok_or_else(|| {
                                ResolverError::manifest_invalid(format_err!(
                                    "config values not provided"
                                ))
                            })?
                            .clone(),
                    ),
                },
            };

            let sub_dir = pseudo_directory!(
                "fake_file" => read_only(b"content"),
            );
            let sub_dir_proxy = vfs::directory::serve(
                sub_dir,
                vfs::execution_scope::ExecutionScope::new(),
                fio::PERM_READABLE | fio::PERM_WRITABLE,
            );
            let client = sub_dir_proxy.into_client_end().unwrap();

            let maybe_blocker = { guard.blockers.get_mut(name).and_then(|b| b.take()) };
            (decl, config_values, maybe_blocker, client)
        };
        if let Some(blocker) = maybe_blocker {
            let (send, recv) = blocker;
            send.send(()).unwrap();
            let _ = recv.await.unwrap();
        }

        Ok(ResolvedComponent {
            context_to_resolve_children: None,
            decl: decl.clone(),
            package: Some(ResolvedPackage { url: "pkg".to_string(), directory: client }),
            config_values,
            abi_revision: Some(
                version_history_data::HISTORY
                    .get_example_supported_version_for_tests()
                    .abi_revision,
            ),
            dependencies: DirectedGraph::new(),
        })
    }

    pub fn add_component(&self, name: &str, component: ComponentDecl) {
        self.inner.lock().components.insert(name.to_string(), component);
    }

    pub fn add_config_values(&self, path: &str, values: ConfigValuesData) {
        self.inner.lock().configs.insert(path.into(), values);
    }

    pub async fn add_blocker(
        &self,
        path: &str,
        send: oneshot::Sender<()>,
        recv: oneshot::Receiver<()>,
    ) {
        self.inner.lock().blockers.insert(path.to_string(), Some((send, recv)));
    }

    #[cfg(not(feature = "src_model_tests"))]
    pub fn get_component_decl(&self, name: &str) -> Option<ComponentDecl> {
        self.inner.lock().components.get(name).map(Clone::clone)
    }
}

#[async_trait]
impl Resolver for MockResolver {
    async fn resolve(
        &self,
        component_address: &ComponentAddress,
    ) -> Result<ResolvedComponent, ResolverError> {
        self.resolve_async(component_address.url().to_string()).await
    }
}

pub type HostFn = Box<dyn Fn(ServerEnd<fio::DirectoryMarker>) + Send + Sync>;

pub type ControllerResponseFn = Box<dyn Fn() -> ControllerActionResponse + Send + Sync>;

pub type ManagedNamespace = Mutex<Namespace>;

struct MockRunnerInner {
    /// List of URLs started by this runner instance.
    urls_run: Vec<String>,

    /// Vector of waiters that wish to be notified when a new URL is run (used by `wait_for_urls`).
    url_waiters: Vec<futures::channel::oneshot::Sender<()>>,

    /// Namespace for each component, mapping resolved URL to the component's namespace.
    namespaces: HashMap<String, Arc<Mutex<Namespace>>>,

    /// Namespace for each component, mapping resolved URL to the component's numbered handles.
    numbered_handles: HashMap<String, Vec<fprocess::HandleInfo>>,

    /// Functions for serving the `outgoing` and `runtime` directories
    /// of a given component. When a component is started, these
    /// functions will be called with the server end of the directories.
    outgoing_host_fns: HashMap<String, Arc<HostFn>>,
    runtime_host_fns: HashMap<String, Arc<HostFn>>,

    /// Set of URLs that the MockRunner will fail the `start` call for.
    failing_urls: HashSet<String>,

    /// Functions for setting the controller's response to stop and kill
    /// requests. If not found, the controller will reply with success.
    controller_response_fns: HashMap<String, Arc<ControllerResponseFn>>,

    /// Map from the `Koid` of `Channel` owned by a `ComponentController` to
    /// the messages received by that controller.
    runner_requests: Arc<Mutex<HashMap<Koid, Vec<ControlMessage>>>>,

    controller_abort_handles: HashMap<Koid, AbortHandle>,

    controller_control_handles: HashMap<Koid, fcrunner::ComponentControllerControlHandle>,

    last_checker: Option<ScopedPolicyChecker>,
}

pub struct MockRunner {
    // The internal runner state.
    //
    // Inner state is guarded by a fuchsia_sync::Mutex to avoid helper
    // functions needing "async" (and propagating to callers).
    // fuchsia_sync::MutexGuard doesn't have the "Send" trait, so the
    // compiler will prevent us calling ".await" while holding the lock.
    inner: SyncMutex<MockRunnerInner>,
}

impl MockRunner {
    pub fn new() -> Self {
        MockRunner {
            inner: SyncMutex::new(MockRunnerInner {
                urls_run: vec![],
                url_waiters: vec![],
                namespaces: HashMap::new(),
                numbered_handles: HashMap::new(),
                outgoing_host_fns: HashMap::new(),
                runtime_host_fns: HashMap::new(),
                failing_urls: HashSet::new(),
                runner_requests: Arc::new(Mutex::new(HashMap::new())),
                controller_abort_handles: HashMap::new(),
                controller_control_handles: HashMap::new(),
                last_checker: None,
                controller_response_fns: HashMap::new(),
            }),
        }
    }

    /// Cause the URL `url` to return an error when started.
    #[cfg(not(feature = "src_model_tests"))]
    pub fn add_failing_url(&self, url: &str) {
        self.inner.lock().failing_urls.insert(url.to_string());
    }

    /// Cause the component `name` to return an error when started.
    #[cfg(not(feature = "src_model_tests"))]
    pub fn cause_failure(&self, name: &str) {
        self.add_failing_url(&format!("test:///{}", name))
    }

    /// Register `function` to serve the outgoing directory of component with `url`.
    pub fn add_host_fn(&self, url: &str, function: HostFn) {
        self.inner.lock().outgoing_host_fns.insert(url.to_string(), Arc::new(function));
    }

    /// Register `function` to override the controller response for the component with URL `url`.
    pub fn add_controller_response(&self, url: &str, function: ControllerResponseFn) {
        self.inner.lock().controller_response_fns.insert(url.to_string(), Arc::new(function));
    }

    /// Get the input namespace for component `name`.
    pub fn get_namespace(&self, name: &str) -> Option<Arc<ManagedNamespace>> {
        self.inner.lock().namespaces.get(name).map(Arc::clone)
    }

    /// Take the numbered handles for component `name` with `HandleType`.
    #[cfg(feature = "src_model_tests")]
    pub fn take_numbered_handle(
        &self,
        name: &str,
        ordinal: fuchsia_runtime::HandleType,
    ) -> Option<zx::NullableHandle> {
        let mut inner = self.inner.lock();
        let handles = inner.numbered_handles.get_mut(name)?;
        for i in 0..handles.len() {
            let info = fuchsia_runtime::HandleInfo::try_from(handles[i].id)
                .unwrap_or_else(|e| panic!("take_numbered_handle: {e}"));
            if info.handle_type() == ordinal {
                return Some(handles.remove(i).handle);
            }
        }
        None
    }

    #[cfg(not(feature = "src_model_tests"))]
    pub fn get_request_map(&self) -> Arc<Mutex<HashMap<Koid, Vec<ControlMessage>>>> {
        self.inner.lock().runner_requests.clone()
    }

    /// Returns a future that completes when `expected_url` is launched by the runner.
    pub async fn wait_for_url(&self, expected_url: &str) {
        self.wait_for_urls(&[expected_url]).await
    }

    /// Returns a future that completes when `expected_urls` were launched by the runner, in any
    /// order.
    pub async fn wait_for_urls(&self, expected_urls: &[&str]) {
        loop {
            let (sender, receiver) = oneshot::channel();
            {
                let mut inner = self.inner.lock();
                let expected_urls: HashSet<&str> = expected_urls.iter().map(|s| *s).collect();
                let urls_run: HashSet<&str> = inner.urls_run.iter().map(|s| s as &str).collect();
                if expected_urls.is_subset(&urls_run) {
                    return;
                } else {
                    inner.url_waiters.push(sender);
                }
            }
            receiver.await.expect("failed to receive url notice")
        }
    }

    /// If the runner has ran a component with this URL, forget this fact.
    /// This is useful when `wait_for_url` is to be used repeatedly to run a
    /// component with the same URL.
    pub fn reset_wait_for_url(&self, expected_url: &str) {
        let mut inner = self.inner.lock();
        inner.urls_run.retain(|url| url != expected_url);
    }

    pub fn abort_controller(&self, koid: &Koid) {
        let mut state = self.inner.lock();
        state.controller_control_handles.remove(koid).expect("koid was not available");
        let handle = state.controller_abort_handles.get(koid).expect("koid was not available");
        handle.abort();
    }

    /// Sends a `OnEscrow` event on the controller channel identified by `koid`.
    #[cfg(feature = "src_model_tests")]
    pub fn send_on_escrow(
        &self,
        koid: &Koid,
        request: fcrunner::ComponentControllerOnEscrowRequest,
    ) {
        let state = self.inner.lock();
        let handle = state.controller_control_handles.get(koid).expect("koid was not available");
        handle.send_on_escrow(request).unwrap();
    }

    /// Sends a `OnStopInfo` event on the controller channel identified by `koid`.
    #[cfg(feature = "src_model_tests")]
    pub fn send_on_stop_info(&self, koid: &Koid, info: fcrunner::ComponentStopInfo) {
        let state = self.inner.lock();
        let handle = state.controller_control_handles.get(koid).expect("koid was not available");
        handle.send_on_stop(info).unwrap();
    }

    fn start(
        &self,
        mut start_info: fcrunner::ComponentStartInfo,
        server_end: ServerEnd<fcrunner::ComponentControllerMarker>,
    ) {
        let outgoing_host_fn;
        let runtime_host_fn;
        let runner_requests;
        let resolved_url = start_info.resolved_url.unwrap();

        // The koid is the only unique piece of information we have about a
        // component start request. Two start requests for the same component
        // URL look identical to the Runner, the only difference being the
        // Channel passed to the Runner to use for the ComponentController
        // protocol.
        let channel_koid = server_end.as_handle_ref().basic_info().expect("basic info failed").koid;
        {
            let mut state = self.inner.lock();

            // Trigger a failure if previously requested.
            if state.failing_urls.contains(&resolved_url) {
                let status = zx::Status::UNAVAILABLE;
                server_end.into_channel().close_with_epitaph(status).unwrap();
                return;
            }

            // Fetch host functions, which will provide the outgoing and runtime directories
            // for the component.
            //
            // If functions were not provided, then start_info.outgoing_dir will be
            // automatically closed once it goes out of scope at the end of this
            // function.
            outgoing_host_fn = state.outgoing_host_fns.get(&resolved_url).map(Arc::clone);
            runtime_host_fn = state.runtime_host_fns.get(&resolved_url).map(Arc::clone);
            runner_requests = state.runner_requests.clone();

            // Create a namespace for the component.
            state.namespaces.insert(
                resolved_url.clone(),
                Arc::new(Mutex::new(start_info.ns.unwrap().try_into().unwrap())),
            );
            if let Some(numbered_handles) = start_info.numbered_handles.take() {
                state.numbered_handles.insert(resolved_url.clone(), numbered_handles);
            }

            let controller = match state.controller_response_fns.get(&resolved_url).map(|f| f()) {
                Some(response) => MockController::new_with_responses(
                    server_end,
                    runner_requests,
                    channel_koid,
                    response.clone(),
                    response,
                ),
                None => MockController::new(server_end, runner_requests, channel_koid),
            };
            let control_handle = controller.request_stream.control_handle();
            let abort_handle = controller.serve();
            state.controller_abort_handles.insert(channel_koid, abort_handle);
            state.controller_control_handles.insert(channel_koid, control_handle);

            // Start serving the outgoing/runtime directories.
            if let Some(outgoing_host_fn) = outgoing_host_fn {
                outgoing_host_fn(start_info.outgoing_dir.unwrap());
            }
            if let Some(runtime_host_fn) = runtime_host_fn {
                runtime_host_fn(start_info.runtime_dir.unwrap());
            }

            // Record that this URL has been started.
            state.urls_run.push(resolved_url.clone());
            let url_waiters = mem::replace(&mut state.url_waiters, vec![]);
            for waiter in url_waiters {
                waiter.send(()).expect("failed to send url notice");
            }
        }
    }

    pub async fn handle_stream(
        self: Arc<Self>,
        mut stream: fcrunner::ComponentRunnerRequestStream,
    ) {
        while let Ok(Some(request)) = stream.try_next().await {
            let fcrunner::ComponentRunnerRequest::Start { start_info, controller, .. } = request
            else {
                panic!("unknown runner request");
            };
            self.start(start_info, controller);
        }
    }
}

impl BuiltinRunnerFactory for MockRunner {
    fn get_scoped_runner(
        self: Arc<Self>,
        checker: ScopedPolicyChecker,
        open_request: OpenRequest<'_>,
    ) -> Result<(), zx::Status> {
        self.inner.lock().last_checker = Some(checker);
        open_request.open_service(endpoint(move |scope, server_end| {
            let stream = fcrunner::ComponentRunnerRequestStream::from_channel(server_end);
            scope.spawn(self.clone().handle_stream(stream));
        }))
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum ControlMessage {
    Stop,
    Kill,
}

#[derive(Clone)]
/// What the MockController should do when it receives a message.
pub struct ControllerActionResponse {
    pub close_channel: bool,
    pub delay: Option<zx::MonotonicDuration>,
    pub termination_status: Option<Result<(), zx::Status>>,
    pub exit_code: Option<i64>,
}

pub struct MockController {
    pub messages: Arc<Mutex<HashMap<Koid, Vec<ControlMessage>>>>,
    request_stream: fcrunner::ComponentControllerRequestStream,
    koid: Koid,
    stop_resp: ControllerActionResponse,
    kill_resp: ControllerActionResponse,
}

pub const MOCK_EXIT_CODE: i64 = 42;

impl MockController {
    /// Create a `MockController` that listens to the `server_end` and inserts
    /// `ControlMessage`'s into the Vec in the HashMap keyed under the provided
    /// `Koid`. When either a request to stop or kill a component is received
    /// the `MockController` will close the control channel immediately.
    pub fn new(
        server_end: ServerEnd<fcrunner::ComponentControllerMarker>,
        messages: Arc<Mutex<HashMap<Koid, Vec<ControlMessage>>>>,
        koid: Koid,
    ) -> MockController {
        Self::new_with_responses(
            server_end,
            messages,
            koid,
            ControllerActionResponse {
                close_channel: true,
                delay: None,
                termination_status: Some(Ok(())),
                exit_code: Some(MOCK_EXIT_CODE),
            },
            ControllerActionResponse {
                close_channel: true,
                delay: None,
                termination_status: Some(Ok(())),
                exit_code: Some(MOCK_EXIT_CODE),
            },
        )
    }

    /// Create a MockController that listens to the `server_end` and inserts
    /// `ControlMessage`'s into the Vec in the HashMap keyed under the provided
    /// `Koid`. The `stop_response` controls the delay used before taking any
    /// action on the control channel when a request to stop is received. The
    /// `kill_response` provides the same control when the a request to kill is
    /// received.
    pub fn new_with_responses(
        server_end: ServerEnd<fcrunner::ComponentControllerMarker>,
        messages: Arc<Mutex<HashMap<Koid, Vec<ControlMessage>>>>,
        koid: Koid,
        stop_response: ControllerActionResponse,
        kill_response: ControllerActionResponse,
    ) -> MockController {
        MockController {
            messages: messages,
            request_stream: server_end.into_stream(),
            koid: koid,
            stop_resp: stop_response,
            kill_resp: kill_response,
        }
    }

    fn on_stop_info_for_stop(&self) -> fcrunner::ComponentStopInfo {
        fcrunner::ComponentStopInfo {
            termination_status: self
                .stop_resp
                .termination_status
                .map(|s| zx::Status::result_into_raw(s)),
            exit_code: self.stop_resp.exit_code,
            ..Default::default()
        }
    }

    fn on_stop_info_for_kill(&self) -> fcrunner::ComponentStopInfo {
        fcrunner::ComponentStopInfo {
            termination_status: self
                .kill_resp
                .termination_status
                .map(|s| zx::Status::result_into_raw(s)),
            exit_code: self.kill_resp.exit_code,
            ..Default::default()
        }
    }

    /// Create a future which takes ownership of `server_end` and inserts
    /// `ControlMessage`s into `messages` based on events sent on the
    /// `ComponentController` channel.
    pub fn into_serve_future(mut self) -> impl Future<Output = ()> {
        let job_dup = fuchsia_runtime::job_default()
            .duplicate_handle(zx::Rights::SAME_RIGHTS)
            .expect("duplicate default job");
        let control = self.request_stream.control_handle();
        control
            .send_on_publish_diagnostics(ComponentDiagnostics {
                tasks: Some(ComponentTasks {
                    component_task: Some(DiagnosticsTask::Job(job_dup)),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .unwrap_or_else(|e| {
                warn!("sending diagnostics failed: {:?}", e);
            });
        async move {
            self.messages.lock().await.insert(self.koid, Vec::new());
            while let Ok(Some(request)) = self.request_stream.try_next().await {
                let control = control.clone();
                match request {
                    fcrunner::ComponentControllerRequest::Stop { control_handle: _ } => {
                        self.messages
                            .lock()
                            .await
                            .get_mut(&self.koid)
                            .expect("component channel koid key missing from mock runner map")
                            .push(ControlMessage::Stop);
                        let stop_info = self.on_stop_info_for_stop();
                        if let Some(delay) = self.stop_resp.delay {
                            let delay_copy = delay.clone();
                            let close_channel = self.stop_resp.close_channel;
                            let control = control.clone();
                            fasync::Task::spawn(async move {
                                fasync::Timer::new(fasync::MonotonicInstant::after(delay_copy))
                                    .await;
                                if close_channel {
                                    let _ = control.send_on_stop(stop_info);
                                }
                            })
                            .detach();
                        } else if self.stop_resp.close_channel {
                            let _ = control.send_on_stop(stop_info);
                            break;
                        }
                    }
                    fcrunner::ComponentControllerRequest::Kill { control_handle: _ } => {
                        self.messages
                            .lock()
                            .await
                            .get_mut(&self.koid)
                            .expect("component channel koid key missing from mock runner map")
                            .push(ControlMessage::Kill);
                        let stop_info = self.on_stop_info_for_kill();
                        if let Some(delay) = self.kill_resp.delay {
                            let delay_copy = delay.clone();
                            let close_channel = self.kill_resp.close_channel;
                            fasync::Task::spawn(async move {
                                fasync::Timer::new(fasync::MonotonicInstant::after(delay_copy))
                                    .await;
                                if close_channel {
                                    let _ = control.send_on_stop(stop_info);
                                }
                            })
                            .detach();
                            if self.kill_resp.close_channel {
                                break;
                            }
                        } else if self.kill_resp.close_channel {
                            let _ = control.send_on_stop(stop_info);
                            break;
                        }
                    }
                    fcrunner::ComponentControllerRequest::_UnknownMethod { .. } => {
                        panic!("unknown controller request");
                    }
                }
            }
        }
    }

    /// Spawn an async execution context which takes ownership of `server_end`
    /// and inserts `ControlMessage`s into `messages` based on events sent on
    /// the `ComponentController` channel. This simply spawns a future which
    /// awaits self.run().
    pub fn serve(self) -> AbortHandle {
        // Listen to the ComponentController server end and record the messages
        // that arrive. Exit after the first one, as this is the contract we
        // have implemented so far. Exiting will cause our handle to the
        // channel to drop and close the channel.

        let (handle, registration) = AbortHandle::new_pair();
        let fut = Abortable::new(self.into_serve_future(), registration);
        // Send default job for the sake of testing only.
        fasync::Task::spawn(async move {
            fut.await.unwrap_or(()); // Ignore cancellation.
        })
        .detach();
        handle
    }
}

/// Starts a program that does nothing but let us intercept requests to control its lifecycle.
#[cfg(not(feature = "src_model_tests"))]
pub fn mock_program() -> (Program, ServerEnd<fcrunner::ComponentControllerMarker>) {
    let (controller, server_end) = create_endpoints::<fcrunner::ComponentControllerMarker>();
    (Program::mock_from_controller(controller), server_end)
}

// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{power, startup};
use anyhow::{Context as _, Error, anyhow};
use fidl::endpoints::{ClientEnd, ServerEnd, create_proxy};
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_power_broker as fbroker;
use fidl_fuchsia_session as fsession;
use fidl_fuchsia_session_power as fpower;
use fuchsia_component::server::{ServiceFs, ServiceObjLocal};
use fuchsia_inspect_contrib::nodes::BoundedListNode;
use fuchsia_sync::Mutex;
use futures::{StreamExt, TryFutureExt, TryStreamExt};
use log::{error, warn};
use std::sync::Arc;

/// Maximum number of concurrent connections to the protocols served by `SessionManager`.
const MAX_CONCURRENT_CONNECTIONS: usize = 10_000;

/// The name for the inspect node that tracks session restart timestamps.
const DIAGNOSTICS_SESSION_STARTED_AT_NAME: &str = "session_started_at";

/// The max size for the session restart timestamps list.
const DIAGNOSTICS_SESSION_STARTED_AT_SIZE: usize = 100;

/// The name of the property for each entry in the `session_started_at` list for
/// the start timestamp.
const DIAGNOSTICS_TIME_PROPERTY_NAME: &str = "@time";

/// A request to connect to a protocol exposed by `SessionManager`.
pub enum IncomingRequest {
    Launcher(fsession::LauncherRequestStream),
    Restarter(fsession::RestarterRequestStream),
    Lifecycle(fsession::LifecycleRequestStream),
    Handoff(fpower::HandoffRequestStream),
}

struct Diagnostics {
    /// A list of session start/restart timestamps.
    session_started_at: BoundedListNode,
}

impl Diagnostics {
    pub fn record_session_start(&mut self) {
        self.session_started_at.add_entry(|node| {
            node.record_int(
                DIAGNOSTICS_TIME_PROPERTY_NAME,
                zx::MonotonicInstant::get().into_nanos(),
            );
        });
    }
}

/// State for a session that will be started in the future.
struct PendingSession {
    /// The server end on which the session's exposed directory will be served.
    ///
    /// This is the other end of `exposed_dir`.
    pub exposed_dir_server_end: ServerEnd<fio::DirectoryMarker>,
}

impl PendingSession {
    fn new() -> (fio::DirectoryProxy, Self) {
        let (exposed_dir, exposed_dir_server_end) = create_proxy::<fio::DirectoryMarker>();
        (exposed_dir, Self { exposed_dir_server_end })
    }
}

/// State of a started session.
///
/// The component has been created and started, but is not guaranteed to be running since it
/// may be stopped through external means.
struct StartedSession {
    /// The component URL of the session.
    url: String,
}

enum Session {
    Pending(PendingSession),
    Started(StartedSession),
}

impl Session {
    fn new_pending() -> (fio::DirectoryProxy, Self) {
        let (proxy, pending_session) = PendingSession::new();
        (proxy, Self::Pending(pending_session))
    }
}

struct PowerState {
    /// The power element corresponding to the session.
    ///
    /// The async mutex exists to serialize concurrent power lease operations, where
    /// we need to take a lock over async FIDL calls.
    power_element: futures::lock::Mutex<Option<power::PowerElement>>,

    /// Whether the system supports suspending.
    suspend_enabled: bool,
}

impl PowerState {
    pub fn new(suspend_enabled: bool) -> Self {
        Self { power_element: futures::lock::Mutex::default(), suspend_enabled }
    }

    /// Attempt to ensures that `session_manager` has a lease on the application activity element.
    ///
    /// This method is idempotent if it is a success.
    pub async fn ensure_power_lease(&self) {
        if !self.suspend_enabled {
            return;
        }
        let power_element = &mut *self.power_element.lock().await;
        if let Some(power_element) = power_element
            && power_element.has_lease()
        {
            return;
        }
        *power_element = match power::PowerElement::new().await {
            Ok(element) => Some(element),
            Err(err) => {
                warn!("Failed to create power element: {err}");
                None
            }
        };
    }

    pub async fn take_power_lease(
        &self,
    ) -> Result<ClientEnd<fbroker::LeaseControlMarker>, fpower::HandoffError> {
        if !self.suspend_enabled {
            log::warn!(
                "Session component wants to take our power lease, but the platform is \
                configured to not support suspend"
            );
            return Err(fpower::HandoffError::Unavailable);
        }
        log::info!("Session component is taking our power lease");
        let lease = match &mut *self.power_element.lock().await {
            Some(power_element) => power_element.take_lease(),
            None => return Err(fpower::HandoffError::Unavailable),
        }
        .ok_or(fpower::HandoffError::AlreadyTaken)?;
        Ok(lease)
    }
}

struct SessionManagerState {
    /// The component URL for the default session.
    default_session_url: Option<String>,

    /// State of the session.
    session: futures::lock::Mutex<Session>,

    /// The realm in which session components will be created.
    realm: fcomponent::RealmProxy,

    /// Power-related state.
    power: PowerState,

    // Debug-related state.
    debug: Arc<crate::debug::DebugManager>,

    /// Other mutable state.
    inner: Mutex<Inner>,
}

struct Inner {
    /// Collection of diagnostics nodes.
    diagnostics: Diagnostics,

    /// The current directory proxy we should use.  When pending, requests are queued.
    exposed_dir: fio::DirectoryProxy,
}

impl SessionManagerState {
    /// Start the session with the default session component URL, if any.
    ///
    /// # Errors
    ///
    /// Returns an error if the is no default session URL or the session could not be launched.
    async fn start_default(&self) -> Result<(), Error> {
        let session_url = self
            .default_session_url
            .as_ref()
            .ok_or_else(|| anyhow!("no default session URL configured"))?
            .clone();
        self.start(session_url, vec![]).await?;
        Ok(())
    }

    /// Start a session, replacing any already session.
    async fn start(
        &self,
        url: String,
        config_capabilities: Vec<fdecl::Configuration>,
    ) -> Result<(), startup::StartupError> {
        self.power.ensure_power_lease().await;
        self.start_impl(&mut *self.session.lock().await, config_capabilities, url).await
    }

    async fn start_impl(
        &self,
        session: &mut Session,
        config_capabilities: Vec<fdecl::Configuration>,
        url: String,
    ) -> Result<(), startup::StartupError> {
        let (proxy_on_failure, new_pending) = Session::new_pending();
        let pending_session = std::mem::replace(session, new_pending);
        let pending = match pending_session {
            Session::Pending(pending) => pending,
            Session::Started(_) => {
                let (proxy, pending) = PendingSession::new();
                self.inner.lock().exposed_dir = proxy;
                pending
            }
        };
        if let Err(e) = startup::launch_session(
            &url,
            config_capabilities,
            pending.exposed_dir_server_end,
            &self.realm,
        )
        .await
        {
            self.inner.lock().exposed_dir = proxy_on_failure;
            return Err(e);
        }
        *session = Session::Started(StartedSession { url });
        self.inner.lock().diagnostics.record_session_start();

        self.debug.clone().start_media_buttons_listener();

        Ok(())
    }

    /// Stops the session, if any.
    async fn stop(&self) -> Result<(), startup::StartupError> {
        self.power.ensure_power_lease().await;
        self.debug.stop();
        let mut session = self.session.lock().await;
        if let Session::Started(_) = &*session {
            let (proxy, new_pending) = Session::new_pending();
            *session = new_pending;
            self.inner.lock().exposed_dir = proxy;
            startup::stop_session(&self.realm).await?;
        }
        Ok(())
    }

    /// Restarts a session.
    async fn restart(&self) -> Result<(), startup::StartupError> {
        self.power.ensure_power_lease().await;
        let mut session = self.session.lock().await;
        let Session::Started(StartedSession { url }) = &mut *session else {
            return Err(startup::StartupError::NotRunning);
        };
        let url = url.clone();
        self.start_impl(&mut session, vec![], url).await?;
        Ok(())
    }

    async fn take_power_lease(
        &self,
    ) -> Result<ClientEnd<fbroker::LeaseControlMarker>, fpower::HandoffError> {
        let lease = self.power.take_power_lease().await?;
        Ok(lease)
    }
}

impl vfs::remote::GetRemoteDir for SessionManagerState {
    #[allow(clippy::unwrap_in_result)]
    fn get_remote_dir(&self) -> Result<fio::DirectoryProxy, zx::Status> {
        Ok(Clone::clone(&self.inner.lock().exposed_dir))
    }
}

/// Manages the session lifecycle and provides services to control the session.
#[derive(Clone)]
pub struct SessionManager {
    state: Arc<SessionManagerState>,
}

impl SessionManager {
    /// Constructs a new `SessionManager`.
    ///
    /// # Parameters
    /// - `realm`: The realm in which session components will be created.
    pub fn new(
        realm: fcomponent::RealmProxy,
        inspector: &fuchsia_inspect::Inspector,
        default_session_url: Option<String>,
        suspend_enabled: bool,
        enable_debug_shortcut: bool,
    ) -> Self {
        let session_started_at = BoundedListNode::new(
            inspector.root().create_child(DIAGNOSTICS_SESSION_STARTED_AT_NAME),
            DIAGNOSTICS_SESSION_STARTED_AT_SIZE,
        );
        let diagnostics = Diagnostics { session_started_at };
        let (proxy, new_pending) = Session::new_pending();
        let state = SessionManagerState {
            default_session_url,
            session: futures::lock::Mutex::new(new_pending),
            realm,
            power: PowerState::new(suspend_enabled),
            debug: Arc::new(crate::debug::DebugState::new(enable_debug_shortcut)),
            inner: Mutex::new(Inner { exposed_dir: proxy, diagnostics }),
        };
        SessionManager { state: Arc::new(state) }
    }

    #[cfg(test)]
    pub fn new_default(
        realm: fcomponent::RealmProxy,
        inspector: &fuchsia_inspect::Inspector,
    ) -> Self {
        Self::new(realm, inspector, None, false, false)
    }

    /// Starts the session with the default session component URL, if any.
    ///
    /// # Errors
    ///
    /// Returns an error if the is no default session URL or the session could not be launched.
    pub async fn start_default_session(&mut self) -> Result<(), Error> {
        self.state.start_default().await?;
        Ok(())
    }

    /// Starts serving [`IncomingRequest`] from `svc`.
    ///
    /// This will return once the [`ServiceFs`] stops serving requests.
    ///
    /// # Errors
    /// Returns an error if there is an issue serving the `svc` directory handle.
    pub async fn serve(
        &mut self,
        fs: &mut ServiceFs<ServiceObjLocal<'_, IncomingRequest>>,
    ) -> Result<(), Error> {
        fs.dir("svc")
            .add_fidl_service(IncomingRequest::Launcher)
            .add_fidl_service(IncomingRequest::Restarter)
            .add_fidl_service(IncomingRequest::Lifecycle)
            .add_fidl_service(IncomingRequest::Handoff);

        // Requests to /svc_from_session are forwarded to the session's exposed dir.
        fs.add_entry_at("svc_from_session", self.state.clone());

        fs.take_and_serve_directory_handle()?;

        fs.for_each_concurrent(MAX_CONCURRENT_CONNECTIONS, |request| {
            let mut session_manager = self.clone();
            async move {
                session_manager
                    .handle_incoming_request(request)
                    .unwrap_or_else(|err| error!("{err:?}"))
                    .await
            }
        })
        .await;

        Ok(())
    }

    /// Handles an [`IncomingRequest`].
    ///
    /// This will return once the protocol connection has been closed.
    ///
    /// # Errors
    /// Returns an error if there is an issue serving the request.
    async fn handle_incoming_request(&mut self, request: IncomingRequest) -> Result<(), Error> {
        match request {
            IncomingRequest::Launcher(request_stream) => {
                self.handle_launcher_request_stream(request_stream)
                    .await
                    .context("Session Launcher request stream got an error.")?;
            }
            IncomingRequest::Restarter(request_stream) => {
                self.handle_restarter_request_stream(request_stream)
                    .await
                    .context("Session Restarter request stream got an error.")?;
            }
            IncomingRequest::Lifecycle(request_stream) => {
                self.handle_lifecycle_request_stream(request_stream)
                    .await
                    .context("Session Lifecycle request stream got an error.")?;
            }
            IncomingRequest::Handoff(request_stream) => {
                self.handle_handoff_request_stream(request_stream)
                    .await
                    .context("Session Handoff request stream got an error.")?;
            }
        }

        Ok(())
    }

    /// Serves a specified [`LauncherRequestStream`].
    ///
    /// # Parameters
    /// - `request_stream`: the `LauncherRequestStream`.
    ///
    /// # Errors
    /// When an error is encountered reading from the request stream.
    pub async fn handle_launcher_request_stream(
        &mut self,
        mut request_stream: fsession::LauncherRequestStream,
    ) -> Result<(), Error> {
        while let Some(request) =
            request_stream.try_next().await.context("Error handling Launcher request stream")?
        {
            match request {
                fsession::LauncherRequest::Launch { configuration, responder } => {
                    let result = self.handle_launch_request(configuration).await;
                    let _ = responder.send(result);
                }
            }
        }
        Ok(())
    }

    /// Serves a specified [`RestarterRequestStream`].
    ///
    /// # Parameters
    /// - `request_stream`: the `RestarterRequestStream`.
    ///
    /// # Errors
    /// When an error is encountered reading from the request stream.
    pub async fn handle_restarter_request_stream(
        &mut self,
        mut request_stream: fsession::RestarterRequestStream,
    ) -> Result<(), Error> {
        while let Some(request) =
            request_stream.try_next().await.context("Error handling Restarter request stream")?
        {
            match request {
                fsession::RestarterRequest::Restart { responder } => {
                    let result = self.handle_restart_request().await;
                    let _ = responder.send(result);
                }
            }
        }
        Ok(())
    }

    /// Serves a specified [`LifecycleRequestStream`].
    ///
    /// # Parameters
    /// - `request_stream`: the `LifecycleRequestStream`.
    ///
    /// # Errors
    /// When an error is encountered reading from the request stream.
    pub async fn handle_lifecycle_request_stream(
        &mut self,
        mut request_stream: fsession::LifecycleRequestStream,
    ) -> Result<(), Error> {
        while let Some(request) =
            request_stream.try_next().await.context("Error handling Lifecycle request stream")?
        {
            match request {
                fsession::LifecycleRequest::Start { payload, responder } => {
                    let result = self.handle_lifecycle_start_request(payload.session_url).await;
                    let _ = responder.send(result);
                }
                fsession::LifecycleRequest::Stop { responder } => {
                    let result = self.handle_lifecycle_stop_request().await;
                    let _ = responder.send(result);
                }
                fsession::LifecycleRequest::Restart { responder } => {
                    let result = self.handle_lifecycle_restart_request().await;
                    let _ = responder.send(result);
                }
                fsession::LifecycleRequest::_UnknownMethod { ordinal, .. } => {
                    warn!(ordinal:%; "Lifecycle received an unknown method");
                }
            }
        }
        Ok(())
    }

    pub async fn handle_handoff_request_stream(
        &mut self,
        mut request_stream: fpower::HandoffRequestStream,
    ) -> Result<(), Error> {
        while let Some(request) =
            request_stream.try_next().await.context("Error handling Handoff request stream")?
        {
            match request {
                fpower::HandoffRequest::Take { responder } => {
                    let result = self.handle_handoff_take_request().await;
                    let _ = responder.send(result.map(|lease| lease.into_channel().into_handle()));
                }
                fpower::HandoffRequest::_UnknownMethod { ordinal, .. } => {
                    warn!(ordinal:%; "Lifecycle received an unknown method")
                }
            }
        }
        Ok(())
    }

    /// Handles calls to `Launcher.Launch()`.
    ///
    /// # Parameters
    /// - configuration: The launch configuration for the new session.
    async fn handle_launch_request(
        &mut self,
        configuration: fsession::LaunchConfiguration,
    ) -> Result<(), fsession::LaunchError> {
        let session_url = configuration.session_url.ok_or(fsession::LaunchError::InvalidArgs)?;
        let config_capabilities = configuration.config_capabilities.unwrap_or_default();
        self.state.start(session_url, config_capabilities).await.map_err(Into::into)
    }

    /// Handles a `Restarter.Restart()` request.
    async fn handle_restart_request(&mut self) -> Result<(), fsession::RestartError> {
        self.state.restart().await.map_err(Into::into)
    }

    /// Handles a `Lifecycle.Start()` request.
    ///
    /// # Parameters
    /// - `session_url`: The component URL for the session to start.
    async fn handle_lifecycle_start_request(
        &mut self,
        session_url: Option<String>,
    ) -> Result<(), fsession::LifecycleError> {
        let session_url = session_url
            .as_ref()
            .or(self.state.default_session_url.as_ref())
            .ok_or(fsession::LifecycleError::NotFound)?
            .to_owned();
        self.state.start(session_url, vec![]).await.map_err(Into::into)
    }

    /// Handles a `Lifecycle.Stop()` request.
    async fn handle_lifecycle_stop_request(&mut self) -> Result<(), fsession::LifecycleError> {
        self.state.stop().await.map_err(Into::into)
    }

    /// Handles a `Lifecycle.Restart()` request.
    async fn handle_lifecycle_restart_request(&mut self) -> Result<(), fsession::LifecycleError> {
        self.state.restart().await.map_err(Into::into)
    }

    /// Handles a `Handoff.Take()` request.
    async fn handle_handoff_take_request(
        &mut self,
    ) -> Result<ClientEnd<fbroker::LeaseControlMarker>, fpower::HandoffError> {
        self.state.take_power_lease().await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::SessionManager;
    use anyhow::{Error, anyhow};
    use diagnostics_assertions::{AnyProperty, assert_data_tree};
    use fidl::endpoints::{ServerEnd, create_proxy_and_stream};
    use fidl_fuchsia_component as fcomponent;
    use fidl_fuchsia_io as fio;
    use fidl_fuchsia_session as fsession;
    use fidl_test_util::spawn_stream_handler;
    use futures::channel::mpsc;
    use futures::prelude::*;
    use session_testing::{spawn_directory_server, spawn_noop_directory_server, spawn_server};
    use std::sync::LazyLock;
    use test_util::Counter;

    fn serve_launcher(session_manager: SessionManager) -> fsession::LauncherProxy {
        let (launcher_proxy, launcher_stream) =
            create_proxy_and_stream::<fsession::LauncherMarker>();
        {
            let mut session_manager_ = session_manager.clone();
            fuchsia_async::Task::spawn(async move {
                session_manager_
                    .handle_launcher_request_stream(launcher_stream)
                    .await
                    .expect("Session launcher request stream got an error.");
            })
            .detach();
        }
        launcher_proxy
    }

    fn serve_restarter(session_manager: SessionManager) -> fsession::RestarterProxy {
        let (restarter_proxy, restarter_stream) =
            create_proxy_and_stream::<fsession::RestarterMarker>();
        {
            let mut session_manager_ = session_manager.clone();
            fuchsia_async::Task::spawn(async move {
                session_manager_
                    .handle_restarter_request_stream(restarter_stream)
                    .await
                    .expect("Session restarter request stream got an error.");
            })
            .detach();
        }
        restarter_proxy
    }

    fn serve_lifecycle(session_manager: SessionManager) -> fsession::LifecycleProxy {
        let (lifecycle_proxy, lifecycle_stream) =
            create_proxy_and_stream::<fsession::LifecycleMarker>();
        {
            let mut session_manager_ = session_manager.clone();
            fuchsia_async::Task::spawn(async move {
                session_manager_
                    .handle_lifecycle_request_stream(lifecycle_stream)
                    .await
                    .expect("Session lifecycle request stream got an error.");
            })
            .detach();
        }
        lifecycle_proxy
    }

    fn spawn_noop_controller_server(server_end: ServerEnd<fcomponent::ControllerMarker>) {
        spawn_server(server_end, move |controller_request| match controller_request {
            fcomponent::ControllerRequest::Start { responder, .. } => {
                let _ = responder.send(Ok(()));
            }
            fcomponent::ControllerRequest::IsStarted { .. } => unimplemented!(),
            fcomponent::ControllerRequest::GetExposedDictionary { .. } => {
                unimplemented!()
            }
            fcomponent::ControllerRequest::GetOutputDictionary { .. } => {
                unimplemented!()
            }
            fcomponent::ControllerRequest::OpenExposedDir { .. } => {
                unimplemented!()
            }
            fcomponent::ControllerRequest::Destroy { .. } => {
                unimplemented!()
            }
            fcomponent::ControllerRequest::_UnknownMethod { .. } => {
                unimplemented!()
            }
        });
    }

    fn open_session_exposed_dir(
        session_manager: SessionManager,
        path: &str,
        server_end: ServerEnd<fio::DirectoryMarker>,
    ) {
        session_manager
            .state
            .inner
            .lock()
            .exposed_dir
            .open(path, fio::PERM_READABLE, &fio::Options::default(), server_end.into_channel())
            .unwrap();
    }

    /// Verifies that Launcher.Launch creates a new session.
    #[fuchsia::test]
    async fn test_launch() {
        let session_url = "session";

        let realm = spawn_stream_handler(move |realm_request| async move {
            match realm_request {
                fcomponent::RealmRequest::DestroyChild { child: _, responder } => {
                    let _ = responder.send(Ok(()));
                }
                fcomponent::RealmRequest::CreateChild { collection: _, decl, args, responder } => {
                    assert_eq!(decl.url.unwrap(), session_url);
                    spawn_noop_controller_server(args.controller.unwrap());
                    let _ = responder.send(Ok(()));
                }
                fcomponent::RealmRequest::OpenExposedDir { child: _, exposed_dir, responder } => {
                    spawn_noop_directory_server(exposed_dir);
                    let _ = responder.send(Ok(()));
                }
                _ => panic!("Realm handler received an unexpected request"),
            }
        });

        let inspector = fuchsia_inspect::Inspector::default();
        let session_manager = SessionManager::new_default(realm, &inspector);
        let launcher = serve_launcher(session_manager);

        assert!(
            launcher
                .launch(&fsession::LaunchConfiguration {
                    session_url: Some(session_url.to_string()),
                    ..Default::default()
                })
                .await
                .is_ok()
        );
        assert_data_tree!(inspector, root: {
            session_started_at: {
                "0": {
                    "@time": AnyProperty
                }
            }
        });
    }

    /// Verifies that Restarter.Restart restarts an existing session.
    #[fuchsia::test]
    async fn test_restarter_restart() {
        let session_url = "session";

        let realm = spawn_stream_handler(move |realm_request| async move {
            match realm_request {
                fcomponent::RealmRequest::DestroyChild { child: _, responder } => {
                    let _ = responder.send(Ok(()));
                }
                fcomponent::RealmRequest::CreateChild { collection: _, decl, args, responder } => {
                    assert_eq!(decl.url.unwrap(), session_url);
                    spawn_noop_controller_server(args.controller.unwrap());
                    let _ = responder.send(Ok(()));
                }
                fcomponent::RealmRequest::OpenExposedDir { child: _, exposed_dir, responder } => {
                    spawn_noop_directory_server(exposed_dir);
                    let _ = responder.send(Ok(()));
                }
                _ => panic!("Realm handler received an unexpected request"),
            }
        });

        let inspector = fuchsia_inspect::Inspector::default();
        let session_manager = SessionManager::new_default(realm, &inspector);
        let launcher = serve_launcher(session_manager.clone());
        let restarter = serve_restarter(session_manager);

        assert!(
            launcher
                .launch(&fsession::LaunchConfiguration {
                    session_url: Some(session_url.to_string()),
                    ..Default::default()
                })
                .await
                .expect("could not call Launch")
                .is_ok()
        );

        assert!(restarter.restart().await.expect("could not call Restart").is_ok());

        assert_data_tree!(inspector, root: {
            session_started_at: {
                "0": {
                    "@time": AnyProperty
                },
                "1": {
                    "@time": AnyProperty
                }
            }
        });
    }

    /// Verifies that Launcher.Restart return an error if there is no running existing session.
    #[fuchsia::test]
    async fn test_restarter_restart_error_not_running() {
        let realm = spawn_stream_handler(move |_realm_request| async move {
            panic!("Realm should not receive any requests as there is no session to launch")
        });

        let inspector = fuchsia_inspect::Inspector::default();
        let session_manager = SessionManager::new_default(realm, &inspector);
        let restarter = serve_restarter(session_manager);

        assert_eq!(
            Err(fsession::RestartError::NotRunning),
            restarter.restart().await.expect("could not call Restart")
        );

        assert_data_tree!(inspector, root: {
            session_started_at: {}
        });
    }

    /// Verifies that Lifecycle.Start creates a new session.
    #[fuchsia::test]
    async fn test_start() {
        let session_url = "session";

        let realm = spawn_stream_handler(move |realm_request| async move {
            match realm_request {
                fcomponent::RealmRequest::DestroyChild { child: _, responder } => {
                    let _ = responder.send(Ok(()));
                }
                fcomponent::RealmRequest::CreateChild { collection: _, decl, args, responder } => {
                    assert_eq!(decl.url.unwrap(), session_url);
                    spawn_noop_controller_server(args.controller.unwrap());
                    let _ = responder.send(Ok(()));
                }
                fcomponent::RealmRequest::OpenExposedDir { child: _, exposed_dir, responder } => {
                    spawn_noop_directory_server(exposed_dir);
                    let _ = responder.send(Ok(()));
                }
                _ => panic!("Realm handler received an unexpected request"),
            }
        });

        let inspector = fuchsia_inspect::Inspector::default();
        let session_manager = SessionManager::new_default(realm, &inspector);
        let lifecycle = serve_lifecycle(session_manager);

        assert!(
            lifecycle
                .start(&fsession::LifecycleStartRequest {
                    session_url: Some(session_url.to_string()),
                    ..Default::default()
                })
                .await
                .is_ok()
        );
        assert_data_tree!(inspector, root: {
            session_started_at: {
                "0": {
                    "@time": AnyProperty
                }
            }
        });
    }

    /// Verifies that Lifecycle.Start starts the default session if no URL is provided.
    #[fuchsia::test]
    async fn test_start_default() {
        let default_session_url = "session";

        let realm = spawn_stream_handler(move |realm_request| async move {
            match realm_request {
                fcomponent::RealmRequest::DestroyChild { child: _, responder } => {
                    let _ = responder.send(Ok(()));
                }
                fcomponent::RealmRequest::CreateChild { collection: _, decl, args, responder } => {
                    assert_eq!(decl.url.unwrap(), default_session_url);
                    spawn_noop_controller_server(args.controller.unwrap());
                    let _ = responder.send(Ok(()));
                }
                fcomponent::RealmRequest::OpenExposedDir { child: _, exposed_dir, responder } => {
                    spawn_noop_directory_server(exposed_dir);
                    let _ = responder.send(Ok(()));
                }
                _ => panic!("Realm handler received an unexpected request"),
            }
        });

        let inspector = fuchsia_inspect::Inspector::default();
        let session_manager = SessionManager::new(
            realm,
            &inspector,
            Some(default_session_url.to_owned()),
            false,
            false,
        );
        let lifecycle = serve_lifecycle(session_manager);

        assert!(
            lifecycle
                .start(&fsession::LifecycleStartRequest { session_url: None, ..Default::default() })
                .await
                .is_ok()
        );
        assert_data_tree!(inspector, root: {
            session_started_at: {
                "0": {
                    "@time": AnyProperty
                }
            }
        });
    }

    /// Verifies that Lifecycle.Stop stops an existing session by destroying its component.
    #[fuchsia::test]
    async fn test_stop_destroys_component() {
        static NUM_DESTROY_CHILD_CALLS: LazyLock<Counter> = LazyLock::new(|| Counter::new(0));

        let session_url = "session";

        let realm = spawn_stream_handler(move |realm_request| async move {
            match realm_request {
                fcomponent::RealmRequest::DestroyChild { child: _, responder } => {
                    NUM_DESTROY_CHILD_CALLS.inc();
                    let _ = responder.send(Ok(()));
                }
                fcomponent::RealmRequest::CreateChild { collection: _, decl, args, responder } => {
                    assert_eq!(decl.url.unwrap(), session_url);
                    spawn_noop_controller_server(args.controller.unwrap());
                    let _ = responder.send(Ok(()));
                }
                fcomponent::RealmRequest::OpenExposedDir { child: _, exposed_dir, responder } => {
                    spawn_noop_directory_server(exposed_dir);
                    let _ = responder.send(Ok(()));
                }
                _ => panic!("Realm handler received an unexpected request"),
            }
        });

        let inspector = fuchsia_inspect::Inspector::default();
        let session_manager = SessionManager::new_default(realm, &inspector);
        let lifecycle = serve_lifecycle(session_manager);

        assert!(
            lifecycle
                .start(&fsession::LifecycleStartRequest {
                    session_url: Some(session_url.to_string()),
                    ..Default::default()
                })
                .await
                .is_ok()
        );
        // Start attempts to destroy any existing session first.
        assert_eq!(NUM_DESTROY_CHILD_CALLS.get(), 1);
        assert_data_tree!(inspector, root: {
            session_started_at: {
                "0": {
                    "@time": AnyProperty
                }
            }
        });

        assert!(lifecycle.stop().await.is_ok());
        assert_eq!(NUM_DESTROY_CHILD_CALLS.get(), 2);
    }

    /// Verifies that Lifecycle.Restart restarts an existing session.
    #[fuchsia::test]
    async fn test_lifecycle_restart() {
        let session_url = "session";

        let realm = spawn_stream_handler(move |realm_request| async move {
            match realm_request {
                fcomponent::RealmRequest::DestroyChild { child: _, responder } => {
                    let _ = responder.send(Ok(()));
                }
                fcomponent::RealmRequest::CreateChild { collection: _, decl, args, responder } => {
                    assert_eq!(decl.url.unwrap(), session_url);
                    spawn_noop_controller_server(args.controller.unwrap());
                    let _ = responder.send(Ok(()));
                }
                fcomponent::RealmRequest::OpenExposedDir { child: _, exposed_dir, responder } => {
                    spawn_noop_directory_server(exposed_dir);
                    let _ = responder.send(Ok(()));
                }
                _ => panic!("Realm handler received an unexpected request"),
            }
        });

        let inspector = fuchsia_inspect::Inspector::default();
        let session_manager = SessionManager::new_default(realm, &inspector);
        let lifecycle = serve_lifecycle(session_manager.clone());

        assert!(
            lifecycle
                .start(&fsession::LifecycleStartRequest {
                    session_url: Some(session_url.to_string()),
                    ..Default::default()
                })
                .await
                .expect("could not call Launch")
                .is_ok()
        );

        assert!(lifecycle.restart().await.expect("could not call Restart").is_ok());

        assert_data_tree!(inspector, root: {
            session_started_at: {
                "0": {
                    "@time": AnyProperty
                },
                "1": {
                    "@time": AnyProperty
                }
            }
        });
    }

    /// Verifies that a node can be opened in the session's exposed dir before the session is
    /// started, and that it is connected once the session is started.
    #[fuchsia::test]
    async fn test_svc_from_session_before_start() -> Result<(), Error> {
        let session_url = "session";
        let svc_path = "foo";

        let (path_sender, mut path_receiver) = mpsc::channel(1);

        let session_exposed_dir_handler = move |directory_request| match directory_request {
            fio::DirectoryRequest::Open { path, .. } => {
                let mut path_sender: mpsc::Sender<String> = path_sender.clone();
                path_sender.try_send(path).unwrap();
            }
            _ => panic!("Directory handler received an unexpected request"),
        };

        let realm = spawn_stream_handler(move |realm_request| {
            let session_exposed_dir_handler = session_exposed_dir_handler.clone();
            async move {
                match realm_request {
                    fcomponent::RealmRequest::DestroyChild { responder, .. } => {
                        let _ = responder.send(Ok(()));
                    }
                    fcomponent::RealmRequest::CreateChild { args, responder, .. } => {
                        spawn_noop_controller_server(args.controller.unwrap());
                        let _ = responder.send(Ok(()));
                    }
                    fcomponent::RealmRequest::OpenExposedDir { exposed_dir, responder, .. } => {
                        spawn_directory_server(exposed_dir, session_exposed_dir_handler);
                        let _ = responder.send(Ok(()));
                    }
                    _ => panic!("Realm handler received an unexpected request"),
                }
            }
        });

        let inspector = fuchsia_inspect::Inspector::default();
        let session_manager = SessionManager::new_default(realm, &inspector);
        let lifecycle = serve_lifecycle(session_manager.clone());

        // Open an arbitrary node in the session's exposed dir.
        // The actual protocol does not matter because it's not being served.
        let (_client_end, server_end) = fidl::endpoints::create_proxy();

        open_session_exposed_dir(session_manager, svc_path, server_end);
        // Start the session.
        lifecycle
            .start(&fsession::LifecycleStartRequest {
                session_url: Some(session_url.to_string()),
                ..Default::default()
            })
            .await?
            .map_err(|err| anyhow!("failed to start: {err:?}"))?;

        // The exposed dir should have received the Open request.
        assert_eq!(path_receiver.next().await.unwrap(), svc_path);

        Ok(())
    }

    /// Verifies that a node in the session's exposed dir can be opened after the session has
    /// started.
    #[fuchsia::test]
    async fn test_svc_from_session_after_start() -> Result<(), Error> {
        let session_url = "session";
        let svc_path = "foo";

        let (path_sender, mut path_receiver) = mpsc::channel(1);

        let session_exposed_dir_handler = move |directory_request| match directory_request {
            fio::DirectoryRequest::Open { path, .. } => {
                let mut path_sender = path_sender.clone();
                path_sender.try_send(path).unwrap();
            }
            _ => panic!("Directory handler received an unexpected request"),
        };

        let realm = spawn_stream_handler(move |realm_request| {
            let session_exposed_dir_handler = session_exposed_dir_handler.clone();
            async move {
                match realm_request {
                    fcomponent::RealmRequest::DestroyChild { responder, .. } => {
                        let _ = responder.send(Ok(()));
                    }
                    fcomponent::RealmRequest::CreateChild { args, responder, .. } => {
                        spawn_noop_controller_server(args.controller.unwrap());
                        let _ = responder.send(Ok(()));
                    }
                    fcomponent::RealmRequest::OpenExposedDir { exposed_dir, responder, .. } => {
                        spawn_directory_server(exposed_dir, session_exposed_dir_handler);
                        let _ = responder.send(Ok(()));
                    }
                    _ => panic!("Realm handler received an unexpected request"),
                }
            }
        });

        let inspector = fuchsia_inspect::Inspector::default();
        let session_manager = SessionManager::new_default(realm, &inspector);
        let lifecycle = serve_lifecycle(session_manager.clone());

        lifecycle
            .start(&fsession::LifecycleStartRequest {
                session_url: Some(session_url.to_string()),
                ..Default::default()
            })
            .await?
            .map_err(|err| anyhow!("failed to start: {err:?}"))?;

        // Open an arbitrary node in the session's exposed dir.
        // The actual protocol does not matter because it's not being served.
        let (_client_end, server_end) = fidl::endpoints::create_proxy();

        open_session_exposed_dir(session_manager, svc_path, server_end);

        assert_eq!(path_receiver.next().await.unwrap(), svc_path);

        Ok(())
    }

    /// Verifies that `DebugState` is reset when a session is stopped.
    #[fuchsia::test]
    async fn test_stop_resets_debug_state() {
        let session_url = "session";

        let realm = spawn_stream_handler(move |realm_request| async move {
            match realm_request {
                fcomponent::RealmRequest::DestroyChild { child: _, responder } => {
                    let _ = responder.send(Ok(()));
                }
                fcomponent::RealmRequest::CreateChild { collection: _, decl, args, responder } => {
                    assert_eq!(decl.url.unwrap(), session_url);
                    spawn_noop_controller_server(args.controller.unwrap());
                    let _ = responder.send(Ok(()));
                }
                fcomponent::RealmRequest::OpenExposedDir { child: _, exposed_dir, responder } => {
                    spawn_noop_directory_server(exposed_dir);
                    let _ = responder.send(Ok(()));
                }
                _ => panic!("Realm handler received an unexpected request"),
            }
        });

        let inspector = fuchsia_inspect::Inspector::default();
        let session_manager = SessionManager::new(realm, &inspector, None, false, true);
        let lifecycle = serve_lifecycle(session_manager.clone());

        assert!(
            lifecycle
                .start(&fsession::LifecycleStartRequest {
                    session_url: Some(session_url.to_string()),
                    ..Default::default()
                })
                .await
                .is_ok()
        );

        session_manager.state.debug.set_button_press_state_count(3);
        assert_eq!(session_manager.state.debug.get_button_press_state_count(), 3);

        assert!(lifecycle.stop().await.is_ok());

        assert_eq!(session_manager.state.debug.get_button_press_state_count(), 0);
    }
}

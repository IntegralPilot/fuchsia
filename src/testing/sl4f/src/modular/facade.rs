// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::modular::types::{
    BasemgrResult, KillSessionResult, RestartSessionResult, StartBasemgrRequest,
};
use anyhow::{Error, format_err};
use fuchsia_component::client::connect_to_protocol;
use serde_json::{Value, from_value};
use {
    fidl_fuchsia_component_decl as fdecl, fidl_fuchsia_session as fsession,
    fidl_fuchsia_sys2 as fsys,
};

// The session runs as `./core/session-manager/session:session`. The parts are:
const SESSION_PARENT_MONIKER: &str = "./core/session-manager";
const SESSION_COLLECTION_NAME: &str = "session";
const SESSION_CHILD_NAME: &str = "session";
const SESSION_MONIKER: &str = "./core/session-manager/session:session";

/// Facade providing access to session testing interfaces.
#[derive(Debug)]
pub struct ModularFacade {
    session_launcher: fsession::LauncherProxy,
    session_restarter: fsession::RestarterProxy,
    lifecycle_controller: fsys::LifecycleControllerProxy,
    realm_query: fsys::RealmQueryProxy,
}

impl ModularFacade {
    pub fn new() -> ModularFacade {
        let session_launcher = connect_to_protocol::<fsession::LauncherMarker>()
            .expect("failed to connect to fuchsia.session.Launcher");
        let session_restarter = connect_to_protocol::<fsession::RestarterMarker>()
            .expect("failed to connect to fuchsia.session.Restarter");
        let lifecycle_controller = connect_to_protocol::<fsys::LifecycleControllerMarker>()
            .expect("failed to connect to fuchsia.sys2.LifecycleController");
        let realm_query =
            fuchsia_component::client::connect_to_protocol::<fsys::RealmQueryMarker>()
                .expect("failed to connect to fuchsia.sys2.RealmQuery");
        ModularFacade { session_launcher, session_restarter, lifecycle_controller, realm_query }
    }

    pub fn new_with_proxies(
        session_launcher: fsession::LauncherProxy,
        session_restarter: fsession::RestarterProxy,
        lifecycle_controller: fsys::LifecycleControllerProxy,
        realm_query: fsys::RealmQueryProxy,
    ) -> ModularFacade {
        ModularFacade { session_launcher, session_restarter, lifecycle_controller, realm_query }
    }

    /// Returns true if a session is currently running.
    pub async fn is_session_running(&self) -> Result<bool, Error> {
        Ok(self
            .realm_query
            .get_instance(SESSION_MONIKER)
            .await?
            .ok()
            .and_then(|instance| instance.resolved_info)
            .and_then(|info| info.execution_info)
            .is_some())
    }

    /// Restarts the currently running session.
    pub async fn restart_session(&self) -> Result<RestartSessionResult, Error> {
        if self.session_restarter.restart().await?.is_err() {
            return Ok(RestartSessionResult::Fail);
        }
        Ok(RestartSessionResult::Success)
    }

    /// Facade to stop the session.
    pub async fn stop_session(&self) -> Result<KillSessionResult, Error> {
        if !self.is_session_running().await? {
            return Ok(KillSessionResult::NoSessionRunning);
        }

        // Use a root LifecycleController to kill the session. It will send a shutdown signal to the
        // session so it can terminate gracefully.
        self.lifecycle_controller
            .destroy_instance(
                SESSION_PARENT_MONIKER,
                &fdecl::ChildRef {
                    name: SESSION_CHILD_NAME.to_string(),
                    collection: Some(SESSION_COLLECTION_NAME.to_string()),
                },
            )
            .await?
            .map_err(|err| format_err!("failed to destroy session: {:?}", err))?;

        Ok(KillSessionResult::Success)
    }

    /// Starts a session component.
    ///
    /// If a session is already running, it will be stopped first.
    ///
    /// `session_url` is required.
    ///
    /// # Arguments
    /// * `args`: A serde_json Value parsed into [`StartBasemgrRequest`]
    pub async fn start_session(&self, args: Value) -> Result<BasemgrResult, Error> {
        let req: StartBasemgrRequest = from_value(args)?;

        // If a session is running, stop it before starting a new one.
        if self.is_session_running().await? {
            self.stop_session().await?;
        }

        self.launch_session(&req.session_url).await?;

        Ok(BasemgrResult::Success)
    }

    /// Launches a session.
    ///
    /// # Arguments
    /// * `session_url`: Component URL for the session to launch.
    async fn launch_session(&self, session_url: &str) -> Result<(), Error> {
        let config = fsession::LaunchConfiguration {
            session_url: Some(session_url.to_string()),
            ..Default::default()
        };
        self.session_launcher
            .launch(&config)
            .await?
            .map_err(|err| format_err!("failed to launch session: {:?}", err))
    }
}

#[cfg(test)]
mod tests {
    use crate::modular::facade::{
        ModularFacade, SESSION_CHILD_NAME, SESSION_COLLECTION_NAME, SESSION_MONIKER,
        SESSION_PARENT_MONIKER,
    };
    use crate::modular::types::{BasemgrResult, KillSessionResult, RestartSessionResult};
    use anyhow::Error;
    use assert_matches::assert_matches;
    use fidl_test_util::spawn_stream_handler;
    use futures::Future;
    use serde_json::json;
    use std::sync::LazyLock;
    use test_util::Counter;
    use {fidl_fuchsia_session as fsession, fidl_fuchsia_sys2 as fsys};

    const TEST_SESSION_URL: &str = "fuchsia-pkg://fuchsia.com/test_session#meta/test_session.cm";

    // This function sets up a facade with launcher, restarter, and lifecycle_controller mocks and
    // provides counters to check how often each is called. To use it, pass in a function that
    // operates on a facade, executing the operations of the test, and pass in the expected number
    // of calls to launch, destroy, and restart.
    async fn test_facade<Fut>(
        run_facade_fns: impl Fn(ModularFacade) -> Fut,
        is_session_running: bool,
        expected_launch_count: usize,
        expected_destroy_count: usize,
        expected_restart_count: usize,
    ) -> Result<(), Error>
    where
        Fut: Future<Output = Result<(), Error>>,
    {
        static SESSION_LAUNCH_CALL_COUNT: LazyLock<Counter> = LazyLock::new(|| Counter::new(0));
        static DESTROY_CHILD_CALL_COUNT: LazyLock<Counter> = LazyLock::new(|| Counter::new(0));
        static SESSION_RESTART_CALL_COUNT: LazyLock<Counter> = LazyLock::new(|| Counter::new(0));

        let session_launcher = spawn_stream_handler(move |launcher_request| async move {
            match launcher_request {
                fsession::LauncherRequest::Launch { configuration, responder } => {
                    assert!(configuration.session_url.is_some());
                    let session_url = configuration.session_url.unwrap();
                    assert!(session_url == TEST_SESSION_URL.to_string());

                    SESSION_LAUNCH_CALL_COUNT.inc();
                    let _ = responder.send(Ok(()));
                }
            }
        });

        let session_restarter = spawn_stream_handler(move |restarter_request| async move {
            match restarter_request {
                fsession::RestarterRequest::Restart { responder } => {
                    SESSION_RESTART_CALL_COUNT.inc();
                    let _ = responder.send(Ok(()));
                }
            }
        });

        let lifecycle_controller = spawn_stream_handler(|lifecycle_controller_request| async {
            match lifecycle_controller_request {
                fsys::LifecycleControllerRequest::DestroyInstance {
                    parent_moniker,
                    child,
                    responder,
                } => {
                    assert_eq!(parent_moniker, SESSION_PARENT_MONIKER.to_string());
                    assert_eq!(child.name, SESSION_CHILD_NAME);
                    assert_eq!(child.collection.unwrap(), SESSION_COLLECTION_NAME);

                    DESTROY_CHILD_CALL_COUNT.inc();
                    let _ = responder.send(Ok(()));
                }
                r => {
                    panic!("didn't expect request: {:?}", r)
                }
            }
        });

        let realm_query = spawn_stream_handler(move |realm_query_request| async move {
            match realm_query_request {
                fsys::RealmQueryRequest::GetInstance { moniker, responder } => {
                    assert_eq!(moniker, SESSION_MONIKER.to_string());
                    let instance = if is_session_running {
                        fsys::Instance {
                            moniker: Some(SESSION_MONIKER.to_string()),
                            url: Some("fake".to_string()),
                            instance_id: None,
                            resolved_info: Some(fsys::ResolvedInfo {
                                resolved_url: Some("fake".to_string()),
                                execution_info: Some(fsys::ExecutionInfo {
                                    start_reason: Some("fake".to_string()),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            }),
                            ..Default::default()
                        }
                    } else {
                        fsys::Instance {
                            moniker: Some(SESSION_MONIKER.to_string()),
                            url: Some("fake".to_string()),
                            instance_id: None,
                            resolved_info: None,
                            ..Default::default()
                        }
                    };
                    let _ = responder.send(Ok(&instance));
                }
                r => {
                    panic!("didn't expect request: {:?}", r)
                }
            }
        });

        let facade = ModularFacade::new_with_proxies(
            session_launcher,
            session_restarter,
            lifecycle_controller,
            realm_query,
        );

        run_facade_fns(facade).await?;

        assert_eq!(
            SESSION_LAUNCH_CALL_COUNT.get(),
            expected_launch_count,
            "SESSION_LAUNCH_CALL_COUNT"
        );
        assert_eq!(
            DESTROY_CHILD_CALL_COUNT.get(),
            expected_destroy_count,
            "DESTROY_CHILD_CALL_COUNT"
        );
        assert_eq!(
            SESSION_RESTART_CALL_COUNT.get(),
            expected_restart_count,
            "SESSION_RESTART_CALL_COUNT"
        );

        Ok(())
    }

    #[fuchsia_async::run(2, test)]
    async fn test_stop_session() -> Result<(), Error> {
        async fn stop_session_steps(facade: ModularFacade) -> Result<(), Error> {
            assert_matches!(facade.is_session_running().await, Ok(true));
            assert_matches!(facade.stop_session().await, Ok(KillSessionResult::Success));
            Ok(())
        }

        test_facade(
            &stop_session_steps,
            true, // is_session_running
            0,    // SESSION_LAUNCH_CALL_COUNT
            1,    // DESTROY_CHILD_CALL_COUNT
            0,    // SESSION_RESTART_CALL_COUNT
        )
        .await
    }

    #[fuchsia_async::run(2, test)]
    async fn test_start_session_without_config() -> Result<(), Error> {
        async fn start_session_v2_steps(facade: ModularFacade) -> Result<(), Error> {
            let start_session_args = json!({
                "session_url": TEST_SESSION_URL,
            });
            assert_matches!(
                facade.start_session(start_session_args).await,
                Ok(BasemgrResult::Success)
            );
            Ok(())
        }

        test_facade(
            &start_session_v2_steps,
            false, // is_session_running
            1,     // SESSION_LAUNCH_CALL_COUNT
            0,     // DESTROY_CHILD_CALL_COUNT
            0,     // SESSION_RESTART_CALL_COUNT
        )
        .await
    }

    #[fuchsia_async::run(2, test)]
    async fn test_start_session_shutdown_existing() -> Result<(), Error> {
        async fn start_existing_steps(facade: ModularFacade) -> Result<(), Error> {
            let start_session_args = json!({
                "session_url": TEST_SESSION_URL,
            });

            assert_matches!(facade.is_session_running().await, Ok(true));
            // start_session() will notice that there's an existing session, destroy it, then launch
            // the new session.
            assert_matches!(
                facade.start_session(start_session_args).await,
                Ok(BasemgrResult::Success)
            );
            Ok(())
        }

        test_facade(
            &start_existing_steps,
            true, // is_session_running
            1,    // SESSION_LAUNCH_CALL_COUNT
            1,    // DESTROY_CHILD_CALL_COUNT
            0,    // SESSION_RESTART_CALL_COUNT
        )
        .await
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_is_session_running_not_running() -> Result<(), Error> {
        async fn not_running_steps(facade: ModularFacade) -> Result<(), Error> {
            assert_matches!(facade.is_session_running().await, Ok(false));

            Ok(())
        }

        test_facade(
            &not_running_steps,
            false, // is_session_running
            0,     // SESSION_LAUNCH_CALL_COUNT
            0,     // DESTROY_CHILD_CALL_COUNT
            0,     // SESSION_RESTART_CALL_COUNT
        )
        .await
    }

    #[fuchsia_async::run(2, test)]
    async fn test_is_session_running() -> Result<(), Error> {
        async fn is_running_steps(facade: ModularFacade) -> Result<(), Error> {
            assert_matches!(facade.is_session_running().await, Ok(true));

            Ok(())
        }

        test_facade(
            &is_running_steps,
            true, // is_session_running
            0,    // SESSION_LAUNCH_CALL_COUNT
            0,    // DESTROY_CHILD_CALL_COUNT
            0,    // SESSION_RESTART_CALL_COUNT
        )
        .await
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_restart_session() -> Result<(), Error> {
        async fn restart_steps(facade: ModularFacade) -> Result<(), Error> {
            assert_matches!(facade.restart_session().await, Ok(RestartSessionResult::Success));

            Ok(())
        }

        test_facade(
            &restart_steps,
            true, // is_session_running
            0,    // SESSION_LAUNCH_CALL_COUNT
            0,    // DESTROY_CHILD_CALL_COUNT
            1,    // SESSION_RESTART_CALL_COUNT
        )
        .await
    }

    #[fuchsia_async::run(2, test)]
    async fn test_restart_session_does_not_destroy() -> Result<(), Error> {
        async fn restart_steps(facade: ModularFacade) -> Result<(), Error> {
            assert_matches!(facade.is_session_running().await, Ok(true));

            assert_matches!(facade.restart_session().await, Ok(RestartSessionResult::Success));

            Ok(())
        }

        test_facade(
            &restart_steps,
            true, // is_session_running
            0,    // SESSION_LAUNCH_CALL_COUNT
            0,    // DESTROY_CHILD_CALL_COUNT
            1,    // SESSION_RESTART_CALL_COUNT
        )
        .await
    }
}

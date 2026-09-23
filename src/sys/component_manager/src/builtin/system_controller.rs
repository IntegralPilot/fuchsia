// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::model::actions::ShutdownType;
use crate::model::component::ComponentInstance;
use crate::model::component::manager::ComponentManagerInstance;
use anyhow::{Context as _, Error, format_err};
use fidl_fuchsia_sys2::*;
use fuchsia_async::{self as fasync};

use futures::prelude::*;
use futures::select;
use log::*;
use std::collections::VecDeque;
use std::pin::pin;
use std::sync::{Arc, Weak};
use std::time::Duration;

const SHUTDOWN_WATCHDOG_INTERVAL: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(15);

pub struct SystemController {
    top_instance: Weak<ComponentManagerInstance>,
    request_timeout: Duration,
}

impl SystemController {
    // TODO(jmatt) allow timeout to be supplied in the constructor
    pub fn new(top_instance: Weak<ComponentManagerInstance>, request_timeout: Duration) -> Self {
        Self { top_instance, request_timeout }
    }

    pub async fn serve(self, mut stream: SystemControllerRequestStream) -> Result<(), Error> {
        while let Some(request) = stream.try_next().await? {
            // TODO(jmatt) There is the potential for a race here. If
            // the thing that called SystemController.Shutdown is a
            // component that component_manager controls, it should
            // be gone by now. Sending a response doesn't make a lot
            // of sense in this case. However, the caller might live
            // outside component_manager, in which case a response
            // does make sense. Figure out if our behavior should be
            // different and/or whether we should drop the response
            // from this API.
            match request {
                // Shutting down the root component causes component_manager to
                // exit. main.rs waits on the model to observe the root disappear.
                SystemControllerRequest::Shutdown { responder } => {
                    let timeout = zx::MonotonicDuration::from(self.request_timeout);
                    info!("Component manager is shutting down the system");
                    let root = self
                        .top_instance
                        .upgrade()
                        .ok_or_else(|| format_err!("model is dropped"))?
                        .root()
                        .clone();

                    // Kick off a background task to log when shutdown is taking too long.
                    let _watchdog = fuchsia_async::Task::spawn(shutdown_watchdog(root.clone()));

                    let mut shutdown_fut = pin!(root.shutdown(ShutdownType::System).fuse());
                    let mut timer =
                        pin!(fasync::Timer::new(fasync::MonotonicInstant::after(timeout)).fuse());
                    select! {
                        res = shutdown_fut => {
                            res.context("got error waiting for shutdown action to complete")?;
                        }
                        () = timer => {
                            panic!("Component manager did not complete shutdown in allowed time.");
                        }
                    }
                    match responder.send() {
                        Ok(()) => {}
                        Err(e) => {
                            warn!(e:%; "Error sending response to shutdown requester. Shut down proceeding");
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

async fn shutdown_watchdog(root: Arc<ComponentInstance>) {
    let mut interval = fuchsia_async::Interval::new(SHUTDOWN_WATCHDOG_INTERVAL);
    while let Some(_) = interval.next().await {
        info!(
            "Shutdown not yet complete, pending components: {}.\n\
            To troubleshoot, try searching for component_manager log lines that begin with \
            `=` such as =RS, =PS, and =FS (actions/shutdown.rs for details)",
            num_still_running(&root).await
        );
    }
}

async fn num_still_running(root: &Arc<ComponentInstance>) -> usize {
    let mut ctr = 0;
    let mut queue = VecDeque::new();
    queue.push_back(root.clone());

    while let Some(next) = queue.pop_front() {
        let state = next.lock_state().await;
        if let Some(resolved_state) = state.get_resolved_state() {
            queue.extend(resolved_state.children().map(|(_, i)| i.clone()));
        }
        if state.is_started() {
            ctr += 1;
        }
    }

    ctr
}

#[cfg(all(test, not(feature = "src_model_tests")))]
mod tests {
    use super::*;
    use crate::model::testing::test_helpers::{
        ActionsTest, ComponentInfo, component_decl_with_test_runner,
    };
    use async_trait::async_trait;
    use cm_rust_testing::*;
    use errors::ModelError;
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_sys2 as fsys;
    use fuchsia_async as fasync;
    use hooks::{Event, EventType, Hook, HooksRegistration};
    use moniker::Moniker;

    /// Use SystemController to shut down a system whose root has the child `a`
    /// and `a` has descendents as shown in the diagram below.
    ///  a
    ///   \
    ///    b
    ///   / \
    ///  c   d
    #[fuchsia::test]
    async fn test_system_controller() {
        // Configure and start component
        let components = vec![
            ("root", ComponentDeclBuilder::new().child_default("a").build()),
            (
                "a",
                ComponentDeclBuilder::new()
                    .child(ChildBuilder::new().name("b").eager().build())
                    .build(),
            ),
            (
                "b",
                ComponentDeclBuilder::new()
                    .child(ChildBuilder::new().name("c").eager().build())
                    .child(ChildBuilder::new().name("d").eager().build())
                    .build(),
            ),
            ("c", component_decl_with_test_runner()),
            ("d", component_decl_with_test_runner()),
        ];
        let test = ActionsTest::new("root", components, None).await;

        // Start each component.
        test.start(Moniker::root()).await;
        let component_a = test.start(["a"].try_into().unwrap()).await;
        let component_b = test.start(["a", "b"].try_into().unwrap()).await;
        let component_c = test.start(["a", "b", "c"].try_into().unwrap()).await;
        let component_d = test.start(["a", "b", "d"].try_into().unwrap()).await;

        // Wire up connections to SystemController
        let sys_controller = SystemController::new(
            Arc::downgrade(test.model.top_instance()),
            // allow simulated shutdown to take up to 30 days
            Duration::from_secs(60 * 60 * 24 * 30),
        );
        let (controller_proxy, stream) = create_proxy_and_stream::<fsys::SystemControllerMarker>();
        let _task = fasync::Task::spawn(async move {
            sys_controller.serve(stream).await.expect("error serving system controller");
        });

        let root_component_info = ComponentInfo::new(test.model.root().clone()).await;
        let component_a_info = ComponentInfo::new(component_a.clone()).await;
        let component_b_info = ComponentInfo::new(component_b.clone()).await;
        let component_c_info = ComponentInfo::new(component_c.clone()).await;
        let component_d_info = ComponentInfo::new(component_d.clone()).await;

        // Check that the root component is still here
        root_component_info.check_not_shut_down(&test.runner).await;
        component_a_info.check_not_shut_down(&test.runner).await;
        component_b_info.check_not_shut_down(&test.runner).await;
        component_c_info.check_not_shut_down(&test.runner).await;
        component_d_info.check_not_shut_down(&test.runner).await;

        // Ask the SystemController to shut down the system and wait to be
        // notified that the root component stopped.
        let builtin_environment = test.builtin_environment.lock().await;
        let completion = builtin_environment.wait_for_root_stop();
        controller_proxy.shutdown().await.expect("shutdown request failed");
        completion.await;
        drop(builtin_environment);

        // Check state bits to confirm root component looks shut down
        root_component_info.check_is_shut_down(&test.runner).await;
        component_a_info.check_is_shut_down(&test.runner).await;
        component_b_info.check_is_shut_down(&test.runner).await;
        component_c_info.check_is_shut_down(&test.runner).await;
        component_d_info.check_is_shut_down(&test.runner).await;
    }

    #[fuchsia::test]
    #[should_panic(expected = "Component manager did not complete shutdown in allowed time.")]
    fn test_timeout() {
        const TIMEOUT_SECONDS: i64 = 6;
        const EVENT_PAUSE_SECONDS: i64 = TIMEOUT_SECONDS + 1;
        struct StopHook;
        #[async_trait]
        impl Hook for StopHook {
            async fn on(self: Arc<Self>, _event: &Event) -> Result<(), ModelError> {
                fasync::Timer::new(fasync::MonotonicInstant::after(
                    zx::MonotonicDuration::from_seconds(EVENT_PAUSE_SECONDS.into()),
                ))
                .await;
                Ok(())
            }
        }

        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let mut test_logic = Box::pin(async {
            // Configure and start component
            let components = vec![
                (
                    "root",
                    ComponentDeclBuilder::new()
                        .child(ChildBuilder::new().name("a").eager().build())
                        .build(),
                ),
                ("a", ComponentDeclBuilder::new().build()),
            ];

            let s = StopHook {};
            let s_hook: Arc<dyn Hook> = Arc::new(s);
            let hooks_reg = HooksRegistration::new(
                "stop hook",
                vec![EventType::Stopped],
                Arc::downgrade(&s_hook),
            );

            let test = ActionsTest::new_with_hooks("root", components, None, vec![hooks_reg]).await;

            // Start root and `a`.
            test.start(Moniker::root()).await;
            let component_a = test.start(["a"].try_into().unwrap()).await;

            // Wire up connections to SystemController
            let sys_controller = SystemController::new(
                Arc::downgrade(test.model.top_instance()),
                // require shutdown in a second
                Duration::from_secs(u64::try_from(TIMEOUT_SECONDS).unwrap()),
            );
            let (controller_proxy, stream) =
                create_proxy_and_stream::<fsys::SystemControllerMarker>();
            let _task = fasync::Task::spawn(async move {
                sys_controller.serve(stream).await.expect("error serving system controller");
            });

            let root_component_info = ComponentInfo::new(test.model.root().clone()).await;
            let component_a_info = ComponentInfo::new(component_a.clone()).await;

            // Check that the root component is still here
            root_component_info.check_not_shut_down(&test.runner).await;
            component_a_info.check_not_shut_down(&test.runner).await;

            // Ask the SystemController to shut down the system and wait to be
            // notified that the root component stopped.
            let builtin_environment = test.builtin_environment.lock().await;
            let _completion = builtin_environment.wait_for_root_stop();
            controller_proxy.shutdown().await.expect("shutdown request failed");
        });

        assert_eq!(std::task::Poll::Pending, exec.run_until_stalled(&mut test_logic));

        let new_time = fasync::MonotonicInstant::from_nanos(
            exec.now().into_nanos()
                + zx::MonotonicDuration::from_seconds(TIMEOUT_SECONDS).into_nanos(),
        );

        exec.set_fake_time(new_time);
        exec.wake_expired_timers();

        assert_eq!(std::task::Poll::Pending, exec.run_until_stalled(&mut test_logic));
    }

    #[fuchsia::test]
    fn test_no_panic_after_successful_shutdown_when_time_advances() {
        const TIMEOUT_SECONDS: i64 = 5;
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let mut test_logic = Box::pin(async {
            let components = vec![("root", ComponentDeclBuilder::new().build())];
            let test = ActionsTest::new("root", components, None).await;
            test.start(Moniker::root()).await;

            let sys_controller = SystemController::new(
                Arc::downgrade(test.model.top_instance()),
                Duration::from_secs(u64::try_from(TIMEOUT_SECONDS).unwrap()),
            );
            let (controller_proxy, stream) =
                create_proxy_and_stream::<fsys::SystemControllerMarker>();
            let _task = fasync::Task::spawn(async move {
                sys_controller.serve(stream).await.expect("error serving system controller");
            });

            let builtin_environment = test.builtin_environment.lock().await;
            let completion = builtin_environment.wait_for_root_stop();
            controller_proxy.shutdown().await.expect("shutdown request failed");
            completion.await;
        });

        // Run until shutdown finishes.
        assert_eq!(std::task::Poll::Ready(()), exec.run_until_stalled(&mut test_logic));

        // Advance fake time past the timeout. If the timeout timer leaked / was not cancelled,
        // it would panic here when expired timers are woken.
        let new_time = fasync::MonotonicInstant::from_nanos(
            exec.now().into_nanos()
                + zx::MonotonicDuration::from_seconds(TIMEOUT_SECONDS + 5).into_nanos(),
        );
        exec.set_fake_time(new_time);
        exec.wake_expired_timers();

        // The executor runs cleanly without panicking.
        let mut placeholder = Box::pin(async {});
        assert_eq!(std::task::Poll::Ready(()), exec.run_until_stalled(&mut placeholder));
    }
}

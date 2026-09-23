// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::model::escrow::EscrowedState;
use crate::model::token::InstanceToken;
use crate::runner::RemoteRunner;
use ::runner::component::StopInfo;
use errors::{StartError, StopError};
use fidl::endpoints;
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_component_runner as fcrunner;
use fidl_fuchsia_component_sandbox as fsandbox;
use fidl_fuchsia_data as fdata;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_mem as fmem;
use fidl_fuchsia_process as fprocess;
use futures::FutureExt;
use futures::channel::oneshot;
use futures::future::{BoxFuture, Either};
use moniker::Moniker;
use serve_processargs::NamespaceBuilder;

mod component_controller;
use component_controller::ComponentController;

/// A [Program] is a unit of execution.
///
/// After a program starts running, it may optionally publish diagnostics,
/// and may eventually terminate.
pub struct Program {
    controller: ComponentController,

    /// The directory that presents runtime information about the component. The
    /// runner must either serve the server endpoint, or drop it to avoid
    /// blocking any consumers indefinitely.
    runtime_dir: fio::DirectoryProxy,
}

/// Everything about a stopped execution.
#[derive(Debug)]
pub struct StopConclusion {
    /// How the execution was stopped.
    pub disposition: StopDisposition,

    /// Optional escrow request sent by the component.
    pub escrow_request: Option<EscrowRequest>,
}

/// [`StopDisposition`] describes how the execution was stopped, e.g. did it timeout.
#[derive(Debug, PartialEq, Clone)]
pub enum StopDisposition {
    /// The component did not stop in time, but was killed before the kill
    /// timeout. Additionally contains a status from the runner.
    Killed(StopInfo),

    /// The component did not stop in time and was killed after the kill
    /// timeout was reached.
    KilledAfterTimeout,

    /// The component had no Controller, no request was sent, and therefore no
    /// error occurred in the stop process.
    NoController,

    /// The component stopped within the timeout. Note that in this case a
    /// component may also have stopped on its own without the framework asking.
    /// Additionally contains a status from the runner.
    Stopped(StopInfo),
}

impl StopDisposition {
    pub fn stop_info(&self) -> StopInfo {
        match self {
            StopDisposition::Killed(stop_info) => stop_info.clone(),
            StopDisposition::KilledAfterTimeout => {
                StopInfo::from_status(zx::Status::TIMED_OUT, None)
            }
            StopDisposition::NoController => StopInfo::from_ok(None),
            StopDisposition::Stopped(stop_info) => stop_info.clone(),
        }
    }
}

impl Program {
    /// Starts running a program using the `runner`.
    ///
    /// After successfully starting the program, it will serve the namespace given to the
    /// program. If [Program] is dropped or [Program::kill] is called, the namespace will no
    /// longer be served.
    ///
    /// TODO(https://fxbug.dev/42073001): Change `start_info` to a `Dict` and `DeliveryMap` as runners
    /// migrate to use sandboxes.
    ///
    /// TODO(https://fxbug.dev/42073001): This API allows users to create orphaned programs that's not
    /// associated with anything else. Once we have a bedrock component concept, we might
    /// want to require a containing component to start a program.
    ///
    /// TODO(https://fxbug.dev/42073001): Since diagnostic information is only available once,
    /// the framework should be the one that get it. That's another reason to limit this API.
    pub fn start(
        runner: &RemoteRunner,
        start_info: StartInfo,
        escrowed_state: EscrowedState,
        diagnostics_sender: oneshot::Sender<fcrunner::ComponentDiagnostics>,
        moniker: Moniker,
        escrow_stats_node: fuchsia_inspect::Node,
    ) -> Result<Program, StartError> {
        let (controller, server_end) =
            endpoints::create_proxy::<fcrunner::ComponentControllerMarker>();
        let (runtime_dir, runtime_server) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();

        let start_info = start_info.into_fidl(escrowed_state, runtime_server)?;

        runner.start(start_info, server_end);
        let controller = ComponentController::new(
            controller,
            Some(diagnostics_sender),
            moniker,
            escrow_stats_node,
        );
        Ok(Program { controller, runtime_dir })
    }

    /// Gets the runtime directory of the program.
    pub fn runtime(&self) -> &fio::DirectoryProxy {
        &self.runtime_dir
    }

    /// Request to stop the program.
    pub fn stop(&self) -> Result<(), StopError> {
        self.controller.stop().map_err(StopError::Internal)?;
        Ok(())
    }

    /// Request to stop this program immediately.
    pub fn kill(&self) -> Result<(), StopError> {
        self.controller.kill().map_err(StopError::Internal)?;
        Ok(())
    }

    /// Wait for the program to terminate, with an epitaph specified in the
    /// `fuchsia.component.runner/ComponentController` FIDL protocol documentation.
    pub fn on_terminate(&self) -> BoxFuture<'static, StopInfo> {
        self.controller.wait_for_termination()
    }

    /// Stops or kills the program or returns early if either operation times out.
    ///
    /// If the program does not stop within the timeout, i.e. if `stop_timer` returns a value
    /// before the program stops, then the program is killed.
    ///
    /// Returns an error if the program could not be killed within the `kill_timer` timeout.
    ///
    /// Reaps any escrow request and stop information sent by the component.
    pub async fn stop_or_kill_with_timeout<'a, 'b>(
        self,
        stop_timer: BoxFuture<'a, ()>,
        kill_timer: BoxFuture<'b, ()>,
    ) -> Result<StopConclusion, StopError> {
        let disposition = match self.stop_with_timeout(stop_timer).await {
            Some(r) => r,
            None => {
                // We must have hit the stop timeout because calling stop didn't return
                // a result, move to killing the component.
                self.kill_with_timeout(kill_timer).await
            }
        }?;
        let FinalizedProgram { escrow_request } = self.finalize();
        Ok(StopConclusion { disposition, escrow_request })
    }

    /// Stops the program or returns early if the operation times out.
    ///
    /// Returns None on timeout, when `stop_timer` returns a value before the program terminates.
    async fn stop_with_timeout<'a>(
        &self,
        stop_timer: BoxFuture<'a, ()>,
    ) -> Option<Result<StopDisposition, StopError>> {
        // Ask the controller to stop the component
        if let Err(err) = self.stop() {
            return Some(Err(err));
        }

        // Wait for the controller to close the channel
        let channel_close = self.on_terminate().boxed();

        // Wait for either the timer to fire or the channel to close
        match futures::future::select(stop_timer, channel_close).await {
            Either::Left(((), _channel_close)) => None,
            Either::Right((_timer, _close_result)) => {
                Some(Ok(StopDisposition::Stopped(self.on_terminate().await)))
            }
        }
    }

    /// Kills the program or returns early if the operation times out.
    ///
    /// Returns None on timeout, when `kill_timer` returns a value before the program terminates.
    async fn kill_with_timeout<'a>(
        &self,
        kill_timer: BoxFuture<'a, ()>,
    ) -> Result<StopDisposition, StopError> {
        self.kill()?;

        // Wait for the controller to close the channel
        let channel_close = self.on_terminate().boxed();

        // If the control channel closes first, report the component to be
        // kill "normally", otherwise report it as killed after timeout.
        let disposition = match futures::future::select(kill_timer, channel_close).await {
            Either::Left(((), _channel_close)) => StopDisposition::KilledAfterTimeout,
            Either::Right((_timer, _close_result)) => {
                StopDisposition::Killed(self.on_terminate().await)
            }
        };
        Ok(disposition)
    }

    /// Drops the program and returns state that the program has escrowed, if any.
    fn finalize(self) -> FinalizedProgram {
        let Program { controller, .. } = self;
        let escrow_request = controller.finalize();
        FinalizedProgram { escrow_request }
    }

    /// Gets a [`Koid`] that will uniquely identify this program.
    #[cfg(test)]
    pub fn koid(&self) -> zx::Koid {
        self.controller.peer_koid()
    }

    /// Creates a program that does nothing but let us intercept requests to control its lifecycle.
    #[cfg(all(test, not(feature = "src_model_tests")))]
    pub fn mock_from_controller(
        controller: endpoints::ClientEnd<fcrunner::ComponentControllerMarker>,
    ) -> Program {
        let controller = ComponentController::new(
            controller.into_proxy(),
            None,
            Moniker::root(),
            fuchsia_inspect::Inspector::default().root().clone_weak(),
        );
        let (runtime_dir, _runtime_server) =
            fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        Program { controller, runtime_dir }
    }
}

/// Information and capabilities used to start a program.
pub struct StartInfo {
    /// The URL of the component.
    pub url: String,

    /// The component's program declaration.
    /// This information originates from `ComponentDecl.program`.
    pub program: fdata::Dictionary,

    /// The namespace to provide to the component instance.
    ///
    /// A namespace specifies the set of directories that a component instance
    /// receives at start-up. Through the namespace directories, a component
    /// may access capabilities available to it. The contents of the namespace
    /// are mainly determined by the component's `use` declarations but may
    /// also contain additional capabilities automatically provided by the
    /// framework.
    ///
    /// By convention, a component's namespace typically contains some or all
    /// of the following directories:
    ///
    /// - "/svc": A directory containing services that the component requested
    ///           to use via its "import" declarations.
    /// - "/pkg": A directory containing the component's package, including its
    ///           binaries, libraries, and other assets.
    ///
    /// The mount points specified in each entry must be unique and
    /// non-overlapping. For example, [{"/foo", ..}, {"/foo/bar", ..}] is
    /// invalid.
    ///
    /// TODO(b/298106231): eventually this should become a sandbox and delivery map.
    pub namespace: NamespaceBuilder,

    /// The numbered handles that were passed to the component.
    ///
    /// If the component does not support numbered handles, the runner is expected
    /// to close the handles.
    pub numbered_handles: Vec<fprocess::HandleInfo>,

    /// Binary representation of the component's configuration.
    ///
    /// # Layout
    ///
    /// The first 2 bytes of the data should be interpreted as an unsigned 16-bit
    /// little-endian integer which denotes the number of bytes following it that
    /// contain the configuration checksum. After the checksum, all the remaining
    /// bytes are a persistent FIDL message of a top-level struct. The struct's
    /// fields match the configuration fields of the component's compiled manifest
    /// in the same order.
    pub encoded_config: Option<fmem::Data>,

    /// An eventpair that debuggers can use to defer the launch of the component.
    ///
    /// For example, ELF runners hold off from creating processes in the component
    /// until ZX_EVENTPAIR_PEER_CLOSED is signaled on this eventpair. They also
    /// ensure that runtime_dir is served before waiting on this eventpair.
    /// ELF debuggers can query the runtime_dir to decide whether to attach before
    /// they drop the other side of the eventpair, which is sent in the payload of
    /// the DebugStarted event in fuchsia.component.events.
    pub break_on_start: Option<zx::EventPair>,

    /// An opaque token that represents the component instance.
    ///
    /// The `fuchsia.component/Introspector` protocol may be used to get the
    /// string moniker of the instance from this token.
    ///
    /// Runners may publish this token as part of diagnostics information, to
    /// identify the running component without knowing its moniker.
    ///
    /// The token is invalidated when the component instance is destroyed.
    pub component_instance: InstanceToken,
}

impl StartInfo {
    fn into_fidl(
        self,
        escrowed_state: EscrowedState,
        runtime_server_end: ServerEnd<fio::DirectoryMarker>,
    ) -> Result<fcrunner::ComponentStartInfo, StartError> {
        let EscrowedState {
            outgoing_dir,
            escrowed_dictionary,
            escrowed_dictionary_handle,
            _recoverable_bytes,
        } = escrowed_state;
        let ns = self.namespace.serve().map_err(StartError::ServeNamespace)?;
        Ok(fcrunner::ComponentStartInfo {
            resolved_url: Some(self.url),
            program: Some(self.program),
            ns: Some(ns.into()),
            outgoing_dir: Some(outgoing_dir),
            runtime_dir: Some(runtime_server_end),
            numbered_handles: Some(self.numbered_handles),
            encoded_config: self.encoded_config,
            break_on_start: self.break_on_start,
            component_instance: Some(self.component_instance.into()),
            escrowed_dictionary,
            escrowed_dictionary_handle,
            ..Default::default()
        })
    }
}

/// Information and capabilities that the framework holds on behalf of a component,
/// to be delivered on the next execution.
#[derive(Debug, Default)]
pub struct EscrowRequest {
    // Escrow the outgoing directory server endpoint. Whenever the
    // component is started, the framework will return this channel via
    // `ComponentStartInfo.outgoing_dir`.
    pub outgoing_dir: Option<ServerEnd<fio::DirectoryMarker>>,

    /// Escrow some user defined state. Whenever the component is started,
    /// the framework will return these handles via
    /// `ComponentStartInfo.escrowed_dictionary`.
    pub escrowed_dictionary: Option<fsandbox::DictionaryRef>,

    /// Escrow some user defined state. Whenever the component is started,
    /// the framework will return these handles via
    /// `ComponentStartInfo.escrowed_dictionary`.
    pub escrowed_dictionary_handle: Option<zx::EventPair>,

    /// The inspect property of the number of bytes of memory which are reclaimed when this
    /// component stops running.
    pub recoverable_bytes: Option<fuchsia_inspect::UintProperty>,
}

impl EscrowRequest {
    pub fn from_fidl(
        value: fcrunner::ComponentControllerOnEscrowRequest,
        moniker: &Moniker,
        escrow_stats_node: &fuchsia_inspect::Node,
    ) -> Self {
        log::warn!("escrowed {moniker}, saved {:?} bytes", value.recoverable_bytes);
        Self {
            outgoing_dir: value.outgoing_dir,
            escrowed_dictionary: value.escrowed_dictionary,
            escrowed_dictionary_handle: value.escrowed_dictionary_handle,
            recoverable_bytes: value
                .recoverable_bytes
                .map(|n| escrow_stats_node.create_uint(moniker.to_string(), n)),
        }
    }
}

#[derive(Debug, Default)]
struct FinalizedProgram {
    pub escrow_request: Option<EscrowRequest>,
}

#[cfg(all(test, not(feature = "src_model_tests")))]
pub mod tests {
    use super::*;
    use crate::model::testing::mocks::{
        self, ControlMessage, ControllerActionResponse, MOCK_EXIT_CODE, MockController,
    };
    use assert_matches::assert_matches;
    use fidl::endpoints::ControlHandle;
    use fuchsia_async as fasync;
    use futures::lock::Mutex;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::task::Poll;
    use zx::{AsHandleRef, Koid};

    #[fuchsia::test]
    /// Test scenario where we tell the controller to stop the component and
    /// the component stops immediately.
    async fn stop_component_well_behaved_component_stop() {
        // Create a mock program which simulates immediately shutting down
        // the component.
        let (program, server) = mocks::mock_program();
        let stop_timeout = zx::Duration::from_millis(500);
        let kill_timeout = zx::Duration::from_millis(500);

        // Create a request map which the MockController will fill with
        // requests it received related to mocked component.
        let requests: Arc<Mutex<HashMap<Koid, Vec<ControlMessage>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let controller = MockController::new(server, requests.clone(), program.koid());
        controller.serve();

        let stop_timer = Box::pin(async move {
            let timer = fasync::Timer::new(fasync::MonotonicInstant::after(stop_timeout));
            timer.await;
        });
        let kill_timer = Box::pin(async move {
            let timer = fasync::Timer::new(fasync::MonotonicInstant::after(kill_timeout));
            timer.await;
        });
        let program_koid = program.koid();
        match program.stop_or_kill_with_timeout(stop_timer, kill_timer).await {
            Ok(StopConclusion {
                disposition:
                    StopDisposition::Stopped(StopInfo {
                        termination_status: Ok(()),
                        exit_code: Some(MOCK_EXIT_CODE),
                    }),
                ..
            }) => {}
            Ok(result) => {
                panic!("unexpected successful stop result {:?}", result);
            }
            Err(e) => {
                panic!("unexpected error stopping component {:?}", e);
            }
        }

        let msg_map = requests.lock().await;
        let msg_list = msg_map.get(&program_koid).expect("No messages received on the channel");

        // The controller should have only seen a STOP message since it stops
        // the component immediately.
        assert_eq!(msg_list, &vec![ControlMessage::Stop]);
    }

    #[fuchsia::test]
    /// Test where the control channel is already closed when we try to stop
    /// the component.
    async fn stop_component_successful_component_already_gone() {
        let (program, server) = mocks::mock_program();
        let stop_timeout = zx::MonotonicDuration::from_millis(100);
        let kill_timeout = zx::MonotonicDuration::from_millis(1);

        let stop_timer = Box::pin(async move {
            let timer = fasync::Timer::new(fasync::MonotonicInstant::after(stop_timeout));
            timer.await;
        });
        let kill_timer = Box::pin(async move {
            let timer = fasync::Timer::new(fasync::MonotonicInstant::after(kill_timeout));
            timer.await;
        });

        // Drop the server end so it closes
        drop(server);
        match program.stop_or_kill_with_timeout(stop_timer, kill_timer).await {
            Ok(StopConclusion {
                disposition:
                    StopDisposition::Stopped(StopInfo {
                        termination_status: Err(zx::Status::PEER_CLOSED),
                        exit_code: None,
                    }),
                ..
            }) => {}
            Ok(result) => {
                panic!("unexpected successful stop result {:?}", result);
            }
            Err(e) => {
                panic!("unexpected error stopping component {:?}", e);
            }
        }
    }

    #[fuchsia::test]
    /// The scenario where the controller stops the component after a delay
    /// which is before the controller reaches its timeout.
    fn stop_component_successful_stop_with_delay() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();

        // Create a mock program which simulates shutting down the component
        // after a delay. The delay is much shorter than the period allotted
        // for the component to stop.
        let (program, server) = mocks::mock_program();
        let stop_timeout = zx::MonotonicDuration::from_seconds(5);
        let kill_timeout = zx::MonotonicDuration::from_millis(1);

        // Create a request map which the MockController will fill with
        // requests it received related to mocked component.
        let requests: Arc<Mutex<HashMap<Koid, Vec<ControlMessage>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let component_stop_delay =
            zx::MonotonicDuration::from_millis(stop_timeout.into_millis() / 1_000);
        let controller = MockController::new_with_responses(
            server,
            requests.clone(),
            program.koid(),
            // stop the component after 60ms
            ControllerActionResponse {
                close_channel: true,
                delay: Some(component_stop_delay),
                termination_status: Some(Ok(())),
                exit_code: Some(MOCK_EXIT_CODE),
            },
            ControllerActionResponse {
                close_channel: true,
                delay: Some(component_stop_delay),
                termination_status: Some(Ok(())),
                exit_code: Some(MOCK_EXIT_CODE),
            },
        );
        controller.serve();

        // Create the stop call that we expect to stop the component.
        let stop_timer = Box::pin(async move {
            let timer = fasync::Timer::new(fasync::MonotonicInstant::after(stop_timeout));
            timer.await;
        });
        let kill_timer = Box::pin(async move {
            let timer = fasync::Timer::new(fasync::MonotonicInstant::after(kill_timeout));
            timer.await;
        });
        let program_koid = program.koid();
        let mut stop_future = Box::pin(program.stop_or_kill_with_timeout(stop_timer, kill_timer));

        // Poll the stop component future to where it has asked the controller
        // to stop the component. This should also cause the controller to
        // spawn the future with a delay to close the control channel.
        assert!(exec.run_until_stalled(&mut stop_future).is_pending());

        // Advance the clock beyond where the future to close the channel
        // should fire.
        let new_time = fasync::MonotonicInstant::from_nanos(
            exec.now().into_nanos() + component_stop_delay.into_nanos(),
        );
        exec.set_fake_time(new_time);
        exec.wake_expired_timers();

        // The controller channel should be closed so we can drive the stop
        // future to completion.
        match exec.run_until_stalled(&mut stop_future) {
            Poll::Ready(Ok(StopConclusion {
                disposition:
                    StopDisposition::Stopped(StopInfo {
                        termination_status: Ok(()),
                        exit_code: Some(MOCK_EXIT_CODE),
                    }),
                ..
            })) => {}
            Poll::Ready(Ok(result)) => {
                panic!("unexpected successful stop result {:?}", result);
            }
            Poll::Ready(Err(e)) => {
                panic!("unexpected error stopping component {:?}", e);
            }
            Poll::Pending => {
                panic!("future should have completed!");
            }
        }

        // Check that what we expect to be in the message map is there.
        let mut test_fut = Box::pin(async {
            let msg_map = requests.lock().await;
            let msg_list = msg_map.get(&program_koid).expect("No messages received on the channel");

            // The controller should have only seen a STOP message since it stops
            // the component before the timeout is hit.
            assert_eq!(msg_list, &vec![ControlMessage::Stop]);
        });
        assert!(exec.run_until_stalled(&mut test_fut).is_ready());
    }

    #[fuchsia::test]
    /// Test scenario where the controller does not stop the component within
    /// the allowed period and the component stop state machine has to send
    /// the `kill` message to the controller. The runner then does not kill the
    /// component within the kill time out period.
    fn stop_component_successful_with_kill_timeout_result() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();

        // Create a controller which takes far longer than allowed to stop the
        // component.
        let (program, server) = mocks::mock_program();
        let stop_timeout = zx::MonotonicDuration::from_seconds(5);
        let kill_timeout = zx::MonotonicDuration::from_millis(200);

        // Create a request map which the MockController will fill with
        // requests it received related to mocked component.
        let requests: Arc<Mutex<HashMap<Koid, Vec<ControlMessage>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let stop_resp_delay = zx::MonotonicDuration::from_millis(stop_timeout.into_millis() / 10);
        // since we want the mock controller to close the controller channel
        // before the kill timeout, set the response delay to less than the timeout
        let kill_resp_delay = zx::MonotonicDuration::from_millis(kill_timeout.into_millis() * 2);
        let controller = MockController::new_with_responses(
            server,
            requests.clone(),
            program.koid(),
            // Process the stop message, but fail to close the channel. Channel
            // closure is the indication that a component stopped.
            ControllerActionResponse {
                close_channel: false,
                delay: Some(stop_resp_delay),
                termination_status: Some(Ok(())),
                exit_code: Some(MOCK_EXIT_CODE),
            },
            ControllerActionResponse {
                close_channel: true,
                delay: Some(kill_resp_delay),
                termination_status: Some(Ok(())),
                exit_code: Some(MOCK_EXIT_CODE),
            },
        );
        let mut mock_controller_future = Box::pin(controller.into_serve_future());

        let stop_timer = Box::pin(async move {
            let timer = fasync::Timer::new(fasync::MonotonicInstant::after(stop_timeout));
            timer.await;
        });
        let kill_timer = Box::pin(async move {
            let timer = fasync::Timer::new(fasync::MonotonicInstant::after(kill_timeout));
            timer.await;
        });
        let program_koid = program.koid();
        let mut stop_fut = Box::pin(program.stop_or_kill_with_timeout(stop_timer, kill_timer));

        // The stop fn has sent the stop message and is now waiting for a response
        assert!(exec.run_until_stalled(&mut stop_fut).is_pending());
        assert!(exec.run_until_stalled(&mut mock_controller_future).is_pending());

        let mut check_msgs = Box::pin(async {
            // Check if the mock controller got all the messages we expected it to get.
            let msg_map = requests.lock().await;
            let msg_list = msg_map.get(&program_koid).expect("No messages received on the channel");
            assert_eq!(msg_list, &vec![ControlMessage::Stop]);
        });
        assert!(exec.run_until_stalled(&mut check_msgs).is_ready());
        drop(check_msgs);

        // Roll time passed the stop timeout.
        let mut new_time = fasync::MonotonicInstant::from_nanos(
            exec.now().into_nanos() + stop_timeout.into_nanos(),
        );
        exec.set_fake_time(new_time);
        exec.wake_expired_timers();

        // Advance our futures, we expect the mock controller will do nothing
        // before the deadline
        assert!(exec.run_until_stalled(&mut mock_controller_future).is_pending());

        // The stop fn should now send a kill signal
        assert!(exec.run_until_stalled(&mut stop_fut).is_pending());

        // The kill signal should cause the mock controller to exit its loop
        // We still expect the mock controller to be holding responses
        assert!(exec.run_until_stalled(&mut mock_controller_future).is_ready());

        // The ComponentController should have sent the kill message by now
        let mut check_msgs = Box::pin(async {
            // Check if the mock controller got all the messages we expected it to get.
            let msg_map = requests.lock().await;
            let msg_list = msg_map.get(&program_koid).expect("No messages received on the channel");
            assert_eq!(msg_list, &vec![ControlMessage::Stop, ControlMessage::Kill]);
        });
        assert!(exec.run_until_stalled(&mut check_msgs).is_ready());
        drop(check_msgs);

        // Roll time beyond the kill timeout period
        new_time = fasync::MonotonicInstant::from_nanos(
            exec.now().into_nanos() + kill_timeout.into_nanos(),
        );
        exec.set_fake_time(new_time);
        exec.wake_expired_timers();

        // The stop fn should have given up and returned a result
        assert_matches!(
            exec.run_until_stalled(&mut stop_fut),
            Poll::Ready(Ok(StopConclusion {
                disposition: StopDisposition::KilledAfterTimeout,
                escrow_request: None,
            }))
        );
    }

    #[fuchsia::test]
    /// Test scenario where the controller does not stop the component within
    /// the allowed period and the component stop state machine has to send
    /// the `kill` message to the controller. The controller then kills the
    /// component before the kill timeout is reached.
    fn stop_component_successful_with_kill_result() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();

        // Create a controller which takes far longer than allowed to stop the
        // component.
        let (program, server) = mocks::mock_program();
        let stop_timeout = zx::MonotonicDuration::from_seconds(5);
        let kill_timeout = zx::MonotonicDuration::from_millis(200);

        // Create a request map which the MockController will fill with
        // requests it received related to mocked component.
        let requests: Arc<Mutex<HashMap<Koid, Vec<ControlMessage>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let kill_resp_delay = zx::MonotonicDuration::from_millis(kill_timeout.into_millis() / 2);
        let controller = MockController::new_with_responses(
            server,
            requests.clone(),
            program.koid(),
            // Process the stop message, but fail to close the channel. Channel
            // closure is the indication that a component stopped.
            ControllerActionResponse {
                close_channel: false,
                delay: None,
                termination_status: Some(Ok(())),
                exit_code: Some(MOCK_EXIT_CODE),
            },
            ControllerActionResponse {
                close_channel: true,
                delay: Some(kill_resp_delay),
                termination_status: Some(Ok(())),
                exit_code: Some(MOCK_EXIT_CODE),
            },
        );
        controller.serve();

        let stop_timer = Box::pin(async move {
            let timer = fasync::Timer::new(fasync::MonotonicInstant::after(stop_timeout));
            timer.await;
        });
        let kill_timer = Box::pin(async move {
            let timer = fasync::Timer::new(fasync::MonotonicInstant::after(kill_timeout));
            timer.await;
        });
        let mut stop_fut = Box::pin(program.stop_or_kill_with_timeout(stop_timer, kill_timer));

        // it should be the case we stall waiting for a response from the
        // controller
        assert!(exec.run_until_stalled(&mut stop_fut).is_pending());

        // Roll time passed the stop timeout.
        let mut new_time = fasync::MonotonicInstant::from_nanos(
            exec.now().into_nanos() + stop_timeout.into_nanos(),
        );
        exec.set_fake_time(new_time);
        exec.wake_expired_timers();
        assert!(exec.run_until_stalled(&mut stop_fut).is_pending());

        // Roll forward to where the mock controller should have closed the
        // controller channel.
        new_time = fasync::MonotonicInstant::from_nanos(
            exec.now().into_nanos() + kill_resp_delay.into_nanos(),
        );
        exec.set_fake_time(new_time);
        exec.wake_expired_timers();

        // At this point stop_component() will have completed, but the
        // controller's future was not polled to completion.
        assert_matches!(
            exec.run_until_stalled(&mut stop_fut),
            Poll::Ready(Ok(StopConclusion {
                disposition: StopDisposition::Killed(StopInfo {
                    termination_status: Ok(()),
                    exit_code: Some(MOCK_EXIT_CODE)
                }),
                escrow_request: None,
            }))
        );
    }

    #[fuchsia::test]
    /// In this case we expect success, but that the stop state machine races
    /// with the controller. The state machine's timer expires, but when it
    /// goes to send the kill message, it finds the control channel is closed,
    /// indicating the component stopped.
    fn stop_component_successful_race_with_controller() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();

        // Create a program which takes far longer than allowed to stop the
        // component.
        let (program, server) = mocks::mock_program();
        let stop_timeout = zx::MonotonicDuration::from_seconds(5);
        let kill_timeout = zx::MonotonicDuration::from_millis(1);

        // Create a request map which the MockController will fill with
        // requests it received related to mocked component.
        let requests: Arc<Mutex<HashMap<Koid, Vec<ControlMessage>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let close_delta = zx::MonotonicDuration::from_millis(10);
        let resp_delay = zx::MonotonicDuration::from_millis(
            stop_timeout.into_millis() + close_delta.into_millis(),
        );
        let controller = MockController::new_with_responses(
            server,
            requests.clone(),
            program.koid(),
            // Process the stop message, but fail to close the channel after
            // the timeout of stop_component()
            ControllerActionResponse {
                close_channel: true,
                delay: Some(resp_delay),
                termination_status: Some(Ok(())),
                exit_code: Some(MOCK_EXIT_CODE),
            },
            // This is irrelevant because the controller should never receive
            // the kill message
            ControllerActionResponse {
                close_channel: true,
                delay: Some(resp_delay),
                termination_status: Some(Ok(())),
                exit_code: Some(MOCK_EXIT_CODE),
            },
        );
        controller.serve();

        let stop_timer = Box::pin(async move {
            let timer = fasync::Timer::new(fasync::MonotonicInstant::after(stop_timeout));
            timer.await;
        });
        let kill_timer = Box::pin(async move {
            let timer = fasync::Timer::new(fasync::MonotonicInstant::after(kill_timeout));
            timer.await;
        });
        let epitaph_fut = program.on_terminate();
        let program_koid = program.koid();
        let mut stop_fut = Box::pin(program.stop_or_kill_with_timeout(stop_timer, kill_timer));

        // it should be the case we stall waiting for a response from the
        // controller
        assert!(exec.run_until_stalled(&mut stop_fut).is_pending());

        // Roll time passed the stop timeout and beyond when the controller
        // will close the channel
        let new_time =
            fasync::MonotonicInstant::from_nanos(exec.now().into_nanos() + resp_delay.into_nanos());
        exec.set_fake_time(new_time);
        exec.wake_expired_timers();

        // This future waits for the client channel to close. This creates a
        // rendezvous between the controller's execution context and the test.
        // Without this the message map state may be inconsistent.
        let mut check_msgs = Box::pin(async {
            epitaph_fut.await;

            let msg_map = requests.lock().await;
            let msg_list = msg_map.get(&program_koid).expect("No messages received on the channel");

            assert_eq!(msg_list, &vec![ControlMessage::Stop]);
        });

        // Expect the message check future to complete because the controller
        // should close the channel.
        assert!(exec.run_until_stalled(&mut check_msgs).is_ready());
        drop(check_msgs);

        // At this point stop_component() should now poll to completion because
        // the control channel is closed, but stop_component will perceive this
        // happening after its timeout expired.
        assert_matches!(
            exec.run_until_stalled(&mut stop_fut),
            Poll::Ready(Ok(StopConclusion {
                disposition: StopDisposition::Killed(StopInfo {
                    termination_status: Ok(()),
                    exit_code: Some(MOCK_EXIT_CODE)
                }),
                escrow_request: None,
            }))
        );
    }

    /// Test that `Program::finalize` will reap the `OnEscrow` event received from the
    /// `ComponentController`.
    #[fuchsia::test]
    async fn finalize_program() {
        let (program, server) = mocks::mock_program();
        let (stream, control) = server.into_stream_and_control_handle();
        let (outgoing_dir_client, outgoing_dir_server) = fidl::endpoints::create_endpoints();

        control
            .send_on_escrow(fcrunner::ComponentControllerOnEscrowRequest {
                outgoing_dir: Some(outgoing_dir_server),
                ..Default::default()
            })
            .unwrap();
        control.shutdown();
        drop(control);
        drop(stream);
        program.on_terminate().await;

        let escrow = program.finalize().escrow_request;
        let received_outgoing_dir_server = escrow.unwrap().outgoing_dir.unwrap();
        assert_eq!(
            outgoing_dir_client.as_handle_ref().basic_info().unwrap().koid,
            received_outgoing_dir_server.as_handle_ref().basic_info().unwrap().related_koid
        );
    }
}

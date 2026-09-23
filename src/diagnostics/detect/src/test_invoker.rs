// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{DetectionOpts, RunsDetection};
use fidl_fuchsia_diagnostics_test::{
    DetectControllerRequest, DetectControllerRequestStream, TestCaseControllerRequest,
    TestCaseControllerRequestStream,
};
use futures::StreamExt;
use futures::lock::Mutex;
use std::sync::Arc;

/// This file serves the fuchsia.diagnostics.test.DetectController protocol,
/// which is used for performance testing.
/// This file is unrelated to lib.rs::Mode::Testing which is used for integration testing.
pub(crate) async fn run_test_service(
    mut stream: DetectControllerRequestStream,
    detection_runner: Arc<Mutex<impl RunsDetection>>,
) {
    while let Some(Ok(request)) = stream.next().await {
        match request {
            DetectControllerRequest::EnterTestMode { test_controller, responder } => {
                let mut detection_runner = detection_runner.lock().await;
                let _: Result<_, _> = responder.send();
                run_test_case_service(test_controller.into_stream(), &mut *detection_runner).await;
            }
        }
    }
}

async fn run_test_case_service(
    mut stream: TestCaseControllerRequestStream,
    detection_runner: &mut impl RunsDetection,
) {
    while let Some(Ok(request)) = stream.next().await {
        let TestCaseControllerRequest::RunDefaultCycle { responder } = request;
        detection_runner.run_detection(DetectionOpts { cpu_test: true }).await;
        let _: Result<_, _> = responder.send();
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::Stats;
    use fidl_fuchsia_diagnostics_test::{
        DetectControllerMarker, DetectControllerProxy, TestCaseControllerMarker,
    };
    use fuchsia_async::{Scope, TestExecutor};
    use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};

    struct TestDetectionRunner {
        run_sender: UnboundedSender<()>,
        stats: Stats,
    }

    impl TestDetectionRunner {
        fn create() -> (Arc<Mutex<Self>>, UnboundedReceiver<()>) {
            let (run_sender, run_receiver) = mpsc::unbounded();
            (Arc::new(Mutex::new(Self { run_sender, stats: Stats::default() })), run_receiver)
        }
    }

    impl RunsDetection for TestDetectionRunner {
        async fn run_detection(&mut self, _opts: DetectionOpts) {
            self.run_sender.unbounded_send(()).unwrap();
        }

        fn stats(&self) -> &Stats {
            &self.stats
        }
    }

    trait CountsMessages {
        async fn expect_n_messages(&mut self, n: usize);
    }

    impl CountsMessages for UnboundedReceiver<()> {
        async fn expect_n_messages(&mut self, n: usize) {
            for _ in 0..n {
                assert_eq!(self.next().await, Some(()));
            }
            assert!(self.try_next().is_err());
        }
    }

    fn get_controller_proxy(
        scope: &Scope,
        detection_runner: Arc<Mutex<impl RunsDetection + Send + 'static>>,
    ) -> DetectControllerProxy {
        let (client_end, server_end) =
            fidl::endpoints::create_endpoints::<DetectControllerMarker>();
        scope.spawn(run_test_service(server_end.into_stream(), detection_runner));
        client_end.into_proxy()
    }

    /// Make sure that the FIDL server doesn't hang or crash, and does call run_detection
    /// when it should.
    #[fuchsia::test(allow_stalls = false)]
    async fn exercise_server() {
        let (detection_runner, mut run_receiver) = TestDetectionRunner::create();
        let scope = Scope::new();
        let controller_proxy = get_controller_proxy(&scope, detection_runner);
        // Create the tearoff for a test-mode session
        let (test_case_proxy, first_test_case_server_end) =
            fidl::endpoints::create_proxy::<TestCaseControllerMarker>();
        controller_proxy.enter_test_mode(first_test_case_server_end).await.unwrap();
        run_receiver.expect_n_messages(0).await;
        test_case_proxy.run_default_cycle().await.unwrap();
        run_receiver.expect_n_messages(1).await;
        test_case_proxy.run_default_cycle().await.unwrap();
        run_receiver.expect_n_messages(1).await;
        std::mem::drop(test_case_proxy);
        std::mem::drop(controller_proxy);
        scope.await;
    }

    /// Make sure that the program doesn't run a test in a second session while
    /// a first session is still open.
    #[fuchsia::test(allow_stalls = false)]
    async fn ensure_non_overlapping_sessions() {
        let (detection_runner, mut run_receiver) = TestDetectionRunner::create();
        let scope = Scope::new();
        let controller_proxy = get_controller_proxy(&scope, detection_runner);
        // Create the tearoff for a test-mode session. This test should demonstrate correct behavior
        // even though we don't run a cycle on this proxy.
        let (first_test_case_proxy, first_test_case_server_end) =
            fidl::endpoints::create_proxy::<TestCaseControllerMarker>();
        controller_proxy.enter_test_mode(first_test_case_server_end).await.unwrap();
        let controller_proxy_clone = controller_proxy.clone();
        // The second test-mode session should run, but only after the first is done.
        scope.spawn(async move {
            let (second_test_case_proxy, second_test_case_server_end) =
                fidl::endpoints::create_proxy::<TestCaseControllerMarker>();
            controller_proxy_clone.enter_test_mode(second_test_case_server_end).await.unwrap();
            second_test_case_proxy.run_default_cycle().await.unwrap();
        });
        // Test the test-mode-lockout logic by giving the spawned second_test_case code an
        // opportunity to run as far as it can. It shouldn't run its test cycle yet.
        let _ = TestExecutor::poll_until_stalled(std::future::pending::<()>()).await;
        // Yup, no tests ran yet - right?
        run_receiver.expect_n_messages(0).await;
        // End the first session. Now the second session should run.
        std::mem::drop(first_test_case_proxy);
        run_receiver.expect_n_messages(1).await;
        std::mem::drop(controller_proxy);
        scope.await;
    }

    /// Make sure the program doesn't run a test in a second session while a first session is open,
    /// even if the sessions are on different connections.
    #[fuchsia::test(allow_stalls = false)]
    async fn ensure_non_overlapping_connections() {
        let (detection_runner, mut run_receiver) = TestDetectionRunner::create();
        let scope = Scope::new();
        let controller_proxy = get_controller_proxy(&scope, detection_runner.clone());
        // Create the tearoff for a test-mode session. This test should demonstrate correct behavior
        // even though we don't run a cycle on this proxy.
        let (first_test_case_proxy, first_test_case_server_end) =
            fidl::endpoints::create_proxy::<TestCaseControllerMarker>();
        // We've had controller proxy. What about second controller proxy?
        let second_controller_proxy = get_controller_proxy(&scope, detection_runner);
        // Second controller connected, but it's not in test mode, so first controller should work
        // normally.
        controller_proxy.enter_test_mode(first_test_case_server_end).await.unwrap();
        run_receiver.expect_n_messages(0).await;
        first_test_case_proxy.run_default_cycle().await.unwrap();
        run_receiver.expect_n_messages(1).await;
        // Now we'll try to run a test cycle on second controller, while first controller is still
        // in test mode.
        scope.spawn(async move {
            let (second_test_case_proxy, second_test_case_server_end) =
                fidl::endpoints::create_proxy::<TestCaseControllerMarker>();
            second_controller_proxy.enter_test_mode(second_test_case_server_end).await.unwrap();
            second_test_case_proxy.run_default_cycle().await.unwrap();
        });
        // Test the test-mode-lockout logic by giving the spawned second_test_case code an
        // opportunity to run as far as it can. It shouldn't run its test cycle yet.
        let _ = TestExecutor::poll_until_stalled(std::future::pending::<()>()).await;
        // The first controller is still in test mode. The second was able to connect,
        // but can't enter test mode to run its test cycle. (It'll be waiting for a
        // response to enter_test_mode().)
        run_receiver.expect_n_messages(0).await;
        // End the first session. Now the second session (on the second controller) should run.
        std::mem::drop(first_test_case_proxy);
        run_receiver.expect_n_messages(1).await;
        // Drop the controller_proxy client end of the FIDL server
        // (second_controller_proxy was dropped in the spawned block)
        std::mem::drop(controller_proxy);
        scope.await;
    }
}

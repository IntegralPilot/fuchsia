// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::future::Future;
use fdf_component::{Driver, DriverContext, DriverError};
use fidl_fuchsia_hardware_power as fhw_power;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_power_broker as fpower_broker;
use fidl_fuchsia_power_system as fpower;
use fuchsia_async as fasync;
use fuchsia_component::client::{Connect, SVC_DIR};
use fuchsia_component::directory::Directory;
use futures::TryStreamExt;
use log::{error, warn};
use std::sync::{Arc, Weak};
use zx::Status;

use fidl_next as _;
use fidl_next_fuchsia_hardware_powerdomain as fpowerdomain;

/// Implement this trait if you'd like to get notifications when the system is about to go into
/// suspend and come out of resume.
pub trait SuspendableDriver: Driver {
    /// Called prior the system entering suspend. The system is not guaranteed to enter suspend,
    /// but `resume` will be called regardless before a subsequent suspension attempt occurs.
    fn suspend(&self) -> impl Future<Output = ()> + Send;

    /// Called after `suspend` to indicate the system is no longer in suspend. The system may not
    /// have actually entered suspension in between `suspend` and `resume` invocations. NOTE: there
    /// is no initial call to `resume`; drivers are expected to start in a non-suspended state and
    /// `resume` will only be called after `suspend`.
    fn resume(&self) -> impl Future<Output = ()> + Send;

    /// Returns whether or not suspend is enabled. If false is returned, suspend and resume methods
    /// will never be called.
    fn suspend_enabled(&self) -> bool;
}

/// Wrapper trait to indicate the driver supports power operations.
pub struct Suspendable<T: Driver> {
    #[expect(unused)]
    scope: Option<fasync::Scope>,
    driver: Arc<T>,
}

async fn run_suspend_blocker<T: SuspendableDriver>(
    driver: Weak<T>,
    mut service: fpower::SuspendBlockerRequestStream,
) {
    use fpower::SuspendBlockerRequest::*;
    while let Some(req) = service.try_next().await.unwrap() {
        match req {
            BeforeSuspend { responder, .. } => {
                if let Some(driver) = driver.upgrade() {
                    driver.suspend().await;
                } else {
                    return;
                }
                let _ = responder.send();
            }
            AfterResume { responder, .. } => {
                if let Some(driver) = driver.upgrade() {
                    driver.resume().await;
                } else {
                    return;
                }
                let _ = responder.send();
            }
            // Ignore unknown requests.
            _ => {
                warn!("Received unknown sag listener request");
            }
        }
    }
}

async fn run_element_runner<T: SuspendableDriver>(
    driver: Weak<T>,
    mut service: fpower_broker::ElementRunnerRequestStream,
) {
    let mut first_activation_occurred = false;
    while let Some(req) = service.try_next().await.unwrap_or_default() {
        if let fpower_broker::ElementRunnerRequest::SetLevel { level, responder } = req {
            let Some(driver) = driver.upgrade() else { return };
            if level != fhw_power::FrameworkElementLevels::Off.into_primitive() as u8 {
                // Hide the initial resume because drivers should start in a resumed state and it is
                // easier for users if we guarantee that a call to resume always follows a call to
                // suspend.
                if first_activation_occurred {
                    driver.resume().await;
                }
            } else {
                driver.suspend().await;
            }
            let _ = responder.send();
            first_activation_occurred = true;
        }
    }
}

impl<T: SuspendableDriver + Send + Sync> Driver for Suspendable<T> {
    const NAME: &str = T::NAME;

    async fn start(mut context: DriverContext) -> Result<Self, DriverError> {
        let mut runner = context
            .start_args
            .power_element_args
            .as_mut()
            .and_then(|args| args.runner_server.take());

        let (svc, svc_server) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        context
            .incoming
            .open(
                SVC_DIR,
                fio::Flags::PROTOCOL_DIRECTORY | fio::PERM_READABLE,
                svc_server.into_channel(),
            )
            .map_err(|error| {
                error!(error:?; "Error opening svc directory");
                Status::INTERNAL
            })?;

        let driver = Arc::new(T::start(context).await?);

        let scope = if driver.suspend_enabled() {
            let scope = fasync::Scope::new_with_name("suspend");
            if let Some(runner) = runner.take() {
                let weak_driver = Arc::downgrade(&driver);
                scope.spawn(
                    async move { run_element_runner(weak_driver, runner.into_stream()).await },
                );
            } else {
                let sag =
                    fpower::ActivityGovernorProxy::connect_at_dir_root(&svc).map_err(|error| {
                        error!(error:?; "Error connecting to sag");
                        Status::INTERNAL
                    })?;

                let (client, server) = fidl::endpoints::create_endpoints();

                let _ = sag
                    .register_suspend_blocker(
                        fpower::ActivityGovernorRegisterSuspendBlockerRequest {
                            suspend_blocker: Some(client),
                            name: Some(Self::NAME.into()),
                            ..Default::default()
                        },
                    )
                    .await
                    .map_err(|error| {
                        error!(error:?; "Error connecting to sag");
                        Status::INTERNAL
                    })?
                    .map_err(|error| {
                        error!(error:?; "Error connecting to sag");
                        Status::INTERNAL
                    })?;

                let weak_driver = Arc::downgrade(&driver);
                scope.spawn(
                    async move { run_suspend_blocker(weak_driver, server.into_stream()).await },
                );
            }
            Some(scope)
        } else {
            None
        };

        Ok(Self { driver, scope })
    }

    async fn stop(&self) {
        self.driver.stop().await;
    }
}

/// Extension trait for [`DriverContext`] to simplify connecting to power resources.
pub trait PowerExt {
    /// Connects to a power domain resource with the given instance name.
    fn connect_to_powerdomain(
        &self,
        instance: &str,
    ) -> Result<fidl_next::Client<fpowerdomain::Domain>, DriverError>;
}

impl PowerExt for DriverContext {
    fn connect_to_powerdomain(
        &self,
        instance: &str,
    ) -> Result<fidl_next::Client<fpowerdomain::Domain>, DriverError> {
        let service = self
            .incoming
            .service::<fdf_component::ServiceInstance<fpowerdomain::Service>>()
            .instance(instance)
            .connect_next()?;
        let (client, server) = fidl_next::fuchsia::create_channel();
        service.domain(server)?;
        Ok(client.spawn())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fdf_component::testing::harness::TestHarness;
    use fidl_fuchsia_driver_framework as fdf;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct TestDriver {
        suspend_called: Arc<AtomicBool>,
        resume_called: Arc<AtomicBool>,
        suspend_enabled: bool,
        stop_called: Arc<AtomicBool>,
    }

    impl Driver for TestDriver {
        const NAME: &str = "test_driver";

        async fn start(_context: DriverContext) -> Result<Self, DriverError> {
            Ok(Self {
                suspend_called: Arc::new(AtomicBool::new(false)),
                resume_called: Arc::new(AtomicBool::new(false)),
                suspend_enabled: true,
                stop_called: Arc::new(AtomicBool::new(false)),
            })
        }

        async fn stop(&self) {
            self.stop_called.store(true, Ordering::SeqCst);
        }
    }

    impl SuspendableDriver for TestDriver {
        async fn suspend(&self) {
            self.suspend_called.store(true, Ordering::SeqCst);
        }

        async fn resume(&self) {
            self.resume_called.store(true, Ordering::SeqCst);
        }

        fn suspend_enabled(&self) -> bool {
            self.suspend_enabled
        }
    }

    #[fuchsia::test]
    async fn test_suspend_resume_with_runner() {
        let (runner_client, runner_server) =
            fidl::endpoints::create_endpoints::<fpower_broker::ElementRunnerMarker>();

        let mut harness = TestHarness::<Suspendable<TestDriver>>::new().set_power_element_args(
            fdf::PowerElementArgs { runner_server: Some(runner_server), ..Default::default() },
        );

        let driver_under_test = harness.start_driver().await.expect("Failed to start driver");
        let (test_driver_stop_called, test_driver_resume_called, test_driver_suspend_called) = {
            let suspendable = driver_under_test.get_driver().expect("Failed to get driver");
            (
                suspendable.driver.stop_called.clone(),
                suspendable.driver.resume_called.clone(),
                suspendable.driver.suspend_called.clone(),
            )
        };

        let runner_proxy = runner_client.into_proxy();

        // Level 1 should NOT trigger resume as it's the initial activation
        runner_proxy.set_level(1).await.expect("Failed to set level");
        assert!(!test_driver_resume_called.load(Ordering::SeqCst));

        // Level 0 should trigger suspend
        runner_proxy.set_level(0).await.expect("Failed to set level");
        assert!(test_driver_suspend_called.load(Ordering::SeqCst));
        test_driver_suspend_called.store(false, Ordering::SeqCst);

        // Level 1 again should trigger resume
        runner_proxy.set_level(1).await.expect("Failed to set level");
        assert!(test_driver_resume_called.load(Ordering::SeqCst));

        driver_under_test.stop_driver().await;
        assert!(test_driver_stop_called.load(Ordering::SeqCst));
    }
}

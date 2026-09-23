// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{StarnixKernel, generate_kernel_name};
use anyhow::Error;
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_runner as frunner;
use fidl_fuchsia_power_system as fpower;
use frunner::{ComponentControllerMarker, ComponentStartInfo};
use fuchsia_component::client::connect_to_protocol;
use fuchsia_sync::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use vfs::execution_scope::ExecutionScope;
use zx;

/// The component URL of the Starnix kernel.
const KERNEL_URL: &str = "starnix_kernel#meta/starnix_kernel.cm";

/// Create the power lease name for better readability based on the Starnix kernel name.
fn create_lease_name(kernel_name: &str) -> String {
    format!("starnix-kernel-{}", kernel_name)
}

/// [`Kernels`] manages a collection of starnix kernels.
pub struct Kernels {
    kernels: Arc<Mutex<HashMap<zx::Koid, StarnixKernel>>>,
    background_tasks: ExecutionScope,
}

impl Kernels {
    /// Creates a new [`Kernels`] instance.
    pub fn new() -> Self {
        let kernels = Default::default();
        Self { kernels, background_tasks: ExecutionScope::new() }
    }

    /// Runs a new starnix kernel and adds it to the collection.
    pub async fn start(
        &self,
        start_info: ComponentStartInfo,
        controller: ServerEnd<ComponentControllerMarker>,
    ) -> Result<(), Error> {
        let realm =
            connect_to_protocol::<fcomponent::RealmMarker>().expect("Failed to connect to realm.");

        let kernel_name = generate_kernel_name(&start_info)?;
        let wake_lease = 'out: {
            let Ok(activity_governor) = connect_to_protocol::<fpower::ActivityGovernorMarker>()
            else {
                break 'out None;
            };

            match activity_governor
                .take_application_activity_lease(&create_lease_name(&kernel_name))
                .await
            {
                Ok(l) => Some(l),
                Err(e) => {
                    log::warn!("Failed to acquire application activity lease for kernel: {:?}", e);
                    None
                }
            }
        };

        if let Ok(boot_control) = connect_to_protocol::<fpower::BootControlMarker>() {
            log::info!(
                "Notifying Fuchsia SystemActivityGovernor that Starnix has booted to allow suspend"
            );
            let _ = boot_control.set_boot_complete().await;
            if let Err(e) = boot_control.set_boot_complete().await {
                log::warn!(
                    "Failed to notify Fuchsia SystemActivityGovernor that Starnix has booted: {:?}",
                    e
                );
            }
        }

        let (kernel, on_stop) =
            StarnixKernel::create(realm, KERNEL_URL, start_info, controller).await?;
        let kernel_job = kernel.job.clone();
        let kernel_koid = kernel.job.koid()?;

        *kernel.wake_lease.lock() = wake_lease;
        log::info!("Acquired wake lease for {:?}", kernel_job);

        self.kernels.lock().insert(kernel_koid, kernel);

        let kernels = self.kernels.clone();
        self.background_tasks.spawn(async move {
            on_stop.await;
            _ = kernels.lock().remove(&kernel_koid);
        });

        Ok(())
    }

    /// Gets a momentary snapshot of all kernel jobs.
    pub fn all_jobs(&self) -> Vec<Arc<zx::Job>> {
        self.kernels.lock().iter().map(|(_, k)| Arc::clone(k.job())).collect()
    }

    /// Drops any active wake lease for the container running in the given `container_job`.
    pub fn drop_wake_lease(&self, container_job: &zx::Job) -> Result<(), Error> {
        // LINT.IfChange
        fuchsia_trace::instant!(
            "power",
            "starnix-runner:drop-application-activity-lease",
            fuchsia_trace::Scope::Process
        );
        // LINT.ThenChange(//src/performance/lib/trace_processing/metrics/suspend.py)
        let job_koid = container_job.koid()?;
        if let Some(kernel) = self.kernels.lock().get(&job_koid) {
            kernel.wake_lease.lock().take();
            log::info!("Dropped wake lease for {:?}", container_job);
        }
        Ok(())
    }

    /// Acquires a wake lease for the container running in the given `container_job`.
    pub async fn acquire_wake_lease(&self, container_job: &zx::Job) -> Result<(), Error> {
        // LINT.IfChange
        fuchsia_trace::duration!("power", "starnix-runner:acquire-application-activity-lease");
        // LINT.ThenChange(//src/performance/lib/trace_processing/metrics/suspend.py)
        let job_koid = container_job.koid()?;

        let kernel_name = {
            let guard = self.kernels.lock();
            let Some(kernel) = guard.get(&job_koid) else {
                return Ok(());
            };
            kernel.name.clone()
        };

        let activity_governor = connect_to_protocol::<fpower::ActivityGovernorMarker>()?;
        let wake_lease = match activity_governor
            .take_application_activity_lease(&create_lease_name(&kernel_name))
            .await
        {
            Ok(l) => l,
            Err(e) => {
                log::warn!("Failed to acquire application activity lease for kernel: {:?}", e);
                return Ok(());
            }
        };

        if let Some(kernel) = self.kernels.lock().get(&job_koid) {
            *kernel.wake_lease.lock() = Some(wake_lease);
            log::info!("Acquired wake lease for {:?}", container_job);
        }

        Ok(())
    }
}

impl Drop for Kernels {
    fn drop(&mut self) {
        self.background_tasks.shutdown();
    }
}

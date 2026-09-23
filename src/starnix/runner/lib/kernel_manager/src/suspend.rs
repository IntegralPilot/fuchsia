// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::Kernels;
use anyhow::Error;
use fidl::Peered;
use fidl_fuchsia_starnix_runner as fstarnixrunner;
use fuchsia_inspect::{self as inspect, UintExponentialHistogramProperty, UintProperty};
use log::warn;
use starnix_sync::{LockDepMutex, WakeSourcesLock, WakeWatchersLock};
use std::sync::Arc;
use zx::Task;

/// The signal that the kernel raises to indicate that it's awake.
pub const AWAKE_SIGNAL: zx::Signals = zx::Signals::USER_0;

/// The signal that the kernel raises to indicate that it's suspended.
pub const ASLEEP_SIGNAL: zx::Signals = zx::Signals::USER_1;

pub struct WakeSource {
    handle: zx::NullableHandle,
    name: String,
    signals: zx::Signals,
}

impl WakeSource {
    pub fn from_counter(counter: zx::Counter, name: String) -> Self {
        Self { handle: counter.into_handle(), name, signals: zx::Signals::COUNTER_POSITIVE }
    }

    pub fn from_handle(handle: zx::NullableHandle, name: String, signals: zx::Signals) -> Self {
        Self { handle, name, signals }
    }

    fn as_wait_item(&self) -> zx::WaitItem<'_> {
        self.handle.wait_item(self.signals)
    }
}

pub type WakeSources = std::collections::HashMap<zx::Koid, WakeSource>;

pub struct SuspendContext {
    pub wake_sources: Arc<LockDepMutex<WakeSources, WakeSourcesLock>>,
    pub wake_watchers: Arc<LockDepMutex<Vec<zx::EventPair>, WakeWatchersLock>>,

    /// Inspect node for suspend-related metrics.
    pub node: inspect::Node,
    /// Histogram recording the boot timeline duration (in nanoseconds) of successful container
    /// suspensions.
    pub suspend_duration_histogram: UintExponentialHistogramProperty,
    /// The total number of times the container has attempted to suspend.
    pub suspend_attempts_count: UintProperty,
    /// The total number of times the container has successfully suspended.
    pub suspend_successes_count: UintProperty,
    /// The total number of times the container has failed to suspend.
    pub suspend_failures_count: UintProperty,
}

impl Default for SuspendContext {
    fn default() -> Self {
        let inspector = inspect::component::inspector();
        let node = inspector.root().create_child("suspend");
        let suspend_duration_histogram = node.create_uint_exponential_histogram(
            "suspend_duration_boot_ns",
            inspect::ExponentialHistogramParams {
                floor: 100_000,
                initial_step: 100_000,
                step_multiplier: 2,
                buckets: 32,
            },
        );
        let suspend_attempts_count = node.create_uint("suspend_attempts_count", 0);
        let suspend_successes_count = node.create_uint("suspend_successes_count", 0);
        let suspend_failures_count = node.create_uint("suspend_failures_count", 0);
        Self {
            wake_sources: Default::default(),
            wake_watchers: Default::default(),
            node,
            suspend_duration_histogram,
            suspend_attempts_count,
            suspend_successes_count,
            suspend_failures_count,
        }
    }
}

/// Suspends the container specified by the `payload`.
pub async fn suspend_container(
    payload: fstarnixrunner::ManagerSuspendContainerRequest,
    suspend_context: &Arc<SuspendContext>,
    kernels: &Kernels,
) -> Result<
    Result<fstarnixrunner::ManagerSuspendContainerResponse, fstarnixrunner::SuspendError>,
    Error,
> {
    fuchsia_trace::duration!("power", "starnix-runner:suspending-container");
    let Some(container_job) = payload.container_job else {
        warn!(
            "error suspending container: could not find container job {:?}",
            payload.container_job
        );
        return Ok(Err(fstarnixrunner::SuspendError::SuspendFailure));
    };

    // These handles need to kept alive until the end of the block, as they will
    // resume the kernel when dropped.
    log::info!("Suspending all container processes.");
    let _suspend_handles = match suspend_job(&container_job).await {
        Ok(handles) => handles,
        Err(e) => {
            warn!("error suspending container {:?}", e);
            fuchsia_trace::instant!(
                "power",
                "starnix-runner:suspend-failed-actual",
                fuchsia_trace::Scope::Process
            );
            return Ok(Err(fstarnixrunner::SuspendError::SuspendFailure));
        }
    };
    log::info!("Finished suspending all container processes.");

    let suspend_start = zx::BootInstant::get();
    let resume_reason = {
        // Take locks in a scope that will be closed before awaiting to ensure no deadlock.
        if let Some(wake_locks) = payload.wake_locks {
            match wake_locks
                .wait_one(zx::Signals::EVENT_SIGNALED, zx::MonotonicInstant::ZERO)
                .to_result()
            {
                Ok(_) => {
                    // There were wake locks active after suspending all processes, resume
                    // and fail the suspend call.
                    warn!("error suspending container: Linux wake locks exist");
                    fuchsia_trace::instant!(
                        "power",
                        "starnix-runner:suspend-failed-with-wake-locks",
                        fuchsia_trace::Scope::Process
                    );
                    return Ok(Err(fstarnixrunner::SuspendError::WakeLocksExist));
                }
                Err(_) => {}
            };
        }

        {
            log::info!("Notifying wake watchers of container suspend.");
            let mut watchers = suspend_context.wake_watchers.lock();
            let (clear_mask, set_mask) = (AWAKE_SIGNAL, ASLEEP_SIGNAL);
            watchers.retain(|event| match event.signal_peer(clear_mask, set_mask) {
                Err(zx::Status::PEER_CLOSED) => false,
                Ok(()) => true,
                Err(e) => {
                    log::warn!("Failed to signal wake watcher of suspension: {e:?}");
                    true
                }
            });
        }
        log::info!("Pre-drop wake lease");
        kernels.drop_wake_lease(&container_job)?;
        log::info!("Post-drop wake lease");

        let wake_sources = suspend_context.wake_sources.lock();
        let mut wait_items: Vec<zx::WaitItem<'_>> =
            wake_sources.values().map(|w| w.as_wait_item()).collect();

        // TODO: We will likely have to handle a larger number of wake sources in the
        // future, at which point we may want to consider a Port-based approach. This
        // would also allow us to unblock this thread.
        let wait_result = {
            fuchsia_trace::duration!("power", "starnix-runner:waiting-on-container-wake");
            if wait_items.len() > 0 {
                log::info!("Waiting on container to receive incoming message on wake proxies");
                zx::object_wait_many(
                    &mut wait_items,
                    zx::MonotonicInstant::after(zx::Duration::from_seconds(9)),
                )
                .inspect_err(|e| {
                    warn!("error waiting for wake event {:?}", e);
                })
                .map(|_| ())
            } else {
                Ok(())
            }
        };
        log::info!("Finished waiting on container wake proxies.");

        let mut resume_reasons: Vec<String> = Vec::new();
        for (wake_source, wait_item) in wake_sources.values().zip(&wait_items) {
            if (wait_item.pending() & wait_item.waiting_for()) != zx::Signals::NONE {
                log::info!("Woke container from sleep for: {}", wake_source.name,);
                resume_reasons.push(wake_source.name.clone());
            }
        }

        if resume_reasons.is_empty() {
            match wait_result {
                // Expose the suspend timeout injected by Starnix.
                Err(zx::Status::TIMED_OUT) => Some("starnix-container-timeout".into()),
                // An error was already printed earlier. Ok(_) was always silent.
                _ => None,
            }
        } else {
            Some(resume_reasons.join(","))
        }
    };

    log::info!("Pre-acquire wake lease");
    kernels.acquire_wake_lease(&container_job).await?;
    log::info!("Post-acquire wake lease");

    log::info!("Notifying wake watchers of container wakeup.");
    let mut watchers = suspend_context.wake_watchers.lock();
    let (clear_mask, set_mask) = (ASLEEP_SIGNAL, AWAKE_SIGNAL);
    watchers.retain(|event| match event.signal_peer(clear_mask, set_mask) {
        Err(zx::Status::PEER_CLOSED) => false,
        Ok(()) => true,
        Err(e) => {
            log::warn!("Failed to signal wake watcher of wakeup: {e:?}");
            true
        }
    });

    log::info!("Returning successfully from suspend container");
    Ok(Ok(fstarnixrunner::ManagerSuspendContainerResponse {
        suspend_time: Some((zx::BootInstant::get() - suspend_start).into_nanos()),
        resume_reason,
        ..Default::default()
    }))
}

/// Suspends the provided `zx::Job` by suspending each process in the job individually.
///
/// Returns the suspend handles for all the suspended processes.
///
/// Returns an error if any individual suspend failed. Any suspend handles will be dropped before
/// the error is returned.
async fn suspend_job(kernel_job: &zx::Job) -> Result<Vec<zx::NullableHandle>, Error> {
    let mut handles = std::collections::HashMap::<zx::Koid, zx::NullableHandle>::new();
    loop {
        let process_koids = kernel_job.processes().expect("failed to get processes");
        let mut found_new_process = false;
        let mut processes = vec![];

        for process_koid in process_koids {
            if handles.get(&process_koid).is_some() {
                continue;
            }

            found_new_process = true;

            if let Ok(process_handle) = kernel_job.get_child(&process_koid, zx::Rights::SAME_RIGHTS)
            {
                let process = zx::Process::from(process_handle);
                match process.suspend() {
                    Ok(suspend_handle) => {
                        handles.insert(process_koid, suspend_handle);
                    }
                    Err(zx::Status::BAD_STATE) => {
                        // The process was already dead or dying, and thus can't be suspended.
                        continue;
                    }
                    Err(e) => {
                        log::warn!("Failed process suspension: {:?}", e);
                        return Err(e.into());
                    }
                };
                processes.push(process);
            }
        }

        for process in processes {
            let threads = process.threads().expect("failed to get threads");
            for thread_koid in &threads {
                fuchsia_trace::duration!("power", "starnix-runner:suspend_kernel", "thread_koid" => *thread_koid);
                if let Ok(thread_handle) = process.get_child(&thread_koid, zx::Rights::SAME_RIGHTS)
                {
                    let thread_obj = zx::Thread::from(thread_handle);
                    let mut watchdog_count = 0;
                    loop {
                        if let Ok(info) = thread_obj.info() {
                            if let zx::ThreadState::Blocked(zx::ThreadBlockType::Exception(_)) =
                                info.state
                            {
                                let thread_name = thread_obj
                                    .get_name()
                                    .map(|n| n.to_string())
                                    .unwrap_or_else(|_| "unknown".to_string());
                                log::warn!(
                                    "Thread {} (Koid: {:?}) is blocked on exception, skipping suspend wait.",
                                    thread_name,
                                    thread_koid
                                );
                                break;
                            }
                        }

                        match thread_obj
                            .wait_one(
                                zx::Signals::THREAD_SUSPENDED | zx::Signals::THREAD_TERMINATED,
                                zx::MonotonicInstant::after(zx::Duration::from_millis(100)),
                            )
                            .to_result()
                        {
                            Err(zx::Status::TIMED_OUT) => {
                                watchdog_count += 1;
                                if watchdog_count == 100 || watchdog_count % 600 == 0 {
                                    let process_name = process
                                        .get_name()
                                        .map(|n| n.to_string())
                                        .unwrap_or_else(|_| "unknown".to_string());
                                    let thread_name = thread_obj
                                        .get_name()
                                        .map(|n| n.to_string())
                                        .unwrap_or_else(|_| "unknown".to_string());
                                    let thread_state = thread_obj
                                        .info()
                                        .map(|info| format!("{:?}", info.state))
                                        .unwrap_or_else(|_| "unknown".to_string());
                                    log::warn!(
                                        "[SUSPEND_WATCHDOG] Timeout waiting for task suspension. Thread Koid: {:?} Name: '{}', Process: '{}', State: {}, continuing to wait...",
                                        thread_koid,
                                        thread_name,
                                        process_name,
                                        thread_state
                                    );
                                }
                            }
                            Err(e) => {
                                log::warn!("Error waiting for task suspension: {:?}", e);
                                return Err(e.into());
                            }
                            _ => break,
                        }
                    }
                }
            }
        }

        if !found_new_process {
            break;
        }
    }

    Ok(handles.into_values().collect())
}

// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::events::{SagEvent, SagEventLogger};
use crate::power_observability::{WakeSourceObservability, WakeSourceReport};

use anyhow::Result;
use async_lock::OnceCell as AsyncOnceCell;
use async_trait::async_trait;
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_hardware_power_suspend as fhsuspend;
use fidl_fuchsia_power_broker as fbroker;
use fidl_fuchsia_power_suspend as fsuspend;
use fidl_fuchsia_power_system::CpuLevel;
use fuchsia_async as fasync;
use fuchsia_inspect::Node as INode;
use futures::channel::mpsc::{self, Receiver, Sender};
use futures::lock::Mutex;
use futures::{FutureExt, StreamExt};
use power_broker_client::PowerElementContext;
use std::cell::{Cell, OnceCell};
use std::rc::Rc;

/// The result of a suspend request.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SuspendResult {
    /// Suspend request succeeded.
    Success,
    /// Suspend request was not allowed at the time it was triggered.
    NotAllowed,
    /// Suspend request failed.
    Fail,
}

/// An updater for an [`fsuspend::SuspendStats`] object.
pub trait SuspendStatsUpdater {
    fn update<'a>(&self, update: Box<dyn FnOnce(&mut Option<fsuspend::SuspendStats>) -> bool + 'a>);
}

/// A listener for suspend/resume operations.
/// Also provides statistics about suspend/resume.
#[async_trait(?Send)]
pub trait SuspendResumeListener {
    /// Gets the manager of suspend stats.
    fn suspend_stats(&self) -> &dyn SuspendStatsUpdater;
    /// Leases (Execution State, Suspending). Called after system suspension ends.
    async fn on_suspend_ended(&self, result: SuspendResult);
    /// Notify the listeners that system suspension is about to begin
    async fn notify_on_suspend(&self);
    /// Notify the listeners of a suspend success.
    async fn notify_on_resume(&self);
}

/// Vends "suspend blockers", object references that, when held, indicate that the system should
/// not suspend. Practically speaking, these blockers are used to guarantee that the system does
/// not suspend in the gap between when a wake lease is acquired and when the lease becomes
/// satisfied.
pub struct SuspendBlockManager {
    marker: Mutex<Rc<()>>,
    sag_event_logger: SagEventLogger,
}

impl SuspendBlockManager {
    pub fn new(sag_event_logger: SagEventLogger) -> Self {
        SuspendBlockManager { marker: Mutex::new(Rc::new(())), sag_event_logger }
    }

    /// Returns a suspend blocker, possibly needing to wait until an in-flight suspend attempt
    /// completes.
    pub async fn get_blocker(&self) -> SuspendBlockGuard<std::rc::Weak<()>> {
        let marker = self.marker.lock().await;
        SuspendBlockGuard::new_suspend_blocker(
            Rc::downgrade(&marker),
            self.sag_event_logger.clone(),
        )
    }

    /// Attempts to acquire a suspend blocker immediately, returning None if the system is currently
    /// executing a suspend attempt.
    pub fn try_get_blocker(&self) -> Option<SuspendBlockGuard<std::rc::Weak<()>>> {
        self.marker.try_lock().map(|marker| {
            SuspendBlockGuard::new_suspend_blocker(
                Rc::downgrade(&marker),
                self.sag_event_logger.clone(),
            )
        })
    }

    /// If suspend is allowed, returns a guard that blocks further get_blocker() calls as long as it
    /// is held. Otherwise, returns None.
    pub(crate) fn suspend_allowed(
        &self,
    ) -> Option<SuspendBlockGuard<futures::lock::MutexGuard<'_, Rc<()>>>> {
        match self.marker.try_lock() {
            None => None,
            Some(marker) => {
                if Rc::weak_count(&marker) > 0 {
                    None
                } else {
                    Some(SuspendBlockGuard::new_suspend_lock(marker, self.sag_event_logger.clone()))
                }
            }
        }
    }
}

/// A guard for a "suspend blocker" returned by SuspendBlockManager.
/// This object exists to allow tracking of when suspend is allowed again
/// after a suspend blocker is dropped.
pub struct SuspendBlockGuard<T> {
    /// Data that is protected by this guard.
    data: T,
    /// SAG event logger used to track when lock/block is created and dropped.
    sag_event_logger: SagEventLogger,
    /// Function that is called when the guard is dropped.
    drop_fn: fn(&Self),
}

impl<T> SuspendBlockGuard<T> {
    /// Creates a "suspend lock" style guard.
    /// Only one suspend lock is expected to exist at any time.
    pub fn new_suspend_lock(data: T, sag_event_logger: SagEventLogger) -> Self {
        sag_event_logger.log(SagEvent::SuspendLockAcquired);
        SuspendBlockGuard {
            data,
            sag_event_logger,
            drop_fn: |this| {
                this.sag_event_logger.log(SagEvent::SuspendLockDropped);
            },
        }
    }
}

impl SuspendBlockGuard<std::rc::Weak<()>> {
    /// Creates a "suspend blocker" style guard.
    /// Multiple suspend blockers may exist at any time. As long as one suspend
    /// blocker exists, suspension will be blocked.
    pub fn new_suspend_blocker(data: std::rc::Weak<()>, sag_event_logger: SagEventLogger) -> Self {
        // Only signal SuspendBlockerAcquired when the first guard is created.
        if data.weak_count() == 1 {
            sag_event_logger.log(SagEvent::SuspendBlockerAcquired);
        }

        SuspendBlockGuard {
            data,
            sag_event_logger,
            drop_fn: |this| {
                // Only signal SuspendBlockerDropped when the last guard is dropped.
                if this.data.weak_count() == 1 {
                    this.sag_event_logger.log(SagEvent::SuspendBlockerDropped);
                }
            },
        }
    }
}

impl<T> Drop for SuspendBlockGuard<T> {
    fn drop(&mut self) {
        (self.drop_fn)(&self);
    }
}

impl<T> std::ops::Deref for SuspendBlockGuard<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

/// Controls access to CPU power element and suspend management.
struct CpuManagerInner {
    /// The context used to manage the CPU power element.
    cpu: Rc<PowerElementContext>,
    /// The FIDL proxy to the device used to trigger system suspend.
    suspender: Rc<AsyncOnceCell<Option<fhsuspend::SuspenderProxy>>>,
    /// The suspend state index that will be passed to the suspender when system suspend is
    /// triggered.
    suspend_state_index: u64,
    /// The flag used to track whether suspension is allowed based on CPU's power level.
    /// If true, CPU has transitioned from a higher power state to CpuLevel::Inactive
    /// and is still at the CpuLevel::Inactive power level.
    cpu_element_is_inactive: bool,
    /// Allows the upper layer of SAG to block system suspension.
    suspend_block_manager: Rc<SuspendBlockManager>,
}

/// Manager of the CPU power element and suspend logic.
pub struct CpuManager {
    /// State of the CPU power element and suspend controls.
    inner: Mutex<CpuManagerInner>,
    /// SuspendResumeListener object to notify of suspend/resume.
    suspend_resume_listener: OnceCell<Rc<dyn SuspendResumeListener>>,
    /// Logger for system-wide activity governor events.
    sag_event_logger: SagEventLogger,
    /// The power observability data collection.
    observability: Rc<AsyncOnceCell<WakeSourceObservability>>,
    /// The flag used to track whether the system is shutting down.
    is_shutting_down: Rc<Cell<bool>>,
}

impl CpuManager {
    /// Creates a new CpuManager.
    pub fn new(
        cpu: Rc<PowerElementContext>,
        suspender: Rc<AsyncOnceCell<Option<fhsuspend::SuspenderProxy>>>,
        sag_event_logger: SagEventLogger,
        observability: Rc<AsyncOnceCell<WakeSourceObservability>>,
        is_shutting_down: Rc<Cell<bool>>,
    ) -> Self {
        Self {
            inner: Mutex::new(CpuManagerInner {
                cpu,
                suspender,
                suspend_state_index: 0,
                cpu_element_is_inactive: false,
                suspend_block_manager: Rc::new(SuspendBlockManager::new(sag_event_logger.clone())),
            }),
            suspend_resume_listener: OnceCell::new(),
            sag_event_logger,
            observability,
            is_shutting_down,
        }
    }

    /// Sets the suspend resume listener.
    /// The listener can only be set once. Subsequent calls will result in a panic.
    pub fn set_suspend_resume_listener(
        &self,
        suspend_resume_listener: Rc<dyn SuspendResumeListener>,
    ) {
        self.suspend_resume_listener
            .set(suspend_resume_listener)
            .map_err(|_| anyhow::anyhow!("suspend_resume_listener is already set"))
            .unwrap();
    }

    /// Updates the power level of the CPU power element.
    ///
    /// Returns a Result that indicates whether the system should suspend or not.
    /// If an error occurs while updating the power level, the error is forwarded to the caller.
    pub async fn update_current_level(&self, required_level: fbroker::PowerLevel) -> Result<bool> {
        log::debug!(required_level:?; "update_current_level: acquiring inner lock");
        let mut inner = self.inner.lock().await;

        // After other elements have been informed of required_level for cpu,
        // check whether the system can be suspended.
        if required_level == CpuLevel::Inactive.into_primitive() {
            if self.is_shutting_down.get() {
                log::info!("System is shutting down, halting cpu power element transitions");
                futures::future::pending::<()>().await;
            }

            log::debug!("beginning suspend process for cpu");
            inner.cpu_element_is_inactive = true;
            return Ok(true);
        } else {
            inner.cpu_element_is_inactive = false;
            return Ok(false);
        }
    }

    pub async fn cpu(&self) -> Rc<PowerElementContext> {
        self.inner.lock().await.cpu.clone()
    }

    pub async fn suspend_block_manager(&self) -> Rc<SuspendBlockManager> {
        self.inner.lock().await.suspend_block_manager.clone()
    }

    async fn report_wake_reason(&self, reason: Option<&fhsuspend::WakeReason>) {
        if !self.observability.is_initialized() {
            log::warn!("power observability: observer server not yet available");
        }

        if let Some(reason) = reason {
            let wake_vectors_type =
                reason.wake_vectors_type.unwrap_or(fhsuspend::WakeVectorType::Koid);
            assert_eq!(wake_vectors_type, fhsuspend::WakeVectorType::Koid);
            let overflow = reason.wake_vectors_overflow.unwrap_or(false);
            if overflow {
                log::warn!("power observability: wake vectors overflow, this is not expected");
            }
            let wake_vectors =
                reason.wake_vectors.as_ref().map(|v: &Vec<_>| v.clone()).unwrap_or_default();
            let reports = wake_vectors
                .into_iter()
                .map(|v| WakeSourceReport::new(zx::Koid::from_raw(v)))
                .collect();
            self.observability.wait().await.register_wake_source_reports(reports).await;
        }
    }

    /// Attempts to suspend the system.
    ///
    /// Returns an enum representing the result of the suspend attempt.
    pub async fn trigger_suspend(&self) -> SuspendResult {
        let listener = self.suspend_resume_listener.get().unwrap();
        let mut suspend_failed = false;
        {
            let inner = self.inner.lock().await;
            let suspender = inner.suspender.clone();

            // Drop the lock while we're waiting for the cell to be set
            let (inner, suspender_proxy) = if !suspender.is_initialized() {
                drop(inner);
                let suspender_proxy = suspender.wait().await.as_ref();
                log::debug!("trigger_suspend: acquiring inner lock");

                (self.inner.lock().await, suspender_proxy)
            } else {
                (inner, suspender.wait_blocking().as_ref())
            };

            if !inner.cpu_element_is_inactive {
                log::info!("Suspend not allowed because CPU element is not inactive");
                self.sag_event_logger.log(SagEvent::SuspendAttemptBlocked);
                return SuspendResult::NotAllowed;
            }

            let _lock = match inner.suspend_block_manager.suspend_allowed() {
                Some(lock) => lock,
                None => {
                    log::info!("Suspend not allowed due to outstanding wake leases");
                    self.sag_event_logger.log(SagEvent::SuspendAttemptBlocked);
                    return SuspendResult::NotAllowed;
                }
            };

            self.sag_event_logger.log(SagEvent::SuspendAttempted);
            // LINT.IfChange
            log::info!("Suspending");
            // LINT.ThenChange(//src/testing/end_to_end/honeydew/honeydew/affordances/starnix/system_power_state_controller.py)

            let response = if let Some(suspender) = suspender_proxy {
                // LINT.IfChange
                fuchsia_trace::duration!("power", "system-activity-governor:suspend");
                // LINT.ThenChange(
                //                 //src/performance/lib/trace_processing/metrics/suspend.py,
                //                 //src/performance/lib/trace_processing/tests/power_metrics_test.py
                //)
                Some(
                    suspender
                        .suspend(&fhsuspend::SuspenderSuspendRequest {
                            state_index: Some(inner.suspend_state_index),
                            ..Default::default()
                        })
                        .await,
                )
            } else {
                None
            };
            // LINT.IfChange
            log::info!(response:?; "Resuming");
            // LINT.ThenChange(//src/testing/end_to_end/honeydew/honeydew/affordances/starnix/system_power_state_controller.py)

            if let Some(Ok(Ok(fhsuspend::SuspenderSuspendResponse {
                suspend_duration: Some(suspend_duration),
                ref reason,
                ..
            }))) = response
            {
                let new_cumulative =
                    self.sag_event_logger.update_cumulative_suspend_duration(suspend_duration);
                self.sag_event_logger.log(SagEvent::SuspendResumed {
                    suspend_duration,
                    cumulative_duration: new_cumulative,
                });
                let r = reason.as_ref();
                self.report_wake_reason(r).await;
            } else {
                self.sag_event_logger.log(SagEvent::SuspendFailed);
            };

            listener.suspend_stats().update(Box::new(
                |stats_opt: &mut Option<fsuspend::SuspendStats>| {
                    let stats = stats_opt.as_mut().expect("stats is uninitialized");

                    match response {
                        Some(Ok(Ok(res))) => {
                            stats.last_time_in_suspend = res.suspend_duration;
                            stats.last_time_in_suspend_operations = res.suspend_overhead;

                            if let Some(time_in_suspend) = stats.last_time_in_suspend {
                                stats.success_count = stats.success_count.map(|c| c + 1);
                                stats.total_time_in_suspend =
                                    stats.total_time_in_suspend.map(|t| t + time_in_suspend);
                            } else {
                                log::warn!("Failed to suspend in Suspender");
                                suspend_failed = true;
                                stats.fail_count = stats.fail_count.map(|c| c + 1);
                            }
                        }
                        Some(error) => {
                            let readable_error = error.as_ref().map(|result| {
                                result.as_ref().map_err(|err| zx::Status::err_from_raw(*err))
                            });
                            log::warn!(readable_error:?; "Failed to suspend");
                            stats.fail_count = stats.fail_count.map(|c| c + 1);
                            suspend_failed = true;

                            if let Ok(Err(error)) = error {
                                stats.last_failed_error = Some(error);
                            }
                        }
                        None => {
                            log::warn!("No suspender available, suspend was a no-op");
                            stats.fail_count = stats.fail_count.map(|c| c + 1);
                            stats.last_failed_error = Some(zx::sys::ZX_ERR_NOT_SUPPORTED);
                            suspend_failed = true;
                        }
                    }
                    true
                },
            ));
        }
        // At this point, the suspend request is no longer in flight and has been handled. With
        // `inner` going out of scope, other tasks can modify flags and update the power level of
        // CPU power element.
        let result = if suspend_failed { SuspendResult::Fail } else { SuspendResult::Success };
        listener.on_suspend_ended(result).await;
        result
    }

    pub fn run(
        self: &Rc<Self>,
        element_runner: ServerEnd<fbroker::ElementRunnerMarker>,
        power_elements_node: &INode,
    ) {
        let (suspend_tx, suspend_rx) = mpsc::channel(1);
        self.run_suspend_task(suspend_rx);
        self.run_power_element(element_runner, power_elements_node, suspend_tx);
    }

    fn run_suspend_task(self: &Rc<Self>, mut suspend_signal: Receiver<()>) {
        let cpu_manager = self.clone();

        fasync::Task::local(async move {
            loop {
                log::debug!("awaiting suspend signals");
                suspend_signal.next().await;
                log::info!("trigger_suspend result: {:?}", cpu_manager.trigger_suspend().await);
            }
        })
        .detach();
    }

    pub fn run_power_element(
        self: &Rc<Self>,
        element_runner: ServerEnd<fbroker::ElementRunnerMarker>,
        power_elements_node: &INode,
        suspend_signaller: Sender<()>,
    ) {
        let cpu_manager = self.clone();
        let cpu_node = power_elements_node.create_child("cpu");

        fasync::Task::local(async move {
            let cpu = cpu_manager.cpu().await;

            cpu.run(
                element_runner,
                Some(cpu_node),
                Some(Box::new(move |new_power_level: fbroker::PowerLevel| {
                    let cpu_manager = cpu_manager.clone();
                    let mut suspend_signaller = suspend_signaller.clone();

                    async move {
                        let update_res = cpu_manager.update_current_level(new_power_level).await;
                        if let Ok(true) = update_res {
                            let _ = suspend_signaller.start_send(());
                        }
                    }
                    .boxed_local()
                })),
            )
            .await;
        })
        .detach();
    }
}

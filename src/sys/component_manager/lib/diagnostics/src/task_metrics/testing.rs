// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use crate::task_metrics::runtime_stats_source::*;
use crate::task_metrics::task_info::{TaskInfo, TaskState};
use async_trait::async_trait;
use fuchsia_async::{self as fasync, DurationExt};
use fuchsia_sync::Mutex;
use futures::channel::oneshot;
use injectable_time::{IncrementingFakeTime, TimeSource};
use std::collections::VecDeque;
use std::sync::Arc;
use zx::sys as zx_sys;

/// Mock for a Task. Holds a queue of runtime infos (measurements) that will be fetched for test
/// purposes.
#[derive(Clone, Debug)]
pub struct FakeTask {
    values: Arc<Mutex<VecDeque<zx::TaskRuntimeInfo>>>,
    koid: zx_sys::zx_koid_t,
    event: Arc<zx::Event>,
}

impl Default for FakeTask {
    fn default() -> Self {
        Self::new(0, vec![])
    }
}

impl FakeTask {
    pub fn new(koid: zx_sys::zx_koid_t, values: Vec<zx::TaskRuntimeInfo>) -> Self {
        Self {
            koid,
            values: Arc::new(Mutex::new(values.into())),
            event: Arc::new(zx::Event::create()),
        }
    }

    pub fn terminate(&self) {
        self.event
            .signal(zx::Signals::NONE, zx::Signals::TASK_TERMINATED)
            .expect("signal task terminated");
    }
}

#[async_trait]
impl RuntimeStatsSource for FakeTask {
    fn koid(&self) -> Result<zx_sys::zx_koid_t, zx::Status> {
        Ok(self.koid.clone())
    }

    fn handle_ref(&self) -> zx::HandleRef<'_> {
        self.event.as_handle_ref()
    }

    fn get_runtime_info(&self) -> Result<zx::TaskRuntimeInfo, zx::Status> {
        Ok(self.values.lock().pop_front().unwrap_or(zx::TaskRuntimeInfo::default()))
    }
}

impl TaskInfo<FakeTask> {
    pub async fn force_terminate(&self) {
        {
            let mut guard = self.most_recent_measurement_nanos.lock();
            *guard = Some(self.time_source.now());
        }

        match &*self.task.lock() {
            TaskState::Alive(t) | TaskState::Terminated(t) => t.terminate(),
            TaskState::TerminatedAndMeasured => {}
        }

        // Since the terminate is done asynchronously, ensure we actually have marked this task as
        // terminated to avoid flaking.
        loop {
            if matches!(
                *self.task.lock(),
                TaskState::Terminated(_) | TaskState::TerminatedAndMeasured
            ) {
                return;
            }
            fasync::Timer::new(zx::MonotonicDuration::from_millis(100).after_now()).await;
        }
    }
}

/// Mock for the `RuntimeInfo` object that is provided through the Started hook.
pub struct FakeRuntime {
    container: Mutex<Option<FakeDiagnosticsContainer>>,
    start_time: IncrementingFakeTime,
}

impl FakeRuntime {
    pub fn new(container: FakeDiagnosticsContainer) -> Self {
        Self::new_with_start_times(
            container,
            IncrementingFakeTime::new(0, std::time::Duration::from_nanos(1)),
        )
    }

    pub fn new_with_start_times(
        container: FakeDiagnosticsContainer,
        start_time: IncrementingFakeTime,
    ) -> Self {
        Self { container: Mutex::new(Some(container)), start_time }
    }
}

#[async_trait]
impl ComponentStartedInfo<FakeDiagnosticsContainer, FakeTask> for FakeRuntime {
    fn get_receiver(&self) -> Option<oneshot::Receiver<FakeDiagnosticsContainer>> {
        match self.container.lock().take() {
            None => None,
            Some(container) => {
                let (snd, rcv) = oneshot::channel();
                snd.send(container).unwrap();
                Some(rcv)
            }
        }
    }

    fn start_time(&self) -> zx::BootInstant {
        zx::BootInstant::from_nanos(self.start_time.now())
    }
}

/// Mock for the `ComponentDiagnostics` object coming from the runner containing the optional
/// parent task and the component task.
#[derive(Debug)]
pub struct FakeDiagnosticsContainer {
    parent_task: Option<FakeTask>,
    component_task: Option<FakeTask>,
}

impl FakeDiagnosticsContainer {
    pub fn new(component_task: FakeTask, parent_task: Option<FakeTask>) -> Self {
        Self { component_task: Some(component_task), parent_task }
    }
}

#[async_trait]
impl RuntimeStatsContainer<FakeTask> for FakeDiagnosticsContainer {
    fn take_component_task(&mut self) -> Option<FakeTask> {
        self.component_task.take()
    }

    fn take_parent_task(&mut self) -> Option<FakeTask> {
        self.parent_task.take()
    }
}

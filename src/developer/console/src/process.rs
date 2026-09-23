// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_process as fprocess;
use fuchsia_async as fasync;
use fuchsia_runtime::{HandleInfo, HandleType};
use futures::FutureExt as _;
use log::{debug, error};
use std::pin::pin;
use thiserror::Error;
use zx::Task as _;

use crate::util::{self, ConnectToProtocolError};

#[derive(Debug)]
pub struct Process {
    pub name: zx::Name,
    pub vmo: zx::Vmo,
    pub loader: zx::NullableHandle,
    pub io_handles: Vec<fprocess::HandleInfo>,
    pub args: Vec<String>,
    pub env: Vec<String>,
    pub namespace: Vec<fprocess::NameInfo>,
    pub stopper: Option<zx::EventPair>,
}

impl Process {
    pub async fn launch(self) -> Result<LaunchedProcess, ProcessError> {
        debug!("launching console process {self:?}");
        let Self { name, vmo, loader, io_handles: mut handles, args, env, namespace, stopper } =
            self;

        let launcher = util::connect_to_protocol::<fprocess::LauncherMarker>()?;
        // Create a job for the process.
        let job = fuchsia_runtime::job_default().create_child_job().map_err(ProcessError::Job)?;
        job.set_name(&zx::Name::from_bytes("developer-console".as_bytes()).unwrap())
            .map_err(ProcessError::Job)?;

        // Add the rest of the handles.
        handles.push(fprocess::HandleInfo {
            handle: job
                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                .map_err(ProcessError::Job)?
                .into_handle(),
            id: HandleInfo::new(HandleType::DefaultJob, 0).as_raw(),
        });
        handles.push(fprocess::HandleInfo {
            handle: loader,
            id: HandleInfo::new(HandleType::LdsvcLoader, 0).as_raw(),
        });
        handles.push(fprocess::HandleInfo {
            handle: fuchsia_runtime::duplicate_utc_clock_handle(zx::Rights::SAME_RIGHTS)
                .map_err(ProcessError::Utc)?
                .into_handle(),
            id: HandleInfo::new(HandleType::ClockUtc, 0).as_raw(),
        });

        let env = env.into_iter().map(String::into_bytes).collect::<Vec<_>>();
        launcher.add_environs(&env[..])?;
        let args = args.into_iter().map(String::into_bytes).collect::<Vec<_>>();
        launcher.add_args(&args[..])?;
        launcher.add_handles(handles)?;
        launcher.add_names(namespace)?;

        let job_dup = job.duplicate_handle(zx::Rights::SAME_RIGHTS).map_err(ProcessError::Job)?;

        let (status, process) = launcher
            .launch(fprocess::LaunchInfo { name: name.to_string(), job: job_dup, executable: vmo })
            .await?;
        zx::Status::ok(status).map_err(ProcessError::LaunchingProcess)?;
        let process = process.ok_or(ProcessError::LaunchingProcess(zx::Status::INTERNAL))?;
        Ok(LaunchedProcess { process, job, stopper })
    }
}

#[derive(Error, Debug)]
pub enum ProcessError {
    #[error(transparent)]
    ConnectToProtocol(#[from] ConnectToProtocolError),
    #[error("failure preparing parent job: {0}")]
    Job(zx::Status),
    #[error("failure preparing UTC handle: {0}")]
    Utc(zx::Status),
    #[error("unexpected FIDL error: {0}")]
    Fidl(#[from] fidl::Error),
    #[error("launch process failed: {0}")]
    LaunchingProcess(zx::Status),
    #[error("failed to observe termination and return code: {0}")]
    WaitingTermination(zx::Status),
    #[error("failed to kill job on request: {0}")]
    KillJobOnStop(zx::Status),
}

pub struct LaunchedProcess {
    process: zx::Process,
    job: zx::Job,
    stopper: Option<zx::EventPair>,
}

impl LaunchedProcess {
    pub async fn wait(self) -> Result<i64, ProcessError> {
        let LaunchedProcess { process, job, stopper } = self;

        let mut job = Some(job);

        let stop_future = if let Some(stopper) = stopper.as_ref() {
            fasync::OnSignals::new(stopper, zx::Signals::EVENTPAIR_PEER_CLOSED)
                .fuse()
                .map(|r| match r {
                    Ok(_) => (),
                    Err(e) => {
                        error!("failed to wait on stopper: {e}, killing job anyway");
                    }
                })
                .left_future()
        } else {
            futures::future::pending().right_future()
        };
        let mut stop_future = pin!(stop_future);

        let mut process_wait =
            pin!(fasync::OnSignals::new(&process, zx::Signals::PROCESS_TERMINATED).fuse());
        let result = loop {
            futures::select! {
                r = process_wait => break r,
                () = stop_future => {
                    if let Some(job) = job.take() {
                        debug!("observed stop request, killing job");
                        job.kill().map_err(ProcessError::KillJobOnStop)?;
                    }
                }
            }
        };

        // If we haven't killed the job yet kill it.
        //
        // Our default behavior is to mimic a critical root process for this
        // job, but sidestepping the excessive logging that comes with that.
        if let Some(job) = job.take() {
            job.kill().unwrap_or_else(|e| {
                error!("failed to kill job after process terminated: {e}");
            });
        }

        let _: zx::Signals = result.map_err(ProcessError::WaitingTermination)?;

        Ok(process.info().map_err(ProcessError::WaitingTermination)?.return_code)
    }
}

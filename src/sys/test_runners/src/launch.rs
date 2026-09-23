// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Helpers for launching components.

use crate::logs::{LoggerError, LoggerStream, create_log_stream, create_std_combined_log_stream};
use anyhow::Error;
use cm_types::NamespacePath;
use fidl_fuchsia_component::IntrospectorMarker;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_process as fproc;
use fuchsia_component::client::connect_to_protocol;
use fuchsia_component::directory::AsRefDirectory;
use fuchsia_runtime as runtime;
use namespace::Namespace;
use runtime::{HandleInfo, HandleType};
use thiserror::Error;
use zx::{Process, Rights, Task};

/// The basic rights to use when creating or duplicating a UTC clock. Restrict these
/// on a case-by-case basis only.
///
/// Rights:
///
/// - `Rights::DUPLICATE`, `Rights::TRANSFER`: used to forward the UTC clock in runners.
/// - `Rights::READ`: used to read the clock indication.
/// - `Rights::WAIT`: used to wait on signals such as "clock is updated" or "clock is started".
/// - `Rights::MAP`, `Rights::INSPECT`: used to memory-map the UTC clock.
///
/// The `Rights::WRITE` is notably absent, since on Fuchsia this right is given to particular
/// components only and a writable clock can not be obtained via procargs.
pub static UTC_CLOCK_BASIC_RIGHTS: std::sync::LazyLock<zx::Rights> =
    std::sync::LazyLock::new(|| {
        Rights::DUPLICATE
            | Rights::READ
            | Rights::WAIT
            | Rights::TRANSFER
            | Rights::MAP
            | Rights::INSPECT
    });

/// Error encountered while launching a component.
#[derive(Debug, Error)]
pub enum LaunchError {
    #[error("{:?}", _0)]
    Logger(#[from] LoggerError),

    #[error("Error connecting to launcher: {:?}", _0)]
    Launcher(Error),

    #[error("{:?}", _0)]
    LoadInfo(runner::component::LaunchError),

    #[error("Error launching process: {:?}", _0)]
    LaunchCall(fidl::Error),

    #[error("Error launching process: {:?}", _0)]
    ProcessLaunch(zx::Status),

    #[error("Error duplicating vDSO: {:?}", _0)]
    DuplicateVdso(zx::Status),

    #[error("Error launching process: {:?}", _0)]
    Fidl(#[from] fidl::Error),

    #[error("Error launching process, cannot create socket {:?}", _0)]
    CreateSocket(zx::Status),

    #[error("Error cloning UTC clock: {:?}", _0)]
    UtcClock(zx::Status),

    #[error("unexpected error")]
    UnExpectedError,
}

/// Arguments to launch_process.
pub struct LaunchProcessArgs<'a> {
    /// Relative binary path to /pkg.
    pub bin_path: &'a str,
    /// Name of the binary to add to process. This will be truncated to
    /// `zx::sys::ZX_MAX_NAME_LEN` bytes.
    pub process_name: &'a str,
    /// Job used launch process, if None, a new child of default_job() is used.
    pub job: Option<zx::Job>,
    /// Namespace for binary process to be launched.
    pub ns: Namespace,
    /// Arguments to binary. Binary name is automatically appended as first argument.
    pub args: Option<Vec<String>>,
    /// Extra names to add to namespace. by default only names from `ns` are added.
    pub name_infos: Option<Vec<fproc::NameInfo>>,
    /// Process environment variables.
    pub environs: Option<Vec<String>>,
    /// Extra handle infos to add. Handles for stdout, stderr, and utc_clock are added.
    /// The UTC clock handle is cloned from the current process.
    pub handle_infos: Option<Vec<fproc::HandleInfo>>,
    /// Handle to lib loader protocol client.
    pub loader_proxy_chan: Option<zx::Channel>,
    /// VMO containing mapping to executable binary.
    pub executable_vmo: Option<zx::Vmo>,
    /// Options to create process with.
    pub options: zx::ProcessOptions,
    // The structured config vmo.
    pub config_vmo: Option<zx::Vmo>,
    // The component instance, used only in tracing
    pub component_instance: Option<fidl::Event>,
    // The component URL, used only in tracing
    pub url: Option<String>,
}

/// Launches process, assigns a combined logger stream as stdout/stderr to launched process.
pub async fn launch_process(
    args: LaunchProcessArgs<'_>,
) -> Result<(Process, ScopedJob, LoggerStream), LaunchError> {
    let launcher = connect_to_protocol::<fproc::LauncherMarker>().map_err(LaunchError::Launcher)?;
    let (logger, stdout_handle, stderr_handle) =
        create_std_combined_log_stream().map_err(LaunchError::Logger)?;
    let (process, job) = launch_process_impl(args, launcher, stdout_handle, stderr_handle).await?;
    Ok((process, job, logger))
}

/// Launches process, assigns two separate stdout and stderr streams to launched process.
/// Returns (process, job, stdout_logger, stderr_logger)
pub async fn launch_process_with_separate_std_handles(
    args: LaunchProcessArgs<'_>,
) -> Result<(Process, ScopedJob, LoggerStream, LoggerStream), LaunchError> {
    let launcher = connect_to_protocol::<fproc::LauncherMarker>().map_err(LaunchError::Launcher)?;
    let (stdout_logger, stdout_handle) = create_log_stream().map_err(LaunchError::Logger)?;
    let (stderr_logger, stderr_handle) = create_log_stream().map_err(LaunchError::Logger)?;
    let (process, job) = launch_process_impl(args, launcher, stdout_handle, stderr_handle).await?;
    Ok((process, job, stdout_logger, stderr_logger))
}

async fn launch_process_impl(
    args: LaunchProcessArgs<'_>,
    launcher: fproc::LauncherProxy,
    stdout_handle: zx::NullableHandle,
    stderr_handle: zx::NullableHandle,
) -> Result<(Process, ScopedJob), LaunchError> {
    const STDOUT: u16 = 1;
    const STDERR: u16 = 2;

    let mut handle_infos = args.handle_infos.unwrap_or(vec![]);

    handle_infos.push(fproc::HandleInfo {
        handle: stdout_handle,
        id: HandleInfo::new(HandleType::FileDescriptor, STDOUT).as_raw(),
    });

    handle_infos.push(fproc::HandleInfo {
        handle: stderr_handle,
        id: HandleInfo::new(HandleType::FileDescriptor, STDERR).as_raw(),
    });

    handle_infos.push(fproc::HandleInfo {
        handle: runtime::duplicate_utc_clock_handle(*UTC_CLOCK_BASIC_RIGHTS)
            .map_err(LaunchError::UtcClock)?
            .into_handle(),
        id: HandleInfo::new(HandleType::ClockUtc, 0).as_raw(),
    });

    if let Some(config_vmo) = args.config_vmo {
        handle_infos.push(fproc::HandleInfo {
            handle: config_vmo.into_handle(),
            id: HandleInfo::new(HandleType::ComponentConfigVmo, 0).as_raw(),
        });
    }

    if let Some(svc_dir) = args.ns.get(&NamespacePath::new("/svc").unwrap()) {
        let (client, server) = zx::Channel::create();
        svc_dir
            .as_ref_directory()
            .open("fuchsia.logger.LogSink", fio::Flags::PROTOCOL_SERVICE, server.into())
            .expect("open LogSink for test");
        handle_infos.push(fproc::HandleInfo {
            handle: client.into(),
            id: runtime::HandleInfo::new(runtime::HandleType::LogSink, 0).as_raw(),
        });
    }

    let LaunchProcessArgs {
        bin_path,
        process_name,
        args,
        options,
        ns,
        job,
        name_infos,
        environs,
        loader_proxy_chan,
        executable_vmo,
        component_instance,
        url,
        ..
    } = args;
    // Load the component
    let launch_info =
        runner::component::configure_launcher(runner::component::LauncherConfigArgs {
            bin_path,
            name: process_name,
            args,
            options,
            ns,
            job,
            handle_infos: Some(handle_infos),
            name_infos,
            environs,
            launcher: &launcher,
            loader_proxy_chan,
            executable_vmo,
        })
        .await
        .map_err(LaunchError::LoadInfo)?;

    let component_job = launch_info
        .job
        .duplicate_handle(zx::Rights::SAME_RIGHTS)
        .expect("handle duplication failed!");

    let (status, process) = launcher.launch(launch_info).await.map_err(LaunchError::LaunchCall)?;

    zx::Status::ok(status).map_err(LaunchError::ProcessLaunch)?;

    let process = process.ok_or_else(|| LaunchError::UnExpectedError)?;

    trace_component_start(&process, component_instance, url).await;

    Ok((process, ScopedJob::new(component_job)))
}

/// Reports the component starting to the trace system, if tracing is enabled.
/// Uses the Introspector protocol, which must be routed to the component to
/// report the moniker correctly.
async fn trace_component_start(
    process: &Process,
    component_instance: Option<fidl::Event>,
    url: Option<String>,
) {
    if fuchsia_trace::category_enabled(c"component:start") {
        let pid = process.koid().unwrap().raw_koid();
        let moniker = match component_instance {
            None => "Missing component instance".to_string(),
            Some(component_instance) => match connect_to_protocol::<IntrospectorMarker>() {
                Ok(introspector) => {
                    let component_instance =
                        component_instance.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
                    match introspector.get_moniker(component_instance).await {
                        Ok(Ok(moniker)) => moniker,
                        Ok(Err(e)) => {
                            format!("Couldn't get moniker: {e:?}")
                        }
                        Err(e) => {
                            format!("Couldn't get the moniker: {e:?}")
                        }
                    }
                }
                Err(e) => {
                    format!("Couldn't get introspector: {e:?}")
                }
            },
        };
        let url = url.unwrap_or_else(|| "Missing URL".to_string());
        fuchsia_trace::instant!(
            c"component:start",
            // If you change this name, include the string "-test-".
            // Scripts will match that to detect processes started by a test runner.
            c"-test-",
            fuchsia_trace::Scope::Thread,
            "moniker" => format!("{}", moniker).as_str(),
            "url" => url.as_str(),
            "pid" => pid
        );
    }
}

// Structure to guard job and kill it when going out of scope.
pub struct ScopedJob {
    pub object: Option<zx::Job>,
}

impl ScopedJob {
    pub fn new(job: zx::Job) -> Self {
        Self { object: Some(job) }
    }

    /// Return the job back from this scoped object
    pub fn take(mut self) -> zx::Job {
        self.object.take().unwrap()
    }
}

impl Drop for ScopedJob {
    fn drop(&mut self) {
        if let Some(job) = self.object.take() {
            job.kill().ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::{ClientEnd, Proxy, create_proxy_and_stream};
    use fidl_fuchsia_component_runner as fcrunner;
    use fidl_fuchsia_io as fio;
    use fuchsia_runtime::{job_default, process_self, swap_utc_clock_handle};
    use futures::prelude::*;

    #[test]
    fn scoped_job_works() {
        let new_job = job_default().create_child_job().unwrap();
        let job_dup = new_job.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();

        // create new child job, else killing a job has no effect.
        let _child_job = new_job.create_child_job().unwrap();

        // check that job is alive
        let info = job_dup.info().unwrap();
        assert!(!info.exited);
        {
            let _job_about_to_die = ScopedJob::new(new_job);
        }

        // check that job was killed
        let info = job_dup.info().unwrap();
        assert!(info.exited);
    }

    #[test]
    fn scoped_job_take_works() {
        let new_job = job_default().create_child_job().unwrap();
        let raw_handle = new_job.raw_handle();

        let scoped = ScopedJob::new(new_job);

        let ret_job = scoped.take();

        // make sure we got back same job handle.
        assert_eq!(ret_job.raw_handle(), raw_handle);
    }

    #[fuchsia::test]
    #[ignore] // TODO: b/422533641 - remove
    async fn utc_clock_is_cloned() {
        let clock = fuchsia_runtime::UtcClock::create(zx::ClockOpts::MONOTONIC, None)
            .expect("failed to create clock");
        let expected_clock_koid = clock.koid().expect("failed to get clock koid");

        // We are affecting the process-wide clock here, but since Rust test cases are run in their
        // own process, this won't interact with other running tests.
        let _ = swap_utc_clock_handle(clock).expect("failed to swap clocks");

        // We can't fake all the arguments, as there is actual IO happening. Pass in the bare
        // minimum that a process needs, and use this test's process handle for real values.
        let pkg = fuchsia_fs::directory::open_in_namespace(
            "/pkg",
            fio::PERM_READABLE | fio::PERM_EXECUTABLE,
        )
        .expect("failed to open pkg");
        let args = LaunchProcessArgs {
            bin_path: "bin/test_runners_lib_lib_test", // path to this binary
            environs: None,
            args: None,
            job: None,
            process_name: "foo",
            name_infos: None,
            handle_infos: None,
            ns: vec![fcrunner::ComponentNamespaceEntry {
                path: Some("/pkg".into()),
                directory: Some(ClientEnd::new(pkg.into_channel().unwrap().into_zx_channel())),
                ..Default::default()
            }]
            .try_into()
            .unwrap(),
            loader_proxy_chan: None,
            executable_vmo: None,
            options: zx::ProcessOptions::empty(),
            config_vmo: None,
            url: None,
            component_instance: None,
        };
        let (mock_proxy, mut mock_stream) = create_proxy_and_stream::<fproc::LauncherMarker>();
        let mock_fut = async move {
            let mut all_handles = vec![];
            while let Some(request) =
                mock_stream.try_next().await.expect("failed to get next message")
            {
                match request {
                    fproc::LauncherRequest::AddHandles { handles, .. } => {
                        all_handles.extend(handles);
                    }
                    fproc::LauncherRequest::Launch { responder, .. } => {
                        responder
                            .send(
                                zx::sys::ZX_OK,
                                Some(
                                    process_self()
                                        .duplicate_handle(zx::Rights::SAME_RIGHTS)
                                        .expect("failed to duplicate process handle"),
                                ),
                            )
                            .expect("failed to send reply");
                    }
                    _ => {}
                }
            }
            return all_handles;
        };
        let (_logger, stdout_handle, stderr_handle) =
            create_std_combined_log_stream().map_err(LaunchError::Logger).unwrap();
        let client_fut = async move {
            let _ = launch_process_impl(args, mock_proxy, stdout_handle, stderr_handle)
                .await
                .expect("failed to launch process");
        };

        let (all_handles, ()) = futures::future::join(mock_fut, client_fut).await;
        let clock_id = HandleInfo::new(HandleType::ClockUtc, 0).as_raw();

        let utc_clock_handle = all_handles
            .into_iter()
            .find_map(
                |hi: fproc::HandleInfo| if hi.id == clock_id { Some(hi.handle) } else { None },
            )
            .expect("UTC clock handle");
        let clock_koid = utc_clock_handle.koid().expect("failed to get koid");
        assert_eq!(expected_clock_koid, clock_koid);
    }
}

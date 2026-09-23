// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::vdso_vmo::get_stable_vdso_vmo;
use anyhow::Context;
use cm_types::NamespacePath;
use fidl_connector::Connect;
use fidl_fuchsia_process as fproc;
use fuchsia_runtime::{HandleInfo, HandleInfoError};
use futures::prelude::*;
use process_builder::{
    BuiltProcess, NamespaceEntry, ProcessBuilder, ProcessBuilderError, StartupHandle,
};
use std::ffi::CString;
use std::fmt::Debug;
use std::sync::{Arc, LazyLock};
use thiserror::Error;
use zx::sys;

/// Internal error type for ProcessLauncher which conveniently wraps errors that might
/// result during process launching and allows for mapping them to an equivalent zx::Status, which
/// is what actually gets returned through the protocol.
#[derive(Error, Debug)]
enum LauncherError {
    #[error("Invalid arg: {}", _0)]
    InvalidArg(&'static str),
    #[error("Failed to build new process: {}", _0)]
    BuilderError(ProcessBuilderError),
    #[error("Invalid handle info: {}", _0)]
    HandleInfoError(HandleInfoError),
}

impl LauncherError {
    pub fn as_zx_status(&self) -> zx::Status {
        match self {
            LauncherError::InvalidArg(_) => zx::Status::INVALID_ARGS,
            LauncherError::BuilderError(e) => e.as_zx_status(),
            LauncherError::HandleInfoError(_) => zx::Status::INVALID_ARGS,
        }
    }
}

impl From<ProcessBuilderError> for LauncherError {
    fn from(err: ProcessBuilderError) -> Self {
        LauncherError::BuilderError(err)
    }
}

impl From<HandleInfoError> for LauncherError {
    fn from(err: HandleInfoError) -> Self {
        LauncherError::HandleInfoError(err)
    }
}

#[derive(Default, Debug)]
struct ProcessLauncherState {
    args: Vec<Vec<u8>>,
    environ: Vec<Vec<u8>>,
    name_info: Vec<fproc::NameInfo>,
    handles: Vec<fproc::HandleInfo>,
    options: zx::ProcessOptions,
}

/// Similar to fproc::LaunchInfo, but with the job wrapped in an Arc (to enable use after the struct
/// is moved).
#[derive(Debug)]
struct LaunchInfo {
    executable: zx::Vmo,
    job: Arc<zx::Job>,
    name: String,
}

impl From<fproc::LaunchInfo> for LaunchInfo {
    fn from(info: fproc::LaunchInfo) -> Self {
        LaunchInfo { executable: info.executable, job: Arc::new(info.job), name: info.name }
    }
}

/// An implementation of the `fuchsia.process.Launcher` protocol using the `process_builder` crate.
pub struct ProcessLauncher;

impl ProcessLauncher {
    /// Serves an instance of the `fuchsia.process.Launcher` protocol given an appropriate
    /// RequestStream. Returns when the channel backing the RequestStream is closed or an
    /// unrecoverable error, like a failure to read from the stream, occurs.
    pub async fn serve(mut stream: fproc::LauncherRequestStream) -> Result<(), fidl::Error> {
        // `fuchsia.process.Launcher is stateful. The Add methods accumulate state that is
        // consumed/reset by either Launch or CreateWithoutStarting.
        let mut state = ProcessLauncherState::default();

        while let Some(req) = stream.try_next().await? {
            match req {
                fproc::LauncherRequest::Launch { info, responder } => {
                    let info = LaunchInfo::from(info);
                    let job = info.job.clone();
                    let name = info.name.clone();

                    match Self::launch_process(info, state).await {
                        Ok(process) => {
                            responder.send(zx::sys::ZX_OK, Some(process))?;
                        }
                        Err(err) => {
                            log_launcher_error(&err, "launch", job, name);
                            responder.send(err.as_zx_status().into_raw(), None)?;
                        }
                    }

                    // Reset state to defaults.
                    state = ProcessLauncherState::default();
                }
                fproc::LauncherRequest::CreateWithoutStarting { info, responder } => {
                    let info = LaunchInfo::from(info);
                    let job = info.job.clone();
                    let name = info.name.clone();

                    match Self::create_process(info, state).await {
                        Ok(built) => {
                            let process_data = fproc::ProcessStartData {
                                process: built.process,
                                root_vmar: built.root_vmar,
                                thread: built.thread,
                                entry: built.entry as u64,
                                stack: built.stack as u64,
                                bootstrap: built.bootstrap,
                                vdso_base: built.vdso_base as u64,
                                base: built.elf_base as u64,
                            };
                            responder.send(zx::sys::ZX_OK, Some(process_data))?;
                        }
                        Err(err) => {
                            log_launcher_error(&err, "create", job, name);
                            responder.send(err.as_zx_status().into_raw(), None)?;
                        }
                    }

                    // Reset state to defaults.
                    state = ProcessLauncherState::default();
                }
                fproc::LauncherRequest::AddArgs { mut args, control_handle: _ } => {
                    state.args.append(&mut args);
                }
                fproc::LauncherRequest::AddEnvirons { mut environ, control_handle: _ } => {
                    state.environ.append(&mut environ);
                }
                fproc::LauncherRequest::AddNames { mut names, control_handle: _ } => {
                    state.name_info.append(&mut names);
                }
                fproc::LauncherRequest::AddHandles { mut handles, control_handle: _ } => {
                    state.handles.append(&mut handles);
                }
                fproc::LauncherRequest::SetOptions { options, .. } => {
                    // These options are passed directly to `zx_process_create`, which
                    // will determine whether or not the options are valid.
                    state.options = zx::ProcessOptions::from_bits_retain(options);
                }
            }
        }
        Ok(())
    }

    async fn launch_process(
        info: LaunchInfo,
        state: ProcessLauncherState,
    ) -> Result<zx::Process, LauncherError> {
        Ok(Self::create_process(info, state).await?.start()?)
    }

    async fn create_process(
        info: LaunchInfo,
        state: ProcessLauncherState,
    ) -> Result<BuiltProcess, LauncherError> {
        Ok(Self::create_process_builder(info, state)?.build().await?)
    }

    fn create_process_builder(
        info: LaunchInfo,
        state: ProcessLauncherState,
    ) -> Result<ProcessBuilder, LauncherError> {
        let proc_name = CString::new(info.name)
            .map_err(|_| LauncherError::InvalidArg("Process name contained null byte"))?;
        let stable_vdso_vmo = get_stable_vdso_vmo().map_err(|_| {
            LauncherError::BuilderError(ProcessBuilderError::BadHandle("Failed to get stable vDSO"))
        })?;
        let mut b = ProcessBuilder::new(
            &proc_name,
            &info.job,
            state.options,
            info.executable,
            stable_vdso_vmo,
        )?;

        let arg_cstr = state
            .args
            .into_iter()
            .map(|a| CString::new(a))
            .collect::<Result<_, _>>()
            .map_err(|_| LauncherError::InvalidArg("Argument contained null byte"))?;
        b.add_arguments(arg_cstr);

        let env_cstr = state
            .environ
            .into_iter()
            .map(|e| CString::new(e))
            .collect::<Result<_, _>>()
            .map_err(|_| LauncherError::InvalidArg("Environment string contained null byte"))?;
        b.add_environment_variables(env_cstr);

        let entries = state
            .name_info
            .into_iter()
            .map(|n| Self::new_namespace_entry(n))
            .collect::<Result<_, _>>()?;
        b.add_namespace_entries(entries)?;

        // Note that clients of ``fuchsia.process.Launcher` provide the `fuchsia.ldsvc.Loader`
        // through AddHandles, with a handle type of [HandleType::LdsvcLoader].
        // [ProcessBuilder::add_handles] automatically handles that for convenience.
        let handles = state
            .handles
            .into_iter()
            .map(|h| Self::new_startup_handle(h))
            .collect::<Result<_, _>>()?;
        b.add_handles(handles)?;

        Ok(b)
    }

    // Can't impl TryFrom for these because both types are from external crates. :(
    // Could wrap in a newtype, but then have to unwrap, so this is simplest.
    fn new_namespace_entry(info: fproc::NameInfo) -> Result<NamespaceEntry, LauncherError> {
        let cstr = CString::new(info.path)
            .map_err(|_| LauncherError::InvalidArg("Namespace path contained null byte"))?;
        Ok(NamespaceEntry { path: cstr, directory: info.directory })
    }

    fn new_startup_handle(info: fproc::HandleInfo) -> Result<StartupHandle, LauncherError> {
        Ok(StartupHandle { handle: info.handle, info: HandleInfo::try_from(info.id)? })
    }
}

#[derive(Debug, PartialEq)]
enum LogStyle {
    JobKilled,
    Warn,
    Error,
}

#[derive(Debug, PartialEq)]
struct LogInfo {
    style: LogStyle,
    job_info: String,
    message: &'static str,
}

fn log_launcher_error(err: &LauncherError, op: &str, job: Arc<zx::Job>, name: String) {
    let job_koid =
        job.koid().map(|j| j.raw_koid().to_string()).unwrap_or_else(|_| "<unknown>".to_string());
    let LogInfo { style, job_info, message } = describe_error(err, job.as_handle_ref().cast());

    let level = match style {
        LogStyle::JobKilled => log::Level::Info,
        LogStyle::Warn => log::Level::Warn,
        LogStyle::Error => log::Level::Error,
    };
    log::log!(level,
        op:%, process_name:% = name, job_koid:%, job_info:%, error:% = err;
        "{message}",
    );
}

/// Describes the process launching error.
///
/// Special case BAD_STATE errors to check the job's exited status and log more info.
/// BAD_STATE errors are expected if the job has exited for reasons outside our control, like
/// another process killing the job or the parent job exiting, while a new process is being
/// created in it.
fn describe_error<'a>(err: &LauncherError, job: zx::Unowned<'a, zx::Job>) -> LogInfo {
    let log_level: LogStyle;
    let job_message: String;
    match err {
        LauncherError::BuilderError(err)
            if err.as_zx_status() == zx::Status::BAD_STATE
                || matches!(err, ProcessBuilderError::LoadDynamicLinkerTimeout()) =>
        {
            match job.info() {
                Ok(job_info) => {
                    match job_info.exited {
                        true => {
                            log_level = match job_info.return_code {
                                sys::ZX_TASK_RETCODE_SYSCALL_KILL => LogStyle::JobKilled,
                                _ => LogStyle::Warn,
                            };
                            let return_code_str = match job_info.return_code {
                                sys::ZX_TASK_RETCODE_SYSCALL_KILL => "killed",
                                sys::ZX_TASK_RETCODE_OOM_KILL => "killed on oom",
                                sys::ZX_TASK_RETCODE_POLICY_KILL => {
                                    "killed due to policy violation"
                                }
                                sys::ZX_TASK_RETCODE_VDSO_KILL => {
                                    "killed due to breaking vdso API contract"
                                }
                                sys::ZX_TASK_RETCODE_EXCEPTION_KILL => "killed due to exception",
                                _ => "exited for unknown reason",
                            };
                            job_message = format!(
                                "job was {} (retcode {})",
                                return_code_str, job_info.return_code
                            );
                        }
                        false => {
                            // If the job has not exited, then the BAD_STATE error was unexpected
                            // and indicates a bug somewhere.
                            log_level = LogStyle::Error;
                            job_message = "job is running".to_string();
                        }
                    }
                }
                Err(status) => {
                    log_level = LogStyle::Error;
                    job_message = format!(" (error {} getting job state)", status);
                }
            }
        }
        _ => {
            // Errors other than process builder error do not concern the job's status.
            log_level = LogStyle::Error;
            job_message = "".to_string();
        }
    }

    let message = match log_level {
        LogStyle::JobKilled => {
            "Process operation failed because the parent job was killed. This is expected."
        }
        LogStyle::Warn | LogStyle::Error => "Process operation failed",
    };

    LogInfo { style: log_level, job_info: job_message, message }
}

pub type Connector = Box<dyn Connect<Proxy = fproc::LauncherProxy> + Send + Sync>;

/// A protocol connector for `fuchsia.process.Launcher` that serves the protocol with the
/// `ProcessLauncher` implementation.
#[cfg(test)]
pub struct BuiltInConnector {}

#[cfg(test)]
impl Connect for BuiltInConnector {
    type Proxy = fproc::LauncherProxy;

    fn connect(&self) -> Result<Self::Proxy, anyhow::Error> {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fproc::LauncherMarker>();
        fuchsia_async::Task::spawn(async move {
            let result = ProcessLauncher::serve(stream).await;
            if let Err(error) = result {
                log::warn!(error:%; "ProcessLauncher.serve failed");
            }
        })
        .detach();
        Ok(proxy)
    }
}

/// A protocol connector for `fuchsia.process.Launcher` that connects to the protocol from a
/// namespace object.
pub struct NamespaceConnector {
    pub namespace: Arc<namespace::Namespace>,
}

#[derive(Error, Debug)]
enum NamespaceConnectorError {
    #[error("Missing /svc in namespace: {0:?}")]
    MissingSvcInNamespace(Vec<NamespacePath>),
}

impl Connect for NamespaceConnector {
    type Proxy = fproc::LauncherProxy;

    fn connect(&self) -> Result<Self::Proxy, anyhow::Error> {
        static PATH: LazyLock<NamespacePath> = LazyLock::new(|| "/svc".parse().unwrap());
        let svc = self.namespace.get(&PATH).ok_or_else(|| {
            NamespaceConnectorError::MissingSvcInNamespace(self.namespace.paths())
        })?;
        fuchsia_component::client::connect_to_protocol_at_dir_root::<fproc::LauncherMarker>(svc)
            .context("failed to connect to external launcher service")
    }
}

#[cfg(test)]
mod tests {
    use fuchsia_runtime::job_default;
    use zx::Task;

    use super::*;

    #[test]
    fn describe_expected_error_in_killed_job() {
        // Create a child job then kill it.
        let job = job_default().create_child_job().unwrap();
        job.kill().unwrap();
        let errors = [
            LauncherError::BuilderError(ProcessBuilderError::CreateProcess(zx::Status::BAD_STATE)),
            LauncherError::BuilderError(ProcessBuilderError::LoadDynamicLinkerTimeout()),
        ];
        for err in &errors {
            let description = describe_error(err, job.as_handle_ref().cast());
            assert_eq!(
                description,
                LogInfo {
                    style: LogStyle::JobKilled,
                    job_info: "job was killed (retcode -1024)".to_string(),
                    message: "Process operation failed because the parent job was killed. This is expected."
                }
            );
        }
    }

    #[test]
    fn describe_unexpected_error_in_killed_job() {
        // Create a child job then kill it.
        let job = job_default().create_child_job().unwrap();
        job.kill().unwrap();
        let expected_err = LauncherError::BuilderError(ProcessBuilderError::CreateProcess(
            zx::Status::ACCESS_DENIED,
        ));
        let description = describe_error(&expected_err, job.as_handle_ref().cast());
        assert_eq!(
            description,
            LogInfo {
                style: LogStyle::Error,
                job_info: "".to_string(),
                message: "Process operation failed"
            }
        );
    }

    #[test]
    fn describe_error_in_running_job() {
        // Use our own job which must be running.
        let job = job_default();
        let err =
            LauncherError::BuilderError(ProcessBuilderError::CreateProcess(zx::Status::BAD_STATE));
        let description = describe_error(&err, job.clone());
        assert_eq!(
            description,
            LogInfo {
                style: LogStyle::Error,
                job_info: "job is running".to_string(),
                message: "Process operation failed"
            }
        );
    }
}

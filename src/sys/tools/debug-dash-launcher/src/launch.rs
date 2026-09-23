// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{layout, trampoline};
use fidl::endpoints::{ClientEnd, Proxy};
use fidl_fuchsia_dash::LauncherError;
use fidl_fuchsia_hardware_pty as pty;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_process as fproc;
use fuchsia_component::client::connect_to_protocol;
use fuchsia_runtime::{HandleInfo as HandleId, HandleType};

pub mod component;
pub mod package;

// -s: force input from stdin
// -i: force interactive
const DASH_ARGS_FOR_INTERACTIVE: [&[u8]; 2] = ["-i".as_bytes(), "-s".as_bytes()];
// TODO(https://fxbug.dev/42055812): Verbose (-v) or write-commands-to-stderr (-x) is required if a command is
// given, else it errors with `Can't open <cmd>`. -c: execute command
const DASH_ARGS_FOR_COMMAND: [&[u8]; 2] = ["-v".as_bytes(), "-c".as_bytes()];

pub struct ExploreArgs<'a> {
    pub stdin: zx::NullableHandle,
    pub stdout: zx::NullableHandle,
    pub stderr: zx::NullableHandle,
    pub tool_urls: Vec<String>,
    pub command: Option<String>,
    pub name_infos: Vec<fproc::NameInfo>,
    pub process_name: String,
    pub package_resolver: &'a mut crate::package_resolver::PackageResolver,
    pub moniker: Option<String>,
}

async fn explore_over_handles(
    args: ExploreArgs<'_>,
) -> Result<(zx::Process, zx::Job), LauncherError> {
    let ExploreArgs {
        stdin,
        stdout,
        stderr,
        tool_urls,
        command,
        mut name_infos,
        process_name,
        package_resolver,
        moniker,
    } = args;
    // recreate a PTY device so we can log to it if there's an error
    let pty_proxy =
        fidl::endpoints::ClientEnd::<fidl_fuchsia_hardware_pty::DeviceMarker>::new(stdout.into())
            .into_proxy();

    let (tools_pkg_dir, tools_path) = trampoline::create_trampolines_from_packages(
        package_resolver,
        tool_urls,
        &pty_proxy,
        moniker,
    )
    .await?;
    layout::add_tools_to_name_infos(tools_pkg_dir, &mut name_infos);

    let internal_pkg_dir =
        fuchsia_fs::directory::open_in_namespace("/pkg", fio::PERM_READABLE | fio::PERM_EXECUTABLE)
            .map_err(|_| LauncherError::Internal)?;
    name_infos.push(fproc::NameInfo {
        path: "/.dash/internal".to_string(),
        directory: internal_pkg_dir.into_channel().unwrap().into_zx_channel().into(),
    });

    // The dash-launcher can be asked to launch multiple dash processes, each of which can make
    // their own process hierarchies. This will look better topologically if we make a child job for
    // each dash process.
    let job =
        fuchsia_runtime::job_default().create_child_job().map_err(|_| LauncherError::Internal)?;

    // dehydrate our pty back into a handle so we can pass it to the dash process
    let stdout_recreated = pty_proxy.into_channel().unwrap().into_zx_channel().into_handle();

    // Add handles for the current job, stdio, library loader and UTC time.
    let handle_infos = create_handle_infos(&job, stdin, stdout_recreated, stderr)?;

    let launcher = connect_to_protocol::<fproc::LauncherMarker>()
        .map_err(|_| LauncherError::ProcessLauncher)?;

    let mut args = Vec::new();
    args.push(b"/.dash/internal/bin/sh".to_vec());
    if let Some(cmd) = command {
        args.extend(DASH_ARGS_FOR_COMMAND.iter().map(|b| b.to_vec()));
        args.push(cmd.into_bytes());
    } else {
        args.extend(DASH_ARGS_FOR_INTERACTIVE.iter().map(|b| b.to_vec()));
    };

    // Spawn the dash process.
    let info = create_launch_info(process_name, &job).await?;
    launcher.add_names(name_infos).map_err(|_| LauncherError::ProcessLauncher)?;
    launcher.add_handles(handle_infos).map_err(|_| LauncherError::ProcessLauncher)?;
    launcher.add_args(&args).map_err(|_| LauncherError::ProcessLauncher)?;
    let path_envvar = trampoline::create_env_path(tools_path);
    let env_vars = &[path_envvar.into_bytes()];
    launcher.add_environs(env_vars).map_err(|_| LauncherError::ProcessLauncher)?;
    let (status, process) =
        launcher.launch(info).await.map_err(|_| LauncherError::ProcessLauncher)?;
    zx::Status::ok(status).map_err(|_| LauncherError::ProcessLauncher)?;
    let process = process.ok_or(LauncherError::ProcessLauncher)?;

    Ok((process, job))
}

fn split_pty_into_handles(
    pty: ClientEnd<pty::DeviceMarker>,
) -> Result<(zx::NullableHandle, zx::NullableHandle, zx::NullableHandle), LauncherError> {
    let pty = pty.into_proxy();

    // Split the PTY into 3 channels (stdin, stdout, stderr).
    let (stdout, to_pty_stdout) = fidl::endpoints::create_endpoints::<pty::DeviceMarker>();
    let (stderr, to_pty_stderr) = fidl::endpoints::create_endpoints::<pty::DeviceMarker>();
    let to_pty_stdout = to_pty_stdout.into_channel().into();
    let to_pty_stderr = to_pty_stderr.into_channel().into();

    // Clone the PTY to also be used for stdout and stderr.
    pty.clone(to_pty_stdout).map_err(|_| LauncherError::Pty)?;
    pty.clone(to_pty_stderr).map_err(|_| LauncherError::Pty)?;

    let stdin = pty.into_channel().unwrap().into_zx_channel().into_handle();

    Ok((stdin, stdout.into(), stderr.into()))
}

fn create_handle_infos(
    job: &zx::Job,
    stdin: zx::NullableHandle,
    stdout: zx::NullableHandle,
    stderr: zx::NullableHandle,
) -> Result<Vec<fproc::HandleInfo>, LauncherError> {
    let stdin_handle = fproc::HandleInfo {
        handle: stdin,
        id: HandleId::new(HandleType::FileDescriptor, 0).as_raw(),
    };

    let stdout_handle = fproc::HandleInfo {
        handle: stdout,
        id: HandleId::new(HandleType::FileDescriptor, 1).as_raw(),
    };

    let stderr_handle = fproc::HandleInfo {
        handle: stderr,
        id: HandleId::new(HandleType::FileDescriptor, 2).as_raw(),
    };

    let job_dup =
        job.duplicate_handle(zx::Rights::SAME_RIGHTS).map_err(|_| LauncherError::Internal)?;
    let job_handle = fproc::HandleInfo {
        handle: zx::NullableHandle::from(job_dup),
        id: HandleId::new(HandleType::DefaultJob, 0).as_raw(),
    };

    let ldsvc = fuchsia_runtime::loader_svc().map_err(|_| LauncherError::Internal)?;
    let ldsvc_handle =
        fproc::HandleInfo { handle: ldsvc, id: HandleId::new(HandleType::LdsvcLoader, 0).as_raw() };

    let utc_clock = {
        let utc_clock = fuchsia_runtime::duplicate_utc_clock_handle(zx::Rights::SAME_RIGHTS)
            .map_err(|_| LauncherError::Internal)?;
        utc_clock.into_handle()
    };
    let utc_clock_handle = fproc::HandleInfo {
        handle: utc_clock,
        id: HandleId::new(HandleType::ClockUtc, 0).as_raw(),
    };

    Ok(vec![stdin_handle, stdout_handle, stderr_handle, job_handle, ldsvc_handle, utc_clock_handle])
}

async fn create_launch_info(
    process_name: String,
    job: &zx::Job,
) -> Result<fproc::LaunchInfo, LauncherError> {
    // Load `/pkg/bin/sh` as an executable VMO and pass it to the Launcher.
    let dash_file = fuchsia_fs::file::open_in_namespace(
        "/pkg/bin/sh",
        fuchsia_fs::PERM_EXECUTABLE | fuchsia_fs::PERM_READABLE,
    )
    .map_err(|_| LauncherError::DashBinary)?;

    let executable = dash_file
        .get_backing_memory(
            fio::VmoFlags::READ | fio::VmoFlags::EXECUTE | fio::VmoFlags::PRIVATE_CLONE,
        )
        .await
        .map_err(|_| LauncherError::DashBinary)?
        .map_err(|_| LauncherError::DashBinary)?;

    let job_dup =
        job.duplicate_handle(zx::Rights::SAME_RIGHTS).map_err(|_| LauncherError::Internal)?;

    let name = truncate_str(&process_name, zx::sys::ZX_MAX_NAME_LEN).to_owned();

    Ok(fproc::LaunchInfo { name, job: job_dup, executable })
}

/// Truncates `s` to be at most `max_len` bytes.
fn truncate_str(s: &str, max_len: usize) -> &str {
    let index = s.floor_char_boundary(max_len);
    &s[..index]
}

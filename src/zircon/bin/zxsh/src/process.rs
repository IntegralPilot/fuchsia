// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::errors::zx_status_str;
use crate::eval::ShellEnv;
use crate::fd::Fd;
use bstr::{BStr, BString, ByteSlice};
use std::fs::File;
use std::io::Read;
use std::os::fd::AsFd;

pub use crate::string::{bstr_to_cstring, bstrings_to_cstrings, cstrings_to_c_strs};

/// Spawns a new OS process given binary name, command arguments, environment variable state, and FD actions.
pub fn spawn_command_with_path(
    binary_name: &BStr,
    argv: &[BString],
    env: &ShellEnv,
    actions: &mut [fdio::SpawnAction<'_>],
) -> Result<zx::Process, zx::Status> {
    if argv.is_empty() {
        return Err(zx::Status::INVALID_ARGS);
    }

    let binary_path = env.path().resolve(binary_name).unwrap_or_else(|| BString::from(binary_name));
    let path_cstr =
        bstr_to_cstring(binary_path.as_slice()).map_err(|_| zx::Status::INVALID_ARGS)?;

    let argv_cstrs = bstrings_to_cstrings(argv).map_err(|_| zx::Status::INVALID_ARGS)?;
    let argv_ptrs = cstrings_to_c_strs(&argv_cstrs);

    let env_cstrs = env.to_spawn_env().map_err(|_| zx::Status::INVALID_ARGS)?;
    let env_ptrs = cstrings_to_c_strs(&env_cstrs);

    let job = fuchsia_runtime::job_default();
    let options = fdio::SpawnOptions::CLONE_ALL & !fdio::SpawnOptions::CLONE_ENVIRONMENT;

    fdio::spawn_etc(&job, options, &path_cstr, &argv_ptrs, Some(&env_ptrs), actions).map_err(
        |(status, err_msg)| {
            eprintln!("fdio::spawn_etc failed: {} (status={})", err_msg, zx_status_str(status));
            status
        },
    )
}

/// Spawns a new OS process given command arguments, environment variable state, and FD actions.
///
/// Resolves `argv[0]` against the `PATH` environment variable in `env` (or uses it directly
/// if it contains a slash). Clones namespace, environment, and job by default.
pub fn spawn_command(
    argv: &[BString],
    env: &ShellEnv,
    actions: &mut [fdio::SpawnAction<'_>],
) -> Result<zx::Process, zx::Status> {
    if argv.is_empty() {
        return Err(zx::Status::INVALID_ARGS);
    }
    spawn_command_with_path(argv[0].as_bstr(), argv, env, actions)
}

/// Creates a unidirectional inter-process communication pipe (`(read_file, write_file)`).
///
/// Wraps Fuchsia `fdio::pipe_half` and converts the underlying socket handle into standard file
/// descriptors.
pub fn make_pipe() -> Result<(File, File), String> {
    // The files make by `pipe_half` differ substantially from a POSIX pipe. For example, the
    // transport is actually bidirectional. However, we have been using this approach in dash and
    // it's likely that this approach will work fine for us here as well.
    let (file, socket) =
        fdio::pipe_half().map_err(|e| format!("pipe_half failed: {}", zx_status_str(e)))?;
    let read_fd = fdio::create_fd(socket.into())
        .map_err(|e| format!("create_fd failed: {}", zx_status_str(e)))?;
    Ok((File::from(read_fd), file))
}

/// Reads all data from the given file descriptor until EOF into a byte vector.
pub fn read_fd_to_end(mut file: File) -> Result<Vec<u8>, std::io::Error> {
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Clones an open file descriptor into an `fdio::SpawnAction` mapped to `target_fd` in a child
/// process.
///
/// Returns `None` if cloning fails or if the underlying handle does not support duplication.
pub fn clone_fd_to_action(fd: &impl AsFd, target_fd: Fd) -> Option<fdio::SpawnAction<'static>> {
    match fdio::clone_fd(fd) {
        Ok(handle) => {
            assert!(!handle.is_invalid(), "fdio::clone_fd returned an invalid handle on Ok");
            let handle_info = fuchsia_runtime::HandleInfo::new(
                fuchsia_runtime::HandleType::FileDescriptor,
                target_fd.raw() as u16,
            );
            Some(fdio::SpawnAction::add_handle(handle_info, handle))
        }
        Err(zx::Status::NOT_SUPPORTED) | Err(zx::Status::INVALID_ARGS) => None,
        Err(status) => {
            eprintln!("Warning: failed to clone fd {} for spawning: {:?}", target_fd, status);
            None
        }
    }
}

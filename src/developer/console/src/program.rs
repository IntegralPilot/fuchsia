// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::ClientEnd;
use fidl_fuchsia_io as fio;
use thiserror::Error;

// We load dash from /boot/bin because dash is going to use the loader service
// it has access to to launch other binaries that exist in /boot/bin (through
// FDIO), so we need to launch it with a correctly scoped loader to match other
// binaries there, which matches the historical pattern within SSH and serial
// console.
//
// We can lift this and package dash with developer-console when there no longer
// is a need to launch shell tools with a correctly scoped loader to /boot/lib,
// and dash is no longer in /boot/bin.
const DEFAULT_SHELL_PATH: &str = "/boot/bin/sh";
const DEFAULT_LIBRARY_PATH: &str = "/boot/lib";
const DEFAULT_SHELL_ENV_PATH: &str = "PATH=/boot/bin:/boot-bin:/bin:/.tools";

pub enum Program {
    DefaultShell,
    Package { path: String, package: fio::DirectoryProxy },
}

impl Program {
    pub fn amend_args(&self, args: Vec<String>) -> Vec<String> {
        match self {
            Program::DefaultShell => {
                [
                    DEFAULT_SHELL_PATH,
                    if args.is_empty() {
                        // Help out forcing interactive mode so that we don't
                        // have dash command line load bearing across the API
                        // boundary.
                        "-i"
                    } else {
                        "-c"
                    },
                ]
                .into_iter()
                .map(|s| s.to_string())
                .chain(args)
                .collect()
            }
            Program::Package { path, .. } => {
                [format!("/pkg/{path}")].into_iter().chain(args).collect()
            }
        }
    }

    pub fn amend_env(&self, env: Vec<String>) -> Vec<String> {
        match self {
            Program::DefaultShell => {
                [DEFAULT_SHELL_ENV_PATH].into_iter().map(|s| s.to_string()).chain(env).collect()
            }
            Program::Package { .. } => env,
        }
    }

    pub async fn to_info(&self) -> Result<ProgramInfo, ProgramError> {
        let (file, lib) = match self {
            Program::DefaultShell => (
                fuchsia_fs::file::open_in_namespace(
                    DEFAULT_SHELL_PATH,
                    fuchsia_fs::PERM_EXECUTABLE | fuchsia_fs::PERM_READABLE,
                )
                .map_err(ProgramError::OpenProgram)?,
                fuchsia_fs::directory::open_in_namespace(
                    DEFAULT_LIBRARY_PATH,
                    fio::PERM_READABLE | fio::PERM_EXECUTABLE,
                )
                .map_err(ProgramError::OpenLib)?,
            ),
            Program::Package { path, package } => (
                fuchsia_fs::directory::open_file(
                    package,
                    path,
                    fuchsia_fs::PERM_EXECUTABLE | fuchsia_fs::PERM_READABLE,
                )
                .await
                .map_err(ProgramError::OpenProgram)?,
                fuchsia_fs::directory::open_directory(
                    package,
                    "lib",
                    fio::PERM_READABLE | fio::PERM_EXECUTABLE,
                )
                .await
                .map_err(ProgramError::OpenLib)?,
            ),
        };
        let (ll_client_chan, ll_service_chan) = zx::Channel::create();
        library_loader::start(lib, ll_service_chan);
        let loader = ll_client_chan.into();

        let vmo = file
            .get_backing_memory(
                fio::VmoFlags::READ | fio::VmoFlags::EXECUTE | fio::VmoFlags::PRIVATE_CLONE,
            )
            .await?
            .map_err(|e| ProgramError::GettingVmo(zx::Status::err_from_raw(e)))?;

        Ok(ProgramInfo { vmo, loader })
    }

    pub fn package(&self) -> Result<Option<ClientEnd<fio::DirectoryMarker>>, ProgramError> {
        match self {
            Program::DefaultShell => Ok(None),
            Program::Package { path: _, package } => {
                let (client, server) = fidl::endpoints::create_endpoints();
                fuchsia_fs::directory::clone_onto(package, server)
                    .map_err(ProgramError::ClonePackage)?;
                Ok(Some(client))
            }
        }
    }
}

pub struct ProgramInfo {
    pub vmo: zx::Vmo,
    pub loader: zx::NullableHandle,
}

#[derive(Error, Debug)]
pub enum ProgramError {
    #[error("failed to open program file: {0}")]
    OpenProgram(fuchsia_fs::node::OpenError),
    #[error("failed to open libraries for loader lib: {0}")]
    OpenLib(fuchsia_fs::node::OpenError),
    #[error("unexpected FIDL error: {0}")]
    Fidl(#[from] fidl::Error),
    #[error("failed to get program VMO: {0}")]
    GettingVmo(zx::Status),
    #[error("failed to clone package dir: {0}")]
    ClonePackage(fuchsia_fs::node::CloneError),
}

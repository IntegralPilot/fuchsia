// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_component::server::{ServiceFs, ServiceFsDir};
use futures::{StreamExt as _, TryFutureExt as _};
use log::{debug, error, warn};

use {fidl_fuchsia_developer_console as fconsole, fuchsia_async as fasync};

use crate::error::{Error, MissingFidlFieldError};
use crate::io::{IoHandles, IoHandlesError};
use crate::namespace::{Namespace, NamespaceError};
use crate::process::{LaunchedProcess, Process, ProcessError};
use crate::program::{ProgramError, ProgramInfo};

mod error;
mod io;
mod namespace;
mod process;
mod program;
mod util;

use program::Program;

#[fuchsia::main(logging_tags=["developer-console"])]
async fn main() {
    let mut service_fs = ServiceFs::new_local();

    let _: &mut ServiceFsDir<'_, _> =
        service_fs.dir("svc").add_fidl_service(|s: fconsole::LauncherRequestStream| s);

    let _: &mut ServiceFs<_> =
        service_fs.take_and_serve_directory_handle().expect("failed to serve outgoing namespace");
    debug!("started");

    let mut requests = service_fs.flatten_unordered(None);

    loop {
        let req = match requests.next().await {
            None => break,
            Some(Ok(req)) => req,
            Some(Err(e)) => {
                error!("error serving FIDL request: {e:?}");
                continue;
            }
        };

        match req {
            fconsole::LauncherRequest::Launch { payload, responder } => {
                let _join_handle: fasync::JoinHandle<()> =
                    fasync::Scope::current().spawn(handle_launch(payload, responder));
            }
            fconsole::LauncherRequest::_UnknownMethod { ordinal, .. } => {
                warn!("received unknown method {ordinal} ");
            }
        }
    }
}

async fn handle_launch(
    options: fconsole::LaunchOptions,
    responder: fconsole::LauncherLaunchResponder,
) {
    // Given every request potentially holds multiple tasks. Create a new
    // scope per launch request.
    let scope = fasync::Scope::new_with_name("launch");
    scope
        .spawn(async move {
            let result = handle_launch_inner(options)
                .and_then(|process| process.wait().map_err(Error::from))
                .await
                .map_err(|e| {
                    warn!("launch failed: {e}");
                    error_to_fidl(e)
                });

            responder.send(result).unwrap_or_else(|e| {
                if !e.is_closed() {
                    error!("failed to send launch response: {e}")
                }
            })
        })
        .await;
    // Once the main task is done, cancel the scope in case we have straggler
    // tasks.
    scope.cancel().await
}

async fn handle_launch_inner(options: fconsole::LaunchOptions) -> Result<LaunchedProcess, Error> {
    let fconsole::LaunchOptions {
        name,
        args,
        program,
        io_handles,
        env,
        namespace_entries,
        stopper,
        directories_fixup,
        __source_breaking,
    } = options;
    let program = match program {
        None | Some(fconsole::Program::DefaultShell(fconsole::Empty {})) => Program::DefaultShell,
        Some(fconsole::Program::FromPackage(fconsole::PackageProgram { package, path })) => {
            let directory = package
                .directory
                .ok_or(MissingFidlFieldError("fuchsia.component.resolution.Package.directory"))?;
            Program::Package { path, package: directory.into_proxy() }
        }
        Some(fconsole::Program::__SourceBreaking { unknown_ordinal }) => {
            return Err(Error::UnknownInteraction { name: "Program", unknown_ordinal });
        }
    };
    let name = name.as_deref().unwrap_or(match &program {
        Program::DefaultShell => "sh",
        Program::Package { path, package: _ } => path.as_str(),
    });
    let name = zx::Name::new_lossy(name);
    let args = program.amend_args(args.unwrap_or_default());
    let env = program.amend_env(env.unwrap_or_default());

    let ProgramInfo { vmo, loader } = program.to_info().await?;
    let namespace = Namespace {
        overrides: namespace_entries.unwrap_or_default(),
        directories_fixup: directories_fixup.unwrap_or(true),
        pkg: program.package()?,
    }
    .build()
    .await?;

    let io_handles = match io_handles {
        None => IoHandles::default(),
        Some(fconsole::IoHandles::RawHandles(raw)) => {
            let fconsole::RawHandles { stdin, stdout, stderr } = raw;
            IoHandles { stdin, stdout, stderr }
        }
        Some(fconsole::IoHandles::PtySocket(socket)) => {
            IoHandles::new_pty_from_socket(socket).await?
        }
        Some(fconsole::IoHandles::__SourceBreaking { unknown_ordinal }) => {
            return Err(Error::UnknownInteraction { name: "IoHandles", unknown_ordinal });
        }
    }
    .into_handle_infos();

    Ok(Process { name, vmo, loader, io_handles, args, env, namespace, stopper }.launch().await?)
}

fn error_to_fidl(error: Error) -> fconsole::LauncherError {
    use fconsole::LauncherError as F;
    match error {
        Error::UnknownInteraction { .. } => F::NotSupported,
        Error::MissingFidlField(_) | Error::Fidl(_) => F::Internal,
        Error::Namespace(e) => namespace_error_to_fidl(e),
        Error::Program(e) => program_error_to_fidl(e),
        Error::Process(e) => process_error_to_fidl(e),
        Error::IoHandles(e) => io_error_to_fidl(e),
    }
}

fn namespace_error_to_fidl(error: NamespaceError) -> fconsole::LauncherError {
    use fconsole::LauncherError as F;
    match error {
        NamespaceError::Proto(_)
        | NamespaceError::Fidl(_)
        | NamespaceError::MissingFidlField(_)
        | NamespaceError::DirectoryWatcherCreate(_)
        | NamespaceError::DirectoryWatcherStream(_)
        | NamespaceError::DirectoryOpen { .. }
        | NamespaceError::ConstructNamespace(_) => F::Internal,
        NamespaceError::InvalidPath(_) => F::InvalidNamespacePath,
        NamespaceError::DuplicatePath(_) => F::DuplicateNamespacePath,
    }
}

fn program_error_to_fidl(error: ProgramError) -> fconsole::LauncherError {
    use fconsole::LauncherError as F;
    match error {
        ProgramError::OpenProgram(_)
        | ProgramError::GettingVmo(_)
        | ProgramError::OpenLib(_)
        | ProgramError::ClonePackage(_) => F::ProgramLoadFailed,
        ProgramError::Fidl(_) => F::Internal,
    }
}

fn process_error_to_fidl(error: ProcessError) -> fconsole::LauncherError {
    use fconsole::LauncherError as F;
    match error {
        ProcessError::ConnectToProtocol(_)
        | ProcessError::Job(_)
        | ProcessError::Utc(_)
        | ProcessError::Fidl(_)
        | ProcessError::KillJobOnStop(_)
        | ProcessError::WaitingTermination(_) => F::Internal,
        ProcessError::LaunchingProcess(_) => F::ProcessLaunchFailed,
    }
}

fn io_error_to_fidl(error: IoHandlesError) -> fconsole::LauncherError {
    use fconsole::LauncherError as F;
    match error {
        IoHandlesError::ConnectToProtocol(_)
        | IoHandlesError::MissingFidlField(_)
        | IoHandlesError::Fidl(_)
        | IoHandlesError::ScopeCancelled => F::Internal,
        IoHandlesError::Pty(_) => F::PtyFailed,
    }
}

// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::common::*;
use argh::{ArgsInfo, FromArgs};
use blocking::Unblock;
use fdomain_client::fidl::Proxy;
use fdomain_fuchsia_developer_remotecontrol as rc;
use fdomain_fuchsia_starnix_container as fstarcontainer;
use fho::{FfxContext, Result};
use fuchsia_async as fasync;
use futures::future::FutureExt;
use futures::join;
use nix::unistd::dup;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use termion::raw::IntoRawMode;

fn forward_stdin(console_in: fdomain_client::Socket) -> Result<()> {
    let mut tx = console_in;

    // We spin off a separate thread to copy data from stdin into this console.
    //
    // We never wait for this thread to complete because we're happy to copy data from stdin into
    // this socket until the process exits.
    let _ = std::thread::spawn(|| {
        let mut executor = fasync::LocalExecutor::default();
        executor.run_singlethreaded(async move {
            let _ = futures::io::copy(Unblock::new(std::io::stdin()), &mut tx).await;
        });
    });

    Ok(())
}

async fn forward_stdout(console_out: fdomain_client::Socket) -> Result<()> {
    let rx = console_out;

    // We spin off a separate thread to copy data from this console to stdout.
    //
    // We wait for this thread to complete using fasync::unblock.
    fasync::unblock(|| {
        let mut executor = fasync::LocalExecutor::default();
        executor.run_singlethreaded(async move {
            // We make a duplicate of stdout so that fs::File can take ownership of the FD.
            let duplicate_stdout = dup(std::io::stdout()).expect("failed to duplicate stdout");
            let sink = std::fs::File::from(duplicate_stdout);

            // Actually copy the data.
            let _ = futures::io::copy(rx, &mut Unblock::new(sink)).await;
        });
    })
    .await;

    Ok(())
}

async fn forward_console(
    console_in: fdomain_client::Socket,
    console_out: fdomain_client::Socket,
) -> Result<()> {
    forward_stdin(console_in)?;
    forward_stdout(console_out).await
}

fn get_environ() -> Vec<String> {
    let mut result = vec![];
    for key in ["TERM"] {
        if let Ok(value) = std::env::var(key) {
            result.push(format!("{key}={value}"));
        }
    }
    result
}

async fn run_console(
    controller: &fstarcontainer::ControllerProxy,
    argv: Vec<String>,
    env: Vec<String>,
) -> Result<u8> {
    let (local_console_in, remote_console_in) = controller.domain().create_stream_socket();
    let (local_console_out, remote_console_out) = controller.domain().create_stream_socket();
    let binary_path = argv[0].clone();
    let (cols, rows) = termion::terminal_size().bug_context("getting terminal size")?;
    let (x_pixels, y_pixels) = (0, 0); // TODO: Need a newer termion for `terminal_size_pixels()`.
    let exit_future = controller
        .spawn_console(fstarcontainer::ControllerSpawnConsoleRequest {
            console_in: Some(remote_console_in),
            console_out: Some(remote_console_out),
            binary_path: Some(binary_path),
            argv: Some(argv),
            environ: Some(env),
            window_size: Some(fstarcontainer::ConsoleWindowSize { rows, cols, x_pixels, y_pixels }),
            ..Default::default()
        })
        .fuse();

    let forward_future = forward_console(local_console_in, local_console_out);

    let raw_mode = std::io::stdout().into_raw_mode().unwrap();
    let (_, exit_result) = join!(forward_future, exit_future);
    std::mem::drop(raw_mode);
    exit_result.bug_context("fidl transport error")?.map_err(|e| {
        fho::user_error!(
            "Failed to spawn console: {e:?}.\n\
             Verify that the console binary exists at the specified path in the container."
        )
    })
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "console",
    example = "ffx starnix console [-e ENV=VAL -e ...] program [argument ...]",
    description = "Attach a console to a starnix container"
)]
pub struct StarnixConsoleCommand {
    /// the moniker of the container in which to create the console
    /// (defaults to looking for a container in the current session)
    #[argh(option, short = 'm')]
    pub moniker: Option<String>,

    /// environment variables to pass to the program.
    #[argh(option, short = 'e')]
    env: Vec<String>,

    /// full path to the program to run in the console and its arguments.
    #[argh(positional, greedy)]
    argv: Vec<String>,
}

pub async fn starnix_console(
    StarnixConsoleCommand { moniker, mut env, argv }: StarnixConsoleCommand,
    rcs_proxy: &rc::RemoteControlProxy,
) -> Result<ConsoleCommandOutput> {
    if !termion::is_tty(&std::io::stdout()) {
        fho::return_user_error!(
            "ffx starnix console must be run in a tty. \
If you are attempting to use this command in a automated \
test, please be aware that this command is intended only \
for interactive use. Please do not use this command in an \
automated test."
        );
    }
    if argv.is_empty() {
        fho::return_user_error!(
            "Please specify a program to run.\n\
               Examples:\n\
               ffx starnix console /bin/bash\n\
               ffx starnix console /bin/ls -l /\n\
               Use ffx starnix console --help for more information."
        );
    }
    let controller = connect_to_controller(&rcs_proxy, moniker).await?;

    env.append(&mut get_environ());

    let exit_code = run_console(&controller, argv, env).await?;
    Ok(ConsoleCommandOutput { exit_code })
}

#[derive(Debug, JsonSchema, Deserialize, Serialize)]
pub struct ConsoleCommandOutput {
    exit_code: u8,
}

impl std::fmt::Display for ConsoleCommandOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "(exit code: {})", self.exit_code)
    }
}

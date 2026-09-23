// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, anyhow};
use fidl_fuchsia_dash as fdash;
use fuchsia_component::client::connect_to_protocol;
use futures::stream::StreamExt as _;

mod args;

pub async fn exec() -> anyhow::Result<()> {
    let args: args::PackageArgs = argh::from_env();

    match args.subcommand {
        crate::args::PackageSubcommand::Explore(args) => {
            // TODO(https://fxbug.dev/296283299): Verify that the optional Launcher protocol is
            // available before connecting.
            let dash_launcher = connect_to_protocol::<fdash::LauncherMarker>()?;
            // TODO(https://fxbug.dev/42077838): Use Stdout::raw when a command is not provided.
            let stdout = socket_to_stdio::Stdout::buffered();

            explore_cmd(args, dash_launcher, stdout).await
        }
    }
}

async fn explore_cmd(
    args: crate::args::ExploreArgs,
    dash_launcher: fdash::LauncherProxy,
    stdout: socket_to_stdio::Stdout<'_>,
) -> anyhow::Result<()> {
    let crate::args::ExploreArgs { url, subpackages, tools, command, fuchsia_pkg_resolver } = args;
    let (client, server) = fidl::Socket::create_stream();
    let () = dash_launcher
        .explore_package_over_socket2(
            fuchsia_pkg_resolver,
            &url,
            &subpackages,
            server,
            &tools,
            command.as_deref(),
        )
        .await
        .context("fuchsia.dash/Launcher.ExplorePackageOverSocket2 fidl error")?
        .map_err(|e| match e {
            fdash::LauncherError::ResolveTargetPackage => {
                anyhow!("No package found matching '{url}' {}.", subpackages.join(" "))
            }
            e => anyhow!("Error exploring package: {e:?}"),
        })?;

    let () = socket_to_stdio::connect_socket_to_stdio(client, stdout).await?;

    let exit_code = wait_for_shell_exit(&dash_launcher).await?;
    std::process::exit(exit_code);
}

async fn wait_for_shell_exit(launcher_proxy: &fdash::LauncherProxy) -> anyhow::Result<i32> {
    match launcher_proxy.take_event_stream().next().await {
        Some(Ok(fdash::LauncherEvent::OnTerminated { return_code })) => Ok(return_code),
        Some(Err(e)) => Err(anyhow!("OnTerminated event error: {e:?}")),
        None => Err(anyhow!("didn't receive an expected OnTerminated event")),
    }
}

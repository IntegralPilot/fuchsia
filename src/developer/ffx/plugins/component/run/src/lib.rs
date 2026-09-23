// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Result};
use async_trait::async_trait;
use component_debug_fdomain::cli::doctor::write_result_table;
use component_debug_fdomain::cli::run_cmd;
use component_debug_fdomain::config::resolve_raw_config_overrides;
use component_debug_fdomain::doctor::validate_routes;
use ffx_component::rcs::{
    connect_to_lifecycle_controller, connect_to_realm_query, connect_to_route_validator,
};
use ffx_component::server::{DefaultPackageServerRunner, maybe_start_server};
use ffx_component_run_args::RunComponentCommand;
use ffx_config::EnvironmentContext;
use ffx_core::macro_deps::errors::ffx_error;
use ffx_log::log_impl;
use ffx_log_args::LogCommand;
use ffx_writer::MachineWriter;
use fho::{FfxMain, FfxTool};
use futures::FutureExt;
use log_command_fdomain::{LogEntry, LogFilterArgs};
use std::io::Write;
use target_holders::RemoteControlProxyHolder;

async fn cmd_impl(
    ctx: &EnvironmentContext,
    rcs_proxy: RemoteControlProxyHolder,
    args: RunComponentCommand,
    mut writer: MachineWriter<LogEntry>,
    connector: target_connector::Connector<RemoteControlProxyHolder>,
) -> Result<(), anyhow::Error> {
    let process_dir: Option<std::path::PathBuf> = ctx.get("repository.process_dir").ok();
    let _guard = if let Some(process_dir) = process_dir {
        let runner = DefaultPackageServerRunner { process_dir };
        maybe_start_server(&runner, ctx.build_dir(), Some(&*rcs_proxy), Some(&args.url)).await?
    } else {
        eprintln!(
            "Warning: repository.process_dir not set in config. Skipping ephemeral package server start."
        );
        None
    };

    let rcs_proxy_clone = rcs_proxy.clone();
    let lifecycle_controller_factory = move || {
        let rcs_proxy_clone = rcs_proxy_clone.clone();
        async move { connect_to_lifecycle_controller(&rcs_proxy_clone).await }.boxed()
    };
    let realm_query = connect_to_realm_query(&rcs_proxy).await?;

    let config_overrides = resolve_raw_config_overrides(
        &realm_query,
        &args.moniker,
        &args.url.to_string(),
        &args.config,
    )
    .await
    .context("resolving config overrides")?;

    // All errors from component_debug library are user-visible.
    run_cmd(
        args.moniker.clone(),
        args.url,
        args.recreate,
        args.connect_stdio,
        config_overrides,
        lifecycle_controller_factory,
        &mut writer,
    )
    .await
    .map_err(|e| ffx_error!(e))?;

    // Run `doctor` on the new component to expose any routing problems.
    let route_validator = connect_to_route_validator(&rcs_proxy).await?;
    let mut route_report = validate_routes(&route_validator, &args.moniker.clone()).await?;
    // Broken routes are expected in case of transitional routes. Do not report those.
    route_report.retain(|r| match r.availability {
        Some(availability) => match availability {
            cm_rust::Availability::Transitional => false,
            cm_rust::Availability::Required
            | cm_rust::Availability::Optional
            | cm_rust::Availability::SameAsTarget => true,
        },
        None => true,
    });
    // If any of the RouteReport objects indicate an error, output the full report.
    if route_report.iter().any(|r| r.error_summary.is_some()) {
        write!(&mut writer, "\n\n")?;
        writeln!(
            &mut writer,
            "WARNING: your component may not run correctly due to some required capabilities not being available:\n"
        )?;
        write_result_table(&args.moniker, &route_report, &mut writer)?;
    }

    if args.follow_logs {
        let log_filter = args.moniker.to_string();
        let log_cmd = LogCommand {
            filters: LogFilterArgs { filter: vec![log_filter], ..Default::default() },
            ..LogCommand::default()
        };
        Box::pin(log_impl(writer, ctx, log_cmd, connector, true)).await?;
    }
    Ok(())
}

#[derive(FfxTool)]
pub struct RunTool {
    #[command]
    cmd: RunComponentCommand,
    rcs: RemoteControlProxyHolder,
    connector: target_connector::Connector<RemoteControlProxyHolder>,
    context: EnvironmentContext,
}

fho::embedded_plugin!(RunTool);

#[async_trait(?Send)]
impl FfxMain for RunTool {
    type Writer = MachineWriter<LogEntry>;

    type Error = ::fho::Error;

    async fn main(self, writer: Self::Writer) -> fho::Result<()> {
        Box::pin(cmd_impl(&self.context, self.rcs, self.cmd, writer, self.connector)).await?;
        Ok(())
    }
}

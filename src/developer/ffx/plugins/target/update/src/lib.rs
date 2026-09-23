// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A tool to update the target device.

use async_trait::async_trait;
use errors;
use fdomain_client::AsHandleRef;
use fdomain_client::fidl::Proxy as _;
use fdomain_fuchsia_buildinfo::ProviderProxy as BuildInfoProviderProxy;
use fdomain_fuchsia_update::{
    CheckOptions, CommitStatusProviderProxy, Initiator, ManagerProxy, MonitorMarker,
    MonitorRequest, MonitorRequestStream,
};
use fdomain_fuchsia_update_channel as fupdate_channel;
use fdomain_fuchsia_update_channelcontrol::ChannelControlProxy;
use fdomain_fuchsia_update_installer::{self as finstaller, InstallerProxy};
use ffx_config::EnvironmentContext;
use ffx_update_args as args;
use ffx_update_args::ForceInstall;
use ffx_writer::SimpleWriter;
use fho::{Deferred, FfxMain, FfxTool, bug, deferred};
use fidl::Signals;
use fidl_fuchsia_update_ext::State;
use fidl_fuchsia_update_installer_ext as installer;
use fuchsia_async::Timer;
use futures::future::{FusedFuture as _, FutureExt as _};
use futures::{StreamExt as _, TryStreamExt as _, pin_mut, select};
use pkg::PkgServerInstanceInfo as _;
use std::time::Duration;
use target_connector::Connector;
use target_holders::{HostAddrHolder, RemoteControlProxyHolder, TargetInfoQueryHolder, moniker};

mod server;

const WARNING_DURATION: Duration = Duration::from_secs(30);

use fho::FfxError;
use thiserror::Error;

#[derive(FfxError, Error, Debug)]
pub enum UpdateError {
    #[exit_with_code(1)]
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[exit_with_code(1)]
    #[error("Config error: {0}")]
    Config(#[from] ffx_config::api::ConfigError),

    #[exit_with_code(1)]
    #[error("FFX Writer error: {0}")]
    Writer(#[from] ffx_writer::Error),

    #[exit_with_code(1)]
    #[error("FIDL error: {0}")]
    Fidl(#[from] fidl::Error),

    #[exit_with_code(1)]
    #[error("FDomain client error: {0}")]
    FidlFDomain(#[from] fdomain_client::Error),

    #[exit_with_code(1)]
    #[error("Cannot specify a product bundle without the the `--product-bundle option.")]
    ProductBundleWithoutOption,

    #[exit_with_code(1)]
    #[error("Package server exited successfully, but prematurely")]
    PackageServerExitedPrematurely,

    #[exit_with_code(1)]
    #[error("Package server failed to run: {0}")]
    PackageServerRunFailed(#[source] fho::Error),

    #[exit_with_code(1)]
    #[error("Failed to resolve product bundle port: {0}")]
    ProductBundlePortResolution(#[source] anyhow::Error),

    #[exit_with_code(1)]
    #[error("Not started error on check-now: {0:?}")]
    CheckNowNotStarted(fdomain_fuchsia_update::CheckNotStartedReason),

    #[exit_with_code(1)]
    #[error("No package servers are running, could not determine the packageless update url")]
    NoPackageServerRunning,

    #[exit_with_code(1)]
    #[error("Tunnel required, cannot use existing package server, try --product-bundle")]
    TunnelRequired,

    #[exit_with_code(1)]
    #[error(
        "Multiple package servers are running, could not determine the packageless update url, please specify\n{0:#?}"
    )]
    MultiplePackageServersRunning(Vec<pkg::PkgServerInfo>),

    #[exit_with_code(1)]
    #[error(
        "No product bundle path specified nor configured. Run `ffx product-bundle get` or specify one with the appropriate flag."
    )]
    NoProductBundlePath,

    #[exit_with_code(1)]
    #[error(
        "Install failed prepare: {0:?}. Inspect target logs with `ffx log --filter system-updater` for details."
    )]
    InstallFailedPrepare(installer::PrepareFailureReason),

    #[exit_with_code(1)]
    #[error(
        "Install failed stage: {0:?}. Inspect target logs with `ffx log --filter system-updater` for details."
    )]
    InstallFailedStage(installer::StageFailureReason),

    #[exit_with_code(1)]
    #[error(
        "Install failed fetch: {0:?}. Inspect target logs with `ffx log --filter system-updater` for details."
    )]
    InstallFailedFetch(installer::FetchFailureReason),

    #[exit_with_code(1)]
    #[error("Install failed: canceled")]
    InstallCanceled,

    #[exit_with_code(1)]
    #[error("Internal error encountered during installation at {0:?} percent.")]
    InternalInstallation(Option<f32>),

    #[exit_with_code(1)]
    #[error("Update Manager encountered an error while checking for an update.")]
    UpdateCheckingFailed,

    #[transparent]
    #[error(transparent)]
    Fho(#[from] fho::Error),
}

#[derive(FfxTool)]
#[main_error(UpdateError)]
pub struct UpdateTool {
    #[command]
    cmd: args::Update,
    context: EnvironmentContext,
    #[with(deferred(moniker("/core/system-update")))]
    update_manager_proxy: Deferred<ManagerProxy>,
    #[with(deferred(moniker("/core/system-update")))]
    channel_provider_proxy: Deferred<fupdate_channel::ProviderProxy>,
    #[with(deferred(moniker("/core/system-update")))]
    channel_control_proxy: Deferred<ChannelControlProxy>,
    #[with(deferred(moniker("/core/system-update/system-updater")))]
    installer_proxy: Deferred<InstallerProxy>,
    #[with(deferred(moniker("/core/system-update")))]
    commit_status_provider_proxy: Deferred<CommitStatusProviderProxy>,
    #[with(deferred(moniker("/core/build-info")))]
    build_info_proxy: Deferred<BuildInfoProviderProxy>,
    target_spec: Deferred<TargetInfoQueryHolder>,
    rcs_proxy_connector: Connector<RemoteControlProxyHolder>,
    host_address: Deferred<HostAddrHolder>,
}

fho::embedded_plugin!(UpdateTool, UpdateError);

#[async_trait(?Send)]
impl FfxMain for UpdateTool {
    type Writer = SimpleWriter;
    type Error = UpdateError;

    /// Main entry point for the `update` subcommand.
    async fn main(self, mut writer: SimpleWriter) -> Result<(), Self::Error> {
        let update = self.cmd.clone();

        match update.cmd {
            args::Command::Channel(args::Channel { ref cmd }) => {
                handle_channel_control_cmd(
                    &cmd,
                    self.channel_provider_proxy,
                    self.channel_control_proxy,
                    &mut writer,
                )
                .await?;
            }
            args::Command::CheckNow(ref check_now) => {
                Box::pin(self.handle_check_now_cmd(check_now, &mut writer)).await?;
            }
            args::Command::ForceInstall(ref args) => {
                Box::pin(self.handle_force_install_cmd(args, &mut writer)).await?;
            }
            args::Command::WaitForCommit(_args) => {
                handle_wait_for_commit(
                    &self.commit_status_provider_proxy.await?,
                    &mut Printer { writer, warning_duration: WARNING_DURATION },
                    WARNING_DURATION,
                )
                .await?;
            }
        }
        Ok(())
    }
}

impl UpdateTool {
    async fn validate_board_compatibility<W: std::io::Write>(
        product_path: &camino::Utf8Path,
        build_info_proxy: &BuildInfoProviderProxy,
        writer: &mut W,
    ) {
        let pb = match product_bundle::ProductBundle::try_load_from(product_path) {
            Ok(pb) => pb,
            Err(_) => return,
        };
        let product_bundle::ProductBundle::V2(pb_v2) = pb;
        let pb_board = match pb_v2.release_info.as_ref().and_then(|r| {
            r.system_a
                .as_ref()
                .or(r.system_b.as_ref())
                .or(r.system_r.as_ref())
                .map(|s| s.board.info.name.as_str())
        }) {
            Some(b) if !b.is_empty() => b,
            _ => return,
        };

        if let Ok(build_info) = build_info_proxy.get_build_info().await {
            if let Some(target_board) = build_info.board_config.as_deref() {
                if !target_board.is_empty() && target_board != pb_board {
                    let _ = writeln!(
                        writer,
                        "Warning: Board mismatch: product bundle board is '{pb_board}', target device board is '{target_board}'."
                    );
                }
            }
        }
    }

    /// If there's a new version available, update to it, printing progress to the
    /// console during the process.
    async fn handle_check_now_cmd<W: std::io::Write>(
        self,
        cmd: &args::CheckNow,
        writer: &mut W,
    ) -> Result<(), UpdateError> {
        let package_server_task = if cmd.product_bundle {
            let product_path =
                Self::get_product_bundle_path(&cmd.product_bundle_path, &self.context)?;
            let build_info_proxy = self.build_info_proxy.await?;
            Self::validate_board_compatibility(&product_path, &build_info_proxy, writer).await;
            let repo_port: u16 = cmd
                .product_bundle_port(&self.context)
                .map_err(UpdateError::ProductBundlePortResolution)?
                .try_into()
                .unwrap();
            Some(
                server::package_server_task(
                    self.target_spec,
                    self.rcs_proxy_connector.clone(),
                    self.host_address,
                    self.context.clone(),
                    product_path,
                    repo_port,
                    true,
                )
                .await?,
            )
        } else if cmd.product_bundle_path.is_some() {
            return Err(UpdateError::ProductBundleWithoutOption);
        } else {
            None
        };

        let update_manager_proxy = self.update_manager_proxy.await?;

        if let Some(server_task) = package_server_task {
            // Use select! to run the package server at the same time as the others. This is preferable
            // to using detach(), since we can get error result from the package server.
            let server::PackageServerTask { repo_name, repo_url_rx: _, task } = server_task;
            let rcs_proxy_connector = self.rcs_proxy_connector.clone();

            // wait for the server to be registered before running the check.
            let check = async {
                server::wait_for_device_task(&repo_name, rcs_proxy_connector).await?;
                Self::check_for_update(&update_manager_proxy, cmd, writer).await
            };

            let fused_server_task = task.fuse();
            let check_task = check.fuse();

            pin_mut!(fused_server_task, check_task);

            select!(
                server_task_result = fused_server_task => {
                    // The server should start and run indefiniitely, if we get here there is a problem.
                    match server_task_result {
                        Ok(_) => return Err(UpdateError::PackageServerExitedPrematurely),
                        Err(e) => return Err(UpdateError::PackageServerRunFailed(e)),
                    }
                }
                update_task_result = check_task => {
                    server::unregister_pb_repo_server(&repo_name, self.rcs_proxy_connector).await?;
                    return update_task_result;
                }
            );
        } else {
            Self::check_for_update(&update_manager_proxy, cmd, writer).await?;
        }

        Ok(())
    }

    async fn check_for_update<W: std::io::Write>(
        update_manager_proxy: &ManagerProxy,
        cmd: &args::CheckNow,
        writer: &mut W,
    ) -> Result<(), UpdateError> {
        let do_monitor = cmd.monitor || cmd.product_bundle;
        let options = CheckOptions {
            initiator: Some(if cmd.service_initiated {
                Initiator::Service
            } else {
                Initiator::User
            }),
            allow_attaching_to_existing_update_check: Some(true),
            ..Default::default()
        };

        // Create the monitor client if requested, or if using a product bundle as the source.
        // This is needed for product bundles to make sure the package server continues to run
        // until the update is completed.
        let (monitor_client, monitor_server) = if do_monitor {
            let client = update_manager_proxy.domain();
            let (client_end, request_stream) = client.create_request_stream::<MonitorMarker>();
            (Some(client_end), Some(request_stream))
        } else {
            (None, None)
        };

        match update_manager_proxy.check_now(&options, monitor_client).await {
            Ok(ok_result) => {
                if let Err(e) = ok_result {
                    return Err(UpdateError::CheckNowNotStarted(e));
                }
            }
            Err(e) => return Err(UpdateError::Fidl(e)),
        };
        writeln!(writer, "Checking for an update.")?;
        if let Some(monitor_server) = monitor_server {
            monitor_state(monitor_server, writer).await?;
        }
        Ok(())
    }

    /// Change to a specific version, regardless of whether it's newer or older than
    /// the current system software.
    async fn handle_force_install_cmd<W: std::io::Write>(
        self,
        cmd: &ForceInstall,
        writer: &mut W,
    ) -> Result<(), UpdateError> {
        let (mut package_server_task, host_address) =
            if cmd.product_bundle || cmd.product_bundle_path.is_some() {
                let product_path =
                    Self::get_product_bundle_path(&cmd.product_bundle_path, &self.context)?;
                let build_info_proxy = self.build_info_proxy.await?;
                Self::validate_board_compatibility(&product_path, &build_info_proxy, writer).await;

                let repo_port: u16 = cmd
                    .product_bundle_port(&self.context)
                    .map_err(UpdateError::ProductBundlePortResolution)?
                    .try_into()
                    .unwrap();
                (
                    Some(
                        server::package_server_task(
                            self.target_spec,
                            self.rcs_proxy_connector.clone(),
                            self.host_address,
                            self.context.clone(),
                            product_path,
                            repo_port,
                            !cmd.packageless,
                        )
                        .await?,
                    ),
                    None,
                )
            } else {
                (None, Some(self.host_address))
            };

        let installer_proxy = self.installer_proxy.await?;

        let update_url = if let Some(url) = &cmd.update_url {
            url.parse::<http::Uri>().map_err(|e| UpdateError::Fho(bug!(e)))?
        } else if cmd.packageless {
            if let Some(server_task) = &mut package_server_task {
                // Need to get the repo url from the package server task, because it might
                // be a tunneled connection.
                let mut repo_url =
                    timeout::timeout(Duration::from_secs(30), server_task.repo_url_rx.next())
                        .await
                        .map_err(|e| UpdateError::Fho(bug!(e)))?
                        .ok_or_else(|| UpdateError::Fho(bug!("Failed to get the repo url")))?;
                // If package_server_task enters connection loop, we want to get the latest address.
                while let Ok(Some(url)) = server_task.repo_url_rx.try_next() {
                    repo_url = url;
                }
                let url = format!("{repo_url}/ota_manifest");
                url.parse::<http::Uri>().map_err(|e| UpdateError::Fho(bug!(e)))?
            } else {
                let instance_root =
                    self.context.get("repository.process_dir").map_err(UpdateError::Config)?;
                let mgr = pkg::PkgServerInstances::new(instance_root);
                let mut instances = mgr.list_instances().map_err(|e| UpdateError::Fho(bug!(e)))?;
                instances.retain(|i| i.is_running());
                match instances.as_slice() {
                    [] => return Err(UpdateError::NoPackageServerRunning),
                    [instance] => {
                        let host_address: Option<ffx_ssh::parse::HostAddr> =
                            host_address.unwrap().await?.into();
                        match pkg::repo::create_repo_host(
                            instance.address,
                            host_address.map(|t| t.0),
                        )
                        .map_err(|e| UpdateError::Fho(bug!(e)))?
                        {
                            pkg::repo::RepoHostAddr::Direct(repo_host) => {
                                let url =
                                    format!("http://{}/{}/ota_manifest", repo_host, instance.name);
                                url.parse::<http::Uri>().map_err(|e| UpdateError::Fho(bug!(e)))?
                            }
                            pkg::repo::RepoHostAddr::Tunnel => {
                                return Err(UpdateError::TunnelRequired);
                            }
                        }
                    }
                    _ => return Err(UpdateError::MultiplePackageServersRunning(instances)),
                }
            }
        } else {
            http::Uri::from_static("fuchsia-pkg://fuchsia.com/update")
        };
        writeln!(writer, "Using update url: {update_url}")?;

        if let Some(server_task) = package_server_task {
            // Use select! to run the package server at the same time as the others. This is preferable
            // to using detach(), since we can get error result from the package server.
            let server::PackageServerTask { repo_name, repo_url_rx: _, task } = server_task;
            let rcs_proxy_connector = self.rcs_proxy_connector.clone();

            // wait for the server to be registered before running the check.
            let install = async {
                // Packageless update does not need the server to be registered on the target.
                if !cmd.packageless {
                    server::wait_for_device_task(&repo_name, rcs_proxy_connector).await?;
                }
                Self::force_install(update_url, cmd.reboot, installer_proxy, writer).await
            };

            let fused_server_task = task.fuse();
            let install_task = install.fuse();

            pin_mut!(fused_server_task, install_task);

            select!(
                server_task_result = fused_server_task =>  {
                    // The server should start and run indefiniitely, if we get here there is a problem.
                    match server_task_result {
                        Ok(_) => return Err(UpdateError::PackageServerExitedPrematurely),
                        Err(e) => return Err(UpdateError::PackageServerRunFailed(e))
                    }
                }
                update_task_result =  install_task => {
                    if !cmd.packageless {
                        if cmd.reboot {
                            Timer::new(Duration::from_secs(15)).await;
                        }
                        server::unregister_pb_repo_server(&repo_name, self.rcs_proxy_connector).await?;
                    }
                    return update_task_result;
                }
            );
        } else {
            Self::force_install(update_url, cmd.reboot, installer_proxy, writer).await
        }
    }

    async fn force_install<W: std::io::Write>(
        update_url: http::Uri,
        reboot: bool,
        installer_proxy: InstallerProxy,
        writer: &mut W,
    ) -> Result<(), UpdateError> {
        let options = installer::Options {
            initiator: installer::Initiator::User,
            should_write_recovery: true,
            allow_attach_to_existing_attempt: true,
            manifest_range: None,
            manifest_headers: vec![],
        };

        let client = installer_proxy.domain();
        let (reboot_controller, reboot_controller_server_end) =
            client.create_proxy::<finstaller::RebootControllerMarker>();

        let mut update_attempt: installer::UpdateAttemptFDomain = installer::start_update_fdomain(
            &update_url,
            options,
            &installer_proxy,
            Some(reboot_controller_server_end),
        )
        .await
        .map_err(|e| UpdateError::Fho(bug!(e)))?;

        writeln!(writer, "Installing an update.")?;
        if update_url.scheme_str() == Some("fuchsia-pkg") {
            writeln!(
                writer,
                "Progress reporting is based on the fraction of packages resolved, so if one package is much
larger than the others, then the reported progress could appear to stall near the end.
Until the update process is improved to have more granular reporting, try using
    ffx inspect show 'core/pkg-resolver'
for more detail on the progress of update-related downloads.\n"
            )?;
        }
        if !reboot {
            reboot_controller.detach().map_err(|e| UpdateError::Fho(bug!(e)))?;
        }
        write_progress("\nStarting install", writer)?;
        while let Some(state) =
            update_attempt.try_next().await.map_err(|e| UpdateError::Fho(bug!(e)))?
        {
            match state {
                fidl_fuchsia_update_installer_ext::State::WaitToReboot(info) => {
                    // if waiting for reboot, wait for a while to get a head start, hopefully returning after
                    // the shutdown.
                    write_progress_kb(&info, "Waiting to Reboot", writer)?;
                    if reboot {
                        writeln!(writer)?;
                        return Ok(());
                    }
                }
                fidl_fuchsia_update_installer_ext::State::Reboot(info)
                | fidl_fuchsia_update_installer_ext::State::DeferReboot(info)
                | fidl_fuchsia_update_installer_ext::State::Complete(info) => {
                    write_progress_kb(&info, "Complete", writer)?;
                    writeln!(writer)?;
                    return Ok(());
                }

                fidl_fuchsia_update_installer_ext::State::FailPrepare(reason) => {
                    writeln!(writer)?;
                    return Err(UpdateError::InstallFailedPrepare(reason));
                }
                fidl_fuchsia_update_installer_ext::State::FailStage(data) => {
                    writeln!(writer)?;
                    return Err(UpdateError::InstallFailedStage(data.reason()));
                }
                fidl_fuchsia_update_installer_ext::State::FailFetch(data) => {
                    writeln!(writer)?;
                    return Err(UpdateError::InstallFailedFetch(data.reason()));
                }
                fidl_fuchsia_update_installer_ext::State::Canceled => {
                    writeln!(writer)?;
                    return Err(UpdateError::InstallCanceled);
                }

                fidl_fuchsia_update_installer_ext::State::Prepare => {
                    write_progress(&format!("{:6.2}% Preparing", 0.0), writer)?
                }
                fidl_fuchsia_update_installer_ext::State::Stage(info) => {
                    write_progress_kb(&info, "Staging", writer)?
                }
                fidl_fuchsia_update_installer_ext::State::Fetch(info) => {
                    write_progress_kb(&info, "Fetching", writer)?
                }
                fidl_fuchsia_update_installer_ext::State::Commit(info) => {
                    write_progress_kb(&info, "Commit", writer)?
                }
                fidl_fuchsia_update_installer_ext::State::FailCommit(info) => {
                    write_progress_kb(&info, "Failed commit", writer)?
                }
            }
        }

        Ok(())
    }

    fn get_product_bundle_path(
        product_bundle_path: &Option<camino::Utf8PathBuf>,
        context: &EnvironmentContext,
    ) -> Result<camino::Utf8PathBuf, UpdateError> {
        let pb_path = match product_bundle_path {
            Some(product_path) => product_path.clone(),
            None => context
                .get::<Option<String>, _>("product.path")
                .map_err(UpdateError::Config)?
                .ok_or(UpdateError::NoProductBundlePath)?
                .into(),
        };
        Ok(pb_path)
    }
}

fn write_progress<W: std::io::Write>(s: &str, writer: &mut W) -> Result<(), UpdateError> {
    // Use escape sequences to make this line overwrite the current terminal line.
    // \r: send cursor to start of line
    // \x1b[K: clear to end of line
    if termion::is_tty(&std::io::stdout()) {
        write!(writer, "\r{s}\x1b[K").map_err(UpdateError::Io)?;
    } else {
        writeln!(writer, "{s}").map_err(UpdateError::Io)?;
    }
    writer.flush().map_err(UpdateError::Io)
}

fn format_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;

    if bytes >= GIB {
        format!("{:.2} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.2} MiB", bytes as f64 / MIB as f64)
    } else {
        format!("{:.2} KiB", bytes as f64 / KIB as f64)
    }
}

fn write_progress_kb<W: std::io::Write>(
    info: &fidl_fuchsia_update_installer_ext::UpdateInfoAndProgress,
    status: &str,
    writer: &mut W,
) -> Result<(), UpdateError> {
    write_progress(
        &format!(
            "{:6.2}% ({}) {}",
            info.progress().fraction_completed() * 100.0,
            format_bytes(info.progress().bytes_downloaded()),
            status
        ),
        writer,
    )
}

/// Handle subcommands for `update channel`.
async fn handle_channel_control_cmd<W: std::io::Write>(
    cmd: &args::channel::Command,
    channel_provider: Deferred<fupdate_channel::ProviderProxy>,
    channel_control: Deferred<fdomain_fuchsia_update_channelcontrol::ChannelControlProxy>,
    writer: &mut W,
) -> Result<(), UpdateError> {
    match cmd {
        args::channel::Command::Get(_) => {
            let channel = channel_provider.await?.get_current().await?;
            writeln!(writer, "current channel: {}", channel)?;
        }
        args::channel::Command::Target(_) => {
            let channel = channel_control.await?.get_target().await?;
            writeln!(writer, "target channel: {}", channel)?;
        }
        args::channel::Command::Set(args::channel::Set { channel }) => {
            channel_control.await?.set_target(&channel).await?;
        }
        args::channel::Command::List(_) => {
            let channels = channel_control.await?.get_target_list().await?;
            if channels.is_empty() {
                writeln!(writer, "known channels list is empty.")?;
            } else {
                writeln!(writer, "known channels:")?;
                for channel in channels {
                    writeln!(writer, "{}", channel)?;
                }
            }
        }
    }
    Ok(())
}

/// Wait for and print state changes. For informational / DX purposes.
async fn monitor_state<W: std::io::Write>(
    mut stream: MonitorRequestStream,
    writer: &mut W,
) -> Result<(), UpdateError> {
    while let Some(event) = stream.try_next().await.map_err(|e| UpdateError::Fho(bug!(e)))? {
        match event {
            MonitorRequest::OnState { state, responder } => {
                responder.send().map_err(|e| UpdateError::Fho(bug!(e)))?;

                let state = State::from(state);
                match &state {
                    State::CheckingForUpdates => write_progress("Checking for updates", writer)?,
                    State::NoUpdateAvailable => write_progress("No update available", writer)?,
                    State::InstallationDeferredByPolicy(installation_deferred_data) => {
                        let reason =
                            if let Some(reason) = &installation_deferred_data.deferral_reason {
                                format!("{reason:?}")
                            } else {
                                "".into()
                            };
                        write_progress(&format!("Update deferred by policy: {reason}"), writer)?
                    }
                    State::InstallingUpdate(installing_data) => {
                        let pct = if let Some(progress) = &installing_data.installation_progress {
                            format!("{:6.2}%", progress.fraction_completed.unwrap_or(0.0) * 100.0)
                        } else {
                            "".into()
                        };
                        write_progress(&format!("{pct} Installing"), writer)?;
                    }
                    State::WaitingForReboot(installing_data) => {
                        let pct = if let Some(progress) = &installing_data.installation_progress {
                            format!("{:6.2}%", progress.fraction_completed.unwrap_or(0.0) * 100.0)
                        } else {
                            "".into()
                        };
                        write_progress(&format!("{pct} Waiting for reboot"), writer)?;
                        Timer::new(Duration::from_secs(15)).await;
                    }
                    State::InstallationError(installing_data) => {
                        let pct = installing_data
                            .installation_progress
                            .as_ref()
                            .and_then(|p| p.fraction_completed);
                        return Err(UpdateError::InternalInstallation(pct));
                    }
                    State::ErrorCheckingForUpdate => {
                        return Err(UpdateError::UpdateCheckingFailed);
                    }
                };
                if state.is_terminal() {
                    writeln!(writer, "\n")?;
                    return Ok(());
                }
            }
        }
    }
    Ok(())
}

/// The set of events associated with the `wait-for-commit` path.
#[derive(Debug, PartialEq)]
enum CommitEvent {
    Begin,
    Warning,
    End,
}

/// An observer of `update wait-for-commit`.
trait CommitObserver {
    fn on_event(&mut self, event: CommitEvent) -> std::io::Result<()>;
}

/// A `CommitObserver` that forwards the events to writer.
struct Printer<W: std::io::Write> {
    writer: W,
    warning_duration: Duration,
}

impl<W: std::io::Write> CommitObserver for Printer<W> {
    fn on_event(&mut self, event: CommitEvent) -> std::io::Result<()> {
        match event {
            CommitEvent::Begin => writeln!(&mut self.writer, "Waiting for commit."),
            CommitEvent::Warning => writeln!(
                &mut self.writer,
                "It's been {} seconds. Something may be wrong.",
                self.warning_duration.as_secs(),
            ),
            CommitEvent::End => writeln!(&mut self.writer, "Committed!"),
        }
    }
}

/// Waits for the system to commit (e.g. when the EventPair observes a signal).
async fn wait_for_commit(proxy: &CommitStatusProviderProxy) -> Result<(), UpdateError> {
    let p = proxy.is_current_system_committed().await.map_err(|e| UpdateError::Fho(bug!(e)))?;
    fdomain_client::OnFDomainSignals::new(&p.as_handle_ref(), Signals::USER_0)
        .await
        .map_err(UpdateError::FidlFDomain)?;
    Ok(())
}

/// Waits for the commit and sends updates to the observer. This is abstracted from the regular
/// `handle_wait_for_commit` fn so we can test events without having to wait the `warning_duration`.
/// The [testability rubric](https://fuchsia.dev/fuchsia-src/concepts/testing/testability_rubric)
/// exempts logs from testing, but in this case we test them anyway because of the additional layer
/// of complexity that the warning timeout introduces.
async fn handle_wait_for_commit(
    proxy: &CommitStatusProviderProxy,
    observer: &mut impl CommitObserver,
    warning_duration: Duration,
) -> Result<(), UpdateError> {
    observer.on_event(CommitEvent::Begin).map_err(UpdateError::Io)?;

    let commit_fut = wait_for_commit(proxy).fuse();
    futures::pin_mut!(commit_fut);
    let mut timer_fut = fuchsia_async::Timer::new(warning_duration).fuse();

    // Send a warning after the WARNING_DURATION.
    let () = futures::select! {
        commit_res = commit_fut => commit_res?,
        _ = timer_fut => observer.on_event(CommitEvent::Warning).map_err(UpdateError::Io)?,
    };

    // If we timed out on WARNING_DURATION, try again.
    if !commit_fut.is_terminated() {
        let () = commit_fut.await?;
    }

    let () = observer.on_event(CommitEvent::End).map_err(UpdateError::Io)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fdomain_client::Peered;
    use fdomain_fuchsia_update::{CommitStatusProviderRequest, ManagerRequest};
    use fdomain_fuchsia_update_channelcontrol::ChannelControlRequest;
    use ffx_update_args::Update;
    use ffx_writer::TestBuffers;
    use futures::prelude::*;
    use mock_installer_fdomain::MockUpdateInstallerService;
    use std::sync::Arc;
    use target_holders::{fake_async_proxy, fake_proxy};

    fn unused_proxy<T: 'static>() -> Deferred<T> {
        Deferred::from_output(Err(bug!("unused proxy")))
    }

    async fn perform_channel_provider_test<V, O>(
        argument: args::channel::Command,
        verifier: V,
        output: O,
    ) where
        V: Fn(fupdate_channel::ProviderRequest),
        O: Fn(String),
    {
        let client = fdomain_local::local_client_empty();
        let (proxy, mut stream) =
            client.create_proxy_and_stream::<fupdate_channel::ProviderMarker>();
        let mut buf = Vec::new();
        let fut = async {
            assert_matches!(
                handle_channel_control_cmd(
                    &argument,
                    Deferred::from_output(Ok(proxy)),
                    unused_proxy(),
                    &mut buf
                )
                .await,
                Ok(())
            );
        };
        let stream_fut = async move {
            let result = stream.next().await.unwrap();
            match result {
                Ok(cmd) => verifier(cmd),
                err => panic!("Err in request handler: {:?}", err),
            }
        };
        future::join(fut, stream_fut).await;
        let out = String::from_utf8(buf).unwrap();
        output(out);
    }

    async fn perform_channel_control_test<V, O>(
        argument: args::channel::Command,
        verifier: V,
        output: O,
    ) where
        V: Fn(ChannelControlRequest),
        O: Fn(String),
    {
        let client = fdomain_local::local_client_empty();
        let (proxy, mut stream) = client
            .create_proxy_and_stream::<fdomain_fuchsia_update_channelcontrol::ChannelControlMarker>(
            );
        let mut buf = Vec::new();
        let fut = async {
            assert_matches!(
                handle_channel_control_cmd(
                    &argument,
                    unused_proxy(),
                    Deferred::from_output(Ok(proxy)),
                    &mut buf
                )
                .await,
                Ok(())
            );
        };
        let stream_fut = async move {
            let result = stream.next().await.unwrap();
            match result {
                Ok(cmd) => verifier(cmd),
                err => panic!("Err in request handler: {:?}", err),
            }
        };
        future::join(fut, stream_fut).await;
        let out = String::from_utf8(buf).unwrap();
        output(out);
    }

    async fn write_product_bundle(pb_dir: &camino::Utf8Path, board_name: Option<&str>) {
        let blobs_dir = pb_dir.join("blobs");

        let repo_name = "fuchsia.com";
        let metadata_path = pb_dir.join(repo_name);
        fuchsia_repo::test_utils::make_repo_dir(metadata_path.as_ref(), blobs_dir.as_ref(), None)
            .await;

        std::fs::write(metadata_path.join("ota_manifest"), b"mock ota manifest content").unwrap();

        let release_info =
            board_name.map(|board| assembly_release_info::ProductBundleReleaseInfo {
                name: "test".into(),
                version: "test-product-version".into(),
                sdk_version: "test-sdk-version".into(),
                system_a: Some(assembly_release_info::SystemReleaseInfo {
                    board: assembly_release_info::BoardReleaseInfo {
                        info: assembly_release_info::ReleaseInfo {
                            name: board.to_string(),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    ..Default::default()
                }),
                system_b: None,
                system_r: None,
            });

        let pb = product_bundle::ProductBundle::V2(product_bundle::ProductBundleV2 {
            product_name: "test".into(),
            product_version: "test-product-version".into(),
            partitions: assembly_partitions_config::PartitionsConfig::default(),
            sdk_version: "test-sdk-version".into(),
            system_a: None,
            system_b: None,
            system_r: None,
            platform_tools_a: vec![],
            platform_tools_b: vec![],
            platform_tools_r: vec![],
            repositories: vec![product_bundle::Repository {
                name: repo_name.into(),
                metadata_path: metadata_path.into(),
                blobs_path: blobs_dir.clone().into(),
                delivery_blob_type: 1,
                root_private_key_path: None,
                targets_private_key_path: None,
                snapshot_private_key_path: None,
                timestamp_private_key_path: None,
                ota_manifest_signature_path: None,
                ota_manifest_path: None,
            }],
            update_package_hash: None,
            virtual_devices_path: None,
            release_info,
        });
        pb.write(&pb_dir).unwrap();
    }

    fn fake_build_info_proxy(
        client: Arc<fdomain_client::Client>,
        board: Option<&'static str>,
    ) -> BuildInfoProviderProxy {
        fake_proxy(Arc::clone(&client), move |req| match req {
            fdomain_fuchsia_buildinfo::ProviderRequest::GetBuildInfo { responder } => {
                let info = fdomain_fuchsia_buildinfo::BuildInfo {
                    board_config: board.map(|s| s.to_string()),
                    ..Default::default()
                };
                responder.send(&info).unwrap();
            }
        })
    }

    #[fuchsia::test]
    async fn test_validate_board_compatibility_success() {
        let client = fdomain_local::local_client_empty();
        let fake_build_info_proxy = fake_build_info_proxy(Arc::clone(&client), Some("sherlock"));
        let pb_dir_temp = tempfile::tempdir().unwrap();
        let pb_dir = camino::Utf8PathBuf::from_path_buf(pb_dir_temp.path().to_path_buf()).unwrap();
        write_product_bundle(&pb_dir, Some("sherlock")).await;

        let mut output = Vec::new();
        UpdateTool::validate_board_compatibility(&pb_dir, &fake_build_info_proxy, &mut output)
            .await;
        assert!(output.is_empty());
    }

    #[fuchsia::test]
    async fn test_validate_board_compatibility_mismatch() {
        let client = fdomain_local::local_client_empty();
        let fake_build_info_proxy = fake_build_info_proxy(Arc::clone(&client), Some("sherlock"));
        let pb_dir_temp = tempfile::tempdir().unwrap();
        let pb_dir = camino::Utf8PathBuf::from_path_buf(pb_dir_temp.path().to_path_buf()).unwrap();
        write_product_bundle(&pb_dir, Some("iris")).await;

        let mut output = Vec::new();
        UpdateTool::validate_board_compatibility(&pb_dir, &fake_build_info_proxy, &mut output)
            .await;
        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains(
            "Board mismatch: product bundle board is 'iris', target device board is 'sherlock'."
        ));
    }

    #[fuchsia::test]
    async fn test_channel_get() {
        perform_channel_provider_test(
            args::channel::Command::Get(args::channel::Get {}),
            |cmd| match cmd {
                fupdate_channel::ProviderRequest::GetCurrent { responder } => {
                    responder.send("channel").unwrap();
                }
            },
            |output| assert_eq!(output, "current channel: channel\n"),
        )
        .await;
    }

    #[fuchsia::test]
    async fn test_channel_target() {
        perform_channel_control_test(
            args::channel::Command::Target(args::channel::Target {}),
            |cmd| match cmd {
                ChannelControlRequest::GetTarget { responder } => {
                    responder.send("target-channel").unwrap();
                }
                request => panic!("Unexpected request: {:?}", request),
            },
            |output| assert_eq!(output, "target channel: target-channel\n"),
        )
        .await;
    }

    #[fuchsia::test]
    async fn test_channel_set() {
        perform_channel_control_test(
            args::channel::Command::Set(args::channel::Set { channel: "new-channel".to_string() }),
            |cmd| match cmd {
                ChannelControlRequest::SetTarget { channel, responder } => {
                    assert_eq!(channel, "new-channel");
                    responder.send().unwrap();
                }
                request => panic!("Unexpected request: {:?}", request),
            },
            |output| assert!(output.is_empty()),
        )
        .await;
    }

    #[fuchsia::test]
    async fn test_channel_list_no_channels() {
        perform_channel_control_test(
            args::channel::Command::List(args::channel::List {}),
            |cmd| match cmd {
                ChannelControlRequest::GetTargetList { responder } => {
                    responder.send(&[]).unwrap();
                }
                request => panic!("Unexpected request: {:?}", request),
            },
            |output| assert_eq!(output, "known channels list is empty.\n"),
        )
        .await;
    }

    #[fuchsia::test]
    async fn test_channel_list_with_channels() {
        perform_channel_control_test(
            args::channel::Command::List(args::channel::List {}),
            |cmd| match cmd {
                ChannelControlRequest::GetTargetList { responder } => {
                    responder
                        .send(&["some-channel".to_owned(), "other-channel".to_owned()])
                        .unwrap();
                }
                request => panic!("Unexpected request: {:?}", request),
            },
            |output| assert_eq!(output, "known channels:\nsome-channel\nother-channel\n"),
        )
        .await;
    }

    #[fuchsia::test]
    async fn test_check_now() {
        let client = fdomain_local::local_client_empty();
        let test_env = ffx_config::test_init().expect("test env");

        let fake_update_manager_proxy = fake_proxy(Arc::clone(&client), move |req| {
            match req {
                ManagerRequest::CheckNow { responder, .. } => {
                    responder.send(Ok(())).expect("send ok")
                }
                _ => panic!("Unexpected request: {:?}", req),
            };
        });

        let fake_env = crate::server::tests::FakeTestEnv::new(&test_env).await;

        let tool = UpdateTool {
            cmd: Update {
                cmd: args::Command::CheckNow(args::CheckNow {
                    service_initiated: false,
                    monitor: true,
                    product_bundle: false,
                    product_bundle_path: None,
                    product_bundle_port: None,
                }),
            },
            context: test_env.context.clone(),
            update_manager_proxy: Deferred::from_output(Ok(fake_update_manager_proxy)),
            channel_provider_proxy: unused_proxy(),
            channel_control_proxy: unused_proxy(),
            installer_proxy: unused_proxy(),
            commit_status_provider_proxy: unused_proxy(),
            build_info_proxy: unused_proxy(),
            target_spec: fake_env.target_spec,
            rcs_proxy_connector: fake_env.rcs_proxy_connector,
            host_address: fake_env.host_address,
        };
        let buffers = TestBuffers::default();
        let writer = SimpleWriter::new_test(&buffers);

        let result = tool.main(writer).await;

        let (stdout, stderr) = buffers.into_strings();

        assert!(result.is_ok(), "Expected Ok got {result:?}");
        assert_eq!(stdout, "Checking for an update.\n");
        assert_eq!(stderr, "");
    }

    #[fuchsia::test]
    async fn test_force_install() {
        let client = fdomain_local::local_client_empty();
        let test_env = ffx_config::test_init().expect("test env");
        let update_info = installer::UpdateInfo::builder().download_size(1000).build();
        let mock_installer = Arc::new(MockUpdateInstallerService::with_states(vec![
            installer::State::Prepare,
            installer::State::Fetch(
                installer::UpdateInfoAndProgress::new(update_info, installer::Progress::none())
                    .unwrap(),
            ),
            installer::State::Stage(
                installer::UpdateInfoAndProgress::new(
                    update_info,
                    installer::Progress::builder()
                        .fraction_completed(0.5)
                        .bytes_downloaded(500)
                        .build(),
                )
                .unwrap(),
            ),
            installer::State::WaitToReboot(installer::UpdateInfoAndProgress::done(update_info)),
        ]));
        let fake_installer_proxy = mock_installer.spawn_installer_service(Arc::clone(&client));

        let args = ForceInstall {
            reboot: true,
            update_url: Some("fuchsia-pkg://fuchsia.test/update".into()),
            product_bundle: false,
            product_bundle_port: None,
            product_bundle_path: None,
            packageless: false,
        };

        let fake_env = crate::server::tests::FakeTestEnv::new(&test_env).await;

        let tool = UpdateTool {
            cmd: Update { cmd: args::Command::ForceInstall(args) },
            context: test_env.context.clone(),
            update_manager_proxy: unused_proxy(),
            channel_provider_proxy: unused_proxy(),
            channel_control_proxy: unused_proxy(),
            installer_proxy: Deferred::from_output(Ok(fake_installer_proxy)),
            commit_status_provider_proxy: unused_proxy(),
            build_info_proxy: unused_proxy(),
            target_spec: fake_env.target_spec,
            rcs_proxy_connector: fake_env.rcs_proxy_connector,
            host_address: fake_env.host_address,
        };

        let buffers = TestBuffers::default();
        let writer = SimpleWriter::new_test(&buffers);
        tool.main(writer).await.expect("success");

        let (stdout, stderr) = buffers.into_strings();

        assert_eq!(stderr, "");
        assert_eq!(
            stdout,
            "Using update url: fuchsia-pkg://fuchsia.test/update\n\
            Installing an update.\n\
            Progress reporting is based on the fraction of packages resolved, so if one package is much\n\
            larger than the others, then the reported progress could appear to stall near the end.\n\
            Until the update process is improved to have more granular reporting, try using\
            \n    ffx inspect show 'core/pkg-resolver'\n\
            for more detail on the progress of update-related downloads.\n\n\n\
            Starting install\
            \n  0.00% Preparing\
            \n  0.00% (0.00 KiB) Fetching\
            \n 50.00% (0.49 KiB) Staging\
            \n100.00% (0.98 KiB) Waiting to Reboot\n\n"
        );
    }

    #[fuchsia::test]
    async fn test_force_install_packageless() {
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path().to_path_buf();
        let test_env = ffx_config::test_env()
            .user_config("repository.process_dir", temp_path.to_str().unwrap())
            .build()
            .unwrap();
        let client = fdomain_local::local_client_empty();

        let update_info = installer::UpdateInfo::builder().download_size(1000).build();
        let mock_installer = Arc::new(MockUpdateInstallerService::with_states(vec![
            installer::State::Prepare,
            installer::State::Fetch(
                installer::UpdateInfoAndProgress::new(update_info, installer::Progress::none())
                    .unwrap(),
            ),
            installer::State::Stage(
                installer::UpdateInfoAndProgress::new(
                    update_info,
                    installer::Progress::builder()
                        .fraction_completed(0.5)
                        .bytes_downloaded(500)
                        .build(),
                )
                .unwrap(),
            ),
            installer::State::WaitToReboot(installer::UpdateInfoAndProgress::done(update_info)),
        ]));
        let fake_installer_proxy = mock_installer.spawn_installer_service(Arc::clone(&client));

        let args = ForceInstall {
            reboot: true,
            update_url: None,
            product_bundle: false,
            product_bundle_port: None,
            product_bundle_path: None,
            packageless: true,
        };

        let repo_url = fuchsia_url::RepositoryUrl::parse_host("fuchsia.com".into()).unwrap();
        let repo_config = fidl_fuchsia_pkg_ext::RepositoryConfigBuilder::new(repo_url).build();

        pkg::write_instance_info(
            &test_env.context,
            pkg::ServerMode::Foreground,
            "devhost.fuchsia.com",
            &"1.2.3.4:8083".parse().unwrap(),
            fuchsia_repo::repository::RepositorySpec::Pm {
                path: "/tmp".into(),
                aliases: std::collections::BTreeSet::new(),
            },
            fidl_fuchsia_pkg_ext::RepositoryStorageType::Ephemeral,
            fidl_fuchsia_pkg_ext::RepositoryRegistrationAliasConflictMode::Replace,
            repo_config,
        )
        .await
        .expect("write instance info");

        let fake_env = crate::server::tests::FakeTestEnv::new(&test_env).await;

        let tool = UpdateTool {
            cmd: Update { cmd: args::Command::ForceInstall(args) },
            context: test_env.context.clone(),
            update_manager_proxy: unused_proxy(),
            channel_provider_proxy: unused_proxy(),
            channel_control_proxy: unused_proxy(),
            installer_proxy: Deferred::from_output(Ok(fake_installer_proxy)),
            commit_status_provider_proxy: unused_proxy(),
            build_info_proxy: unused_proxy(),
            target_spec: fake_env.target_spec,
            rcs_proxy_connector: fake_env.rcs_proxy_connector,
            host_address: fake_env.host_address,
        };

        let buffers = TestBuffers::default();
        let writer = SimpleWriter::new_test(&buffers);
        tool.main(writer).await.expect("success");

        let (stdout, stderr) = buffers.into_strings();

        assert_eq!(stderr, "");
        assert_eq!(
            stdout,
            "Using update url: http://127.0.0.1:8083/devhost.fuchsia.com/ota_manifest\n\
            Installing an update.\n\n\
            Starting install\
            \n  0.00% Preparing\
            \n  0.00% (0.00 KiB) Fetching\
            \n 50.00% (0.49 KiB) Staging\
            \n100.00% (0.98 KiB) Waiting to Reboot\n\n"
        );
    }

    #[fuchsia::test]
    async fn test_force_install_product_bundle_packageless() {
        let test_env = ffx_config::test_init().expect("test env");
        let client = fdomain_local::local_client_empty();
        let pb_dir_temp = tempfile::tempdir().unwrap();
        let pb_dir = camino::Utf8PathBuf::from_path_buf(pb_dir_temp.path().to_path_buf()).unwrap();
        write_product_bundle(&pb_dir, None).await;

        let args = ForceInstall {
            reboot: true,
            update_url: None,
            product_bundle: false,
            product_bundle_port: None,
            product_bundle_path: Some(pb_dir),
            packageless: true,
        };

        let update_info = installer::UpdateInfo::builder().download_size(1000).build();
        let (mut states_tx, states_rx) = futures::channel::mpsc::channel(1);
        let mock_installer =
            Arc::new(MockUpdateInstallerService::builder().states_receiver(states_rx).build());
        let fake_installer_proxy =
            Arc::clone(&mock_installer).spawn_installer_service(Arc::clone(&client));

        let fake_env = crate::server::tests::FakeTestEnv::new(&test_env).await;

        let tool = UpdateTool {
            cmd: Update { cmd: args::Command::ForceInstall(args) },
            context: test_env.context.clone(),
            update_manager_proxy: unused_proxy(),
            channel_provider_proxy: unused_proxy(),
            channel_control_proxy: unused_proxy(),
            installer_proxy: Deferred::from_output(Ok(fake_installer_proxy)),
            commit_status_provider_proxy: unused_proxy(),
            build_info_proxy: Deferred::from_output(Ok(fake_build_info_proxy(
                Arc::clone(&client),
                Some("test"),
            ))),
            target_spec: fake_env.target_spec,
            rcs_proxy_connector: fake_env.rcs_proxy_connector,
            host_address: fake_env.host_address,
        };

        let buffers = TestBuffers::default();
        let writer = SimpleWriter::new_test(&buffers);
        let tool_task = fuchsia_async::Task::local(async move {
            tool.main(writer).await.expect("success");
        });

        let mut url_str = None;
        for _ in 0..100 {
            if let Some(mock_installer_fdomain::CapturedUpdateInstallerRequest::StartUpdate {
                url,
                ..
            }) = mock_installer.captured_args().lock().get(0)
            {
                url_str = Some(url.clone());
                break;
            }
            fuchsia_async::Timer::new(std::time::Duration::from_millis(100)).await;
        }
        let url = url_str.expect("StartUpdate should be called");
        let client = fuchsia_hyper::new_client();
        let res = client
            .request(
                hyper::Request::get(&url)
                    .body(http_body_util::Full::<hyper::body::Bytes>::default())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), hyper::StatusCode::OK);

        // Unblock the update process after the URL is verified.
        states_tx.send(installer::State::Prepare).await.unwrap();
        states_tx
            .send(installer::State::Fetch(
                installer::UpdateInfoAndProgress::new(update_info, installer::Progress::none())
                    .unwrap(),
            ))
            .await
            .unwrap();
        states_tx
            .send(installer::State::Stage(
                installer::UpdateInfoAndProgress::new(
                    update_info,
                    installer::Progress::builder()
                        .fraction_completed(0.5)
                        .bytes_downloaded(500)
                        .build(),
                )
                .unwrap(),
            ))
            .await
            .unwrap();
        states_tx
            .send(installer::State::WaitToReboot(installer::UpdateInfoAndProgress::done(
                update_info,
            )))
            .await
            .unwrap();
        drop(states_tx);

        tool_task.await;

        let (stdout, stderr) = buffers.into_strings();
        assert_eq!(stderr, "");
        assert!(
            stdout.ends_with(
                "Installing an update.\n\n\
                Starting install\
                \n  0.00% Preparing\
                \n  0.00% (0.00 KiB) Fetching\
                \n 50.00% (0.49 KiB) Staging\
                \n100.00% (0.98 KiB) Waiting to Reboot\n\n"
            ),
            "stdout: {stdout}",
        );
    }

    struct TestCommitObserver {
        events: Vec<CommitEvent>,
    }
    impl TestCommitObserver {
        fn new() -> Self {
            Self { events: vec![] }
        }
        fn take_events(&mut self) -> Vec<CommitEvent> {
            self.events.drain(..).collect()
        }
    }
    impl CommitObserver for TestCommitObserver {
        fn on_event(&mut self, event: CommitEvent) -> std::io::Result<()> {
            self.events.push(event);
            Ok(())
        }
    }

    #[fuchsia::test]
    async fn test_wait_for_commit() {
        let client = fdomain_local::local_client_empty();
        let client_clone = Arc::clone(&client);
        let proxy = fake_async_proxy(Arc::clone(&client), async move |req| {
            let CommitStatusProviderRequest::IsCurrentSystemCommitted { responder } = req;

            let (lhs, rhs) = client_clone.create_event_pair();
            let () = responder.send(lhs).unwrap();

            fuchsia_async::Timer::new(Duration::from_millis(500)).await;

            let () = rhs.signal_peer(Signals::NONE, Signals::USER_0).await.unwrap();

            ()
        });

        let mut observer = TestCommitObserver::new();

        handle_wait_for_commit(&proxy, &mut observer, Duration::from_millis(1000)).await.unwrap();
        assert_eq!(observer.take_events(), &[CommitEvent::Begin, CommitEvent::End]);

        handle_wait_for_commit(&proxy, &mut observer, Duration::from_millis(50)).await.unwrap();
        assert_eq!(
            observer.take_events(),
            &[CommitEvent::Begin, CommitEvent::Warning, CommitEvent::End]
        );
    }

    #[fuchsia::test]
    async fn test_force_install_fail_prepare() {
        let client = fdomain_local::local_client_empty();
        let test_env = ffx_config::test_init().expect("test env");
        let mock_installer = Arc::new(MockUpdateInstallerService::with_states(vec![
            installer::State::Prepare,
            installer::State::FailPrepare(installer::PrepareFailureReason::Internal),
        ]));
        let fake_installer_proxy = mock_installer.spawn_installer_service(Arc::clone(&client));

        let args = ForceInstall {
            reboot: true,
            update_url: Some("fuchsia-pkg://fuchsia.test/update".into()),
            product_bundle: false,
            product_bundle_port: None,
            product_bundle_path: None,
            packageless: false,
        };

        let fake_env = crate::server::tests::FakeTestEnv::new(&test_env).await;

        let tool = UpdateTool {
            cmd: Update { cmd: args::Command::ForceInstall(args) },
            context: test_env.context.clone(),
            update_manager_proxy: unused_proxy(),
            channel_provider_proxy: unused_proxy(),
            channel_control_proxy: unused_proxy(),
            installer_proxy: Deferred::from_output(Ok(fake_installer_proxy)),
            commit_status_provider_proxy: unused_proxy(),
            build_info_proxy: unused_proxy(),
            target_spec: fake_env.target_spec,
            rcs_proxy_connector: fake_env.rcs_proxy_connector,
            host_address: fake_env.host_address,
        };

        let buffers = TestBuffers::default();
        let writer = SimpleWriter::new_test(&buffers);
        let result = tool.main(writer).await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            UpdateError::InstallFailedPrepare(installer::PrepareFailureReason::Internal)
        ));
    }

    #[fuchsia::test]
    async fn test_check_now_monitor_error() {
        use fdomain_fuchsia_update::ManagerRequest;
        let client = fdomain_local::local_client_empty();
        let test_env = ffx_config::test_init().expect("test env");

        let fake_update_manager_proxy = fake_proxy(Arc::clone(&client), move |req| {
            match req {
                ManagerRequest::CheckNow { monitor, responder, .. } => {
                    let monitor_client = monitor.unwrap();
                    let monitor_proxy = monitor_client.into_proxy();
                    fuchsia_async::Task::local(async move {
                        let state = fdomain_fuchsia_update::State::ErrorCheckingForUpdate(
                            fdomain_fuchsia_update::ErrorCheckingForUpdateData::default(),
                        );
                        monitor_proxy.on_state(&state).await.unwrap();
                    })
                    .detach();
                    responder.send(Ok(())).expect("send ok")
                }
                _ => panic!("Unexpected request: {:?}", req),
            };
        });

        let fake_env = crate::server::tests::FakeTestEnv::new(&test_env).await;

        let tool = UpdateTool {
            cmd: Update {
                cmd: args::Command::CheckNow(args::CheckNow {
                    service_initiated: false,
                    monitor: true,
                    product_bundle: false,
                    product_bundle_path: None,
                    product_bundle_port: None,
                }),
            },
            context: test_env.context.clone(),
            update_manager_proxy: Deferred::from_output(Ok(fake_update_manager_proxy)),
            channel_provider_proxy: unused_proxy(),
            channel_control_proxy: unused_proxy(),
            installer_proxy: unused_proxy(),
            commit_status_provider_proxy: unused_proxy(),
            build_info_proxy: unused_proxy(),
            target_spec: fake_env.target_spec,
            rcs_proxy_connector: fake_env.rcs_proxy_connector,
            host_address: fake_env.host_address,
        };
        let buffers = TestBuffers::default();
        let writer = SimpleWriter::new_test(&buffers);

        let result = tool.main(writer).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), UpdateError::UpdateCheckingFailed));
    }

    #[fuchsia::test]
    async fn test_check_now_monitor_installation_error() {
        use fdomain_fuchsia_update::ManagerRequest;
        let client = fdomain_local::local_client_empty();
        let test_env = ffx_config::test_init().expect("test env");

        let fake_update_manager_proxy = fake_proxy(Arc::clone(&client), move |req| {
            match req {
                ManagerRequest::CheckNow { monitor, responder, .. } => {
                    let monitor_client = monitor.unwrap();
                    let monitor_proxy = monitor_client.into_proxy();
                    fuchsia_async::Task::local(async move {
                        let state = fdomain_fuchsia_update::State::InstallationError(
                            fdomain_fuchsia_update::InstallationErrorData::default(),
                        );
                        monitor_proxy.on_state(&state).await.unwrap();
                    })
                    .detach();
                    responder.send(Ok(())).expect("send ok")
                }
                _ => panic!("Unexpected request: {:?}", req),
            };
        });

        let fake_env = crate::server::tests::FakeTestEnv::new(&test_env).await;

        let tool = UpdateTool {
            cmd: Update {
                cmd: args::Command::CheckNow(args::CheckNow {
                    service_initiated: false,
                    monitor: true,
                    product_bundle: false,
                    product_bundle_path: None,
                    product_bundle_port: None,
                }),
            },
            context: test_env.context.clone(),
            update_manager_proxy: Deferred::from_output(Ok(fake_update_manager_proxy)),
            channel_provider_proxy: unused_proxy(),
            channel_control_proxy: unused_proxy(),
            installer_proxy: unused_proxy(),
            commit_status_provider_proxy: unused_proxy(),
            build_info_proxy: unused_proxy(),
            target_spec: fake_env.target_spec,
            rcs_proxy_connector: fake_env.rcs_proxy_connector,
            host_address: fake_env.host_address,
        };
        let buffers = TestBuffers::default();
        let writer = SimpleWriter::new_test(&buffers);

        let result = tool.main(writer).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), UpdateError::InternalInstallation(None)));
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(0), "0.00 KiB");
        assert_eq!(format_bytes(500), "0.49 KiB");
        assert_eq!(format_bytes(1024), "1.00 KiB");
        assert_eq!(format_bytes(1024 * 500), "500.00 KiB");
        assert_eq!(format_bytes(1024 * 1024), "1.00 MiB");
        assert_eq!(format_bytes(1024 * 1024 * 50), "50.00 MiB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.00 GiB");
        assert_eq!(format_bytes((1024 * 1024 * 1024 * 3) + (1024 * 1024 * 512)), "3.50 GiB");
    }
}

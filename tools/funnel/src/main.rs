// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::errors::{FunnelError, IntoExitCode};
use crate::metrics::MetricsService;
use crate::ssh::do_ssh;
use crate::target::choose_target;
#[cfg(feature = "update")]
use anyhow::anyhow;
use anyhow::{Context, Result};
use argh::FromArgs;
#[cfg(feature = "update")]
use camino::Utf8PathBuf;
use chrono::Local;
use discovery::query::TargetInfoQuery;
use discovery::{
    DiscoveryBuilder, DiscoverySources, FastbootConnectionState, TargetEvent, TargetState,
};
use ffx_config::environment::ExecutableKind;
use ffx_config::{ConfigMap, EnvironmentContext};
use futures::{Stream, StreamExt};
use log::LevelFilter;
use signal_hook::consts::signal::*;
use signal_hook::iterator::Signals;
use std::collections::HashMap;
use std::io;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use target::{TargetInfo, TargetMode};
use timeout::timeout;

mod errors;
mod logging;
mod metrics;
mod ssh;
mod target;
mod update;

fn default_log_level() -> LevelFilter {
    LevelFilter::Error
}

fn default_repository_ports() -> Vec<u32> {
    vec![8083]
}

#[derive(FromArgs)]
/// ffx Remote forwarding.
struct Funnel {
    /// sets the log level for  output (default = Error). Other
    /// possible values are Info, Debug, Warn, and Trace
    #[argh(option, short = 'l', default = "default_log_level()")]
    log_level: LevelFilter,

    #[argh(subcommand)]
    nested: FunnelSubcommands,
}

#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
enum FunnelSubcommands {
    Host(SubCommandHost),
    #[cfg(feature = "update")]
    Update(SubCommandUpdate),
    Cleanup(SubCommandCleanupRemote),
    CloseLocalTunnel(SubCommandCloseLocalTunnel),
    ListTargets(SubCommandListTargets),
}

impl FunnelSubcommands {
    fn command_name(&self) -> String {
        match self {
            Self::Host(_) => "host",
            #[cfg(feature = "update")]
            Self::Update(_) => "update",
            Self::Cleanup(_) => "cleanup",
            Self::CloseLocalTunnel(_) => "close-local-tunnel",
            Self::ListTargets(_) => "list-targets",
        }
        .to_string()
    }
}

#[derive(FromArgs, PartialEq, Debug)]
/// Remotely forwards a target from your local host to a remote one.
#[argh(subcommand, name = "host")]
struct SubCommandHost {
    /// the remote host to forward to
    #[argh(positional)]
    host: String,

    /// the name of the target to forward
    #[argh(option, short = 't')]
    target_name: Option<String>,

    /// the repository ports to forward to the remote host (defaults to 8083 if none are specified).
    #[argh(option, short = 'r')]
    repository_ports: Vec<u32>,

    /// additional ports to forward from the remote host to the target.
    #[argh(option, short = 'p')]
    additional_port_forwards: Vec<u32>,

    /// time to wait to discover targets.
    #[argh(option, short = 'w', default = "1")]
    wait_for_target_time: u64,
}

#[allow(dead_code)] // TODO(https://fxbug.dev/351850455)
#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand, name = "self-update")]
/// Perform a self-update.
///
/// This assumes that your machine both has the cipd binary available in your
/// $PATH and that there is a file named funnel-cipd-manifest in the same
/// directory that the `funnel` tool resides in, which is a valid cipd-manifest
struct SubCommandUpdate {}

#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand, name = "cleanup-remote-host")]
/// Cleans up a remote host's connections.
///
/// This will ssh to the provided remote host and look for sshd processes that
/// are actively listening to port 8022. If there are any, then it will kill them.
/// This needs to be run from an interactive shell as it will prompt the user for
/// their password (sudo).
struct SubCommandCleanupRemote {
    /// the remote host to cleanup
    #[argh(positional)]
    host: String,
}

#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand, name = "close-local-tunnel")]
/// Closes the local tunnel if it exists.
///
/// Instructs the ControlMaster running for funnel to exit
struct SubCommandCloseLocalTunnel {
    /// the remote host to cleanup
    #[argh(positional)]
    host: String,
}

#[derive(FromArgs, PartialEq, Debug)]
/// Remotely forwards a target from your local host to a remote one.
#[argh(subcommand, name = "list-targets")]
struct SubCommandListTargets {
    /// time to wait to discover targets.
    #[argh(switch, short = 'w')]
    watch: bool,
}

#[fuchsia::main]
async fn main() -> Result<()> {
    let args: Funnel = argh::from_env();

    logging::init(args.log_level)?;

    let command_name = args.nested.command_name();
    let metrics_service = metrics::GaMetricsService::new("0.1".to_string()).await?;

    let ctx =
        EnvironmentContext::no_context(ExecutableKind::Subtool, ConfigMap::default(), None, false)?;

    let res = match args.nested {
        FunnelSubcommands::Host(host_command) => funnel_main(&ctx, host_command).await,
        #[cfg(feature = "update")]
        FunnelSubcommands::Update(update_command) => update_main(update_command).await,
        FunnelSubcommands::Cleanup(cleanup_command) => cleanup_main(cleanup_command).await,
        FunnelSubcommands::CloseLocalTunnel(close_existing_tunnel) => {
            close_existing_tunnel_main(close_existing_tunnel).await
        }
        FunnelSubcommands::ListTargets(list_targets) => list_targets_main(&ctx, list_targets).await,
    };

    let (exit_code, error_message) = match res {
        Ok(_) => (0, None),
        Err(ref e) => (e.exit_code(), Some(format!("{}", e))),
    };

    if let Err(record_res) =
        metrics_service.record_invocation(command_name, exit_code, error_message).await
    {
        log::warn!("Error recording metrics: {}", record_res);
    }

    match res {
        Ok(_) => Ok(()),
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(e.exit_code())
        }
    }
}

async fn close_existing_tunnel_main(args: SubCommandCloseLocalTunnel) -> Result<(), FunnelError> {
    ssh::close_existing_tunnel(args.host).map_err(FunnelError::from)
}

async fn cleanup_main(args: SubCommandCleanupRemote) -> Result<(), FunnelError> {
    ssh::cleanup_remote_sshd(args.host).await.map_err(FunnelError::from)
}

#[cfg(feature = "update")]
async fn update_main(_args: SubCommandUpdate) -> Result<(), FunnelError> {
    let current_exe_path = std::env::current_exe()?;
    let exe_path_buf = Utf8PathBuf::from_path_buf(current_exe_path)
        .map_err(|p| anyhow!("Non-Utf8 Path passed: {}", p.display()))?;
    update::self_update(exe_path_buf).await.map_err(FunnelError::from)
}

async fn list_targets_main(
    ctx: &EnvironmentContext,
    args: SubCommandListTargets,
) -> Result<(), FunnelError> {
    // Only want added events
    let discovery = DiscoveryBuilder::default()
        .set_source(DiscoverySources::MDNS | DiscoverySources::USB_FASTBOOT)
        .with_timeout_msecs(None)
        .build(ctx);
    let mut device_stream =
        discovery.discovery_stream(TargetInfoQuery::First).map_err(anyhow::Error::from)?;

    let mut stdout = io::stdout().lock();

    if args.watch {
        while let Some(event) = device_stream.next().await {
            let now = Local::now();
            write!(stdout, "{}\t", now.format("%Y-%m-%d %H:%M:%S"))?;
            write_target_event(&mut stdout, event)?;
        }
    } else {
        let wait_duration = Duration::from_secs(2);
        let targets = discover_target_events(&mut stdout, device_stream, wait_duration).await?;
        for target in targets {
            writeln!(stdout, "{}", target)?;
        }
    }

    Ok(())
}

async fn funnel_main(ctx: &EnvironmentContext, args: SubCommandHost) -> Result<(), FunnelError> {
    log::trace!("Discoving targets...");
    let wait_duration = Duration::from_secs(args.wait_for_target_time);

    // Only want added events
    let discovery = DiscoveryBuilder::default()
        .set_source(DiscoverySources::MDNS | DiscoverySources::USB_FASTBOOT)
        .build(ctx);
    let device_stream =
        discovery.discovery_stream(TargetInfoQuery::First).map_err(anyhow::Error::from)?;

    let mut stdout = io::stdout().lock();
    let targets = discover_target_events(&mut stdout, device_stream, wait_duration).await?;

    let mut stdin = io::stdin().lock();
    let target = choose_target(&mut stdin, &mut stdout, targets, args.target_name).await?;
    // Drop the locks we have on stdin and stdout
    drop(stdin);

    log::debug!("Target to forward: {:?}", target);
    log::info!("Additional port forwards: {:?}", args.additional_port_forwards);
    let host = args.host.clone();
    let repository_ports = if args.repository_ports.len() > 0 {
        args.repository_ports
    } else {
        default_repository_ports()
    };
    let do_ssh_fut =
        do_ssh(&mut stdout, host.clone(), target, repository_ports, args.additional_port_forwards);
    // Need to both do ssh and listen for signals
    let mut signals = Signals::new(&[SIGINT]).unwrap();
    let handle = signals.handle();
    // Need something to signal that we exited due to a signal
    let term_requested = Arc::new(Mutex::new(false));
    let thread_reqested = Arc::clone(&term_requested);
    let signal_thread = std::thread::spawn(move || {
        if let Some(signal) = signals.forever().next() {
            assert_eq!(signal, SIGINT);
            eprintln!("\nCaught interrupt. Shutting down the tunnel...");
            let res = ssh::close_existing_tunnel(host.clone());
            log::debug!("Result from closing existing tunnel: {:?}", res);
            *thread_reqested.lock().unwrap() = true;
            return;
        }
    });

    let do_ssh_res = do_ssh_fut.await;
    drop(stdout);
    log::info!("result from do_ssh: {:?}", do_ssh_res);
    match do_ssh_res {
        Ok(_) => {}
        e @ Err(ssh::TunnelError::PortForwardingFailed { remote_host: _ }) => {
            // If this returned an error due to our signal just log and return
            let was_requested = term_requested.lock().unwrap();
            if !*was_requested {
                // We did not get a signal. Error out
                e?;
            }
        }
        e @ _ => {
            // We got an unexpected error
            e?;
        }
    }
    handle.close();
    signal_thread.join().expect("signal thread to shutdown without panic");
    Ok(())
}

fn write_target_event<W: Write>(mut writer: W, event: TargetEvent) -> Result<()> {
    let symbol = match event {
        TargetEvent::Added(_) => "+",
        TargetEvent::Removed(_) => "-",
    };

    let handle = match event {
        TargetEvent::Added(handle) => handle,
        TargetEvent::Removed(handle) => handle,
    };

    let node_name = handle.node_name.unwrap_or_else(|| "<unknown>".to_string());

    write!(writer, "({symbol})\t{node_name}\t")?;
    write_target_state(&mut writer, handle.state)?;
    write!(writer, "\n")?;
    Ok(())
}

fn write_target_state<W: Write>(writer: &mut W, state: TargetState) -> Result<()> {
    match state {
        TargetState::Unknown => write!(writer, "Unknown")?,
        TargetState::Product { addrs, .. } => {
            write!(writer, "Product\t")?;
            let addr_strs =
                addrs.iter().map(|addr| format!("{}", addr)).collect::<Vec<_>>().join("\t");
            write!(writer, "{}", addr_strs)?
        }
        TargetState::Zedboot => write!(writer, "Zedboot")?,
        TargetState::Fastboot(fastboot_state) => {
            write!(writer, "Fastboot\t")?;
            match fastboot_state.connection_state {
                FastbootConnectionState::Usb => write!(writer, "{}", fastboot_state.serial_number)?,
                FastbootConnectionState::Tcp(addrs) | FastbootConnectionState::Udp(addrs) => {
                    let addr_strs =
                        addrs.iter().map(|addr| format!("{}", addr)).collect::<Vec<_>>().join("\t");
                    write!(writer, "{}", addr_strs)?
                }
            }
        }
    };
    Ok(())
}

async fn discover_target_events<W>(
    w: &mut W,
    mut stream: impl Stream<Item = TargetEvent> + Unpin,
    wait_time: Duration,
) -> Result<Vec<TargetInfo>>
where
    W: Write,
{
    let mut targets = HashMap::new();
    let task = async {
        while let Some(event) = stream.next().await {
            match event {
                TargetEvent::Removed(handle) => {
                    targets.remove(&handle.node_name.unwrap_or_else(|| "".to_string()));
                }
                TargetEvent::Added(handle) => match handle.state {
                    TargetState::Product { addrs: addr, .. } => {
                        targets.insert(
                            handle.node_name.unwrap_or_else(|| "".to_string()),
                            (addr, TargetMode::Product),
                        );
                    }
                    TargetState::Fastboot(fb_state) => {
                        match fb_state.connection_state {
                            FastbootConnectionState::Usb => {
                                log::warn!(
                                    "Discovered Target {} in Fastboot USB mode. It cannot be used with `funnel`. Skipping",
                                    fb_state.serial_number
                                );
                                writeln!(w, "Discovered Target {} in Fastboot over USB mode, which funnel does not support.", fb_state.serial_number).context("writing to stdout")?;
                            }
                            FastbootConnectionState::Tcp(tcp_state) => {
                                // We support fastboot over tcp!
                                targets.insert(
                                    handle.node_name.unwrap_or_else(|| "".to_string()),
                                    (
                                        tcp_state.iter().map(Into::into).collect(),
                                        TargetMode::Fastboot,
                                    ),
                                );
                            }
                            FastbootConnectionState::Udp(udp_state) => {
                                log::warn!(
                                    "Discovered Target at address: {:?} in Fastboot Over UDP mode, which funnel does not support.",
                                    udp_state
                                );
                            }
                        }
                    }
                    TargetState::Zedboot => {
                        log::warn!(
                            "Discovered Target in Zedboot mode. It cannot be used with `funnel`. Skipping."
                        );
                        writeln!(
                            w,
                            "Discovered Target in Zedboot mode, which funnel does not support."
                        )?;
                    }
                    TargetState::Unknown => {
                        log::warn!("Discovered a target in an unknown state. Skipping");
                        writeln!(w, "Discovered a Target in an unknown state.")?;
                    }
                },
            }
        }
        Ok::<(), anyhow::Error>(())
    };

    match timeout(wait_time, task).await {
        Ok(_) => {
            log::warn!(
                "Got an okay result from the discover target loop.... this shouldnt happen we should always timeout."
            )
        }
        Err(_) => {
            log::trace!("Timeout reached");
        }
    }
    let res = targets
        .iter()
        .map(|(nodename, (addresses, mode))| TargetInfo {
            nodename: nodename.to_string(),
            addresses: addresses.clone(),
            mode: mode.clone(),
        })
        .collect();
    Ok(res)
}

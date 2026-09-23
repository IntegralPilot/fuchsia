// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::num::NonZeroU32;
use std::time::Duration;

use component_debug_fdomain::lifecycle::{self, CreateError};
use errors::ffx_error;
use fdomain_fuchsia_developer_ffx_speedtest as fspeedtest;
use ffx_speedtest_args::{Ping, Socket, SpeedtestCommand, Subcommand};
use ffx_writer::{ToolIO as _, VerifiedMachineWriter};
use fho::{Deferred, FfxMain, FfxTool};
use fuchsia_async as fasync;
use fuchsia_url::fuchsia_pkg::AbsoluteComponentUrl;
use moniker::Moniker;
use speedtest_fdomain::client;
use target_holders::RemoteControlProxyHolder;

#[derive(FfxTool)]
pub struct SpeedtestTool {
    remote_control: Deferred<RemoteControlProxyHolder>,
    #[command]
    cmd: SpeedtestCommand,
}

fho::embedded_plugin!(SpeedtestTool);

#[async_trait::async_trait(?Send)]
impl FfxMain for SpeedtestTool {
    type Writer = VerifiedMachineWriter<Vec<client::SpeedtestReport>>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        let Self { cmd, remote_control } = self;
        let moniker = Moniker::parse_str("/core/ffx-laboratory:speedtest").unwrap();
        let SpeedtestCommand { overnet, mut repeat, delay, cmd } = cmd;
        let client = if overnet {
            return Err(fho::user_error!("Overnet no longer supported"));
        } else {
            let proxy = start_speedtest_component(&moniker, &remote_control.await?).await?;
            client::Client::new(proxy).await.map_err(|e| fho::bug!(e))?
        };

        let mut reports = Vec::new();
        loop {
            let report = match cmd {
                Subcommand::Ping(Ping { count }) => {
                    let report = client.ping(count).await.map_err(|e| fho::bug!(e))?;
                    client::SpeedtestReport::Ping(report)
                }
                Subcommand::Socket(Socket {
                    transfer_mb,
                    buffer_kb,
                    rx,
                    fdomain_individual_reads,
                    fdomain_writes_in_flight,
                }) => {
                    let data_len = transfer_mb
                        .checked_mul(NonZeroU32::new(1_000_000).unwrap())
                        .ok_or_else(|| fho::user_error!("transfer too large"))?;
                    let buffer_len = buffer_kb
                        .checked_mul(NonZeroU32::new(1_000).unwrap())
                        .ok_or_else(|| fho::user_error!("buffer size too large"))?;
                    let writes_in_flight =
                        fdomain_writes_in_flight.unwrap_or_else(|| data_len.div_ceil(buffer_len));
                    let direction = if rx { client::Direction::Rx } else { client::Direction::Tx };
                    let params = client::SocketTransferParams {
                        direction,
                        params: client::TransferParams {
                            data_len,
                            buffer_len,
                            fdomain_params: client::FDomainTransferParams {
                                streaming_read: !fdomain_individual_reads,
                                writes_in_flight,
                            },
                        },
                    };

                    let report = client.socket(params).await.map_err(|e| fho::bug!(e))?;
                    client::SpeedtestReport::Socket(report)
                }
            };

            writer.line(&report)?;
            if writer.is_machine() {
                reports.push(report);
            }

            match repeat {
                0 => {}
                1 => break,
                _ => {
                    repeat -= 1;
                }
            }

            if delay != Duration::ZERO {
                fasync::Timer::new(delay).await;
            }
        }

        writer.machine(&reports)?;

        Ok(())
    }
}

fn unpack_moniker(
    moniker: &Moniker,
) -> (Moniker, &cm_types::BorrowedLongName, &cm_types::BorrowedName) {
    let parent = moniker.parent().unwrap();
    let leaf = moniker.leaf().unwrap();
    let child_name = leaf.name();
    let collection = leaf.collection().unwrap();

    (parent, child_name, collection)
}

async fn start_speedtest_component(
    moniker: &Moniker,
    remote_control: &RemoteControlProxyHolder,
) -> fho::Result<fspeedtest::SpeedtestProxy> {
    let lifecycle_controller =
        ffx_component::rcs::connect_to_lifecycle_controller(&remote_control).await?;

    let (parent, child_name, collection) = unpack_moniker(moniker);
    let url = AbsoluteComponentUrl::parse("fuchsia-pkg://fuchsia.com/speedtest#meta/speedtest.cm")
        .unwrap();
    lifecycle::create_instance_in_collection(
        &lifecycle_controller,
        &parent,
        collection,
        child_name,
        &url,
        vec![],
        None,
    )
    .await
    .or_else(|e| match e {
        CreateError::InstanceAlreadyExists => Ok(()),
        e => Err(ffx_error!(e)),
    })?;

    rcs::connect_with_timeout::<fspeedtest::SpeedtestMarker>(
        Duration::from_secs(10),
        moniker.as_ref(),
        &remote_control,
    )
    .await
    .map_err(|e| fho::bug!(e))
}

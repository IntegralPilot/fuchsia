// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fmt::{self, Display};
use std::num::{NonZeroU32, TryFromIntError};
use std::time::{Duration, Instant};

use flex_client::ProxyHasDomain;
use flex_fuchsia_developer_ffx_speedtest as fspeedtest;
use futures::TryFutureExt as _;
use schemars::JsonSchema;
use serde::Serialize;
use thiserror::Error;

use crate::throughput::BytesFormatter;
use crate::{Throughput, socket};
#[cfg(feature = "fdomain")]
pub use socket::FDomainTransferParams;
pub use socket::TransferParams;

pub struct Client {
    proxy: fspeedtest::SpeedtestProxy,
}

#[derive(Error, Debug)]
pub enum ClientError {
    #[error(transparent)]
    Fidl(#[from] fidl::Error),
    #[error("integer conversion error {0}")]
    Conversion(#[from] TryFromIntError),
    #[error(transparent)]
    Transfer(#[from] socket::TransferError),
    #[error(transparent)]
    Encoding(#[from] socket::MissingFieldError),
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PingReport {
    pub min: Duration,
    pub avg: Duration,
    pub max: Duration,
}

impl Display for PingReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self { max, avg, min } = self;
        write!(f, "min = {min:?}, avg = {avg:?}, max = {max:?}")?;
        Ok(())
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Tx,
    Rx,
}

impl Direction {
    pub fn flip(self) -> Self {
        match self {
            Self::Tx => Self::Rx,
            Self::Rx => Self::Tx,
        }
    }

    fn local_label(&self) -> &'static str {
        match self {
            Direction::Tx => "sender",
            Direction::Rx => "receiver",
        }
    }
}

#[derive(Debug)]
pub struct SocketTransferParams {
    pub direction: Direction,
    pub params: socket::TransferParams,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SocketTransferReport {
    pub direction: Direction,
    pub client: SocketTransferReportInner,
    pub server: SocketTransferReportInner,
}

impl Display for SocketTransferReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self { direction, client, server } = self;
        let local_label = direction.local_label();
        let remote_label = direction.flip().local_label();
        writeln!(f, "local({local_label}): {client}")?;
        write!(f, "remote({remote_label}): {server}")?;
        Ok(())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SocketTransferReportInner {
    pub transfer_len: NonZeroU32,
    pub duration: Duration,
    pub throughput: Throughput,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum SpeedtestReport {
    Ping(PingReport),
    Socket(SocketTransferReport),
}

impl Display for SpeedtestReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ping(report) => Display::fmt(report, f),
            Self::Socket(report) => Display::fmt(report, f),
        }
    }
}

impl Display for SocketTransferReportInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self { transfer_len, duration, throughput } = self;
        let transfer_len = BytesFormatter(transfer_len.get().into());
        write!(f, "{transfer_len} in {duration:.1?} => {throughput}")
    }
}

impl SocketTransferReportInner {
    fn new(params: &socket::TransferParams, report: socket::Report) -> Self {
        let socket::Report { duration } = report;
        let transfer_len = params.data_len;
        let throughput = Throughput::from_len_and_duration(transfer_len.get(), duration);
        Self { transfer_len, duration, throughput }
    }
}

impl Client {
    pub async fn new(proxy: fspeedtest::SpeedtestProxy) -> Result<Self, ClientError> {
        // Run a ping to ensure the service has started.
        proxy.ping().await?;
        Ok(Self { proxy })
    }

    pub async fn ping(&self, repeat: NonZeroU32) -> Result<PingReport, ClientError> {
        let mut total = Duration::ZERO;
        let mut max = Duration::ZERO;
        let mut min = Duration::MAX;
        for _ in 0..repeat.get() {
            let start = Instant::now();
            self.proxy.ping().await?;
            let dur = Instant::now() - start;
            total += dur;
            max = max.max(dur);
            min = min.min(dur);
        }

        Ok(PingReport { max, avg: total / repeat.get(), min })
    }

    pub async fn socket(
        &self,
        params: SocketTransferParams,
    ) -> Result<SocketTransferReport, ClientError> {
        let SocketTransferParams { direction, params } = params;
        let (client, server) = self.proxy.domain().create_stream_socket();
        let transfer = socket::Transfer { socket: client, params: params.clone() };
        let (server_report, client_report) = match direction {
            Direction::Tx => {
                let server_fut = self
                    .proxy
                    .socket_down(server, &params.clone().try_into()?)
                    .map_err(ClientError::from);
                let client_fut = transfer.send().map_err(ClientError::from);
                futures::future::try_join(server_fut, client_fut).await?
            }
            Direction::Rx => {
                let server_fut = self
                    .proxy
                    .socket_up(server, &params.clone().try_into()?)
                    .map_err(ClientError::from);
                let client_fut = transfer.receive().map_err(ClientError::from);
                futures::future::try_join(server_fut, client_fut).await?
            }
        };
        Ok(SocketTransferReport {
            direction,
            client: SocketTransferReportInner::new(&params, client_report),
            server: SocketTransferReportInner::new(&params, server_report.try_into()?),
        })
    }
}

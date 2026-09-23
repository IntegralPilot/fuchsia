// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use fuchsia_async::{self as fasync, TimeoutExt as _};
use futures::{FutureExt as _, SinkExt as _, TryFutureExt as _, TryStreamExt as _};
use net_types::ip::{Ipv4, Ipv6};
use std::net::SocketAddr;

const PING_MESSAGE: &str = "Hello from reachability monitor!";
const SEQ_MIN: u16 = 1;
const SEQ_MAX: u16 = 3;
const TIMEOUT: fasync::MonotonicDuration = fasync::MonotonicDuration::from_seconds(1);

#[derive(Debug, thiserror::Error)]
pub enum PingError {
    #[error("failed to create socket")]
    CreateSocket(#[source] std::io::Error),
    #[error("failed to bind socket to device {interface_name}")]
    BindSocket {
        interface_name: String,
        #[source]
        err: std::io::Error,
    },
    #[error("failed to send ping (seq={seq})")]
    SendPing {
        seq: u16,
        #[source]
        err: ping::PingError,
    },
    #[error("timed out sending ping (seq={seq})")]
    SendPingTimeout { seq: u16 },
    #[error("failed to receive ping")]
    ReceivePing(#[source] ping::PingError),
    #[error("ping reply stream ended unexpectedly")]
    StreamEndedUnexpectedly,
    #[error("received unexpected ping sequence number; got: {got}, want: {min}..={max}")]
    UnexpectedSequenceNumber { got: u16, min: u16, max: u16 },
    #[error("no ping reply received")]
    NoReply,
}

impl PingError {
    /// Returns a short, simplified string describing the error.
    /// Each string should only take at most 14 characters, else the row labels for the
    /// `gateway_ping_results` and `internet_ping_results` time series in internal visualization
    /// tool would be truncated.
    pub fn short_name(&self) -> String {
        let (name, os_err) = match self {
            Self::CreateSocket(err) => ("CreateSock", err.raw_os_error()),
            Self::BindSocket { err, .. } => ("BindSock", err.raw_os_error()),
            Self::SendPing { err, .. } => {
                let os_err = match err {
                    ping::PingError::Send(io) => io.raw_os_error(),
                    ping::PingError::Recv(io) => io.raw_os_error(),
                    _ => None,
                };
                ("SendPing", os_err)
            }
            Self::SendPingTimeout { .. } => return "SendTimeout".to_string(),
            Self::ReceivePing(err) => {
                let os_err = match err {
                    ping::PingError::Send(io) => io.raw_os_error(),
                    ping::PingError::Recv(io) => io.raw_os_error(),
                    _ => None,
                };
                ("RecvPing", os_err)
            }
            Self::StreamEndedUnexpectedly => return "StreamEnded".to_string(),
            Self::UnexpectedSequenceNumber { .. } => return "BadSeqNum".to_string(),
            Self::NoReply => return "NoReply".to_string(),
        };

        if let Some(code) = os_err { format!("{name}_{code}") } else { name.to_string() }
    }
}

pub(crate) fn ping_result_short_name(result: &Result<(), PingError>) -> String {
    match result {
        Ok(()) => "Success".to_string(),
        Err(e) => format!("e_{}", e.short_name()),
    }
}

async fn ping<I>(interface_name: &str, addr: I::SockAddr) -> Result<(), PingError>
where
    I: ping::FuchsiaIpExt,
    I::SockAddr: std::fmt::Display + Copy,
{
    let socket = ping::new_icmp_socket::<I>().map_err(PingError::CreateSocket)?;
    let () = socket
        .bind_device(Some(interface_name.as_bytes()))
        .map_err(|err| PingError::BindSocket { interface_name: interface_name.to_string(), err })?;
    let (mut sink, mut stream) = ping::new_unicast_sink_and_stream::<
        I,
        _,
        { PING_MESSAGE.len() + ping::ICMP_HEADER_LEN },
    >(&socket, &addr, PING_MESSAGE.as_bytes());

    for seq in SEQ_MIN..=SEQ_MAX {
        let deadline = fasync::MonotonicInstant::after(TIMEOUT);
        let () = sink
            .send(seq)
            .map_err(|err| PingError::SendPing { seq, err })
            .on_timeout(deadline, || Err(PingError::SendPingTimeout { seq }))
            .await?;
        if match stream.try_next().map(Some).on_timeout(deadline, || None).await {
            None => Ok(false),
            Some(Err(err)) => Err(PingError::ReceivePing(err)),
            Some(Ok(None)) => Err(PingError::StreamEndedUnexpectedly),
            Some(Ok(Some(got))) if got >= SEQ_MIN && got <= seq => Ok(true),
            Some(Ok(Some(got))) => {
                Err(PingError::UnexpectedSequenceNumber { got, min: SEQ_MIN, max: seq })
            }
        }? {
            return Ok(());
        }
    }
    Err(PingError::NoReply)
}

/// Trait that can send ICMP echo requests, and receive and validate replies.
#[async_trait]
pub trait Ping {
    /// Returns `Ok(())` if the address is reachable, or a `PingError` otherwise.
    async fn ping(&self, interface_name: &str, addr: SocketAddr) -> Result<(), PingError>;
}

pub struct Pinger;

#[async_trait]
impl Ping for Pinger {
    async fn ping(&self, interface_name: &str, addr: SocketAddr) -> Result<(), PingError> {
        match addr {
            SocketAddr::V4(addr_v4) => ping::<Ipv4>(interface_name, addr_v4).await,
            SocketAddr::V6(addr_v6) => ping::<Ipv6>(interface_name, addr_v6).await,
        }
    }
}

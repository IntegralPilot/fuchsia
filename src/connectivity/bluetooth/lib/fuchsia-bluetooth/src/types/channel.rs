// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::{ClientEnd, Proxy};
use fidl_fuchsia_bluetooth as fidl_bt;
use fidl_fuchsia_bluetooth_bredr as bredr;
use fuchsia_sync::Mutex;
use futures::sink::Sink;
use futures::stream::{FusedStream, Stream};
use futures::{Future, StreamExt};
use log::warn;
use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use crate::error::Error;

pub mod fidl_client;
pub mod fidl_server;
pub mod socket;

use fidl_client::FidlClientConnection;
use fidl_server::FidlServerConnection;
use socket::SocketConnection;

/// The maximum size of a FIDL channel message is 64KB. We use 60KB as a safe limit
/// to leave headroom for serialization overhead and other message headers.
pub(crate) const MAX_BATCH_SIZE_BYTES: usize = 60 * 1024;

/// Estimated overhead per packet in a batched FIDL Send/Receive request.
/// (16 bytes vector header + up to 8 bytes padding).
pub(crate) const PACKET_OVERHEAD: usize = 24;

/// The Channel mode in use for a L2CAP channel.
#[derive(PartialEq, Debug, Clone)]
pub enum ChannelMode {
    Basic,
    EnhancedRetransmissionMode,
    LeCreditBasedFlowControl,
    EnhancedCreditBasedFlowControl,
}

impl fmt::Display for ChannelMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChannelMode::Basic => write!(f, "Basic"),
            ChannelMode::EnhancedRetransmissionMode => write!(f, "ERTM"),
            ChannelMode::LeCreditBasedFlowControl => write!(f, "LE_Credit"),
            ChannelMode::EnhancedCreditBasedFlowControl => write!(f, "Credit"),
        }
    }
}

pub enum A2dpDirection {
    Normal,
    Source,
    Sink,
}

impl From<A2dpDirection> for bredr::A2dpDirectionPriority {
    fn from(pri: A2dpDirection) -> Self {
        match pri {
            A2dpDirection::Normal => bredr::A2dpDirectionPriority::Normal,
            A2dpDirection::Source => bredr::A2dpDirectionPriority::Source,
            A2dpDirection::Sink => bredr::A2dpDirectionPriority::Sink,
        }
    }
}

impl TryFrom<fidl_bt::ChannelMode> for ChannelMode {
    type Error = Error;
    fn try_from(fidl: fidl_bt::ChannelMode) -> Result<Self, Error> {
        match fidl {
            fidl_bt::ChannelMode::Basic => Ok(ChannelMode::Basic),
            fidl_bt::ChannelMode::EnhancedRetransmission => {
                Ok(ChannelMode::EnhancedRetransmissionMode)
            }
            fidl_bt::ChannelMode::LeCreditBasedFlowControl => {
                Ok(ChannelMode::LeCreditBasedFlowControl)
            }
            fidl_bt::ChannelMode::EnhancedCreditBasedFlowControl => {
                Ok(ChannelMode::EnhancedCreditBasedFlowControl)
            }
            x => Err(Error::FailedConversion(format!("Unsupported channel mode type: {x:?}"))),
        }
    }
}

impl From<ChannelMode> for fidl_bt::ChannelMode {
    fn from(x: ChannelMode) -> Self {
        match x {
            ChannelMode::Basic => fidl_bt::ChannelMode::Basic,
            ChannelMode::EnhancedRetransmissionMode => fidl_bt::ChannelMode::EnhancedRetransmission,
            ChannelMode::LeCreditBasedFlowControl => fidl_bt::ChannelMode::LeCreditBasedFlowControl,
            ChannelMode::EnhancedCreditBasedFlowControl => {
                fidl_bt::ChannelMode::EnhancedCreditBasedFlowControl
            }
        }
    }
}

#[derive(PartialEq, Clone, Copy, Debug)]
pub enum ConnectionBackendType {
    Socket,
    FidlClient,
    FidlServer,
}

/// A trait representing a Bluetooth data connection.
/// Concrete implementations handle the specific transport mechanism (e.g., socket or FIDL protocol)
/// while fulfilling the `Sink` and `Stream` contracts for data transfer.
pub trait Connection:
    Stream<Item = Result<Vec<u8>, zx::Status>>
    + Sink<Vec<u8>, Error = zx::Status>
    + Send
    + Sync
    + std::fmt::Debug
    + Unpin
{
    /// Returns a future that resolves when the connection is closed.
    fn closed(&self) -> Pin<Box<dyn Future<Output = Result<(), zx::Status>> + Send + 'static>>;

    /// Returns the type of the connection backend.
    fn connection_type(&self) -> ConnectionBackendType;

    /// Writes data to the connection. This is a non-blocking fast path.
    /// Returns `SHOULD_WAIT` if the buffer is full.
    fn write(&self, bytes: &[u8]) -> Result<usize, zx::Status>;

    /// Returns true if the connection is currently closed.
    fn is_closed(&self) -> bool;

    /// Consumes the connection and returns a partially filled FIDL channel
    /// containing the transport (e.g., socket handle) if applicable.
    fn into_fidl_channel(self: Box<Self>) -> Result<bredr::Channel, zx::Status>;
}

/// A wrapper for Bluetooth channel. Profiles interact with this struct.
#[derive(Debug)]
pub struct Channel {
    pub(crate) connection: Box<dyn Connection>,
    mode: ChannelMode,
    max_tx_size: usize,
    flush_timeout: Arc<Mutex<Option<zx::MonotonicDuration>>>,
    audio_direction_ext: Option<bredr::AudioDirectionExtProxy>,
    l2cap_parameters_ext: Option<bredr::L2capParametersExtProxy>,
    audio_offload_ext: Option<bredr::AudioOffloadExtProxy>,
    terminated: bool,
}

impl Channel {
    pub const DEFAULT_MAX_TX: usize = 672;

    pub fn from_socket(socket: zx::Socket, max_tx_size: usize) -> Result<Self, zx::Status> {
        let connection = Box::new(SocketConnection::new(socket));
        Ok(Channel {
            connection,
            mode: ChannelMode::Basic,
            max_tx_size,
            flush_timeout: Arc::new(Mutex::new(None)),
            audio_direction_ext: None,
            l2cap_parameters_ext: None,
            audio_offload_ext: None,
            terminated: false,
        })
    }

    pub fn from_fidl_client(proxy: fidl_bt::ChannelProxy, max_tx_size: usize) -> Self {
        let connection = Box::new(FidlClientConnection::new(proxy, max_tx_size));
        Channel {
            connection,
            mode: ChannelMode::Basic,
            max_tx_size,
            flush_timeout: Arc::new(Mutex::new(None)),
            audio_direction_ext: None,
            l2cap_parameters_ext: None,
            audio_offload_ext: None,
            terminated: false,
        }
    }

    pub fn from_fidl_server(
        request_stream: fidl_bt::ChannelRequestStream,
        max_tx_size: usize,
    ) -> Self {
        let connection = Box::new(FidlServerConnection::new(request_stream, max_tx_size));
        Channel {
            connection,
            mode: ChannelMode::Basic,
            max_tx_size,
            flush_timeout: Arc::new(Mutex::new(None)),
            audio_direction_ext: None,
            l2cap_parameters_ext: None,
            audio_offload_ext: None,
            terminated: false,
        }
    }

    pub fn from_socket_infallible(socket: zx::Socket, max_tx_size: usize) -> Self {
        Self::from_socket(socket, max_tx_size).unwrap()
    }

    pub fn create_socket_pair() -> (Self, Self) {
        Self::create_socket_pair_with_max_tx(Self::DEFAULT_MAX_TX)
    }

    pub fn create_socket_pair_with_max_tx(max_tx_size: usize) -> (Self, Self) {
        let (remote, local) = zx::Socket::create_datagram();
        (
            Channel::from_socket(remote, max_tx_size).unwrap(),
            Channel::from_socket(local, max_tx_size).unwrap(),
        )
    }

    pub fn max_tx_size(&self) -> usize {
        self.max_tx_size
    }

    pub fn connection_type(&self) -> ConnectionBackendType {
        self.connection.connection_type()
    }

    pub fn channel_mode(&self) -> &ChannelMode {
        &self.mode
    }

    pub fn flush_timeout(&self) -> Option<zx::MonotonicDuration> {
        self.flush_timeout.lock().clone()
    }

    pub fn closed(&self) -> impl Future<Output = Result<(), zx::Status>> + Send + 'static {
        self.connection.closed()
    }

    pub fn is_closed(&self) -> bool {
        self.connection.is_closed()
    }

    pub fn write(&self, bytes: &[u8]) -> Result<usize, zx::Status> {
        self.connection.write(bytes)
    }

    pub fn set_audio_priority(
        &self,
        dir: A2dpDirection,
    ) -> impl Future<Output = Result<(), Error>> + use<> {
        let proxy = self.audio_direction_ext.clone();
        async move {
            match proxy {
                None => return Err(Error::profile("audio priority not supported")),
                Some(proxy) => proxy
                    .set_priority(dir.into())
                    .await?
                    .map_err(|e| Error::profile(format!("setting priority failed: {e:?}"))),
            }
        }
    }

    pub fn set_flush_timeout(
        &self,
        duration: Option<zx::MonotonicDuration>,
    ) -> impl Future<Output = Result<Option<zx::MonotonicDuration>, Error>> + use<> {
        let flush_timeout = self.flush_timeout.clone();
        let current = self.flush_timeout.lock().clone();
        let proxy = self.l2cap_parameters_ext.clone();
        async move {
            match (current, duration) {
                (None, None) => return Ok(None),
                (Some(old), Some(new)) if (old - new).into_millis().abs() < 2 => {
                    return Ok(current);
                }
                _ => {}
            };
            let proxy =
                proxy.ok_or_else(|| Error::profile("l2cap parameter changing not supported"))?;
            let parameters = fidl_bt::ChannelParameters {
                flush_timeout: duration.clone().map(zx::MonotonicDuration::into_nanos),
                ..Default::default()
            };
            let new_params = proxy.request_parameters(&parameters).await?;
            let new_timeout = new_params.flush_timeout.map(zx::MonotonicDuration::from_nanos);
            *(flush_timeout.lock()) = new_timeout.clone();
            Ok(new_timeout)
        }
    }

    pub fn audio_offload(&self) -> Option<bredr::AudioOffloadExtProxy> {
        self.audio_offload_ext.clone()
    }
}

impl Stream for Channel {
    type Item = Result<Vec<u8>, zx::Status>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.terminated {
            warn!("Stream was polled after termination");
            return Poll::Ready(None);
        }
        let res = this.connection.poll_next_unpin(cx);
        if let Poll::Ready(None) = res {
            this.terminated = true;
        }
        res
    }
}

impl FusedStream for Channel {
    fn is_terminated(&self) -> bool {
        self.terminated
    }
}

impl Sink<Vec<u8>> for Channel {
    type Error = zx::Status;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut *self.get_mut().connection).poll_ready(cx)
    }

    fn start_send(self: Pin<&mut Self>, item: Vec<u8>) -> Result<(), Self::Error> {
        Pin::new(&mut *self.get_mut().connection).start_send(item)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut *self.get_mut().connection).poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut *self.get_mut().connection).poll_close(cx)
    }
}

impl TryFrom<Channel> for bredr::Channel {
    type Error = Error;

    fn try_from(channel: Channel) -> Result<Self, Self::Error> {
        let mut fidl_channel = channel
            .connection
            .into_fidl_channel()
            .map_err(|e| Error::profile(format!("Failed to convert to FIDL channel: {e:?}")))?;

        fidl_channel.channel_mode = Some(channel.mode.into());
        fidl_channel.max_tx_sdu_size = Some(channel.max_tx_size as u16);

        let flush_timeout = channel.flush_timeout.lock().clone();
        fidl_channel.flush_timeout = flush_timeout.map(zx::MonotonicDuration::into_nanos);

        fidl_channel.ext_direction = channel
            .audio_direction_ext
            .map(|proxy| {
                let chan = proxy.into_channel()?;
                Ok(ClientEnd::new(chan.into()))
            })
            .transpose()
            .map_err(|_: bredr::AudioDirectionExtProxy| {
                Error::profile("AudioDirection proxy in use")
            })?;

        fidl_channel.ext_l2cap = channel
            .l2cap_parameters_ext
            .map(|proxy| {
                let chan = proxy.into_channel()?;
                Ok(ClientEnd::new(chan.into()))
            })
            .transpose()
            .map_err(|_: bredr::L2capParametersExtProxy| {
                Error::profile("l2cap parameters proxy in use")
            })?;

        fidl_channel.ext_audio_offload = channel
            .audio_offload_ext
            .map(|proxy| {
                let chan = proxy.into_channel()?;
                Ok(ClientEnd::new(chan.into()))
            })
            .transpose()
            .map_err(|_: bredr::AudioOffloadExtProxy| {
                Error::profile("audio offload proxy in use")
            })?;

        Ok(fidl_channel)
    }
}

impl TryFrom<fidl_fuchsia_bluetooth_bredr::Channel> for Channel {
    type Error = zx::Status;

    fn try_from(fidl: bredr::Channel) -> Result<Self, Self::Error> {
        let mode = match fidl.channel_mode.unwrap_or(fidl_bt::ChannelMode::Basic).try_into() {
            Err(e) => {
                warn!("Unsupported channel mode type: {e:?}");
                return Err(zx::Status::INTERNAL);
            }
            Ok(c) => c,
        };

        let max_tx_size = fidl.max_tx_sdu_size.ok_or(zx::Status::INVALID_ARGS)? as usize;

        let connection: Box<dyn Connection> = if let Some(conn) = fidl.connection {
            let proxy = conn.into_proxy();
            Box::new(FidlClientConnection::new(proxy, max_tx_size)) as Box<dyn Connection>
        } else if let Some(socket) = fidl.socket {
            Box::new(SocketConnection::new(socket)) as Box<dyn Connection>
        } else {
            return Err(zx::Status::INVALID_ARGS);
        };

        Ok(Self {
            connection,
            mode,
            max_tx_size,
            flush_timeout: Arc::new(Mutex::new(
                fidl.flush_timeout.map(zx::MonotonicDuration::from_nanos),
            )),
            audio_direction_ext: fidl.ext_direction.map(|e| e.into_proxy()),
            l2cap_parameters_ext: fidl.ext_l2cap.map(|e| e.into_proxy()),
            audio_offload_ext: fidl.ext_audio_offload.map(|c| c.into_proxy()),
            terminated: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::create_request_stream;
    use fidl_fuchsia_bluetooth as fidl_bt;
    use fidl_fuchsia_bluetooth_bredr as bredr;
    use fuchsia_async as fasync;
    use futures::StreamExt;
    use std::pin::pin;

    fn build_socket_bredr_channel() -> (bredr::Channel, zx::Socket) {
        let (remote, local) = zx::Socket::create_datagram();
        (
            bredr::Channel {
                socket: Some(remote),
                channel_mode: Some(fidl_bt::ChannelMode::Basic),
                max_tx_sdu_size: Some(1004),
                ..Default::default()
            },
            local,
        )
    }

    #[test]
    fn direction_ext() {
        let mut exec = fasync::TestExecutor::new();

        let (no_ext, _local) = build_socket_bredr_channel();
        let channel = Channel::try_from(no_ext).unwrap();

        assert!(
            exec.run_singlethreaded(channel.set_audio_priority(A2dpDirection::Normal)).is_err()
        );
        assert!(exec.run_singlethreaded(channel.set_audio_priority(A2dpDirection::Sink)).is_err());

        let (mut ext, _local) = build_socket_bredr_channel();
        let (client_end, mut direction_request_stream) =
            create_request_stream::<bredr::AudioDirectionExtMarker>();
        ext.ext_direction = Some(client_end);

        let channel = Channel::try_from(ext).unwrap();

        let audio_direction_fut = channel.set_audio_priority(A2dpDirection::Normal);
        let mut audio_direction_fut = pin!(audio_direction_fut);

        assert!(exec.run_until_stalled(&mut audio_direction_fut).is_pending());

        match exec.run_until_stalled(&mut direction_request_stream.next()) {
            Poll::Ready(Some(Ok(bredr::AudioDirectionExtRequest::SetPriority {
                priority,
                responder,
            }))) => {
                assert_eq!(bredr::A2dpDirectionPriority::Normal, priority);
                responder.send(Ok(())).expect("response to send cleanly");
            }
            x => panic!("Expected a item to be ready on the request stream, got {:?}", x),
        };

        match exec.run_until_stalled(&mut audio_direction_fut) {
            Poll::Ready(Ok(())) => {}
            _x => panic!("Expected ok result from audio direction response"),
        };

        let audio_direction_fut = channel.set_audio_priority(A2dpDirection::Sink);
        let mut audio_direction_fut = pin!(audio_direction_fut);

        assert!(exec.run_until_stalled(&mut audio_direction_fut).is_pending());

        match exec.run_until_stalled(&mut direction_request_stream.next()) {
            Poll::Ready(Some(Ok(bredr::AudioDirectionExtRequest::SetPriority {
                priority,
                responder,
            }))) => {
                assert_eq!(bredr::A2dpDirectionPriority::Sink, priority);
                responder
                    .send(Err(fidl_fuchsia_bluetooth::ErrorCode::Failed))
                    .expect("response to send cleanly");
            }
            x => panic!("Expected a item to be ready on the request stream, got {:?}", x),
        };

        match exec.run_until_stalled(&mut audio_direction_fut) {
            Poll::Ready(Err(_)) => {}
            _x => panic!("Expected error result from audio direction response"),
        };
    }

    #[test]
    fn flush_timeout() {
        let mut exec = fasync::TestExecutor::new();

        let (mut no_ext, _local) = build_socket_bredr_channel();
        no_ext.flush_timeout = Some(50_000_000); // 50 milliseconds
        let channel = Channel::try_from(no_ext).unwrap();

        assert_eq!(Some(zx::MonotonicDuration::from_millis(50)), channel.flush_timeout());

        // Within 2 milliseconds, doesn't change.
        let res = exec.run_singlethreaded(
            channel.set_flush_timeout(Some(zx::MonotonicDuration::from_millis(49))),
        );
        assert_eq!(Some(zx::MonotonicDuration::from_millis(50)), res.expect("shouldn't error"));
        let res = exec.run_singlethreaded(
            channel.set_flush_timeout(Some(zx::MonotonicDuration::from_millis(51))),
        );
        assert_eq!(Some(zx::MonotonicDuration::from_millis(50)), res.expect("shouldn't error"));

        assert!(
            exec.run_singlethreaded(
                channel.set_flush_timeout(Some(zx::MonotonicDuration::from_millis(200)))
            )
            .is_err()
        );
        assert!(exec.run_singlethreaded(channel.set_flush_timeout(None)).is_err());

        let (mut ext, _local) = build_socket_bredr_channel();
        let (client_end, mut l2cap_request_stream) =
            create_request_stream::<bredr::L2capParametersExtMarker>();
        ext.ext_l2cap = Some(client_end);

        let channel = Channel::try_from(ext).unwrap();

        {
            let flush_timeout_fut = channel.set_flush_timeout(None);
            let mut flush_timeout_fut = pin!(flush_timeout_fut);

            // Requesting no change returns right away with no change.
            match exec.run_until_stalled(&mut flush_timeout_fut) {
                Poll::Ready(Ok(None)) => {}
                x => panic!("Expected no flush timeout to not stall, got {:?}", x),
            }
        }

        let req_duration = zx::MonotonicDuration::from_millis(42);

        {
            let flush_timeout_fut = channel.set_flush_timeout(Some(req_duration));
            let mut flush_timeout_fut = pin!(flush_timeout_fut);

            assert!(exec.run_until_stalled(&mut flush_timeout_fut).is_pending());

            match exec.run_until_stalled(&mut l2cap_request_stream.next()) {
                Poll::Ready(Some(Ok(bredr::L2capParametersExtRequest::RequestParameters {
                    request,
                    responder,
                }))) => {
                    assert_eq!(Some(req_duration.into_nanos()), request.flush_timeout);
                    // Send a different response
                    let params = fidl_bt::ChannelParameters {
                        flush_timeout: Some(50_000_000), // 50ms
                        ..Default::default()
                    };
                    responder.send(&params).expect("response to send cleanly");
                }
                x => panic!("Expected a item to be ready on the request stream, got {:?}", x),
            };

            match exec.run_until_stalled(&mut flush_timeout_fut) {
                Poll::Ready(Ok(Some(duration))) => {
                    assert_eq!(zx::MonotonicDuration::from_millis(50), duration)
                }
                x => panic!("Expected ready result from params response, got {:?}", x),
            };
        }

        // Channel should have recorded the new flush timeout.
        assert_eq!(Some(zx::MonotonicDuration::from_millis(50)), channel.flush_timeout());
    }

    #[test]
    fn audio_offload() {
        let _exec = fasync::TestExecutor::new();

        let (no_ext, _local) = build_socket_bredr_channel();
        let channel = Channel::try_from(no_ext).unwrap();

        assert!(channel.audio_offload().is_none());

        let (mut ext, _local) = build_socket_bredr_channel();
        let (client_end, mut _audio_offload_ext_req_stream) =
            create_request_stream::<bredr::AudioOffloadExtMarker>();
        ext.ext_audio_offload = Some(client_end);

        let channel = Channel::try_from(ext).unwrap();

        let offload_ext = channel.audio_offload();
        assert!(offload_ext.is_some());
        // We can get the audio offload multiple times without dropping
        assert!(channel.audio_offload().is_some());
        // And with dropping
        drop(offload_ext);
        assert!(channel.audio_offload().is_some());
    }

    #[test]
    fn channel_from_fidl_priority() {
        let _exec = fasync::TestExecutor::new();

        // Case 1: Both FIDL connection and socket are present.
        // FIDL connection should be preferred over socket.
        let (client_end, _server_end) =
            fidl::endpoints::create_endpoints::<fidl_bt::ChannelMarker>();
        let (mut fidl_both, _socket_local) = build_socket_bredr_channel();
        fidl_both.connection = Some(client_end);

        let chan = Channel::try_from(fidl_both).expect("to convert successfully");
        assert_eq!(chan.connection.connection_type(), ConnectionBackendType::FidlClient);

        // Case 2: Only socket is present.
        // Should fall back to traditional socket transport.
        let (socket_only, _socket_local) = build_socket_bredr_channel();

        let chan = Channel::try_from(socket_only).expect("to convert successfully");
        assert_eq!(chan.connection.connection_type(), ConnectionBackendType::Socket);

        // Case 3: Neither is present.
        // Should fail to convert as we need at least one transport.
        let fidl_empty = bredr::Channel {
            channel_mode: Some(fidl_bt::ChannelMode::Basic),
            max_tx_sdu_size: Some(1004),
            ..Default::default()
        };
        assert!(Channel::try_from(fidl_empty).is_err());
    }
}

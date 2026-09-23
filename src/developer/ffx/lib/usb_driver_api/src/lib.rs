// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_ffx_usb_common::{
    self as usb_fidl, ConnectionId, control_ordinals, ffx_usb_ordinals, list_devices_ordinals,
};
use fidl_message::{MaybeUnknown, TransactionHeader};
use fuchsia_async as fasync;
use futures::channel::oneshot;
use futures::lock::Mutex;
use futures::stream::repeat_with;
use futures::{Stream, StreamExt};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex as SyncMutex};
use std::task::{Poll, Waker};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt, WriteHalf};
use tokio::net::UnixStream;

/// Protocol version we expect to use to communicate with the driver.
const CURRENT_VERSION: u32 = 0;

/// Config element for the path of the socket we will use to communicate.
pub const CONFIG_USB_SOCKET_PATH: &str = "connectivity.usb_socket_path";

#[derive(Debug, Error)]
#[error("$XDG_RUNTIME_DIR is not set")]
pub struct XdgRuntimeDirNotSet;

/// Errors that occur while establishing a USB VSOCK connection.
#[derive(Debug, Error)]
pub enum ConnectError {
    #[error(transparent)]
    ProtocolError(#[from] ProtocolError),
    #[error("Invalid CID: {0}")]
    CidInvalid(u32),
    #[error("CID {0} Not Found")]
    CidNotFound(u32),
    #[error("Connection failed: {0}")]
    Failed(String),
    #[error("Port {0} in use")]
    PortInUse(u32),
    #[error("Port {0} out of range")]
    PortOutOfRange(u32),
}

impl From<usb_fidl::ConnectionError> for ConnectError {
    fn from(value: usb_fidl::ConnectionError) -> Self {
        match value {
            usb_fidl::ConnectionError::CidInvalid(usb_fidl::CidInvalid { cid }) => {
                ConnectError::CidInvalid(cid)
            }
            usb_fidl::ConnectionError::CidNotFound(usb_fidl::CidNotFound { cid }) => {
                ConnectError::CidNotFound(cid)
            }
            usb_fidl::ConnectionError::Failed(usb_fidl::Failed { message }) => {
                ConnectError::Failed(message)
            }
            usb_fidl::ConnectionError::PortInUse(usb_fidl::PortInUse { port }) => {
                ConnectError::PortInUse(port)
            }
            usb_fidl::ConnectionError::PortOutOfRange(usb_fidl::PortOutOfRange { port }) => {
                ConnectError::PortOutOfRange(port)
            }
            _ => ConnectError::Failed("undecodable error".into()),
        }
    }
}

impl From<ReadMessageError> for ConnectError {
    fn from(value: ReadMessageError) -> Self {
        ConnectError::from(ProtocolError::from(value))
    }
}

/// Error messages from `Device::read_message`. These shouldn't make it out to public APIs.
enum ReadMessageError {
    IO(std::io::Error),
    Fidl(fidl::Error),
}

/// Errors that occur while accepting an incoming a USB VSOCK connection.
#[derive(Debug, Error)]
pub enum AcceptError {
    #[error(transparent)]
    ProtocolError(#[from] ProtocolError),
    #[error("Accepting connection failed: {0}")]
    Failed(String),
}

impl From<ReadMessageError> for AcceptError {
    fn from(value: ReadMessageError) -> Self {
        AcceptError::from(ProtocolError::from(value))
    }
}

/// Errors that occur while rejecting an incoming a USB VSOCK connection.
#[derive(Debug, Error)]
enum RejectError {
    #[error(transparent)]
    ProtocolError(#[from] ProtocolError),
    #[error("Connection did not exist: {0:?}")]
    NoSuchConnection(usb_fidl::ConnectionId),
    #[error("Unknown error occurred")]
    Unknown,
}

/// Errors that occur while trying to listen for USB VSOCK connections.
#[derive(Debug, Error)]
pub enum ListenError {
    #[error(transparent)]
    ProtocolError(#[from] ProtocolError),
    #[error("Port {0} in use")]
    PortInUse(u32),
    #[error("Unknown error occurred")]
    Unknown,
}

/// Errors that occur while trying to listen for USB VSOCK connections.
#[derive(Debug, Error)]
pub enum StopListenError {
    #[error(transparent)]
    ProtocolError(#[from] ProtocolError),
    #[error("Not previously listening on port {0}")]
    NotListening(u32),
    #[error("Unknown error occurred")]
    Unknown,
}

/// Errors that occur while communicating with the USB driver.
#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("Could not connect to driver: {0}")]
    Connect(std::io::Error),
    #[error("FIDL protocol error: {0}")]
    Fidl(fidl::Error),
    #[error("Could not send request message to driver: {0}")]
    Write(std::io::Error),
    #[error("Could not receive request reply from driver: {0}")]
    Read(std::io::Error),
    #[error(
        "Version mismatch (Driver is version {0} and supports down to {1}, we are version {CURRENT_VERSION})"
    )]
    VersionMismatch(u32, u32),
}

impl Clone for ProtocolError {
    fn clone(&self) -> Self {
        fn clone_io_error(e: &std::io::Error) -> std::io::Error {
            if let Some(e) = e.raw_os_error() {
                std::io::Error::from_raw_os_error(e)
            } else if let Some(internal) = e.get_ref() {
                std::io::Error::new(e.kind(), internal.to_string())
            } else {
                unreachable!("io::Error of unknown construction");
            }
        }

        match self {
            Self::Connect(arg0) => Self::Connect(clone_io_error(&arg0)),
            Self::Fidl(arg0) => Self::Fidl(arg0.clone()),
            Self::Write(arg0) => Self::Write(clone_io_error(&arg0)),
            Self::Read(arg0) => Self::Read(clone_io_error(&arg0)),
            Self::VersionMismatch(arg0, arg1) => Self::VersionMismatch(arg0.clone(), arg1.clone()),
        }
    }
}

impl From<ReadMessageError> for ProtocolError {
    fn from(value: ReadMessageError) -> Self {
        match value {
            ReadMessageError::IO(error) => ProtocolError::Read(error),
            ReadMessageError::Fidl(error) => ProtocolError::Fidl(error),
        }
    }
}

/// How `Driver::stop_listening` should handle incoming connections that are already queued.
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum StopListenQueueBehavior {
    Keep,
    Reject,
}

/// Errors that occur when listening for devices.
#[derive(Debug, Error)]
pub enum DeviceListenError {
    #[error("IO error: {0}")]
    IO(std::io::Error),
    #[error("FIDL error: {0}")]
    Fidl(fidl::Error),
}

impl From<ReadMessageError> for DeviceListenError {
    fn from(value: ReadMessageError) -> Self {
        match value {
            ReadMessageError::IO(error) => DeviceListenError::IO(error),
            ReadMessageError::Fidl(error) => DeviceListenError::Fidl(error),
        }
    }
}

/// A FIDL transaction header and the body of the message it comes with.
struct FidlMessage {
    header: TransactionHeader,
    body: Vec<u8>,
}

impl FidlMessage {
    /// Decode the transaction header from a FIDL message and separate it from
    /// the rest of the body.
    fn from_bytes(bytes: &[u8]) -> Result<Self, ReadMessageError> {
        let (header, body) =
            fidl::encoding::decode_transaction_header(bytes).map_err(ReadMessageError::Fidl)?;
        let body = body.to_vec();

        Ok(FidlMessage { header, body })
    }

    /// Decode this message.
    fn decode_message<T>(self) -> Result<T, ReadMessageError>
    where
        T: fidl_message::Body,
        <T::MarkerAtTopLevel as fidl::encoding::TypeMarker>::Owned:
            fidl::encoding::Decode<T::MarkerAtTopLevel, fidl::encoding::NoHandleResourceDialect>,
    {
        fidl_message::decode_message::<T>(self.header, &self.body).map_err(ReadMessageError::Fidl)
    }

    /// Decode this message as a flexible response.
    fn decode_response_flexible<T>(self) -> Result<MaybeUnknown<T>, ProtocolError>
    where
        T: fidl_message::Body,
        <T::MarkerInResultUnion as fidl::encoding::TypeMarker>::Owned:
            fidl::encoding::Decode<T::MarkerInResultUnion, fidl::encoding::NoHandleResourceDialect>,
    {
        fidl_message::decode_response_flexible::<T>(self.header, &self.body)
            .map_err(ProtocolError::Fidl)
    }

    /// Decode this message as a flexible response with a potential error value.
    fn decode_response_flexible_result<T, E>(
        self,
    ) -> Result<MaybeUnknown<Result<T, E>>, ProtocolError>
    where
        T: fidl_message::Body,
        E: fidl_message::ErrorType,
        <T::MarkerInResultUnion as fidl::encoding::TypeMarker>::Owned:
            fidl::encoding::Decode<T::MarkerInResultUnion, fidl::encoding::NoHandleResourceDialect>,
    {
        fidl_message::decode_response_flexible_result::<T, E>(self.header, &self.body)
            .map_err(ProtocolError::Fidl)
    }
}

/// Represents an instance of the USB driver that we are communicating with.
pub struct Driver {
    path: PathBuf,
    session_id: u64,
    next_txid: AtomicU32,
    control_writer: Mutex<WriteHalf<UnixStream>>,
    handlers: Arc<SyncMutex<HashMap<u32, oneshot::Sender<Result<FidlMessage, ProtocolError>>>>>,
    incoming: Arc<SyncMutex<(VecDeque<usb_fidl::ConnectionId>, Waker)>>,
    _control_conn_scope: fasync::Scope,
}

impl Driver {
    /// Connect to the USB driver so that we can access USB devices.
    pub async fn init(path: impl AsRef<Path>) -> Result<Self, ProtocolError> {
        let mut control_conn = UnixStream::connect(&path).await.map_err(ProtocolError::Connect)?;
        Driver::send_flexible_request(
            &mut control_conn,
            ffx_usb_ordinals::INITIALIZE_CONTROL,
            1,
            (),
        )
        .await?;

        let res = Driver::read_response(1, &mut control_conn)
            .await?
            .decode_response_flexible::<usb_fidl::FfxUsbInitializeControlResponse>()?;

        let MaybeUnknown::Known(res) = res else {
            return Err(ProtocolError::Fidl(fidl::Error::UnsupportedMethod {
                method_name: "InitializeControl",
                protocol_name: "fuchsia.ffx.usb.FfxUsb",
            }));
        };
        let control_conn_scope = fasync::Scope::new_with_name("USB Driver control socket handler");

        let incoming = Arc::new(SyncMutex::new((VecDeque::new(), Waker::noop().clone())));
        let incoming_read = Arc::clone(&incoming);
        let handlers = Arc::new(SyncMutex::new(HashMap::new()));
        let handlers_read = Arc::clone(&handlers);
        let (control_read, control_writer) = tokio::io::split(control_conn);
        control_conn_scope.spawn(async move {
            let Err(e) = Driver::handle_control_stream_read(
                control_read,
                Arc::clone(&handlers_read),
                incoming_read,
            )
            .await;

            for (_, handler) in handlers_read.lock().unwrap().drain() {
                let _ = handler.send(Err(e.clone()));
            }
        });

        if res.minimum > CURRENT_VERSION {
            Err(ProtocolError::VersionMismatch(res.current, res.minimum))
        } else {
            log::info!("Connected to USB driver which logs to {}", res.log_path);
            Ok(Driver {
                path: path.as_ref().into(),
                next_txid: AtomicU32::new(1),
                session_id: res.session_id,
                handlers,
                control_writer: Mutex::new(control_writer),
                incoming,
                _control_conn_scope: control_conn_scope,
            })
        }
    }

    /// Continuous task that handles incoming control stream messages. These can
    /// either be replies to requests, or incoming connection events.
    async fn handle_control_stream_read(
        mut stream: impl AsyncReadExt + Unpin,
        handlers: Arc<SyncMutex<HashMap<u32, oneshot::Sender<Result<FidlMessage, ProtocolError>>>>>,
        incoming: Arc<SyncMutex<(VecDeque<usb_fidl::ConnectionId>, Waker)>>,
    ) -> Result<std::convert::Infallible, ProtocolError> {
        loop {
            let msg = Driver::read_message(&mut stream).await?;

            if msg.header.tx_id == 0 {
                if msg.header.ordinal != control_ordinals::ON_INCOMING {
                    log::warn!(
                        "Unrecognized event with ordinal {:?} on USB control protocol",
                        msg.header.ordinal
                    );
                    continue;
                }

                let conn_id = msg.decode_message::<usb_fidl::ConnectionId>()?;
                let mut incoming = incoming.lock().unwrap();
                incoming.0.push_back(conn_id);
                incoming.1.wake_by_ref();
            } else {
                let handler = handlers.lock().unwrap().remove(&msg.header.tx_id);

                if let Some(handler) = handler {
                    let _ = handler.send(Ok(msg));
                } else {
                    log::warn!("Got response for unknown txid {}", msg.header.tx_id);
                }
            }
        }
    }

    /// Helper for reading a FIDL message out of a stream. Returns the
    /// transaction header and raw body.
    async fn read_message(
        stream: &mut (impl AsyncReadExt + Unpin),
    ) -> Result<FidlMessage, ReadMessageError> {
        let len = stream.read_u32_le().await.map_err(ReadMessageError::IO)?;
        let len = usize::try_from(len).unwrap();
        let mut buf = vec![0u8; len];
        stream.read_exact(&mut buf).await.map_err(ReadMessageError::IO)?;

        FidlMessage::from_bytes(&buf)
    }

    /// Like `read_message` but validates a particular transaction_id.
    async fn read_response(
        txid: u32,
        stream: &mut (impl AsyncReadExt + Unpin),
    ) -> Result<FidlMessage, ReadMessageError> {
        let msg = Driver::read_message(stream).await?;
        if msg.header.tx_id == txid {
            Ok(msg)
        } else {
            Err(ReadMessageError::Fidl(fidl::Error::InvalidResponseTxid))
        }
    }

    /// Helper for sending a flexible method request on a FIDL protocol stream.
    async fn send_flexible_request<T: fidl_message::Body>(
        stream: &mut (impl AsyncWriteExt + Unpin),
        ordinal: u64,
        txid: u32,
        message: T,
    ) -> Result<(), ProtocolError> {
        let header = fidl::encoding::TransactionHeader::new(
            txid,
            ordinal,
            fidl::encoding::DynamicFlags::FLEXIBLE,
        );
        let msg = fidl_message::encode_message(header, message).map_err(ProtocolError::Fidl)?;
        stream
            .write_u32_le(u32::try_from(msg.len()).unwrap())
            .await
            .map_err(ProtocolError::Write)?;
        stream.write_all(&msg).await.map_err(ProtocolError::Write)?;
        Ok(())
    }

    /// Returns a stream of [`DeviceEvent`] values telling us when USB devices
    /// appear and disappear. The stream will begin with an "appear" event for
    /// all currently-connected devices.
    pub async fn listen_for_devices(
        &self,
    ) -> Result<
        impl Stream<Item = Result<DeviceEvent, DeviceListenError>> + Send + 'static,
        ProtocolError,
    > {
        let mut stream = UnixStream::connect(&self.path).await.map_err(ProtocolError::Connect)?;
        Driver::send_flexible_request(
            &mut stream,
            ffx_usb_ordinals::INITIALIZE_LIST_DEVICES,
            1,
            (),
        )
        .await?;

        let res = Driver::read_response(1, &mut stream).await?.decode_response_flexible::<()>()?;

        if let fidl_message::MaybeUnknown::Unknown = res {
            return Err(ProtocolError::Fidl(fidl::Error::UnsupportedMethod {
                method_name: "InitializeListDevices",
                protocol_name: "fuchsia.ffx.usb.FfxUsb",
            }));
        }

        let stream = Arc::new(Mutex::new(stream));
        Ok(repeat_with(move || {
            let stream = Arc::clone(&stream);
            async move {
                let mut stream = stream.lock().await;
                let msg = Driver::read_message(&mut *stream).await?;
                if msg.header.tx_id != 0 {
                    return Err(DeviceListenError::Fidl(fidl::Error::InvalidRequestTxid));
                }
                match msg.header.ordinal {
                    list_devices_ordinals::ON_DEVICE_APPEARED => {
                        let info = msg.decode_message::<usb_fidl::DeviceInfo>()?;
                        Ok(DeviceEvent::Added { cid: info.cid, serial: info.meta.serial })
                    }
                    list_devices_ordinals::ON_DEVICE_DISAPPEARED => {
                        let response = msg
                            .decode_message::<usb_fidl::ListDevicesOnDeviceDisappearedRequest>()?;
                        Ok(DeviceEvent::Removed { cid: response.cid })
                    }
                    other => Err(DeviceListenError::Fidl(fidl::Error::UnknownOrdinal {
                        ordinal: other,
                        protocol_name: "fuchsia.ffx.usb.ListDevices",
                    })),
                }
            }
        })
        .then(|f| f))
    }

    /// Establish a USB VSOCK connection to the given `cid` and `port`.
    pub async fn connect(&self, cid: u32, port: u32) -> Result<UnixStream, ConnectError> {
        let mut stream = UnixStream::connect(&self.path).await.map_err(ProtocolError::Connect)?;
        let req = usb_fidl::FfxUsbInitializeConnectToRequest { cid, port };
        Driver::send_flexible_request(&mut stream, ffx_usb_ordinals::INITIALIZE_CONNECT_TO, 1, req)
            .await?;

        let res = Driver::read_response(1, &mut stream)
            .await?
            .decode_response_flexible_result::<(), usb_fidl::ConnectionError>()?;

        let MaybeUnknown::Known(res) = res else {
            return Err(ProtocolError::Fidl(fidl::Error::UnsupportedMethod {
                method_name: "InitializeConnectTo",
                protocol_name: "fuchsia.ffx.usb.FfxUsb",
            })
            .into());
        };

        res?;
        Ok(stream)
    }

    /// Start listening for incoming connections on a particular port.
    pub async fn listen(&self, port: u32) -> Result<(), ListenError> {
        let message = usb_fidl::ControlListenRequest { port };
        let txid = self.next_txid.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        let got = self.handlers.lock().unwrap().insert(txid, sender);
        assert!(got.is_none(), "Transaction ID allocated twice!");

        {
            let mut stream = self.control_writer.lock().await;
            Driver::send_flexible_request(&mut *stream, control_ordinals::LISTEN, txid, message)
                .await?;
        }

        let res = receiver
            .await
            .map_err(|_| ListenError::Unknown)??
            .decode_response_flexible_result::<(), usb_fidl::ListenError>()?;

        let MaybeUnknown::Known(res) = res else {
            return Err(ProtocolError::Fidl(fidl::Error::UnsupportedMethod {
                method_name: "Listen",
                protocol_name: "fuchsia.ffx.usb.Control",
            })
            .into());
        };

        match res {
            Ok(()) => Ok(()),
            Err(usb_fidl::ListenError::PortInUse(usb_fidl::PortInUse { port })) => {
                Err(ListenError::PortInUse(port))
            }
            Err(_) => Err(ListenError::Unknown),
        }
    }

    /// Reject an incoming connection. This is an internal method that assumes
    /// the connection ID given has already been taken off of the queue.
    async fn reject(&self, to_reject: ConnectionId) -> Result<(), RejectError> {
        let txid = self.next_txid.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        let got = self.handlers.lock().unwrap().insert(txid, sender);
        assert!(got.is_none(), "Transaction ID allocated twice!");

        {
            let mut stream = self.control_writer.lock().await;
            Driver::send_flexible_request(&mut *stream, control_ordinals::REJECT, txid, to_reject)
                .await?;
        }

        let res = receiver
            .await
            .map_err(|_| RejectError::Unknown)??
            .decode_response_flexible_result::<(), usb_fidl::RejectError>()?;

        let MaybeUnknown::Known(res) = res else {
            return Err(ProtocolError::Fidl(fidl::Error::UnsupportedMethod {
                method_name: "Reject",
                protocol_name: "fuchsia.ffx.usb.Control",
            })
            .into());
        };

        match res {
            Ok(()) => Ok(()),
            Err(usb_fidl::RejectError::NoSuchConnection(cid)) => {
                Err(RejectError::NoSuchConnection(cid))
            }
            Err(_) => Err(RejectError::Unknown),
        }
    }

    /// Stop listening for incoming connections on a particular port.
    pub async fn stop_listening(
        &self,
        port: u32,
        queue_behavior: StopListenQueueBehavior,
    ) -> Result<(), StopListenError> {
        let message = usb_fidl::ControlStopListenRequest { port };
        let txid = self.next_txid.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        let got = self.handlers.lock().unwrap().insert(txid, sender);
        assert!(got.is_none(), "Transaction ID allocated twice!");

        {
            let mut stream = self.control_writer.lock().await;
            Driver::send_flexible_request(
                &mut *stream,
                control_ordinals::STOP_LISTEN,
                txid,
                message,
            )
            .await?;
        }

        let res = receiver
            .await
            .map_err(|_| StopListenError::Unknown)??
            .decode_response_flexible_result::<(), usb_fidl::StopListenError>()?;

        let MaybeUnknown::Known(res) = res else {
            return Err(ProtocolError::Fidl(fidl::Error::UnsupportedMethod {
                method_name: "StopListen",
                protocol_name: "fuchsia.ffx.usb.Control",
            })
            .into());
        };

        match res {
            Ok(()) => {
                if let StopListenQueueBehavior::Reject = queue_behavior {
                    let to_reject = {
                        let mut incoming = self.incoming.lock().unwrap();
                        let mut to_reject = Vec::with_capacity(incoming.0.len());

                        incoming.0.retain(|x| {
                            if x.local_port == port {
                                to_reject.push(*x);
                                false
                            } else {
                                true
                            }
                        });

                        to_reject
                    };

                    for to_reject in to_reject {
                        if let Err(e) = self.reject(to_reject).await {
                            log::warn!("Failed to reject connection: {e:?}");
                        }
                    }
                }
                Ok(())
            }
            Err(usb_fidl::StopListenError::NotListening(usb_fidl::NotListening { port })) => {
                Err(StopListenError::NotListening(port))
            }
            Err(_) => Err(StopListenError::Unknown),
        }
    }

    /// Accept the next connection we are listening for.
    pub async fn accept_next(&self) -> Result<(UnixStream, usb_fidl::ConnectionId), AcceptError> {
        loop {
            let conn = futures::future::poll_fn(|cx| {
                let mut queue = self.incoming.lock().unwrap();

                if let Some(conn) = queue.0.pop_front() {
                    Poll::Ready(conn)
                } else {
                    queue.1 = cx.waker().clone();
                    Poll::Pending
                }
            })
            .await;

            let mut stream =
                UnixStream::connect(&self.path).await.map_err(ProtocolError::Connect)?;
            let req = usb_fidl::FfxUsbInitializeAcceptRequest { conn, session_id: self.session_id };
            Driver::send_flexible_request(&mut stream, ffx_usb_ordinals::INITIALIZE_ACCEPT, 1, req)
                .await?;

            let res = Driver::read_response(1, &mut stream)
                .await?
                .decode_response_flexible_result::<(), usb_fidl::AcceptError>()?;

            let MaybeUnknown::Known(res) = res else {
                return Err(ProtocolError::Fidl(fidl::Error::UnsupportedMethod {
                    method_name: "InitializeAccept",
                    protocol_name: "fuchsia.ffx.usb.FfxUsb",
                })
                .into());
            };

            return match res {
                Ok(()) => Ok((stream, conn)),
                Err(usb_fidl::AcceptError::NoSuchConnection(cid)) => {
                    log::warn!(
                        "Driver didn't recognize an incoming connection ID it previously sent us: {cid:?}"
                    );
                    continue;
                }
                Err(_) => Err(AcceptError::Failed("undecodable error".into())),
            };
        }
    }
}

/// An event indicating a new USB device has appeared or a device has
/// disappeared.
#[derive(Debug, Clone)]
pub enum DeviceEvent {
    Added { cid: u32, serial: Option<String> },
    Removed { cid: u32 },
}

#[cfg(test)]
mod test {
    use super::*;
    use usb_driver_impl::HostDriver;
    use usb_vsock::CID_HOST;
    use usb_vsock_host::TestConnection;

    #[fuchsia::test]
    async fn test_list_devices() {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("test_sock");
        let TestConnection {
            cid,
            connection: _connection,
            serial: _,
            incoming_requests: _,
            abort_transfer: _,
            scope: _scope,
        } = HostDriver::new_for_test(sock_path.clone());

        let driver = Driver::init(sock_path).await.unwrap();
        let dev = std::pin::pin!(driver.listen_for_devices().await.unwrap())
            .next()
            .await
            .unwrap()
            .unwrap();

        let DeviceEvent::Added { cid: got_cid, serial: _ } = dev else { panic!() };

        assert_eq!(cid, got_cid);
    }

    #[fuchsia::test]
    async fn test_connect() {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("test_sock");
        let TestConnection {
            cid,
            connection,
            serial: _,
            mut incoming_requests,
            abort_transfer: _,
            scope,
        } = HostDriver::new_for_test(sock_path.clone());

        let driver = Arc::new(Driver::init(sock_path).await.unwrap());

        let driver_clone = Arc::clone(&driver);
        let got_conn = scope.compute_local(async move { driver_clone.connect(cid, 1234).await });

        let incoming = incoming_requests.next().await.unwrap();

        let addr = incoming.address();

        assert_eq!(cid, addr.device_cid);
        assert_eq!(CID_HOST, addr.host_cid);
        assert_eq!(1234, addr.device_port);

        let (mut a, other_end) = UnixStream::pair().unwrap();
        let _state = connection.accept(incoming, other_end.into()).await.unwrap();
        let mut b = got_conn.await.unwrap();

        static TEST_STR_1: &[u8] = b"Sometimes I could just really use a Tuesday, y'know?";
        static TEST_STR_2: &[u8] =
            b"Alas that we are theme park animatronics, and our existence is inherently whimsical.";

        a.write_all(TEST_STR_1).await.unwrap();
        let mut buf = [0u8; TEST_STR_1.len()];
        b.read_exact(&mut buf).await.unwrap();
        assert_eq!(TEST_STR_1, &buf);

        b.write_all(TEST_STR_2).await.unwrap();
        let mut buf = [0u8; TEST_STR_2.len()];
        a.read_exact(&mut buf).await.unwrap();
        assert_eq!(TEST_STR_2, &buf);
    }

    #[fuchsia::test]
    async fn test_listen() {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("test_sock");
        let TestConnection {
            cid,
            connection,
            serial: _,
            incoming_requests: _,
            abort_transfer: _,
            scope,
        } = HostDriver::new_for_test(sock_path.clone());

        let driver = Driver::init(sock_path).await.unwrap();
        driver.listen(1234).await.unwrap();

        let connection = Arc::new(connection);
        let connection_clone = Arc::clone(&connection);
        let (mut a, other_end) = UnixStream::pair().unwrap();
        let got_conn = scope.compute_local(async move {
            connection_clone
                .connect(
                    usb_vsock::Address {
                        device_cid: cid,
                        device_port: 9090,
                        host_cid: CID_HOST,
                        host_port: 1234,
                    },
                    other_end.into(),
                )
                .await
        });

        let (mut b, conn_id) = driver.accept_next().await.unwrap();

        let _a_conn_state = got_conn.await.unwrap();

        assert_eq!(cid, conn_id.remote_cid);
        assert_eq!(9090, conn_id.remote_port);
        assert_eq!(1234, conn_id.local_port);

        static TEST_STR_1: &[u8] = b"Sometimes I could just really use a Tuesday, y'know?";
        static TEST_STR_2: &[u8] =
            b"Alas that we are theme park animatronics, and our existence is inherently whimsical.";

        a.write_all(TEST_STR_1).await.unwrap();
        let mut buf = [0u8; TEST_STR_1.len()];
        b.read_exact(&mut buf).await.unwrap();
        assert_eq!(TEST_STR_1, &buf);

        b.write_all(TEST_STR_2).await.unwrap();
        let mut buf = [0u8; TEST_STR_2.len()];
        a.read_exact(&mut buf).await.unwrap();
        assert_eq!(TEST_STR_2, &buf);

        driver.stop_listening(1234, StopListenQueueBehavior::Reject).await.unwrap();

        let (_unused, other_end) = UnixStream::pair().unwrap();
        assert!(
            connection
                .connect(
                    usb_vsock::Address {
                        device_cid: cid,
                        device_port: 9091,
                        host_cid: CID_HOST,
                        host_port: 1234,
                    },
                    other_end.into()
                )
                .await
                .is_err()
        );
    }
}

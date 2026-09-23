// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error, anyhow};
use fidl_fuchsia_bluetooth_pandora::{
    RootcanalClientControllerRequest, RootcanalClientControllerRequestStream, ServiceError,
};
use fidl_fuchsia_hardware_bluetooth::{
    VirtualControllerCreateLoopbackDeviceRequest, VirtualControllerMarker,
};
use fuchsia_async::net::TcpStream;
use fuchsia_async::{self as fasync};
use fuchsia_bluetooth::constants::DEV_DIR;
use fuchsia_component::server::ServiceFs;
use fuchsia_sync::Mutex;

use futures::future::Either;
use futures::io::{ReadHalf, WriteHalf};
use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, StreamExt, TryFutureExt};
use std::net::{IpAddr, SocketAddr};
use std::pin::pin;
use std::str::FromStr;
use std::sync::Arc;

// Across all three link types, ACL has the largest frame at 1028. Add a byte of UART header.
const UART_MAX_FRAME_BUFFER_SIZE: usize = 1029;

/// The name of the test node to which bt_hci_virtual can expect to bind, i.e. the first positional
/// argument in `ffx driver test-node add ______ fuchsia.COMPATIBLE=bt`.
fn emulator_test_node_name() -> String {
    "bt-hci-emulator".to_string()
}

// Read from `read_stream` exactly as many bytes as fit in `buf`. Returns error if EOF encountered
// before that number of bytes can be read.
async fn read_exact(
    read_stream: &mut ReadHalf<impl AsyncRead>,
    buf: &mut [u8],
) -> Result<(), Error> {
    read_stream.read_exact(buf).await.map_err(|e| anyhow!("Unable to read channel {:?}", e))
}

// Read next HCI packet from `read_stream` into `buf`. Assumes that the last field of the packet
// header at octet position `length_field_pos` encodes the length of the subsequent data segment
// beginning at octet position `data_segment_start`. Only supports length fields of 1 or 2 octets.
//
// Returns the size of the HCI packet.
async fn read_hci_packet(
    mut read_stream: &mut ReadHalf<impl AsyncRead>,
    buf: &mut [u8],
    length_field_pos: usize,
    data_segment_start: usize,
) -> Result<usize, Error> {
    // Read header.
    read_exact(&mut read_stream, &mut buf[..data_segment_start]).await?;

    let data_total_length = match data_segment_start - length_field_pos {
        1 => buf[length_field_pos] as usize,
        2 => u16::from_le_bytes(buf[length_field_pos..data_segment_start].try_into().unwrap())
            as usize,
        _ => return Err(anyhow!("Cannot read length fields greater than 2 octets")),
    };

    // Read data.
    read_exact(
        &mut read_stream,
        &mut buf[data_segment_start..data_segment_start + data_total_length],
    )
    .await?;
    Ok(data_segment_start + data_total_length)
}

/// Reads the TCP stream from the host from the `read_stream` and writes all data to the loopback
/// driver over `channel`.
async fn stream_reader(
    mut read_stream: ReadHalf<impl AsyncRead>,
    channel: &fasync::Channel,
) -> Result<(), Error> {
    let mut buf = [0u8; UART_MAX_FRAME_BUFFER_SIZE];
    let mut handles = Vec::new();
    loop {
        // Read H4 packet type byte.
        read_exact(&mut read_stream, &mut buf[..1]).await?;

        let hci_packet_size = match buf[0] {
            // HCI ACL packet.
            2 => {
                read_hci_packet(
                    &mut read_stream,
                    &mut buf[1..],
                    /*length_field_pos=*/ 2,
                    /*data_segment_start=*/ 4,
                )
                .await?
            }
            // HCI Event packet.
            4 => {
                read_hci_packet(
                    &mut read_stream,
                    &mut buf[1..],
                    /*length_field_pos=*/ 1,
                    /*data_segment_start=*/ 2,
                )
                .await?
            }
            unsupported_type => {
                return Err(anyhow!("Received unsupported packet type: {}", unsupported_type));
            }
        };

        channel
            .write(&buf[0..hci_packet_size + 1], &mut handles)
            .map_err(|e| anyhow!("Unable to write to emulator channel {:?}", e))?;
    }
}

/// Reads the `channel` from the loopback device and writes all data to TCP stream to the host
/// over the `write_stream`.
async fn channel_reader(
    mut write_stream: WriteHalf<impl AsyncWrite>,
    channel: &fasync::Channel,
) -> Result<(), Error> {
    loop {
        let mut buffer = zx::MessageBuf::new();
        channel
            .recv_msg(&mut buffer)
            .await
            .map_err(|e| anyhow!("Error read from channel {:?}", e))?;
        let _size = write_stream
            .write(buffer.bytes())
            .await
            .map_err(|e| anyhow!("Unable to write to TCP stream {:?}", e))?;
    }
}

/// Opens the virtual loopback device, creates a channel to pass to it and returns that channel.
async fn open_virtual_device(control_device: &str) -> Result<fasync::Channel, Error> {
    let dev_directory =
        fuchsia_fs::directory::open_in_namespace(DEV_DIR, fuchsia_fs::PERM_READABLE)
            .expect("unable to open directory");

    let controller = device_watcher::recursive_wait_and_open::<VirtualControllerMarker>(
        &dev_directory,
        control_device,
    )
    .await
    .with_context(|| format!("failed to open {}", control_device))?;

    let (remote_channel, local_channel) = zx::Channel::create();
    let request = VirtualControllerCreateLoopbackDeviceRequest {
        uart_channel: Some(remote_channel),
        __source_breaking: fidl::marker::SourceBreaking,
    };
    controller.create_loopback_device(request)?;
    Ok(fasync::Channel::from_channel(local_channel))
}

enum ClientTask {
    None,
    Starting,
    Running { _task: fasync::Task<Result<(), Error>> },
}

impl ClientTask {
    // Set state to Starting if it is previously None. Return Err otherwise.
    fn set_starting(&mut self) -> Result<(), (ServiceError, Error)> {
        if matches!(self, Self::None) {
            *self = Self::Starting;
            return Ok(());
        }
        return Err((ServiceError::AlreadyRunning, anyhow!("Rootcanal task already running")));
    }

    // Set state to None if it is previously Running, clearing the contained Task.
    fn stop(&mut self) {
        *self = Self::None;
    }
}

/// Abstracts a connection to a Rootcanal server.
struct RootcanalClient {
    task: Mutex<ClientTask>,
}

impl RootcanalClient {
    pub fn new() -> Self {
        RootcanalClient { task: Mutex::new(ClientTask::None) }
    }

    /// Connect to Rootcanal server at `socket_addr` & loopback device and begin proxying data between them.
    async fn connect(&self, socket_addr: SocketAddr) -> Result<(), (ServiceError, Error)> {
        self.task.lock().set_starting()?;

        log::debug!("Opening host {}", socket_addr);
        let connector_res = TcpStream::connect(socket_addr);
        let Ok(tcp_connector) = connector_res else {
            return Err((ServiceError::ConnectionFailed, connector_res.unwrap_err().into()));
        };
        let stream_res = tcp_connector.await;
        let Ok(tcp_stream) = stream_res else {
            return Err((ServiceError::ConnectionFailed, stream_res.unwrap_err().into()));
        };
        log::debug!("Connected");

        let channel_res =
            open_virtual_device(&(emulator_test_node_name() + "/bt_hci_virtual")).await;
        let Ok(channel) = channel_res else {
            return Err((ServiceError::Failed, channel_res.unwrap_err().into()));
        };

        *self.task.lock() = ClientTask::Running {
            _task: fuchsia_async::Task::spawn(Self::run(tcp_stream, channel)),
        };

        Ok(())
    }

    /// Disconnect this client from the server if connected.
    async fn disconnect(&self) {
        self.task.lock().stop();
    }

    /// Run reader futures on both ends.
    async fn run(
        stream: impl AsyncRead + AsyncWrite + Sized,
        channel: fasync::Channel,
    ) -> Result<(), Error> {
        let (read_stream, write_stream) = stream.split();

        let chan_fut = pin!(channel_reader(write_stream, &channel));

        let stream_fut = pin!(stream_reader(read_stream, &channel));

        match futures::future::select(chan_fut, stream_fut).await {
            Either::Left((res, _)) => res,
            Either::Right((res, _)) => res,
        }
    }
}

async fn run_fidl_server(
    mut stream: RootcanalClientControllerRequestStream,
    rootcanal_client: Arc<RootcanalClient>,
) -> Result<(), Error> {
    while let Ok(request) = stream.next().await.context("failed FIDL request")? {
        match request {
            // ffx bluetooth pandora start --rootcanal-ip |ip| --rootcanal-port |port|
            RootcanalClientControllerRequest::Start { payload, responder, .. } => {
                let ip_res = IpAddr::from_str(&payload.ip.unwrap());
                let Ok(ip) = ip_res else {
                    let _ = responder.send(Err(ServiceError::InvalidIp));
                    return Err(ip_res.unwrap_err().into());
                };
                let socket_addr: SocketAddr = (ip, payload.port.unwrap()).into();

                if let Err(err) = rootcanal_client.connect(socket_addr).await {
                    let _ = responder.send(Err(err.0));
                    return Err(err.1);
                }
                let _ = responder.send(Ok(()));
            }

            // ffx bluetooth pandora stop
            RootcanalClientControllerRequest::Stop { responder } => {
                rootcanal_client.disconnect().await;
                let _ = responder.send();
            }

            _ => return Err(anyhow!("unknown FIDL request")),
        }
    }
    Ok(())
}

#[fuchsia::main(logging_tags = ["bt-rootcanal"])]
async fn main() -> Result<(), Error> {
    let mut fs = ServiceFs::new_local();
    let _ = fs.dir("svc").add_fidl_service(|s: RootcanalClientControllerRequestStream| s);
    let _ = fs.take_and_serve_directory_handle()?;

    log::debug!("Listening for incoming Rootcanal FIDL connections...");
    let rootcanal_client = Arc::new(RootcanalClient::new());
    fs.for_each(|stream| {
        run_fidl_server(stream, Arc::clone(&rootcanal_client))
            .unwrap_or_else(|e| log::info!("FIDL server encountered an error: {:?}", e))
    })
    .await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::task::Poll;

    #[fuchsia::test]
    fn test_bidirectional_comms() {
        let mut exec = fasync::TestExecutor::new();

        // Mock channel setup
        let (txc, rxc) = zx::Channel::create();
        let async_channel = fasync::Channel::from_channel(rxc);

        let (txs, rxs) = zx::Socket::create_stream();
        let async_socket = fasync::Socket::from_socket(rxs);

        let mut fut = Box::pin(RootcanalClient::run(async_socket, async_channel));

        // Run with nothing to read yet. Futures should be waiting on both streams.
        assert!(exec.run_until_stalled(&mut fut).is_pending());

        // Write to the channel
        let mut handles = Vec::new();
        let bytes = [0x01, 0x02, 0x03, 0x04];
        txc.write(&bytes, &mut handles).expect("write failed");

        // Pump to read bytes from channel and write to the socket.
        assert!(exec.run_until_stalled(&mut fut).is_pending());

        // Read from the socket
        let mut read_buf: [u8; 4] = [0xff, 0xff, 0xff, 0xff];
        assert_eq!(txs.read(&mut read_buf).expect("unable to read"), 4);
        assert_eq!(bytes, read_buf);

        // Write HCI Event to the socket
        let bytes = [0x04, 0x01, 0x02, 0x16, 0x17];
        assert_eq!(txs.write(&bytes).expect("write failed"), 5);

        // Pump to read bytes from socket and write to the channel.
        assert!(exec.run_until_stalled(&mut fut).is_pending());

        // Read from the channel
        let mut buffer = zx::MessageBuf::new();
        txc.read(&mut buffer).expect("unable to read");
        assert_eq!(bytes, buffer.bytes());

        // Drop channel and expect the futures to return.
        txs.half_close().expect("should close");
        drop(txs);
        match exec.run_until_stalled(&mut fut) {
            Poll::Ready(Err(_)) => {}
            _ => {
                assert!(false, "still pending");
            }
        };
    }
}

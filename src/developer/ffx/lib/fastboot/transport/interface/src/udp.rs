// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use byteorder::{BigEndian, ByteOrder};
use futures::Future;
use futures::io::{AsyncRead, AsyncWrite};
use futures::task::{Context, Poll};
use std::fmt;
use std::net::SocketAddr;
use std::num::Wrapping;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use timeout::timeout;
use tokio::net::UdpSocket;
use zerocopy::byteorder::big_endian::U16;
use zerocopy::{FromBytes, Immutable, KnownLayout, Ref, SplitByteSlice, Unaligned};

const HOST_PORT: u16 = 5554;
const REPLY_TIMEOUT: Duration = Duration::from_millis(500);
const MAX_SIZE: u16 = 2048; // Maybe handle larger?

#[derive(Debug, PartialEq, Eq)]
enum PacketType {
    Error,
    Query,
    Init,
    Fastboot,
}

#[derive(KnownLayout, FromBytes, Immutable, Unaligned, Debug, PartialEq, Eq)]
#[repr(C)]
struct Header {
    id: u8,
    flags: u8,
    sequence: U16,
}

struct Packet<B: SplitByteSlice> {
    header: Ref<B, Header>,
    data: B,
}

impl<B: SplitByteSlice> Packet<B> {
    fn parse(bytes: B) -> Option<Packet<B>> {
        let (header, data) = Ref::from_prefix(bytes).ok()?;
        Some(Self { header, data })
    }

    #[allow(dead_code)]
    fn is_continuation(&self) -> bool {
        self.header.flags & 0x001 != 0
    }

    fn packet_type(&self) -> Result<PacketType, crate::FastbootTransportError> {
        match self.header.id {
            0x00 => Ok(PacketType::Error),
            0x01 => Ok(PacketType::Query),
            0x02 => Ok(PacketType::Init),
            0x03 => Ok(PacketType::Fastboot),
            _ => Err(crate::FastbootTransportError::ParseError),
        }
    }
}

///////////////////////////////////////////////////////////////////////////////
// UdpNetworkInterface
//

pub struct UdpNetworkInterface {
    maximum_size: u16,
    sequence: Wrapping<u16>,
    socket: Arc<UdpSocket>,
    read_task: Option<Pin<Box<dyn Future<Output = std::io::Result<(usize, Vec<u8>)>> + Send>>>,
    write_task: Option<Pin<Box<dyn Future<Output = std::io::Result<usize>> + Send>>>,
}

impl fmt::Debug for UdpNetworkInterface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UdpNetworkInterface")
            .field("maximum_size", &self.maximum_size)
            .field("sequence", &self.sequence)
            .field("socket", &self.socket)
            .finish()
    }
}

impl UdpNetworkInterface {
    fn create_fastboot_packets(
        &mut self,
        buf: &[u8],
    ) -> Result<Vec<Vec<u8>>, crate::FastbootTransportError> {
        // Leave space for the header; each packet must carry at least one byte of payload.
        let header_size = std::mem::size_of::<Header>() as u16;
        if self.maximum_size <= header_size {
            return Err(crate::FastbootTransportError::InvalidHandshake);
        }
        let max_chunk_size = self.maximum_size - header_size;
        let mut seq = self.sequence;
        let mut result = Vec::new();
        let mut iter = buf.chunks(max_chunk_size.into()).peekable();
        while let Some(chunk) = iter.next() {
            let mut packet: Vec<u8> = Vec::with_capacity(chunk.len() + header_size as usize);
            packet.push(0x03);
            if iter.peek().is_none() {
                packet.push(0x00);
            } else {
                packet.push(0x01); // Mark as continuation.
            }
            for _ in 0..2 {
                packet.push(0);
            }
            BigEndian::write_u16(&mut packet[2..4], seq.0);
            seq += Wrapping(1u16);
            packet.extend_from_slice(chunk);
            result.push(packet);
        }
        Ok(result)
    }
}

impl AsyncRead for UdpNetworkInterface {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.read_task.is_none() {
            let socket = self.socket.clone();
            let seq = self.sequence;
            self.read_task.replace(Box::pin(async move {
                let (out_buf, sz) = send_to_device(&make_empty_fastboot_packet(seq.0), &socket)
                    .await
                    .map_err(|e| {
                        std::io::Error::other(format!(
                            "Could not send empty fastboot packet to device: {}",
                            e
                        ))
                    })?;
                let packet = Packet::parse(&out_buf[..sz]).ok_or_else(|| {
                    std::io::Error::other(format!("Could not parse response packet"))
                })?;
                let mut buf_inner = Vec::new();
                match packet.packet_type() {
                    Ok(PacketType::Fastboot) => {
                        let size = packet.data.len();
                        buf_inner.extend(packet.data);
                        Ok((size, buf_inner))
                    }
                    Ok(PacketType::Error) => {
                        let msg = String::from_utf8_lossy(&packet.data);
                        Err(std::io::Error::other(format!("Device returned error: {msg}")))
                    }
                    _ => Err(std::io::Error::other(format!("Unexpected reply from device"))),
                }
            }));
        }

        if let Some(ref mut task) = self.read_task {
            match task.as_mut().poll(cx) {
                Poll::Ready(Ok((sz, out_buf))) => {
                    self.read_task = None;
                    if sz > buf.len() {
                        return Poll::Ready(Err(std::io::Error::other(format!(
                            "Buffer too small: received {sz} bytes, but buffer capacity is {}",
                            buf.len()
                        ))));
                    }
                    buf[..sz].copy_from_slice(&out_buf[..sz]);
                    self.sequence += Wrapping(1u16);
                    Poll::Ready(Ok(sz))
                }
                Poll::Ready(Err(e)) => {
                    self.read_task = None;
                    Poll::Ready(Err(e))
                }
                Poll::Pending => Poll::Pending,
            }
        } else {
            // Really shouldn't get here
            Poll::Ready(Err(std::io::Error::other(format!("Could not add async task to read"))))
        }
    }
}

impl AsyncWrite for UdpNetworkInterface {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.write_task.is_none() {
            // TODO(https://fxbug.dev/42159159): unfortunately the Task requires the 'static lifetime so we have to
            // copy the bytes and move them into the async block.
            let packets = self.create_fastboot_packets(buf).map_err(|e| {
                std::io::Error::other(format!("Could not create fastboot packets: {}", e))
            })?;
            let socket = self.socket.clone();
            self.write_task.replace(Box::pin(async move {
                for packet in &packets {
                    let (out_buf, sz) = send_to_device(&packet, &socket).await.map_err(|e| {
                        std::io::Error::other(format!(
                            "Could not send fastboot packet to device: {}",
                            e
                        ))
                    })?;
                    let response = Packet::parse(&out_buf[..sz]).ok_or_else(|| {
                        std::io::Error::other(format!("Could not parse response packet"))
                    })?;
                    match response.packet_type() {
                        Ok(PacketType::Fastboot) => (),
                        Ok(PacketType::Error) => {
                            let msg = String::from_utf8_lossy(&response.data);
                            return Err(std::io::Error::other(format!(
                                "Device returned error: {msg}"
                            )));
                        }
                        _ => {
                            return Err(std::io::Error::other(format!(
                                "Unexpected Response packet"
                            )));
                        }
                    }
                }
                Ok(packets.len())
            }));
        }

        if let Some(ref mut task) = self.write_task {
            match task.as_mut().poll(cx) {
                Poll::Ready(Ok(s)) => {
                    self.write_task = None;
                    for _i in 0..s {
                        self.sequence += Wrapping(1u16);
                    }
                    Poll::Ready(Ok(buf.len()))
                }
                Poll::Ready(Err(e)) => {
                    self.write_task = None;
                    Poll::Ready(Err(e))
                }
                Poll::Pending => Poll::Pending,
            }
        } else {
            // Really shouldn't get here
            Poll::Ready(Err(std::io::Error::other(format!("Could not add async task to write"))))
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        unimplemented!();
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        unimplemented!();
    }
}

pub async fn open(addr: SocketAddr) -> Result<UdpNetworkInterface, crate::FastbootTransportError> {
    let mut to_sock: SocketAddr = addr.clone();
    // TODO(https://fxbug.dev/42159161): get the port from the mdns packet
    to_sock.set_port(HOST_PORT);
    let socket = make_sender_socket(to_sock).await?;
    let (buf, sz) = send_to_device(&make_query_packet(), &socket).await?;
    let packet = Packet::parse(&buf[..sz]).ok_or(crate::FastbootTransportError::ParseError)?;
    let sequence = match packet.packet_type() {
        Ok(PacketType::Query) => {
            if packet.data.len() < 2 {
                return Err(crate::FastbootTransportError::ParseError);
            }
            BigEndian::read_u16(&packet.data)
        }
        Ok(PacketType::Error) => {
            let msg = String::from_utf8_lossy(&packet.data);
            return Err(crate::FastbootTransportError::Io(std::io::Error::other(format!(
                "Device returned error: {msg}"
            ))));
        }
        _ => return Err(crate::FastbootTransportError::ParseError),
    };
    let (buf, sz) = send_to_device(&make_init_packet(sequence), &socket).await?;
    let packet = Packet::parse(&buf[..sz]).ok_or(crate::FastbootTransportError::ParseError)?;
    let (version, max) = match packet.packet_type() {
        Ok(PacketType::Init) => {
            if packet.data.len() < 4 {
                return Err(crate::FastbootTransportError::ParseError);
            }
            (BigEndian::read_u16(&packet.data[..2]), BigEndian::read_u16(&packet.data[2..4]))
        }
        Ok(PacketType::Error) => {
            let msg = String::from_utf8_lossy(&packet.data);
            return Err(crate::FastbootTransportError::Io(std::io::Error::other(format!(
                "Device returned error: {msg}"
            ))));
        }
        _ => return Err(crate::FastbootTransportError::ParseError),
    };
    // Leave space for the header; each packet must carry at least one byte of payload.
    let header_size = std::mem::size_of::<Header>() as u16;
    if max <= header_size {
        return Err(crate::FastbootTransportError::InvalidHandshake);
    }
    let maximum_size = std::cmp::min(max, MAX_SIZE);
    log::debug!(
        "Fastboot over UDP connection established. Version {}. Max Size: {}",
        version,
        maximum_size
    );

    Ok(UdpNetworkInterface {
        socket: Arc::new(socket),
        maximum_size,
        sequence: Wrapping(sequence + 1),
        read_task: None,
        write_task: None,
    })
}

async fn send_to_device(
    buf: &[u8],
    socket: &UdpSocket,
) -> Result<([u8; MAX_SIZE as usize], usize), crate::FastbootTransportError> {
    let (expected_id, expected_seq) = if let Some(packet) = Packet::parse(buf) {
        (Some(packet.header.id), Some(packet.header.sequence.get()))
    } else {
        (None, None)
    };

    // Try sending twice
    socket.send(buf).await.map_err(crate::FastbootTransportError::SendError)?;
    match wait_for_response(expected_id, expected_seq, socket).await {
        Ok(r) => Ok(r),
        Err(e) => {
            log::warn!("Could not get reply from Fastboot device - trying again: {}", e);
            socket.send(buf).await.map_err(crate::FastbootTransportError::SendError)?;
            wait_for_response(expected_id, expected_seq, socket).await
        }
    }
}

async fn wait_for_response(
    expected_id: Option<u8>,
    expected_seq: Option<u16>,
    socket: &UdpSocket,
) -> Result<([u8; MAX_SIZE as usize], usize), crate::FastbootTransportError> {
    let deadline = std::time::Instant::now() + REPLY_TIMEOUT;
    let mut buf = [0u8; MAX_SIZE as usize];
    loop {
        let now = std::time::Instant::now();
        if now >= deadline {
            return Err(crate::FastbootTransportError::Timeout);
        }
        let remaining = deadline - now;
        let size = timeout(remaining, Box::pin(socket.recv(&mut buf[..])))
            .await
            .map_err(|_| crate::FastbootTransportError::Timeout)?
            .map_err(crate::FastbootTransportError::RecvError)?;

        if let Some(packet) = Packet::parse(&buf[..size]) {
            let seq_matches = expected_seq.map_or(true, |seq| packet.header.sequence.get() == seq);
            let id_matches =
                expected_id.map_or(true, |id| packet.header.id == id || packet.header.id == 0x00);
            if seq_matches && id_matches {
                return Ok((buf, size));
            } else {
                log::debug!(
                    "Ignoring unexpected fastboot UDP packet: id=0x{:02x} (expected {:?}), seq={} (expected {:?})",
                    packet.header.id,
                    expected_id.map(|id| format!("0x{:02x}", id)),
                    packet.header.sequence.get(),
                    expected_seq,
                );
            }
        } else {
            log::debug!("Ignoring unparsable fastboot UDP packet of size {}", size);
        }
    }
}

async fn make_sender_socket(addr: SocketAddr) -> Result<UdpSocket, crate::FastbootTransportError> {
    let socket: std::net::UdpSocket = match addr {
        SocketAddr::V4(ref _saddr) => socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )
        .map_err(|e| crate::FastbootTransportError::Io(e))?,
        SocketAddr::V6(ref _saddr) => socket2::Socket::new(
            socket2::Domain::IPV6,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )
        .map_err(|e| crate::FastbootTransportError::Io(e))?,
    }
    .into();
    let result = UdpSocket::from_std(socket).map_err(|e| crate::FastbootTransportError::Io(e))?;
    result.connect(addr).await.map_err(|e| crate::FastbootTransportError::Io(e))?;
    Ok(result)
}

fn make_query_packet() -> [u8; 4] {
    let mut packet = [0u8; 4];
    packet[0] = 0x01;
    packet
}

fn make_init_packet(sequence: u16) -> [u8; 8] {
    let mut packet = [0u8; 8];
    packet[0] = 0x02;
    packet[1] = 0x00;
    BigEndian::write_u16(&mut packet[2..4], sequence);
    BigEndian::write_u16(&mut packet[4..6], 1);
    BigEndian::write_u16(&mut packet[6..8], MAX_SIZE);
    packet
}

fn make_empty_fastboot_packet(sequence: u16) -> [u8; 4] {
    let mut packet = [0u8; 4];
    packet[0] = 0x03;
    packet[1] = 0x00;
    BigEndian::write_u16(&mut packet[2..4], sequence);
    packet
}

#[cfg(test)]
mod test {
    use super::*;
    use anyhow::Result;
    use futures::{AsyncReadExt, AsyncWriteExt};
    use pretty_assertions::assert_eq;

    #[fuchsia::test]
    fn test_packet_parsing() {
        // Query packet
        let query_bytes = [0x01, 0x00, 0x00, 0x00, 0x12, 0x34];
        let packet = Packet::parse(&query_bytes[..]).expect("valid packet");
        assert_eq!(packet.header.id, 0x01);
        assert_eq!(packet.header.flags, 0x00);
        assert_eq!(packet.header.sequence.get(), 0x0000);
        assert_eq!(packet.packet_type().unwrap(), PacketType::Query);
        assert!(!packet.is_continuation());
        assert_eq!(packet.data, &[0x12, 0x34]);

        // Fastboot continuation packet
        let fb_bytes = [0x03, 0x01, 0x00, 0x2A, b't', b'e', b's', b't'];
        let packet = Packet::parse(&fb_bytes[..]).expect("valid packet");
        assert_eq!(packet.header.id, 0x03);
        assert_eq!(packet.header.flags, 0x01);
        assert_eq!(packet.header.sequence.get(), 42);
        assert_eq!(packet.packet_type().unwrap(), PacketType::Fastboot);
        assert!(packet.is_continuation());
        assert_eq!(packet.data, b"test");

        // Error packet
        let err_bytes = [0x00, 0x00, 0x00, 0x05, b'e', b'r', b'r'];
        let packet = Packet::parse(&err_bytes[..]).expect("valid packet");
        assert_eq!(packet.packet_type().unwrap(), PacketType::Error);
        assert_eq!(packet.header.sequence.get(), 5);
        assert_eq!(packet.data, b"err");

        // Short packet
        assert!(Packet::parse(&[0x01, 0x00, 0x00][..]).is_none());
    }

    #[fuchsia::test]
    async fn test_create_fastboot_packets() -> Result<()> {
        let socket = UdpSocket::bind("127.0.0.1:0").await?;
        let mut interface = UdpNetworkInterface {
            maximum_size: 10,
            sequence: Wrapping(100),
            socket: Arc::new(socket),
            read_task: None,
            write_task: None,
        };

        // Header is 4 bytes, max size is 10, so max chunk size is 6.
        let data = b"0123456789ABCDE"; // 15 bytes -> 6 + 6 + 3 (3 chunks)
        let packets = interface.create_fastboot_packets(data).expect("packets created");
        assert_eq!(packets.len(), 3);

        // Chunk 1: continuation flag set, seq = 100
        assert_eq!(packets[0][0], 0x03);
        assert_eq!(packets[0][1], 0x01); // continuation
        assert_eq!(BigEndian::read_u16(&packets[0][2..4]), 100);
        assert_eq!(&packets[0][4..], b"012345");

        // Chunk 2: continuation flag set, seq = 101
        assert_eq!(packets[1][0], 0x03);
        assert_eq!(packets[1][1], 0x01); // continuation
        assert_eq!(BigEndian::read_u16(&packets[1][2..4]), 101);
        assert_eq!(&packets[1][4..], b"6789AB");

        // Chunk 3: continuation flag NOT set (last chunk), seq = 102
        assert_eq!(packets[2][0], 0x03);
        assert_eq!(packets[2][1], 0x00);
        assert_eq!(BigEndian::read_u16(&packets[2][2..4]), 102);
        assert_eq!(&packets[2][4..], b"CDE");

        Ok(())
    }

    #[fuchsia::test]
    async fn test_wait_for_response_discards_stale_packets() -> Result<()> {
        let server = UdpSocket::bind("127.0.0.1:0").await?;
        let server_addr = server.local_addr()?;

        let client = UdpSocket::bind("127.0.0.1:0").await?;
        client.connect(server_addr).await?;
        let client_addr = client.local_addr()?;

        // Send a stale packet (seq 10), an unparsable packet, and then the expected packet (seq 11)
        let stale_packet = [0x03, 0x00, 0x00, 0x0A, b'o', b'l', b'd'];
        let bad_packet = [0x03, 0x00];
        let good_packet = [0x03, 0x00, 0x00, 0x0B, b'g', b'o', b'o', b'd'];

        server.send_to(&stale_packet, client_addr).await?;
        server.send_to(&bad_packet, client_addr).await?;
        server.send_to(&good_packet, client_addr).await?;

        // wait_for_response expecting seq 11 should skip the stale and bad packets
        let (buf, sz) = wait_for_response(Some(0x03), Some(11), &client).await?;
        let packet = Packet::parse(&buf[..sz]).expect("valid packet");
        assert_eq!(packet.header.sequence.get(), 11);
        assert_eq!(packet.data, b"good");

        Ok(())
    }

    #[fuchsia::test]
    async fn test_wait_for_response_accepts_error_packet() -> Result<()> {
        let server = UdpSocket::bind("127.0.0.1:0").await?;
        let server_addr = server.local_addr()?;

        let client = UdpSocket::bind("127.0.0.1:0").await?;
        client.connect(server_addr).await?;
        let client_addr = client.local_addr()?;

        // Send an Error packet with matching sequence
        let err_packet = [0x00, 0x00, 0x00, 0x15, b'f', b'a', b'i', b'l'];
        server.send_to(&err_packet, client_addr).await?;

        let (buf, sz) = wait_for_response(Some(0x03), Some(21), &client).await?;
        let packet = Packet::parse(&buf[..sz]).expect("valid packet");
        assert_eq!(packet.packet_type().unwrap(), PacketType::Error);
        assert_eq!(packet.header.sequence.get(), 21);
        assert_eq!(packet.data, b"fail");

        Ok(())
    }

    #[fuchsia::test]
    async fn test_udp_network_interface_read_and_write() -> Result<()> {
        let server = UdpSocket::bind("127.0.0.1:0").await?;
        let server_addr = server.local_addr()?;

        let client = UdpSocket::bind("127.0.0.1:0").await?;
        client.connect(server_addr).await?;

        let mut interface = UdpNetworkInterface {
            maximum_size: 512,
            sequence: Wrapping(50),
            socket: Arc::new(client),
            read_task: None,
            write_task: None,
        };

        // Server task: simulate device responding to write and read
        let _server_task = tokio::spawn(async move {
            let mut buf = [0u8; 1500];

            // 1. Receive write packet (seq 50)
            let (sz, peer) = server.recv_from(&mut buf).await.unwrap();
            let req = Packet::parse(&buf[..sz]).unwrap();
            assert_eq!(req.header.sequence.get(), 50);
            assert_eq!(req.data, b"DOWNLOAD");
            // Send ack for write (seq 50)
            let resp = [0x03, 0x00, 0x00, 0x32];
            server.send_to(&resp, peer).await.unwrap();

            // 2. Receive read polling packet (seq 51)
            let (sz, peer) = server.recv_from(&mut buf).await.unwrap();
            let req = Packet::parse(&buf[..sz]).unwrap();
            assert_eq!(req.header.sequence.get(), 51);
            // Send response data for read (seq 51)
            let resp = [0x03, 0x00, 0x00, 0x33, b'D', b'A', b'T', b'A'];
            server.send_to(&resp, peer).await.unwrap();
        });

        // Perform write
        interface.write_all(b"DOWNLOAD").await?;
        assert_eq!(interface.sequence.0, 51);

        // Perform read
        let mut read_buf = [0u8; 4];
        interface.read_exact(&mut read_buf).await?;
        assert_eq!(&read_buf, b"DATA");
        assert_eq!(interface.sequence.0, 52);

        Ok(())
    }

    #[fuchsia::test]
    async fn test_create_fastboot_packets_too_small_max_size() -> Result<()> {
        let socket = UdpSocket::bind("127.0.0.1:0").await?;
        let mut interface = UdpNetworkInterface {
            maximum_size: 4, // equals header size
            sequence: Wrapping(100),
            socket: Arc::new(socket),
            read_task: None,
            write_task: None,
        };
        assert!(interface.create_fastboot_packets(b"data").is_err());

        interface.maximum_size = 2; // smaller than header size
        assert!(interface.create_fastboot_packets(b"data").is_err());

        Ok(())
    }

    #[fuchsia::test]
    async fn test_udp_network_interface_read_buffer_too_small() -> Result<()> {
        let server = UdpSocket::bind("127.0.0.1:0").await?;
        let server_addr = server.local_addr()?;

        let client = UdpSocket::bind("127.0.0.1:0").await?;
        client.connect(server_addr).await?;

        let mut interface = UdpNetworkInterface {
            maximum_size: 512,
            sequence: Wrapping(50),
            socket: Arc::new(client),
            read_task: None,
            write_task: None,
        };

        let _server_task = tokio::spawn(async move {
            let mut buf = [0u8; MAX_SIZE as usize];
            let (sz, peer) = server.recv_from(&mut buf).await.unwrap();
            let req = Packet::parse(&buf[..sz]).unwrap();
            assert_eq!(req.header.sequence.get(), 50);
            let resp = [0x03, 0x00, 0x00, 0x32, b'D', b'A', b'T', b'A'];
            server.send_to(&resp, peer).await.unwrap();
        });

        // Pass a buffer that is too small (2 bytes when 4 bytes were received)
        let mut small_buf = [0u8; 2];
        let res = interface.read_exact(&mut small_buf).await;
        assert!(res.is_err());

        Ok(())
    }
}

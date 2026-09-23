// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_sync::{Mutex, MutexGuard};
use std::cmp::min;
use std::future::Future;
use std::iter::FusedIterator;
use std::ops::DerefMut;
use std::pin::Pin;
use std::sync::atomic;
use std::task::{Context, Poll, Waker};

use futures::task::AtomicWaker;
use zerocopy::{Immutable, IntoBytes, KnownLayout, TryFromBytes, Unaligned, little_endian};

use crate::Address;

/// The serializable enumeration of packet types that can be used over a usb vsock link. These
/// roughly correspond to the state machine described by the `fuchsia.hardware.vsock` fidl library.
#[repr(u8)]
#[derive(
    Debug,
    TryFromBytes,
    IntoBytes,
    KnownLayout,
    Immutable,
    Unaligned,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Clone,
    Copy,
)]
pub enum PacketType {
    /// Synchronizes the connection between host and device. Each side must send and receive a
    /// sync packet with the same payload before any other packet may be recognized on the usb
    /// connection. If this packet is received mid-stream, all connections must be considered
    /// reset to avoid data loss. It should also only ever be the last vsock packet in a given
    /// usb packet.
    Sync = b'S',
    /// An outbound echo request. The other end should reply to this with the same body and all the
    /// same fields in a [`PacketType::EchoReply`] packet, no matter the state of the connection.
    Echo = b'E',
    /// A reply to a [`PacketType::Echo`] request packet. The body and all header fields should be
    /// set the same as the original echo packet's.
    EchoReply = b'e',
    /// Connect to a cid:port pair from a cid:port pair on the other side. The payload must be empty.
    Connect = b'C',
    /// Notify the other end that this connection should be closed. The other end should respond
    /// with an [`PacketType::Reset`] when the connection has been closed on the other end. The
    /// payload must be empty.
    Finish = b'F',
    /// Terminate or refuse a connection on a particular cid:port pair set. There must have been a
    /// previous [`PacketType::Connect`] request for this, and after this that particular set of
    /// pairs must be considered disconnected and no more [`PacketType::Data`] packets may be sent
    /// for it unless a new connection is initiated. The payload must be empty.
    Reset = b'R',
    /// Accepts a connection previously requested with [`PacketType::Connect`] on the given cid:port
    /// pair set. The payload must be empty.
    Accept = b'A',
    /// A data packet for a particular cid:port pair set previously established with a [`PacketType::Connect`]
    /// and [`PacketType::Accept`] message. If all of the cid and port fields of the packet are
    /// zero, this is for a special data stream between the host and device that does not require
    /// an established connection.
    Data = b'D',
    /// Advisory flow control message. Payload indicates flow control state "on"
    /// or "off". If "on", it is recommended that the receiver not send more data
    /// for this connection if possible, until the state becomes "off" again.
    /// State is assumed "off" when no packet has been received.
    Pause = b'X',
}

/// The packet header for a vsock packet passed over the usb vsock link. Each usb packet can contain
/// one or more packets, each of which must start with a valid header and correct payload length.
#[repr(C, packed(1))]
#[derive(
    Debug,
    TryFromBytes,
    IntoBytes,
    KnownLayout,
    Immutable,
    Unaligned,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Clone,
)]
pub struct Header {
    magic: [u8; 3],
    /// The type of this packet
    pub packet_type: PacketType,
    /// For Connect, Reset, Accept, and Data packets this represents the device side's address.
    /// Usually this will be a special value representing either that it is simply "the device",
    /// or zero along with the rest of the cid and port fields to indicate that it's a control stream
    /// packet. Must be zero for any other packet type.
    pub device_cid: little_endian::U32,
    /// For Connect, Reset, Accept, and Data packets this represents the host side's address.
    /// Usually this will be a special value representing either that it is simply "the host",
    /// or zero along with the rest of the cid and port fields to indicate that it's a control stream
    /// packet. Must be zero for any other packet type.
    pub host_cid: little_endian::U32,
    /// For Connect, Reset, Accept, and Data packets this represents the device side's port.
    /// This must be a valid positive value for any of those packet types, unless all of the cid and
    /// port fields are also zero, in which case it is a control stream packet. Must be zero for any
    /// other packet type.
    pub device_port: little_endian::U32,
    /// For Connect, Reset, Accept, and Data packets this represents the host side's port.
    /// This must be a valid positive value for any of those packet types, unless all of the cid and
    /// port fields are also zero, in which case it is a control stream packet. Must be zero for any
    /// other packet type.
    pub host_port: little_endian::U32,
    /// The length of the packet payload. This must be zero for any packet type other than Data,
    /// Echo, EchoReply, or Pause.
    pub payload_len: little_endian::U32,
}

impl Header {
    /// Helper constant for the size of a header on the wire
    pub const SIZE: usize = size_of::<Self>();
    const MAGIC: &'static [u8; 3] = b"ffx";

    /// Builds a new packet with correct magic value and packet type and all other fields
    /// initialized to zero.
    pub fn new(packet_type: PacketType) -> Self {
        let device_cid = 0.into();
        let host_cid = 0.into();
        let device_port = 0.into();
        let host_port = 0.into();
        let payload_len = 0.into();
        Header {
            magic: *Self::MAGIC,
            packet_type,
            device_cid,
            host_cid,
            device_port,
            host_port,
            payload_len,
        }
    }

    /// Gets the size of this packet on the wire with the header and a payload of length
    /// [`Self::payload_len`].
    pub fn packet_size(&self) -> usize {
        Packet::size_with_payload(self.payload_len.get() as usize)
    }

    /// Sets the address fields of this packet header based on the normalized address in `addr`.
    pub fn set_address(&mut self, addr: &Address) {
        self.device_cid.set(addr.device_cid);
        self.host_cid.set(addr.host_cid);
        self.device_port.set(addr.device_port);
        self.host_port.set(addr.host_port);
    }
}

/// A typed reference to the contents of a packet in a buffer.
#[derive(Debug, Eq, PartialEq, PartialOrd, Ord)]
pub struct Packet<'a> {
    /// The packet's header
    pub header: &'a Header,
    /// The packet's payload
    pub payload: &'a [u8],
}

impl<'a> Packet<'a> {
    /// The size of this packet according to its header (as [`Self::payload`] may have been
    /// over-allocated for the size of the packet).
    pub fn size(&self) -> usize {
        self.header.packet_size()
    }

    fn size_with_payload(payload_size: usize) -> usize {
        size_of::<Header>() + payload_size
    }

    fn parse_next(buf: &'a [u8]) -> Result<(Self, &'a [u8]), std::io::Error> {
        // split off and validate the header
        let Some((header, body)) = buf.split_at_checked(size_of::<Header>()) else {
            return Err(std::io::Error::other("insufficient data for last packet"));
        };
        let header = Header::try_ref_from_bytes(header).map_err(|err| {
            std::io::Error::other(format!("failed to parse usb vsock header: {err:?}"))
        })?;
        if header.magic != *Header::MAGIC {
            return Err(std::io::Error::other(format!("invalid magic bytes on usb vsock header")));
        }
        // validate the payload length
        let payload_len = Into::<u64>::into(header.payload_len) as usize;
        let body_len = body.len();
        if payload_len > body_len {
            return Err(std::io::Error::other(format!(
                "payload length ({payload_len}) on usb vsock header {header:#?} was larger than available in buffer ({body_len})"
            )));
        }

        let (payload, remain) = body.split_at(payload_len);
        Ok((Packet { header, payload }, remain))
    }

    /// Writes the packet to a buffer when the buffer is known to be large enough to hold it. Note
    /// that the packet header's [`Header::payload_len`] must be correct before calling this, it
    /// does not use the size of [`Self::payload`] to decide how much of the payload buffer is
    /// valid.
    ///
    /// # Panics
    ///
    /// Panics if the buffer is not large enough for the packet.
    pub fn write_to_unchecked(&'a self, buf: &'a mut [u8]) -> &'a mut [u8] {
        let (packet, remain) = buf.split_at_mut(self.size());
        let payload_len = u32::from(self.header.payload_len) as usize;
        self.header.write_to_prefix(packet).unwrap();
        self.payload[..payload_len].write_to_suffix(packet).unwrap();
        remain
    }
}

/// A typed mutable reference to the contents of a packet in a buffer.
#[derive(Debug, Eq, PartialEq, PartialOrd, Ord)]
pub struct PacketMut<'a> {
    /// The packet's header.
    pub header: &'a mut Header,
    /// The packet's payload.
    pub payload: &'a mut [u8],
}

impl<'a> PacketMut<'a> {
    /// Creates a new [`PacketMut`] inside the given buffer and initializes the header to the given
    /// [`PacketType`] before returning it. All other fields in the header will be zeroed, and the
    /// [`PacketMut::payload`] will be the remaining area of the buffer after the header.
    ///
    /// Use [`PacketMut::finish`] to validate and write the proper packet length and return the
    /// total size of the packet.
    ///
    /// # Panics
    ///
    /// The buffer must be large enough to hold at least a packet header, and this will panic if
    /// it's not.
    pub fn new_in(packet_type: PacketType, buf: &'a mut [u8]) -> Self {
        Header::new(packet_type)
            .write_to_prefix(buf)
            .expect("not enough room in buffer for packet header");
        let (header_bytes, payload) = buf.split_at_mut(Header::SIZE);
        let header = Header::try_mut_from_bytes(header_bytes).unwrap();
        PacketMut { header, payload }
    }

    /// Validates the correctness of the packet and returns the size of the packet within the
    /// original buffer.
    pub fn finish(self, payload_len: usize) -> Result<usize, PacketTooBigError> {
        if payload_len <= self.payload.len() {
            self.header.payload_len.set(u32::try_from(payload_len).map_err(|_| PacketTooBigError)?);
            Ok(Header::SIZE + payload_len)
        } else {
            Err(PacketTooBigError)
        }
    }
}

/// Reads a sequence of vsock packets from a given usb packet buffer
pub struct VsockPacketIterator<'a> {
    buf: Option<&'a [u8]>,
}

impl<'a> VsockPacketIterator<'a> {
    /// Creates a new [`PacketStream`] from the contents of `buf`. The returned stream will
    /// iterate over individual vsock packets.
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf: Some(buf) }
    }
}

impl<'a> FusedIterator for VsockPacketIterator<'a> {}
impl<'a> Iterator for VsockPacketIterator<'a> {
    type Item = Result<Packet<'a>, std::io::Error>;

    fn next(&mut self) -> Option<Self::Item> {
        // return immediately if we've already returned `None` or `Some(Err)`
        let data = self.buf.take()?;

        // also return immediately if there's no more data in the buffer
        if data.len() == 0 {
            return None;
        }

        match Packet::parse_next(data) {
            Ok((header, rest)) => {
                // update our pointer for next time
                self.buf = Some(rest);
                Some(Ok(header))
            }
            Err(err) => Some(Err(err)),
        }
    }
}

/// Builds an aggregate usb packet out of vsock packets and gives readiness
/// notifications when there is room to add another packet or data available to send.
pub struct UsbPacketBuilder<B> {
    buffer: B,
    offset: usize,
    space_waker: AtomicWaker,
    packet_waker: AtomicWaker,
}

/// the size of the packet would have been too large even if the buffer was empty
#[derive(Debug, Copy, Clone)]
pub struct PacketTooBigError;

impl<B> UsbPacketBuilder<B> {
    /// Creates a new builder from `buffer`, which is a type that can be used as a mutable slice
    /// with space available for storing vsock packets. The `readable_notify` will have a message
    /// sent to it whenever a usb packet could be transmitted.
    pub fn new(buffer: B) -> Self {
        let offset = 0;
        let space_waker = AtomicWaker::default();
        let packet_waker = AtomicWaker::default();
        Self { buffer, offset, space_waker, packet_waker }
    }

    /// Returns true if the packet has data in it
    pub fn has_data(&self) -> bool {
        self.offset > 0
    }
}

impl<B> UsbPacketBuilder<B>
where
    B: std::ops::DerefMut<Target = [u8]>,
{
    /// Gets the space currently available for another packet in the buffer
    pub fn available(&self) -> usize {
        self.buffer.len() - self.offset
    }

    /// Writes the given packet into the buffer. The packet and header must be able to fit
    /// within the buffer provided at creation time, and `header.payload_length` must be correctly
    /// set.
    pub fn write_vsock_packet(&mut self, packet: &Packet<'_>) -> Result<(), PacketTooBigError> {
        let packet_size = packet.size();
        if self.available() >= packet_size {
            packet.write_to_unchecked(&mut self.buffer[self.offset..]);
            self.offset += packet_size;
            self.packet_waker.wake();
            Ok(())
        } else {
            Err(PacketTooBigError)
        }
    }

    /// Takes the current usb packet, if there is one. The returned mutable slice
    /// will be only the data written to the buffer so far, and packet writing will be reset to the
    /// beginning of the buffer.
    pub fn take_usb_packet(&mut self) -> Option<&mut [u8]> {
        let written = self.offset;
        if written == 0 {
            return None;
        }
        self.offset = 0;
        self.space_waker.wake();
        Some(&mut self.buffer[0..written])
    }
}

/// We have shut down a packet filler and so cannot act on it further.
#[derive(Debug, Copy, Clone)]
pub struct ShutdownError;

impl From<ShutdownError> for std::io::Error {
    fn from(_: ShutdownError) -> Self {
        std::io::Error::new(std::io::ErrorKind::BrokenPipe, "Transport shut down")
    }
}

/// Errors returned from [`UsbPacketFiller::write_vsock_packet`].
#[derive(Debug, Copy, Clone)]
pub(crate) enum WritePacketError {
    /// See [`PacketTooBigError`]
    PacketTooBig(PacketTooBigError),
    /// See [`ShutdownError`]
    Shutdown(ShutdownError),
}

impl From<PacketTooBigError> for WritePacketError {
    fn from(value: PacketTooBigError) -> Self {
        WritePacketError::PacketTooBig(value)
    }
}

impl From<ShutdownError> for WritePacketError {
    fn from(value: ShutdownError) -> Self {
        WritePacketError::Shutdown(value)
    }
}

/// Extension for [`WritePacketError`] to allow asserting [`PacketTooBigError`]
/// while extracting [`ShutdownError`].
pub(crate) trait WritePacketErrorExt {
    /// The type we return when we've extracted and handled any [`PacketTooBigError`]
    type Residue;

    /// Assert that this type is not a [`PacketTooBigError`] and map it to a new
    /// type that cannot represent `PacketTooBigError`.
    fn assert_right_size(self) -> Self::Residue;

    /// Same as `assert_right_size` but allow a custom error message if we find
    /// a [`PacketTooBigError`].
    fn expect_right_size(self, err: &str) -> Self::Residue;
}

impl WritePacketErrorExt for WritePacketError {
    type Residue = ShutdownError;

    fn assert_right_size(self) -> ShutdownError {
        self.expect_right_size("Unexpected PacketTooBigError in WritePacketError")
    }

    fn expect_right_size(self, err: &str) -> ShutdownError {
        return match self {
            WritePacketError::PacketTooBig(_) => panic!("{err}"),
            WritePacketError::Shutdown(shutdown_error) => shutdown_error,
        };
    }
}

impl<T> WritePacketErrorExt for Result<T, WritePacketError> {
    type Residue = Result<T, ShutdownError>;

    fn assert_right_size(self) -> Result<T, ShutdownError> {
        match self {
            Ok(x) => Ok(Ok(x)),
            Err(WritePacketError::Shutdown(s)) => Ok(Err(s)),
            Err(WritePacketError::PacketTooBig(p)) => Err(p),
        }
        .unwrap()
    }

    fn expect_right_size(self, err: &str) -> Result<T, ShutdownError> {
        match self {
            Ok(x) => Ok(Ok(x)),
            Err(WritePacketError::Shutdown(s)) => Ok(Err(s)),
            Err(WritePacketError::PacketTooBig(p)) => Err(p),
        }
        .expect(err)
    }
}

pub(crate) struct UsbPacketFiller<B> {
    current_out_packet: Mutex<Option<UsbPacketBuilder<B>>>,
    out_packet_wakers: Mutex<Vec<Waker>>,
    filled_packet_waker: AtomicWaker,
    is_shutdown: atomic::AtomicBool,
}

impl<B> Default for UsbPacketFiller<B> {
    fn default() -> Self {
        let current_out_packet = Mutex::default();
        let out_packet_wakers = Mutex::default();
        let filled_packet_waker = AtomicWaker::default();
        let is_shutdown = atomic::AtomicBool::new(false);
        Self { current_out_packet, out_packet_wakers, filled_packet_waker, is_shutdown }
    }
}

impl<B> UsbPacketFiller<B> {
    fn is_shutdown(&self) -> bool {
        self.is_shutdown.load(atomic::Ordering::Acquire)
    }
}

impl<B: DerefMut<Target = [u8]> + Unpin> UsbPacketFiller<B> {
    fn wait_for_fillable(&self, min_packet_size: usize) -> WaitForFillable<'_, B> {
        WaitForFillable { filler: &self, min_packet_size }
    }

    pub fn shutdown(&self) {
        let mut out_packets = self.current_out_packet.lock();
        let mut out_packet_wakers = self.out_packet_wakers.lock();
        self.is_shutdown.store(true, atomic::Ordering::Release);
        *out_packets = None;
        out_packet_wakers.drain(..).for_each(Waker::wake);
        self.filled_packet_waker.wake();
    }

    pub async fn write_vsock_packet(&self, packet: &Packet<'_>) -> Result<(), WritePacketError> {
        let mut builder = self.wait_for_fillable(packet.size()).await?;
        builder.as_mut().unwrap().write_vsock_packet(packet)?;
        self.filled_packet_waker.wake();
        Ok(())
    }

    pub async fn write_vsock_data(
        &self,
        address: &Address,
        payload: &[u8],
    ) -> Result<usize, ShutdownError> {
        let header = &mut Header::new(PacketType::Data);
        header.set_address(&address);
        let mut builder = self.wait_for_fillable(Header::SIZE + 1).await?;
        let builder = builder.as_mut().unwrap();
        let writing = min(payload.len(), builder.available() - Header::SIZE);
        header.payload_len.set(writing as u32);
        builder.write_vsock_packet(&Packet { header, payload: &payload[..writing] }).unwrap();
        self.filled_packet_waker.wake();
        Ok(writing)
    }

    pub async fn write_vsock_data_all(
        &self,
        address: &Address,
        payload: &[u8],
    ) -> Result<(), ShutdownError> {
        let mut written = 0;
        while written < payload.len() {
            written += self.write_vsock_data(address, &payload[written..]).await?;
        }
        Ok(())
    }

    /// Provides a packet builder for the state machine to write packets to. Returns a future that
    /// will be fulfilled when there is data available to send on the packet.
    ///
    /// # Panics
    ///
    /// Panics if called while another [`Self::fill_usb_packet`] future is pending.
    pub fn fill_usb_packet(&self, builder: UsbPacketBuilder<B>) -> FillUsbPacket<'_, B> {
        FillUsbPacket(&self, Some(builder))
    }
}

pub(crate) struct FillUsbPacket<'a, B>(&'a UsbPacketFiller<B>, Option<UsbPacketBuilder<B>>);

impl<'a, B: Unpin> Future for FillUsbPacket<'a, B> {
    type Output = Result<UsbPacketBuilder<B>, ShutdownError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // if we're holding a `PacketBuilder` we haven't been waited on yet. Otherwise we want
        // to return ready when there's a packet and it's got data in it.
        if let Some(builder) = self.1.take() {
            // if the packet we were handed for some reason already has data in it, hand it back
            if builder.has_data() {
                return Poll::Ready(Ok(builder));
            }

            let mut current_out_packet = self.0.current_out_packet.lock();
            if self.0.is_shutdown() {
                return Poll::Ready(Err(ShutdownError));
            }
            assert!(current_out_packet.is_none(), "Can't fill more than one packet at a time");
            current_out_packet.replace(builder);

            // before we drop the lock, register our interest in packets being filled in.
            self.0.filled_packet_waker.register(cx.waker());
            // drop the lock so that we aren't immediately stalling any tasks we wake below.
            drop(current_out_packet);

            // wake all pending vsock packet writes so that we can fill the buffer as quickly as
            // possible.
            let mut wakers = self.0.out_packet_wakers.lock();
            for waker in wakers.drain(..) {
                waker.wake();
            }
            if self.0.is_shutdown() { Poll::Ready(Err(ShutdownError)) } else { Poll::Pending }
        } else {
            let mut current_out_packet = self.0.current_out_packet.lock();
            if self.0.is_shutdown() {
                return Poll::Ready(Err(ShutdownError));
            }
            let Some(builder) = current_out_packet.take() else {
                panic!("Packet builder was somehow removed from connection prematurely");
            };

            if builder.has_data() {
                self.0.filled_packet_waker.wake();
                Poll::Ready(Ok(builder))
            } else {
                // if there hasn't been any data placed in the packet, put the builder back and
                // return Pending.
                current_out_packet.replace(builder);
                Poll::Pending
            }
        }
    }
}

pub(crate) struct WaitForFillable<'a, B> {
    filler: &'a UsbPacketFiller<B>,
    min_packet_size: usize,
}

impl<'a, B: DerefMut<Target = [u8]> + Unpin> Future for WaitForFillable<'a, B> {
    type Output = Result<MutexGuard<'a, Option<UsbPacketBuilder<B>>>, ShutdownError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let current_out_packet = self.filler.current_out_packet.lock();
        if self.filler.is_shutdown() {
            return Poll::Ready(Err(ShutdownError));
        }
        let Some(builder) = &*current_out_packet else {
            self.filler.out_packet_wakers.lock().push(cx.waker().clone());
            return Poll::Pending;
        };
        if builder.available() >= self.min_packet_size {
            Poll::Ready(Ok(current_out_packet))
        } else {
            self.filler.out_packet_wakers.lock().push(cx.waker().clone());
            Poll::Pending
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use fuchsia_async::Task;
    use futures::poll;

    async fn assert_pending<F: Future>(fut: F) {
        let fut = std::pin::pin!(fut);
        if let Poll::Ready(_) = poll!(fut) {
            panic!("Future was ready when it shouldn't have been");
        }
    }
    #[test]
    fn packet_write_doesnt_clobber_header_with_incorrect_len() {
        let header = &mut Header::new(PacketType::Pause);
        let payload = &[1];
        let packet = Packet { header, payload };
        let mut buf = vec![0; packet.size()];
        packet.write_to_unchecked(&mut buf);

        let read_packet = Packet::parse_next(&buf).unwrap().0;
        assert_eq!(&read_packet.header, &packet.header);
    }

    #[fuchsia::test]
    async fn roundtrip_packet() {
        let payload = b"hello world!";
        let packet = Packet {
            payload,
            header: &Header {
                device_cid: 1.into(),
                host_cid: 2.into(),
                device_port: 3.into(),
                host_port: 4.into(),
                payload_len: little_endian::U32::from(payload.len() as u32),
                ..Header::new(PacketType::Data)
            },
        };
        let buffer = vec![0; packet.size()];
        let builder = UsbPacketBuilder::new(buffer);
        let filler = UsbPacketFiller::default();
        let mut filled_fut = filler.fill_usb_packet(builder);
        println!("we should not be ready to pull a usb packet off yet");
        assert_pending(&mut filled_fut).await;

        println!("we should be able to write a packet though ({} bytes)", packet.size());
        filler.write_vsock_packet(&packet).await.unwrap();

        println!("we shouldn't have any space for another packet now");
        assert_pending(filler.wait_for_fillable(1)).await;

        println!("but we should have a new usb packet available");
        let mut builder = filled_fut.await.unwrap();
        let buffer = builder.take_usb_packet().unwrap();

        println!("the packet we get back out should be the same one we put in");
        let (read_packet, remain) = Packet::parse_next(buffer).unwrap();
        assert_eq!(packet, read_packet);
        assert!(remain.is_empty());
    }

    #[fuchsia::test]
    async fn many_packets() {
        fn make_numbered_packet(num: u32) -> (Header, String) {
            let payload = format!("packet #{num}!");
            let header = Header {
                device_cid: num.into(),
                device_port: num.into(),
                host_cid: num.into(),
                host_port: num.into(),
                payload_len: little_endian::U32::from(payload.len() as u32),
                ..Header::new(PacketType::Data)
            };
            (header, payload)
        }
        const BUFFER_SIZE: usize = 256;
        let mut builder = UsbPacketBuilder::new(vec![0; BUFFER_SIZE]);
        let filler = Arc::new(UsbPacketFiller::default());

        let send_filler = filler.clone();
        let send_task = Task::spawn(async move {
            for packet_num in 0..1024 {
                let next_packet = make_numbered_packet(packet_num);
                let next_packet =
                    Packet { header: &next_packet.0, payload: next_packet.1.as_ref() };
                send_filler.write_vsock_packet(&next_packet).await.unwrap();
            }
        });

        let mut read_packet_num = 0;
        while read_packet_num < 1024 {
            builder = filler.fill_usb_packet(builder).await.unwrap();
            let buffer = builder.take_usb_packet().unwrap();
            let mut num_packets = 0;
            for packet in VsockPacketIterator::new(&buffer) {
                let packet_compare = make_numbered_packet(read_packet_num);
                let packet_compare =
                    Packet { header: &packet_compare.0, payload: &packet_compare.1.as_ref() };
                assert_eq!(packet.unwrap(), packet_compare);
                read_packet_num += 1;
                num_packets += 1;
            }
            println!(
                "Read {num_packets} vsock packets from usb packet buffer, had {count} bytes left",
                count = BUFFER_SIZE - buffer.len()
            );
        }
        send_task.await;
        assert_eq!(1024, read_packet_num);
    }

    #[fuchsia::test]
    async fn packet_fillable_futures() {
        let filler = UsbPacketFiller::default();

        for _ in 0..10 {
            println!("register an interest in filling a usb packet");
            let mut fillable_fut = filler.wait_for_fillable(1);
            println!("make sure we have nothing to fill");
            assert!(poll!(&mut fillable_fut).is_pending());

            println!("register a packet for filling");
            let mut filled_fut = filler.fill_usb_packet(UsbPacketBuilder::new(vec![0; 1024]));
            println!("make sure we've registered the buffer");
            assert!(poll!(&mut filled_fut).is_pending());

            println!("now put some things in the packet");
            let header = &mut Header::new(PacketType::Data);
            header.payload_len.set(99);
            {
                let Poll::Ready(Ok(mut builder)) = poll!(fillable_fut) else {
                    panic!("should have been ready to fill a packet")
                };
                builder
                    .as_mut()
                    .unwrap()
                    .write_vsock_packet(&Packet { header, payload: &[b'a'; 99] })
                    .unwrap();
            }
            {
                let Poll::Ready(Ok(mut builder)) = poll!(filler.wait_for_fillable(1)) else {
                    panic!("should have been ready to fill a packet(2)")
                };
                builder
                    .as_mut()
                    .unwrap()
                    .write_vsock_packet(&Packet { header, payload: &[b'a'; 99] })
                    .unwrap();
            }

            println!("but if we ask for too much space we'll get pending");
            assert!(poll!(filler.wait_for_fillable(1024 - (99 * 2) + 1)).is_pending());

            println!("and now resolve the filled future and get our data back");
            let mut filled = filled_fut.await.unwrap();
            let packets =
                Vec::from_iter(VsockPacketIterator::new(filled.take_usb_packet().unwrap()));
            assert_eq!(packets.len(), 2);
        }
    }
}
